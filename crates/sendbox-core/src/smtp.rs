//! The SMTP listener: our own tokio accept loop, one task per session, each
//! driving `smtp-proto`'s sans-IO parser. Emails are Captured, never relayed.

use std::io;
use std::slice::Iter;

use smtp_proto::request::receiver::{
    DataReceiver, DummyDataReceiver, LineReceiver, RequestReceiver,
};
use smtp_proto::{AUTH_LOGIN, AUTH_PLAIN, Request};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::inbox::{Envelope, Inbox};
use crate::listener::Serve;

const HOSTNAME: &str = "sendbox";
/// The largest Email accepted, in bytes (25 MiB), advertised in EHLO as SIZE.
const MAX_EMAIL_SIZE: usize = 25 * 1024 * 1024;
const READ_BUFFER_SIZE: usize = 8 * 1024;

/// The SMTP listener: accepts sessions on its port, each Capturing into
/// `inbox`.
pub(crate) fn server(inbox: Inbox) -> Serve {
    Box::new(move |listener, stop| Box::pin(accept_loop(listener, inbox.clone(), stop)))
}

async fn accept_loop(listener: TcpListener, inbox: Inbox, stop: CancellationToken) {
    let mut sessions = JoinSet::new();
    loop {
        tokio::select! {
            _ = stop.cancelled() => break,
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

/// What the session expects the client's next bytes to be.
enum Input {
    Commands,
    /// The client's answer to an AUTH challenge, on a line of its own.
    AuthResponse {
        step: AuthStep,
        line: LineReceiver<()>,
    },
    /// The content of a DATA command, up to the lone dot ending it.
    Data(IncomingData),
}

/// Which challenge of an AUTH exchange the client is answering.
#[derive(Clone, Copy)]
enum AuthStep {
    PlainCredentials,
    LoginUsername,
    LoginPassword,
}

impl AuthStep {
    /// The 334 challenge asking for this step's answer.
    fn challenge(self) -> &'static str {
        match self {
            Self::PlainCredentials => "",
            Self::LoginUsername => "VXNlcm5hbWU6", // base64 "Username:"
            Self::LoginPassword => "UGFzc3dvcmQ6", // base64 "Password:"
        }
    }

    /// The step after this one, or `None` once the credentials are complete.
    fn next(self) -> Option<Self> {
        match self {
            Self::LoginUsername => Some(Self::LoginPassword),
            Self::PlainCredentials | Self::LoginPassword => None,
        }
    }
}

/// Receiving the content of a DATA command.
enum IncomingData {
    /// Kept as it arrives while within the size limit.
    WithinLimit {
        receiver: DataReceiver,
        content: Vec<u8>,
    },
    /// Over the size limit: the rest is read to its end and dropped, so the
    /// session can go on.
    OverLimit(DummyDataReceiver),
}

/// How a DATA content ended.
enum DataEnd {
    Complete(Vec<u8>),
    OverLimit,
}

impl IncomingData {
    fn new() -> Self {
        let mut receiver = DataReceiver::new();
        // The receiver only recognises a dot (stuffed, or the terminator) after
        // a CRLF; the DATA command line just ended with one, so say so.
        receiver.ingest(&mut b"\r\n".iter(), &mut Vec::new());
        Self::WithinLimit {
            receiver,
            content: Vec::new(),
        }
    }

    /// Takes in the content found in `bytes`, returning how it ended once its
    /// terminating dot has arrived.
    fn ingest(&mut self, bytes: &mut Iter<'_, u8>) -> Option<DataEnd> {
        match self {
            Self::WithinLimit { receiver, content } => {
                let complete = receiver.ingest(bytes, content);
                if content.len() > MAX_EMAIL_SIZE {
                    if complete {
                        return Some(DataEnd::OverLimit);
                    }
                    *self = Self::OverLimit(DummyDataReceiver::new_data(receiver));
                    None
                } else {
                    complete.then(|| DataEnd::Complete(std::mem::take(content)))
                }
            }
            Self::OverLimit(rest) => rest.ingest(bytes).then_some(DataEnd::OverLimit),
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
    input: Input,
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
            input: Input::Commands,
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

    /// Processes every complete command, AUTH response and DATA content in
    /// `bytes`, queueing replies.
    async fn ingest(&mut self, bytes: &[u8]) -> Next {
        let mut bytes = bytes.iter();
        while bytes.len() > 0 {
            match &mut self.input {
                Input::Commands => {
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
                Input::AuthResponse { step, line } => {
                    if !line.ingest(&mut bytes) {
                        break;
                    }
                    let (step, response) = (*step, std::mem::take(&mut line.buf));
                    self.continue_auth(step, &response);
                }
                Input::Data(data) => match data.ingest(&mut bytes) {
                    None => break,
                    Some(DataEnd::Complete(content)) => {
                        self.input = Input::Commands;
                        self.capture(content).await;
                    }
                    Some(DataEnd::OverLimit) => self.reject_too_large(),
                },
            }
        }
        Next::Continue
    }

    fn handle(&mut self, request: Request<String>) -> Next {
        match request {
            Request::Ehlo { .. } => {
                self.start_over();
                // Never STARTTLS: nodemailer treats a failed upgrade as fatal.
                // Never PIPELINING: clients then wait for each reply.
                let size = format!("SIZE {MAX_EMAIL_SIZE}");
                let extensions = ["AUTH PLAIN LOGIN", "8BITMIME", "SMTPUTF8", &size];
                self.reply_lines(250, &[&[HOSTNAME][..], &extensions].concat());
            }
            Request::Helo { .. } => {
                self.start_over();
                self.reply(250, HOSTNAME);
            }
            Request::Mail { from } => {
                if !self.greeted {
                    self.reply(503, "Send EHLO or HELO first");
                } else if self.transaction.is_some() {
                    self.reply(503, "Sender already given");
                } else if from.size > MAX_EMAIL_SIZE {
                    self.reject_too_large();
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
                    self.input = Input::Data(IncomingData::new());
                    self.reply(354, "End data with <CR><LF>.<CR><LF>");
                }
                Some(_) => self.reply(503, "Send RCPT first"),
                None => self.reply(503, "Send MAIL first"),
            },
            Request::Auth {
                mechanism,
                initial_response,
            } => self.start_auth(mechanism, &initial_response),
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

    /// Sendbox accepts any credentials, never even decoding them: it only
    /// Captures, locally. They are read only to keep clients happy.
    fn start_auth(&mut self, mechanism: u64, initial_response: &str) {
        match (mechanism, initial_response.is_empty()) {
            (AUTH_PLAIN, false) => self.reply(235, "Authenticated"),
            (AUTH_PLAIN, true) => self.challenge(AuthStep::PlainCredentials),
            (AUTH_LOGIN, true) => self.challenge(AuthStep::LoginUsername),
            (AUTH_LOGIN, false) => self.challenge(AuthStep::LoginPassword),
            _ => self.reply(504, "Unrecognized authentication mechanism"),
        }
    }

    fn challenge(&mut self, step: AuthStep) {
        self.input = Input::AuthResponse {
            step,
            line: LineReceiver::new(()),
        };
        self.reply(334, step.challenge());
    }

    /// The client answered the challenge for `step` with `response`.
    fn continue_auth(&mut self, step: AuthStep, response: &[u8]) {
        self.input = Input::Commands;
        match step.next() {
            _ if response == b"*" => self.reply(501, "Authentication cancelled"),
            Some(next) => self.challenge(next),
            None => self.reply(235, "Authenticated"),
        }
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

    /// Refuses an Email over the size limit, declared at MAIL or found in
    /// DATA, abandoning its transaction: nothing is Captured.
    fn reject_too_large(&mut self) {
        self.input = Input::Commands;
        self.transaction = None;
        self.reply(
            552,
            &format!("Email too large: the limit is {MAX_EMAIL_SIZE} bytes"),
        );
    }

    fn reply_to_invalid(&mut self, error: smtp_proto::Error) {
        match error {
            smtp_proto::Error::UnknownCommand => self.reply(500, "Unknown command"),
            smtp_proto::Error::ResponseTooLong => self.reply(500, "Line too long"),
            error => self.reply(501, &error.to_string()),
        }
    }

    /// Starts the session over after EHLO or HELO.
    fn start_over(&mut self) {
        self.greeted = true;
        self.transaction = None;
    }

    fn reply(&mut self, code: u16, text: &str) {
        self.reply_lines(code, &[text]);
    }

    /// Queues a multi-line reply: every line but the last is marked with `-`.
    fn reply_lines(&mut self, code: u16, lines: &[&str]) {
        for (index, text) in lines.iter().enumerate() {
            let separator = if index + 1 == lines.len() { ' ' } else { '-' };
            self.replies
                .extend_from_slice(format!("{code}{separator}{text}\r\n").as_bytes());
        }
    }

    async fn flush(&mut self) -> io::Result<()> {
        if !self.replies.is_empty() {
            self.stream.write_all(&self.replies).await?;
            self.replies.clear();
        }
        Ok(())
    }
}
