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
    AttachmentInfo, ClientCmd, Command, IdentityInfo, MessageInfo, PeerInfo, ServerMsg, Snapshot,
    TransportState, PROTOCOL_VERSION,
};
use rchat_rns::{BridgeAttachment, BridgeEvent};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path as FilePath;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, RwLock};
use tower_http::cors::CorsLayer;
use tracing::{debug, warn};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

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

pub struct AppState {
    pub store: Arc<Store>,
    pub token: String,
    /// Where attachment bytes live. Owned by the daemon, never derived here.
    pub blobs: Arc<BlobStore>,
    pub tx: broadcast::Sender<String>,
    pub rns: RwLock<Option<mpsc::UnboundedSender<String>>>,
    pub transport: RwLock<TransportState>,
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
                *self.transport.write().await = state;
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
                    json!({ "transport": state, "address": address, "detail": detail }),
                );
                if state == TransportState::Ready {
                    self.flush_queued().await;
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
            BridgeEvent::Announce { address, name } => {
                if address.len() != 32 {
                    return;
                }
                // Ignore our own announce echoed back through the mesh.
                if let Ok(Some(me)) = self.store.identity() {
                    if me.address.as_deref() == Some(address.as_str()) {
                        return;
                    }
                }
                let peer = PeerInfo {
                    id: address.clone(),
                    address: Some(address.clone()),
                    nickname: None,
                    announced_name: name,
                    last_seen: Some(now_ms()),
                    bandwidth_class: "unknown".into(),
                    caps: Default::default(),
                };
                if let Err(err) = self.store.upsert_peer(&peer) {
                    warn!(error = %err, "could not store announced peer");
                    return;
                }
                let _ = self.publish("peer.updated", json!({ "peer": peer }));
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
