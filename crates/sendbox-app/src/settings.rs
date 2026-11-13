//! The Settings: what the user sets in the Settings window. They are kept as
//! JSON in the platform config directory, give the core its config at
//! startup, and are applied to the running core when they change.
//!
//! Launch at login is not in the file: the OS keeps it, and the user can
//! also change it there (System Settings, Task Manager), so it is read from
//! the OS each time.

use std::num::NonZeroUsize;
use std::path::PathBuf;

use sendbox_core::{Config, CoreHandle};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_autostart::ManagerExt as _;

use crate::inbox::Core;

const SETTINGS_FILE: &str = "settings.json";

/// Mirrors `Settings` in `ui/src/settings.ts`. A setting missing from the
/// file (one added in a later version, say) takes its default.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// The most Emails the Inbox keeps.
    retention_limit: NonZeroUsize,
    /// Whether an Email an agent reads over MCP becomes read.
    mcp_marks_read: bool,
}

impl Default for Settings {
    /// The core's defaults.
    fn default() -> Self {
        let defaults = Config::new(PathBuf::new());
        Self {
            retention_limit: defaults.retention_limit,
            mcp_marks_read: defaults.mcp_marks_read,
        }
    }
}

impl Settings {
    /// The core config these settings give, with the Inbox in `data_dir`.
    fn config(&self, data_dir: PathBuf) -> Config {
        Config {
            retention_limit: self.retention_limit,
            mcp_marks_read: self.mcp_marks_read,
            ..Config::new(data_dir)
        }
    }

    /// Applies these settings to a running core, without restarting it.
    async fn apply(&self, core: &CoreHandle) -> Result<(), sendbox_core::Error> {
        core.set_mcp_marks_read(self.mcp_marks_read);
        core.set_retention_limit(self.retention_limit).await
    }
}

/// The config to start the core with: the saved settings, with the Inbox in
/// the platform local data directory.
pub fn core_config(app: &AppHandle) -> tauri::Result<Config> {
    Ok(load(app).config(app.path().app_local_data_dir()?))
}

fn settings_path(app: &AppHandle) -> tauri::Result<PathBuf> {
    Ok(app.path().app_config_dir()?.join(SETTINGS_FILE))
}

/// The saved settings. A missing or unreadable file gives the defaults, so
/// that Sendbox always starts.
fn load(app: &AppHandle) -> Settings {
    settings_path(app)
        .ok()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let path = settings_path(app).map_err(|error| error.to_string())?;
    let json = serde_json::to_vec_pretty(settings).map_err(|error| error.to_string())?;
    // Written aside then renamed over the old file, so that a crash never
    // leaves a half-written one behind.
    let written = path.with_extension("json.tmp");
    let write = || -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&written, json)?;
        std::fs::rename(&written, &path)
    };
    write().map_err(|error| format!("Sendbox could not save its settings: {error}"))
}

/// The saved settings.
#[tauri::command]
pub async fn get_settings(app: AppHandle) -> Settings {
    load(&app)
}

/// Saves `settings` and applies them to the running core at once: a lower
/// Retention limit prunes the Inbox right away.
#[tauri::command]
pub async fn set_settings(
    app: AppHandle,
    core: State<'_, Core>,
    settings: Settings,
) -> Result<(), String> {
    save(&app, &settings)?;
    // Without a running core, the settings apply when it next starts.
    if let Core::Running(core) = &*core {
        settings
            .apply(core)
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Whether Sendbox starts when the user logs in.
#[tauri::command]
pub async fn launch_at_login(app: AppHandle) -> Result<bool, String> {
    app.autolaunch()
        .is_enabled()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn set_launch_at_login(app: AppHandle, enabled: bool) -> Result<(), String> {
    let autostart = app.autolaunch();
    if enabled {
        autostart.enable()
    } else {
        autostart.disable()
    }
    .map_err(|error| error.to_string())
}
