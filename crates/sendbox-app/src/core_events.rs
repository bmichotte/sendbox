//! Following the core's live events, as the Inbox list, the tray and the
//! listeners' status do.

use sendbox_core::CoreEvent;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;

/// Waits for the next of `events` that `matters`, or for news that some
/// were missed: one may have mattered, so whatever follows them must be
/// shown again anyway. `false` once the core has stopped.
pub async fn changed(events: &mut Receiver<CoreEvent>, matters: fn(&CoreEvent) -> bool) -> bool {
    loop {
        match events.recv().await {
            Ok(event) if matters(&event) => return true,
            Ok(_) => {}
            Err(RecvError::Lagged(_)) => return true,
            Err(RecvError::Closed) => return false,
        }
    }
}
