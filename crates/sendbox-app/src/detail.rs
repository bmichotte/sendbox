//! The Email detail: one Email as the window shows it (HTML, text, headers
//! with the envelope, raw source, attachments), and saving an attachment
//! through a native save dialog.

use std::path::Path;

use sendbox_core::{Attachment, EmailId, EmailView, Header};
use serde::Serialize;
use tauri::{State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;

use crate::inbox::{Core, EmailRow};

/// One Email in full. Mirrors `EmailDetail` in `ui/src/inbox.ts`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailDetail {
    #[serde(flatten)]
    row: EmailRow,
    /// Addresses in the `Cc` header.
    cc: Vec<String>,
    /// `MAIL FROM`; `None` for a null sender (`<>`).
    envelope_sender: Option<String>,
    /// `RCPT TO`, Bcc included.
    envelope_recipients: Vec<String>,
    text: Option<String>,
    /// The HTML body with its inline images embedded, ready for the frame.
    html: Option<String>,
    headers: Vec<HeaderField>,
    attachments: Vec<AttachmentInfo>,
    /// The raw source as text, every byte kept (see the core's view).
    raw_source: String,
}

#[derive(Serialize)]
pub struct HeaderField {
    name: String,
    value: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentInfo {
    name: Option<String>,
    content_type: String,
    size: u64,
}

impl EmailDetail {
    fn new(view: EmailView) -> Self {
        let EmailView {
            summary,
            text,
            html_with_inline_images,
            headers,
            attachments,
            raw_source_text,
            ..
        } = view;
        Self {
            cc: summary.cc.clone(),
            envelope_sender: summary.envelope_sender.clone(),
            envelope_recipients: summary.envelope_recipients.clone(),
            row: EmailRow::from(summary),
            text,
            html: html_with_inline_images,
            headers: headers
                .into_iter()
                .map(|Header { name, value }| HeaderField { name, value })
                .collect(),
            attachments: attachments
                .into_iter()
                .map(
                    |Attachment {
                         name,
                         content_type,
                         size,
                     }| AttachmentInfo {
                        name,
                        content_type,
                        size,
                    },
                )
                .collect(),
            raw_source: raw_source_text,
        }
    }
}

/// One Email in full, opened in the window: it becomes read, whether or not
/// MCP reads mark Emails read, and the UI and the tray hear it.
#[tauri::command]
pub async fn get_email(core: State<'_, Core>, id: i64) -> Result<EmailDetail, String> {
    let core = core.handle()?;
    let id = EmailId(id);
    core.mark_read(id)
        .await
        .map_err(|error| error.to_string())?;
    core.email(id)
        .await
        .map_err(|error| error.to_string())?
        .map(EmailDetail::new)
        .ok_or_else(|| "This Email is no longer in the Inbox.".to_owned())
}

/// Asks where to save attachment `index` of Email `email_id`, then writes
/// it there. `false` if the user cancelled the dialog.
#[tauri::command]
pub async fn save_attachment(
    window: WebviewWindow,
    core: State<'_, Core>,
    email_id: i64,
    index: usize,
) -> Result<bool, String> {
    let content = core
        .handle()?
        .attachment(EmailId(email_id), index)
        .await
        .map_err(|error| error.to_string())?
        .ok_or("This attachment is no longer in the Inbox.")?;
    let (chosen, path) = tokio::sync::oneshot::channel();
    // The callback form: the blocking one would park a worker until the
    // user answers.
    window
        .dialog()
        .file()
        .set_parent(&window)
        .set_file_name(file_name(content.attachment.name.as_deref()))
        .save_file(move |path| {
            let _ = chosen.send(path);
        });
    let Some(path) = path.await.map_err(|error| error.to_string())? else {
        return Ok(false);
    };
    let path = path.into_path().map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || std::fs::write(path, content.bytes))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| format!("Could not save the attachment: {error}"))?;
    Ok(true)
}

/// The name the save dialog suggests. It comes from the Email, so any
/// directory part and any character Windows forbids in file names go.
fn file_name(name: Option<&str>) -> String {
    let base = name
        .and_then(|name| Path::new(name).file_name())
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let cleaned: String = base
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    if cleaned.trim().is_empty() {
        "attachment".to_owned()
    } else {
        cleaned
    }
}
