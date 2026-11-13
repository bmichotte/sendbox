mod common;

use common::{Agent, STEP_TIMEOUT, SmtpClient, TestCore, test_config};
use sendbox_core::{Config, Listener, ListenerStatus};
use serde_json::json;
use tokio::time::timeout;

#[tokio::test]
async fn shutdown_releases_the_mcp_port_even_with_an_agent_connected() {
    let first = TestCore::start().await;
    let port = first.mcp_port();
    assert_ne!(port, 0, "the handle reports the bound port");
    let agent = first.agent().await;
    agent.list_emails(json!({})).await;

    timeout(STEP_TIMEOUT, first.handle.shutdown())
        .await
        .expect("shutdown does not wait for connected agents");

    let refused = Agent::try_connect(port).await;
    assert!(refused.is_err(), "an agent gets a plain connection error");
    let data_dir = tempfile::TempDir::new().unwrap();
    let config = Config {
        mcp_port: port,
        ..test_config(&data_dir)
    };
    let second = TestCore::start_with_config(config, data_dir).await;
    assert_eq!(second.mcp_port(), port);
    second.agent().await.list_emails(json!({})).await;
}

#[tokio::test]
async fn shutdown_releases_the_smtp_port_for_a_new_core() {
    let first = TestCore::start().await;
    let port = first.smtp_port();
    assert_ne!(
        port, 0,
        "the handle reports the bound port, not the requested 0"
    );
    tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("core accepts connections on its reported port");

    first.handle.shutdown().await;

    let second = TestCore::start_with(port, tempfile::TempDir::new().unwrap()).await;
    assert_eq!(
        second.listener_status(Listener::Smtp),
        ListenerStatus::Running { port },
        "the port is free again"
    );
}

#[tokio::test]
async fn the_inbox_survives_a_restart_on_the_same_data_directory() {
    let core = TestCore::start().await;
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;
    for subject in ["Before restart 1", "Before restart 2"] {
        let data = format!("Subject: {subject}\r\n\r\nKeep me\r\n.\r\n");
        smtp.send_transaction("app@acme.test", &["alice@example.com"], data.as_bytes())
            .await;
    }
    drop(smtp);
    let listed_before = core.inbox().await;
    assert_eq!(listed_before.len(), 2);

    let core = core.restart().await;

    assert_eq!(core.inbox().await, listed_before);
    let raw_source = core.raw_source(listed_before[0].id).await.unwrap();
    assert_eq!(
        raw_source.as_deref(),
        Some(&b"Subject: Before restart 2\r\n\r\nKeep me\r\n"[..])
    );
}

#[tokio::test]
async fn the_smtp_and_mcp_listeners_are_unreachable_from_other_interfaces() {
    let core = TestCore::start().await;
    let Some(lan_address) = non_loopback_ipv4() else {
        eprintln!("skipped: this machine has no non-loopback IPv4 address");
        return;
    };

    for (listener, port) in [("SMTP", core.smtp_port()), ("MCP", core.mcp_port())] {
        let attempt = timeout(
            STEP_TIMEOUT,
            tokio::net::TcpStream::connect((lan_address, port)),
        )
        .await;

        assert!(
            !matches!(attempt, Ok(Ok(_))),
            "connected to the {listener} port through {lan_address}"
        );
    }
}

/// The address this machine would use to reach the network, if any. Nothing
/// is sent: connecting a UDP socket only picks a route.
fn non_loopback_ipv4() -> Option<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let address = socket.local_addr().ok()?.ip();
    (!address.is_loopback() && !address.is_unspecified()).then_some(address)
}

#[tokio::test]
async fn an_inbox_written_before_from_and_to_were_kept_still_opens_and_captures() {
    let data_dir = tempfile::TempDir::new().unwrap();
    // The Inbox exactly as the first version of the core left it.
    let db = rusqlite::Connection::open(data_dir.path().join("inbox.sqlite3")).unwrap();
    db.execute_batch(
        "CREATE TABLE emails (
            id                  INTEGER PRIMARY KEY AUTOINCREMENT,
            received_at_ns      INTEGER NOT NULL,
            envelope_sender     TEXT NOT NULL,
            envelope_recipients TEXT NOT NULL,
            subject             TEXT,
            size                INTEGER NOT NULL,
            raw_source          BLOB NOT NULL
        );
        INSERT INTO emails (received_at_ns, envelope_sender, envelope_recipients, subject, size, raw_source)
        VALUES (1, 'old@acme.test', 'alice@example.com', 'Kept from before', 2, x'0d0a');",
    )
    .unwrap();
    drop(db);

    let core = TestCore::start_with(0, data_dir).await;
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;
    let reply = smtp
        .send_transaction(
            "new@acme.test",
            &["bob@example.com"],
            b"From: new@acme.test\r\nTo: bob@example.com\r\nSubject: New\r\n\r\n.\r\n",
        )
        .await;

    assert_eq!(reply.code, 250);
    let emails = core.inbox().await;
    let listed: Vec<_> = emails
        .iter()
        .map(|email| {
            (
                email.subject.as_deref(),
                email.from.as_deref(),
                email.to.clone(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            (
                Some("New"),
                Some("new@acme.test"),
                vec!["bob@example.com".to_string()]
            ),
            (Some("Kept from before"), None, vec![]),
        ]
    );
}
