//! The menu bar / notification-area icon: left-click toggles the window,
//! right-click opens the menu. It shows how many Emails are Unread.

use tauri::AppHandle;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

use crate::{inbox, window};

const TRAY: &str = "main";

const OPEN: &str = "open";
const CLEAR_ALL: &str = "clear-all";
const SETTINGS: &str = "settings";
const QUIT: &str = "quit";

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let menu = Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, OPEN, "Open", true, None::<&str>)?,
            // The ellipsis says a confirmation comes first.
            &MenuItem::with_id(app, CLEAR_ALL, "Clear all…", true, None::<&str>)?,
            &MenuItem::with_id(app, SETTINGS, "Settings…", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, QUIT, "Quit", true, None::<&str>)?,
        ],
    )?;
    let tray = TrayIconBuilder::with_id(TRAY)
        .tooltip("Sendbox")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            OPEN => window::show(app),
            CLEAR_ALL => inbox::ask_to_clear_all(app),
            SETTINGS => window::show_settings(app),
            QUIT => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                window::toggle(tray.app_handle());
            }
        });

    // macOS draws a black-and-alpha template icon in the menu bar's own
    // colours; Windows shows the full-colour app icon.
    #[cfg(target_os = "macos")]
    let tray = tray
        .icon(tauri::include_image!("icons/tray-template.png"))
        .icon_as_template(true);
    #[cfg(not(target_os = "macos"))]
    let tray = tray.icon(app_icon());

    tray.build(app)?;
    Ok(())
}

/// Shows how many Emails are `unread`, and nothing more at 0. macOS shows
/// the count as text next to the menu bar icon; Windows cannot put text next
/// to a tray icon, so it draws the count on the icon and says it in the
/// tooltip.
pub fn show_unread_count(app: &AppHandle, unread: usize) -> tauri::Result<()> {
    let Some(tray) = app.tray_by_id(TRAY) else {
        return Ok(());
    };
    #[cfg(target_os = "macos")]
    tray.set_title((unread > 0).then(|| unread.to_string()))?;
    #[cfg(not(target_os = "macos"))]
    {
        let (icon, tooltip) = match unread {
            0 => (app_icon(), "Sendbox".to_owned()),
            unread => (
                crate::badge::with_count(&app_icon(), unread),
                format!("{unread} unread"),
            ),
        };
        tray.set_icon(Some(icon))?;
        tray.set_tooltip(Some(tooltip))?;
    }
    Ok(())
}

/// The app icon the Windows tray shows, and draws the Unread count on. A
/// fixed image, rather than the window icon, whose size follows the display
/// scale.
#[cfg(not(target_os = "macos"))]
fn app_icon() -> tauri::image::Image<'static> {
    tauri::include_image!("icons/32x32.png")
}
