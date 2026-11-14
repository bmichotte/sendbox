//! Bridges the core to the UI: starts the core, answers the UI's commands
//! from the core handle, and tells the UI and the tray when the Inbox changes.

use std::time::UNIX_EPOCH;

use sendbox_core::{
    Config, CoreEvent, CoreHandle, EmailFilter, EmailId, EmailSelection, EmailSummary, SenderTab,
    SenderTabKey,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};
use tauri_plugin_dialog::{
    DialogExt, MessageDialogBuilder, MessageDialogButtons, MessageDialogKind,
};
use tokio::sync::broadcast::Receiver;
use tokio::sync::oneshot;

use crate::{core_events, tray, window};

/// Tells the UI to list the Inbox again.
const INBOX_CHANGED: &str = "inbox-changed";

/// The core as the window sees it: running, or the reason it could not start.
pub enum Core {
    Running(CoreHandle),
    Failed(String),
}

impl Core {
    pub fn handle(&self) -> Result<&CoreHandle, String> {
        match self {
            Core::Running(handle) => Ok(handle),
            Core::Failed(reason) => Err(reason.clone()),
        }
    }
}

/// Starts the core with `config` and keeps the UI and the tray (created
/// first) up to date. If it cannot start, the app still runs and the window
/// shows why.
pub fn start(app: &AppHandle, config: Config) {
    let core = match tauri::async_runtime::block_on(sendbox_core::start(config)) {
        Ok(handle) => Core::Running(handle),
        Err(error) => Core::Failed(format!("Sendbox could not start Capturing: {error}")),
    };
    // Subscribed before the core is managed, so no event is missed; forwarded
    // after, so the task can count Unread Emails through the managed core.
    let events = core.handle().ok().map(CoreHandle::subscribe);
    app.manage(core);
    if let Some(events) = events {
        forward_events(app.clone(), events);
    }
}

fn forward_events(app: AppHandle, mut events: Receiver<CoreEvent>) {
    tauri::async_runtime::spawn(async move {
        show_unread_count(&app).await;
        let inbox_changed = |event: &CoreEvent| {
            matches!(event, CoreEvent::EmailCaptured(_) | CoreEvent::InboxChanged)
        };
        while core_events::changed(&mut events, inbox_changed).await {
            let _ = app.emit(INBOX_CHANGED, ());
            show_unread_count(&app).await;
        }
    });
}

/// Shows the Inbox's Unread count on the tray icon. If the core cannot
/// count, the tray keeps the last count it showed.
async fn show_unread_count(app: &AppHandle) {
    let core = app.state::<Core>();
    let Ok(core) = core.handle() else { return };
    if let Ok(unread) = core.unread_count().await {
        tray::show_unread_count(app, unread);
    }
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
    /// False while the Email is Unread.
    read: bool,
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
            read: email.read,
        }
    }
}

/// Which Sender tab: the one of `address`, or "(no sender)" for none.
/// Mirrors `SenderTabKey` in `ui/src/inbox.ts`.
#[derive(Deserialize)]
pub struct TabKey {
    address: Option<String>,
}

impl From<TabKey> for SenderTabKey {
    fn from(tab: TabKey) -> Self {
        match tab.address {
            Some(address) => SenderTabKey::Address(address),
            None => SenderTabKey::NoSender,
        }
    }
}

/// The Emails of `tab` (every Email for "All") whose sender, recipients or
/// subject contain `search` (any Email for an empty one), newest first.
#[tauri::command]
pub async fn list_emails(
    core: State<'_, Core>,
    tab: Option<TabKey>,
    search: String,
) -> Result<Vec<EmailRow>, String> {
    let filter = EmailFilter {
        search: Some(search),
        sender_tab: tab.map(SenderTabKey::from),
        ..EmailFilter::default()
    };
    let emails = core
        .handle()?
        .list_emails(&filter, None)
        .await
        .map_err(|error| error.to_string())?;
    Ok(emails.into_iter().map(EmailRow::from).collect())
}

/// The tabs above the list: "All" and the Sender tabs. Mirrors `Tabs` in
/// `ui/src/inbox.ts`.
#[derive(Serialize)]
pub struct Tabs {
    /// How many Emails in the Inbox are Unread, for "All".
    unread: usize,
    /// Ordered by their most recent Email.
    senders: Vec<SenderTabRow>,
}

/// One Sender tab. Mirrors `SenderTab` in `ui/src/inbox.ts`.
#[derive(Serialize)]
pub struct SenderTabRow {
    /// The sender's address, lowercased; `None` for "(no sender)".
    address: Option<String>,
    unread: usize,
}

impl From<SenderTab> for SenderTabRow {
    fn from(tab: SenderTab) -> Self {
        Self {
            address: match tab.key {
                SenderTabKey::Address(address) => Some(address),
                SenderTabKey::NoSender => None,
            },
            unread: tab.unread,
        }
    }
}

/// "All" and the Sender tabs, each with its Unread count.
#[tauri::command]
pub async fn tabs(core: State<'_, Core>) -> Result<Tabs, String> {
    let core = core.handle()?;
    let senders = core
        .sender_tabs()
        .await
        .map_err(|error| error.to_string())?;
    let unread = core
        .unread_count()
        .await
        .map_err(|error| error.to_string())?;
    Ok(Tabs {
        unread,
        senders: senders.into_iter().map(SenderTabRow::from).collect(),
    })
}

/// Deletes one Email. The UI hears that the Inbox changed.
#[tauri::command]
pub async fn delete_email(core: State<'_, Core>, id: i64) -> Result<(), String> {
    core.handle()?
        .delete_emails(EmailSelection::Ids(vec![EmailId(id)]))
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Clear all from the window: asks for confirmation on the window, then
/// deletes every Email. Returns whether the Inbox was cleared.
#[tauri::command]
pub async fn clear_all(window: WebviewWindow, core: State<'_, Core>) -> Result<bool, String> {
    let core = core.handle()?;
    let (answer, answered) = oneshot::channel();
    clear_all_dialog(window.app_handle())
        .parent(&window)
        .show(move |confirmed| {
            let _ = answer.send(confirmed);
        });
    if !answered.await.unwrap_or(false) {
        return Ok(false);
    }
    core.delete_emails(EmailSelection::All)
        .await
        .map_err(|error| error.to_string())?;
    Ok(true)
}

/// Clear all from the tray menu: asks for confirmation, then deletes every
/// Email. Menu handlers run on the main thread, which the dialog needs, so
/// this never waits for the answer.
pub fn ask_to_clear_all(app: &AppHandle) {
    // Choosing a tray menu item does not bring a menu-bar app to the front,
    // so a dialog on its own could open behind other apps: show it on the
    // Inbox window, which also lets the user see what is being cleared.
    window::show(app);
    let mut dialog = clear_all_dialog(app);
    if let Some(window) = window::main_window(app) {
        dialog = dialog.parent(&window);
    }
    let app = app.clone();
    dialog.show(move |confirmed| {
        if !confirmed {
            return;
        }
        tauri::async_runtime::spawn(async move {
            let cleared = match app.state::<Core>().handle() {
                Ok(core) => core
                    .delete_emails(EmailSelection::All)
                    .await
                    .map_err(|error| error.to_string()),
                Err(reason) => Err(reason),
            };
            if let Err(reason) = cleared {
                app.dialog()
                    .message(format!("The Inbox could not be cleared: {reason}"))
                    .kind(MessageDialogKind::Error)
                    .show(|_| {});
            }
        });
    });
}

/// The confirmation both Clear alls ask for, worded once.
fn clear_all_dialog(app: &AppHandle) -> MessageDialogBuilder<tauri::Wry> {
    app.dialog()
        .message("Delete every Email in the Inbox? This cannot be undone.")
        .title("Clear all Emails?")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Clear all".to_string(),
            "Cancel".to_string(),
        ))
}
