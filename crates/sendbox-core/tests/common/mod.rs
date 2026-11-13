//! Integration-test harness: start a core on ephemeral ports with a temporary
//! data directory, send Emails with `lettre`, drive raw SMTP sessions, or
//! connect an agent over MCP.

#![allow(dead_code)] // each test binary uses a different subset of the harness

use std::ops::Deref;
use std::time::Duration;

use lettre::transport::smtp::AsyncSmtpTransportBuilder;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult, ClientConfig};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::StreamableHttpClientTransport;
use sendbox_core::{Config, CoreHandle, EmailFilter, EmailSummary};
use serde_json::Value;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;

/// How long a test waits for any single network step before failing.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(5);

/// A core running on ephemeral SMTP and MCP ports with its own temporary data
/// directory. Derefs to its [`CoreHandle`], so tests call the handle directly.
pub struct TestCore {
    pub handle: CoreHandle,
    pub data_dir: TempDir,
}

impl TestCore {
    pub async fn start() -> Self {
        Self::start_with(0, TempDir::new().expect("create temp data dir")).await
    }

    pub async fn start_with(smtp_port: u16, data_dir: TempDir) -> Self {
        let config = Config {
            smtp_port,
            ..test_config(&data_dir)
        };
        Self::start_with_config(config, data_dir).await
    }

    /// Starts a core with `config`, which must keep its Inbox in `data_dir`.
    pub async fn start_with_config(config: Config, data_dir: TempDir) -> Self {
        let handle = sendbox_core::start(config).await.expect("core starts");
        Self { handle, data_dir }
    }

    /// Stops the core and starts a fresh one on the same data directory.
    pub async fn restart(self) -> Self {
        let Self { handle, data_dir } = self;
        handle.shutdown().await;
        Self::start_with(0, data_dir).await
    }

    /// Every Email in the Inbox, newest first.
    pub async fn inbox(&self) -> Vec<EmailSummary> {
        self.list_emails(&EmailFilter::default(), None)
            .await
            .expect("the Inbox lists")
    }

    /// Connects an agent to this core's MCP endpoint.
    pub async fn agent(&self) -> Agent {
        Agent::connect(self.mcp_port()).await
    }
}

/// The default config, with ephemeral ports and the Inbox in `data_dir`.
pub fn test_config(data_dir: &TempDir) -> Config {
    Config {
        smtp_port: 0,
        mcp_port: 0,
        ..Config::new(data_dir.path())
    }
}

impl Deref for TestCore {
    type Target = CoreHandle;

    fn deref(&self) -> &CoreHandle {
        &self.handle
    }
}

/// A `lettre` transport to the core configured as a developer's app would:
/// plaintext, no TLS. Tests add credentials or mechanisms before sending.
pub fn lettre_transport(port: u16) -> AsyncSmtpTransportBuilder {
    AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous("127.0.0.1").port(port)
}

/// Sends `message` to the core over SMTP with `lettre`, as a developer's app would.
pub async fn send_with_lettre(port: u16, message: lettre::Message) {
    send_through(lettre_transport(port), message).await;
}

/// Sends `message` through `transport`, failing the test if it is not accepted.
pub async fn send_through(transport: AsyncSmtpTransportBuilder, message: lettre::Message) {
    timeout(STEP_TIMEOUT, transport.build().send(message))
        .await
        .expect("lettre send timed out")
        .expect("lettre send succeeds");
}

/// One SMTP reply (all lines of a multi-line reply share its code).
#[derive(Debug)]
pub struct Reply {
    pub code: u16,
    /// The text of each line, after the code and separator.
    pub lines: Vec<String>,
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
        let mut lines = Vec::new();
        loop {
            let mut line = String::new();
            let read = timeout(STEP_TIMEOUT, self.stream.read_line(&mut line))
                .await
                .expect("reply timed out")
                .expect("read reply");
            assert!(read > 0, "connection closed while waiting for a reply");
            let line = line.trim_end_matches(['\r', '\n']);
            let code = line[..3].parse().expect("reply starts with a code");
            lines.push(line.get(4..).unwrap_or_default().to_string());
            if line.as_bytes().get(3) != Some(&b'-') {
                return Reply { code, lines };
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

    /// Has the core Capture `raw_source` (CRLF lines, none starting with a
    /// dot) with this envelope, failing the test if it is not accepted.
    pub async fn capture(&mut self, from: &str, to: &[&str], raw_source: &str) {
        assert!(
            raw_source.ends_with("\r\n"),
            "the raw source ends with CRLF"
        );
        let data = format!("{raw_source}.\r\n");
        let reply = self.send_transaction(from, to, data.as_bytes()).await;
        assert_eq!(reply.code, 250, "Email not Captured: {reply:?}");
    }
}

/// An agent connected to a core's MCP endpoint, as Claude Code would be.
pub struct Agent {
    client: RunningService<RoleClient, ClientConfig>,
}

impl Agent {
    /// Connects over streamable HTTP to `http://127.0.0.1:<port>/mcp`.
    pub async fn connect(port: u16) -> Self {
        Self::try_connect(port).await.expect("MCP client connects")
    }

    /// Like [`Agent::connect`], but returns why the agent could not connect.
    pub async fn try_connect(port: u16) -> Result<Self, String> {
        let transport =
            StreamableHttpClientTransport::from_uri(format!("http://127.0.0.1:{port}/mcp"));
        let client = timeout(STEP_TIMEOUT, ClientConfig::default().serve(transport))
            .await
            .expect("MCP connect timed out")
            .map_err(|error| error.to_string())?;
        Ok(Self { client })
    }

    /// Calls `tool` with `arguments` (a JSON object) and returns its result,
    /// failing the test on a protocol error.
    pub async fn call(&self, tool: &'static str, arguments: Value) -> CallToolResult {
        let arguments = arguments
            .as_object()
            .expect("tool arguments are a JSON object")
            .clone();
        let request = CallToolRequestParams::new(tool).with_arguments(arguments);
        timeout(STEP_TIMEOUT, self.client.call_tool(request))
            .await
            .expect("MCP call timed out")
            .expect("MCP call succeeds")
    }

    /// Calls `tool` and returns its structured result, failing the test if
    /// the tool reports an error.
    pub async fn call_ok(&self, tool: &'static str, arguments: Value) -> Value {
        let result = self.call(tool, arguments).await;
        assert_ne!(result.is_error, Some(true), "{tool} failed: {result:?}");
        result
            .structured_content
            .expect("the tool returns structured content")
    }

    /// The Emails `list_emails` returns for `arguments`.
    pub async fn list_emails(&self, arguments: Value) -> Vec<Value> {
        let mut result = self.call_ok("list_emails", arguments).await;
        match result["emails"].take() {
            Value::Array(emails) => emails,
            other => panic!("list_emails returned no list of Emails: {other:?}"),
        }
    }
}
