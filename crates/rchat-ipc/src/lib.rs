//! Wire protocol shared between the rchat engine and any client (desktop UI,
//! future mobile client, test harness).
//!
//! Design notes that matter:
//! - Every server event carries a monotonic `seq` so a client can reconnect and
//!   replay what it missed instead of re-fetching everything.
//! - `ServerMsg` is tagged, JSON for now. A binary frame type is reserved for
//!   the future audio path; the transport must stay capable of carrying both.
//! - Capability flags are advertised from day one even though only text is
//!   implemented. Voice is a later consumer of the same negotiation.

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

/// What a peer claims to support. Text is the only thing implemented in v1;
/// the rest exist so the UI and the protocol do not need a breaking change
/// when voice notes and live voice arrive.
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
            attachments: false,
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
    /// LXMF delivery address once Reticulum is ready.
    pub address: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageInfo {
    pub id: String,
    pub conversation_id: String,
    pub author_id: String,
    pub ts: i64,
    pub body: String,
    /// "text" now; "voicenote" reserved.
    pub kind: String,
    pub reply_to: Option<String>,
    /// queued | sending | sent | delivered | failed
    pub state: String,
    pub route: Option<String>,
    pub edited_at: Option<i64>,
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
            ClientCmd::SendMessage { conversation_id, body, .. } => {
                assert_eq!(conversation_id, "c1");
                assert_eq!(body, "hi");
            }
            other => panic!("wrong variant: {other:?}"),
        }
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
