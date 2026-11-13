//! The Inbox window: declared hidden in `tauri.conf.json`, shown and hidden
//! from the tray, never destroyed while the app runs.

use tauri::{AppHandle, Manager, WebviewWindow};

/// Hides the window if it is in front of the user, brings it there otherwise.
pub fn toggle(app: &AppHandle) {
    let Some(window) = main_window(app) else {
        return;
    };
    let on_screen = window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(false);
    // Clicking the notification area takes focus away from the window before
    // the click arrives on Windows, so only macOS can tell "behind other
    // windows" from "in front".
    let in_front =
        on_screen && (cfg!(not(target_os = "macos")) || window.is_focused().unwrap_or(false));
    if in_front {
        let _ = window.hide();
    } else {
        bring_to_front(&window);
    }
}

/// Brings the window to the front, focused.
pub fn show(app: &AppHandle) {
    if let Some(window) = main_window(app) {
        bring_to_front(&window);
    }
}

pub fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
    // The label of the window declared in `tauri.conf.json`.
    app.get_webview_window("main")
}

fn bring_to_front(window: &WebviewWindow) {
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
}

/// The menu macOS uses for keyboard shortcuts (it is never displayed: the
/// app has no menu bar). It keeps Cmd+W and the editing shortcuts, and leaves
/// out Quit so that Cmd+Q cannot stop Capture: quitting is from the tray.
#[cfg(target_os = "macos")]
pub fn macos_app_menu(app: &AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, PredefinedMenuItem as Item, Submenu};

    Menu::with_items(
        app,
        &[
            &Submenu::with_items(app, "Sendbox", true, &[&Item::close_window(app, None)?])?,
            &Submenu::with_items(
                app,
                "Edit",
                true,
                &[
                    &Item::undo(app, None)?,
                    &Item::redo(app, None)?,
                    &Item::separator(app)?,
                    &Item::cut(app, None)?,
                    &Item::copy(app, None)?,
                    &Item::paste(app, None)?,
                    &Item::select_all(app, None)?,
                ],
            )?,
        ],
    )
}
