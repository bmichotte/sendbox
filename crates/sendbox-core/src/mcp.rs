//! The MCP endpoint agents connect to: `rmcp`'s streamable HTTP service,
//! mounted at `/mcp` on `127.0.0.1` (ADR-0001).
//!
//! Tools go through the same Inbox queries as the handle, so agents and the
//! window always agree on what the Inbox holds and what is Unread.

use std::io;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::SystemTime;

use chrono::{DateTime, SubsecRound, Utc};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{Implementation, ServerCapabilities, ServerConfig};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::inbox::Inbox;
use crate::{EmailFilter, EmailId, EmailSelection, EmailSummary, EmailView};

/// The path of the MCP endpoint, as in `http://127.0.0.1:1026/mcp`.
const ENDPOINT: &str = "/mcp";
/// The only `Host` names a request may use.
const LOCAL_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];
/// The only `Origin`s a request may come from, when it has one: pages served
/// from this machine, on any port.
const LOCAL_ORIGINS: [&str; 3] = ["http://localhost:*", "http://127.0.0.1:*", "http://[::1]:*"];

pub(crate) struct Listener {
    port: u16,
    stop: CancellationToken,
    task: JoinHandle<()>,
}

impl Listener {
    /// Binds `127.0.0.1:port` (loopback only) and starts serving `/mcp`.
    pub(crate) async fn bind(port: u16, inbox: Inbox, marks_read: bool) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
        let port = listener.local_addr()?.port();
        let stop = CancellationToken::new();
        let tools = Tools::new(inbox, marks_read);
        let config = StreamableHttpServerConfig::default()
            // Against DNS rebinding: a web page can make the browser send
            // requests here under its own name (Host) and with its own Origin.
            // Agents send a local Host and no Origin. Entries without a port
            // match any port.
            .with_allowed_hosts(LOCAL_HOSTS)
            .with_allowed_origins(LOCAL_ORIGINS)
            // The token also closes open sessions; without it, shutdown would
            // wait forever for a connected agent's event stream to end.
            .with_cancellation_token(stop.child_token());
        let service: StreamableHttpService<Tools, LocalSessionManager> = StreamableHttpService::new(
            move || Ok(tools.clone()),
            Arc::new(LocalSessionManager::default()),
            config,
        );
        let router = axum::Router::new().nest_service(ENDPOINT, service);
        let stopped = stop.clone().cancelled_owned();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(stopped)
                .await;
        });
        Ok(Self { port, stop, task })
    }

    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    pub(crate) async fn shutdown(self) {
        self.stop.cancel();
        let _ = self.task.await;
    }
}

/// The tools of one MCP session; every session shares the same Inbox.
#[derive(Clone)]
struct Tools {
    inbox: Inbox,
    /// Whether `get_email` marks the Email it returns read.
    marks_read: bool,
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ListParams {
    /// Only Emails whose sender (From header or envelope) contains this, ignoring case.
    from: Option<String>,
    /// Only Emails with a recipient (To, Cc or envelope, so Bcc too) containing this, ignoring case.
    to: Option<String>,
    /// Only Emails whose subject contains this, ignoring case.
    subject: Option<String>,
    /// Only Emails Captured at or after this time (RFC 3339).
    since: Option<DateTime<Utc>>,
    /// Only Unread Emails.
    #[serde(default)]
    unread_only: bool,
    /// At most this many Emails (default 20).
    #[serde(default = "default_limit")]
    limit: u32,
}

fn default_limit() -> u32 {
    20
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct EmailList {
    /// Newest first.
    emails: Vec<Summary>,
}

/// One Email as `list_emails` shows it, without its content.
#[derive(Debug, Serialize, schemars::JsonSchema)]
struct Summary {
    /// Pass it to `get_email`.
    id: i64,
    /// The From header address, else the envelope sender.
    from: Option<String>,
    /// The To header addresses.
    to: Vec<String>,
    subject: Option<String>,
    /// When Sendbox Captured the Email.
    date: DateTime<Utc>,
    /// False while the Email is Unread.
    read: bool,
}

impl From<EmailSummary> for Summary {
    fn from(email: EmailSummary) -> Self {
        Self {
            id: email.id.0,
            from: email.sender().map(str::to_owned),
            to: email.to,
            subject: email.subject,
            date: to_date(email.received_at),
            read: email.read,
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct GetParams {
    /// The id of the Email, as list_emails gives it.
    id: i64,
}

/// One Email as `get_email` shows it: everything but the attachments' bytes.
#[derive(Debug, Serialize, schemars::JsonSchema)]
struct Email {
    id: i64,
    /// The From header address, else the envelope sender.
    from: Option<String>,
    /// The To header addresses.
    to: Vec<String>,
    /// The Cc header addresses.
    cc: Vec<String>,
    subject: Option<String>,
    /// When Sendbox Captured the Email.
    date: DateTime<Utc>,
    /// False while the Email is Unread.
    read: bool,
    /// Who the sending app said the Email was from and to over SMTP; the
    /// recipients include Bcc.
    envelope: Envelope,
    /// The plain-text body, if any.
    text: Option<String>,
    /// The HTML body, if any.
    html: Option<String>,
    /// Every header field, in order, decoded.
    headers: Vec<HeaderField>,
    /// Link targets in the HTML, then URLs in the text, each once.
    links: Vec<String>,
    /// What is attached (the content is not available over MCP).
    attachments: Vec<AttachmentInfo>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct Envelope {
    /// `MAIL FROM`; null for a null sender (`<>`).
    sender: Option<String>,
    /// `RCPT TO`, in order.
    recipients: Vec<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct HeaderField {
    name: String,
    value: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct AttachmentInfo {
    /// The file name, if the sender gave one.
    name: Option<String>,
    content_type: String,
    /// Bytes, once decoded.
    size: u64,
}

impl From<EmailView> for Email {
    fn from(view: EmailView) -> Self {
        let EmailView {
            summary,
            text,
            html,
            headers,
            links,
            attachments,
        } = view;
        Self {
            id: summary.id.0,
            from: summary.sender().map(str::to_owned),
            date: to_date(summary.received_at),
            read: summary.read,
            envelope: Envelope {
                sender: Some(summary.envelope_sender).filter(|sender| !sender.is_empty()),
                recipients: summary.envelope_recipients,
            },
            to: summary.to,
            cc: summary.cc,
            subject: summary.subject,
            text,
            html,
            headers: headers
                .into_iter()
                .map(|header| HeaderField {
                    name: header.name,
                    value: header.value,
                })
                .collect(),
            links,
            attachments: attachments
                .into_iter()
                .map(|attachment| AttachmentInfo {
                    name: attachment.name,
                    content_type: attachment.content_type,
                    size: attachment.size,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct DeleteParams {
    /// Delete the Emails with these ids, as list_emails gives them.
    ids: Option<Vec<i64>>,
    /// Delete the Emails whose sender (From header or envelope) contains this, ignoring case.
    from: Option<String>,
    /// Delete the Emails with a recipient (To, Cc or envelope, so Bcc too) containing this, ignoring case.
    to: Option<String>,
    /// Delete the Emails whose subject contains this, ignoring case.
    subject: Option<String>,
    /// Delete the Emails Captured at or after this time (RFC 3339).
    since: Option<DateTime<Utc>>,
    /// Delete only Unread Emails.
    #[serde(default)]
    unread_only: bool,
    /// Delete every Email, emptying the Inbox.
    #[serde(default)]
    all: bool,
}

impl DeleteParams {
    /// The Emails the parameters select. An agent must say which Emails it
    /// means, in exactly one way: deleting everything takes `all: true`,
    /// never an empty request, and a mix could be read more than one way.
    fn selection(self) -> Result<EmailSelection, String> {
        let filter = EmailFilter {
            from: self.from,
            to: self.to,
            subject: self.subject,
            since: self.since.map(SystemTime::from),
            unread_only: self.unread_only,
        };
        let filtered = filter != EmailFilter::default();
        match (self.ids, filtered, self.all) {
            (Some(ids), false, false) => {
                Ok(EmailSelection::Ids(ids.into_iter().map(EmailId).collect()))
            }
            (None, true, false) => Ok(EmailSelection::Matching(filter)),
            (None, false, true) => Ok(EmailSelection::All),
            (None, false, false) => Err("Say which Emails to delete: their ids, \
                 filters (from, to, subject, since, unread_only), or all: true to \
                 empty the Inbox."
                .to_string()),
            _ => Err("Give only one of ids, filters (from, to, subject, since, \
                 unread_only) or all: true, so that it is clear which Emails to delete."
                .to_string()),
        }
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct Deleted {
    /// How many Emails were deleted.
    deleted: usize,
}

/// A time to the millisecond, which is all an agent needs to read or compare.
fn to_date(time: SystemTime) -> DateTime<Utc> {
    DateTime::<Utc>::from(time).trunc_subsecs(3)
}

#[tool_router]
impl Tools {
    fn new(inbox: Inbox, marks_read: bool) -> Self {
        Self {
            inbox,
            marks_read,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "List the Emails Sendbox Captured, newest first, without their content. \
            Filters combine; text filters match case-insensitive substrings. \
            Listing never changes whether an Email is read.",
        annotations(read_only_hint = true)
    )]
    async fn list_emails(
        &self,
        Parameters(params): Parameters<ListParams>,
    ) -> Result<Json<EmailList>, ErrorData> {
        let filter = EmailFilter {
            from: params.from,
            to: params.to,
            subject: params.subject,
            since: params.since.map(SystemTime::from),
            unread_only: params.unread_only,
        };
        let emails = self
            .inbox
            .list(filter, Some(params.limit as usize))
            .await
            .map_err(internal_error)?;
        Ok(Json(EmailList {
            emails: emails.into_iter().map(Summary::from).collect(),
        }))
    }

    #[tool(
        description = "Read one Email: its text and HTML bodies, headers, SMTP envelope \
            (recipients include Bcc), the links it contains (to follow a verification \
            link, say) and its attachments' names, types and sizes. Marks the Email \
            read, unless the user turned that off in Sendbox's settings."
    )]
    async fn get_email(
        &self,
        Parameters(GetParams { id }): Parameters<GetParams>,
    ) -> Result<Json<Email>, String> {
        let id = EmailId(id);
        if self.marks_read {
            self.inbox
                .mark_read(id)
                .await
                .map_err(|error| error.to_string())?;
        }
        // An unknown id is the agent's mistake: a tool error it can read and
        // act on, rather than a protocol error.
        match self.inbox.view(id).await {
            Ok(Some(view)) => Ok(Json(Email::from(view))),
            Ok(None) => Err(format!(
                "No Email has id {}: it may have been deleted. \
                 List the Emails with list_emails to get their ids.",
                id.0
            )),
            Err(error) => Err(error.to_string()),
        }
    }

    #[tool(
        description = "Delete Emails for good and return how many were deleted, to start \
            a test from a known Inbox. Select them in exactly one way: by ids, by the \
            list_emails filters (from, to, subject, since, unread_only; they combine), \
            or with all: true to empty the Inbox. A call that selects nothing, or mixes \
            these, is refused.",
        annotations(destructive_hint = true)
    )]
    async fn delete_emails(
        &self,
        Parameters(params): Parameters<DeleteParams>,
    ) -> Result<Json<Deleted>, String> {
        let deleted = self
            .inbox
            .delete(params.selection()?)
            .await
            .map_err(|error| error.to_string())?;
        Ok(Json(Deleted { deleted }))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Tools {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("sendbox", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Sendbox Captures every Email sent to its local SMTP server and never \
                 delivers any. Find Captured Emails with list_emails, then read one \
                 (bodies, headers, links, attachments) with get_email. \
                 Remove Emails with delete_emails.",
            )
    }
}

fn internal_error(error: crate::Error) -> ErrorData {
    ErrorData::internal_error(error.to_string(), None)
}
