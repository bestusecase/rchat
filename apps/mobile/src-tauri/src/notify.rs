//! Background message notifications.
//!
//! This cannot live in the webview: when the activity goes to the background
//! the WebView suspends its JavaScript, so the page never sees the engine event
//! that says a message arrived. The engine keeps running inside the foreground
//! service, so this module watches its event stream directly and posts through
//! JNI.
//!
//! What is posted is a conversation thread per conversation, not a bare count.
//! [`Threads`] is the whole state machine: an engine event, whether output is
//! muted, and a way to ask who can see the app go in, and the snapshot Kotlin
//! should render comes out. Keeping the decisions here, away from the platform,
//! is what makes them testable without a device.

use crate::android;
use crate::Running;
use rchat_core::store::Store;
use serde::Serialize;
use std::sync::Mutex;
use tokio::sync::broadcast;

/// How many incoming messages one conversation keeps.
///
/// Eight caps shade bloat and process memory; it is not a claim about what the
/// expanded shade can render, so trimming can hide text the platform could have
/// displayed. The conversation's unread count is tracked separately and stays
/// truthful past this bound.
const MESSAGES_KEPT: usize = 8;

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
            // Muting is sampled now, when the event arrives; visibility is
            // asked later, when the append would commit and again at
            // delivery. One value carries both so the asymmetry is visible
            // where it is passed.
            let audience = Audience::new(crate::muted(), android::user_can_see_app);
            deliver(&text, &identity, &store, &audience, post);
        }
    });
}

/// Own one engine event from arrival to shade.
///
/// Locks the accumulated threads across both the commit and the post, so the
/// foreground clear cannot interleave between them. The decision itself is
/// [`Threads::deliver`], which the tests drive directly on an isolated
/// accumulator.
fn deliver(
    event: &str,
    identity: &str,
    store: &Store,
    audience: &Audience<impl Fn() -> bool>,
    post: impl FnOnce(&Snapshot),
) -> bool {
    THREADS
        .lock()
        .unwrap()
        .deliver(event, identity, store, audience, post)
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

/// Who may be told about a message.
///
/// The two inputs are sampled at different moments on purpose: `muted` is read
/// when the event arrives, so a muted app accumulates nothing, while `visible`
/// is asked only for an event that could actually be posted, and only when the
/// append would commit (and again at delivery). Bundling them keeps that
/// asymmetry visible at every call site.
pub struct Audience<F> {
    muted: bool,
    visible: F,
}

impl<F: Fn() -> bool> Audience<F> {
    pub fn new(muted: bool, visible: F) -> Self {
        Self { muted, visible }
    }

    fn is_muted(&self) -> bool {
        self.muted
    }

    fn is_visible(&self) -> bool {
        (self.visible)()
    }
}

impl Threads {
    pub const fn new() -> Self {
        Self {
            conversations: Vec::new(),
        }
    }

    /// Feed one engine event; returns the snapshot to post, if any.
    ///
    /// The audience is asked here, while the lock is held, immediately before
    /// the append is committed. The foreground path flips its flag before it
    /// takes that same lock to clear, so the two can only interleave two safe
    /// ways: if the clear wins, this sample sees a visible app and declines; if
    /// the append wins, the clear that follows discards it. A message read in
    /// the app can therefore never resurface in a later notification.
    ///
    /// The question is deliberately asked after muting and parsing: an event
    /// that can never be posted is decided without consulting the platform, so
    /// the timing of this sample is part of the behaviour, not an accident.
    pub fn observe(
        &mut self,
        event: &str,
        identity: &str,
        store: &Store,
        audience: &Audience<impl Fn() -> bool>,
    ) -> Option<Snapshot> {
        if audience.is_muted() {
            return None;
        }
        let incoming = Incoming::parse(event, identity)?;
        // Commit-time sample: everything above ran while the app was away, but
        // the user may have come back in the meantime.
        if audience.is_visible() {
            return None;
        }
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

    /// Own one engine event from arrival to shade.
    ///
    /// Callers hold the threads lock across this call and the poster; the
    /// module-level [`deliver`] does that for the event loop, and the tests
    /// drive this method on an isolated accumulator.
    ///
    /// The accumulator decides whether anything accumulates; if it does, the
    /// visibility question is asked once more, because a resume can land
    /// between the commit and the post. A refused snapshot is forgotten — the
    /// user is looking at the messages now, so they must not reappear in the
    /// next background notification — and `post` is handed the snapshot the
    /// shade would show. Returns whether anything was posted.
    ///
    /// Asking twice is deliberate: the commit-time sample stops a message being
    /// accumulated, and this delivery-time sample is the backstop for what was
    /// accumulated a moment too early. Neither replaces the other.
    pub fn deliver(
        &mut self,
        event: &str,
        identity: &str,
        store: &Store,
        audience: &Audience<impl Fn() -> bool>,
        post: impl FnOnce(&Snapshot),
    ) -> bool {
        let Some(snapshot) = self.observe(event, identity, store, audience) else {
            return false;
        };
        // Delivery-time sample: a resume after the commit still makes this
        // snapshot stale, and refusing without forgetting is what let text
        // already read surface in a later notification.
        if audience.is_visible() {
            self.clear();
            return false;
        }
        post(&snapshot);
        true
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
            unread_count: self
                .conversations
                .iter()
                .map(|thread| thread.unread)
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
    unread: u32,
    latest_ts: i64,
    /// Oldest first, at most [`MESSAGES_KEPT`] entries.
    messages: Vec<MessageView>,
}

impl Thread {
    fn new(id: String, title: String, entry: MessageView) -> Self {
        Self {
            id,
            title,
            unread: 1,
            latest_ts: entry.ts,
            messages: vec![entry],
        }
    }

    fn push(&mut self, title: String, entry: MessageView) {
        self.title = title;
        self.unread = self.unread.saturating_add(1);
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
            unread: self.unread,
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
            body: line_body(message),
            // A missing clock would sort the message to the head of its thread
            // and get it trimmed first while still counted, so treat it like
            // any other malformed event and ignore it.
            ts: message.get("ts")?.as_i64()?,
        })
    }
}

/// The line a notification thread shows for a message.
///
/// A caption stays the line; an attachment-only message would otherwise be
/// blank, so it reads as its kind — or its filename when there is exactly one
/// file. The wording matches the conversation-list preview the engine derives,
/// so the shade and the list never disagree.
fn line_body(message: &serde_json::Value) -> String {
    let body = message
        .get("body")
        .and_then(|body| body.as_str())
        .unwrap_or_default()
        .to_string();
    if !body.trim().is_empty() {
        return body;
    }
    attachment_line(message).unwrap_or(body)
}

/// One line for a notification thread, taken from the engine's own preview rule
/// so the shade and the conversation list cannot drift apart.
fn attachment_line(message: &serde_json::Value) -> Option<String> {
    let items: Vec<rchat_ipc::AttachmentInfo> = message
        .get("attachments")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    rchat_core::store::preview_text(&items)
}

/// What Kotlin renders. One payload per event, so the two sides cannot disagree
/// about which thread a message belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub conversations: Vec<ThreadView>,
    pub conversation_count: u32,
    /// The sum of every conversation's unread count, which is what the summary
    /// carries as its number and the launcher badge reads.
    pub unread_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadView {
    pub id: String,
    pub title: String,
    /// Every message that arrived while nobody was looking, including ones the
    /// bound has already trimmed from `messages`.
    pub unread: u32,
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
    use std::cell::Cell;

    const IDENTITY: &str = "self";

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
                    notes: None,
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
            self.observe_while(conversation, author, body, ts, || false)
        }

        /// Observe one message while the caller decides, at any moment it
        /// likes, whether the user can see the app.
        fn observe_while(
            &mut self,
            conversation: &str,
            author: &str,
            body: &str,
            ts: i64,
            visible: impl Fn() -> bool,
        ) -> Option<Snapshot> {
            self.observe_event(&event(conversation, author, body, ts), false, visible)
        }

        /// Observe a raw event, with the suppression inputs named.
        fn observe_event(
            &mut self,
            event: &str,
            muted: bool,
            visible: impl Fn() -> bool,
        ) -> Option<Snapshot> {
            self.threads
                .observe(event, IDENTITY, &self.store, &Audience::new(muted, visible))
        }

        /// Deliver one message through the higher seam, which decides whether
        /// the snapshot reaches the shade.
        fn deliver(
            &mut self,
            conversation: &str,
            author: &str,
            body: &str,
            ts: i64,
            visible: impl Fn() -> bool,
            post: impl FnOnce(&Snapshot),
        ) -> bool {
            self.threads.deliver(
                &event(conversation, author, body, ts),
                IDENTITY,
                &self.store,
                &Audience::new(false, visible),
                post,
            )
        }

        fn observe_muted(
            &mut self,
            conversation: &str,
            author: &str,
            body: &str,
            ts: i64,
        ) -> Option<Snapshot> {
            self.threads.observe(
                &event(conversation, author, body, ts),
                IDENTITY,
                &self.store,
                &Audience::new(true, || false),
            )
        }
    }

    fn event(conversation: &str, author: &str, body: &str, ts: i64) -> String {
        event_with(conversation, author, body, ts, serde_json::json!([]))
    }

    /// An event carrying attachment descriptors, as the engine publishes them.
    fn event_with(
        conversation: &str,
        author: &str,
        body: &str,
        ts: i64,
        attachments: serde_json::Value,
    ) -> String {
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
                "attachments": attachments,
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
        assert_eq!(snapshot.unread_count, 1);
        let thread = &snapshot.conversations[0];
        assert_eq!(thread.title, "Alice");
        assert_eq!(thread.unread, 1);
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
    fn an_attachment_only_message_reads_as_its_kind() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        let chat = case.direct("alice");

        let photo = serde_json::json!([{
            "name": "shot.jpg",
            "mime": "image/jpeg",
            "size": 2048,
            "hash": "aa",
        }]);
        let snapshot = case
            .observe_event(&event_with(&chat, "alice", "", 1_000, photo), false, || false)
            .unwrap();
        assert_eq!(bodies(&snapshot, 0), ["Photo"]);

        let pdf = serde_json::json!([{
            "name": "statement.pdf",
            "mime": "application/pdf",
            "size": 4096,
            "hash": "bb",
        }]);
        let snapshot = case
            .observe_event(&event_with(&chat, "alice", "", 2_000, pdf), false, || false)
            .unwrap();
        // Exactly one file names that file; the same rule as the list preview.
        assert_eq!(bodies(&snapshot, 0), ["Photo", "statement.pdf"]);
    }

    #[test]
    fn a_caption_stays_the_notification_line() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        let chat = case.direct("alice");

        let photo = serde_json::json!([{
            "name": "shot.jpg",
            "mime": "image/jpeg",
            "size": 2048,
            "hash": "aa",
        }]);
        let snapshot = case
            .observe_event(
                &event_with(&chat, "alice", "look at this", 1_000, photo),
                false,
                || false,
            )
            .unwrap();
        assert_eq!(bodies(&snapshot, 0), ["look at this"]);
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
        assert_eq!(snapshot.unread_count, 3);
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
            assert!(case.observe_event(event, false, || false).is_none());
        }
        assert_eq!(case.threads.snapshot().conversations, []);
    }

    /// A `message.new` event whose clock field is written as `ts`.
    ///
    /// `None` omits the field entirely, so every malformed-clock case shares
    /// one builder and changing the event shape stays one edit.
    fn event_with_ts(conversation: &str, ts: Option<serde_json::Value>) -> String {
        let mut message = serde_json::json!({
            "conversation_id": conversation,
            "author_id": "alice",
            "body": "no clock",
        });
        if let Some(ts) = ts {
            message["ts"] = ts;
        }
        serde_json::json!({
            "type": "event",
            "name": "message.new",
            "data": { "message": message },
        })
        .to_string()
    }

    #[test]
    fn an_event_without_an_integer_timestamp_is_ignored() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        let chat = case.direct("alice");

        for ts in [
            None,
            Some(serde_json::json!("1000")),
            Some(serde_json::json!(1000.5)),
            Some(serde_json::json!(null)),
        ] {
            assert!(case
                .observe_event(&event_with_ts(&chat, ts), false, || false)
                .is_none());
        }
        assert_eq!(case.threads.snapshot().conversations, []);
    }

    #[test]
    fn muting_suppresses_and_leaves_nothing_behind() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        let chat = case.direct("alice");

        assert!(case
            .observe_muted(&chat, "alice", "unmuted-yet?", 1_000)
            .is_none());

        // The count starts at the unmute, not at the first arrival.
        let snapshot = case
            .observe(&chat, "alice", "now you see me", 2_000)
            .unwrap();
        assert_eq!(bodies(&snapshot, 0), ["now you see me"]);
        assert_eq!(snapshot.unread_count, 1);
    }

    #[test]
    fn events_that_cannot_be_posted_never_ask_whether_the_user_is_looking() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        let chat = case.direct("alice");
        let asked = Cell::new(0);
        let counting = || {
            asked.set(asked.get() + 1);
            false
        };

        // Muted: the first gate, so nothing is accumulated before the
        // visibility question exists.
        assert!(case
            .observe_event(&event(&chat, "alice", "muted", 1_000), true, counting)
            .is_none());
        // Malformed, unrelated and the app's own message can never be posted
        // either, so none of them may consult the platform.
        assert!(case.observe_event("not json", false, counting).is_none());
        assert!(case
            .observe_event(
                &serde_json::json!({ "type": "hello" }).to_string(),
                false,
                counting,
            )
            .is_none());
        assert!(case
            .observe_event(&event(&chat, IDENTITY, "mine", 1_000), false, counting)
            .is_none());

        assert_eq!(asked.get(), 0, "an unpostable event must not ask");
        assert_eq!(case.threads.snapshot().conversations, []);
    }

    #[test]
    fn a_committable_message_asks_the_visibility_question_once_at_commit() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        let chat = case.direct("alice");
        let asked = Cell::new(0);

        let snapshot = case
            .observe_while(&chat, "alice", "only once", 1_000, || {
                asked.set(asked.get() + 1);
                false
            })
            .unwrap();

        assert_eq!(asked.get(), 1);
        assert_eq!(bodies(&snapshot, 0), ["only once"]);
    }

    #[test]
    fn a_visible_answer_at_commit_asks_once_and_accumulates_nothing() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        let chat = case.direct("alice");
        let asked = Cell::new(0);

        let snapshot = case.observe_while(&chat, "alice", "already read", 1_000, || {
            asked.set(asked.get() + 1);
            true
        });

        assert!(snapshot.is_none());
        assert_eq!(asked.get(), 1);
        assert_eq!(case.threads.snapshot().conversations, []);
    }

    #[test]
    fn a_message_seen_in_the_app_never_surfaces_later() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        let chat = case.direct("alice");

        assert!(case
            .observe_while(&chat, "alice", "already read", 1_000, || true)
            .is_none());

        let snapshot = case
            .observe(&chat, "alice", "arrived later", 2_000)
            .unwrap();
        assert_eq!(bodies(&snapshot, 0), ["arrived later"]);
        assert_eq!(snapshot.unread_count, 1);
    }

    #[test]
    fn a_resume_before_the_append_commits_leaves_nothing_accumulated() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        let chat = case.direct("alice");
        let visible = Cell::new(false);

        // The event is read while nobody is looking, then the user taps the
        // icon before the append is committed. The provider is asked at commit
        // time, so it sees the resume and the message is not accumulated.
        let snapshot = case.observe_while(&chat, "alice", "already read", 1_000, || {
            visible.set(true);
            visible.get()
        });

        assert!(snapshot.is_none());
        assert_eq!(case.threads.snapshot().conversations, []);
    }

    #[test]
    fn a_resume_after_the_append_discards_what_was_accumulated() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        let chat = case.direct("alice");

        // The append wins the lock first: the message is accumulated and a
        // notification is posted.
        let snapshot = case
            .observe_while(&chat, "alice", "read it", 1_000, || false)
            .unwrap();
        assert_eq!(bodies(&snapshot, 0), ["read it"]);

        // The resume follows, flipping its flag before the foreground clear
        // takes the lock. The clear discards what the append committed.
        case.threads.clear();
        assert_eq!(case.threads.snapshot().conversations, []);

        // Once the phone is pocketed again, only what arrived after the resume
        // can surface.
        let snapshot = case
            .observe_while(&chat, "alice", "arrived later", 2_000, || false)
            .unwrap();
        assert_eq!(bodies(&snapshot, 0), ["arrived later"]);
    }

    #[test]
    fn a_resume_between_commit_and_post_forgets_the_snapshot() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        let chat = case.direct("alice");
        let asked = Cell::new(0);
        let mut posted = Vec::new();

        // The append commits while nobody is looking, then the user taps the
        // icon before the post. The delivery-time answer is "visible", so the
        // snapshot is refused and what was committed is forgotten.
        let delivered = case.deliver(
            &chat,
            "alice",
            "already read",
            1_000,
            || {
                asked.set(asked.get() + 1);
                asked.get() > 1
            },
            |snapshot| posted.push(snapshot.clone()),
        );

        assert!(!delivered);
        assert!(posted.is_empty());
        assert_eq!(asked.get(), 2, "commit and delivery each ask once");
        assert_eq!(case.threads.snapshot().conversations, []);

        // The forget must not touch anything that arrived after the resume:
        // the next background message stands alone.
        let snapshot = case
            .observe(&chat, "alice", "arrived later", 2_000)
            .unwrap();
        assert_eq!(bodies(&snapshot, 0), ["arrived later"]);
        assert_eq!(snapshot.unread_count, 1);
    }

    #[test]
    fn a_delivered_snapshot_keeps_what_came_before() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        let chat = case.direct("alice");
        let mut posted = Vec::new();

        assert!(case.deliver(
            &chat,
            "alice",
            "first",
            1_000,
            || false,
            |snapshot| posted.push(snapshot.clone()),
        ));
        assert!(case.deliver(
            &chat,
            "alice",
            "second",
            2_000,
            || false,
            |snapshot| posted.push(snapshot.clone()),
        ));

        assert_eq!(posted.len(), 2);
        assert_eq!(bodies(&posted[0], 0), ["first"]);
        assert_eq!(bodies(&posted[1], 0), ["first", "second"]);
        assert_eq!(posted[1].unread_count, 2);
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
        assert_eq!(snapshot.conversations[0].unread, 10);
        assert_eq!(snapshot.unread_count, 10);
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
        assert_eq!(snapshot.unread_count, 3);
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
    fn equal_timestamps_keep_arrival_order_through_trimming() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        let chat = case.direct("alice");

        // Ten messages sent within the same millisecond, so only arrival order
        // distinguishes them. Trimming must drop the earliest arrivals and
        // leave the rest in that order.
        let mut snapshot = None;
        for index in 0..10 {
            snapshot = case.observe(&chat, "alice", &format!("message {index}"), 5_000);
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
        assert_eq!(snapshot.conversations[0].unread, 10);
    }

    #[test]
    fn the_total_unread_is_the_sum_of_each_conversation() {
        let mut case = Case::new();
        case.peer("alice", Some("Alice"), None);
        case.peer("bob", Some("Bob"), None);
        let alice = case.direct("alice");
        let bob = case.direct("bob");

        case.observe(&alice, "alice", "one", 1_000);
        case.observe(&alice, "alice", "two", 2_000);
        case.observe(&bob, "bob", "one", 3_000);
        case.observe(&bob, "bob", "two", 4_000);
        let snapshot = case.observe(&bob, "bob", "three", 5_000).unwrap();

        // Newest conversation first: Bob, then Alice.
        assert_eq!(snapshot.conversations[0].title, "Bob");
        assert_eq!(snapshot.conversations[0].unread, 3);
        assert_eq!(snapshot.conversations[1].title, "Alice");
        assert_eq!(snapshot.conversations[1].unread, 2);
        assert_eq!(snapshot.unread_count, 5);
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
        assert_eq!(snapshot.unread_count, 3);
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
        assert_eq!(snapshot.unread_count, 3);
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
        assert_eq!(snapshot.unread_count, 1);
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
        assert_eq!(value["unreadCount"], 1);
        assert_eq!(value["conversations"][0]["unread"], 1);
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
