//! Bridges the core to the UI: starts the core, answers the UI's commands
//! from the core handle, and tells the UI when the Inbox changes.

use std::time::UNIX_EPOCH;

use sendbox_core::{Config, CoreEvent, CoreHandle, EmailFilter, EmailSummary};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::broadcast::error::RecvError;

/// Tells the UI to list the Inbox again.
const INBOX_CHANGED: &str = "inbox-changed";

/// The core as the window sees it: running, or the reason it could not start.
pub enum Core {
    Running(CoreHandle),
    Failed(String),
}

impl Core {
    fn handle(&self) -> Result<&CoreHandle, String> {
        match self {
            Core::Running(handle) => Ok(handle),
            Core::Failed(reason) => Err(reason.clone()),
        }
    }
}

/// Starts the core with its defaults (SMTP on 1025, MCP on 1026) and the
/// Inbox in the platform local data directory. If it cannot start, the app
/// still runs and the window shows why.
pub fn start(app: &AppHandle) -> tauri::Result<()> {
    let config = Config::new(app.path().app_local_data_dir()?);
    let core = match tauri::async_runtime::block_on(sendbox_core::start(config)) {
        Ok(handle) => {
            forward_events(app.clone(), &handle);
            Core::Running(handle)
        }
        Err(error) => Core::Failed(format!("Sendbox could not start Capturing: {error}")),
    };
    app.manage(core);
    Ok(())
}

fn forward_events(app: AppHandle, core: &CoreHandle) {
    let mut events = core.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            match events.recv().await {
                // Lagged: some events were missed, so the UI must re-list anyway.
                Ok(CoreEvent::EmailCaptured(_)) | Err(RecvError::Lagged(_)) => {
                    let _ = app.emit(INBOX_CHANGED, ());
                }
                Ok(_) => {}
                Err(RecvError::Closed) => break,
            }
        }
    });
}

/// One row of the Inbox list. Mirrors `EmailRow` in `ui/src/inbox.ts`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailRow {
    id: i64,
    /// Milliseconds since the Unix epoch.
    received_at: u64,
    sender: Option<String>,
    to: Vec<String>,
    subject: Option<String>,
}

impl From<EmailSummary> for EmailRow {
    fn from(email: EmailSummary) -> Self {
        Self {
            id: email.id.0,
            received_at: email
                .received_at
                .duration_since(UNIX_EPOCH)
                .map_or(0, |since| since.as_millis() as u64),
            sender: email.sender().map(str::to_owned),
            to: email.to,
            subject: email.subject,
        }
    }
}

/// Every Email in the Inbox, newest first.
#[tauri::command]
pub async fn list_emails(core: State<'_, Core>) -> Result<Vec<EmailRow>, String> {
    let emails = core
        .handle()?
        .list_emails(&EmailFilter::default(), None)
        .await
        .map_err(|error| error.to_string())?;
    Ok(emails.into_iter().map(EmailRow::from).collect())
}

/// The port the SMTP listener is bound to.
#[tauri::command]
pub fn smtp_port(core: State<'_, Core>) -> Result<u16, String> {
    Ok(core.handle()?.smtp_port())
}
