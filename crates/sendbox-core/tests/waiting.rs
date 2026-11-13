//! Agents wait for an Email: they trigger a flow in the app they test (a
//! sign-up, a password reset), then get the Email it sends in one call.

mod common;

use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, SecondsFormat, Utc};
use common::{Agent, SmtpClient, TestCore};
use sendbox_core::Config;
use serde_json::{Value, json};
use tempfile::TempDir;

/// Long enough for a wait to have started waiting before a Capture.
const WAIT_STARTED: Duration = Duration::from_millis(300);

/// Has the core Capture an Email from `sender` with this subject.
async fn capture(core: &TestCore, sender: &str, subject: &str) {
    let mut smtp = SmtpClient::after_ehlo(core.smtp_port()).await;
    let raw = format!("From: {sender}\r\nSubject: {subject}\r\n\r\nHi\r\n");
    smtp.capture(sender, &["alice@example.com"], &raw).await;
}

/// The Email `wait_for_email` returns for `arguments`, or `None` if none
/// arrived in time.
async fn wait_for_email(agent: &Agent, arguments: Value) -> Option<Value> {
    let mut waited = agent.call_ok("wait_for_email", arguments).await;
    match waited["email"].take() {
        Value::Null => None,
        email => Some(email),
    }
}

#[tokio::test]
async fn a_wait_returns_the_next_matching_email_as_soon_as_it_is_captured() {
    let core = TestCore::start().await;
    let agent = core.agent().await;
    let started = Instant::now();

    let (email, ()) = tokio::join!(
        wait_for_email(&agent, json!({ "subject": "verify" })),
        async {
            tokio::time::sleep(WAIT_STARTED).await;
            capture(&core, "app@acme.test", "Welcome").await;
            capture(&core, "app@acme.test", "Verify your Email").await;
        },
    );

    let email = email.expect("the Email arrived");
    assert_eq!(email["subject"], "Verify your Email");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "returned on Capture, not after the 30 s timeout: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_wait_returns_the_oldest_matching_unread_email_already_captured_right_away() {
    let core = TestCore::start().await;
    for subject in ["Reset your password", "Reset it again", "Welcome", "Reset!"] {
        capture(&core, "app@acme.test", subject).await;
    }
    let agent = core.agent().await;
    let oldest = core.inbox().await.last().expect("Captured").id.0;
    agent.call_ok("get_email", json!({ "id": oldest })).await;
    let started = Instant::now();

    let email = wait_for_email(&agent, json!({ "subject": "RESET" })).await;

    assert_eq!(
        email.expect("an Email already arrived")["subject"],
        "Reset it again",
        "the oldest one still Unread"
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "returned right away: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn consecutive_waits_return_each_email_once_as_get_email_shows_it() {
    let core = TestCore::start().await;
    capture(&core, "app@acme.test", "Your code is 1234").await;
    capture(&core, "app@acme.test", "Your code is 5678").await;
    let agent = core.agent().await;

    let first = wait_for_email(&agent, json!({ "subject": "code" })).await;
    let second = wait_for_email(&agent, json!({ "subject": "code" })).await;

    let first = first.expect("an Email already arrived");
    let second = second.expect("another Email already arrived");
    assert_eq!(first["subject"], "Your code is 1234");
    assert_eq!(second["subject"], "Your code is 5678");
    assert_eq!(first["read"], true, "the Email returned is read");
    let read = agent
        .call_ok("get_email", json!({ "id": first["id"] }))
        .await;
    assert_eq!(first, read, "the same view as get_email");
    assert!(
        core.inbox().await.iter().all(|email| email.read),
        "the window sees them read"
    );
}

#[tokio::test]
async fn a_wait_that_times_out_says_no_email_was_received_without_failing() {
    let core = TestCore::start().await;
    capture(&core, "app@acme.test", "Welcome").await;
    let agent = core.agent().await;
    let started = Instant::now();

    let result = agent
        .call(
            "wait_for_email",
            json!({ "subject": "verify", "timeout_seconds": 1 }),
        )
        .await;

    let elapsed = started.elapsed();
    assert_ne!(result.is_error, Some(true), "not an error: {result:?}");
    assert_eq!(result.structured_content, Some(json!({ "email": null })));
    let message = &result.content[0].as_text().expect("a text message").text;
    assert!(message.contains("No Email received"), "{message}");
    assert!(
        Duration::from_secs(1) <= elapsed && elapsed < Duration::from_secs(3),
        "waited the timeout: {elapsed:?}"
    );
}

#[tokio::test]
async fn with_read_marking_off_since_skips_the_emails_already_there() {
    let data_dir = TempDir::new().unwrap();
    let config = Config {
        mcp_marks_read: false,
        ..common::test_config(&data_dir)
    };
    let core = TestCore::start_with_config(config, data_dir).await;
    capture(&core, "app@acme.test", "Your code is 1234").await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let since =
        DateTime::<Utc>::from(SystemTime::now()).to_rfc3339_opts(SecondsFormat::Nanos, true);
    let agent = core.agent().await;

    let again = wait_for_email(&agent, json!({ "subject": "code" })).await;
    let (fresh, ()) = tokio::join!(
        wait_for_email(&agent, json!({ "subject": "code", "since": since })),
        async {
            tokio::time::sleep(WAIT_STARTED).await;
            capture(&core, "app@acme.test", "Your code is 5678").await;
        },
    );

    let again = again.expect("the Email is still Unread");
    assert_eq!(again["subject"], "Your code is 1234");
    assert_eq!(again["read"], false, "waiting left it Unread");
    let fresh = fresh.expect("a new Email arrived");
    assert_eq!(fresh["subject"], "Your code is 5678");
    assert_eq!(fresh["read"], false);
}

#[tokio::test]
async fn a_cancelled_wait_stops_at_once_and_takes_no_email() {
    let core = TestCore::start().await;
    let agent = core.agent().await;
    // Giving up on the call (dropping it) cancels it over MCP, as an agent
    // whose user interrupts it does.
    let abandoned = tokio::time::timeout(
        WAIT_STARTED,
        agent.call("wait_for_email", json!({ "subject": "verify" })),
    )
    .await;
    assert!(abandoned.is_err(), "nothing arrived yet");
    tokio::time::sleep(WAIT_STARTED).await;

    // A wait still running would take this Email (and mark it read) the
    // moment it is Captured.
    capture(&core, "app@acme.test", "Verify your Email").await;
    let email = wait_for_email(&agent, json!({ "subject": "verify", "timeout_seconds": 1 })).await;

    assert_eq!(
        email.expect("the cancelled wait left the Email")["subject"],
        "Verify your Email"
    );
}

#[tokio::test]
async fn concurrent_waits_with_different_filters_each_get_their_own_email() {
    let core = TestCore::start().await;
    let agent = core.agent().await;

    let (from_shop, from_auth, ()) = tokio::join!(
        wait_for_email(&agent, json!({ "from": "shop@" })),
        wait_for_email(&agent, json!({ "from": "auth@" })),
        async {
            tokio::time::sleep(WAIT_STARTED).await;
            capture(&core, "auth@acme.test", "Reset your password").await;
            capture(&core, "shop@acme.test", "Your order").await;
        },
    );

    assert_eq!(
        from_shop.expect("an Email from the shop")["subject"],
        "Your order"
    );
    assert_eq!(
        from_auth.expect("an Email from auth")["subject"],
        "Reset your password"
    );
}

#[tokio::test]
async fn concurrent_waits_for_the_same_emails_never_get_the_same_one() {
    let core = TestCore::start().await;
    let agent = core.agent().await;
    let other_agent = core.agent().await;
    let code = json!({ "subject": "code" });

    let (first, second, ()) = tokio::join!(
        wait_for_email(&agent, code.clone()),
        wait_for_email(&other_agent, code.clone()),
        async {
            tokio::time::sleep(WAIT_STARTED).await;
            capture(&core, "app@acme.test", "Your code is 1234").await;
            capture(&core, "app@acme.test", "Your code is 5678").await;
        },
    );

    let mut subjects = [first, second].map(|email| email.expect("an Email")["subject"].clone());
    subjects.sort_by_key(|subject| subject.to_string());
    assert_eq!(subjects, ["Your code is 1234", "Your code is 5678"]);
}
