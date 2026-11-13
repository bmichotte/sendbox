//! Runs a core and prints each Email it Captures, for trying it by hand.
//!
//! ```sh
//! cargo run -p sendbox-core --example capture [-- <smtp port> [<data dir>]]
//! ```
//!
//! Defaults: port 1025, Inbox in `<temp dir>/sendbox-example` (kept across runs).
//! The MCP endpoint listens on the next port (1026 by default), so an agent can
//! connect with `claude mcp add --transport http sendbox http://127.0.0.1:1026/mcp`.

use std::error::Error;
use std::path::PathBuf;

use sendbox_core::{Config, CoreEvent, EmailFilter, EmailSummary, Listener, ListenerStatus};
use tokio::sync::broadcast::error::RecvError;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let smtp_port: u16 = args
        .next()
        .map(|port| port.parse())
        .transpose()?
        .unwrap_or(1025);
    let data_dir = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("sendbox-example"));

    let core = sendbox_core::start(Config {
        smtp_port,
        // Port 0 (ephemeral) for SMTP means ephemeral for MCP too.
        mcp_port: if smtp_port == 0 {
            0
        } else {
            smtp_port.saturating_add(1)
        },
        ..Config::new(&data_dir)
    })
    .await
    .map_err(|error| format!("cannot open the Inbox in {}: {error}", data_dir.display()))?;
    let smtp = describe(core.listener_status(Listener::Smtp), |port| {
        format!("127.0.0.1:{port}")
    });
    let mcp = describe(core.listener_status(Listener::Mcp), |port| {
        format!("http://127.0.0.1:{port}/mcp")
    });
    println!(
        "SMTP {smtp} — MCP {mcp} — Inbox in {} — Ctrl+C to stop",
        data_dir.display()
    );
    let mut events = core.subscribe();
    let existing = core.list_emails(&EmailFilter::default(), None).await?;
    println!("{} Email(s) already in the Inbox", existing.len());
    existing.iter().rev().for_each(print_email);

    let stop = tokio::signal::ctrl_c();
    tokio::pin!(stop);
    loop {
        tokio::select! {
            _ = &mut stop => break,
            event = events.recv() => match event {
                Ok(CoreEvent::EmailCaptured(email)) => print_email(&email),
                Ok(_) => {}
                Err(RecvError::Lagged(missed)) => println!("({missed} events missed)"),
                Err(RecvError::Closed) => break,
            },
        }
    }
    core.shutdown().await;
    println!("Stopped");
    Ok(())
}

fn describe(status: ListenerStatus, address: fn(u16) -> String) -> String {
    match status {
        ListenerStatus::Running { port } => format!("on {}", address(port)),
        ListenerStatus::PortInUse { port } => format!("not running: port {port} in use"),
    }
}

fn print_email(email: &EmailSummary) {
    println!(
        "#{} {} → {} | {} | {} bytes",
        email.id.0,
        if email.envelope_sender.is_empty() {
            "<>"
        } else {
            &email.envelope_sender
        },
        email.envelope_recipients.join(", "),
        email.subject.as_deref().unwrap_or("(no subject)"),
        email.size,
    );
}
