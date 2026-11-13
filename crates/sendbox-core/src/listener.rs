//! The core's listeners (SMTP and the MCP endpoint) as the handle sees them:
//! each on its own port of `127.0.0.1`, reporting whether it runs. A port
//! another program holds stops only that listener, never the core.

use std::future::Future;
use std::io;
use std::net::Ipv4Addr;
use std::pin::Pin;
use std::sync::Mutex;

use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::ListenerStatus;

/// What a listener does with its bound port: serve it until the token is
/// cancelled, then close what is still open.
pub(crate) type Serve = Box<
    dyn Fn(TcpListener, CancellationToken) -> Pin<Box<dyn Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// One listener: serving its port, or stopped because the port is in use.
pub(crate) struct Slot {
    serve: Serve,
    running: tokio::sync::Mutex<Option<Running>>,
    /// Kept apart from `running` so that it reads without waiting.
    status: Mutex<ListenerStatus>,
}

impl Slot {
    /// Binds `port` and serves it; if the port is in use, the listener stays
    /// stopped and says so.
    pub(crate) async fn start(port: u16, serve: Serve) -> Self {
        let (running, status) = launch(&serve, port).await;
        Self {
            serve,
            running: tokio::sync::Mutex::new(running),
            status: Mutex::new(status),
        }
    }

    pub(crate) fn status(&self) -> ListenerStatus {
        *self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Moves the listener to `port`, or tries its port again if it was in
    /// use, and returns its status. A listener already running on `port` is
    /// left alone; otherwise `changed` hears the new status before any later
    /// move starts, so that moves are reported in order.
    pub(crate) async fn move_to(
        &self,
        port: u16,
        changed: impl FnOnce(ListenerStatus),
    ) -> ListenerStatus {
        let mut running = self.running.lock().await;
        let current = self.status();
        if current == (ListenerStatus::Running { port }) {
            return current;
        }
        if let Some(old) = running.take() {
            old.stop().await;
        }
        let (moved, status) = launch(&self.serve, port).await;
        *running = moved;
        *self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = status;
        changed(status);
        status
    }

    /// Stops listening and closes what is open. When this returns, the port
    /// is free.
    pub(crate) async fn shutdown(&self) {
        if let Some(running) = self.running.lock().await.take() {
            running.stop().await;
        }
    }
}

/// A listener serving its port in a task of its own.
struct Running {
    stop: CancellationToken,
    task: JoinHandle<()>,
}

impl Running {
    async fn stop(self) {
        self.stop.cancel();
        let _ = self.task.await;
    }
}

async fn launch(serve: &Serve, port: u16) -> (Option<Running>, ListenerStatus) {
    match bind(port).await {
        Ok((listener, port)) => {
            let stop = CancellationToken::new();
            let task = tokio::spawn(serve(listener, stop.clone()));
            (
                Some(Running { stop, task }),
                ListenerStatus::Running { port },
            )
        }
        // Another program holding the port is the only failure to expect
        // when binding a loopback port.
        Err(_) => (None, ListenerStatus::PortInUse { port }),
    }
}

/// Binds `127.0.0.1:port` (loopback only) and returns the port actually
/// bound: for 0, the ephemeral port the system chose.
async fn bind(port: u16) -> io::Result<(TcpListener, u16)> {
    #[cfg(unix)]
    if port != 0 {
        refuse_if_held_on_every_interface(port)?;
    }
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
    let port = listener.local_addr()?.port();
    Ok((listener, port))
}

/// Fails if another program holds `port` on every interface (`0.0.0.0`, or
/// a dual-stack `[::]`), as Mailpit does: loopback traffic for that port
/// goes to it. On macOS, binding `127.0.0.1` beside it would succeed (both
/// sockets allow address reuse), so bind the wildcard address for a moment,
/// without listening, to find out.
#[cfg(unix)]
fn refuse_if_held_on_every_interface(port: u16) -> io::Result<()> {
    let probe = tokio::net::TcpSocket::new_v4()?;
    // As our listener does, so that our own closed connections still
    // waiting out TIME_WAIT on the port do not count.
    probe.set_reuseaddr(true)?;
    probe.bind((Ipv4Addr::UNSPECIFIED, port).into())
}
