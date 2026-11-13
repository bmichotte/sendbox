//! Agents list and read Emails over MCP, as Claude Code does once connected
//! with `claude mcp add --transport http sendbox http://127.0.0.1:1026/mcp`.

mod common;

use std::time::SystemTime;

use chrono::{DateTime, FixedOffset, SecondsFormat, Utc};
use common::{STEP_TIMEOUT, SmtpClient, TestCore, send_with_lettre};
use lettre::Message;
use lettre::message::header::ContentType;
use lettre::message::{Attachment, MultiPart};
use sendbox_core::Config;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

#[tokio::test]
async fn an_agent_lists_the_inbox_newest_first_with_a_summary_of_each_email() {
    let core = TestCore::start().await;
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;
    let before = SystemTime::now();
    smtp.capture(
        "bounces@mailer.test",
        &["alice@example.com", "audit@acme.test"],
        "From: Acme Shop <shop@acme.test>\r\n\
         To: Alice <alice@example.com>\r\n\
         Subject: Welcome aboard\r\n\
         \r\n\
         Hello Alice\r\n",
    )
    .await;
    smtp.capture(
        "app@acme.test",
        &["bob@example.com"],
        "Subject: Second\r\n\r\nHi\r\n",
    )
    .await;
    let after = SystemTime::now();
    let ids: Vec<i64> = core.inbox().await.iter().map(|email| email.id.0).collect();

    let emails = core.agent().await.list_emails(json!({})).await;

    assert_eq!(emails.len(), 2);
    let [newest, oldest] = &emails[..] else {
        unreachable!()
    };
    assert_eq!(newest["id"], ids[0]);
    assert_eq!(
        newest["from"], "app@acme.test",
        "envelope sender without From"
    );
    assert_eq!(newest["to"], json!([]));
    assert_eq!(newest["subject"], "Second");
    assert_eq!(oldest["id"], ids[1]);
    assert_eq!(oldest["from"], "shop@acme.test");
    assert_eq!(oldest["to"], json!(["alice@example.com"]));
    assert_eq!(oldest["subject"], "Welcome aboard");
    for email in [newest, oldest] {
        assert_eq!(email["read"], false);
        let date: DateTime<Utc> = email["date"]
            .as_str()
            .expect("the date is a string")
            .parse()
            .expect("the date is RFC 3339");
        let date = SystemTime::from(date);
        // RFC 3339 dates here are to the millisecond.
        let before = before - std::time::Duration::from_millis(1);
        assert!(
            before <= date && date <= after,
            "{date:?} is when it was Captured"
        );
    }
}

/// The subjects of `emails` as listed over MCP, in order.
fn subjects(emails: &[serde_json::Value]) -> Vec<&str> {
    emails
        .iter()
        .map(|email| email["subject"].as_str().expect("a subject"))
        .collect()
}

#[tokio::test]
async fn the_from_and_subject_filters_match_case_insensitive_substrings() {
    let core = TestCore::start().await;
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;
    for (envelope_sender, from, subject) in [
        (
            "bounces@mailer.test",
            "Shop@Acme.test",
            "Your ORDER is confirmed",
        ),
        ("app@acme.test", "app@acme.test", "Verify your Email"),
        ("news@other.test", "news@other.test", "Order something new"),
        ("bounces@mailer.test", "Zoë@café.test", "Café ☕ order"),
    ] {
        let raw = format!("From: {from}\r\nSubject: {subject}\r\n\r\nHi\r\n");
        smtp.capture(envelope_sender, &["alice@example.com"], &raw)
            .await;
    }
    let agent = core.agent().await;

    let from_acme = agent.list_emails(json!({ "from": "ACME.test" })).await;
    let ordered = agent.list_emails(json!({ "subject": "order" })).await;
    let ordered_from_acme = agent
        .list_emails(json!({ "from": "acme", "subject": "order" }))
        .await;
    let accented = agent
        .list_emails(json!({ "from": "ZOË@CAFÉ", "subject": "CAFÉ" }))
        .await;
    let by_envelope_sender = agent.list_emails(json!({ "from": "mailer.test" })).await;

    assert_eq!(
        subjects(&from_acme),
        ["Verify your Email", "Your ORDER is confirmed"]
    );
    assert_eq!(
        subjects(&ordered),
        [
            "Café ☕ order",
            "Order something new",
            "Your ORDER is confirmed"
        ]
    );
    assert_eq!(subjects(&ordered_from_acme), ["Your ORDER is confirmed"]);
    assert_eq!(subjects(&accented), ["Café ☕ order"]);
    assert_eq!(
        subjects(&by_envelope_sender),
        ["Café ☕ order", "Your ORDER is confirmed"],
        "the envelope sender counts as a sender"
    );
}

#[tokio::test]
async fn the_to_filter_matches_to_cc_and_envelope_recipients_so_bcc_is_found() {
    let core = TestCore::start().await;
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;
    // The header recipients are deliberately not all in the envelope, so each
    // source of recipients is checked on its own.
    smtp.capture(
        "app@acme.test",
        &["envelope-only@example.com"],
        "To: To-Header@Example.com\r\n\
         Cc: Zoë <cc-header@example.com>\r\n\
         Subject: Team update\r\n\
         \r\n\
         Hi all\r\n",
    )
    .await;
    smtp.capture(
        "app@acme.test",
        &["dave@example.com"],
        "To: dave@example.com\r\nSubject: Someone else\r\n\r\nHi\r\n",
    )
    .await;
    let agent = core.agent().await;

    for recipient in ["to-header", "CC-HEADER", "envelope-only", "example.com"] {
        let emails = agent.list_emails(json!({ "to": recipient })).await;
        let expected: &[&str] = if recipient == "example.com" {
            &["Someone else", "Team update"]
        } else {
            &["Team update"]
        };
        assert_eq!(subjects(&emails), expected, "to: {recipient}");
    }
    assert!(
        agent.list_emails(json!({ "to": "zoë" })).await.is_empty(),
        "names don't count"
    );
}

#[tokio::test]
async fn since_lists_only_the_emails_captured_from_that_time_on() {
    let core = TestCore::start().await;
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;
    smtp.capture(
        "app@acme.test",
        &["a@example.com"],
        "Subject: Before\r\n\r\nHi\r\n",
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let since = DateTime::<Utc>::from(SystemTime::now());
    for subject in ["After 1", "After 2"] {
        let raw = format!("Subject: {subject}\r\n\r\nHi\r\n");
        smtp.capture("app@acme.test", &["a@example.com"], &raw)
            .await;
    }
    let agent = core.agent().await;

    let in_utc = since.to_rfc3339_opts(SecondsFormat::Nanos, true);
    let in_brussels = since
        .with_timezone(&FixedOffset::east_opt(3600).unwrap())
        .to_rfc3339_opts(SecondsFormat::Nanos, false);
    for since in [in_utc, in_brussels] {
        let emails = agent.list_emails(json!({ "since": since })).await;
        assert_eq!(subjects(&emails), ["After 2", "After 1"], "since {since}");
    }
    let not_a_time = agent
        .call("list_emails", json!({ "since": "yesterday" }))
        .await;
    assert_eq!(not_a_time.is_error, Some(true), "since must be RFC 3339");
}

#[tokio::test]
async fn the_list_holds_the_20_newest_emails_unless_a_limit_is_given() {
    let core = TestCore::start().await;
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;
    for number in 1..=25 {
        let raw = format!("Subject: Email {number}\r\n\r\nHi\r\n");
        smtp.capture("app@acme.test", &["a@example.com"], &raw)
            .await;
    }
    let agent = core.agent().await;

    let by_default = agent.list_emails(json!({})).await;
    let limited = agent.list_emails(json!({ "limit": 3 })).await;
    let filtered_then_limited = agent
        .list_emails(json!({ "subject": "Email 1", "limit": 2 }))
        .await;

    assert_eq!(by_default.len(), 20);
    assert_eq!(subjects(&by_default)[..2], ["Email 25", "Email 24"]);
    assert_eq!(subjects(&by_default)[19], "Email 6");
    assert_eq!(subjects(&limited), ["Email 25", "Email 24", "Email 23"]);
    assert_eq!(subjects(&filtered_then_limited), ["Email 19", "Email 18"]);
}

/// A sign-up Email as an app would send it: text and HTML alternatives with
/// a verification link, an attached invoice, and a Bcc.
fn verification_email() -> Message {
    let text = "Verify your address: https://acme.test/verify?token=abc.\r\nThanks!\r\n";
    let html = "<p><a href=\"https://acme.test/verify?token=abc&amp;src=email\">Verify</a> \
                or <a href='https://acme.test/help'>get help</a>.</p>";
    let invoice = Attachment::new("invoice.pdf".to_string()).body(
        b"%PDF-1.4 not really".to_vec(),
        ContentType::parse("application/pdf").unwrap(),
    );
    Message::builder()
        .from("Acme Shop <shop@acme.test>".parse().unwrap())
        .to("Alice <alice@example.com>".parse().unwrap())
        .cc("bob@example.com".parse().unwrap())
        .bcc("audit@acme.test".parse().unwrap())
        .subject("Verify your Email")
        .header(XCampaign("Sign-up".to_string()))
        .multipart(
            MultiPart::mixed()
                .multipart(MultiPart::alternative_plain_html(
                    text.to_string(),
                    html.to_string(),
                ))
                .singlepart(invoice),
        )
        .unwrap()
}

/// A custom header, as apps add for tracking.
#[derive(Clone)]
struct XCampaign(String);

impl lettre::message::header::Header for XCampaign {
    fn name() -> lettre::message::header::HeaderName {
        lettre::message::header::HeaderName::new_from_ascii_str("X-Campaign")
    }

    fn parse(value: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Self(value.to_string()))
    }

    fn display(&self) -> lettre::message::header::HeaderValue {
        lettre::message::header::HeaderValue::new(Self::name(), self.0.clone())
    }
}

#[tokio::test]
async fn get_email_returns_its_text_html_headers_envelope_links_and_attachments() {
    let core = TestCore::start().await;
    send_with_lettre(core.smtp_port(), verification_email()).await;
    let id = core.inbox().await[0].id.0;

    let email = core
        .agent()
        .await
        .call_ok("get_email", json!({ "id": id }))
        .await;

    assert_eq!(email["id"], id);
    assert_eq!(email["from"], "shop@acme.test");
    assert_eq!(email["to"], json!(["alice@example.com"]));
    assert_eq!(email["cc"], json!(["bob@example.com"]));
    assert_eq!(email["subject"], "Verify your Email");
    assert_eq!(email["envelope"]["sender"], "shop@acme.test");
    let mut recipients: Vec<_> = email["envelope"]["recipients"]
        .as_array()
        .expect("envelope recipients")
        .iter()
        .map(|recipient| recipient.as_str().unwrap())
        .collect();
    recipients.sort();
    assert_eq!(
        recipients,
        ["alice@example.com", "audit@acme.test", "bob@example.com"]
    );
    assert_eq!(
        email["text"],
        "Verify your address: https://acme.test/verify?token=abc.\r\nThanks!\r\n"
    );
    assert!(
        email["html"]
            .as_str()
            .expect("an HTML body")
            .contains("<a href='https://acme.test/help'>get help</a>"),
        "{}",
        email["html"]
    );
    assert_eq!(
        email["links"],
        json!([
            "https://acme.test/verify?token=abc&src=email",
            "https://acme.test/help",
            "https://acme.test/verify?token=abc",
        ])
    );
    let headers = email["headers"].as_array().expect("headers");
    for (name, value) in [
        ("From", "\"Acme Shop\" <shop@acme.test>"),
        ("Subject", "Verify your Email"),
        ("X-Campaign", "Sign-up"),
    ] {
        assert!(
            headers.contains(&json!({ "name": name, "value": value })),
            "{name}: {value} in {headers:?}"
        );
    }
    assert_eq!(
        email["attachments"],
        json!([{ "name": "invoice.pdf", "content_type": "application/pdf", "size": 19 }]),
        "metadata only, never the bytes"
    );
}

#[tokio::test]
async fn get_email_with_an_unknown_id_tells_the_agent_no_email_has_it() {
    let core = TestCore::start().await;

    let result = core
        .agent()
        .await
        .call("get_email", json!({ "id": 42 }))
        .await;

    assert_eq!(result.is_error, Some(true));
    let message = &result.content[0].as_text().expect("a text message").text;
    assert!(message.contains("No Email has id 42"), "{message}");
}

/// Captures one Email per subject, oldest first, and returns their ids newest first.
async fn capture_subjects(core: &TestCore, subjects: &[&str]) -> Vec<i64> {
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;
    for subject in subjects {
        let raw = format!("Subject: {subject}\r\n\r\nHi\r\n");
        smtp.capture("app@acme.test", &["alice@example.com"], &raw)
            .await;
    }
    core.inbox().await.iter().map(|email| email.id.0).collect()
}

#[tokio::test]
async fn reading_an_email_over_mcp_marks_it_read_but_listing_never_does() {
    let core = TestCore::start().await;
    let ids = capture_subjects(&core, &["Older", "Newer"]).await;
    let agent = core.agent().await;
    agent.list_emails(json!({})).await;
    agent.list_emails(json!({ "unread_only": true })).await;
    let listed = agent.list_emails(json!({})).await;
    assert!(
        listed.iter().all(|email| email["read"] == false),
        "listing left them Unread: {listed:?}"
    );

    let read = agent.call_ok("get_email", json!({ "id": ids[1] })).await;

    assert_eq!(read["read"], true, "the Email returned is read");
    let unread = agent.list_emails(json!({ "unread_only": true })).await;
    assert_eq!(subjects(&unread), ["Newer"]);
    let listed = agent.list_emails(json!({})).await;
    assert_eq!(listed[0]["read"], false);
    assert_eq!(listed[1]["read"], true);
    let in_the_window: Vec<_> = core.inbox().await.iter().map(|email| email.read).collect();
    assert_eq!(
        in_the_window,
        [false, true],
        "the window sees the same read state"
    );
}

#[tokio::test]
async fn with_mcp_read_marking_off_reading_an_email_leaves_it_unread() {
    let data_dir = tempfile::TempDir::new().unwrap();
    let config = Config {
        mcp_marks_read: false,
        ..common::test_config(&data_dir)
    };
    let core = TestCore::start_with_config(config, data_dir).await;
    let ids = capture_subjects(&core, &["Welcome"]).await;
    let agent = core.agent().await;

    let read = agent.call_ok("get_email", json!({ "id": ids[0] })).await;

    assert_eq!(read["read"], false);
    let unread = agent.list_emails(json!({ "unread_only": true })).await;
    assert_eq!(subjects(&unread), ["Welcome"]);
}

/// Sends an MCP `initialize` request over raw HTTP with these `Host` and
/// `Origin` headers, as a web page could through DNS rebinding, and returns
/// the response status code.
async fn initialize_status(port: u16, host: &str, origin: Option<&str>) -> u16 {
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"page","version":"0"}}}"#;
    let origin = origin
        .map(|origin| format!("Origin: {origin}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: {host}\r\n{origin}\
         Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = vec![0; 64];
    let read = timeout(STEP_TIMEOUT, stream.read(&mut response))
        .await
        .expect("response timed out")
        .unwrap();
    let status_line = String::from_utf8_lossy(&response[..read]);
    status_line
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("not an HTTP response: {status_line}"))
}

#[tokio::test]
async fn requests_with_a_non_local_host_or_origin_are_rejected() {
    let core = TestCore::start().await;
    let port = core.mcp_port();
    let local = format!("127.0.0.1:{port}");

    for host in [
        local.clone(),
        format!("localhost:{port}"),
        format!("[::1]:{port}"),
    ] {
        assert_eq!(
            initialize_status(port, &host, None).await,
            200,
            "Host {host}"
        );
    }
    assert_eq!(
        initialize_status(port, &local, Some("http://localhost:5173")).await,
        200,
        "a local dev server's page"
    );
    for host in ["evil.test".to_string(), format!("evil.test:{port}")] {
        assert_eq!(
            initialize_status(port, &host, None).await,
            403,
            "Host {host}"
        );
    }
    for origin in ["http://evil.test", "https://evil.test:443", "null"] {
        assert_eq!(
            initialize_status(port, &local, Some(origin)).await,
            403,
            "Origin {origin}"
        );
    }
}
