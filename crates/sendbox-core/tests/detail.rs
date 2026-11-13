//! The window shows an Email's detail through the core handle: its HTML with
//! inline images it can display without fetching anything, and the content
//! of its attachments to save.

mod common;

use common::{SmtpClient, TestCore};
use sendbox_core::{EmailId, EmailView};

/// Captures `raw_source` into `core` and returns the new Email's id.
async fn capture(core: &TestCore, raw_source: &str) -> EmailId {
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;
    smtp.capture("app@acme.test", &["alice@example.com"], raw_source)
        .await;
    core.inbox().await[0].id
}

async fn view(core: &TestCore, id: EmailId) -> EmailView {
    core.email(id).await.unwrap().expect("the Email exists")
}

/// An order confirmation: HTML showing an inline logo, with a text
/// alternative.
const WITH_INLINE_LOGO: &str = "From: shop@acme.test\r\n\
    Subject: Your order\r\n\
    Content-Type: multipart/alternative; boundary=\"alternative\"\r\n\
    \r\n\
    --alternative\r\n\
    Content-Type: text/plain\r\n\
    \r\n\
    Thanks for your order.\r\n\
    --alternative\r\n\
    Content-Type: multipart/related; boundary=\"related\"\r\n\
    \r\n\
    --related\r\n\
    Content-Type: text/html\r\n\
    \r\n\
    <p><img src=\"cid:logo@acme\" alt=\"Acme\"> Thanks for your order.</p>\r\n\
    --related\r\n\
    Content-Type: image/png\r\n\
    Content-ID: <logo@acme>\r\n\
    Content-Transfer-Encoding: base64\r\n\
    \r\n\
    iVBORw0KGgo=\r\n\
    --related--\r\n\
    --alternative--\r\n";

#[tokio::test]
async fn inline_images_are_embedded_in_the_html_the_window_displays() {
    let core = TestCore::start().await;
    let id = capture(&core, WITH_INLINE_LOGO).await;

    let view = view(&core, id).await;

    assert_eq!(
        view.html_with_inline_images.as_deref(),
        Some(
            "<p><img src=\"data:image/png;base64,iVBORw0KGgo=\" alt=\"Acme\"> \
             Thanks for your order.</p>"
        )
    );
    assert!(
        view.html.expect("the HTML body").contains("cid:logo@acme"),
        "the HTML as written keeps its references, without the image's bytes"
    );
}

#[tokio::test]
async fn a_cid_reference_is_replaced_by_exactly_the_part_it_names() {
    let core = TestCore::start().await;
    let id = capture(
        &core,
        "Subject: Newsletter\r\n\
         Content-Type: multipart/related; boundary=\"related\"\r\n\
         \r\n\
         --related\r\n\
         Content-Type: text/html\r\n\
         \r\n\
         <body background=\"CID:bg@acme\">\r\n\
         <img src=cid:dot%40acme>\r\n\
         <div style=\"background: url(cid:dot@acme.test)\">\r\n\
         <img src=\"cid:missing@acme\"></div>\r\n\
         --related\r\n\
         Content-Type: image/gif\r\n\
         Content-ID: <dot@acme>\r\n\
         Content-Transfer-Encoding: base64\r\n\
         \r\n\
         R0lG\r\n\
         --related\r\n\
         Content-Type: image/png\r\n\
         Content-ID: <dot@acme.test>\r\n\
         Content-Transfer-Encoding: base64\r\n\
         \r\n\
         iVBORw0KGgo=\r\n\
         --related\r\n\
         Content-Type: image/jpeg\r\n\
         Content-ID: <bg@acme>\r\n\
         Content-Transfer-Encoding: base64\r\n\
         \r\n\
         /9j/\r\n\
         --related--\r\n",
    )
    .await;

    let html = view(&core, id).await.html_with_inline_images;

    // The scheme ignores case and the id may be URL-encoded (RFC 2392); an
    // id naming no part is left for the frame to show as a broken image.
    assert_eq!(
        html.as_deref(),
        Some(
            "<body background=\"data:image/jpeg;base64,/9j/\">\r\n\
             <img src=data:image/gif;base64,R0lG>\r\n\
             <div style=\"background: url(data:image/png;base64,iVBORw0KGgo=)\">\r\n\
             <img src=\"cid:missing@acme\"></div>"
        )
    );
}

/// An invoice Email: an inline logo, a PDF and a forwarded Email attached.
const WITH_ATTACHMENTS: &str = "Subject: Your invoice\r\n\
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
    Content-Type: image/png\r\n\
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
    --mixed--\r\n";

#[tokio::test]
async fn each_attachment_in_the_view_can_be_saved_with_its_decoded_content() {
    let core = TestCore::start().await;
    let id = capture(&core, WITH_ATTACHMENTS).await;
    let attachments = view(&core, id).await.attachments;

    let mut saved = Vec::new();
    for index in 0..attachments.len() {
        let content = core.attachment(id, index).await.unwrap();
        saved.push(content.expect("the attachment exists"));
    }

    let saved: Vec<_> = saved
        .iter()
        .map(|content| {
            (
                content.attachment.name.as_deref(),
                content.attachment.content_type.as_str(),
                content.bytes.as_slice(),
            )
        })
        .collect();
    assert_eq!(
        saved,
        [
            (None, "image/png", b"\x89PNG\r\n\x1a\n".as_slice()),
            (Some("invoice.pdf"), "application/pdf", b"%PDF-1.4\n"),
            (
                None,
                "message/rfc822",
                b"From: inner@example.com\r\nSubject: Forwarded\r\n\r\nInner body"
            ),
        ]
    );
}

#[tokio::test]
async fn there_is_nothing_to_save_past_the_last_attachment_or_of_a_missing_email() {
    let core = TestCore::start().await;
    let id = capture(&core, WITH_ATTACHMENTS).await;

    assert_eq!(core.attachment(id, 3).await.unwrap(), None);
    assert_eq!(core.attachment(EmailId(42), 0).await.unwrap(), None);
}
