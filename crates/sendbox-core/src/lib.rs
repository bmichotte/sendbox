//! Sendbox core: Captures Emails over SMTP into a single persistent Inbox, and
//! lets agents list, read and delete them over MCP.
//!
//! The core has no Tauri dependency. A shell starts it from a [`Config`] with
//! [`start`] and drives it through the returned [`CoreHandle`].

mod inbox;
mod listener;
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
    /// When the Email was Captured, to the millisecond: the precision agents
    /// and the window see, so that a date one of them got back compares as
    /// they expect.
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

    /// The Sender tab the Email is in: its [sender](Self::sender), ignoring
    /// case, or the "(no sender)" tab.
    pub fn sender_tab(&self) -> SenderTabKey {
        match self.sender() {
            Some(sender) => SenderTabKey::Address(sender.to_lowercase()),
            None => SenderTabKey::NoSender,
        }
    }
}

/// Which Sender tab an Email is in.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SenderTabKey {
    /// The Emails from this address, lowercased: the `From` header address
    /// (display name ignored), else the envelope sender.
    Address(String),
    /// The Emails with neither a `From` header address nor an envelope
    /// sender: the "(no sender)" tab.
    NoSender,
}

/// One Sender tab, as the window shows it next to "All".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderTab {
    pub key: SenderTabKey,
    /// How many of the tab's Emails are Unread.
    pub unread: usize,
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
    /// Part of the sender, of a recipient or of the subject, as `from`, `to`
    /// and `subject` see them: any one of them may contain it. The window's
    /// search box; empty, it matches every Email.
    pub search: Option<String>,
    /// Only Emails Captured after this time: an Email's own
    /// [`received_at`](EmailSummary::received_at) leaves it out.
    pub since: Option<SystemTime>,
    /// Only Unread Emails.
    pub unread_only: bool,
    /// Only the Emails in this Sender tab.
    pub sender_tab: Option<SenderTabKey>,
}

impl EmailFilter {
    /// Whether `email` is one of the Emails this filter lists.
    pub(crate) fn matches(&self, email: &EmailSummary) -> bool {
        let subject = || email.subject.as_deref().into_iter();
        let search = self.search.as_deref().filter(|search| !search.is_empty());
        contains_any(self.from.as_deref(), senders(email))
            && contains_any(self.to.as_deref(), recipients(email))
            && contains_any(self.subject.as_deref(), subject())
            && contains_any(
                search,
                senders(email).chain(recipients(email)).chain(subject()),
            )
            && self.since.is_none_or(|since| email.received_at > since)
            && !(self.unread_only && email.read)
            && self
                .sender_tab
                .as_ref()
                .is_none_or(|tab| *tab == email.sender_tab())
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

/// The addresses the sender filters look in: the `From` header address and
/// the envelope sender.
fn senders(email: &EmailSummary) -> impl Iterator<Item = &str> {
    [email.from.as_deref(), Some(email.envelope_sender.as_str())]
        .into_iter()
        .flatten()
}

/// The addresses the recipient filters look in: `To`, `Cc` and the
/// envelope, so Bcc too.
fn recipients(email: &EmailSummary) -> impl Iterator<Item = &str> {
    [&email.to, &email.cc, &email.envelope_recipients]
        .into_iter()
        .flatten()
        .map(String::as_str)
}

/// True if there is no `needle`, or if one of `haystacks` contains it,
/// ignoring case (in any script, not only ASCII).
fn contains_any<'a>(needle: Option<&str>, haystacks: impl IntoIterator<Item = &'a str>) -> bool {
    let Some(needle) = needle else { return true };
    let needle = needle.to_lowercase();
    haystacks
        .into_iter()
        .any(|haystack| haystack.to_lowercase().contains(&needle))
}

/// One of the core's two listeners, each on its own port of `127.0.0.1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Listener {
    /// The SMTP listener apps send Emails to.
    Smtp,
    /// The MCP endpoint agents connect to.
    Mcp,
}

/// Whether a listener is listening.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenerStatus {
    /// Listening on `port`: the port actually bound, so for 0 the ephemeral
    /// port the system chose.
    Running { port: u16 },
    /// Not listening, because another program holds `port`. The rest of the
    /// core keeps running.
    PortInUse { port: u16 },
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
    /// A listener's status changed after [`CoreHandle::set_port`].
    ListenerChanged {
        listener: Listener,
        status: ListenerStatus,
    },
}

/// A running core. Dropping it without [`CoreHandle::shutdown`] leaves the
/// listeners running until the async runtime stops.
pub struct CoreHandle {
    inbox: Inbox,
    smtp: listener::Slot,
    mcp: listener::Slot,
    /// Whether MCP reads mark Emails as read, shared with the MCP endpoint.
    mcp_marks_read: Arc<AtomicBool>,
}

/// Starts a core: opens the Inbox in the data directory, binds the SMTP
/// listener and the MCP endpoint, and begins Capturing. A port in use stops
/// only its listener, which says so in [`CoreHandle::listener_status`]: only
/// an Inbox that cannot open fails the start.
pub async fn start(config: Config) -> Result<CoreHandle, Error> {
    let inbox = Inbox::open(&config.data_dir, config.retention_limit).await?;
    let marks_read = Arc::new(AtomicBool::new(config.mcp_marks_read));
    let smtp = listener::Slot::start(config.smtp_port, smtp::server(inbox.clone())).await;
    let mcp_server = mcp::server(inbox.clone(), marks_read.clone());
    let mcp = listener::Slot::start(config.mcp_port, mcp_server).await;
    Ok(CoreHandle {
        inbox,
        smtp,
        mcp,
        mcp_marks_read: marks_read,
    })
}

impl CoreHandle {
    /// Whether `listener` is listening, and on which port.
    pub fn listener_status(&self, listener: Listener) -> ListenerStatus {
        self.listener(listener).status()
    }

    /// Moves `listener` to `port` while the core runs (0 for an ephemeral
    /// port), closing what was open on the old one, and returns its new
    /// status: on a port in use, the listener stops and says so. The same
    /// port tries again a port that was in use, and leaves alone a listener
    /// already running there. Each change also emits
    /// [`CoreEvent::ListenerChanged`].
    pub async fn set_port(&self, listener: Listener, port: u16) -> ListenerStatus {
        let notify = |status| {
            self.inbox
                .notify(CoreEvent::ListenerChanged { listener, status })
        };
        self.listener(listener).move_to(port, notify).await
    }

    fn listener(&self, listener: Listener) -> &listener::Slot {
        match listener {
            Listener::Smtp => &self.smtp,
            Listener::Mcp => &self.mcp,
        }
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

    /// One Sender tab per sender with Emails in the Inbox, the tab with the
    /// most recent Email first. List a tab's Emails with
    /// [`EmailFilter::sender_tab`].
    pub async fn sender_tabs(&self) -> Result<Vec<SenderTab>, Error> {
        self.inbox.sender_tabs().await
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
