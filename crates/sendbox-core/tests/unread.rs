//! What nobody has read yet: the window and the tray show the total Unread
//! count, and hear live whenever it may have changed, including when an
//! agent reads an Email over MCP.

mod common;

use common::{STEP_TIMEOUT, SmtpClient, TestCore, test_config};
use sendbox_core::{Config, CoreEvent, EmailId, EmailSelection};
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::broadcast;
use tokio::time::timeout;

/// Captures one Email per subject, oldest first, and returns their ids
/// newest first.
async fn capture(core: &TestCore, subjects: &[&str]) -> Vec<EmailId> {
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;
    for subject in subjects {
        let raw = format!("From: app@acme.test\r\nSubject: {subject}\r\n\r\nHi\r\n");
        smtp.capture("app@acme.test", &["alice@example.com"], &raw)
            .await;
    }
    core.inbox().await.iter().map(|email| email.id).collect()
}

#[tokio::test]
async fn the_handle_counts_the_unread_emails_as_they_are_captured_read_and_deleted() {
    let core = TestCore::start().await;
    assert_eq!(core.unread_count().await.unwrap(), 0, "an empty Inbox");
    let ids = capture(&core, &["First", "Second", "Third"]).await;
    assert_eq!(core.unread_count().await.unwrap(), 3);

    core.mark_read(ids[0]).await.unwrap();
    core.mark_read(ids[0]).await.unwrap();
    assert_eq!(
        core.unread_count().await.unwrap(),
        2,
        "reading an Email twice counts once"
    );

    core.delete_emails(EmailSelection::Ids(vec![ids[0], ids[1]]))
        .await
        .unwrap();
    assert_eq!(
        core.unread_count().await.unwrap(),
        1,
        "deleting a read Email and an Unread one leaves one Unread"
    );
}

/// The next event, failing the test if none comes in time.
async fn next_event(events: &mut broadcast::Receiver<CoreEvent>) -> CoreEvent {
    timeout(STEP_TIMEOUT, events.recv())
        .await
        .expect("an event comes in time")
        .expect("the core is still running")
}

#[tokio::test]
async fn marking_an_email_read_emits_an_inbox_changed_event_unless_it_was_read_already() {
    let core = TestCore::start().await;
    let ids = capture(&core, &["Welcome"]).await;
    let mut events = core.subscribe();

    core.mark_read(ids[0]).await.unwrap();
    core.mark_read(ids[0]).await.unwrap();
    core.mark_read(EmailId(ids[0].0 + 100)).await.unwrap();
    capture(&core, &["Next"]).await;

    assert_eq!(next_event(&mut events).await, CoreEvent::InboxChanged);
    assert!(
        matches!(next_event(&mut events).await, CoreEvent::EmailCaptured(_)),
        "reading it again, or an Email that does not exist, changed nothing"
    );
}

#[tokio::test]
async fn an_email_an_agent_waits_for_becomes_read_and_the_window_hears_it() {
    let core = TestCore::start().await;
    capture(&core, &["Verify your address"]).await;
    let mut events = core.subscribe();

    let waited = core
        .agent()
        .await
        .call_ok("wait_for_email", json!({ "subject": "verify" }))
        .await;

    assert_eq!(waited["email"]["subject"], "Verify your address");
    assert_eq!(next_event(&mut events).await, CoreEvent::InboxChanged);
    assert_eq!(core.unread_count().await.unwrap(), 0);
}

#[tokio::test]
async fn an_agent_reading_an_email_over_mcp_lowers_the_unread_count_and_the_window_hears_it() {
    let core = TestCore::start().await;
    let ids = capture(&core, &["Older", "Newer"]).await;
    let mut events = core.subscribe();

    core.agent()
        .await
        .call_ok("get_email", json!({ "id": ids[0].0 }))
        .await;

    assert_eq!(next_event(&mut events).await, CoreEvent::InboxChanged);
    assert_eq!(core.unread_count().await.unwrap(), 1);
}

#[tokio::test]
async fn with_mcp_read_marking_off_agents_change_nothing_but_the_window_still_marks_emails_read() {
    let data_dir = TempDir::new().unwrap();
    let config = Config {
        mcp_marks_read: false,
        ..test_config(&data_dir)
    };
    let core = TestCore::start_with_config(config, data_dir).await;
    let ids = capture(&core, &["Older", "Newer"]).await;
    let mut events = core.subscribe();
    let agent = core.agent().await;

    agent.call_ok("get_email", json!({ "id": ids[0].0 })).await;
    agent.call_ok("wait_for_email", json!({})).await;
    capture(&core, &["Latest"]).await;
    core.mark_read(ids[0]).await.unwrap();

    assert!(
        matches!(next_event(&mut events).await, CoreEvent::EmailCaptured(_)),
        "the agent's reads changed nothing"
    );
    assert_eq!(
        next_event(&mut events).await,
        CoreEvent::InboxChanged,
        "opening an Email in the window marks it read whatever the setting"
    );
    assert_eq!(core.unread_count().await.unwrap(), 2);
}
