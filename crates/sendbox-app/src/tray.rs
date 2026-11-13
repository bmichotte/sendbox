//! The menu bar / notification-area icon: left-click toggles the window,
//! right-click opens the menu. It shows how many Emails are Unread and,
//! while a port is in use, an error state: a badge on the icon, and the
//! menu and tooltip say which port.

use std::sync::{Mutex, PoisonError};

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::{inbox, window};

const TRAY: &str = "main";

const OPEN: &str = "open";
const CLEAR_ALL: &str = "clear-all";
const SETTINGS: &str = "settings";
const QUIT: &str = "quit";

/// What the tray shows, kept whole: on Windows the Unread count and the
/// error state share the icon and the tooltip, so each change redraws both.
/// Only the main thread uses it: see [`update`].
#[derive(Default)]
struct Shown {
    unread: usize,
    ports_in_use: Vec<u16>,
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    app.manage(Mutex::new(Shown::default()));
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
    update(app, move |_, tray, shown| {
        shown.unread = unread;
        #[cfg(target_os = "macos")]
        tray.set_title((unread > 0).then(|| unread.to_string()))?;
        #[cfg(not(target_os = "macos"))]
        show_on_icon(tray, shown)?;
        Ok(())
    });
}

/// Shows the error state while `ports` are in use, or clears it when there
/// are none.
pub fn show_ports_in_use(app: &AppHandle, ports: Vec<u16>) {
    update(app, move |app, tray, shown| {
        tray.set_menu(Some(menu(app, &ports)?))?;
        shown.ports_in_use = ports;
        #[cfg(target_os = "macos")]
        {
            let mut icon = template_icon();
            if !shown.ports_in_use.is_empty() {
                // Only alpha counts in a template: an opaque badge, its "!" a hole.
                icon = with_error_badge(&icon, [0, 0, 0, 0xFF], CLEAR);
            }
            // `set_icon` would drop the template flag.
            tray.set_icon_with_as_template(Some(icon), true)?;
            tray.set_tooltip(Some(tooltip(shown)))?;
        }
        #[cfg(not(target_os = "macos"))]
        show_on_icon(tray, shown)?;
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
    change: impl FnOnce(&AppHandle, &TrayIcon, &mut Shown) -> tauri::Result<()> + Send + 'static,
) {
    let app = app.clone();
    let on_main_thread = app.clone();
    // A failed change leaves the tray as it was: there is nobody to tell.
    let _ = on_main_thread.run_on_main_thread(move || {
        let Some(tray) = app.tray_by_id(TRAY) else {
            return;
        };
        let shown = app.state::<Mutex<Shown>>();
        let mut shown = shown.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = change(&app, &tray, &mut shown);
    });
}

/// The Windows icon and tooltip: the error state takes the icon over the
/// Unread count, which the tooltip still gives.
#[cfg(not(target_os = "macos"))]
fn show_on_icon(tray: &TrayIcon, shown: &Shown) -> tauri::Result<()> {
    let icon = if !shown.ports_in_use.is_empty() {
        with_error_badge(&app_icon(), [0xDC, 0x26, 0x26, 0xFF], [0xFF; 4])
    } else if shown.unread > 0 {
        crate::badge::with_count(&app_icon(), shown.unread)
    } else {
        app_icon()
    };
    tray.set_icon(Some(icon))?;
    tray.set_tooltip(Some(tooltip(shown)))
}

/// The ports in use, then (on Windows, which has no tray title) the Unread
/// count; just "Sendbox" when there is neither.
fn tooltip(shown: &Shown) -> String {
    #[cfg_attr(target_os = "macos", allow(unused_mut))]
    let mut lines: Vec<String> = shown.ports_in_use.iter().copied().map(in_use).collect();
    #[cfg(not(target_os = "macos"))]
    if shown.unread > 0 {
        lines.push(format!("{} unread", shown.unread));
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

const CLEAR: [u8; 4] = [0; 4];

/// `icon` with a round badge in its bottom-right corner, `fill` with a `mark`
/// coloured "!", set apart from the icon by a transparent ring.
fn with_error_badge(icon: &Image<'_>, fill: [u8; 4], mark: [u8; 4]) -> Image<'static> {
    let (width, height) = (icon.width(), icon.height());
    let size = width.min(height) as f32;
    let radius = size * 0.26;
    let ring = size * 0.06;
    let (center_x, center_y) = (width as f32 - radius, height as f32 - radius);
    let mut rgba = icon.rgba().to_vec();
    for y in 0..height {
        for x in 0..width {
            let (dx, dy) = (x as f32 + 0.5 - center_x, y as f32 + 0.5 - center_y);
            let distance = dx.hypot(dy);
            let pixel = if distance <= radius {
                if on_exclamation_mark(dx, dy, radius) {
                    mark
                } else {
                    fill
                }
            } else if distance <= radius + ring {
                CLEAR
            } else {
                continue;
            };
            let at = ((y * width + x) * 4) as usize;
            rgba[at..at + 4].copy_from_slice(&pixel);
        }
    }
    Image::new_owned(rgba, width, height)
}

/// Whether the point `(dx, dy)` from the centre of a badge of `radius` is on
/// its "!": a bar above a dot.
fn on_exclamation_mark(dx: f32, dy: f32, radius: f32) -> bool {
    let half_width = radius * 0.17;
    let bar = -radius * 0.62..=radius * 0.16;
    let dot = radius * 0.34..=radius * 0.34 + 2.0 * half_width;
    dx.abs() <= half_width && (bar.contains(&dy) || dot.contains(&dy))
}
