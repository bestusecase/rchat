//! Daemon wiring: HTTP/WebSocket surface, command handling, and the bridge
//! event pump.
//!
//! The daemon is the product; UIs are views. Everything here is independent of
//! whether the client is a Tauri window, a browser tab, or a test script.

use anyhow::{anyhow, bail, Context, Result};
use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use rchat_core::attachments::{
    ingest_bytes, BlobStore, Ingested, MAX_ATTACHMENTS, REASON_EVICTED, REASON_MISSING,
    REASON_TOO_LARGE, REASON_UNREADABLE,
};
use rchat_core::now_ms;
use rchat_core::store::Store;
use rchat_ipc::{
    AttachmentInfo, ClientCmd, Command, IdentityInfo, InterfaceRow, MeshCounters, MeshInterface,
    MeshStatus, MessageInfo, PathEntry, PeerInfo, PeerPath, PeerReachability, ServerMsg,
    ServiceEntry, Snapshot, TransportState, PROTOCOL_VERSION,
};
use rchat_rns::{BridgeAttachment, BridgeEvent};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path as FilePath;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, mpsc, oneshot, RwLock};
use tokio::time::{timeout, Duration};
use tower_http::cors::CorsLayer;
use tracing::{debug, warn};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How long a token-correlated bridge request may take before the engine gives
/// up. Applying an interface can involve a TCP connect, so it is generous.
const BRIDGE_REQUEST_TIMEOUT: Duration = Duration::from_secs(25);

/// Total attachment bytes one message may carry over a fast peer. A judgement
/// call: a phone photo fits, and an IP link delivers it.
pub const FAST_PEER_LIMIT_BYTES: u64 = 8 * 1024 * 1024;
/// A path that has not been measured yet is not assumed to be fast.
pub const UNKNOWN_PEER_LIMIT_BYTES: u64 = 1024 * 1024;
/// A LoRa-class link spends airtime, not bandwidth.
pub const RADIO_PEER_LIMIT_BYTES: u64 = 64 * 1024;
/// The largest payload the bridge will stage in either direction.
pub const INBOUND_LIMIT_BYTES: u64 = FAST_PEER_LIMIT_BYTES;
/// What all attachments on the device may use before eviction starts.
pub const ATTACHMENT_BUDGET_BYTES: u64 = 500 * 1024 * 1024;
/// The upload route accepts more than any permitted message so the engine's
/// own refusal is what a client sees, not a bare transport error.
pub const MAX_UPLOAD_BYTES: u64 = 16 * 1024 * 1024;
/// The longest name rchat keeps for display. A local display name and a peer
/// nickname share it, so neither can overflow a list row.
const MAX_NAME_CHARS: usize = 48;

/// Read a byte count override so a test (or a curious user) can force eviction
/// without writing half a gigabyte.
fn bytes_override(name: &str) -> Option<u64> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
}

pub fn attachment_budget() -> u64 {
    bytes_override("RCHAT_ATTACHMENT_BUDGET_BYTES").unwrap_or(ATTACHMENT_BUDGET_BYTES)
}

pub fn inbound_limit() -> u64 {
    bytes_override("RCHAT_ATTACHMENT_MAX_BYTES").unwrap_or(INBOUND_LIMIT_BYTES)
}

/// The per-message ceiling for one peer class.
pub fn peer_limit(bandwidth_class: &str) -> u64 {
    match bandwidth_class {
        "lan" | "relay" => FAST_PEER_LIMIT_BYTES,
        "radio" => RADIO_PEER_LIMIT_BYTES,
        _ => UNKNOWN_PEER_LIMIT_BYTES,
    }
}

/// The minimal path table the bridge samples, cached wholesale. Clients page it
/// through [`ClientCmd::MeshPaths`]; only known peers appear in `MeshStatus`.
pub struct AppState {
    pub store: Arc<Store>,
    pub token: String,
    /// Where attachment bytes live. Owned by the daemon, never derived here.
    pub blobs: Arc<BlobStore>,
    pub tx: broadcast::Sender<String>,
    pub rns: RwLock<Option<mpsc::UnboundedSender<String>>>,
    pub transport: RwLock<TransportState>,
    mesh: RwLock<MeshCache>,
    bridge_pending: Mutex<HashMap<u64, oneshot::Sender<BridgeAnswer>>>,
    bridge_token: AtomicU64,
    descriptor: RwLock<Option<Value>>,
    /// Network settings an older shell carried over, kept until the config
    /// import has had its turn.
    pub legacy_network: RwLock<Option<rchat_core::store::LegacyNetwork>>,
}

struct BridgeAnswer {
    ok: bool,
    data: Option<Value>,
    error: Option<String>,
}

#[derive(Debug, Default)]
struct MeshCache {
    transport: String,
    address: Option<String>,
    started_at: Option<i64>,
    interfaces: Vec<MeshInterface>,
    counters: MeshCounters,
    paths: Vec<PathEntry>,
    path_total: u64,
    paths_truncated: bool,
    other_services: Vec<ServiceEntry>,
}

/// The bridge's `mesh` payload. The engine keeps the full path table here and
/// hands out smaller, queryable views.
#[derive(Debug, Deserialize)]
struct BridgeMesh {
    #[serde(default)]
    transport: String,
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    started_at: Option<i64>,
    #[serde(default)]
    interfaces: Vec<MeshInterface>,
    #[serde(default)]
    counters: MeshCounters,
    #[serde(default)]
    path_total: u64,
    #[serde(default)]
    paths_truncated: bool,
    #[serde(default)]
    paths: Vec<PathEntry>,
    #[serde(default)]
    other_services: Vec<ServiceEntry>,
}

impl AppState {
    pub fn new(store: Arc<Store>, token: String, blobs: Arc<BlobStore>) -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self {
            store,
            token,
            blobs,
            tx,
            rns: RwLock::new(None),
            transport: RwLock::new(TransportState::Offline),
            mesh: RwLock::new(MeshCache {
                transport: "offline".into(),
                ..Default::default()
            }),
            bridge_pending: Mutex::new(HashMap::new()),
            bridge_token: AtomicU64::new(1),
            descriptor: RwLock::new(None),
            legacy_network: RwLock::new(None),
        }
    }

    pub async fn attach_transport(&self, sender: mpsc::UnboundedSender<String>) {
        *self.rns.write().await = Some(sender);
        *self.transport.write().await = TransportState::Starting;
    }

    fn identity_info(&self) -> Result<IdentityInfo> {
        let row = self
            .store
            .identity()?
            .context("identity has not been initialised")?;
        Ok(IdentityInfo {
            id: row.id,
            display_name: row.display_name,
            display_name_set: self.store.display_name_set()?,
            address: row.address,
            caps: Default::default(),
        })
    }

    fn publish(&self, name: &str, data: Value) -> Result<i64> {
        let seq = self.store.record_event(name, &data)?;
        let msg = ServerMsg::Event {
            seq,
            name: name.to_string(),
            data,
        };
        // A missing subscriber is normal (no window open yet).
        let _ = self.tx.send(serde_json::to_string(&msg)?);
        Ok(seq)
    }

    async fn handle_command(&self, cmd: ClientCmd) -> Result<Value> {
        match cmd {
            ClientCmd::Snapshot => {
                let snapshot = Snapshot {
                    identity: self.identity_info()?,
                    transport: *self.transport.read().await,
                    conversations: self.store.list_conversations()?,
                    peers: self.store.list_peers()?,
                    mesh: Some(self.mesh_status().await?),
                };
                Ok(serde_json::to_value(snapshot)?)
            }
            ClientCmd::ListMessages {
                conversation_id,
                before,
                limit,
            } => {
                let messages =
                    self.store
                        .list_messages(&conversation_id, before, limit.unwrap_or(80))?;
                let messages = self.present_all(messages);
                Ok(json!({ "conversation_id": conversation_id, "messages": messages }))
            }
            ClientCmd::SendMessage {
                conversation_id,
                body,
                reply_to,
                attachments,
            } => {
                self.send_message(&conversation_id, &body, reply_to, &attachments)
                    .await
            }
            ClientCmd::CreateConversation {
                kind,
                title,
                peer_ids,
            } => {
                if kind == "direct" {
                    let peer_id = peer_ids
                        .first()
                        .context("a direct conversation needs a peer")?;
                    let conv = self.store.open_direct(peer_id)?;
                    self.publish("conversation.updated", json!({ "conversation": conv }))?;
                    return Ok(json!({ "conversation": conv }));
                }
                if kind != "group" {
                    bail!("unknown conversation kind: {kind}");
                }
                if peer_ids.is_empty() {
                    bail!("a group needs at least one member");
                }
                for peer_id in &peer_ids {
                    if self.store.peer(peer_id)?.is_none() {
                        bail!("unknown peer: {peer_id}");
                    }
                }
                let conv = self.store.create_conversation("group", title.as_deref(), &peer_ids)?;
                self.publish("conversation.updated", json!({ "conversation": conv }))?;
                Ok(json!({ "conversation": conv }))
            }
            ClientCmd::OpenDirect { peer_id } => {
                let conv = self.store.open_direct(&peer_id)?;
                self.publish("conversation.updated", json!({ "conversation": conv }))?;
                Ok(json!({ "conversation": conv }))
            }
            ClientCmd::AddPeer { address, nickname } => {
                let address = address.trim().to_ascii_lowercase();
                if address.len() != 32 || !address.chars().all(|c| c.is_ascii_hexdigit()) {
                    bail!("expected a 32-character LXMF address (hex)");
                }
                let peer = PeerInfo {
                    id: address.clone(),
                    address: Some(address),
                    nickname: nickname.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()),
                    announced_name: None,
                    last_seen: Some(now_ms()),
                    bandwidth_class: "unknown".into(),
                    caps: Default::default(),
                };
                self.store.upsert_peer(&peer)?;
                self.publish("peer.updated", json!({ "peer": peer }))?;
                Ok(json!({ "peer": peer }))
            }
            ClientCmd::SetDisplayName { name } => {
                let name = name.trim();
                if name.is_empty() {
                    bail!("display name cannot be empty");
                }
                if name.chars().count() > MAX_NAME_CHARS {
                    bail!("display name is too long");
                }
                self.store.set_display_name(name)?;
                self.publish("identity.updated", json!({ "identity": self.identity_info()? }))?;
                if let Some(sender) = self.rns.read().await.clone() {
                    let _ = sender
                        .send(json!({ "cmd": "rename", "name": name }).to_string());
                }
                Ok(json!({ "identity": self.identity_info()? }))
            }
            ClientCmd::MarkRead { conversation_id } => {
                self.store.mark_read(&conversation_id)?;
                let conv = self.store.get_conversation(&conversation_id)?;
                self.publish("conversation.updated", json!({ "conversation": conv }))?;
                Ok(json!({}))
            }
            ClientCmd::Search { query, limit } => {
                let messages = self.store.search(&query, limit.unwrap_or(50))?;
                let messages = self.present_all(messages);
                Ok(json!({ "messages": messages }))
            }
            ClientCmd::EventsSince { seq } => {
                let events = self.store.events_since(seq)?;
                Ok(json!({ "events": events }))
            }
            ClientCmd::MeshStatus => Ok(json!({ "mesh": self.mesh_status().await? })),
            ClientCmd::MeshPaths {
                query,
                offset,
                limit,
            } => self.mesh_paths(query, offset, limit).await,
            ClientCmd::MeshAnnounces { before, limit } => {
                let announces = self.store.list_announces(before, limit.unwrap_or(100))?;
                Ok(json!({ "announces": announces }))
            }
            ClientCmd::DescribeInterfaces => Ok(self.describe_interfaces().await?),
            ClientCmd::ListInterfaces => {
                let interfaces = self.store.list_interfaces()?;
                let extra: Vec<Value> = self
                    .store
                    .extra_config()?
                    .into_iter()
                    .map(|(section, raw)| json!({ "section": section, "raw": raw }))
                    .collect();
                let read_only = self.interface_read_only(None).await;
                Ok(json!({
                    "interfaces": interfaces,
                    "extra": extra,
                    "read_only": read_only,
                }))
            }
            ClientCmd::CreateInterface {
                name,
                kind,
                config,
            } => self.create_interface(name, kind, config).await,
            ClientCmd::UpdateInterface {
                interface_id,
                config,
            } => self.update_interface(&interface_id, config).await,
            ClientCmd::SetInterfaceEnabled {
                interface_id,
                enabled,
            } => self.set_interface_enabled(&interface_id, enabled).await,
            ClientCmd::DeleteInterface { interface_id } => {
                self.delete_interface(&interface_id).await
            }
            ClientCmd::ListDevices { kind } => self.list_devices(&kind).await,
        }
    }

    async fn send_message(
        &self,
        conversation_id: &str,
        body: &str,
        reply_to: Option<String>,
        hashes: &[String],
    ) -> Result<Value> {
        let body = body.trim();
        if body.is_empty() && hashes.is_empty() {
            bail!("cannot send an empty message");
        }
        if body.chars().count() > 12_000 {
            bail!("message is too long for one Reticulum message");
        }
        if hashes.len() > MAX_ATTACHMENTS {
            bail!(
                "a message can carry at most {MAX_ATTACHMENTS} attachments, not {}",
                hashes.len()
            );
        }
        let identity = self.identity_info()?;
        let conv = self.store.get_conversation(conversation_id)?;

        // Resolve the bytes before anything is written: a refused send must not
        // leave a message row behind for the user to retry into the same wall.
        let mut attachments = Vec::with_capacity(hashes.len());
        for hash in hashes {
            let Some(row) = self.store.attachment_by_hash(hash)? else {
                bail!("that attachment was not uploaded to this engine");
            };
            if let Some(reason) = row.reason.as_deref() {
                bail!("\"{}\" cannot be sent: {reason}", row.name);
            }
            if row.hash.is_empty() || !self.blobs.contains(&row.hash) {
                bail!(
                    "\"{}\" is no longer on this device; attach it again",
                    row.name
                );
            }
            attachments.push(row);
        }
        let (class, limit) = self.target_limit(&conv.peer_ids)?;
        let total: u64 = attachments.iter().map(|item| item.size).sum();
        if total > limit {
            bail!(
                "These attachments total {}, which is over the {} limit for a peer with \
                 bandwidth class \"{}\". Send fewer or smaller files.",
                human_bytes(total),
                human_bytes(limit),
                class
            );
        }

        let kind = message_kind(&attachments);
        let msg = MessageInfo {
            id: uuid::Uuid::new_v4().to_string(),
            conversation_id: conv.id.clone(),
            author_id: identity.id.clone(),
            ts: now_ms(),
            body: body.to_string(),
            kind: kind.to_string(),
            reply_to,
            state: "queued".into(),
            route: None,
            edited_at: None,
            attachments: attachments.clone(),
        };
        self.store
            .insert_message_with_attachments(&msg, false, &attachments)?;
        if !hashes.is_empty() {
            self.store
                .forget_unclaimed(&attachments.iter().map(|item| item.hash.clone()).collect::<Vec<_>>())?;
        }
        self.publish("message.new", json!({ "message": msg }))?;
        self.publish(
            "conversation.updated",
            json!({ "conversation": self.store.get_conversation(&conv.id)? }),
        )?;

        self.dispatch(&msg).await?;

        let stored = self.store.get_message(&msg.id)?.unwrap_or(msg);
        Ok(json!({ "message": stored }))
    }

    /// The strictest ceiling among the peers this message reaches, and the class
    /// that produced it. A group pays the cost of its slowest member.
    fn target_limit(&self, peer_ids: &[String]) -> Result<(String, u64)> {
        let mut worst: Option<(String, u64)> = None;
        for peer_id in peer_ids {
            let class = self
                .store
                .peer(peer_id)?
                .map(|peer| peer.bandwidth_class)
                .unwrap_or_else(|| "unknown".to_string());
            let limit = peer_limit(&class);
            let replace = match &worst {
                Some((_, current)) => limit < *current,
                None => true,
            };
            if replace {
                worst = Some((class, limit));
            }
        }
        Ok(worst.unwrap_or_else(|| ("unknown".to_string(), UNKNOWN_PEER_LIMIT_BYTES)))
    }

    /// Hand one message to every addressed member. Group sends are a fan-out in
    /// v1: no server-side relay, which keeps the no-infrastructure promise.
    async fn dispatch(&self, msg: &MessageInfo) -> Result<()> {
        if *self.transport.read().await != TransportState::Ready {
            return Ok(());
        }
        let Some(sender) = self.rns.read().await.clone() else {
            return Ok(());
        };
        let conv = self.store.get_conversation(&msg.conversation_id)?;
        // Bytes stay on disk; the bridge reads the paths it is handed.
        let attachments: Vec<Value> = msg
            .attachments
            .iter()
            .filter(|item| item.has_bytes())
            .map(|item| {
                json!({
                    "name": item.name,
                    "path": self.blobs.path(&item.hash).to_string_lossy(),
                })
            })
            .collect();
        let mut targets = 0usize;
        for peer_id in &conv.peer_ids {
            let Some(peer) = self.store.peer(peer_id)? else {
                continue;
            };
            let Some(address) = peer.address.as_deref() else {
                continue;
            };
            let mut payload = json!({
                "cmd": "send",
                "id": msg.id,
                "to": address,
                "body": msg.body,
                "title": "",
            });
            if !attachments.is_empty() {
                payload["attachments"] = Value::Array(attachments.clone());
            }
            if sender.send(payload.to_string()).is_ok() {
                targets += 1;
            }
        }
        if targets > 0 {
            self.store.update_message_state(&msg.id, "sending")?;
            self.publish(
                "message.state",
                json!({ "id": msg.id, "state": "sending", "route": "reticulum" }),
            )?;
        }
        Ok(())
    }

    pub async fn flush_queued(&self) {
        if *self.transport.read().await != TransportState::Ready {
            return;
        }
        match self.store.queued_messages() {
            Ok(messages) => {
                for msg in messages {
                    if let Err(err) = self.dispatch(&msg).await {
                        warn!(error = %err, "queued message dispatch failed");
                    }
                }
            }
            Err(err) => warn!(error = %err, "could not read queued messages"),
        }
    }

    pub async fn on_bridge_event(self: &Arc<Self>, event: BridgeEvent) {
        match event {
            BridgeEvent::State {
                state,
                address,
                name,
                detail,
            } => {
                let derived = {
                    let mut mesh = self.mesh.write().await;
                    mesh.transport = state.as_str().to_string();
                    if address.is_some() {
                        mesh.address = address.clone();
                    }
                    if state != TransportState::Ready {
                        // The interface sample belongs to the previous bridge
                        // session. Keeping it would let the first "ready" after
                        // a restart claim an online interface that has not been
                        // reattached yet, then degrade a moment later.
                        mesh.interfaces.clear();
                    }
                    derived_transport(&mesh.transport, &mesh.interfaces)
                };
                *self.transport.write().await = derived;
                // A new connection may speak a different RNS version.
                *self.descriptor.write().await = None;
                if let Some(address) = address.as_deref() {
                    if let Err(err) = self.store.set_address(Some(address)) {
                        warn!(error = %err, "could not persist transport address");
                    }
                }
                if let Some(name) = name.as_deref() {
                    debug!(name, "reticulum bridge reported display name");
                }
                if let Some(detail) = detail.as_deref() {
                    debug!(detail, "transport state detail");
                }
                let _ = self.publish(
                    "transport.state",
                    json!({
                        "transport": derived,
                        "address": address,
                        "detail": detail.or_else(|| derived_detail(derived)),
                    }),
                );
                let _ = self.publish_mesh().await;
                if state == TransportState::Ready {
                    // Requests need the event pump to keep draining bridge
                    // replies, so this cannot run inline.
                    let state = self.clone();
                    tokio::spawn(async move {
                        if let Err(err) = state.ensure_bridge_config().await {
                            warn!(error = %err, "could not configure the bridge");
                        }
                        state.flush_queued().await;
                    });
                }
                if state == TransportState::Starting {
                    // A managed bridge waits briefly for its interface set
                    // before starting Reticulum, so send it now rather than
                    // after `ready` and pay the grace period on every launch.
                    // No rows means a hand-written config owns the list, and
                    // rendering an empty one here would destroy it.
                    let state = self.clone();
                    tokio::spawn(async move {
                        match state.store.list_interfaces() {
                            Ok(rows) if !rows.is_empty() => {
                                if let Err(err) = state.apply_rows(&rows).await {
                                    warn!(error = %err, "could not preconfigure the bridge");
                                }
                            }
                            Ok(_) => {}
                            Err(err) => {
                                warn!(error = %err, "could not read interfaces");
                            }
                        }
                    });
                }
            }
            BridgeEvent::Message {
                id,
                from,
                body,
                ts,
                attachments,
            } => {
                if let Err(err) = self
                    .on_incoming(id, from, body, ts, attachments)
                    .await
                {
                    warn!(error = %err, "failed to store incoming message");
                }
            }
            BridgeEvent::Announce {
                address,
                name,
                aspect,
                interface,
                hops,
            } => {
                if address.len() != 32 {
                    return;
                }
                // Ignore our own announce echoed back through the mesh.
                if let Ok(Some(me)) = self.store.identity() {
                    if me.address.as_deref() == Some(address.as_str()) {
                        return;
                    }
                }
                let ts = now_ms();
                match self.store.record_announce(
                    &address,
                    name.as_deref(),
                    aspect.as_deref(),
                    interface.as_deref(),
                    hops,
                    ts,
                ) {
                    Ok(sighting) => {
                        let _ = self.publish(
                            "announce.received",
                            json!({ "announce": sighting }),
                        );
                    }
                    Err(err) => warn!(error = %err, "could not record announce"),
                }
                // Keep the existing bandwidth class: it comes from the path
                // table, not from this announce.
                let peer = match self.store.peer_by_address(&address) {
                    Ok(Some(mut peer)) => {
                        if name.is_some() {
                            peer.announced_name = name;
                        }
                        peer.last_seen = Some(ts);
                        peer
                    }
                    Ok(None) => PeerInfo {
                        id: address.clone(),
                        address: Some(address.clone()),
                        nickname: None,
                        announced_name: name,
                        last_seen: Some(ts),
                        bandwidth_class: "unknown".into(),
                        caps: Default::default(),
                    },
                    Err(err) => {
                        warn!(error = %err, "could not read announced peer");
                        return;
                    }
                };
                if let Err(err) = self.store.upsert_peer(&peer) {
                    warn!(error = %err, "could not store announced peer");
                    return;
                }
                let _ = self.publish("peer.updated", json!({ "peer": peer }));
            }
            BridgeEvent::Mesh { mesh } => {
                match serde_json::from_value::<BridgeMesh>(mesh) {
                    Ok(payload) => {
                        {
                            let mut cache = self.mesh.write().await;
                            if !payload.transport.is_empty() {
                                cache.transport = payload.transport;
                            }
                            if payload.address.is_some() {
                                cache.address = payload.address;
                            }
                            if payload.started_at.is_some() {
                                cache.started_at = payload.started_at;
                            }
                            cache.interfaces = payload.interfaces;
                            cache.counters = payload.counters;
                            cache.paths = payload.paths;
                            cache.path_total = payload.path_total;
                            cache.paths_truncated = payload.paths_truncated;
                            cache.other_services = payload.other_services;
                        }
                        // Interfaces decide whether the mesh can carry anything,
                        // so the transport word follows the sample.
                        self.sync_transport().await;
                        if let Err(err) = self.publish_mesh().await {
                            warn!(error = %err, "could not publish mesh state");
                        }
                        self.refresh_bandwidth_classes().await;
                    }
                    Err(err) => warn!(error = %err, "could not parse the bridge mesh event"),
                }
            }
            BridgeEvent::Reply {
                token,
                ok,
                data,
                error,
            } => {
                let waiter = self.bridge_pending.lock().unwrap().remove(&token);
                if let Some(waiter) = waiter {
                    let _ = waiter.send(BridgeAnswer { ok, data, error });
                }
            }
            BridgeEvent::Receipt { id, state } => {
                let mapped = match state.as_str() {
                    "sent" => "sent",
                    "delivered" => "delivered",
                    "failed" => "failed",
                    other => other,
                };
                if let Err(err) = self.store.update_message_state(&id, mapped) {
                    warn!(error = %err, "could not update message receipt");
                    return;
                }
                let _ = self.publish(
                    "message.state",
                    json!({ "id": id, "state": mapped, "route": "reticulum" }),
                );
            }
            BridgeEvent::Error { message } => {
                warn!(message, "reticulum bridge error");
                let _ = self.publish("transport.error", json!({ "message": message }));
            }
            BridgeEvent::Log { level, message } => {
                debug!(level, message, "bridge");
            }
        }
    }

    /// Build and broadcast the client-facing mesh picture. Cheap enough to do
    /// on every bridge sample: only known peers are included, not the table.
    async fn publish_mesh(&self) -> Result<()> {
        let mesh = self.mesh_status().await?;
        self.publish("mesh.updated", json!({ "mesh": mesh }))?;
        Ok(())
    }

    /// Re-derive the transport word from the cached interface sample and tell
    /// clients when it moved. Called on every mesh sample, so a relay that goes
    /// quiet turns the status to `degraded` without waiting for a bridge
    /// restart or a client poll.
    async fn sync_transport(self: &Arc<Self>) {
        let (next, address) = {
            let cache = self.mesh.read().await;
            (
                derived_transport(&cache.transport, &cache.interfaces),
                cache.address.clone(),
            )
        };
        let changed = {
            let mut current = self.transport.write().await;
            if *current == next {
                false
            } else {
                *current = next;
                true
            }
        };
        if changed {
            let _ = self.publish(
                "transport.state",
                json!({
                    "transport": next,
                    "address": address,
                    "detail": derived_detail(next),
                }),
            );
            if next == TransportState::Ready {
                // The bridge may already have said "ready" while nothing was
                // online, and the queue that piled up in the meantime has to
                // move now, not on the next bridge restart.
                let state = self.clone();
                tokio::spawn(async move { state.flush_queued().await });
            }
        }
    }

    async fn mesh_status(&self) -> Result<MeshStatus> {
        let cache = self.mesh.read().await;
        let peers = self.store.list_peers()?;
        let by_address: HashMap<&str, &PeerInfo> = peers
            .iter()
            .filter_map(|peer| peer.address.as_deref().map(|address| (address, peer)))
            .collect();
        let peer_paths = cache
            .paths
            .iter()
            .filter_map(|path| by_address.get(path.destination.as_str()).map(|_| path))
            .map(|path| PeerPath {
                address: path.destination.clone(),
                hops: path.hops,
                next_hop: path.next_hop.clone(),
                interface: path.interface.clone(),
                bitrate: path.bitrate,
                state: path.state.clone(),
                age: path.age,
            })
            .collect();
        // Every known peer, not only the ones with a path: "outside, no route"
        // is the answer that stops a user waiting for something undeliverable.
        let paths_by_destination: HashMap<&str, &PathEntry> = cache
            .paths
            .iter()
            .map(|path| (path.destination.as_str(), path))
            .collect();
        let peer_reachability = peers
            .iter()
            .filter_map(|peer| {
                let address = peer.address.as_deref()?;
                let path = paths_by_destination.get(address).copied();
                Some(PeerReachability {
                    address: address.to_string(),
                    path: path.is_some(),
                    hops: path.map(|path| path.hops),
                    next_hop: path.map(|path| path.next_hop.clone()),
                    interface: path.map(|path| path.interface.clone()),
                    bitrate: path.and_then(|path| path.bitrate),
                    state: path
                        .map(|path| path.state.clone())
                        .unwrap_or_else(|| "none".to_string()),
                    // From the announce history, not `peer.last_seen`: adding a
                    // peer by address or receiving a message also moves that.
                    last_announce: self.store.latest_announce(address).ok().flatten(),
                })
            })
            .collect();
        let mut counters = cache.counters.clone();
        counters.path_count = cache.path_total;
        let transport = derived_transport(&cache.transport, &cache.interfaces)
            .as_str()
            .to_string();
        Ok(MeshStatus {
            transport,
            address: cache.address.clone(),
            started_at: cache.started_at,
            interfaces: cache.interfaces.clone(),
            counters,
            peer_paths,
            peer_reachability,
            other_services: cache.other_services.clone(),
        })
    }

    /// Page the cached path table. Ordered by hops, then most recently used.
    async fn mesh_paths(
        &self,
        query: Option<String>,
        offset: Option<u32>,
        limit: Option<u32>,
    ) -> Result<Value> {
        let peers = self.store.list_peers()?;
        let by_address: HashMap<String, &PeerInfo> = peers
            .iter()
            .filter_map(|peer| peer.address.clone().map(|address| (address, peer)))
            .collect();
        let needle = query.unwrap_or_default().trim().to_lowercase();
        let (paths, total, truncated) = {
            let cache = self.mesh.read().await;
            let mut filtered: Vec<PathEntry> = cache
                .paths
                .iter()
                .filter(|path| {
                    if needle.is_empty() {
                        return true;
                    }
                    if path.destination.to_lowercase().starts_with(&needle) {
                        return true;
                    }
                    if path.interface.to_lowercase().contains(&needle) {
                        return true;
                    }
                    by_address
                        .get(&path.destination)
                        .map(|peer| peer.label().to_lowercase().contains(&needle))
                        .unwrap_or(false)
                })
                .cloned()
                .collect();
            for path in &mut filtered {
                if let Some(peer) = by_address.get(&path.destination) {
                    path.name = Some(peer.label());
                }
            }
            filtered.sort_by(|a, b| {
                a.hops.cmp(&b.hops).then(
                    a.age
                        .partial_cmp(&b.age)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
            });
            let total = filtered.len();
            let offset = offset.unwrap_or(0) as usize;
            let limit = limit.unwrap_or(100).clamp(1, 500) as usize;
            (
                filtered.into_iter().skip(offset).take(limit).collect::<Vec<_>>(),
                total,
                cache.paths_truncated,
            )
        };
        Ok(json!({ "paths": paths, "total": total, "truncated": truncated }))
    }

    /// Turn a path's interface bitrate into the coarse class the peer badge
    /// shows, and publish only when it actually moved.
    async fn refresh_bandwidth_classes(&self) {
        let paths = self.mesh.read().await.paths.clone();
        let peers = match self.store.list_peers() {
            Ok(peers) => peers,
            Err(err) => {
                warn!(error = %err, "could not read peers for bandwidth classes");
                return;
            }
        };
        let by_address: HashMap<&str, &PeerInfo> = peers
            .iter()
            .filter_map(|peer| peer.address.as_deref().map(|address| (address, peer)))
            .collect();
        let live: std::collections::HashSet<&str> = paths
            .iter()
            .map(|path| path.destination.as_str())
            .collect();
        for path in &paths {
            let Some(peer) = by_address.get(path.destination.as_str()) else {
                continue;
            };
            let class = bandwidth_class(path.bitrate);
            if peer.bandwidth_class == class {
                continue;
            }
            match self.store.update_peer_bandwidth(&peer.id, class) {
                Ok(true) => {
                    let mut updated = (*peer).clone();
                    updated.bandwidth_class = class.to_string();
                    let _ = self.publish("peer.updated", json!({ "peer": updated }));
                }
                Ok(false) => {}
                Err(err) => warn!(error = %err, "could not update peer bandwidth class"),
            }
        }
        // A peer whose path went away is no longer classifiable.
        for peer in &peers {
            let Some(address) = peer.address.as_deref() else {
                continue;
            };
            if live.contains(address) || peer.bandwidth_class == "unknown" {
                continue;
            }
            if let Ok(true) = self.store.update_peer_bandwidth(&peer.id, "unknown") {
                let mut updated = (*peer).clone();
                updated.bandwidth_class = "unknown".into();
                let _ = self.publish("peer.updated", json!({ "peer": updated }));
            }
        }
    }

    /// Send a token-correlated request to the bridge and await its answer.
    async fn bridge_request(&self, mut payload: Value, wait: Duration) -> Result<Value> {
        let sender = self
            .rns
            .read()
            .await
            .clone()
            .context("the Reticulum bridge is not running")?;
        if !payload.is_object() {
            bail!("bridge requests must be JSON objects");
        }
        let token = self.bridge_token.fetch_add(1, Ordering::Relaxed);
        payload["token"] = json!(token);
        let (tx, rx) = oneshot::channel();
        self.bridge_pending.lock().unwrap().insert(token, tx);
        if sender.send(payload.to_string()).is_err() {
            self.bridge_pending.lock().unwrap().remove(&token);
            bail!("the Reticulum bridge is not accepting commands");
        }
        match timeout(wait, rx).await {
            Ok(Ok(answer)) if answer.ok => Ok(answer.data.unwrap_or_else(|| json!({}))),
            Ok(Ok(answer)) => Err(anyhow!(
                answer
                    .error
                    .unwrap_or_else(|| "the Reticulum bridge rejected the request".into())
            )),
            Ok(Err(_)) => bail!("the Reticulum bridge dropped the request"),
            Err(_) => {
                self.bridge_pending.lock().unwrap().remove(&token);
                bail!("the Reticulum bridge did not answer in time")
            }
        }
    }

    /// First start after an upgrade imports the hand-written config once, then
    /// every bridge start receives the same rows so a restarted bridge comes up
    /// with identical interfaces.
    async fn ensure_bridge_config(&self) -> Result<()> {
        let imported = self
            .store
            .meta("mesh_config_imported")?
            .as_deref()
            == Some("1");
        if !imported {
            match self
                .bridge_request(json!({ "cmd": "import_config" }), Duration::from_secs(10))
                .await
            {
                Ok(data) => {
                    if let Some(items) = data.get("interfaces").and_then(Value::as_array) {
                        for item in items {
                            let name = item
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                            let kind = item
                                .get("kind")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                            let config = item
                                .get("config")
                                .cloned()
                                .unwrap_or_else(|| json!({}));
                            if name.is_empty() || kind.is_empty() || !config.is_object() {
                                continue;
                            }
                            if self.store.interface_by_name(&name)?.is_some() {
                                continue;
                            }
                            let enabled = item
                                .get("enabled")
                                .and_then(Value::as_bool)
                                .unwrap_or(true);
                            if let Err(err) = self.store.create_interface(
                                &name,
                                &kind,
                                &config,
                                enabled,
                                "imported",
                            ) {
                                warn!(name, error = %err, "could not import interface");
                            }
                        }
                    }
                    if let Some(blocks) = data.get("extra").and_then(Value::as_array) {
                        let mut extra = Vec::new();
                        for block in blocks {
                            let Some(pair) = block.as_array() else { continue };
                            let (Some(section), Some(raw)) =
                                (pair.first().and_then(Value::as_str), pair.get(1).and_then(Value::as_str))
                            else {
                                continue;
                            };
                            extra.push((section.to_string(), raw.to_string()));
                        }
                        self.store.replace_extra_config(&extra)?;
                    }
                    self.store.set_meta("mesh_config_imported", "1")?;
                    debug!("imported the existing Reticulum interface configuration");
                }
                Err(err) => warn!(error = %err, "could not import rns/config"),
            }
        }

        let rows = self.store.list_interfaces()?;
        if rows.is_empty() {
            // A config that existed was authoritative until now. Once it has
            // been read (or if it had no interfaces at all), finish first-run
            // setup so a fresh install still starts with discovery.
            let legacy = self.legacy_network.read().await.clone();
            let rows = self
                .store
                .import_network_once(legacy.as_ref(), false)?;
            if rows.is_empty() {
                return Ok(());
            }
            return self.apply_rows(&rows).await;
        }
        self.apply_rows(&rows).await
    }

    async fn apply_rows(&self, rows: &[InterfaceRow]) -> Result<()> {
        let extra = self.store.extra_config()?;
        let payload = json!({
            "cmd": "configure",
            "interfaces": rows,
            "extra": extra,
        });
        self.bridge_request(payload, BRIDGE_REQUEST_TIMEOUT).await?;
        Ok(())
    }

    /// Apply the desired set, telling every client which interface was
    /// refused. The row is not persisted, so the page keeps the user's edits.
    async fn apply_or_report(&self, rows: &[InterfaceRow], name: &str) -> Result<()> {
        if let Err(err) = self.apply_rows(rows).await {
            let _ = self.publish(
                "interface.apply_failed",
                json!({ "name": name, "error": err.to_string() }),
            );
            return Err(err);
        }
        Ok(())
    }

    async fn describe_interfaces(&self) -> Result<Value> {
        if let Some(cached) = self.descriptor.read().await.clone() {
            return Ok(cached);
        }
        let data = self
            .bridge_request(json!({ "cmd": "describe" }), Duration::from_secs(10))
            .await?;
        *self.descriptor.write().await = Some(data.clone());
        Ok(data)
    }

    /// Why interface changes are unavailable, if they are. The bridge owns the
    /// answer: RNS refuses some types outright, and a shared instance owns its
    /// own interfaces. `None` means edits are allowed.
    async fn interface_read_only(&self, kind: Option<&str>) -> Option<String> {
        match self.describe_interfaces().await {
            Ok(descriptor) => read_only_from_descriptor(&descriptor, kind),
            Err(_) => Some(BRIDGE_OFFLINE_REASON.to_string()),
        }
    }

    async fn list_devices(&self, kind: &str) -> Result<Value> {
        self.bridge_request(
            json!({ "cmd": "list_devices", "kind": kind }),
            Duration::from_secs(10),
        )
        .await
    }

    async fn create_interface(&self, name: String, kind: String, config: Value) -> Result<Value> {
        validate_interface_name(&name)?;
        let kind = kind.trim().to_string();
        if kind.is_empty() {
            bail!("choose an interface type");
        }
        check_config(&config)?;
        validate_interface_config(&kind, &config)?;
        let name = name.trim().to_string();
        if let Some(reason) = self.interface_read_only(Some(&kind)).await {
            bail!("{reason}");
        }
        if self.store.interface_by_name(&name)?.is_some() {
            bail!("an interface named \"{name}\" already exists");
        }
        let mut rows = self.store.list_interfaces()?;
        let row = InterfaceRow {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            kind,
            config,
            enabled: true,
            origin: "managed".into(),
            note: None,
        };
        rows.push(row.clone());
        self.apply_or_report(&rows, &row.name).await?;
        self.store.insert_interface_row(&row)?;
        self.publish("interface.changed", json!({ "interface": row }))?;
        Ok(json!({ "interface": row }))
    }

    async fn update_interface(&self, id: &str, config: Value) -> Result<Value> {
        check_config(&config)?;
        let existing = self
            .store
            .interface(id)?
            .with_context(|| format!("unknown interface: {id}"))?;
        validate_interface_config(&existing.kind, &config)?;
        if let Some(reason) = self.interface_read_only(Some(&existing.kind)).await {
            bail!("{reason}");
        }
        let name = existing.name.clone();
        let candidate = InterfaceRow {
            config: config.clone(),
            ..existing
        };
        let rows: Vec<InterfaceRow> = self
            .store
            .list_interfaces()?
            .into_iter()
            .map(|row| if row.id == id { candidate.clone() } else { row })
            .collect();
        self.apply_or_report(&rows, &name).await?;
        let saved = self.store.update_interface_config(id, &config)?;
        self.publish("interface.changed", json!({ "interface": saved }))?;
        Ok(json!({ "interface": saved }))
    }

    async fn set_interface_enabled(&self, id: &str, enabled: bool) -> Result<Value> {
        let existing = self
            .store
            .interface(id)?
            .with_context(|| format!("unknown interface: {id}"))?;
        if existing.enabled == enabled {
            return Ok(json!({ "interface": existing }));
        }
        if let Some(reason) = self.interface_read_only(Some(&existing.kind)).await {
            bail!("{reason}");
        }
        let rows: Vec<InterfaceRow> = self
            .store
            .list_interfaces()?
            .into_iter()
            .map(|row| {
                if row.id == id {
                    InterfaceRow { enabled, ..row }
                } else {
                    row
                }
            })
            .collect();
        self.apply_or_report(&rows, &existing.name).await?;
        let saved = self.store.set_interface_enabled(id, enabled)?;
        self.publish("interface.changed", json!({ "interface": saved }))?;
        Ok(json!({ "interface": saved }))
    }

    async fn delete_interface(&self, id: &str) -> Result<Value> {
        // Not gated on read-only: deleting a row is how a user stops rchat
        // owning an interface RNS refuses to detach. The apply still fails and
        // rolls back if the live instance will not let it go.
        let existing = self
            .store
            .interface(id)?
            .with_context(|| format!("unknown interface: {id}"))?;
        let rows: Vec<InterfaceRow> = self
            .store
            .list_interfaces()?
            .into_iter()
            .filter(|row| row.id != id)
            .collect();
        self.apply_or_report(&rows, &existing.name).await?;
        self.store.delete_interface(id)?;
        self.publish("interface.changed", json!({ "deleted": id }))?;
        Ok(json!({ "deleted": id, "interface": existing }))
    }

    /// Attach the bytes' presence to descriptors before they reach a client.
    /// A row whose blob vanished (evicted, or manually deleted) still reads back
    /// with its metadata and a reason.
    fn present(&self, message: &mut MessageInfo) {
        for attachment in &mut message.attachments {
            if attachment.reason.is_none() && !self.blobs.contains(&attachment.hash) {
                attachment.reason = Some(REASON_MISSING.to_string());
                attachment.thumbnail = None;
                continue;
            }
            if let Some(thumbnail) = attachment.thumbnail.as_deref() {
                if !self.blobs.contains_thumbnail(thumbnail) {
                    attachment.thumbnail = None;
                }
            }
        }
    }

    fn present_all(&self, mut messages: Vec<MessageInfo>) -> Vec<MessageInfo> {
        for message in &mut messages {
            self.present(message);
        }
        messages
    }

    /// Keep the attachment store inside its budget. Received blobs go first,
    /// then sent ones, oldest first in both passes. Rows stay; only bytes go.
    pub(crate) fn enforce_attachment_budget(&self) -> usize {
        let budget = attachment_budget();
        let mut usage = match self.store.attachment_usage() {
            Ok(usage) => usage,
            Err(err) => {
                warn!(error = %err, "could not read attachment usage");
                return 0;
            }
        };
        if usage <= budget {
            return 0;
        }
        let mut evicted = 0;
        for received in [true, false] {
            let candidates = match self.store.evictable_hashes(received) {
                Ok(candidates) => candidates,
                Err(err) => {
                    warn!(error = %err, "could not list attachment candidates");
                    return evicted;
                }
            };
            for (hash, size) in candidates {
                if usage <= budget {
                    return evicted;
                }
                match self.store.evict_attachment(&self.blobs, &hash, REASON_EVICTED) {
                    Ok(true) => {
                        usage = usage.saturating_sub(size);
                        evicted += 1;
                        let _ = self.publish(
                            "attachment.evicted",
                            json!({ "hash": hash, "reason": REASON_EVICTED }),
                        );
                    }
                    Ok(false) => {}
                    Err(err) => warn!(error = %err, "could not evict an attachment"),
                }
            }
        }
        evicted
    }

    async fn on_incoming(
        &self,
        remote_id: String,
        from: String,
        body: String,
        ts: i64,
        staged: Vec<BridgeAttachment>,
    ) -> Result<()> {
        if from.len() != 32 {
            bail!("incoming message had an invalid source address");
        }
        let identity = self.identity_info()?;
        if identity.address.as_deref() == Some(from.as_str()) {
            return Ok(());
        }
        let peer = match self.store.peer_by_address(&from)? {
            Some(peer) => peer,
            None => {
                let peer = PeerInfo {
                    id: from.clone(),
                    address: Some(from.clone()),
                    nickname: None,
                    announced_name: None,
                    last_seen: Some(now_ms()),
                    bandwidth_class: "unknown".into(),
                    caps: Default::default(),
                };
                self.store.upsert_peer(&peer)?;
                let _ = self.publish("peer.updated", json!({ "peer": peer }));
                peer
            }
        };
        let conv = self.store.open_direct(&peer.id)?;

        // LXMF can redeliver; the remote message hash makes this idempotent.
        let id = if remote_id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            format!("rns-{remote_id}")
        };
        if self.store.get_message(&id)?.is_some() {
            self.discard_staged(&staged);
            return Ok(());
        }
        // Keep our own clock authoritative for ordering.
        let ts = ts.max(1);

        let mut attachments = Vec::new();
        let limit = inbound_limit();
        for (index, item) in staged.iter().enumerate() {
            let ordinal = item.ordinal as usize;
            let _ = ordinal;
            let name = if item.name.is_empty() {
                format!("attachment-{}", index + 1)
            } else {
                item.name.clone()
            };
            if index >= MAX_ATTACHMENTS {
                self.discard_staged(std::slice::from_ref(item));
                continue;
            }
            let reason = item.reason.clone().or_else(|| {
                if item.size > limit {
                    Some(REASON_TOO_LARGE.to_string())
                } else {
                    None
                }
            });
            if let Some(reason) = reason {
                let mime = rchat_core::attachments::sniff_mime(&name, &[], None);
                attachments.push(AttachmentInfo {
                    name,
                    mime,
                    size: item.size,
                    hash: String::new(),
                    width: None,
                    height: None,
                    thumbnail: None,
                    reason: Some(reason),
                });
                self.discard_staged(std::slice::from_ref(item));
                continue;
            }
            match self.ingest_staged(item, &name).await {
                Ok(ingested) => attachments.push(ingested.descriptor()),
                Err(err) => {
                    warn!(%name, error = %err, "could not ingest an incoming attachment");
                    let mime = rchat_core::attachments::sniff_mime(&name, &[], None);
                    attachments.push(AttachmentInfo {
                        name,
                        mime,
                        size: item.size,
                        hash: String::new(),
                        width: None,
                        height: None,
                        thumbnail: None,
                        reason: Some(REASON_UNREADABLE.to_string()),
                    });
                }
            }
            self.discard_staged(std::slice::from_ref(item));
        }

        let kind = message_kind(&attachments);
        let msg = MessageInfo {
            id,
            conversation_id: conv.id.clone(),
            author_id: peer.id.clone(),
            ts,
            body,
            kind: kind.to_string(),
            reply_to: None,
            state: "delivered".into(),
            route: Some("reticulum".into()),
            edited_at: None,
            attachments: attachments.clone(),
        };
        self.store
            .insert_message_with_attachments(&msg, true, &attachments)?;
        self.enforce_attachment_budget();
        self.publish("message.new", json!({ "message": msg }))?;
        self.publish(
            "conversation.updated",
            json!({ "conversation": self.store.get_conversation(&conv.id)? }),
        )?;
        Ok(())
    }

    /// Move one staged payload into the blob store. Staging is deleted by the
    /// caller; the engine never reads a path twice.
    async fn ingest_staged(&self, item: &BridgeAttachment, name: &str) -> Result<Ingested> {
        if item.path.is_empty() {
            bail!("the bridge reported no staged path");
        }
        let source = FilePath::new(&item.path).to_path_buf();
        let blobs = self.blobs.clone();
        let name = name.to_string();
        tokio::task::spawn_blocking(move || {
            rchat_core::attachments::ingest(&blobs, &source, &name, None)
        })
        .await
        .context("the attachment ingest task panicked")?
    }

    fn discard_staged(&self, staged: &[BridgeAttachment]) {
        for item in staged {
            if item.path.is_empty() {
                continue;
            }
            let _ = std::fs::remove_file(&item.path);
        }
    }
}

#[derive(Deserialize)]
struct AuthQuery {
    token: Option<String>,
}

#[derive(Deserialize)]
struct DownloadQuery {
    token: Option<String>,
    variant: Option<String>,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/ws", get(ws_handler))
        .route(
            "/attachments",
            post(upload_handler).layer(DefaultBodyLimit::max(
                (MAX_UPLOAD_BYTES as usize).saturating_add(4096),
            )),
        )
        .route("/attachments/{hash}", get(download_handler))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

/// Accept raw bytes and turn them into a descriptor. Nothing is attached to a
/// message until the send names the hash.
async fn upload_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<AuthQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !token_matches(query.token.as_deref().unwrap_or_default(), &state.token) {
        return (StatusCode::UNAUTHORIZED, "bad token").into_response();
    }
    if body.is_empty() {
        return (StatusCode::BAD_REQUEST, "an attachment cannot be empty").into_response();
    }
    if body.len() as u64 > MAX_UPLOAD_BYTES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "This file is {}, which is over the {} this engine accepts for one attachment.",
                human_bytes(body.len() as u64),
                human_bytes(MAX_UPLOAD_BYTES)
            ),
        )
            .into_response();
    }
    let name = headers
        .get("x-rchat-filename")
        .and_then(|value| value.to_str().ok())
        .map(percent_decode)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "attachment".to_string());
    let declared = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    let blobs = state.blobs.clone();
    let name_for_task = name.clone();
    let declared_for_task = declared.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        ingest_bytes(&blobs, &body, &name_for_task, declared_for_task.as_deref())
    })
    .await;
    let ingested = match outcome {
        Ok(Ok(ingested)) => ingested,
        Ok(Err(err)) => {
            return (StatusCode::BAD_REQUEST, format!("{err:#}")).into_response()
        }
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("the attachment ingest task failed: {err}"),
            )
                .into_response()
        }
    };

    let descriptor = ingested.descriptor();
    if let Err(err) = state.store.insert_upload(&descriptor) {
        warn!(error = %err, "could not record an upload");
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("{err:#}")).into_response();
    }
    state.enforce_attachment_budget();
    Json(json!({ "attachment": descriptor })).into_response()
}

/// Serve the original, or its thumbnail on request. Both are immutable, and an
/// unknown hash is indistinguishable from an evicted one.
async fn download_handler(
    State(state): State<Arc<AppState>>,
    Path(hash): Path<String>,
    Query(query): Query<DownloadQuery>,
) -> Response {
    if !token_matches(query.token.as_deref().unwrap_or_default(), &state.token) {
        return (StatusCode::UNAUTHORIZED, "bad token").into_response();
    }
    if !rchat_core::attachments::is_hash(&hash) {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    let row = match state.store.attachment_by_hash(&hash) {
        Ok(Some(row)) => Some(row),
        Ok(None) => None,
        Err(err) => {
            warn!(error = %err, "could not look up an attachment");
            return (StatusCode::INTERNAL_SERVER_ERROR, "lookup failed").into_response();
        }
    };
    let wants_thumbnail = query.variant.as_deref() == Some("thumb");
    let served = match row {
        Some(row) if wants_thumbnail => match row.thumbnail.as_deref() {
            Some(thumbnail) if state.blobs.contains_thumbnail(thumbnail) => Some((
                state.blobs.thumbnail_path(thumbnail),
                "image/jpeg".to_string(),
            )),
            // No thumbnail: the original is still the same picture.
            _ => original_path(&state, &row).map(|path| (path, row.mime.clone())),
        },
        Some(row) => original_path(&state, &row).map(|path| (path, row.mime.clone())),
        // A thumbnail is addressed by its own hash, so the descriptor's
        // thumbnail field works directly as an image URL.
        None => match state.store.attachment_by_thumbnail(&hash) {
            Ok(Some(_)) if state.blobs.contains_thumbnail(&hash) => {
                Some((state.blobs.thumbnail_path(&hash), "image/jpeg".to_string()))
            }
            Ok(_) => None,
            Err(err) => {
                warn!(error = %err, "could not look up a thumbnail");
                return (StatusCode::INTERNAL_SERVER_ERROR, "lookup failed").into_response();
            }
        },
    };
    let (path, content_type) = match served {
        Some(served) => served,
        None => return (StatusCode::NOT_FOUND, "not found").into_response(),
    };
    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            let mut headers = HeaderMap::new();
            if let Ok(value) = HeaderValue::from_str(&content_type) {
                headers.insert(header::CONTENT_TYPE, value);
            }
            headers.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=31536000, immutable"),
            );
            (headers, bytes).into_response()
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            (StatusCode::NOT_FOUND, "not found").into_response()
        }
        Err(err) => {
            warn!(error = %err, "could not read an attachment");
            (StatusCode::INTERNAL_SERVER_ERROR, "read failed").into_response()
        }
    }
}

fn original_path(state: &AppState, row: &AttachmentInfo) -> Option<std::path::PathBuf> {
    if !row.has_bytes() || !state.blobs.contains(&row.hash) {
        return None;
    }
    Some(state.blobs.path(&row.hash))
}

/// Percent-decode a UTF-8 header value, because HTTP headers are not a place
/// for arbitrary filenames and a browser will refuse to send them raw.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let high = (bytes[index + 1] as char).to_digit(16);
            let low = (bytes[index + 2] as char).to_digit(16);
            if let (Some(high), Some(low)) = (high, low) {
                out.push((high * 16 + low) as u8);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

async fn health(State(state): State<Arc<AppState>>) -> Json<Value> {
    let transport = *state.transport.read().await;
    let address = state
        .store
        .identity()
        .ok()
        .flatten()
        .and_then(|identity| identity.address);
    Json(json!({
        "ok": true,
        "version": VERSION,
        "protocol": PROTOCOL_VERSION,
        "transport": transport,
        "address": address,
    }))
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Query(query): Query<AuthQuery>,
) -> Response {
    if !token_matches(query.token.as_deref().unwrap_or_default(), &state.token) {
        return (axum::http::StatusCode::UNAUTHORIZED, "bad token").into_response();
    }
    ws.on_upgrade(move |socket| client_loop(socket, state))
}

async fn client_loop(socket: WebSocket, state: Arc<AppState>) {
    let (mut sink, mut stream) = socket.split();
    let mut events = state.tx.subscribe();

    let hello = match state.identity_info() {
        Ok(identity) => ServerMsg::Hello {
            protocol: PROTOCOL_VERSION,
            version: VERSION.to_string(),
            identity,
            transport: *state.transport.read().await,
        },
        Err(err) => {
            warn!(error = %err, "client connected but identity is missing");
            let _ = sink.send(Message::Close(None)).await;
            return;
        }
    };
    if send_json(&mut sink, &hello).await.is_err() {
        return;
    }

    loop {
        tokio::select! {
            event = events.recv() => {
                match event {
                    Ok(text) => {
                        if sink.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    // A slow client missed events. It should resync; do not kill it.
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        let parsed: Result<Command, _> = serde_json::from_str(text.as_str());
                        let reply = match parsed {
                            Ok(command) => {
                                let id = command.id;
                                match state.handle_command(command.cmd).await {
                                    Ok(data) => ServerMsg::Reply { id, ok: true, data: Some(data), error: None },
                                    Err(err) => ServerMsg::Reply { id, ok: false, data: None, error: Some(err.to_string()) },
                                }
                            }
                            Err(err) => ServerMsg::Reply {
                                id: 0,
                                ok: false,
                                data: None,
                                error: Some(format!("invalid command: {err}")),
                            },
                        };
                        if send_json(&mut sink, &reply).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Binary(_))) => {
                        // Reserved: future media frames ride this channel.
                        let reply = ServerMsg::Reply {
                            id: 0,
                            ok: false,
                            data: None,
                            error: Some("binary frames are reserved for media and not yet accepted".into()),
                        };
                        if send_json(&mut sink, &reply).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
        }
    }
}

async fn send_json<S>(sink: &mut S, msg: &ServerMsg) -> Result<()>
where
    S: SinkExt<Message> + Unpin,
    <S as futures_util::Sink<Message>>::Error: std::fmt::Debug,
{
    let text = serde_json::to_string(msg)?;
    sink.send(Message::Text(text.into()))
        .await
        .map_err(|err| anyhow!("websocket send failed: {err:?}"))
}

fn token_matches(provided: &str, expected: &str) -> bool {
    let a = provided.as_bytes();
    let b = expected.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Coarse link class derived from the interface bitrate of the peer's path.
fn bandwidth_class(bitrate: Option<u64>) -> &'static str {
    match bitrate {
        Some(rate) if rate >= 1_000_000 => "lan",
        Some(rate) if rate >= 10_000 => "relay",
        Some(_) => "radio",
        None => "unknown",
    }
}

/// A message's kind follows what it carries; a caption does not make it text.
fn message_kind(attachments: &[AttachmentInfo]) -> &'static str {
    if attachments.is_empty() {
        "text"
    } else if attachments.iter().all(AttachmentInfo::is_image) {
        "image"
    } else {
        "file"
    }
}

/// A size a person can read in an error message.
pub fn human_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let value = bytes as f64;
    if value >= KB * KB {
        format!("{:.1} MB", value / (KB * KB))
    } else if value >= KB {
        format!("{:.0} KB", value / KB)
    } else {
        format!("{bytes} B")
    }
}

fn validate_interface_name(name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!("give the interface a name");
    }
    if name.chars().count() > 64 {
        bail!("interface names are limited to 64 characters");
    }
    if name.contains('[') || name.contains(']') || name.contains('\n') || name.contains('\r') {
        bail!("interface names cannot contain [ ], or line breaks");
    }
    Ok(())
}

/// Refuse the shapes Reticulum would only reject later, while the form still
/// has the user's text. The bridge stays authoritative for everything else.
fn validate_interface_config(kind: &str, config: &Value) -> Result<()> {
    let Some(map) = config.as_object() else {
        bail!("interface settings must be a JSON object");
    };
    for (key, value) in map {
        if key.ends_with("_port") {
            // `port` itself is a device path on radio types, so it is not a
            // network port. The form writes network ports as numbers.
            let Some(port) = value.as_u64() else {
                bail!("{key} must be a whole number");
            };
            if !(1..=65535).contains(&port) {
                bail!("{key} must be between 1 and 65535");
            }
        }
    }
    for key in ["target_host", "listen_ip", "forward_ip"] {
        if let Some(value) = map.get(key) {
            let Some(host) = value.as_str() else {
                bail!("{key} must be text");
            };
            validate_host(key, host)?;
        }
    }
    if matches!(
        kind,
        "TCPClientInterface" | "BackboneClientInterface" | "WeaveInterface"
    ) && map
        .get("target_host")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .is_none()
    {
        bail!("a host is required");
    }
    Ok(())
}

fn validate_host(key: &str, host: &str) -> Result<()> {
    let host = host.trim();
    if host.is_empty() {
        bail!("{key} cannot be empty");
    }
    if host.len() > 253 {
        bail!("{key} is too long");
    }
    if host.chars().any(char::is_whitespace) || host.contains('/') || host.contains("://") {
        bail!("{key} must be a host name or IP address, not \"{host}\"");
    }
    Ok(())
}

/// What clients are told about the mesh. `ready` means at least one interface
/// is online; a bridge with nothing online is `degraded`, not a success.
fn derived_transport(cache_transport: &str, interfaces: &[MeshInterface]) -> TransportState {
    match cache_transport {
        "" | "offline" => TransportState::Offline,
        "starting" => TransportState::Starting,
        _ => {
            if interfaces.iter().any(|interface| interface.online) {
                TransportState::Ready
            } else {
                TransportState::Degraded
            }
        }
    }
}

/// The sentence that goes with a derived transport word, when the bridge has
/// none of its own.
fn derived_detail(state: TransportState) -> Option<String> {
    match state {
        TransportState::Degraded => Some(
            "The bridge is up but no interface is online, so nothing can be sent or received"
                .to_string(),
        ),
        TransportState::Offline => Some("The Reticulum bridge is not running".to_string()),
        _ => None,
    }
}

fn check_config(config: &Value) -> Result<()> {
    if !config.is_object() {
        bail!("interface configuration must be a JSON object");
    }
    Ok(())
}

/// Why the page cannot edit interfaces while the bridge is down.
const BRIDGE_OFFLINE_REASON: &str =
    "The Reticulum bridge is offline, so interface changes wait until it reconnects.";

/// Pick the reason out of a `describe` reply: a shared instance refuses the
/// whole catalogue, setup otherwise refuses individual types.
fn read_only_from_descriptor(descriptor: &Value, kind: Option<&str>) -> Option<String> {
    if let Some(reason) = descriptor.get("read_only").and_then(Value::as_str) {
        return Some(reason.to_string());
    }
    let kind = kind?;
    descriptor
        .get("types")
        .and_then(Value::as_array)?
        .iter()
        .find(|entry| entry.get("kind").and_then(Value::as_str) == Some(kind))
        .and_then(|entry| entry.get("read_only"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchat_ipc::{Caps, PathEntry};

    fn peer(address: &str, class: &str) -> PeerInfo {
        PeerInfo {
            id: address.to_string(),
            address: Some(address.to_string()),
            nickname: Some("Alice".into()),
            announced_name: Some("Alice".into()),
            last_seen: Some(1),
            bandwidth_class: class.into(),
            caps: Caps::default(),
        }
    }

    fn app() -> Arc<AppState> {
        let store = Arc::new(Store::open_in_memory().unwrap());
        store.ensure_identity().unwrap();
        let blobs = Arc::new(BlobStore::new(std::env::temp_dir().join(format!(
            "rchat-app-blobs-{}-{}",
            std::process::id(),
            now_ms()
        ))));
        Arc::new(AppState::new(store, "token".into(), blobs))
    }

    async fn feed_mesh(state: &Arc<AppState>, interfaces: Value) {
        state
            .on_bridge_event(BridgeEvent::Mesh {
                mesh: json!({
                    "transport": "ready",
                    "address": "ab".repeat(16),
                    "interfaces": interfaces,
                    "counters": {},
                    "paths": [],
                    "other_services": [],
                }),
            })
            .await;
    }

    fn transport_events(state: &AppState) -> Vec<Value> {
        state
            .store
            .events_since(0)
            .unwrap()
            .into_iter()
            .filter(|event| event.name == "transport.state")
            .map(|event| event.data)
            .collect()
    }

    #[tokio::test]
    async fn transport_is_degraded_until_an_interface_is_online() {
        let state = app();
        feed_mesh(&state, json!([])).await;
        assert_eq!(
            state.mesh_status().await.unwrap().transport,
            "degraded",
            "a bridge with no interfaces cannot carry a message"
        );
        assert_eq!(*state.transport.read().await, TransportState::Degraded);

        feed_mesh(
            &state,
            json!([{ "name": "Relay", "kind": "TCPClientInterface",
                     "online": false, "down_since": 1_760_000_000_000i64 }]),
        )
        .await;
        assert_eq!(state.mesh_status().await.unwrap().transport, "degraded");

        feed_mesh(
            &state,
            json!([{ "name": "Relay", "kind": "TCPClientInterface", "online": true }]),
        )
        .await;
        assert_eq!(state.mesh_status().await.unwrap().transport, "ready");
        assert_eq!(*state.transport.read().await, TransportState::Ready);

        // Clients were told, not left to infer it from the interface list.
        let latest = transport_events(&state).pop().expect("a transport event");
        assert_eq!(latest["transport"], "ready");
        let snapshot = state
            .handle_command(ClientCmd::Snapshot)
            .await
            .unwrap();
        assert_eq!(snapshot["transport"], "ready");
        assert_eq!(snapshot["mesh"]["transport"], "ready");
    }

    #[tokio::test]
    async fn an_interface_coming_back_sends_the_queued_messages() {
        let state = app();
        feed_mesh(&state, json!([])).await;
        assert_eq!(*state.transport.read().await, TransportState::Degraded);

        let address = "aa".repeat(16);
        state.store.upsert_peer(&peer(&address, "relay")).unwrap();
        let conversation = state
            .store
            .create_conversation("direct", None, &[address.clone()])
            .unwrap();
        let identity = state.store.identity().unwrap().unwrap();
        let message = MessageInfo {
            id: "queued-1".into(),
            conversation_id: conversation.id.clone(),
            author_id: identity.id,
            ts: 1,
            body: "queued while degraded".into(),
            kind: "text".into(),
            reply_to: None,
            state: "queued".into(),
            route: None,
            edited_at: None,
            attachments: vec![],
        };
        state.store.insert_message(&message, false).unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel();
        state.attach_transport(tx).await;

        feed_mesh(
            &state,
            json!([{ "name": "Relay", "kind": "TCPClientInterface", "online": true }]),
        )
        .await;

        let sent = timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("the queue flushes as soon as an interface is online")
            .expect("a bridge command");
        let payload: Value = serde_json::from_str(&sent).unwrap();
        assert_eq!(payload["cmd"], "send");
        assert_eq!(payload["to"], address);
        assert_eq!(payload["body"], "queued while degraded");
        assert_eq!(
            state.store.get_message("queued-1").unwrap().unwrap().state,
            "sending"
        );
    }

    #[tokio::test]
    async fn a_restarted_bridge_does_not_claim_ready_from_a_stale_sample() {
        let state = app();
        feed_mesh(
            &state,
            json!([{ "name": "Relay", "kind": "TCPClientInterface", "online": true }]),
        )
        .await;
        assert_eq!(*state.transport.read().await, TransportState::Ready);

        state
            .on_bridge_event(BridgeEvent::State {
                state: TransportState::Starting,
                address: None,
                name: None,
                detail: None,
            })
            .await;
        assert_eq!(*state.transport.read().await, TransportState::Starting);

        // Nothing has sampled the new bridge session yet. The old sample's
        // online interface must not make this "ready" again.
        state
            .on_bridge_event(BridgeEvent::State {
                state: TransportState::Ready,
                address: None,
                name: None,
                detail: None,
            })
            .await;
        assert_eq!(*state.transport.read().await, TransportState::Degraded);

        feed_mesh(
            &state,
            json!([{ "name": "Relay", "kind": "TCPClientInterface", "online": true }]),
        )
        .await;
        assert_eq!(*state.transport.read().await, TransportState::Ready);
    }

    #[tokio::test]
    async fn every_known_peer_reports_whether_a_path_exists() {
        let state = app();
        let alice = "aa".repeat(16);
        let bob = "bb".repeat(16);
        state.store.upsert_peer(&peer(&alice, "relay")).unwrap();
        let mut quiet = peer(&bob, "unknown");
        // Recent contact, but the last actual announce is much older.
        quiet.last_seen = Some(1_760_000_000_000);
        state.store.upsert_peer(&quiet).unwrap();
        state
            .store
            .record_announce(
                &bob,
                Some("Bob"),
                None,
                Some("Relay"),
                Some(1),
                1_700_000_000_000,
            )
            .unwrap();

        state
            .on_bridge_event(BridgeEvent::Mesh {
                mesh: json!({
                    "transport": "ready",
                    "interfaces": [],
                    "counters": {},
                    "paths": [{
                        "destination": alice,
                        "hops": 2,
                        "next_hop": "cc".repeat(16),
                        "interface": "Relay",
                        "bitrate": 62_500,
                        "state": "responsive",
                        "age": 1.0,
                    }],
                    "other_services": [],
                }),
            })
            .await;

        let status = state.mesh_status().await.unwrap();
        let reachable = status
            .peer_reachability
            .iter()
            .find(|row| row.address == alice)
            .expect("a reachable peer");
        assert!(reachable.path);
        assert_eq!(reachable.hops, Some(2));
        assert_eq!(reachable.interface.as_deref(), Some("Relay"));
        assert_eq!(reachable.state, "responsive");
        assert_eq!(
            reachable.last_announce, None,
            "a peer added by hand has no announce history"
        );

        let unreachable = status
            .peer_reachability
            .iter()
            .find(|row| row.address == bob)
            .expect("a known peer without a path is still reported");
        assert!(!unreachable.path);
        assert_eq!(unreachable.hops, None);
        assert_eq!(unreachable.state, "none");
        assert_eq!(
            unreachable.last_announce,
            Some(1_700_000_000_000),
            "the announce history answers this, not the peer's last contact"
        );
    }

    #[test]
    fn interface_validation_checks_host_and_port_before_applying() {
        assert!(validate_interface_config(
            "TCPClientInterface",
            &json!({"target_host": "relay.example", "target_port": 4242})
        )
        .is_ok());
        assert!(validate_interface_config(
            "TCPClientInterface",
            &json!({"target_host": "fd00::1", "target_port": 4242})
        )
        .is_ok());

        for bad in [
            json!({"target_host": "", "target_port": 4242}),
            json!({"target_host": "relay.example", "target_port": 0}),
            json!({"target_host": "relay.example", "target_port": 70_000}),
            json!({"target_host": "relay.example", "target_port": "4242"}),
            json!({"target_host": "relay example", "target_port": 4242}),
            json!({"target_host": "tcp://relay.example", "target_port": 4242}),
            json!({"target_port": 4242}),
        ] {
            assert!(
                validate_interface_config("TCPClientInterface", &bad).is_err(),
                "should be refused: {bad}"
            );
        }

        assert!(validate_interface_config(
            "TCPServerInterface",
            &json!({"listen_ip": "0.0.0.0", "listen_port": 4242})
        )
        .is_ok());
        assert!(validate_interface_config("TCPServerInterface", &json!({"listen_port": 0})).is_err());
        // Radio and local types have no address of their own to check.
        assert!(validate_interface_config(
            "RNodeInterface",
            &json!({"port": "/dev/ttyUSB0", "frequency": 868_000_000})
        )
        .is_ok());
        assert!(validate_interface_config("AutoInterface", &json!({"group_id": "reticulum"})).is_ok());
    }

    #[tokio::test]
    async fn a_refused_edit_keeps_the_stored_config_and_the_users_input() {
        let state = app();
        let row = state
            .store
            .create_interface(
                "Relay",
                "TCPClientInterface",
                &json!({"target_host": "relay.example", "target_port": 4242}),
                true,
                "managed",
            )
            .unwrap();
        let error = state
            .handle_command(ClientCmd::UpdateInterface {
                interface_id: row.id.clone(),
                config: json!({"target_host": "relay.example", "target_port": 70_000}),
            })
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("port"),
            "the refusal names the field: {error}"
        );
        let stored = state.store.interface(&row.id).unwrap().unwrap();
        assert_eq!(stored.config["target_port"], 4242, "nothing was written");
    }

    #[tokio::test]
    async fn mesh_status_only_carries_known_peers() {        let state = app();
        let alice = "aa".repeat(16);
        let stranger = "bb".repeat(16);
        state.store.upsert_peer(&peer(&alice, "unknown")).unwrap();
        {
            let mut cache = state.mesh.write().await;
            cache.transport = "ready".into();
            cache.path_total = 2;
            cache.paths = vec![
                PathEntry {
                    destination: alice.clone(),
                    name: None,
                    hops: 2,
                    next_hop: "cc".repeat(16),
                    interface: "Relay".into(),
                    bitrate: Some(62_500),
                    state: "responsive".into(),
                    age: 3.0,
                    aspect: Some("lxmf.delivery".into()),
                },
                PathEntry {
                    destination: stranger.clone(),
                    name: None,
                    hops: 1,
                    next_hop: stranger.clone(),
                    interface: "LAN".into(),
                    bitrate: Some(10_000_000),
                    state: "responsive".into(),
                    age: 1.0,
                    aspect: None,
                },
            ];
            cache.counters.path_unresponsive = 0;
            cache.interfaces = vec![serde_json::from_value::<MeshInterface>(json!({
                "name": "LAN", "kind": "AutoInterface", "online": true
            }))
            .unwrap()];
        }
        let status = state.mesh_status().await.unwrap();
        assert_eq!(status.transport, "ready");
        assert_eq!(status.peer_paths.len(), 1);
        assert_eq!(status.peer_paths[0].address, alice);
        assert_eq!(status.peer_paths[0].hops, 2);
        assert_eq!(status.counters.path_count, 2);
    }

    #[tokio::test]
    async fn bandwidth_class_follows_the_path_bitrate() {
        let state = app();
        let alice = "aa".repeat(16);
        state.store.upsert_peer(&peer(&alice, "unknown")).unwrap();
        {
            let mut cache = state.mesh.write().await;
            cache.paths = vec![PathEntry {
                destination: alice.clone(),
                name: None,
                hops: 1,
                next_hop: alice.clone(),
                interface: "LAN".into(),
                bitrate: Some(10_000_000),
                state: "responsive".into(),
                age: 0.0,
                aspect: None,
            }];
        }
        state.refresh_bandwidth_classes().await;
        assert_eq!(
            state.store.peer(&alice).unwrap().unwrap().bandwidth_class,
            "lan"
        );

        {
            let mut cache = state.mesh.write().await;
            cache.paths[0].bitrate = Some(62_500);
        }
        state.refresh_bandwidth_classes().await;
        assert_eq!(
            state.store.peer(&alice).unwrap().unwrap().bandwidth_class,
            "relay"
        );
        assert_eq!(bandwidth_class(None), "unknown");
        assert_eq!(bandwidth_class(Some(1200)), "radio");

        // A path that goes away stops classifying the peer.
        {
            let mut cache = state.mesh.write().await;
            cache.paths.clear();
        }
        state.refresh_bandwidth_classes().await;
        assert_eq!(
            state.store.peer(&alice).unwrap().unwrap().bandwidth_class,
            "unknown"
        );
    }

    #[tokio::test]
    async fn mesh_paths_filter_and_page() {
        let state = app();
        let alice = "aa".repeat(16);
        state.store.upsert_peer(&peer(&alice, "lan")).unwrap();
        {
            let mut cache = state.mesh.write().await;
            cache.paths = (0..150)
                .map(|i| PathEntry {
                    destination: if i == 0 {
                        alice.clone()
                    } else {
                        format!("{:032x}", i)
                    },
                    name: None,
                    hops: if i == 0 { 4 } else { 1 },
                    next_hop: "00".repeat(16),
                    interface: if i == 0 { "Relay".into() } else { "LAN".into() },
                    bitrate: None,
                    state: "responsive".into(),
                    age: i as f64,
                    aspect: None,
                })
                .collect();
        }
        let page = state.mesh_paths(None, Some(0), Some(100)).await.unwrap();
        assert_eq!(page["total"], 150);
        assert_eq!(page["paths"].as_array().unwrap().len(), 100);
        let next = state.mesh_paths(None, Some(100), Some(100)).await.unwrap();
        assert_eq!(next["paths"].as_array().unwrap().len(), 50);

        let by_name = state.mesh_paths(Some("alice".into()), None, None).await.unwrap();
        assert_eq!(by_name["total"], 1);
        assert_eq!(by_name["paths"][0]["destination"], alice);
        assert_eq!(by_name["paths"][0]["name"], "Alice");

        let by_prefix = state
            .mesh_paths(Some(alice[..6].to_string()), None, None)
            .await
            .unwrap();
        assert_eq!(by_prefix["total"], 1);

        let by_interface = state.mesh_paths(Some("relay".into()), None, None).await.unwrap();
        assert_eq!(by_interface["total"], 1);
    }

    #[test]
    fn interface_name_validation() {
        assert!(validate_interface_name("Relay").is_ok());
        assert!(validate_interface_name("  ").is_err());
        assert!(validate_interface_name("[bad]").is_err());
        assert!(validate_interface_name(&"x".repeat(65)).is_err());
        assert!(check_config(&json!({"a": 1})).is_ok());
        assert!(check_config(&json!([1])).is_err());
    }

    #[test]
    fn read_only_reasons_come_from_the_bridge_descriptor() {
        // A shared instance owns the whole catalogue.
        let shared = json!({
            "types": [{ "kind": "TCPClientInterface", "read_only": "not from here" }],
            "read_only": "attached to a shared instance",
        });
        assert_eq!(
            read_only_from_descriptor(&shared, Some("TCPClientInterface")).as_deref(),
            Some("attached to a shared instance")
        );
        assert_eq!(
            read_only_from_descriptor(&shared, None).as_deref(),
            Some("attached to a shared instance")
        );

        // Otherwise only the refused type is marked.
        let embedded = json!({
            "types": [
                { "kind": "TCPClientInterface" },
                { "kind": "I2PInterface", "read_only": "cannot detach" },
            ],
        });
        assert_eq!(read_only_from_descriptor(&embedded, Some("TCPClientInterface")), None);
        assert_eq!(
            read_only_from_descriptor(&embedded, Some("I2PInterface")).as_deref(),
            Some("cannot detach")
        );
        assert_eq!(read_only_from_descriptor(&embedded, Some("Unknown")), None);
        assert_eq!(read_only_from_descriptor(&embedded, None), None);
    }
}
