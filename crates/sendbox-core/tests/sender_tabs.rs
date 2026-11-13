//! Sender tabs: the window splits the Inbox into one tab per sender, next to
//! "All", each with its Unread count, and searches the current tab with the
//! same filters agents use in `list_emails`.

mod common;

use common::{SmtpClient, TestCore};
use sendbox_core::{EmailFilter, EmailSelection, SenderTab, SenderTabKey};

/// Has the core Capture each `(envelope sender, From header)`, oldest first.
/// A `None` From header leaves it out; an empty envelope sender is a null
/// sender (`MAIL FROM:<>`).
async fn capture(core: &TestCore, emails: &[(&str, Option<&str>)]) {
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;
    for (envelope_sender, from) in emails {
        let from = from.map_or(String::new(), |from| format!("From: {from}\r\n"));
        let raw = format!("{from}Subject: Hello\r\n\r\nHi\r\n");
        smtp.capture(envelope_sender, &["alice@example.com"], &raw)
            .await;
    }
}

fn address(address: &str) -> SenderTabKey {
    SenderTabKey::Address(address.to_string())
}

/// Each tab's key, in the order the window shows them.
async fn tab_keys(core: &TestCore) -> Vec<SenderTabKey> {
    let tabs = core.sender_tabs().await.expect("the Sender tabs list");
    tabs.into_iter().map(|tab: SenderTab| tab.key).collect()
}

#[tokio::test]
async fn there_is_one_sender_tab_per_from_address_ignoring_display_name_and_case() {
    let core = TestCore::start().await;
    capture(
        &core,
        &[
            ("bounces@mailer.test", Some("Acme Shop <Shop@Acme.test>")),
            ("bounces@mailer.test", Some("shop@acme.test")),
            ("bounces@mailer.test", Some("\"Zoë\" <ZOË@exämple.test>")),
            ("bounces@mailer.test", Some("zoë@EXÄMPLE.test")),
        ],
    )
    .await;

    assert_eq!(
        tab_keys(&core).await,
        [address("zoë@exämple.test"), address("shop@acme.test")]
    );
}

#[tokio::test]
async fn an_email_without_a_from_address_is_in_its_envelope_senders_tab_else_in_no_sender() {
    let core = TestCore::start().await;
    capture(
        &core,
        &[
            ("", None),
            ("App@Acme.test", None),
            ("app@acme.test", Some("undisclosed")),
            ("", Some("Nobody")),
        ],
    )
    .await;

    assert_eq!(
        tab_keys(&core).await,
        [SenderTabKey::NoSender, address("app@acme.test")]
    );
}

#[tokio::test]
async fn each_sender_tab_counts_its_unread_emails() {
    let core = TestCore::start().await;
    capture(
        &core,
        &[
            ("shop@acme.test", None),
            ("shop@acme.test", None),
            ("shop@acme.test", None),
            ("app@acme.test", None),
        ],
    )
    .await;
    let inbox = core.inbox().await;
    core.mark_read(inbox[0].id).await.unwrap();
    core.mark_read(inbox[1].id).await.unwrap();

    assert_eq!(
        core.sender_tabs().await.unwrap(),
        [
            SenderTab {
                key: address("app@acme.test"),
                unread: 0,
            },
            SenderTab {
                key: address("shop@acme.test"),
                unread: 2,
            },
        ]
    );
}

#[tokio::test]
async fn sender_tabs_are_ordered_by_their_most_recent_email() {
    let core = TestCore::start().await;
    capture(
        &core,
        &[
            ("app@acme.test", None),
            ("shop@acme.test", None),
            ("shop@acme.test", None),
            ("shop@acme.test", None),
            ("app@acme.test", None),
            ("news@acme.test", None),
        ],
    )
    .await;

    assert_eq!(
        tab_keys(&core).await,
        [
            address("news@acme.test"),
            address("app@acme.test"),
            address("shop@acme.test"),
        ]
    );
}

#[tokio::test]
async fn a_sender_tab_comes_with_its_first_email_and_goes_with_its_last() {
    let core = TestCore::start().await;
    capture(&core, &[("shop@acme.test", None)]).await;
    capture(&core, &[("app@acme.test", None)]).await;
    assert_eq!(
        tab_keys(&core).await,
        [address("app@acme.test"), address("shop@acme.test")]
    );

    let newest = core.inbox().await[0].id;
    core.delete_emails(EmailSelection::Ids(vec![newest]))
        .await
        .unwrap();

    assert_eq!(tab_keys(&core).await, [address("shop@acme.test")]);
    core.delete_emails(EmailSelection::All).await.unwrap();
    assert_eq!(
        tab_keys(&core).await,
        [],
        "an empty Inbox has no Sender tab"
    );
}

/// The subjects of the Emails in `tab` that `filter` also matches, newest first.
async fn subjects_in(core: &TestCore, tab: SenderTabKey, filter: EmailFilter) -> Vec<String> {
    let filter = EmailFilter {
        sender_tab: Some(tab),
        ..filter
    };
    let emails = core.list_emails(&filter, None).await.expect("the list");
    emails
        .into_iter()
        .map(|email| email.subject.expect("a subject"))
        .collect()
}

#[tokio::test]
async fn the_list_can_show_only_the_emails_of_one_sender_tab() {
    let core = TestCore::start().await;
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;
    for (envelope_sender, headers) in [
        (
            "bounces@mailer.test",
            "From: Acme <Shop@Acme.test>\r\nSubject: Order",
        ),
        ("", "Subject: Bounce"),
        ("shop@acme.test", "Subject: Receipt"),
        (
            "bounces@mailer.test",
            "From: app@acme.test\r\nSubject: Welcome",
        ),
        ("shop@acme.test", "From: shop@acme.test\r\nSubject: Shipped"),
    ] {
        let raw = format!("{headers}\r\n\r\nHi\r\n");
        smtp.capture(envelope_sender, &["alice@example.com"], &raw)
            .await;
    }

    let shop = subjects_in(&core, address("shop@acme.test"), EmailFilter::default()).await;
    let no_sender = subjects_in(&core, SenderTabKey::NoSender, EmailFilter::default()).await;

    assert_eq!(shop, ["Shipped", "Receipt", "Order"]);
    assert_eq!(no_sender, ["Bounce"]);
}

#[tokio::test]
async fn searching_a_sender_tab_matches_from_to_and_subject_ignoring_case_in_any_script() {
    let core = TestCore::start().await;
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;
    for (from, to, subject) in [
        (
            "Café Shop <shop@acme.test>",
            "zoë@example.com",
            "Votre CAFÉ est prêt",
        ),
        ("shop@acme.test", "bob@example.com", "Votre café est prêt"),
        ("shop@acme.test", "ZOË@example.com", "Your receipt"),
        ("news@acme.test", "zoë@example.com", "Café news"),
    ] {
        let raw = format!("From: {from}\r\nTo: {to}\r\nSubject: {subject}\r\n\r\nHi\r\n");
        smtp.capture("bounces@mailer.test", &[to], &raw).await;
    }
    let shop = || address("shop@acme.test");
    let search = |from: Option<&str>, to: Option<&str>, subject: Option<&str>| EmailFilter {
        from: from.map(str::to_owned),
        to: to.map(str::to_owned),
        subject: subject.map(str::to_owned),
        ..EmailFilter::default()
    };

    assert_eq!(
        subjects_in(&core, shop(), search(None, None, Some("café"))).await,
        ["Votre café est prêt", "Votre CAFÉ est prêt"]
    );
    assert_eq!(
        subjects_in(&core, shop(), search(None, Some("Zoë@"), None)).await,
        ["Your receipt", "Votre CAFÉ est prêt"]
    );
    assert_eq!(
        subjects_in(&core, shop(), search(None, Some("zoë"), Some("café"))).await,
        ["Votre CAFÉ est prêt"],
        "the search fields narrow the tab together"
    );
    assert_eq!(
        subjects_in(&core, shop(), search(Some("NEWS@"), None, None)).await,
        Vec::<String>::new(),
        "the search never leaves the tab"
    );
}
