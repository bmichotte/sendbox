//! The Inbox: every Captured Email, kept in a single SQLite file.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Row, params};
use tokio::sync::broadcast;

use crate::{CoreEvent, EmailFilter, EmailId, EmailSummary, EmailView, Error};

const DATABASE_FILE: &str = "inbox.sqlite3";
/// Events a slow subscriber may fall behind by before it starts missing some.
const EVENT_BACKLOG: usize = 256;

/// Each step brings the schema up one version, and `PRAGMA user_version`
/// records how many have run. Append new steps; never edit one that may have
/// run on someone's Inbox.
const MIGRATIONS: &[&str] = &[
    // 1: the Inbox as first Captured. Inboxes from before versioning are at
    // version 0 but already have this table.
    "CREATE TABLE IF NOT EXISTS emails (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        received_at_ns      INTEGER NOT NULL,
        envelope_sender     TEXT NOT NULL,
        envelope_recipients TEXT NOT NULL,
        subject             TEXT,
        size                INTEGER NOT NULL,
        raw_source          BLOB NOT NULL
    );",
    // 2: header From and To for the Inbox list; Emails Captured earlier have none.
    "ALTER TABLE emails ADD COLUMN from_address TEXT;
     ALTER TABLE emails ADD COLUMN to_addresses TEXT NOT NULL DEFAULT '';",
    // 3: read state, and header Cc for the `to` filter. Emails Captured
    // earlier are Unread and have no Cc (their envelope still has every
    // recipient).
    "ALTER TABLE emails ADD COLUMN read INTEGER NOT NULL DEFAULT 0;
     ALTER TABLE emails ADD COLUMN cc_addresses TEXT NOT NULL DEFAULT '';",
];

/// The columns [`summary_from_row`] reads, in order.
const SUMMARY_COLUMNS: &str = "id, received_at_ns, envelope_sender, envelope_recipients,
     from_address, to_addresses, cc_addresses, subject, size, read";

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
            let mut db = Connection::open(path)?;
            migrate(&mut db)?;
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
        let headers = SummaryHeaders::parse(&raw_source);
        let email = self
            .with_db(move |db| {
                db.execute(
                    "INSERT INTO emails (received_at_ns, envelope_sender, envelope_recipients,
                                     from_address, to_addresses, cc_addresses, subject,
                                     size, raw_source)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        to_unix_nanos(received_at),
                        envelope.sender,
                        encode_addresses(&envelope.recipients),
                        headers.from,
                        encode_addresses(&headers.to),
                        encode_addresses(&headers.cc),
                        headers.subject,
                        raw_source.len() as i64,
                        raw_source,
                    ],
                )?;
                Ok(EmailSummary {
                    id: EmailId(db.last_insert_rowid()),
                    received_at,
                    from: headers.from,
                    to: headers.to,
                    cc: headers.cc,
                    envelope_sender: envelope.sender,
                    envelope_recipients: envelope.recipients,
                    subject: headers.subject,
                    size: raw_source.len() as u64,
                    read: false,
                })
            })
            .await?;
        // No subscriber is not an error: nobody is watching yet.
        let _ = self.events.send(CoreEvent::EmailCaptured(email.clone()));
        Ok(email)
    }

    /// The Emails `filter` matches, newest first, at most `limit` of them.
    ///
    /// Filtering happens here rather than in SQL: SQLite only folds ASCII
    /// case, and the Inbox is small (bounded by the Retention limit).
    pub(crate) async fn list(
        &self,
        filter: EmailFilter,
        limit: Option<usize>,
    ) -> Result<Vec<EmailSummary>, Error> {
        self.with_db(move |db| {
            let mut statement = db.prepare(&format!(
                "SELECT {SUMMARY_COLUMNS} FROM emails ORDER BY id DESC"
            ))?;
            let mut emails = Vec::new();
            for email in statement.query_map([], summary_from_row)? {
                let email = email?;
                if limit.is_some_and(|limit| emails.len() >= limit) {
                    break;
                }
                if filter.matches(&email) {
                    emails.push(email);
                }
            }
            Ok(emails)
        })
        .await
    }

    /// The oldest Email `filter` matches, if any, marked read first if
    /// `mark_read`. Finding and marking are one step, so concurrent callers
    /// looking for Unread Emails never get the same one.
    pub(crate) async fn oldest(
        &self,
        filter: EmailFilter,
        mark_read: bool,
    ) -> Result<Option<EmailId>, Error> {
        self.with_db(move |db| {
            let mut statement = db.prepare(&format!(
                "SELECT {SUMMARY_COLUMNS} FROM emails ORDER BY id ASC"
            ))?;
            let mut oldest = None;
            for email in statement.query_map([], summary_from_row)? {
                let email = email?;
                if filter.matches(&email) {
                    oldest = Some(email.id);
                    break;
                }
            }
            let Some(id) = oldest else { return Ok(None) };
            if mark_read {
                db.execute("UPDATE emails SET read = 1 WHERE id = ?1", [id.0])?;
            }
            Ok(Some(id))
        })
        .await
    }

    /// Marks one Email read. Nothing happens if no Email has this id.
    pub(crate) async fn mark_read(&self, id: EmailId) -> Result<(), Error> {
        self.with_db(move |db| {
            db.execute("UPDATE emails SET read = 1 WHERE id = ?1", [id.0])?;
            Ok(())
        })
        .await
    }

    /// The view of one Email, or `None` if no Email has this id.
    pub(crate) async fn view(&self, id: EmailId) -> Result<Option<EmailView>, Error> {
        let email = self
            .with_db(move |db| {
                Ok(db
                    .query_row(
                        &format!("SELECT {SUMMARY_COLUMNS}, raw_source FROM emails WHERE id = ?1"),
                        [id.0],
                        |row| Ok((summary_from_row(row)?, row.get::<_, Vec<u8>>(10)?)),
                    )
                    .optional()?)
            })
            .await?;
        let Some((summary, raw_source)) = email else {
            return Ok(None);
        };
        // Parsing a large Email takes a while: keep it off the async workers
        // and outside the database lock.
        blocking(move || Ok(Some(EmailView::parse(summary, &raw_source)))).await
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

/// Runs the migration steps this Inbox has not run yet, all or none.
fn migrate(db: &mut Connection) -> Result<(), Error> {
    let transaction = db.transaction()?;
    let version: i64 = transaction.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    for (done, step) in (1..).zip(MIGRATIONS).skip(version.max(0) as usize) {
        transaction.execute_batch(step)?;
        transaction.pragma_update(None, "user_version", done)?;
    }
    transaction.commit()?;
    Ok(())
}

fn summary_from_row(row: &Row<'_>) -> rusqlite::Result<EmailSummary> {
    Ok(EmailSummary {
        id: EmailId(row.get(0)?),
        received_at: from_unix_nanos(row.get(1)?),
        envelope_sender: row.get(2)?,
        envelope_recipients: decode_addresses(&row.get::<_, String>(3)?),
        from: row.get(4)?,
        to: decode_addresses(&row.get::<_, String>(5)?),
        cc: decode_addresses(&row.get::<_, String>(6)?),
        subject: row.get(7)?,
        size: row.get::<_, i64>(8)? as u64,
        read: row.get(9)?,
    })
}

/// The headers an [`EmailSummary`] carries, decoded once at Capture.
#[derive(Default)]
struct SummaryHeaders {
    from: Option<String>,
    to: Vec<String>,
    cc: Vec<String>,
    subject: Option<String>,
}

impl SummaryHeaders {
    fn parse(raw_source: &[u8]) -> Self {
        let Some(message) = mail_parser::MessageParser::default().parse_headers(raw_source) else {
            return Self::default();
        };
        Self {
            from: message
                .from()
                .and_then(|from| from.first())
                .and_then(|mailbox| mailbox.address())
                .map(str::to_owned),
            to: addresses(message.to()),
            cc: addresses(message.cc()),
            subject: message.subject().map(str::to_owned),
        }
    }
}

/// Every mailbox address in an address header, in order (names dropped).
fn addresses(list: Option<mail_parser::AddressList<'_>>) -> Vec<String> {
    list.map(|list| {
        list.mailboxes()
            .filter_map(|mailbox| mailbox.address())
            .map(str::to_owned)
            .collect()
    })
    .unwrap_or_default()
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    match tokio::task::spawn_blocking(f).await {
        Ok(result) => result,
        Err(join_error) => std::panic::resume_unwind(join_error.into_panic()),
    }
}

// Addresses never contain a line break (headers are unfolded when parsed),
// so one per line is unambiguous.
fn encode_addresses(addresses: &[String]) -> String {
    addresses.join("\n")
}

fn decode_addresses(encoded: &str) -> Vec<String> {
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
