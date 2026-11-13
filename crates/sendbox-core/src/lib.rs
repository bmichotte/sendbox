//! Sendbox core: Captures Emails over SMTP into a single persistent Inbox, and
//! lets agents list, read and delete them over MCP.
//!
//! The core has no Tauri dependency. A shell starts it from a [`Config`] with
//! [`start`] and drives it through the returned [`CoreHandle`].

mod inbox;
mod mcp;
mod smtp;
mod view;

use std::io;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use inbox::Inbox;
use tokio::sync::broadcast;

pub use view::{Attachment, AttachmentContent, EmailView, Header};

/// What a core needs to start. [`Config::new`] gives the defaults.
#[derive(Debug, Clone)]
pub struct Config {
    /// Port of the SMTP listener on `127.0.0.1`; 0 binds an ephemeral port.
    pub smtp_port: u16,
    /// Port of the MCP endpoint (`/mcp`) on `127.0.0.1`; 0 binds an ephemeral port.
    pub mcp_port: u16,
    /// Whether an Email an agent reads over MCP becomes read.
    pub mcp_marks_read: bool,
    /// The most Emails the Inbox keeps: past it, the oldest are pruned.
    pub retention_limit: NonZeroUsize,
    /// Directory holding the Inbox database; created if missing.
    pub data_dir: PathBuf,
}

impl Config {
    /// The defaults: SMTP on 1025, MCP on 1026, MCP reads mark Emails as
    /// read, and a Retention limit of 1000 Emails.
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        Self {
            smtp_port: 1025,
            mcp_port: 1026,
            mcp_marks_read: true,
            retention_limit: NonZeroUsize::new(1000).expect("1000 is not zero"),
            data_dir: data_dir.as_ref().to_path_buf(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("Inbox storage error: {0}")]
    Storage(#[from] rusqlite::Error),
}

/// Identifies one Email in the Inbox. Ids are never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EmailId(pub i64);

/// What the Inbox list shows about one Email.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailSummary {
    pub id: EmailId,
    /// When the Email was Captured.
    pub received_at: SystemTime,
    /// The first address in the `From` header, if any.
    pub from: Option<String>,
    /// The addresses in the `To` header, in order.
    pub to: Vec<String>,
    /// The addresses in the `Cc` header, in order.
    pub cc: Vec<String>,
    /// The SMTP `MAIL FROM` address; empty for a null sender (`MAIL FROM:<>`).
    pub envelope_sender: String,
    /// The SMTP `RCPT TO` addresses, in the order given (includes Bcc).
    pub envelope_recipients: Vec<String>,
    /// The decoded `Subject` header, if any.
    pub subject: Option<String>,
    /// Size of the raw source in bytes.
    pub size: u64,
    /// Whether the Email has been read; Unread otherwise.
    pub read: bool,
}

impl EmailSummary {
    /// Who the Email is from: the `From` header address, else the envelope
    /// sender, else `None` (a null sender and no `From` header).
    pub fn sender(&self) -> Option<&str> {
        self.from
            .as_deref()
            .or((!self.envelope_sender.is_empty()).then_some(self.envelope_sender.as_str()))
    }
}

/// Which Emails to list. The default matches every Email; each field set
/// narrows it further. Text filters are case-insensitive substrings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmailFilter {
    /// Part of the `From` header address or of the envelope sender.
    pub from: Option<String>,
    /// Part of a recipient address: `To`, `Cc` or envelope (so Bcc too).
    pub to: Option<String>,
    /// Part of the subject.
    pub subject: Option<String>,
    /// Only Emails Captured at or after this time.
    pub since: Option<SystemTime>,
    /// Only Unread Emails.
    pub unread_only: bool,
}

impl EmailFilter {
    /// Whether `email` is one of the Emails this filter lists.
    pub(crate) fn matches(&self, email: &EmailSummary) -> bool {
        let senders = [email.from.as_deref(), Some(&email.envelope_sender)];
        let recipients = [&email.to, &email.cc, &email.envelope_recipients];
        contains_any(&self.from, senders.into_iter().flatten())
            && contains_any(
                &self.to,
                recipients.into_iter().flatten().map(String::as_str),
            )
            && contains_any(&self.subject, email.subject.as_deref())
            && self.since.is_none_or(|since| email.received_at >= since)
            && !(self.unread_only && email.read)
    }
}

/// Which Emails to delete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmailSelection {
    /// The Emails with these ids; ids no Email has are ignored.
    Ids(Vec<EmailId>),
    /// The Emails the filter lists.
    Matching(EmailFilter),
    /// Every Email: Clear all.
    All,
}

/// True if there is no `needle`, or if one of `haystacks` contains it,
/// ignoring case (in any script, not only ASCII).
fn contains_any<'a>(needle: &Option<String>, haystacks: impl IntoIterator<Item = &'a str>) -> bool {
    let Some(needle) = needle else { return true };
    let needle = needle.to_lowercase();
    haystacks
        .into_iter()
        .any(|haystack| haystack.to_lowercase().contains(&needle))
}

/// Something that happened in the core, broadcast live to subscribers.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CoreEvent {
    /// An Email was Captured into the Inbox. Past the Retention limit, the
    /// oldest Emails were pruned with it.
    EmailCaptured(EmailSummary),
    /// The Inbox changed other than by a Capture (Emails were deleted or
    /// pruned, or an Unread Email was read, by the user or an agent): list
    /// it again, and count the Unread Emails again.
    InboxChanged,
}

/// A running core. Dropping it without [`CoreHandle::shutdown`] leaves the
/// listeners running until the async runtime stops.
pub struct CoreHandle {
    inbox: Inbox,
    smtp: smtp::Listener,
    mcp: mcp::Listener,
    /// Whether MCP reads mark Emails as read, shared with the MCP endpoint.
    mcp_marks_read: Arc<AtomicBool>,
}

/// Starts a core: opens the Inbox in the data directory, binds the SMTP
/// listener and the MCP endpoint, and begins Capturing.
pub async fn start(config: Config) -> Result<CoreHandle, Error> {
    let inbox = Inbox::open(&config.data_dir, config.retention_limit).await?;
    let smtp = smtp::Listener::bind(config.smtp_port, inbox.clone()).await?;
    let marks_read = Arc::new(AtomicBool::new(config.mcp_marks_read));
    let mcp = match mcp::Listener::bind(config.mcp_port, inbox.clone(), marks_read.clone()).await {
        Ok(mcp) => mcp,
        Err(error) => {
            // Leave nothing listening behind a core that failed to start.
            smtp.shutdown().await;
            return Err(error.into());
        }
    };
    Ok(CoreHandle {
        inbox,
        smtp,
        mcp,
        mcp_marks_read: marks_read,
    })
}

impl CoreHandle {
    /// The port the SMTP listener is actually bound to.
    pub fn smtp_port(&self) -> u16 {
        self.smtp.port()
    }

    /// The port the MCP endpoint is actually bound to.
    pub fn mcp_port(&self) -> u16 {
        self.mcp.port()
    }

    /// The Emails `filter` matches, newest first, at most `limit` of them.
    pub async fn list_emails(
        &self,
        filter: &EmailFilter,
        limit: Option<usize>,
    ) -> Result<Vec<EmailSummary>, Error> {
        self.inbox.list(filter.clone(), limit).await
    }

    /// One Email as read: its content parsed from the raw source, or `None`
    /// if no Email has this id. Reading it does not mark it read.
    pub async fn email(&self, id: EmailId) -> Result<Option<EmailView>, Error> {
        self.inbox.view(id).await
    }

    /// The attachment at `index` in the Email view's
    /// [`attachments`](EmailView::attachments), with its content, for saving
    /// it; `None` if there is no such Email or attachment. Only this gives
    /// attachments' bytes, and only to the shell: never over MCP.
    pub async fn attachment(
        &self,
        id: EmailId,
        index: usize,
    ) -> Result<Option<AttachmentContent>, Error> {
        self.inbox.attachment(id, index).await
    }

    /// How many Emails in the whole Inbox are Unread, as the tray shows it.
    pub async fn unread_count(&self) -> Result<usize, Error> {
        self.inbox.unread_count().await
    }

    /// Marks one Email read, as when the user opens it, whether or not MCP
    /// reads mark Emails read. Emits [`CoreEvent::InboxChanged`] if it was
    /// Unread; nothing happens if no Email has this id.
    pub async fn mark_read(&self, id: EmailId) -> Result<(), Error> {
        self.inbox.mark_read(id).await
    }

    /// Deletes the `selected` Emails for good and returns how many there were.
    pub async fn delete_emails(&self, selected: EmailSelection) -> Result<usize, Error> {
        self.inbox.delete(selected).await
    }

    /// Changes the Retention limit. A lower limit prunes the oldest Emails
    /// beyond it at once, read or not, and emits [`CoreEvent::InboxChanged`]
    /// if it pruned any.
    pub async fn set_retention_limit(&self, limit: NonZeroUsize) -> Result<(), Error> {
        self.inbox.set_retention_limit(limit).await
    }

    /// Turns MCP read-marking on or off: whether an Email an agent reads
    /// becomes read. Applies to agents already connected, from their next call.
    pub fn set_mcp_marks_read(&self, marks_read: bool) {
        self.mcp_marks_read.store(marks_read, Ordering::Relaxed);
    }

    /// Live events from now on. A subscriber that falls more than a few
    /// hundred events behind gets `RecvError::Lagged` and should re-list.
    pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.inbox.subscribe()
    }

    /// The Email exactly as received in DATA (after dot-unstuffing), or
    /// `None` if no Email has this id.
    pub async fn raw_source(&self, id: EmailId) -> Result<Option<Vec<u8>>, Error> {
        self.inbox.raw_source(id).await
    }

    /// Stops listening and closes open SMTP sessions and MCP connections.
    /// When this returns, both ports are free.
    pub async fn shutdown(self) {
        self.smtp.shutdown().await;
        self.mcp.shutdown().await;
    }
}
