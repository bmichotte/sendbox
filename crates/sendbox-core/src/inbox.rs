//! The Inbox: every Captured Email, kept in a single SQLite file.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Row, params};
use tokio::sync::broadcast;

use crate::{CoreEvent, EmailId, EmailSummary, Error};

const DATABASE_FILE: &str = "inbox.sqlite3";
/// Events a slow subscriber may fall behind by before it starts missing some.
const EVENT_BACKLOG: usize = 256;

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS emails (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        received_at_ns      INTEGER NOT NULL,
        envelope_sender     TEXT NOT NULL,
        envelope_recipients TEXT NOT NULL,
        subject             TEXT,
        size                INTEGER NOT NULL,
        raw_source          BLOB NOT NULL
    );
";

/// The SMTP envelope of one Email: who the client said it was from and to.
pub(crate) struct Envelope {
    pub(crate) sender: String,
    pub(crate) recipients: Vec<String>,
}

#[derive(Clone)]
pub(crate) struct Inbox {
    db: Arc<Mutex<Connection>>,
    events: broadcast::Sender<CoreEvent>,
}

impl Inbox {
    pub(crate) async fn open(data_dir: &Path) -> Result<Self, Error> {
        let path = data_dir.join(DATABASE_FILE);
        let data_dir = data_dir.to_path_buf();
        let db = blocking(move || -> Result<Connection, Error> {
            std::fs::create_dir_all(&data_dir)?;
            let db = Connection::open(path)?;
            db.execute_batch(SCHEMA)?;
            Ok(db)
        })
        .await?;
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            events: broadcast::channel(EVENT_BACKLOG).0,
        })
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.events.subscribe()
    }

    /// Keeps `raw_source` exactly as received, with its envelope, and emits
    /// [`CoreEvent::EmailCaptured`].
    pub(crate) async fn capture(
        &self,
        envelope: Envelope,
        raw_source: Vec<u8>,
    ) -> Result<EmailSummary, Error> {
        let received_at = SystemTime::now();
        let subject = mail_parser::MessageParser::default()
            .parse_headers(&raw_source)
            .and_then(|message| message.subject().map(str::to_owned));
        let email = self
            .with_db(move |db| {
                db.execute(
                    "INSERT INTO emails (received_at_ns, envelope_sender, envelope_recipients,
                                     subject, size, raw_source)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        to_unix_nanos(received_at),
                        envelope.sender,
                        encode_recipients(&envelope.recipients),
                        subject,
                        raw_source.len() as i64,
                        raw_source,
                    ],
                )?;
                Ok(EmailSummary {
                    id: EmailId(db.last_insert_rowid()),
                    received_at,
                    envelope_sender: envelope.sender,
                    envelope_recipients: envelope.recipients,
                    subject,
                    size: raw_source.len() as u64,
                })
            })
            .await?;
        // No subscriber is not an error: nobody is watching yet.
        let _ = self.events.send(CoreEvent::EmailCaptured(email.clone()));
        Ok(email)
    }

    /// Every Email, newest first.
    pub(crate) async fn list(&self) -> Result<Vec<EmailSummary>, Error> {
        self.with_db(|db| {
            let mut statement = db.prepare(
                "SELECT id, received_at_ns, envelope_sender, envelope_recipients, subject, size
                 FROM emails ORDER BY id DESC",
            )?;
            let emails = statement
                .query_map([], summary_from_row)?
                .collect::<Result<_, _>>()?;
            Ok(emails)
        })
        .await
    }

    pub(crate) async fn raw_source(&self, id: EmailId) -> Result<Option<Vec<u8>>, Error> {
        self.with_db(move |db| {
            Ok(db
                .query_row(
                    "SELECT raw_source FROM emails WHERE id = ?1",
                    [id.0],
                    |row| row.get(0),
                )
                .optional()?)
        })
        .await
    }

    /// Runs `f` against the database off the async runtime's worker threads.
    async fn with_db<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        let db = Arc::clone(&self.db);
        blocking(move || f(&db.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))).await
    }
}

fn summary_from_row(row: &Row<'_>) -> rusqlite::Result<EmailSummary> {
    Ok(EmailSummary {
        id: EmailId(row.get(0)?),
        received_at: from_unix_nanos(row.get(1)?),
        envelope_sender: row.get(2)?,
        envelope_recipients: decode_recipients(&row.get::<_, String>(3)?),
        subject: row.get(4)?,
        size: row.get::<_, i64>(5)? as u64,
    })
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    match tokio::task::spawn_blocking(f).await {
        Ok(result) => result,
        Err(join_error) => std::panic::resume_unwind(join_error.into_panic()),
    }
}

// SMTP addresses never contain a line break, so one per line is unambiguous.
fn encode_recipients(recipients: &[String]) -> String {
    recipients.join("\n")
}

fn decode_recipients(encoded: &str) -> Vec<String> {
    encoded.lines().map(str::to_owned).collect()
}

// Nanoseconds since the Unix epoch round-trip `SystemTime` exactly on every
// supported platform, and fit in an i64 until the year 2262.
fn to_unix_nanos(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos() as i64)
}

fn from_unix_nanos(nanos: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_nanos(nanos.max(0) as u64)
}
