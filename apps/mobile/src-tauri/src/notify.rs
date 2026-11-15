//! Background message notifications.
//!
//! This cannot live in the webview: when the activity goes to the background
//! the WebView suspends its JavaScript, so the page never sees the engine event
//! that says a message arrived. The engine keeps running inside the foreground
//! service, so this module watches its event stream directly and posts through
//! JNI.
//!
//! What is posted is a conversation thread per conversation, not a bare count.
//! [`Threads`] is the whole state machine: an engine event and a statement about
//! who can see the app go in, and the snapshot Kotlin should render comes out.
//! Keeping the decisions here, away from the platform, is what makes them
//! testable without a device.

use crate::android;
use crate::Running;
use rchat_core::store::Store;
use serde::Serialize;
use std::sync::Mutex;
use tokio::sync::broadcast;

/// How many incoming messages one conversation keeps.
///
/// Eight is more than the expanded shade renders, so trimming never hides
/// something the platform would have displayed. The conversation's unseen count
/// is tracked separately and stays truthful past this bound.
const MESSAGES_KEPT: usize = 8;

/// Who could see the app when the event arrived.
///
/// Both inputs suppress the same way but for different reasons: muting is a
/// request to say nothing, while a visible app means the user is already
/// reading, so the message is not news.
#[derive(Debug, Clone, Copy)]
pub struct Audience {
    pub muted: bool,
    pub visible: bool,
}

impl Audience {
    fn can_notify(self) -> bool {
        !self.muted && !self.visible
    }
}

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
                // A lagged feed only means one notification is skipped: the
                // next event posts a snapshot of the whole state again, so the
                // thread catches up by itself.
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            };
            let audience = Audience {
                muted: crate::muted(),
                visible: android::user_can_see_app(),
            };
            let snapshot = THREADS
                .lock()
                .unwrap()
                .observe(&text, &identity, &store, audience);
            if let Some(snapshot) = snapshot {
                post(&snapshot);
            }
        }
    });
}

/// Forget every accumulated thread.
///
/// The caller is responsible for cancelling what is already in the shade: this
/// only stops the next snapshot from resurrecting it. Used when the user opens
/// the app and when the engine stops, so a restarting process never inherits
/// threads it cannot explain.
pub fn clear() {
    THREADS.lock().unwrap().clear();
}

fn post(snapshot: &Snapshot) {
    if !cfg!(target_os = "android") {
        return;
    }
    let payload = match serde_json::to_string(snapshot) {
        Ok(payload) => payload,
        Err(err) => {
            android::log_error(&format!(
                "could not serialize the notification snapshot: {err}"
            ));
            return;
        }
    };
    if let Err(err) = android::notify_threads(&payload) {
        android::log_error(&format!("could not post the message notification: {err}"));
    }
}

/// The accumulated threads, in the order the summary lists them: most recently
/// active conversation first.
static THREADS: Mutex<Threads> = Mutex::new(Threads::new());

#[derive(Debug, Default)]
pub struct Threads {
    conversations: Vec<Thread>,
}

impl Threads {
    pub const fn new() -> Self {
        Self {
            conversations: Vec::new(),
        }
    }

    /// Feed one engine event; returns the snapshot to post, if any.
    ///
    /// Nothing is appended when the user can see the app, so a message read in
    /// the app can never resurface in a later notification after it is
    /// backgrounded again.
    pub fn observe(
        &mut self,
        event: &str,
        identity: &str,
        store: &Store,
        audience: Audience,
    ) -> Option<Snapshot> {
        if !audience.can_notify() {
            return None;
        }
        let incoming = Incoming::parse(event, identity)?;
        let title = title_for(store, &incoming.conversation_id);
        let entry = MessageView {
            sender_id: incoming.author_id.clone(),
            sender_label: label_for(store, &incoming.author_id),
            body: incoming.body,
            ts: incoming.ts,
        };
        match self
            .conversations
            .iter_mut()
            .find(|thread| thread.id == incoming.conversation_id)
        {
            Some(thread) => thread.push(title, entry),
            None => self
                .conversations
                .push(Thread::new(incoming.conversation_id, title, entry)),
        }
        // Newest conversation first, which is the order the summary lists them
        // in. A stable sort keeps arrival order for equal timestamps.
        self.conversations
            .sort_by_key(|thread| std::cmp::Reverse(thread.latest_ts));
        Some(self.snapshot())
    }

    pub fn clear(&mut self) {
        self.conversations.clear();
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            conversations: self
                .conversations
                .iter()
                .map(Thread::view)
                .collect::<Vec<_>>(),
            conversation_count: self.conversations.len() as u32,
            message_count: self
                .conversations
                .iter()
                .map(|thread| thread.unseen)
                .sum::<u32>(),
        }
    }
}

#[derive(Debug)]
struct Thread {
    id: String,
    title: String,
    /// Every message that arrived while nobody was looking, not just the ones
    /// still listed in `messages`.
    unseen: u32,
    latest_ts: i64,
    /// Oldest first, at most [`MESSAGES_KEPT`] entries.
    messages: Vec<MessageView>,
}

impl Thread {
    fn new(id: String, title: String, entry: MessageView) -> Self {
        Self {
            id,
            title,
            unseen: 1,
            latest_ts: entry.ts,
            messages: vec![entry],
        }
    }

    fn push(&mut self, title: String, entry: MessageView) {
        self.title = title;
        self.unseen = self.unseen.saturating_add(1);
        self.latest_ts = self.latest_ts.max(entry.ts);
        self.messages.push(entry);
        // Arrival order is not send order: every message travels its own link,
        // so two of them can overtake each other on the mesh. Order by the
        // timestamp the sender put on it, which is what the conversation shows
        // too. The sort is stable, so equal timestamps keep arrival order.
        self.messages.sort_by_key(|message| message.ts);
        if self.messages.len() > MESSAGES_KEPT {
            self.messages.remove(0);
        }
    }

    fn view(&self) -> ThreadView {
        ThreadView {
            id: self.id.clone(),
            title: self.title.clone(),
            unseen: self.unseen,
            messages: self.messages.clone(),
        }
    }
}

/// One incoming message, as the engine event describes it.
struct Incoming {
    conversation_id: String,
    author_id: String,
    body: String,
    ts: i64,
}

impl Incoming {
    /// Parse `message.new` and reject everything else, including our own
    /// messages: the engine publishes both directions on the same event.
    fn parse(event: &str, identity: &str) -> Option<Self> {
        let value = serde_json::from_str::<serde_json::Value>(event).ok()?;
        if value.get("type")?.as_str()? != "event" {
            return None;
        }
        if value.get("name")?.as_str()? != "message.new" {
            return None;
        }
        let message = value.get("data")?.get("message")?;
        let author_id = message.get("author_id")?.as_str()?;
        if author_id == identity {
            return None;
        }
        Some(Self {
            conversation_id: message.get("conversation_id")?.as_str()?.to_string(),
            author_id: author_id.to_string(),
            body: message
                .get("body")
                .and_then(|body| body.as_str())
                .unwrap_or_default()
                .to_string(),
            ts: message.get("ts").and_then(|ts| ts.as_i64()).unwrap_or(0),
        })
    }
}

/// What Kotlin renders. One payload per event, so the two sides cannot disagree
/// about which thread a message belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub conversations: Vec<ThreadView>,
    pub conversation_count: u32,
    pub message_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadView {
    pub id: String,
    pub title: String,
    pub unseen: u32,
    /// Oldest first, so the platform's stack renders the newest at the bottom.
    pub messages: Vec<MessageView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageView {
    pub sender_id: String,
    pub sender_label: String,
    /// Stored whole: collapsed and expanded rendering is the platform's job.
    pub body: String,
    pub ts: i64,
}

/// The conversation's own title when it has one, otherwise the peer's label, so
/// the notification and the conversation list name a chat the same way.
fn title_for(store: &Store, conversation_id: &str) -> String {
    let Ok(conversation) = store.get_conversation(conversation_id) else {
        return short_id(conversation_id);
    };
    let title = conversation
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty());
    match title {
        Some(title) => title.to_string(),
        None => conversation
            .peer_ids
            .first()
            .map(|peer_id| label_for(store, peer_id))
            .unwrap_or_else(|| short_id(conversation_id)),
    }
}

/// Nickname, then announced name, then the short identity: the same rule the
/// conversation list uses, borrowed from `PeerInfo::label`.
fn label_for(store: &Store, peer_id: &str) -> String {
    match store.peer(peer_id) {
        Ok(Some(peer)) => peer.label(),
        _ => short_id(peer_id),
    }
}

fn short_id(id: &str) -> String {
    id[..8.min(id.len())].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchat_ipc::{Caps, PeerInfo};

    const IDENTITY: &str = "self";

    /// Nobody is watching and nothing is muted: the ordinary background case.
    fn heard() -> Audience {
        Audience {
            muted: false,
            visible: false,
        }
    }

    struct Case {
        store: Store,
        threads: Threads,
    }

    impl Case {
        fn new() -> Self {
            Self {
                store: Store::open_in_memory().unwrap(),
                threads: Threads::new(),
            }
        }

        fn peer(&self, id: &str, nickname: Option<&str>, announced: Option<&str>) {
            self.store
                .upsert_peer(&PeerInfo {
                    id: id.to_string(),
                    address: None,
                    nickname: nickname.map(str::to_string),
                    announced_name: announced.map(str::to_string),
                    last_seen: None,
                    bandwidth_class: "unknown".to_string(),
                    caps: Caps::default(),
                })
                .unwrap();
        }

        /// A direct conversation with one peer, the shape the UI can create.
        fn direct(&self, peer_id: &str) -> String {
            self.store.open_direct(peer_id).unwrap().id
        }

        fn observe(
            &mut self,
            conversation: &str,
            author: &str,
            body: &str,
            ts: i64,
        ) -> Option<Snapshot> {
            self.observe_as(conversation, author, body, ts, heard())
        }

        fn observe_as(
            &mut self,
            conversation: &str,
            author: &str,
            body: &str,
            ts: i64,
            audience: Audience,
        ) -> Option<Snapshot> {
            self.threads.observe(
                &event(conversation, author, body, ts),
                IDENTITY,
                &self.store,
                audience,
            )
        }
    }

    fn event(conversation: &str, author: &str, body: &str, ts: i64) -> String {
        serde_json::json!({
            "type": "event",
            "seq": 7,
            "name": "message.new",
            "data": { "message": {
                "id": format!("message-{ts}"),
                "conversation_id": conversation,
                "author_id": author,
                "ts": ts,
                "body": body,
                "kind": "text",
                "state": "delivered",
            }},
        })
        .to_string()
    }

    /// Bodies in the order they would be rendered.
    fn bodies(snapshot: &Snapshot, index: usize) -> Vec<String> {
        snapshot.conversations[index]
            .messages
            .iter()
            .map(|message| message.body.clone())
            .collect()
    }

    #[test]
    fn one_incoming_message_becomes_named_thread() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        let chat = case.direct("alice");

        let snapshot = case
            .observe(&chat, "alice", "are you free?", 1_000)
            .expect("a background message deserves a notification");

        assert_eq!(snapshot.conversation_count, 1);
        assert_eq!(snapshot.message_count, 1);
        let thread = &snapshot.conversations[0];
        assert_eq!(thread.title, "Alice");
        assert_eq!(thread.unseen, 1);
        assert_eq!(
            thread.messages,
            [MessageView {
                sender_id: "alice".to_string(),
                sender_label: "Alice".to_string(),
                body: "are you free?".to_string(),
                ts: 1_000,
            }]
        );
    }

    #[test]
    fn a_burst_keeps_send_order_with_the_newest_last() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        let chat = case.direct("alice");

        case.observe(&chat, "alice", "first", 1_000);
        case.observe(&chat, "alice", "second", 2_000);
        let snapshot = case.observe(&chat, "alice", "third", 3_000).unwrap();

        assert_eq!(bodies(&snapshot, 0), ["first", "second", "third"]);
        assert_eq!(snapshot.message_count, 3);
    }

    #[test]
    fn our_own_messages_are_never_notified() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        let chat = case.direct("alice");

        assert!(case.observe(&chat, IDENTITY, "hello", 1_000).is_none());
        assert_eq!(case.threads.snapshot().conversations, []);
    }

    #[test]
    fn irrelevant_events_are_ignored() {
        let mut case = Case::new();
        for event in [
            "not json",
            &serde_json::json!({ "type": "hello" }).to_string(),
            &serde_json::json!({
                "type": "event",
                "name": "message.state",
                "data": { "message": { "author_id": "alice" } },
            })
            .to_string(),
            &serde_json::json!({
                "type": "event",
                "name": "message.new",
                "data": { "message": { "author_id": "alice" } },
            })
            .to_string(),
        ] {
            assert!(case
                .threads
                .observe(event, IDENTITY, &case.store, heard())
                .is_none());
        }
        assert_eq!(case.threads.snapshot().conversations, []);
    }

    #[test]
    fn muting_suppresses_and_leaves_nothing_behind() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        let chat = case.direct("alice");
        let muted = Audience {
            muted: true,
            visible: false,
        };

        assert!(case
            .observe_as(&chat, "alice", "unmuted-yet?", 1_000, muted)
            .is_none());

        // The count starts at the unmute, not at the first arrival.
        let snapshot = case
            .observe(&chat, "alice", "now you see me", 2_000)
            .unwrap();
        assert_eq!(bodies(&snapshot, 0), ["now you see me"]);
        assert_eq!(snapshot.message_count, 1);
    }

    #[test]
    fn a_message_seen_in_the_app_never_surfaces_later() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        let chat = case.direct("alice");
        let visible = Audience {
            muted: false,
            visible: true,
        };

        assert!(case
            .observe_as(&chat, "alice", "already read", 1_000, visible)
            .is_none());

        let snapshot = case
            .observe(&chat, "alice", "arrived later", 2_000)
            .unwrap();
        assert_eq!(bodies(&snapshot, 0), ["arrived later"]);
        assert_eq!(snapshot.message_count, 1);
    }

    #[test]
    fn the_bound_drops_the_oldest_but_keeps_the_count_truthful() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        let chat = case.direct("alice");

        let mut snapshot = None;
        for index in 0..10 {
            snapshot = case.observe(&chat, "alice", &format!("message {index}"), index as i64);
        }
        let snapshot = snapshot.unwrap();

        assert_eq!(
            bodies(&snapshot, 0),
            [
                "message 2",
                "message 3",
                "message 4",
                "message 5",
                "message 6",
                "message 7",
                "message 8",
                "message 9",
            ]
        );
        assert_eq!(snapshot.conversations[0].unseen, 10);
        assert_eq!(snapshot.message_count, 10);
    }

    #[test]
    fn messages_overtaken_in_transit_are_ordered_by_timestamp() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        let chat = case.direct("alice");

        case.observe(&chat, "alice", "first", 1_000);
        // Delivered before the second one, but sent after it: each message
        // travels its own link, so arrival order is not send order.
        case.observe(&chat, "alice", "third", 3_000);
        let snapshot = case.observe(&chat, "alice", "second", 2_000).unwrap();

        assert_eq!(bodies(&snapshot, 0), ["first", "second", "third"]);
        assert_eq!(snapshot.message_count, 3);
    }

    #[test]
    fn ordering_survives_the_bound() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        let chat = case.direct("alice");

        // Ten sent in order, the last two delivered the wrong way round.
        for index in 0..8 {
            case.observe(&chat, "alice", &format!("message {index}"), index as i64);
        }
        case.observe(&chat, "alice", "message 9", 9);
        let snapshot = case.observe(&chat, "alice", "message 8", 8).unwrap();

        // The oldest by timestamp is the one trimmed, not the one that arrived
        // first.
        assert_eq!(
            bodies(&snapshot, 0),
            [
                "message 2",
                "message 3",
                "message 4",
                "message 5",
                "message 6",
                "message 7",
                "message 8",
                "message 9",
            ]
        );
    }

    #[test]
    fn a_second_conversation_gets_its_own_thread_newest_first() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        case.peer("bob", Some("Bob"), None);
        let alice = case.direct("alice");
        let bob = case.direct("bob");

        case.observe(&alice, "alice", "from alice", 1_000);
        case.observe(&bob, "bob", "from bob", 2_000);
        let snapshot = case.observe(&alice, "alice", "alice again", 3_000).unwrap();

        assert_eq!(snapshot.conversation_count, 2);
        assert_eq!(snapshot.message_count, 3);
        assert_eq!(snapshot.conversations[0].title, "Alice");
        assert_eq!(snapshot.conversations[1].title, "Bob");
        assert_eq!(bodies(&snapshot, 0), ["from alice", "alice again"]);
        assert_eq!(bodies(&snapshot, 1), ["from bob"]);
    }

    #[test]
    fn a_lagged_feed_leaves_the_next_snapshot_whole() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        case.peer("bob", Some("Bob"), None);
        let alice = case.direct("alice");
        let bob = case.direct("bob");

        case.observe(&alice, "alice", "from alice", 1_000);
        case.observe(&bob, "bob", "from bob", 2_000);
        // One event was dropped by the broadcast channel and never seen here.
        let snapshot = case
            .observe(&alice, "alice", "after the gap", 4_000)
            .unwrap();

        assert_eq!(snapshot.conversation_count, 2);
        assert_eq!(snapshot.message_count, 3);
        assert_eq!(bodies(&snapshot, 0), ["from alice", "after the gap"]);
        assert_eq!(bodies(&snapshot, 1), ["from bob"]);
    }

    #[test]
    fn clearing_forgets_every_thread() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        let chat = case.direct("alice");

        case.observe(&chat, "alice", "old news", 1_000);
        case.threads.clear();

        assert_eq!(case.threads.snapshot().conversations, []);
        let snapshot = case.observe(&chat, "alice", "fresh news", 2_000).unwrap();
        assert_eq!(bodies(&snapshot, 0), ["fresh news"]);
        assert_eq!(snapshot.message_count, 1);
    }

    #[test]
    fn a_group_uses_its_title_and_attributes_each_author() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        case.peer("bob", None, Some("Bob B."));
        let group = case
            .store
            .create_conversation(
                "group",
                Some("Weekend plans"),
                &["alice".to_string(), "bob".to_string()],
            )
            .unwrap()
            .id;

        case.observe(&group, "alice", "shall we?", 1_000);
        let snapshot = case.observe(&group, "bob", "yes", 2_000).unwrap();

        let thread = &snapshot.conversations[0];
        assert_eq!(thread.title, "Weekend plans");
        assert_eq!(
            thread
                .messages
                .iter()
                .map(|message| message.sender_label.as_str())
                .collect::<Vec<_>>(),
            ["Alice", "Bob B."]
        );
    }

    #[test]
    fn an_unnamed_peer_falls_back_to_the_short_identifier() {
        let mut case = Case::new();
        case.peer("0123456789abcdef", None, None);
        let chat = case.direct("0123456789abcdef");

        let snapshot = case
            .observe(&chat, "0123456789abcdef", "hello", 1_000)
            .unwrap();

        let thread = &snapshot.conversations[0];
        assert_eq!(thread.title, "01234567");
        assert_eq!(thread.messages[0].sender_label, "01234567");
    }

    #[test]
    fn an_unknown_peer_never_leaves_the_notification_unnamed() {
        let mut case = Case::new();
        // A member with no peer row yet, which a received message can produce.
        let chat = case
            .store
            .create_conversation("direct", None, &["fedcba9876543210".to_string()])
            .unwrap()
            .id;

        let snapshot = case
            .observe(&chat, "fedcba9876543210", "hello", 1_000)
            .unwrap();

        assert_eq!(snapshot.conversations[0].title, "fedcba98");
        assert_eq!(
            snapshot.conversations[0].messages[0].sender_label,
            "fedcba98"
        );
    }

    #[test]
    fn the_payload_names_its_fields_for_kotlin() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        let chat = case.direct("alice");
        let snapshot = case.observe(&chat, "alice", "hello", 1_000).unwrap();

        let value = serde_json::to_value(&snapshot).unwrap();

        assert_eq!(value["conversationCount"], 1);
        assert_eq!(value["messageCount"], 1);
        assert_eq!(value["conversations"][0]["unseen"], 1);
        assert_eq!(
            value["conversations"][0]["messages"][0]["senderLabel"],
            "Alice"
        );
        assert_eq!(
            value["conversations"][0]["messages"][0]["senderId"],
            "alice"
        );
    }
}
