//! Emails leave the Inbox when the developer deletes them from the window or
//! the tray (through the handle), or when an agent deletes them over MCP.

mod common;

use common::{STEP_TIMEOUT, SmtpClient, TestCore};
use sendbox_core::{CoreEvent, EmailFilter, EmailId, EmailSelection, EmailSummary};
use serde_json::json;
use tokio::sync::broadcast;
use tokio::time::timeout;

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

fn subjects(emails: &[EmailSummary]) -> Vec<&str> {
    emails
        .iter()
        .map(|email| email.subject.as_deref().expect("a subject"))
        .collect()
}

#[tokio::test]
async fn deleting_emails_by_id_removes_them_for_good_and_says_how_many_were_deleted() {
    let core = TestCore::start().await;
    let ids = capture(
        &core,
        &[
            ("First", "alice@example.com"),
            ("Second", "alice@example.com"),
            ("Third", "alice@example.com"),
        ],
    )
    .await;
    let unknown = EmailId(ids[0].0 + 100);

    let deleted = core
        .delete_emails(EmailSelection::Ids(vec![ids[0], ids[2], unknown]))
        .await
        .unwrap();

    assert_eq!(deleted, 2, "the unknown id deletes nothing");
    assert_eq!(subjects(&core.inbox().await), ["Second"]);
    assert!(core.email(ids[0]).await.unwrap().is_none());
    let core = core.restart().await;
    assert_eq!(
        subjects(&core.inbox().await),
        ["Second"],
        "they stay deleted after a restart"
    );
}

#[tokio::test]
async fn deleting_by_filter_removes_exactly_the_emails_the_filter_lists() {
    let core = TestCore::start().await;
    capture(
        &core,
        &[
            ("Welcome", "alice@example.com"),
            ("Reset your password", "bob@example.com"),
            ("Your invoice", "Alice@Example.com"),
        ],
    )
    .await;
    let to_alice = EmailFilter {
        to: Some("ALICE@".to_string()),
        ..EmailFilter::default()
    };

    let deleted = core
        .delete_emails(EmailSelection::Matching(to_alice))
        .await
        .unwrap();

    assert_eq!(deleted, 2);
    assert_eq!(subjects(&core.inbox().await), ["Reset your password"]);
}

#[tokio::test]
async fn clearing_all_empties_the_inbox_and_capture_goes_on() {
    let core = TestCore::start().await;
    capture(&core, &[("One", "a@example.com"), ("Two", "b@example.com")]).await;

    let deleted = core.delete_emails(EmailSelection::All).await.unwrap();

    assert_eq!(deleted, 2);
    assert!(core.inbox().await.is_empty());
    capture(&core, &[("Three", "c@example.com")]).await;
    assert_eq!(subjects(&core.inbox().await), ["Three"]);
}

/// The next event, failing the test if none comes in time.
async fn next_event(events: &mut broadcast::Receiver<CoreEvent>) -> CoreEvent {
    timeout(STEP_TIMEOUT, events.recv())
        .await
        .expect("an event comes in time")
        .expect("the core is still running")
}

#[tokio::test]
async fn a_deletion_emits_an_inbox_changed_event_unless_it_deleted_nothing() {
    let core = TestCore::start().await;
    let ids = capture(&core, &[("Welcome", "alice@example.com")]).await;
    let mut events = core.subscribe();

    core.delete_emails(EmailSelection::Ids(vec![EmailId(ids[0].0 + 100)]))
        .await
        .unwrap();
    core.delete_emails(EmailSelection::Ids(ids)).await.unwrap();

    assert_eq!(
        next_event(&mut events).await,
        CoreEvent::InboxChanged,
        "the first deletion deleted nothing, so the first event is the second's"
    );
    core.delete_emails(EmailSelection::All).await.unwrap();
    capture(&core, &[("Next", "alice@example.com")]).await;
    assert!(
        matches!(next_event(&mut events).await, CoreEvent::EmailCaptured(_)),
        "clearing an empty Inbox changed nothing"
    );
}

#[tokio::test]
async fn an_agent_deletes_emails_by_id_and_the_window_hears_the_inbox_changed() {
    let core = TestCore::start().await;
    let ids = capture(
        &core,
        &[
            ("First", "alice@example.com"),
            ("Second", "alice@example.com"),
            ("Third", "alice@example.com"),
        ],
    )
    .await;
    let mut events = core.subscribe();

    let result = core
        .agent()
        .await
        .call_ok(
            "delete_emails",
            json!({ "ids": [ids[0].0, ids[2].0, 9999] }),
        )
        .await;

    assert_eq!(result, json!({ "deleted": 2 }));
    assert_eq!(subjects(&core.inbox().await), ["Second"]);
    assert_eq!(next_event(&mut events).await, CoreEvent::InboxChanged);
}

#[tokio::test]
async fn an_agent_deletes_the_emails_the_list_filters_match() {
    let core = TestCore::start().await;
    capture(
        &core,
        &[
            ("Welcome", "alice@example.com"),
            ("Welcome", "bob@example.com"),
            ("Your invoice", "alice@example.com"),
            ("Welcome back", "Alice@Example.com"),
        ],
    )
    .await;
    let agent = core.agent().await;

    let result = agent
        .call_ok(
            "delete_emails",
            json!({ "to": "ALICE@", "subject": "welcome" }),
        )
        .await;

    assert_eq!(result, json!({ "deleted": 2 }));
    assert_eq!(subjects(&core.inbox().await), ["Your invoice", "Welcome"]);
    assert_eq!(
        core.inbox().await[1].envelope_recipients,
        ["bob@example.com"],
        "the Welcome left is Bob's"
    );
}

/// The text of a tool error, failing the test if the call succeeded.
fn tool_error(result: &rmcp::model::CallToolResult) -> &str {
    assert_eq!(result.is_error, Some(true), "the call fails: {result:?}");
    &result.content[0].as_text().expect("a text message").text
}

#[tokio::test]
async fn an_agent_empties_the_inbox_only_by_asking_for_all_explicitly() {
    let core = TestCore::start().await;
    capture(
        &core,
        &[
            ("One", "a@example.com"),
            ("Two", "b@example.com"),
            ("Three", "c@example.com"),
        ],
    )
    .await;
    let agent = core.agent().await;

    for no_selector in [
        json!({}),
        json!({ "all": false }),
        json!({ "unread_only": false }),
    ] {
        let result = agent.call("delete_emails", no_selector.clone()).await;
        let message = tool_error(&result);
        assert!(message.contains("all: true"), "{no_selector}: {message}");
        assert_eq!(core.inbox().await.len(), 3, "{no_selector} deleted nothing");
    }
    let result = agent.call_ok("delete_emails", json!({ "all": true })).await;

    assert_eq!(result, json!({ "deleted": 3 }));
    assert!(core.inbox().await.is_empty());
}

#[tokio::test]
async fn a_deletion_mixing_ids_filters_and_all_is_rejected_as_ambiguous() {
    let core = TestCore::start().await;
    let ids = capture(
        &core,
        &[("Welcome", "a@example.com"), ("Invoice", "b@example.com")],
    )
    .await;
    let agent = core.agent().await;

    for mixed in [
        json!({ "ids": [ids[0].0], "subject": "Invoice" }),
        json!({ "ids": [ids[0].0], "all": true }),
        json!({ "all": true, "from": "nobody@example.com" }),
    ] {
        let result = agent.call("delete_emails", mixed.clone()).await;

        let message = tool_error(&result);
        assert!(message.contains("only one"), "{mixed}: {message}");
        assert_eq!(core.inbox().await.len(), 2, "{mixed} deleted nothing");
    }
}
