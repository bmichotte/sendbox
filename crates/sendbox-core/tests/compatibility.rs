//! Real SMTP clients work against Sendbox with no config tweaks, and protocol
//! edge cases are Captured or rejected cleanly without breaking the session.

mod common;

use common::{SmtpClient, TestCore, lettre_transport, send_through, send_with_lettre};
use lettre::Message;
use lettre::message::header::ContentTransferEncoding;
use lettre::transport::smtp::authentication::{Credentials, Mechanism};

fn welcome_email() -> Message {
    Message::builder()
        .from("app@acme.test".parse().unwrap())
        .to("alice@example.com".parse().unwrap())
        .subject("Welcome")
        .body(String::from("Hello Alice"))
        .unwrap()
}

/// Credentials Sendbox has never heard of: it accepts any.
fn any_credentials() -> Credentials {
    Credentials::new("any-user".into(), "any-password".into())
}

/// The advertised SIZE limit: 25 MiB.
const SIZE_LIMIT: usize = 26_214_400;

/// A raw source of exactly `size` bytes, in lines of 1000 bytes.
fn raw_source_of_size(size: usize) -> Vec<u8> {
    let line = [vec![b'x'; 998], b"\r\n".to_vec()].concat();
    let mut raw_source = b"Subject: Big\r\n\r\n".to_vec();
    while raw_source.len() < size {
        raw_source.extend_from_slice(&line);
    }
    raw_source.truncate(size - 2);
    raw_source.extend_from_slice(b"\r\n");
    raw_source
}

#[tokio::test]
async fn ehlo_advertises_auth_8bitmime_smtputf8_and_size_but_not_starttls_or_pipelining() {
    let core = TestCore::start().await;
    let (mut smtp, _) = SmtpClient::connect(core.smtp_port()).await;

    // nodemailer greets with an address literal.
    let ehlo = smtp.command("EHLO [127.0.0.1]").await;

    assert_eq!(ehlo.code, 250);
    let mut extensions = ehlo.lines[1..].to_vec();
    extensions.sort();
    assert_eq!(
        extensions,
        ["8BITMIME", "AUTH PLAIN LOGIN", "SIZE 26214400", "SMTPUTF8"],
        "nodemailer treats a failed STARTTLS upgrade as fatal, and PIPELINING is not supported"
    );
}

#[tokio::test]
async fn utf8_addresses_and_subject_and_an_8bit_body_are_captured_intact() {
    let core = TestCore::start().await;
    let message = Message::builder()
        .from("Jöhn <jöhn@exämple.test>".parse().unwrap())
        .to("Zoë <zoë@exämple.test>".parse().unwrap())
        .subject("Café ☕ prêt")
        .header(ContentTransferEncoding::EightBit)
        .body(String::from("Voilà, déjà prêt ☕\r\n"))
        .unwrap();
    // lettre ends DATA with "\r\n.\r\n": the CRLF before the dot belongs to the Email.
    let sent = [message.formatted(), b"\r\n".to_vec()].concat();

    send_with_lettre(core.smtp_port(), message).await;

    let emails = core.inbox().await;
    assert_eq!(emails.len(), 1);
    let email = &emails[0];
    assert_eq!(email.envelope_sender.as_deref(), Some("jöhn@exämple.test"));
    assert_eq!(email.envelope_recipients, ["zoë@exämple.test"]);
    assert_eq!(email.subject.as_deref(), Some("Café ☕ prêt"));
    assert_eq!(core.raw_source(email.id).await.unwrap(), Some(sent));
}

#[tokio::test]
async fn an_authenticated_lettre_session_without_tls_is_captured() {
    let core = TestCore::start().await;
    let transport = lettre_transport(core.smtp_port()).credentials(any_credentials());

    send_through(transport, welcome_email()).await;

    let emails = core.inbox().await;
    assert_eq!(emails.len(), 1);
    assert_eq!(emails[0].subject.as_deref(), Some("Welcome"));
}

#[tokio::test]
async fn auth_login_succeeds_with_any_credentials() {
    let core = TestCore::start().await;
    let transport = lettre_transport(core.smtp_port())
        .credentials(any_credentials())
        .authentication(vec![Mechanism::Login]);

    send_through(transport, welcome_email()).await;

    assert_eq!(core.inbox().await.len(), 1);
}

#[tokio::test]
async fn auth_plain_without_an_initial_response_takes_the_credentials_on_the_next_line() {
    let core = TestCore::start().await;
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;

    let challenge = smtp.command("AUTH PLAIN").await;
    assert_eq!((challenge.code, challenge.lines[0].as_str()), (334, ""));

    // base64 of "\0user\0pass"
    assert_eq!(smtp.command("AHVzZXIAcGFzcw==").await.code, 235);
}

#[tokio::test]
async fn auth_login_with_the_username_on_the_command_line_asks_only_for_the_password() {
    let core = TestCore::start().await;
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;

    // base64 of "user"
    let challenge = smtp.command("AUTH LOGIN dXNlcg==").await;
    // base64 of "Password:"
    assert_eq!(
        (challenge.code, challenge.lines[0].as_str()),
        (334, "UGFzc3dvcmQ6")
    );

    // base64 of "pass"
    assert_eq!(smtp.command("cGFzcw==").await.code, 235);
}

#[tokio::test]
async fn a_cancelled_or_unknown_auth_is_refused_and_the_session_goes_on() {
    let core = TestCore::start().await;
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;

    assert_eq!(smtp.command("AUTH LOGIN").await.code, 334);
    assert_eq!(smtp.command("*").await.code, 501, "the client cancelled");
    assert_eq!(smtp.command("AUTH CRAM-MD5").await.code, 504);

    let reply = smtp
        .send_transaction(
            "app@acme.test",
            &["alice@example.com"],
            b"Subject: Still here\r\n\r\n.\r\n",
        )
        .await;
    assert_eq!(reply.code, 250);
    assert_eq!(core.inbox().await.len(), 1);
}

#[tokio::test]
async fn a_declared_size_over_the_limit_is_rejected_at_mail() {
    let core = TestCore::start().await;
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;

    let reply = smtp
        .command("MAIL FROM:<app@acme.test> SIZE=26214401")
        .await;

    assert_eq!(reply.code, 552);
    assert_eq!(
        smtp.command("RCPT TO:<alice@example.com>").await.code,
        503,
        "no transaction was started"
    );
    assert_eq!(
        smtp.command("MAIL FROM:<app@acme.test> SIZE=26214400")
            .await
            .code,
        250,
        "the limit itself is allowed"
    );
}

#[tokio::test]
async fn data_over_the_size_limit_is_rejected_and_nothing_is_captured() {
    let core = TestCore::start().await;
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;
    let too_large = [raw_source_of_size(SIZE_LIMIT + 1), b".\r\n".to_vec()].concat();

    let reply = smtp
        .send_transaction("app@acme.test", &["alice@example.com"], &too_large)
        .await;

    assert_eq!(reply.code, 552);
    assert!(core.inbox().await.is_empty());
    let next = smtp
        .send_transaction(
            "app@acme.test",
            &["alice@example.com"],
            b"Subject: Small\r\n\r\n.\r\n",
        )
        .await;
    assert_eq!(next.code, 250, "the session goes on after the rejection");
    assert_eq!(core.inbox().await.len(), 1);
}

#[tokio::test]
async fn an_email_exactly_at_the_size_limit_is_captured() {
    let core = TestCore::start().await;
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;
    let at_limit = [raw_source_of_size(SIZE_LIMIT), b".\r\n".to_vec()].concat();

    let reply = smtp
        .send_transaction("app@acme.test", &["alice@example.com"], &at_limit)
        .await;

    assert_eq!(reply.code, 250);
    let emails = core.inbox().await;
    assert_eq!(emails[0].size, SIZE_LIMIT as u64);
}

#[tokio::test]
async fn a_null_sender_is_captured() {
    let core = TestCore::start().await;
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;

    // send_transaction sends "MAIL FROM:<>" for an empty sender.
    let reply = smtp
        .send_transaction(
            "",
            &["alice@example.com"],
            b"Subject: Undeliverable\r\n\r\nBounce\r\n.\r\n",
        )
        .await;

    assert_eq!(reply.code, 250);
    let emails = core.inbox().await;
    assert_eq!(emails.len(), 1);
    assert_eq!(emails[0].envelope_sender, None);
}

#[tokio::test]
async fn mail_from_parameters_body_smtputf8_and_size_are_accepted() {
    let core = TestCore::start().await;
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;

    let reply = smtp
        .command("MAIL FROM:<app@acme.test> BODY=8BITMIME SMTPUTF8 SIZE=1234")
        .await;

    assert_eq!(reply.code, 250);
}

#[tokio::test]
async fn raw_utf8_headers_are_captured_intact() {
    let core = TestCore::start().await;
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;
    assert_eq!(
        smtp.command("MAIL FROM:<jöhn@exämple.test> SMTPUTF8 BODY=8BITMIME")
            .await
            .code,
        250
    );
    assert_eq!(smtp.command("RCPT TO:<zoë@exämple.test>").await.code, 250);
    assert_eq!(smtp.command("DATA").await.code, 354);
    let raw_source = "From: Jöhn <jöhn@exämple.test>\r\n\
        Subject: Café ☕ prêt\r\n\
        \r\n\
        Voilà\r\n";

    smtp.write(format!("{raw_source}.\r\n").as_bytes()).await;

    assert_eq!(smtp.read_reply().await.code, 250);
    let emails = core.inbox().await;
    assert_eq!(
        emails[0].envelope_sender.as_deref(),
        Some("jöhn@exämple.test")
    );
    assert_eq!(emails[0].envelope_recipients, ["zoë@exämple.test"]);
    assert_eq!(emails[0].subject.as_deref(), Some("Café ☕ prêt"));
    assert_eq!(
        core.raw_source(emails[0].id).await.unwrap(),
        Some(raw_source.as_bytes().to_vec())
    );
}
