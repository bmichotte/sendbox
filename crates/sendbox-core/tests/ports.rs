//! The SMTP and MCP listeners and their ports. Each listener reports its
//! status (running, or its port in use) at startup and after every change; a
//! port another program holds stops only that listener, never the core.

mod common;

use common::{Agent, STEP_TIMEOUT, SmtpClient, TestCore, test_config};
use sendbox_core::{Config, CoreEvent, Listener, ListenerStatus};
use serde_json::json;
use tempfile::TempDir;
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio::time::timeout;

/// Takes a free port on `127.0.0.1`, as another program would, for as long
/// as the returned listener lives.
fn occupy_a_port() -> (std::net::TcpListener, u16) {
    let other_program = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind a free port");
    let port = other_program.local_addr().unwrap().port();
    (other_program, port)
}

#[tokio::test]
async fn a_core_whose_smtp_port_is_in_use_starts_anyway_and_says_so() {
    let (_other_program, port) = occupy_a_port();
    let data_dir = TempDir::new().unwrap();
    let config = Config {
        smtp_port: port,
        ..test_config(&data_dir)
    };

    let core = TestCore::start_with_config(config, data_dir).await;

    assert_eq!(
        core.listener_status(Listener::Smtp),
        ListenerStatus::PortInUse { port }
    );
    assert!(
        core.agent().await.list_emails(json!({})).await.is_empty(),
        "agents still reach the Inbox"
    );
}

// Tools like Mailpit listen on every interface, loopback included. On macOS,
// binding 127.0.0.1 beside them succeeds when both allow address reuse, as
// servers do, and would quietly take their loopback traffic.
#[cfg(unix)]
#[tokio::test]
async fn a_port_another_program_holds_on_every_interface_is_in_use() {
    let other_program = tokio::net::TcpSocket::new_v4().unwrap();
    other_program.set_reuseaddr(true).unwrap();
    // Bound but not listening: the port is just as taken, and nothing is
    // exposed to the network.
    other_program
        .bind((std::net::Ipv4Addr::UNSPECIFIED, 0).into())
        .unwrap();
    let port = other_program.local_addr().unwrap().port();
    let data_dir = TempDir::new().unwrap();
    let config = Config {
        smtp_port: port,
        ..test_config(&data_dir)
    };

    let core = TestCore::start_with_config(config, data_dir).await;

    assert_eq!(
        core.listener_status(Listener::Smtp),
        ListenerStatus::PortInUse { port }
    );
}

#[tokio::test]
async fn a_core_whose_mcp_port_is_in_use_starts_anyway_and_still_captures() {
    let (_other_program, port) = occupy_a_port();
    let data_dir = TempDir::new().unwrap();
    let config = Config {
        mcp_port: port,
        ..test_config(&data_dir)
    };

    let core = TestCore::start_with_config(config, data_dir).await;

    assert_eq!(
        core.listener_status(Listener::Mcp),
        ListenerStatus::PortInUse { port }
    );
    capture(&core, "Still Captured").await;
    assert_eq!(subjects(&core).await, ["Still Captured"]);
}

#[tokio::test]
async fn moving_the_smtp_listener_to_another_port_captures_there_at_once_and_says_so() {
    let core = TestCore::start().await;
    let old_port = core.smtp_port();
    capture(&core, "Before").await;
    let mut events = core.subscribe();

    // Port 0: whichever free port the system chooses.
    let status = core.set_port(Listener::Smtp, 0).await;

    let ListenerStatus::Running { port: new_port } = status else {
        panic!("the SMTP listener did not move: {status:?}");
    };
    assert_ne!(new_port, old_port);
    assert_eq!(core.listener_status(Listener::Smtp), status);
    assert_eq!(
        next_event(&mut events).await,
        CoreEvent::ListenerChanged {
            listener: Listener::Smtp,
            status
        }
    );
    assert!(
        TcpStream::connect(("127.0.0.1", old_port)).await.is_err(),
        "nothing listens on the old port any more"
    );
    capture(&core, "After").await;
    assert_eq!(subjects(&core).await, ["After", "Before"]);
}

#[tokio::test]
async fn moving_the_mcp_endpoint_does_not_wait_for_connected_agents_and_serves_on_the_new_port() {
    let core = TestCore::start().await;
    let old_port = core.mcp_port();
    let connected = core.agent().await;
    connected.list_emails(json!({})).await;

    let status = timeout(STEP_TIMEOUT, core.set_port(Listener::Mcp, 0))
        .await
        .expect("the move does not wait for connected agents");

    assert!(matches!(status, ListenerStatus::Running { port } if port != old_port));
    assert_eq!(core.listener_status(Listener::Mcp), status);
    assert!(
        Agent::try_connect(old_port).await.is_err(),
        "nothing listens on the old port any more"
    );
    capture(&core, "Listed on the new port").await;
    let listed = core.agent().await.list_emails(json!({})).await;
    assert_eq!(listed[0]["subject"], "Listed on the new port");
}

#[tokio::test]
async fn moving_a_listener_to_a_port_in_use_stops_it_until_a_free_port_is_chosen() {
    let core = TestCore::start().await;
    let old_port = core.smtp_port();
    let (_other_program, taken) = occupy_a_port();
    let mut events = core.subscribe();

    let in_use = core.set_port(Listener::Smtp, taken).await;

    assert_eq!(in_use, ListenerStatus::PortInUse { port: taken });
    assert_eq!(core.listener_status(Listener::Smtp), in_use);
    assert_eq!(
        next_event(&mut events).await,
        CoreEvent::ListenerChanged {
            listener: Listener::Smtp,
            status: in_use
        }
    );
    assert!(
        TcpStream::connect(("127.0.0.1", old_port)).await.is_err(),
        "the listener left its old port"
    );

    let running = core.set_port(Listener::Smtp, 0).await;

    assert!(matches!(running, ListenerStatus::Running { .. }));
    assert_eq!(core.listener_status(Listener::Smtp), running);
    assert_eq!(
        next_event(&mut events).await,
        CoreEvent::ListenerChanged {
            listener: Listener::Smtp,
            status: running
        }
    );
    capture(&core, "Captured again").await;
}

#[tokio::test]
async fn setting_a_port_in_use_again_once_it_is_free_starts_the_listener() {
    let (other_program, port) = occupy_a_port();
    let data_dir = TempDir::new().unwrap();
    let config = Config {
        mcp_port: port,
        ..test_config(&data_dir)
    };
    let core = TestCore::start_with_config(config, data_dir).await;
    assert_eq!(
        core.listener_status(Listener::Mcp),
        ListenerStatus::PortInUse { port }
    );
    drop(other_program);

    let status = core.set_port(Listener::Mcp, port).await;

    assert_eq!(status, ListenerStatus::Running { port });
    core.agent().await.list_emails(json!({})).await;
}

#[tokio::test]
async fn setting_the_port_a_listener_already_runs_on_leaves_it_and_its_sessions_alone() {
    let core = TestCore::start().await;
    let port = core.smtp_port();
    let mut session = SmtpClient::after_ehlo(port).await;
    let mut events = core.subscribe();

    let status = core.set_port(Listener::Smtp, port).await;

    assert_eq!(status, ListenerStatus::Running { port });
    session
        .capture(
            "app@acme.test",
            &["alice@example.com"],
            "Subject: Same session\r\n\r\nHi\r\n",
        )
        .await;
    assert!(
        matches!(next_event(&mut events).await, CoreEvent::EmailCaptured(_)),
        "no listener change to report"
    );
}

async fn next_event(events: &mut broadcast::Receiver<CoreEvent>) -> CoreEvent {
    timeout(STEP_TIMEOUT, events.recv())
        .await
        .expect("an event in time")
        .expect("an event")
}

/// Has the core Capture an Email with this subject over SMTP.
async fn capture(core: &TestCore, subject: &str) {
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;
    let raw = format!("Subject: {subject}\r\n\r\nHi\r\n");
    smtp.capture("app@acme.test", &["alice@example.com"], &raw)
        .await;
}

/// The subjects of the Emails in the Inbox, newest first.
async fn subjects(core: &TestCore) -> Vec<String> {
    core.inbox()
        .await
        .into_iter()
        .filter_map(|email| email.subject)
        .collect()
}
