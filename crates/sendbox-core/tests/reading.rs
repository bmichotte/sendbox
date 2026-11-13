//! The shell reads the Inbox through the core handle: the same filtered
//! list, Email view and read state that agents get over MCP.

mod common;

use common::{SmtpClient, TestCore};
use sendbox_core::{EmailFilter, EmailId, EmailView};

/// Captures one Email per (subject, recipient), oldest first, and returns
/// their ids newest first.
async fn capture(core: &TestCore, emails: &[(&str, &str)]) -> Vec<EmailId> {
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;
    for (subject, recipient) in emails {
        let raw = format!("From: app@acme.test\r\nSubject: {subject}\r\n\r\nHi\r\n");
        smtp.capture("app@acme.test", &[recipient], &raw).await;
    }
    core.inbox().await.iter().map(|email| email.id).collect()
}

fn subjects(emails: &[sendbox_core::EmailSummary]) -> Vec<&str> {
    emails
        .iter()
        .map(|email| email.subject.as_deref().expect("a subject"))
        .collect()
}

#[tokio::test]
async fn the_handle_lists_the_emails_a_filter_matches_newest_first_up_to_a_limit() {
    let core = TestCore::start().await;
    capture(
        &core,
        &[
            ("Welcome", "alice@example.com"),
            ("Reset your password", "bob@example.com"),
            ("Your invoice", "Alice@Example.com"),
            ("Welcome back", "alice@example.com"),
        ],
    )
    .await;
    let to_alice = EmailFilter {
        to: Some("ALICE@".to_string()),
        ..EmailFilter::default()
    };

    let all_to_alice = core.list_emails(&to_alice, None).await.unwrap();
    let newest_to_alice = core.list_emails(&to_alice, Some(2)).await.unwrap();

    assert_eq!(
        subjects(&all_to_alice),
        ["Welcome back", "Your invoice", "Welcome"]
    );
    assert_eq!(subjects(&newest_to_alice), ["Welcome back", "Your invoice"]);
}

#[tokio::test]
async fn an_email_marked_read_through_the_handle_stays_read_after_a_restart() {
    let core = TestCore::start().await;
    let ids = capture(
        &core,
        &[
            ("Older", "alice@example.com"),
            ("Newer", "alice@example.com"),
        ],
    )
    .await;
    core.email(ids[1]).await.unwrap().expect("the Email exists");
    assert!(
        core.inbox().await.iter().all(|email| !email.read),
        "reading the view alone does not mark it read"
    );

    core.mark_read(ids[1]).await.unwrap();

    let unread_only = EmailFilter {
        unread_only: true,
        ..EmailFilter::default()
    };
    let unread = core.list_emails(&unread_only, None).await.unwrap();
    assert_eq!(subjects(&unread), ["Newer"]);
    let view = core.email(ids[1]).await.unwrap().expect("the Email exists");
    assert!(view.summary.read);
    let core = core.restart().await;
    let read: Vec<_> = core.inbox().await.iter().map(|email| email.read).collect();
    assert_eq!(read, [false, true]);
}

/// Captures `raw_source` and returns its Email view.
async fn view_of(raw_source: &str) -> EmailView {
    let core = TestCore::start().await;
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;
    smtp.capture("app@acme.test", &["alice@example.com"], raw_source)
        .await;
    let id = core.inbox().await[0].id;
    core.email(id).await.unwrap().expect("the Email exists")
}

#[tokio::test]
async fn an_html_only_email_has_no_text_and_its_links_are_its_decoded_link_targets() {
    let view = view_of(
        "From: app@acme.test\r\n\
         Subject: Reset\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         \r\n\
         <p><a href=\"#top\">Top</a>\r\n\
         <A HREF = \"https://acme.test/reset?u=1&amp;t=x&#38;y\">Reset</A>\r\n\
         <a href=https://acme.test/bare>bare</a>\r\n\
         <a href='mailto:help@acme.test'>help</a></p>\r\n",
    )
    .await;

    assert_eq!(view.text, None, "no text is made up from the HTML");
    assert!(view.html.expect("the HTML body").contains("<A HREF = "));
    assert_eq!(
        view.links,
        [
            "https://acme.test/reset?u=1&t=x&y",
            "https://acme.test/bare",
            "mailto:help@acme.test",
        ]
    );
}

#[tokio::test]
async fn urls_in_a_text_email_are_links_without_the_punctuation_around_them() {
    let view = view_of(
        "Subject: Links\r\n\
         \r\n\
         Go to https://acme.test/a. Or (https://acme.test/b), or\r\n\
         https://wiki.test/Foo_(bar)? Then HTTPS://acme.test/a again!\r\n",
    )
    .await;

    assert_eq!(view.html, None, "no HTML is made up from the text");
    assert_eq!(
        view.links,
        [
            "https://acme.test/a",
            "https://acme.test/b",
            "https://wiki.test/Foo_(bar)",
            "HTTPS://acme.test/a",
        ]
    );
}

#[tokio::test]
async fn encoded_headers_and_bodies_in_other_charsets_are_decoded() {
    let view = view_of(
        "From: =?UTF-8?Q?Zo=C3=AB?= <zoe@example.com>\r\n\
         Subject: =?UTF-8?B?Q2Fmw6kg4piV?= order\r\n\
         X-Folded: first\r\n second\r\n\
         Content-Type: text/plain; charset=iso-8859-1\r\n\
         Content-Transfer-Encoding: quoted-printable\r\n\
         \r\n\
         Un caf=E9, s'il vous pla=EEt\r\n",
    )
    .await;

    assert_eq!(view.summary.subject.as_deref(), Some("Café ☕ order"));
    assert_eq!(view.text.as_deref(), Some("Un café, s'il vous plaît\r\n"));
    let headers: Vec<_> = view
        .headers
        .iter()
        .map(|header| (header.name.as_str(), header.value.as_str()))
        .collect();
    assert_eq!(
        headers[..3],
        [
            ("From", "Zoë <zoe@example.com>"),
            ("Subject", "Café ☕ order"),
            ("X-Folded", "first second"),
        ]
    );
}

#[tokio::test]
async fn attachments_have_a_name_type_and_decoded_size_including_inline_images_and_emails() {
    let view = view_of(
        "Subject: Your order\r\n\
         Content-Type: multipart/mixed; boundary=\"mixed\"\r\n\
         \r\n\
         --mixed\r\n\
         Content-Type: multipart/related; boundary=\"related\"\r\n\
         \r\n\
         --related\r\n\
         Content-Type: text/html\r\n\
         \r\n\
         <img src=\"cid:logo@acme\">\r\n\
         --related\r\n\
         Content-Type: image/PNG\r\n\
         Content-ID: <logo@acme>\r\n\
         Content-Transfer-Encoding: base64\r\n\
         \r\n\
         iVBORw0KGgo=\r\n\
         --related--\r\n\
         --mixed\r\n\
         Content-Type: application/pdf; name=\"invoice.pdf\"\r\n\
         Content-Disposition: attachment; filename=\"invoice.pdf\"\r\n\
         Content-Transfer-Encoding: base64\r\n\
         \r\n\
         JVBERi0xLjQK\r\n\
         --mixed\r\n\
         Content-Type: message/rfc822\r\n\
         \r\n\
         From: inner@example.com\r\n\
         Subject: Forwarded\r\n\
         \r\n\
         Inner body\r\n\
         --mixed--\r\n",
    )
    .await;

    let attachments: Vec<_> = view
        .attachments
        .iter()
        .map(|attachment| {
            (
                attachment.name.as_deref(),
                attachment.content_type.as_str(),
                attachment.size,
            )
        })
        .collect();
    // Sizes: 8 bytes of PNG signature, 9 of "%PDF-1.4\n", and the forwarded
    // Email's 57 bytes up to the line break before the boundary.
    assert_eq!(
        attachments,
        [
            (None, "image/png", 8),
            (Some("invoice.pdf"), "application/pdf", 9),
            (None, "message/rfc822", 57),
        ]
    );
}

#[tokio::test]
async fn bytes_that_are_not_an_email_are_shown_as_text() {
    let view = view_of("just text, no header\r\n").await;

    assert_eq!(view.text.as_deref(), Some("just text, no header\r\n"));
    assert_eq!(view.html, None);
    assert!(view.headers.is_empty());
    assert!(view.attachments.is_empty());
}

#[tokio::test]
async fn there_is_no_view_of_an_email_that_does_not_exist() {
    let core = TestCore::start().await;

    assert_eq!(core.email(EmailId(42)).await.unwrap(), None);
}
