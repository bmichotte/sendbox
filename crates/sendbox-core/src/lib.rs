//! Sendbox core: Captures Emails over SMTP into a single persistent Inbox.
//!
//! The core has no Tauri dependency. A shell starts it from a [`Config`] with
//! [`start`] and drives it through the returned [`CoreHandle`].

mod inbox;
mod smtp;

use std::io;
use std::path::PathBuf;
use std::time::SystemTime;

use inbox::Inbox;
use tokio::sync::broadcast;

/// What a core needs to start.
#[derive(Debug, Clone)]
pub struct Config {
    /// Port of the SMTP listener on `127.0.0.1`; 0 binds an ephemeral port.
    pub smtp_port: u16,
    /// Directory holding the Inbox database; created if missing.
    pub data_dir: PathBuf,
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
    /// The SMTP `MAIL FROM` address.
    pub envelope_sender: String,
    /// The SMTP `RCPT TO` addresses, in the order given (includes Bcc).
    pub envelope_recipients: Vec<String>,
    /// The decoded `Subject` header, if any.
    pub subject: Option<String>,
    /// Size of the raw source in bytes.
    pub size: u64,
}

/// Something that happened in the core, broadcast live to subscribers.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CoreEvent {
    /// An Email was Captured into the Inbox.
    EmailCaptured(EmailSummary),
}

/// A running core. Dropping it without [`CoreHandle::shutdown`] leaves the
/// listener running until the async runtime stops.
pub struct CoreHandle {
    inbox: Inbox,
    smtp: smtp::Listener,
}

/// Starts a core: opens the Inbox in the data directory, binds the SMTP
/// listener and begins Capturing.
pub async fn start(config: Config) -> Result<CoreHandle, Error> {
    let inbox = Inbox::open(&config.data_dir).await?;
    let smtp = smtp::Listener::bind(config.smtp_port, inbox.clone()).await?;
    Ok(CoreHandle { inbox, smtp })
}

impl CoreHandle {
    /// The port the SMTP listener is actually bound to.
    pub fn smtp_port(&self) -> u16 {
        self.smtp.port()
    }

    /// Every Email in the Inbox, newest first.
    pub async fn list_emails(&self) -> Result<Vec<EmailSummary>, Error> {
        self.inbox.list().await
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

    /// Stops listening and closes open SMTP sessions. When this returns, the
    /// SMTP port is free.
    pub async fn shutdown(self) {
        self.smtp.shutdown().await;
    }
}
