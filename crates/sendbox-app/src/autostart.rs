//! Launch at login. The OS keeps it, not the Settings file, and the user can
//! also change it there, so it is read from the OS each time.
//!
//! On macOS, Sendbox is the main app login item of `SMAppService` (macOS 13+):
//! it shows under System Settings › General › Login Items › Open at Login,
//! where the user can remove it, and it still works once the app is moved.
//! On Windows, the autostart plugin writes the `HKCU` `Run` key, which Task
//! Manager's startup apps toggle also turns off.

use serde::Serialize;
use tauri::AppHandle;

#[cfg(target_os = "macos")]
pub use macos::{get, set};
#[cfg(not(target_os = "macos"))]
pub use plugin::{get, set};

/// Whether Sendbox starts when the user logs in. Mirrors `LaunchAtLogin` in
/// `ui/src/settings.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LaunchAtLogin {
    Off,
    On,
    /// Turned on, but macOS waits for the user to allow it in System Settings
    /// before it launches Sendbox. Only macOS asks.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    NeedsApproval,
}

#[cfg(target_os = "macos")]
mod macos {
    use std::path::PathBuf;

    use smappservice_rs::{AppService, ServiceStatus, ServiceType};
    use tauri::Manager;

    use super::{AppHandle, LaunchAtLogin};

    pub fn get(app: &AppHandle) -> Result<LaunchAtLogin, String> {
        let legacy_agent = legacy_agent(app).is_some_and(|plist| plist.exists());
        Ok(state(
            AppService::new(ServiceType::MainApp).status(),
            legacy_agent,
        ))
    }

    pub fn set(app: &AppHandle, enabled: bool) -> Result<(), String> {
        // Either way, so that Sendbox never launches twice, nor once more
        // after it is turned off.
        remove_legacy_agent(app)?;
        let service = AppService::new(ServiceType::MainApp);
        let changed = if enabled {
            service.register()
        } else {
            service.unregister()
        };
        let status = service.status();
        if let Err(error) = changed
            && !is_as_asked(enabled, status)
        {
            let change = if enabled { "turn on" } else { "turn off" };
            return Err(format!(
                "Sendbox could not {change} launch at login: {error}"
            ));
        }
        // macOS waits for the user to allow it: take them where they do. The
        // Settings window says so too.
        if enabled && status == ServiceStatus::RequiresApproval {
            AppService::open_system_settings_login_items();
        }
        Ok(())
    }

    fn remove_legacy_agent(app: &AppHandle) -> Result<(), String> {
        let Some(plist) = legacy_agent(app) else {
            return Ok(());
        };
        match std::fs::remove_file(&plist) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(format!(
                "Sendbox could not remove its old launch agent {}: {error}",
                plist.display()
            )),
            _ => Ok(()),
        }
    }

    /// The LaunchAgent that earlier versions wrote through the autostart
    /// plugin, named after the app; None without a home directory.
    fn legacy_agent(app: &AppHandle) -> Option<PathBuf> {
        let home = app.path().home_dir().ok()?;
        let name = &app.package_info().name;
        Some(
            home.join("Library/LaunchAgents")
                .join(format!("{name}.plist")),
        )
    }

    /// Whether Sendbox starts at login, given the status of its login item
    /// and whether the LaunchAgent of an earlier version is still there: that
    /// one launches it too.
    fn state(status: ServiceStatus, legacy_agent: bool) -> LaunchAtLogin {
        match status {
            ServiceStatus::Enabled => LaunchAtLogin::On,
            _ if legacy_agent => LaunchAtLogin::On,
            ServiceStatus::RequiresApproval => LaunchAtLogin::NeedsApproval,
            ServiceStatus::NotRegistered | ServiceStatus::NotFound => LaunchAtLogin::Off,
        }
    }

    /// Whether the login item is registered when `enabled`, and gone when
    /// not. Registering it twice, or unregistering it once the user removed
    /// it in System Settings, fails but leaves it as asked.
    fn is_as_asked(enabled: bool, status: ServiceStatus) -> bool {
        let registered = matches!(
            status,
            ServiceStatus::Enabled | ServiceStatus::RequiresApproval
        );
        registered == enabled
    }

    #[cfg(test)]
    mod tests {
        use smappservice_rs::ServiceStatus;

        use super::*;

        #[test]
        fn an_enabled_login_item_is_on() {
            assert_eq!(state(ServiceStatus::Enabled, false), LaunchAtLogin::On);
        }

        #[test]
        fn a_login_item_waiting_for_approval_says_so() {
            assert_eq!(
                state(ServiceStatus::RequiresApproval, false),
                LaunchAtLogin::NeedsApproval
            );
        }

        #[test]
        fn a_launch_agent_left_by_an_earlier_version_is_on() {
            assert_eq!(state(ServiceStatus::NotRegistered, true), LaunchAtLogin::On);
        }

        #[test]
        fn turning_off_a_login_item_the_user_removed_is_done() {
            assert!(is_as_asked(false, ServiceStatus::NotRegistered));
        }

        #[test]
        fn turning_on_a_login_item_that_stays_unregistered_fails() {
            assert!(!is_as_asked(true, ServiceStatus::NotRegistered));
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod plugin {
    use tauri_plugin_autostart::ManagerExt as _;

    use super::{AppHandle, LaunchAtLogin};

    pub fn get(app: &AppHandle) -> Result<LaunchAtLogin, String> {
        let enabled = app
            .autolaunch()
            .is_enabled()
            .map_err(|error| error.to_string())?;
        Ok(if enabled {
            LaunchAtLogin::On
        } else {
            LaunchAtLogin::Off
        })
    }

    pub fn set(app: &AppHandle, enabled: bool) -> Result<(), String> {
        let autostart = app.autolaunch();
        if enabled {
            autostart.enable()
        } else {
            autostart.disable()
        }
        .map_err(|error| error.to_string())
    }
}
