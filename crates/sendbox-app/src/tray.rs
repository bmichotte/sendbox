//! The menu bar / notification-area icon: left-click toggles the window,
//! right-click opens the menu. While a port is in use, the icon wears an
//! error badge and the menu and tooltip say which port.

use tauri::AppHandle;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

use crate::{inbox, window};

const TRAY: &str = "main";

const OPEN: &str = "open";
const CLEAR_ALL: &str = "clear-all";
const SETTINGS: &str = "settings";
const QUIT: &str = "quit";

pub fn create(app: &AppHandle) -> tauri::Result<()> {
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
    let tray = tray
        .icon(tauri::include_image!("icons/tray-template.png"))
        .icon_as_template(true);
    #[cfg(not(target_os = "macos"))]
    let tray = match app.default_window_icon() {
        Some(icon) => tray.icon(icon.clone()),
        None => tray,
    };

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

/// Shows the error state while `ports` are in use, or clears it when there
/// are none.
pub fn show_ports_in_use(app: &AppHandle, ports: &[u16]) -> tauri::Result<()> {
    let Some(tray) = app.tray_by_id(TRAY) else {
        return Ok(());
    };
    tray.set_menu(Some(menu(app, ports)?))?;
    let tooltip = if ports.is_empty() {
        "Sendbox".to_string()
    } else {
        let problems: Vec<String> = ports.iter().copied().map(in_use).collect();
        format!("Sendbox: {}", problems.join(", "))
    };
    tray.set_tooltip(Some(tooltip))?;

    #[cfg(target_os = "macos")]
    {
        let mut icon = tauri::include_image!("icons/tray-template.png");
        if !ports.is_empty() {
            // Only alpha counts in a template: an opaque badge, its "!" a hole.
            icon = with_error_badge(&icon, [0, 0, 0, 0xFF], CLEAR);
        }
        // `set_icon` would drop the template flag.
        tray.set_icon_with_as_template(Some(icon), true)?;
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(icon) = app.default_window_icon() {
        let icon = if ports.is_empty() {
            icon.clone()
        } else {
            with_error_badge(icon, [0xDC, 0x26, 0x26, 0xFF], [0xFF; 4])
        };
        tray.set_icon(Some(icon))?;
    }
    Ok(())
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
