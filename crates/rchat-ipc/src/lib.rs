//! Wire protocol shared between the rchat engine and any client (desktop UI,
//! future mobile client, test harness).
//!
//! Design notes that matter:
//! - Every server event carries a monotonic `seq` so a client can reconnect and
//!   replay what it missed instead of re-fetching everything.
//! - `ServerMsg` is tagged, JSON for now. A binary frame type is reserved for
//!   the future audio path; the transport must stay capable of carrying both.
//! - Capability flags are advertised from day one. Voice is a later consumer of
//!   the same negotiation.
//! - Attachment bytes never travel on this channel. Messages carry descriptors
//!   and the bytes move over the loopback HTTP routes.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportState {
    /// No Reticulum bridge available. Local history still works.
    Offline,
    /// Bridge process started, not ready yet.
    Starting,
    /// Reticulum identity registered and announces are flowing.
    Ready,
}

impl TransportState {
    pub fn as_str(self) -> &'static str {
        match self {
            TransportState::Offline => "offline",
            TransportState::Starting => "starting",
            TransportState::Ready => "ready",
        }
    }
}

/// What a peer claims to support. Text and attachments are implemented; voice
/// notes and live voice arrive later behind the same flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caps {
    pub text: bool,
    pub attachments: bool,
    pub voicenote: bool,
    pub voice_live: bool,
}

impl Default for Caps {
    fn default() -> Self {
        Self {
            text: true,
            attachments: true,
            voicenote: false,
            voice_live: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityInfo {
    /// Stable local id (hex). Not the Reticulum address.
    pub id: String,
    pub display_name: String,
    /// True once the user has chosen a name. The desktop shell uses this to
    /// show a first-run prompt instead of the placeholder name.
    #[serde(default)]
    pub display_name_set: bool,
    /// LXMF delivery address once Reticulum is ready.
    pub address: Option<String>,
    /// What this client supports. Peers cannot learn it over LXMF as used here,
    /// so the UI never gates on it; it is advertised for other transports and
    /// for clients that ask.
    #[serde(default)]
    pub caps: Caps,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub id: String,
    pub address: Option<String>,
    pub nickname: Option<String>,
    pub announced_name: Option<String>,
    pub last_seen: Option<i64>,
    /// Coarse link quality, used by the UI badge and later by media routing.
    /// One of: lan, relay, radio, unknown.
    pub bandwidth_class: String,
    pub caps: Caps,
}

impl PeerInfo {
    pub fn label(&self) -> String {
        self.nickname
            .clone()
            .or_else(|| self.announced_name.clone())
            .unwrap_or_else(|| self.id[..8.min(self.id.len())].to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationInfo {
    pub id: String,
    /// "direct" or "group".
    pub kind: String,
    pub title: Option<String>,
    pub created_at: i64,
    pub last_message_at: Option<i64>,
    pub unread: i64,
    pub preview: Option<String>,
    pub peer_ids: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AttachmentInfo {
    /// The name the sender chose. Display only: never a path component.
    pub name: String,
    /// Sniffed MIME type, e.g. `image/jpeg` or `application/pdf`.
    pub mime: String,
    /// Bytes of the original, whether or not they are still on this device.
    pub size: u64,
    /// SHA-256 of the original, lowercase hex. Empty when the bytes never
    /// arrived, which is why `reason` is then set.
    pub hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// SHA-256 of the display thumbnail, when the attachment is an image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
    /// Present when the bytes are not on this device, and why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl AttachmentInfo {
    pub fn is_image(&self) -> bool {
        self.mime.starts_with("image/")
    }

    /// Whether the bytes can be fetched. A row without bytes still carries
    /// metadata and a reason.
    pub fn has_bytes(&self) -> bool {
        self.reason.is_none() && !self.hash.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageInfo {
    pub id: String,
    pub conversation_id: String,
    pub author_id: String,
    pub ts: i64,
    /// The caption, when there are attachments.
    pub body: String,
    /// "text", "image" or "file".
    pub kind: String,
    pub reply_to: Option<String>,
    /// queued | sending | sent | delivered | failed
    pub state: String,
    pub route: Option<String>,
    pub edited_at: Option<i64>,
    /// Ordered as the sender chose. Empty for text-only messages, and omitted
    /// on the wire so older clients see exactly what they used to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<AttachmentInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub identity: IdentityInfo,
    pub transport: TransportState,
    pub conversations: Vec<ConversationInfo>,
    pub peers: Vec<PeerInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Hello {
        protocol: u32,
        version: String,
        identity: IdentityInfo,
        transport: TransportState,
    },
    Reply {
        id: u64,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    Event {
        seq: i64,
        name: String,
        data: serde_json::Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    pub id: u64,
    #[serde(flatten)]
    pub cmd: ClientCmd,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum ClientCmd {
    Snapshot,
    ListMessages {
        conversation_id: String,
        before: Option<i64>,
        limit: Option<u32>,
    },
    SendMessage {
        conversation_id: String,
        body: String,
        reply_to: Option<String>,
        /// Content hashes from a prior upload, in display order. An old client
        /// that omits the field sends text, exactly as before.
        #[serde(default)]
        attachments: Vec<String>,
    },
    CreateConversation {
        kind: String,
        title: Option<String>,
        peer_ids: Vec<String>,
    },
    /// Open (or create) the direct conversation with a known peer.
    OpenDirect {
        peer_id: String,
    },
    /// Register a peer by LXMF address, optionally with a local nickname.
    AddPeer {
        address: String,
        nickname: Option<String>,
    },
    SetDisplayName {
        name: String,
    },
    MarkRead {
        conversation_id: String,
    },
    Search {
        query: String,
        limit: Option<u32>,
    },
    EventsSince {
        seq: i64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_roundtrip_with_flattened_tag() {
        let raw = r#"{"id":7,"cmd":"send_message","conversation_id":"c1","body":"hi","reply_to":null}"#;
        let cmd: Command = serde_json::from_str(raw).expect("parse");
        assert_eq!(cmd.id, 7);
        match cmd.cmd {
            ClientCmd::SendMessage { conversation_id, body, attachments, .. } => {
                assert_eq!(conversation_id, "c1");
                assert_eq!(body, "hi");
                assert!(attachments.is_empty(), "an old client sends no hashes");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn send_message_carries_hashes_in_order() {
        let raw = r#"{"id":8,"cmd":"send_message","conversation_id":"c1","body":"",
                      "reply_to":null,"attachments":["aa","bb"]}"#;
        let cmd: Command = serde_json::from_str(raw).unwrap();
        match cmd.cmd {
            ClientCmd::SendMessage { attachments, .. } => {
                assert_eq!(attachments, ["aa", "bb"]);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn attachment_descriptors_round_trip_and_absent_bytes_are_explained() {
        let attachment = AttachmentInfo {
            name: "photo.jpg".into(),
            mime: "image/jpeg".into(),
            size: 2048,
            hash: "ab".repeat(32),
            width: Some(1200),
            height: Some(800),
            thumbnail: Some("cd".repeat(32)),
            reason: None,
        };
        let text = serde_json::to_string(&attachment).unwrap();
        let back: AttachmentInfo = serde_json::from_str(&text).unwrap();
        assert_eq!(back, attachment);
        assert!(back.is_image());
        assert!(back.has_bytes());

        let missing = AttachmentInfo {
            hash: String::new(),
            reason: Some("Attachment removed to save space".into()),
            ..attachment
        };
        let value = serde_json::to_value(&missing).unwrap();
        assert_eq!(value["reason"], "Attachment removed to save space");
        assert!(!missing.has_bytes());
    }

    #[test]
    fn text_only_messages_omit_attachments_on_the_wire() {
        let msg = MessageInfo {
            id: "m1".into(),
            conversation_id: "c1".into(),
            author_id: "a".into(),
            ts: 1,
            body: "hello".into(),
            kind: "text".into(),
            reply_to: None,
            state: "sent".into(),
            route: None,
            edited_at: None,
            attachments: Vec::new(),
        };
        let value = serde_json::to_value(&msg).unwrap();
        assert!(
            value.get("attachments").is_none(),
            "an older client must see the exact shape it always did"
        );
        // And an older message still parses into the new type.
        let back: MessageInfo = serde_json::from_value(value).unwrap();
        assert!(back.attachments.is_empty());
    }

    #[test]
    fn identity_advertises_attachment_support() {
        let identity = IdentityInfo {
            id: "i".into(),
            display_name: "you".into(),
            display_name_set: true,
            address: None,
            caps: Caps::default(),
        };
        assert!(identity.caps.attachments);
        let raw = r#"{"id":"i","display_name":"you","display_name_set":true,"address":null}"#;
        let back: IdentityInfo = serde_json::from_str(raw).unwrap();
        assert!(back.caps.attachments, "the field defaults to supported");
    }

    #[test]
    fn event_carries_seq() {
        let msg = ServerMsg::Event {
            seq: 3,
            name: "message.new".into(),
            data: serde_json::json!({"id": "m1"}),
        };
        let text = serde_json::to_string(&msg).unwrap();
        assert!(text.contains(r#""type":"event""#));
        assert!(text.contains(r#""seq":3"#));
    }
}
