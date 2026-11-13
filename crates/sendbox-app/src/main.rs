//! Sendbox: the thin Tauri shell around the core. It lives in the menu bar
//! (macOS) / notification area (Windows), owns the window, and bridges the
//! core handle to the React UI. All behaviour lives in `sendbox-core`.

// No console window next to the app on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod detail;
mod inbox;
mod tray;
mod window;

use tauri::{AppHandle, RunEvent, WindowEvent};
use tauri_plugin_window_state::StateFlags;

fn main() {
    let app = tauri::Builder::default()
        // Registered first so a second instance exits before doing anything else.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            window::show(app);
        }))
        .plugin(
            tauri_plugin_window_state::Builder::default()
                // Not VISIBLE: the window stays hidden at launch, like any tray app.
                .with_state_flags(StateFlags::SIZE | StateFlags::POSITION)
                .build(),
        )
        // Save dialogs for attachments; only Rust shows them, so the window
        // gets no dialog permission.
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            inbox::list_emails,
            inbox::smtp_port,
            detail::get_email,
            detail::save_attachment
        ])
        .setup(|app| {
            // No Dock icon. `LSUIElement` in Info.plist avoids the flash at launch
            // of a bundled app; this covers `tauri dev`, which runs a bare binary.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            #[cfg(target_os = "macos")]
            app.set_menu(window::macos_app_menu(app.handle())?)?;
            inbox::start(app.handle())?;
            tray::create(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing (close button, Cmd+W, Alt+F4) only hides: Capture goes on.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("Sendbox failed to build its Tauri app");

    app.run(on_run_event);
}

#[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
fn on_run_event(app: &AppHandle, event: RunEvent) {
    match event {
        // Only the tray menu's Quit exits, with an explicit code.
        RunEvent::ExitRequested {
            code: None, api, ..
        } => api.prevent_exit(),
        // Launching the app again from Finder while it runs.
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => window::show(app),
        _ => {}
    }
}
