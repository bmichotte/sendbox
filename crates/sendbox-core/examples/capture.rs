//! Runs a core and prints each Email it Captures, for trying it by hand.
//!
//! ```sh
//! cargo run -p sendbox-core --example capture [-- <smtp port> [<data dir>]]
//! ```
//!
//! Defaults: port 1025, Inbox in `<temp dir>/sendbox-example` (kept across runs).

use std::error::Error;
use std::path::PathBuf;

use sendbox_core::{Config, CoreEvent, EmailSummary};
use tokio::sync::broadcast::error::RecvError;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let smtp_port = args
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
        data_dir: data_dir.clone(),
    })
    .await
    .map_err(|error| format!("cannot start on port {smtp_port}: {error}"))?;
    println!(
        "Capturing on 127.0.0.1:{} — Inbox in {} — Ctrl+C to stop",
        core.smtp_port(),
        data_dir.display()
    );
    let mut events = core.subscribe();
    let existing = core.list_emails().await?;
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
