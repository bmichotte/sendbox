//! Shows whether the SMTP listener and the MCP endpoint are listening: the
//! tray's error state when a port is in use, and the windows hear of every
//! change.

use sendbox_core::{CoreEvent, CoreHandle, Listener, ListenerStatus};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::inbox::Core;
use crate::{core_events, tray};

/// Tells the windows the listeners' status changed; carries [`Listeners`].
const LISTENERS_CHANGED: &str = "listeners-changed";

/// Both listeners' status. Mirrors `Listeners` in `ui/src/listeners.ts`.
#[derive(Clone, Serialize)]
pub struct Listeners {
    smtp: Status,
    mcp: Status,
}

/// Mirrors `ListenerStatus` in `ui/src/listeners.ts`.
#[derive(Clone, Copy, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
enum Status {
    Running { port: u16 },
    PortInUse { port: u16 },
}

impl From<ListenerStatus> for Status {
    fn from(status: ListenerStatus) -> Self {
        match status {
            ListenerStatus::Running { port } => Status::Running { port },
            ListenerStatus::PortInUse { port } => Status::PortInUse { port },
        }
    }
}

impl Listeners {
    fn of(core: &CoreHandle) -> Self {
        Self {
            smtp: core.listener_status(Listener::Smtp).into(),
            mcp: core.listener_status(Listener::Mcp).into(),
        }
    }

    fn ports_in_use(&self) -> Vec<u16> {
        [self.smtp, self.mcp]
            .into_iter()
            .filter_map(|status| match status {
                Status::PortInUse { port } => Some(port),
                Status::Running { .. } => None,
            })
            .collect()
    }
}

/// Whether each listener is listening, and on which port.
#[tauri::command]
pub fn listeners(core: State<'_, Core>) -> Result<Listeners, String> {
    Ok(Listeners::of(core.handle()?))
}

/// Shows the listeners' status as the core started them, then again after
/// every change. Call it once the tray exists.
pub fn watch(app: &AppHandle) {
    let Ok(core) = app.state::<Core>().inner().handle() else {
        return;
    };
    let mut events = core.subscribe();
    show(app, core);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let listener_changed =
            |event: &CoreEvent| matches!(event, CoreEvent::ListenerChanged { .. });
        while core_events::changed(&mut events, listener_changed).await {
            if let Ok(core) = app.state::<Core>().inner().handle() {
                show(&app, core);
            }
        }
    });
}

fn show(app: &AppHandle, core: &CoreHandle) {
    let listeners = Listeners::of(core);
    tray::show_ports_in_use(app, listeners.ports_in_use());
    let _ = app.emit(LISTENERS_CHANGED, listeners);
}
