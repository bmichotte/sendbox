mod common;

use common::{SmtpClient, TestCore};

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

    // Starting on that exact port fails if it is still taken.
    let second = TestCore::start_with(port, tempfile::TempDir::new().unwrap()).await;
    assert_eq!(second.smtp_port(), port);
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
    let listed_before = core.list_emails().await.unwrap();
    assert_eq!(listed_before.len(), 2);

    let core = core.restart().await;

    assert_eq!(core.list_emails().await.unwrap(), listed_before);
    let raw_source = core.raw_source(listed_before[0].id).await.unwrap();
    assert_eq!(
        raw_source.as_deref(),
        Some(&b"Subject: Before restart 2\r\n\r\nKeep me\r\n"[..])
    );
}

#[tokio::test]
async fn the_smtp_listener_is_unreachable_from_other_interfaces() {
    let core = TestCore::start().await;
    let Some(lan_address) = non_loopback_ipv4() else {
        eprintln!("skipped: this machine has no non-loopback IPv4 address");
        return;
    };

    let attempt = tokio::time::timeout(
        common::STEP_TIMEOUT,
        tokio::net::TcpStream::connect((lan_address, core.smtp_port())),
    )
    .await;

    assert!(
        !matches!(attempt, Ok(Ok(_))),
        "connected to the SMTP port through {lan_address}"
    );
}

/// The address this machine would use to reach the network, if any. Nothing
/// is sent: connecting a UDP socket only picks a route.
fn non_loopback_ipv4() -> Option<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let address = socket.local_addr().ok()?.ip();
    (!address.is_loopback() && !address.is_unspecified()).then_some(address)
}
