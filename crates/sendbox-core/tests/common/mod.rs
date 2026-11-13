//! Integration-test harness: start a core on ephemeral ports with a temporary
//! data directory, send Emails with `lettre`, or drive raw SMTP sessions.

#![allow(dead_code)] // each test binary uses a different subset of the harness

use std::ops::Deref;
use std::time::Duration;

use sendbox_core::{Config, CoreHandle};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;

/// How long a test waits for any single network step before failing.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(5);

/// A core running on an ephemeral SMTP port with its own temporary data directory.
/// Derefs to its [`CoreHandle`], so tests call the handle directly.
pub struct TestCore {
    pub handle: CoreHandle,
    pub data_dir: TempDir,
}

impl TestCore {
    pub async fn start() -> Self {
        Self::start_with(0, TempDir::new().expect("create temp data dir")).await
    }

    pub async fn start_with(smtp_port: u16, data_dir: TempDir) -> Self {
        let handle = sendbox_core::start(Config {
            smtp_port,
            data_dir: data_dir.path().to_path_buf(),
        })
        .await
        .expect("core starts");
        Self { handle, data_dir }
    }

    /// Stops the core and starts a fresh one on the same data directory.
    pub async fn restart(self) -> Self {
        let Self { handle, data_dir } = self;
        handle.shutdown().await;
        Self::start_with(0, data_dir).await
    }
}

impl Deref for TestCore {
    type Target = CoreHandle;

    fn deref(&self) -> &CoreHandle {
        &self.handle
    }
}

/// Sends `message` to the core over SMTP with `lettre`, as a developer's app would.
pub async fn send_with_lettre(port: u16, message: lettre::Message) {
    use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

    let transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous("127.0.0.1")
        .port(port)
        .build();
    timeout(STEP_TIMEOUT, transport.send(message))
        .await
        .expect("lettre send timed out")
        .expect("lettre send succeeds");
}

/// One SMTP reply (all lines of a multi-line reply share its code).
#[derive(Debug)]
pub struct Reply {
    pub code: u16,
}

/// A raw SMTP client for protocol-level tests.
pub struct SmtpClient {
    stream: BufReader<TcpStream>,
}

impl SmtpClient {
    /// Connects and returns the client with the server greeting.
    pub async fn connect(port: u16) -> (Self, Reply) {
        let stream = timeout(STEP_TIMEOUT, TcpStream::connect(("127.0.0.1", port)))
            .await
            .expect("connect timed out")
            .expect("connect to core");
        let mut client = Self {
            stream: BufReader::new(stream),
        };
        let greeting = client.read_reply().await;
        (client, greeting)
    }

    /// Connects and says EHLO, ready for a mail transaction.
    pub async fn after_ehlo(port: u16) -> Self {
        let (mut client, greeting) = Self::connect(port).await;
        assert_eq!(greeting.code, 220);
        assert_eq!(client.command("EHLO client.test").await.code, 250);
        client
    }

    /// Sends one command line (CRLF appended) and returns the reply.
    pub async fn command(&mut self, line: &str) -> Reply {
        self.write(format!("{line}\r\n").as_bytes()).await;
        self.read_reply().await
    }

    /// Writes raw bytes without reading a reply.
    pub async fn write(&mut self, bytes: &[u8]) {
        let stream = self.stream.get_mut();
        stream.write_all(bytes).await.expect("write to core");
        stream.flush().await.expect("flush to core");
    }

    pub async fn read_reply(&mut self) -> Reply {
        loop {
            let mut line = String::new();
            let read = timeout(STEP_TIMEOUT, self.stream.read_line(&mut line))
                .await
                .expect("reply timed out")
                .expect("read reply");
            assert!(read > 0, "connection closed while waiting for a reply");
            let line = line.trim_end_matches(['\r', '\n']);
            let code = line[..3].parse().expect("reply starts with a code");
            if line.as_bytes().get(3) != Some(&b'-') {
                return Reply { code };
            }
        }
    }

    /// Returns true once the server has closed the connection.
    pub async fn is_closed(&mut self) -> bool {
        let mut rest = String::new();
        matches!(
            timeout(STEP_TIMEOUT, self.stream.read_line(&mut rest)).await,
            Ok(Ok(0))
        )
    }

    /// Runs a full transaction (MAIL, RCPT…, DATA) with `data` sent as-is,
    /// which must already be dot-stuffed and end with the `\r\n.\r\n` terminator.
    pub async fn send_transaction(&mut self, from: &str, to: &[&str], data: &[u8]) -> Reply {
        assert_eq!(self.command(&format!("MAIL FROM:<{from}>")).await.code, 250);
        for rcpt in to {
            assert_eq!(self.command(&format!("RCPT TO:<{rcpt}>")).await.code, 250);
        }
        assert_eq!(self.command("DATA").await.code, 354);
        self.write(data).await;
        self.read_reply().await
    }
}
