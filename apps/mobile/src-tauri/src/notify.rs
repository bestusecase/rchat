//! Background message notifications.
//!
//! This cannot live in the webview: when the activity goes to the background
//! the WebView suspends its JavaScript, so the page never sees the engine event
//! that says a message arrived. The engine keeps running inside the foreground
//! service, so this module watches its event stream directly and posts through
//! JNI. Kotlin decides whether the activity is actually on screen; the message
//! body never leaves the process.

use crate::android;
use crate::Running;
use rchat_core::store::Store;
use tokio::sync::broadcast;

/// Follow engine events until the engine stops.
///
/// Detached on purpose: the task returns as soon as the engine's shutdown is
/// signalled, so a restarted engine gets a fresh one and no duplicate posters.
pub fn spawn(running: &Running) {
    let mut events = running.engine.events();
    let mut shutdown = running.engine.shutdown_handle();
    let identity = running.engine.identity_id().to_string();
    let store = running.engine.store();
    running.runtime.spawn(async move {
        loop {
            let event = tokio::select! {
                _ = shutdown.changed() => break,
                event = events.recv() => event,
            };
            let text = match event {
                Ok(text) => text,
                // Skipping an event only means skipping one notification; the
                // count is recomputed from the store for the next message.
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            };
            if !is_incoming_message(&text, &identity) {
                continue;
            }
            if crate::muted() {
                continue;
            }
            post(unread_total(&store));
        }
    });
}

/// Whether this event says "a peer sent us a message".
///
/// The engine publishes `message.new` for both directions, so the author is
/// what separates them: only a received message deserves a notification.
fn is_incoming_message(text: &str, identity: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return false;
    };
    if value.get("type").and_then(|field| field.as_str()) != Some("event") {
        return false;
    }
    if value.get("name").and_then(|field| field.as_str()) != Some("message.new") {
        return false;
    }
    matches!(
        value
            .pointer("/data/message/author_id")
            .and_then(|field| field.as_str()),
        Some(author) if author != identity
    )
}

/// Unread messages across every conversation: the number the notification
/// summarises. Never zero, because this only runs after a message was stored.
fn unread_total(store: &Store) -> u32 {
    let Ok(conversations) = store.list_conversations() else {
        return 1;
    };
    let total: i64 = conversations
        .iter()
        .map(|conversation| conversation.unread.max(0))
        .sum();
    total.clamp(1, i64::from(u32::MAX)) as u32
}

fn post(count: u32) {
    if !cfg!(target_os = "android") {
        return;
    }
    if let Err(err) = android::notify_messages(count) {
        android::log_error(&format!("could not post the message notification: {err}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDENTITY: &str = "self";

    fn event(name: &str, author: &str) -> String {
        serde_json::json!({
            "type": "event",
            "seq": 7,
            "name": name,
            "data": { "message": { "author_id": author } },
        })
        .to_string()
    }

    #[test]
    fn received_messages_notify() {
        assert!(is_incoming_message(&event("message.new", "peer"), IDENTITY));
    }

    #[test]
    fn our_own_messages_do_not_notify() {
        assert!(!is_incoming_message(&event("message.new", IDENTITY), IDENTITY));
    }

    #[test]
    fn unrelated_events_are_ignored() {
        assert!(!is_incoming_message(&event("message.state", "peer"), IDENTITY));
        assert!(!is_incoming_message("not json", IDENTITY));
        assert!(!is_incoming_message(
            &serde_json::json!({ "type": "hello" }).to_string(),
            IDENTITY
        ));
    }
}
