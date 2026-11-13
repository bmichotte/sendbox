//! The settings a core starts with and the shell changes while it runs: the
//! Retention limit (the Inbox keeps at most that many Emails, pruning the
//! oldest, read or not) and whether MCP reads mark Emails as read.

mod common;

use std::num::NonZeroUsize;

use common::{STEP_TIMEOUT, SmtpClient, TestCore, test_config};
use sendbox_core::{Config, CoreEvent};
use serde_json::json;
use tempfile::TempDir;
use tokio::time::timeout;

fn limit(emails: usize) -> NonZeroUsize {
    NonZeroUsize::new(emails).expect("a Retention limit of at least one Email")
}

async fn start_with_retention_limit(emails: usize) -> TestCore {
    let data_dir = TempDir::new().unwrap();
    let config = Config {
        retention_limit: limit(emails),
        ..test_config(&data_dir)
    };
    TestCore::start_with_config(config, data_dir).await
}

/// Captures one Email per subject, oldest first.
async fn capture(core: &TestCore, subjects: &[&str]) {
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;
    for subject in subjects {
        let raw = format!("Subject: {subject}\r\n\r\nHi\r\n");
        smtp.capture("app@acme.test", &["alice@example.com"], &raw)
            .await;
    }
}

/// The subjects of the Emails in the Inbox, newest first.
async fn subjects(core: &TestCore) -> Vec<String> {
    core.inbox()
        .await
        .into_iter()
        .map(|email| email.subject.expect("a subject"))
        .collect()
}

#[tokio::test]
async fn each_capture_prunes_the_oldest_emails_beyond_the_retention_limit_read_or_not() {
    let core = start_with_retention_limit(3).await;
    capture(&core, &["1", "2", "3"]).await;
    let second = core.inbox().await[1].id;
    core.mark_read(second).await.unwrap();
    assert_eq!(
        subjects(&core).await,
        ["3", "2", "1"],
        "the limit is not exceeded yet"
    );

    capture(&core, &["4", "5"]).await;

    assert_eq!(subjects(&core).await, ["5", "4", "3"]);
}

#[tokio::test]
async fn lowering_the_retention_limit_prunes_the_oldest_emails_at_once_and_says_the_inbox_changed()
{
    let core = TestCore::start().await;
    capture(&core, &["1", "2", "3", "4", "5"]).await;
    let mut events = core.subscribe();

    core.set_retention_limit(limit(2)).await.unwrap();

    assert_eq!(subjects(&core).await, ["5", "4"]);
    let event = timeout(STEP_TIMEOUT, events.recv())
        .await
        .expect("an event in time")
        .expect("an event");
    assert_eq!(event, CoreEvent::InboxChanged);
    capture(&core, &["6"]).await;
    assert_eq!(
        subjects(&core).await,
        ["6", "5"],
        "Captures keep to the new limit"
    );
}

// The shell takes its defaults from `Config::new`. Checking the default by
// Capturing 1001 Emails would take seconds on Windows, where every commit
// is flushed to disk.
#[test]
fn the_retention_limit_is_1000_emails_by_default() {
    assert_eq!(Config::new("inbox").retention_limit.get(), 1000);
}

#[tokio::test]
async fn a_core_started_with_a_lower_retention_limit_prunes_its_inbox_at_once() {
    let core = TestCore::start().await;
    capture(&core, &["1", "2", "3"]).await;
    let TestCore { handle, data_dir } = core;
    handle.shutdown().await;
    let config = Config {
        retention_limit: limit(1),
        ..test_config(&data_dir)
    };

    let core = TestCore::start_with_config(config, data_dir).await;

    assert_eq!(subjects(&core).await, ["3"]);
}

#[tokio::test]
async fn turning_mcp_read_marking_off_and_on_applies_to_an_agent_already_connected() {
    let core = TestCore::start().await;
    capture(&core, &["1", "2", "3"]).await;
    let ids: Vec<i64> = core.inbox().await.iter().map(|email| email.id.0).collect();
    let agent = core.agent().await;

    core.set_mcp_marks_read(false);
    let first = agent.call_ok("get_email", json!({ "id": ids[2] })).await;
    core.set_mcp_marks_read(true);
    let second = agent.call_ok("get_email", json!({ "id": ids[1] })).await;

    assert_eq!(
        (&first["subject"], &first["read"]),
        (&json!("1"), &json!(false))
    );
    assert_eq!(
        (&second["subject"], &second["read"]),
        (&json!("2"), &json!(true))
    );
    let read: Vec<bool> = core.inbox().await.iter().map(|email| email.read).collect();
    assert_eq!(read, [false, true, false]);
}
