mod common;

use std::time::{Duration, SystemTime};

use common::{STEP_TIMEOUT, SmtpClient, TestCore, send_with_lettre};
use lettre::Message;
use sendbox_core::CoreEvent;

#[tokio::test]
async fn an_email_sent_with_lettre_is_captured_into_the_inbox() {
    let core = TestCore::start().await;
    let message = Message::builder()
        .from("Acme App <app@acme.test>".parse().unwrap())
        .to("Alice <alice@example.com>".parse().unwrap())
        .subject("Welcome aboard")
        .body(String::from("Hello Alice"))
        .unwrap();
    // lettre ends DATA with "\r\n.\r\n": the CRLF before the dot belongs to the Email.
    let sent_size = message.formatted().len() + 2;
    let before = SystemTime::now();

    send_with_lettre(core.smtp_port(), message).await;

    let emails = core.inbox().await;
    assert_eq!(emails.len(), 1);
    let email = &emails[0];
    assert_eq!(email.envelope_sender, "app@acme.test");
    assert_eq!(email.envelope_recipients, ["alice@example.com"]);
    assert_eq!(email.subject.as_deref(), Some("Welcome aboard"));
    assert_eq!(email.size, sent_size as u64);
    // Capture times are to the millisecond.
    let before = before - Duration::from_millis(1);
    assert!(email.received_at > before && email.received_at <= SystemTime::now());
}

#[tokio::test]
async fn the_raw_source_is_stored_byte_for_byte_with_dots_unstuffed() {
    let core = TestCore::start().await;
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;
    let on_the_wire: &[u8] = b"..first line starts with a dot\r\n\
        Subject: Dots\r\n\
        \r\n\
        ...two dots\r\n\
        ..\r\n\
        not at the start .\r\n\
        bare\nLF and 8-bit caf\xc3\xa9 \xff\r\n\
        .\r\n";

    let reply = smtp
        .send_transaction("app@acme.test", &["alice@example.com"], on_the_wire)
        .await;

    assert_eq!(reply.code, 250);
    let id = core.inbox().await[0].id;
    let raw_source = core.raw_source(id).await.unwrap().expect("Email exists");
    assert_eq!(
        raw_source,
        b".first line starts with a dot\r\n\
        Subject: Dots\r\n\
        \r\n\
        ..two dots\r\n\
        .\r\n\
        not at the start .\r\n\
        bare\nLF and 8-bit caf\xc3\xa9 \xff\r\n"
    );
}

#[tokio::test]
async fn every_envelope_recipient_is_kept_including_bcc() {
    let core = TestCore::start().await;
    let message = Message::builder()
        .from("app@acme.test".parse().unwrap())
        .to("alice@example.com".parse().unwrap())
        .cc("bob@example.com".parse().unwrap())
        .bcc("carol@example.com".parse().unwrap())
        .subject("Team update")
        .body(String::from("Hi all"))
        .unwrap();

    send_with_lettre(core.smtp_port(), message).await;

    let emails = core.inbox().await;
    let mut recipients = emails[0].envelope_recipients.clone();
    recipients.sort();
    assert_eq!(
        recipients,
        ["alice@example.com", "bob@example.com", "carol@example.com"]
    );
}

#[tokio::test]
async fn several_emails_on_one_connection_are_each_captured_newest_first() {
    let core = TestCore::start().await;
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;

    let first = smtp
        .send_transaction(
            "one@acme.test",
            &["a@example.com"],
            b"Subject: First\r\n\r\nBody\r\n.\r\n",
        )
        .await;
    let second = smtp
        .send_transaction(
            "two@acme.test",
            &["b@example.com", "c@example.com"],
            b"Subject: Second\r\n\r\nBody\r\n.\r\n",
        )
        .await;

    assert_eq!((first.code, second.code), (250, 250));
    let emails = core.inbox().await;
    let listed: Vec<_> = emails
        .iter()
        .map(|email| {
            (
                email.subject.as_deref().unwrap(),
                email.envelope_sender.as_str(),
                email.envelope_recipients.clone(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            (
                "Second",
                "two@acme.test",
                vec!["b@example.com".to_string(), "c@example.com".to_string()]
            ),
            ("First", "one@acme.test", vec!["a@example.com".to_string()]),
        ]
    );
}

#[tokio::test]
async fn rset_abandons_the_transaction_in_progress() {
    let core = TestCore::start().await;
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;
    smtp.command("MAIL FROM:<abandoned@acme.test>").await;
    smtp.command("RCPT TO:<dropped@example.com>").await;

    assert_eq!(smtp.command("RSET").await.code, 250);
    assert_eq!(
        smtp.command("DATA").await.code,
        503,
        "no transaction left after RSET"
    );

    let reply = smtp
        .send_transaction(
            "kept@acme.test",
            &["kept@example.com"],
            b"Subject: Kept\r\n\r\n.\r\n",
        )
        .await;
    assert_eq!(reply.code, 250);
    let emails = core.inbox().await;
    assert_eq!(emails.len(), 1);
    assert_eq!(emails[0].envelope_sender, "kept@acme.test");
    assert_eq!(emails[0].envelope_recipients, ["kept@example.com"]);
}

#[tokio::test]
async fn a_helo_session_with_noop_captures_then_quits() {
    let core = TestCore::start().await;
    let (mut smtp, greeting) = SmtpClient::connect(core.smtp_port()).await;
    assert_eq!(greeting.code, 220);

    assert_eq!(smtp.command("HELO client.test").await.code, 250);
    assert_eq!(smtp.command("NOOP").await.code, 250);
    let reply = smtp
        .send_transaction(
            "app@acme.test",
            &["alice@example.com"],
            b"Subject: Hello\r\n\r\n.\r\n",
        )
        .await;
    assert_eq!(reply.code, 250);
    assert_eq!(smtp.command("QUIT").await.code, 221);

    assert!(
        smtp.is_closed().await,
        "the server closes the connection after QUIT"
    );
    assert_eq!(core.inbox().await.len(), 1);
}

#[tokio::test]
async fn each_capture_emits_an_email_captured_event() {
    let core = TestCore::start().await;
    let mut events = core.subscribe();

    for subject in ["First", "Second"] {
        let message = Message::builder()
            .from("app@acme.test".parse().unwrap())
            .to("alice@example.com".parse().unwrap())
            .subject(subject)
            .body(String::from("Hi"))
            .unwrap();
        send_with_lettre(core.smtp_port(), message).await;
    }

    let mut captured = Vec::new();
    for _ in 0..2 {
        match tokio::time::timeout(STEP_TIMEOUT, events.recv()).await {
            Ok(Ok(CoreEvent::EmailCaptured(email))) => captured.push(email),
            other => panic!("expected an Email Captured event, got {other:?}"),
        }
    }
    let subjects: Vec<_> = captured
        .iter()
        .map(|email| email.subject.as_deref())
        .collect();
    assert_eq!(subjects, [Some("First"), Some("Second")]);
    let mut listed = core.inbox().await;
    listed.reverse();
    assert_eq!(captured, listed, "events carry the Emails as listed");
}

#[tokio::test]
async fn the_list_shows_the_from_and_to_header_addresses_apart_from_the_envelope() {
    let core = TestCore::start().await;
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;

    smtp.send_transaction(
        "bounces@mailer.test",
        &["alice@example.com", "bob@example.com", "audit@acme.test"],
        b"From: Acme Shop <shop@acme.test>\r\n\
          To: Alice <alice@example.com>, bob@example.com\r\n\
          Subject: Your order\r\n\
          \r\n\
          Thanks\r\n\
          .\r\n",
    )
    .await;

    let email = &core.inbox().await[0];
    assert_eq!(email.from.as_deref(), Some("shop@acme.test"));
    assert_eq!(email.to, ["alice@example.com", "bob@example.com"]);
    assert_eq!(email.envelope_sender, "bounces@mailer.test");
}

#[tokio::test]
async fn the_sender_is_the_from_header_address_else_the_envelope_sender() {
    let core = TestCore::start().await;
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;
    for (envelope_sender, data) in [
        (
            "bounces@mailer.test",
            "From: Acme Shop <shop@acme.test>\r\nSubject: With From\r\n\r\n.\r\n",
        ),
        ("app@acme.test", "Subject: Without From\r\n\r\n.\r\n"),
        ("", "Subject: Null sender\r\n\r\n.\r\n"),
    ] {
        let reply = smtp
            .send_transaction(envelope_sender, &["alice@example.com"], data.as_bytes())
            .await;
        assert_eq!(reply.code, 250);
    }

    let emails = core.inbox().await;
    let senders: Vec<_> = emails
        .iter()
        .map(|email| (email.subject.as_deref().unwrap(), email.sender()))
        .collect();
    assert_eq!(
        senders,
        [
            ("Null sender", None),
            ("Without From", Some("app@acme.test")),
            ("With From", Some("shop@acme.test")),
        ]
    );
}
