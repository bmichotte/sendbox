//! The Inbox: every Captured Email, kept in a single SQLite file.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Row, params};
use tokio::sync::broadcast;

use crate::{
    AttachmentContent, CoreEvent, EmailFilter, EmailId, EmailSelection, EmailSummary, EmailView,
    Error, SenderTab,
};

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
    /// `None` for a null sender (`MAIL FROM:<>`).
    pub(crate) sender: Option<String>,
    pub(crate) recipients: Vec<String>,
}

#[derive(Clone)]
pub(crate) struct Inbox {
    db: Arc<Mutex<Connection>>,
    events: broadcast::Sender<CoreEvent>,
    /// The Retention limit, only read or changed while holding the database
    /// lock, so that each prune sees the latest limit.
    retention_limit: Arc<AtomicUsize>,
}

impl Inbox {
    /// Opens the Inbox in `data_dir`, creating it if needed, and prunes it
    /// to `retention_limit` (which may have been lowered since it was last
    /// open).
    pub(crate) async fn open(
        data_dir: &Path,
        retention_limit: NonZeroUsize,
    ) -> Result<Self, Error> {
        let path = data_dir.join(DATABASE_FILE);
        let data_dir = data_dir.to_path_buf();
        let db = blocking(move || -> Result<Connection, Error> {
            std::fs::create_dir_all(&data_dir)?;
            let mut db = Connection::open(path)?;
            migrate(&mut db)?;
            prune(&db, retention_limit.get())?;
            Ok(db)
        })
        .await?;
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            events: broadcast::channel(EVENT_BACKLOG).0,
            retention_limit: Arc::new(AtomicUsize::new(retention_limit.get())),
        })
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.events.subscribe()
    }

    /// Tells subscribers about something that happened, in the Inbox or
    /// outside it: they all listen here.
    pub(crate) fn notify(&self, event: CoreEvent) {
        // No subscriber is not an error: nobody is watching yet.
        let _ = self.events.send(event);
    }

    /// Keeps `raw_source` exactly as received, with its envelope, prunes the
    /// oldest Emails beyond the Retention limit, and emits
    /// [`CoreEvent::EmailCaptured`].
    pub(crate) async fn capture(
        &self,
        envelope: Envelope,
        raw_source: Vec<u8>,
    ) -> Result<EmailSummary, Error> {
        let received_at = to_the_millisecond(SystemTime::now());
        let headers = SummaryHeaders::parse(&raw_source);
        let retention_limit = Arc::clone(&self.retention_limit);
        let email = self
            .with_db(move |db| {
                // Kept and pruned together, or neither: a client told the
                // Capture failed may send the Email again.
                let transaction = db.unchecked_transaction()?;
                transaction.execute(
                    "INSERT INTO emails (received_at_ns, envelope_sender, envelope_recipients,
                                     from_address, to_addresses, cc_addresses, subject,
                                     size, raw_source)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        to_unix_nanos(received_at),
                        encode_sender(envelope.sender.as_deref()),
                        encode_addresses(&envelope.recipients),
                        headers.from,
                        encode_addresses(&headers.to),
                        encode_addresses(&headers.cc),
                        headers.subject,
                        raw_source.len() as i64,
                        raw_source,
                    ],
                )?;
                let id = EmailId(transaction.last_insert_rowid());
                prune(&transaction, retention_limit.load(Ordering::Relaxed))?;
                transaction.commit()?;
                Ok(EmailSummary {
                    id,
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
        self.notify(CoreEvent::EmailCaptured(email.clone()));
        Ok(email)
    }

    /// Keeps at most `limit` Emails from now on, pruning the oldest beyond
    /// it at once.
    pub(crate) async fn set_retention_limit(&self, limit: NonZeroUsize) -> Result<(), Error> {
        let retention_limit = Arc::clone(&self.retention_limit);
        let pruned = self
            .with_db(move |db| {
                retention_limit.store(limit.get(), Ordering::Relaxed);
                prune(db, limit.get())
            })
            .await?;
        if pruned > 0 {
            self.notify(CoreEvent::InboxChanged);
        }
        Ok(())
    }

    /// The Emails `filter` matches, newest first, at most `limit` of them.
    pub(crate) async fn list(
        &self,
        filter: EmailFilter,
        limit: Option<usize>,
    ) -> Result<Vec<EmailSummary>, Error> {
        self.with_db(move |db| select(db, &filter, limit)).await
    }

    /// The oldest Email `filter` matches, if any, marked read first if
    /// `mark_read` (emitting [`CoreEvent::InboxChanged`] if it was Unread).
    /// Finding and marking are one step, so concurrent callers looking for
    /// Unread Emails never get the same one.
    pub(crate) async fn oldest(
        &self,
        filter: EmailFilter,
        mark_read: bool,
    ) -> Result<Option<EmailId>, Error> {
        let (oldest, marked) = self
            .with_db(move |db| {
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
                let Some(id) = oldest else {
                    return Ok((None, false));
                };
                let marked = mark_read && mark_unread_read(db, id)?;
                Ok((Some(id), marked))
            })
            .await?;
        if marked {
            self.notify(CoreEvent::InboxChanged);
        }
        Ok(oldest)
    }

    /// One Sender tab per sender with Emails in the Inbox.
    pub(crate) async fn sender_tabs(&self) -> Result<Vec<SenderTab>, Error> {
        self.with_db(|db| {
            // Newest first, so each tab comes in the order of its most recent Email.
            let mut tabs: Vec<SenderTab> = Vec::new();
            // Where each key's tab is in `tabs`.
            let mut positions = HashMap::new();
            for email in select(db, &EmailFilter::default(), None)? {
                let key = email.sender_tab();
                let position = match positions.get(&key) {
                    Some(&position) => position,
                    None => {
                        positions.insert(key.clone(), tabs.len());
                        tabs.push(SenderTab { key, unread: 0 });
                        tabs.len() - 1
                    }
                };
                if !email.read {
                    tabs[position].unread += 1;
                }
            }
            Ok(tabs)
        })
        .await
    }

    /// How many Emails are Unread.
    pub(crate) async fn unread_count(&self) -> Result<usize, Error> {
        self.with_db(|db| {
            let count: i64 =
                db.query_row("SELECT COUNT(*) FROM emails WHERE read = 0", [], |row| {
                    row.get(0)
                })?;
            Ok(count as usize)
        })
        .await
    }

    /// Marks one Email read and emits [`CoreEvent::InboxChanged`] if it was
    /// Unread. Nothing happens if no Email has this id.
    pub(crate) async fn mark_read(&self, id: EmailId) -> Result<(), Error> {
        let marked = self.with_db(move |db| mark_unread_read(db, id)).await?;
        if marked {
            self.notify(CoreEvent::InboxChanged);
        }
        Ok(())
    }

    /// Deletes the `selected` Emails and returns how many there were. Emits
    /// [`CoreEvent::InboxChanged`] if there were any.
    pub(crate) async fn delete(&self, selected: EmailSelection) -> Result<usize, Error> {
        let deleted = self
            .with_db(move |db| {
                // Selected under the same lock as the deletion, so an Email
                // Captured meanwhile is never deleted unseen.
                let ids = match selected {
                    EmailSelection::All => return Ok(db.execute("DELETE FROM emails", [])?),
                    EmailSelection::Ids(ids) => ids,
                    EmailSelection::Matching(filter) => select(db, &filter, None)?
                        .into_iter()
                        .map(|email| email.id)
                        .collect(),
                };
                delete_ids(db, &ids)
            })
            .await?;
        if deleted > 0 {
            self.notify(CoreEvent::InboxChanged);
        }
        Ok(deleted)
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

    /// The content of one Email's attachment, or `None` if there is no such
    /// Email or attachment.
    pub(crate) async fn attachment(
        &self,
        id: EmailId,
        index: usize,
    ) -> Result<Option<AttachmentContent>, Error> {
        let Some(raw_source) = self.raw_source(id).await? else {
            return Ok(None);
        };
        // Decoding a large attachment takes a while, as parsing does.
        blocking(move || Ok(AttachmentContent::parse(&raw_source, index))).await
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

/// Deletes the oldest Emails, read or not, so that at most `limit` remain.
/// Ids grow with each Capture, so the oldest have the lowest ids.
fn prune(db: &Connection, limit: usize) -> Result<usize, Error> {
    // With `limit` Emails or fewer, the subquery is NULL and nothing matches.
    Ok(db.execute(
        "DELETE FROM emails
         WHERE id <= (SELECT id FROM emails ORDER BY id DESC LIMIT 1 OFFSET ?1)",
        [limit as i64],
    )?)
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

/// The Emails `filter` matches, newest first, at most `limit` of them.
///
/// Filtering happens here rather than in SQL: SQLite only folds ASCII case,
/// and the Inbox is small (bounded by the Retention limit).
fn select(
    db: &Connection,
    filter: &EmailFilter,
    limit: Option<usize>,
) -> Result<Vec<EmailSummary>, Error> {
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
}

/// Marks the Email with this id read, and returns whether it was Unread.
fn mark_unread_read(db: &Connection, id: EmailId) -> Result<bool, Error> {
    let marked = db.execute(
        "UPDATE emails SET read = 1 WHERE id = ?1 AND read = 0",
        [id.0],
    )?;
    Ok(marked > 0)
}

/// Deletes the Emails with these ids, all or none, and returns how many
/// there were.
fn delete_ids(db: &Connection, ids: &[EmailId]) -> Result<usize, Error> {
    let transaction = db.unchecked_transaction()?;
    let mut deleted = 0;
    {
        let mut statement = transaction.prepare("DELETE FROM emails WHERE id = ?1")?;
        for id in ids {
            deleted += statement.execute([id.0])?;
        }
    }
    transaction.commit()?;
    Ok(deleted)
}

fn summary_from_row(row: &Row<'_>) -> rusqlite::Result<EmailSummary> {
    Ok(EmailSummary {
        id: EmailId(row.get(0)?),
        received_at: from_unix_nanos(row.get(1)?),
        envelope_sender: decode_sender(row.get(2)?),
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

// A null sender is stored as an empty address, which no other sender has.
fn encode_sender(sender: Option<&str>) -> &str {
    sender.unwrap_or_default()
}

fn decode_sender(encoded: String) -> Option<String> {
    (!encoded.is_empty()).then_some(encoded)
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

/// Emails Captured before Capture times were kept to the millisecond have
/// more precision: reading drops it.
fn from_unix_nanos(nanos: i64) -> SystemTime {
    to_the_millisecond(UNIX_EPOCH + Duration::from_nanos(nanos.max(0) as u64))
}

/// `time` without its fraction of a millisecond.
fn to_the_millisecond(time: SystemTime) -> SystemTime {
    let since_epoch = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    UNIX_EPOCH + Duration::from_millis(since_epoch.as_millis() as u64)
}
