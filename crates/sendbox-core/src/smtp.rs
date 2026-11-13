//! The SMTP listener: our own tokio accept loop, one task per session, each
//! driving `smtp-proto`'s sans-IO parser. Emails are Captured, never relayed.

use std::io;
use std::net::Ipv4Addr;

use smtp_proto::Request;
use smtp_proto::request::receiver::{DataReceiver, RequestReceiver};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::{JoinHandle, JoinSet};

use crate::inbox::{Envelope, Inbox};

const HOSTNAME: &str = "sendbox";
const READ_BUFFER_SIZE: usize = 8 * 1024;

pub(crate) struct Listener {
    port: u16,
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl Listener {
    /// Binds `127.0.0.1:port` (loopback only) and starts accepting sessions.
    pub(crate) async fn bind(port: u16, inbox: Inbox) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
        let port = listener.local_addr()?.port();
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(accept_loop(listener, inbox, stopped));
        Ok(Self { port, stop, task })
    }

    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    pub(crate) async fn shutdown(self) {
        let _ = self.stop.send(());
        let _ = self.task.await;
    }
}

async fn accept_loop(listener: TcpListener, inbox: Inbox, mut stopped: oneshot::Receiver<()>) {
    let mut sessions = JoinSet::new();
    loop {
        tokio::select! {
            _ = &mut stopped => break,
            accepted = listener.accept() => {
                // Accept errors (e.g. a connection reset before accept) only
                // concern that one connection: keep listening.
                if let Ok((stream, _)) = accepted {
                    sessions.spawn(Session::new(stream, inbox.clone()).run());
                }
            }
            Some(_) = sessions.join_next(), if !sessions.is_empty() => {}
        }
    }
    sessions.shutdown().await;
}

/// Receiving the content of a DATA command.
struct IncomingData {
    receiver: DataReceiver,
    content: Vec<u8>,
}

impl IncomingData {
    fn new() -> Self {
        let mut receiver = DataReceiver::new();
        // The receiver only recognises a dot (stuffed, or the terminator) after
        // a CRLF; the DATA command line just ended with one, so say so.
        receiver.ingest(&mut b"\r\n".iter(), &mut Vec::new());
        Self {
            receiver,
            content: Vec::new(),
        }
    }
}

enum Next {
    Continue,
    Close,
}

struct Session {
    stream: TcpStream,
    inbox: Inbox,
    requests: RequestReceiver,
    greeted: bool,
    /// The mail transaction in progress: its envelope so far, from MAIL on.
    transaction: Option<Envelope>,
    data: Option<IncomingData>,
    replies: Vec<u8>,
}

impl Session {
    fn new(stream: TcpStream, inbox: Inbox) -> Self {
        Self {
            stream,
            inbox,
            requests: RequestReceiver::default(),
            greeted: false,
            transaction: None,
            data: None,
            replies: Vec::new(),
        }
    }

    async fn run(mut self) {
        // An I/O error ends the session; there is nobody left to tell.
        let _ = self.serve().await;
    }

    async fn serve(&mut self) -> io::Result<()> {
        self.reply(220, &format!("{HOSTNAME} ESMTP Sendbox"));
        self.flush().await?;
        let mut buffer = vec![0; READ_BUFFER_SIZE];
        loop {
            let read = self.stream.read(&mut buffer).await?;
            if read == 0 {
                return Ok(());
            }
            let next = self.ingest(&buffer[..read]).await;
            self.flush().await?;
            if let Next::Close = next {
                return Ok(());
            }
        }
    }

    /// Processes every complete command (and DATA content) in `bytes`,
    /// queueing replies.
    async fn ingest(&mut self, bytes: &[u8]) -> Next {
        let mut bytes = bytes.iter();
        while bytes.len() > 0 {
            if let Some(data) = &mut self.data {
                if !data.receiver.ingest(&mut bytes, &mut data.content) {
                    break;
                }
                let content = std::mem::take(&mut data.content);
                self.data = None;
                self.capture(content).await;
                continue;
            }
            let request = match self.requests.ingest(&mut bytes) {
                Ok(request) => request.into_owned(),
                Err(smtp_proto::Error::NeedsMoreData { .. }) => break,
                Err(error) => {
                    self.reply_to_invalid(error);
                    continue;
                }
            };
            if let Next::Close = self.handle(request) {
                return Next::Close;
            }
        }
        Next::Continue
    }

    fn handle(&mut self, request: Request<String>) -> Next {
        match request {
            Request::Ehlo { .. } | Request::Helo { .. } => {
                self.greeted = true;
                self.transaction = None;
                self.reply(250, HOSTNAME);
            }
            Request::Mail { from } => {
                if !self.greeted {
                    self.reply(503, "Send EHLO or HELO first");
                } else if self.transaction.is_some() {
                    self.reply(503, "Sender already given");
                } else {
                    self.transaction = Some(Envelope {
                        sender: from.address,
                        recipients: Vec::new(),
                    });
                    self.reply(250, "Sender OK");
                }
            }
            Request::Rcpt { to } => match &mut self.transaction {
                None => self.reply(503, "Send MAIL first"),
                Some(transaction) => {
                    transaction.recipients.push(to.address);
                    self.reply(250, "Recipient OK");
                }
            },
            Request::Data => match &self.transaction {
                Some(transaction) if !transaction.recipients.is_empty() => {
                    self.data = Some(IncomingData::new());
                    self.reply(354, "End data with <CR><LF>.<CR><LF>");
                }
                Some(_) => self.reply(503, "Send RCPT first"),
                None => self.reply(503, "Send MAIL first"),
            },
            Request::Noop { .. } => self.reply(250, "OK"),
            Request::Rset => {
                self.transaction = None;
                self.reply(250, "Reset");
            }
            Request::Quit => {
                self.reply(221, "Bye");
                return Next::Close;
            }
            _ => self.reply(502, "Command not implemented"),
        }
        Next::Continue
    }

    async fn capture(&mut self, raw_source: Vec<u8>) {
        let Some(envelope) = self.transaction.take() else {
            return;
        };
        match self.inbox.capture(envelope, raw_source).await {
            Ok(_) => self.reply(250, "Captured"),
            Err(_) => self.reply(451, "Could not store the Email, try again"),
        }
    }

    fn reply_to_invalid(&mut self, error: smtp_proto::Error) {
        match error {
            smtp_proto::Error::UnknownCommand => self.reply(500, "Unknown command"),
            smtp_proto::Error::ResponseTooLong => self.reply(500, "Line too long"),
            error => self.reply(501, &error.to_string()),
        }
    }

    fn reply(&mut self, code: u16, text: &str) {
        self.replies
            .extend_from_slice(format!("{code} {text}\r\n").as_bytes());
    }

    async fn flush(&mut self) -> io::Result<()> {
        if !self.replies.is_empty() {
            self.stream.write_all(&self.replies).await?;
            self.replies.clear();
        }
        Ok(())
    }
}
