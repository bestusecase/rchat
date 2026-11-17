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
    /// The bridge is up and at least one interface is online.
    Ready,
    /// The bridge is up but no interface is online, so nothing can move.
    /// Additive on the wire: a client that does not know the word shows it
    /// verbatim instead of breaking.
    Degraded,
}

impl TransportState {
    pub fn as_str(self) -> &'static str {
        match self {
            TransportState::Offline => "offline",
            TransportState::Starting => "starting",
            TransportState::Ready => "ready",
            TransportState::Degraded => "degraded",
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
    /// What the user wrote about this peer. Local only: never sent over the
    /// mesh, never overwritten by an announce, and not part of message search.
    /// Defaulted so payloads from an older engine still parse.
    #[serde(default)]
    pub notes: Option<String>,
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

// ---- mesh observability ---------------------------------------------------

/// Radio telemetry an RNode (or similar) reports through
/// `get_interface_stats`. Every field is optional: most interfaces have none.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MeshRadioStats {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rssi: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snr: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bandwidth: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sf: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cr: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub txpower: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub airtime_short: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub airtime_long: Option<f64>,
}

/// One live interface, whitelisted out of the bridge's raw stats.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeshInterface {
    pub name: String,
    /// RNS interface type, e.g. `TCPClientInterface`.
    pub kind: String,
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub online: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtu: Option<u32>,
    #[serde(default)]
    pub rxb: u64,
    #[serde(default)]
    pub txb: u64,
    #[serde(default)]
    pub rxs: f64,
    #[serde(default)]
    pub txs: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peers: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ifac: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub radio: Option<MeshRadioStats>,
    /// When the current outage began, as epoch milliseconds. Set only while
    /// the interface is offline, so "down for two minutes" is visible instead
    /// of an interface that looks merely idle. Additive on the wire, defaulted
    /// so a bridge that does not sample it still parses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub down_since: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MeshCounters {
    #[serde(default)]
    pub rx_packets: u64,
    #[serde(default)]
    pub tx_packets: u64,
    #[serde(default)]
    pub rx_pps: f64,
    #[serde(default)]
    pub tx_pps: f64,
    #[serde(default)]
    pub announce_rx: u64,
    #[serde(default)]
    pub announce_tx: u64,
    #[serde(default)]
    pub links: u64,
    #[serde(default)]
    pub path_count: u64,
    #[serde(default)]
    pub path_unresponsive: u64,
}

/// A known chat peer's current path. Small on purpose: the full path table is
/// served by [`ClientCmd::MeshPaths`], not repeated in every status push.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeerPath {
    pub address: String,
    pub hops: u32,
    pub next_hop: String,
    pub interface: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<u64>,
    pub state: String,
    pub age: f64,
}

/// Whether a known peer can be reached right now, and how. Derived from the
/// Reticulum path table and the announce history: never from receipts, so a
/// live path with an old announce is still reachable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeerReachability {
    pub address: String,
    /// Whether the path table currently holds a route to this destination.
    pub path: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hops: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_hop: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<u64>,
    /// responsive | unresponsive | none
    pub state: String,
    /// Epoch milliseconds of the last announce heard from this peer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_announce: Option<i64>,
}

/// One row of the full Reticulum path table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathEntry {
    pub destination: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub hops: u32,
    pub next_hop: String,
    pub interface: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<u64>,
    pub state: String,
    pub age: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aspect: Option<String>,
}

/// A non-LXMF destination recently heard on the mesh. In memory only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceEntry {
    pub destination: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aspect: Option<String>,
    /// Epoch milliseconds, like every other timestamp except `started_at`.
    pub last_seen: i64,
}

/// The live mesh picture the engine caches. Never persisted.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MeshStatus {
    /// offline | starting | ready | degraded
    pub transport: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(default)]
    pub interfaces: Vec<MeshInterface>,
    #[serde(default)]
    pub counters: MeshCounters,
    #[serde(default)]
    pub peer_paths: Vec<PeerPath>,
    /// One row per known peer, whether or not a path exists. Omitted when
    /// empty so an older client sees the shape it already knew.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peer_reachability: Vec<PeerReachability>,
    #[serde(default)]
    pub other_services: Vec<ServiceEntry>,
}

/// One coalesced announce sighting from the durable history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnnounceSighting {
    pub address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aspect: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hops: Option<u32>,
    pub bucket: i64,
    pub first_seen: i64,
    pub last_seen: i64,
    pub count: i64,
}

// ---- interface management -------------------------------------------------

/// An interface row rchat owns. `config` is opaque to the engine; only the
/// bridge validates it against Reticulum.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InterfaceRow {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub config: serde_json::Value,
    pub enabled: bool,
    /// managed | imported | legacy | default
    pub origin: String,
    /// Why the row cannot be used as written, when rchat imported something it
    /// could not understand. Shown to the user so nothing is dropped silently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One derived fact the Network page should tell the user. Currently only the
/// upgrade case: a hand-written `rns/config` won, so old app settings were not
/// imported.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InterfaceNotice {
    /// Stable identifier, so a future client can style or speak about the
    /// notice without parsing the message.
    pub kind: String,
    /// Human text naming only the settings that were actually skipped.
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShowIf {
    pub key: String,
    pub equals: serde_json::Value,
}

/// One form control in the descriptor-driven interface form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InterfaceField {
    pub key: String,
    pub label: String,
    /// text | mono | secret | port | int | bool | select | serial
    pub widget: String,
    #[serde(default)]
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// Set for fields on the main form; others live under Advanced.
    #[serde(default)]
    pub common: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_if: Option<ShowIf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choices: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub units: Option<String>,
}

/// The catalogue entry the bridge builds next to RNS.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InterfaceType {
    pub kind: String,
    pub label: String,
    #[serde(default)]
    pub summary: String,
    /// Local | TCP | Radio | Other, used to group the type picker.
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub fields: Vec<InterfaceField>,
    /// Set when RNS refuses to manage this type: the reason to show instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub identity: IdentityInfo,
    pub transport: TransportState,
    pub conversations: Vec<ConversationInfo>,
    pub peers: Vec<PeerInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<MeshStatus>,
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
    /// Set the private alias for a peer. `None` clears it, so the headline
    /// follows the peer's announced name again.
    SetPeerNickname {
        peer_id: String,
        nickname: Option<String>,
    },
    /// Replace the private note kept for a peer. `None` clears it.
    SetPeerNotes {
        peer_id: String,
        notes: Option<String>,
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
    /// Current mesh state, assembled by the engine from the bridge cache.
    MeshStatus,
    /// Page through the cached Reticulum path table.
    MeshPaths {
        query: Option<String>,
        offset: Option<u32>,
        limit: Option<u32>,
    },
    /// Page through the durable announce history.
    MeshAnnounces {
        before: Option<i64>,
        limit: Option<u32>,
    },
    /// Interface type catalogue and field schema (bridge-owned).
    DescribeInterfaces,
    ListInterfaces,
    /// Permanently dismiss the upgrade notice on the interfaces reply.
    DismissInterfaceNotice,
    CreateInterface {
        name: String,
        kind: String,
        config: serde_json::Value,
    },
    UpdateInterface {
        /// Not `id`: the request envelope already owns that key.
        interface_id: String,
        config: serde_json::Value,
    },
    SetInterfaceEnabled {
        interface_id: String,
        enabled: bool,
    },
    DeleteInterface {
        interface_id: String,
    },
    /// Enumerate devices for the `serial` widget. `kind` is "serial" for now.
    ListDevices {
        kind: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_roundtrip_with_flattened_tag() {
        let raw =
            r#"{"id":7,"cmd":"send_message","conversation_id":"c1","body":"hi","reply_to":null}"#;
        let cmd: Command = serde_json::from_str(raw).expect("parse");
        assert_eq!(cmd.id, 7);
        match cmd.cmd {
            ClientCmd::SendMessage {
                conversation_id,
                body,
                attachments,
                ..
            } => {
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

    #[test]
    fn interface_kind_serialises_as_kind() {
        let iface = MeshInterface {
            name: "AutoInterface".into(),
            kind: "AutoInterface".into(),
            hash: "ab".into(),
            online: true,
            bitrate: Some(10_000_000),
            mtu: Some(500),
            rxb: 1,
            txb: 2,
            rxs: 1.5,
            txs: 2.5,
            peers: Some(3),
            mode: Some("full".into()),
            ifac: None,
            radio: None,
            down_since: None,
        };
        let value = serde_json::to_value(&iface).unwrap();
        assert_eq!(value["kind"], "AutoInterface");
        assert!(value.get("type").is_none());
        assert!(value.get("radio").is_none(), "empty radio must be omitted");
        let back: MeshInterface = serde_json::from_value(value).unwrap();
        assert_eq!(back, iface);
    }

    #[test]
    fn mesh_status_roundtrips_and_defaults_are_lenient() {
        let status = MeshStatus {
            transport: "ready".into(),
            address: Some("ab".repeat(16)),
            started_at: Some(1_760_000_000),
            interfaces: vec![],
            counters: MeshCounters {
                rx_packets: 4,
                path_count: 2,
                ..Default::default()
            },
            peer_paths: vec![],
            peer_reachability: vec![],
            other_services: vec![],
        };
        let text = serde_json::to_string(&status).unwrap();
        let back: MeshStatus = serde_json::from_str(&text).unwrap();
        assert_eq!(back, status);

        // A client from an older build sends a snapshot without `mesh`.
        let raw = r#"{"identity":{"id":"i","display_name":"you","display_name_set":true,"address":null},
                      "transport":"offline","conversations":[],"peers":[]}"#;
        let snapshot: Snapshot = serde_json::from_str(raw).unwrap();
        assert!(snapshot.mesh.is_none());
    }

    #[test]
    fn mesh_status_keeps_radio_stats_and_ignores_unknown_fields() {
        let raw = r#"{
          "transport": "ready",
          "interfaces": [{
            "name": "RNode", "kind": "RNodeInterface", "online": true,
            "mode": "full", "peers": 2,
            "radio": {"rssi": -88.0, "snr": 7.5, "frequency": 868000000,
                      "sf": 8, "airtime_short": 1.2},
            "future_field": "ignored"
          }],
          "counters": {"path_count": 1, "unknown_counter": 5},
          "peer_paths": [], "other_services": [],
          "unknown_top_level": true
        }"#;
        let status: MeshStatus = serde_json::from_str(raw).unwrap();
        let iface = &status.interfaces[0];
        assert_eq!(iface.kind, "RNodeInterface");
        assert_eq!(iface.mode.as_deref(), Some("full"));
        assert_eq!(iface.peers, Some(2));
        let radio = iface.radio.as_ref().unwrap();
        assert_eq!(radio.rssi, Some(-88.0));
        assert_eq!(radio.snr, Some(7.5));
        assert_eq!(radio.frequency, Some(868_000_000.0));
        assert_eq!(radio.sf, Some(8.0));
        assert_eq!(radio.airtime_short, Some(1.2));
        assert_eq!(status.counters.path_count, 1);
        let value = serde_json::to_value(&status).unwrap();
        assert!(value.get("unknown_top_level").is_none());
        assert!(value["interfaces"][0].get("future_field").is_none());
    }

    #[test]
    fn mesh_commands_parse() {
        let raw = r#"{"id":1,"cmd":"mesh_paths","query":"ab","offset":100,"limit":50}"#;
        let cmd: Command = serde_json::from_str(raw).unwrap();
        match cmd.cmd {
            ClientCmd::MeshPaths {
                query,
                offset,
                limit,
            } => {
                assert_eq!(query.as_deref(), Some("ab"));
                assert_eq!(offset, Some(100));
                assert_eq!(limit, Some(50));
            }
            other => panic!("wrong variant: {other:?}"),
        }

        let raw = r#"{"id":2,"cmd":"set_interface_enabled","id":"row","enabled":false}"#;
        assert!(
            serde_json::from_str::<Command>(raw).is_err(),
            "id collides with the envelope"
        );
    }

    #[test]
    fn degraded_is_a_third_transport_state_a_client_can_show_verbatim() {
        assert_eq!(TransportState::Degraded.as_str(), "degraded");
        assert_eq!(
            serde_json::to_value(TransportState::Degraded).unwrap(),
            serde_json::json!("degraded")
        );
        assert_eq!(
            serde_json::from_str::<TransportState>("\"degraded\"").unwrap(),
            TransportState::Degraded
        );
    }

    #[test]
    fn an_offline_interface_carries_when_the_outage_began() {
        let raw = r#"{"name":"Relay","kind":"TCPClientInterface","online":false,
                      "down_since":1760000000000}"#;
        let iface: MeshInterface = serde_json::from_str(raw).unwrap();
        assert!(!iface.online);
        assert_eq!(iface.down_since, Some(1_760_000_000_000));

        // An online interface, or a bridge that does not sample it, omits it.
        let online: MeshInterface =
            serde_json::from_str(r#"{"name":"LAN","kind":"AutoInterface","online":true}"#).unwrap();
        assert_eq!(online.down_since, None);
    }

    #[test]
    fn peer_reachability_reports_absence_as_clearly_as_presence() {
        let reachable = PeerReachability {
            address: "aa".repeat(16),
            path: true,
            hops: Some(2),
            next_hop: Some("bb".repeat(16)),
            interface: Some("Relay".into()),
            bitrate: Some(62_500),
            state: "responsive".into(),
            last_announce: Some(1_760_000_000_000),
        };
        let text = serde_json::to_string(&reachable).unwrap();
        assert_eq!(
            serde_json::from_str::<PeerReachability>(&text).unwrap(),
            reachable
        );

        let absent: PeerReachability = serde_json::from_str(
            r#"{"address":"cc","path":false,"state":"none","last_announce":null}"#,
        )
        .unwrap();
        assert!(!absent.path);
        assert_eq!(absent.hops, None);
        assert_eq!(absent.state, "none");
        // An old bridge's mesh payload has no reachability list at all.
        let old: MeshStatus = serde_json::from_str(
            r#"{"transport":"ready","interfaces":[],"counters":{},
                "peer_paths":[],"other_services":[]}"#,
        )
        .unwrap();
        assert!(old.peer_reachability.is_empty());
    }

    #[test]
    fn the_upgrade_notice_carries_a_stable_kind_and_one_message() {
        let notice = InterfaceNotice {
            kind: "legacy-settings-not-imported".into(),
            message: "The old relay was not imported.".into(),
        };
        let value = serde_json::to_value(&notice).unwrap();
        let mut keys: Vec<_> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["kind", "message"]);
        assert_eq!(
            serde_json::from_value::<InterfaceNotice>(value).unwrap(),
            notice
        );
    }

    #[test]
    fn a_client_can_dismiss_the_upgrade_notice() {
        let raw = r#"{"id":9,"cmd":"dismiss_interface_notice"}"#;
        let cmd: Command = serde_json::from_str(raw).unwrap();
        assert!(matches!(cmd.cmd, ClientCmd::DismissInterfaceNotice));
    }

    #[test]
    fn interface_rows_carry_an_import_note_only_when_set() {
        let row = InterfaceRow {
            id: "i".into(),
            name: "Relay".into(),
            kind: "TCPClientInterface".into(),
            config: serde_json::json!({"target_host": "relay.example", "target_port": 4242}),
            enabled: true,
            origin: "legacy".into(),
            note: Some("Imported from the old relay setting".into()),
        };
        let value = serde_json::to_value(&row).unwrap();
        assert_eq!(value["origin"], "legacy");
        assert_eq!(value["note"], "Imported from the old relay setting");
        assert_eq!(serde_json::from_value::<InterfaceRow>(value).unwrap(), row);

        let plain = InterfaceRow { note: None, ..row };
        assert!(
            serde_json::to_value(&plain).unwrap().get("note").is_none(),
            "a row with nothing to explain stays the shape it always was"
        );
    }
}
