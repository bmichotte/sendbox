//! The menu bar / notification-area icon: left-click toggles the window,
//! right-click opens the menu. It shows how many Emails are Unread and,
//! while a port is in use, an error state: a badge on the icon, and the
//! menu and tooltip say which port.

use std::sync::{Mutex, PoisonError};

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::{badge, inbox, window};

const TRAY: &str = "main";

const OPEN: &str = "open";
const CLEAR_ALL: &str = "clear-all";
const SETTINGS: &str = "settings";
const QUIT: &str = "quit";

/// What the tray shows, kept whole: on Windows the Unread count and the
/// error state share the icon and the tooltip, so each change redraws both.
/// Only the main thread uses it: see [`update`].
#[derive(Default)]
struct TrayState {
    unread: usize,
    ports_in_use: Vec<u16>,
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    app.manage(Mutex::new(TrayState::default()));
    let tray = TrayIconBuilder::with_id(TRAY)
        .tooltip("Sendbox")
        .menu(&menu(app, &[])?)
        .show_menu_on_left_click(false)
        // Hears the menu's items whichever menu `show_ports_in_use` put up.
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
    let tray = tray.icon(template_icon()).icon_as_template(true);
    #[cfg(not(target_os = "macos"))]
    let tray = tray.icon(app_icon());

    tray.build(app)?;
    Ok(())
}

/// The tray menu, with a "Port N in use" line atop it for each port in use.
fn menu(app: &AppHandle, ports_in_use: &[u16]) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::new(app)?;
    for port in ports_in_use {
        // Disabled: it only informs.
        let id = format!("port-{port}-in-use");
        menu.append(&MenuItem::with_id(
            app,
            id,
            in_use(*port),
            false,
            None::<&str>,
        )?)?;
    }
    if !ports_in_use.is_empty() {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }
    menu.append_items(&[
        &MenuItem::with_id(app, OPEN, "Open", true, None::<&str>)?,
        // The ellipsis says a confirmation comes first.
        &MenuItem::with_id(app, CLEAR_ALL, "Clear all…", true, None::<&str>)?,
        &MenuItem::with_id(app, SETTINGS, "Settings…", true, None::<&str>)?,
        &PredefinedMenuItem::separator(app)?,
        &MenuItem::with_id(app, QUIT, "Quit", true, None::<&str>)?,
    ])?;
    Ok(menu)
}

fn in_use(port: u16) -> String {
    format!("Port {port} in use")
}

/// Shows how many Emails are `unread`, and nothing more at 0. macOS shows
/// the count as text next to the menu bar icon; Windows cannot put text next
/// to a tray icon, so it draws the count on the icon and says it in the
/// tooltip.
pub fn show_unread_count(app: &AppHandle, unread: usize) {
    update(app, move |_, tray, state| {
        state.unread = unread;
        #[cfg(target_os = "macos")]
        tray.set_title((unread > 0).then(|| unread.to_string()))?;
        #[cfg(not(target_os = "macos"))]
        show_on_icon(tray, state)?;
        Ok(())
    });
}

/// Shows the error state while `ports` are in use, or clears it when there
/// are none.
pub fn show_ports_in_use(app: &AppHandle, ports: Vec<u16>) {
    update(app, move |app, tray, state| {
        tray.set_menu(Some(menu(app, &ports)?))?;
        state.ports_in_use = ports;
        #[cfg(target_os = "macos")]
        {
            let mut icon = template_icon();
            if !state.ports_in_use.is_empty() {
                // Only alpha counts in a template: an opaque badge, its "!" a hole.
                icon = badge::with_error_badge(&icon, [0, 0, 0, 0xFF], badge::CLEAR);
            }
            // `set_icon` would drop the template flag.
            tray.set_icon_with_as_template(Some(icon), true)?;
            tray.set_tooltip(Some(tooltip(state)))?;
        }
        #[cfg(not(target_os = "macos"))]
        show_on_icon(tray, state)?;
        Ok(())
    });
}

/// Makes `change` to the tray and to what it shows on the main thread: at
/// once from the main thread, otherwise as soon as the main thread is free,
/// without waiting for it. Changes show in the order they were made.
///
/// Off the main thread, each tray call waits for the main thread to make it,
/// so a thread holding the lock on what the tray shows while it calls the
/// tray could wait forever on a main thread waiting for that lock. Only the
/// main thread ever takes the lock.
fn update(
    app: &AppHandle,
    change: impl FnOnce(&AppHandle, &TrayIcon, &mut TrayState) -> tauri::Result<()> + Send + 'static,
) {
    let app = app.clone();
    let on_main_thread = app.clone();
    // A failed change leaves the tray as it was: there is nobody to tell.
    let _ = on_main_thread.run_on_main_thread(move || {
        let Some(tray) = app.tray_by_id(TRAY) else {
            return;
        };
        let state = app.state::<Mutex<TrayState>>();
        let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = change(&app, &tray, &mut state);
    });
}

/// The Windows icon and tooltip: the error state takes the icon over the
/// Unread count, which the tooltip still gives.
#[cfg(not(target_os = "macos"))]
fn show_on_icon(tray: &TrayIcon, state: &TrayState) -> tauri::Result<()> {
    let icon = if !state.ports_in_use.is_empty() {
        badge::with_error_badge(&app_icon(), [0xDC, 0x26, 0x26, 0xFF], [0xFF; 4])
    } else if state.unread > 0 {
        badge::with_count(&app_icon(), state.unread)
    } else {
        app_icon()
    };
    tray.set_icon(Some(icon))?;
    tray.set_tooltip(Some(tooltip(state)))
}

/// The ports in use, then (on Windows, which has no tray title) the Unread
/// count; just "Sendbox" when there is neither.
fn tooltip(state: &TrayState) -> String {
    #[cfg_attr(target_os = "macos", allow(unused_mut))]
    let mut lines: Vec<String> = state.ports_in_use.iter().copied().map(in_use).collect();
    #[cfg(not(target_os = "macos"))]
    if state.unread > 0 {
        lines.push(format!("{} unread", state.unread));
    }
    if lines.is_empty() {
        "Sendbox".to_owned()
    } else {
        lines.join(", ")
    }
}

#[cfg(target_os = "macos")]
fn template_icon() -> Image<'static> {
    tauri::include_image!("icons/tray-template.png")
}

/// The app icon the Windows tray shows, and draws the Unread count on. A
/// fixed image, rather than the window icon, whose size follows the display
/// scale.
#[cfg(not(target_os = "macos"))]
fn app_icon() -> Image<'static> {
    tauri::include_image!("icons/32x32.png")
}
