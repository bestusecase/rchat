//! Daemon wiring: HTTP/WebSocket surface, command handling, and the bridge
//! event pump.
//!
//! The daemon is the product; UIs are views. Everything here is independent of
//! whether the client is a Tauri window, a browser tab, or a test script.

use anyhow::{anyhow, bail, Context, Result};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use rchat_core::now_ms;
use rchat_core::store::Store;
use rchat_ipc::{
    ClientCmd, Command, IdentityInfo, MessageInfo, PeerInfo, ServerMsg, Snapshot,
    TransportState, PROTOCOL_VERSION,
};
use rchat_rns::BridgeEvent;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, RwLock};
use tower_http::cors::CorsLayer;
use tracing::{debug, warn};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct AppState {
    pub store: Arc<Store>,
    pub token: String,
    pub tx: broadcast::Sender<String>,
    pub rns: RwLock<Option<mpsc::UnboundedSender<String>>>,
    pub transport: RwLock<TransportState>,
}

impl AppState {
    pub fn new(store: Arc<Store>, token: String) -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self {
            store,
            token,
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
                Ok(json!({ "conversation_id": conversation_id, "messages": messages }))
            }
            ClientCmd::SendMessage {
                conversation_id,
                body,
                reply_to,
            } => self.send_message(&conversation_id, &body, reply_to).await,
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
                if name.chars().count() > 48 {
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
    ) -> Result<Value> {
        let body = body.trim();
        if body.is_empty() {
            bail!("cannot send an empty message");
        }
        if body.chars().count() > 12_000 {
            bail!("message is too long for one Reticulum message");
        }
        let identity = self.identity_info()?;
        let conv = self.store.get_conversation(conversation_id)?;
        let msg = MessageInfo {
            id: uuid::Uuid::new_v4().to_string(),
            conversation_id: conv.id.clone(),
            author_id: identity.id.clone(),
            ts: now_ms(),
            body: body.to_string(),
            kind: "text".into(),
            reply_to,
            state: "queued".into(),
            route: None,
            edited_at: None,
        };
        self.store.insert_message(&msg, false)?;
        self.publish("message.new", json!({ "message": msg }))?;
        self.publish(
            "conversation.updated",
            json!({ "conversation": self.store.get_conversation(&conv.id)? }),
        )?;

        self.dispatch(&msg).await?;

        let stored = self.store.get_message(&msg.id)?.unwrap_or(msg);
        Ok(json!({ "message": stored }))
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
        let mut targets = 0usize;
        for peer_id in &conv.peer_ids {
            let Some(peer) = self.store.peer(peer_id)? else {
                continue;
            };
            let Some(address) = peer.address.as_deref() else {
                continue;
            };
            let payload = json!({
                "cmd": "send",
                "id": msg.id,
                "to": address,
                "body": msg.body,
                "title": "",
            });
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

    pub async fn on_bridge_event(&self, event: BridgeEvent) {
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
            } => {
                if let Err(err) = self.on_incoming(id, from, body, ts).await {
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

    async fn on_incoming(&self, remote_id: String, from: String, body: String, ts: i64) -> Result<()> {
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
            return Ok(());
        }
        // Keep our own clock authoritative for ordering.
        let ts = ts.max(1);
        let msg = MessageInfo {
            id,
            conversation_id: conv.id.clone(),
            author_id: peer.id.clone(),
            ts,
            body,
            kind: "text".into(),
            reply_to: None,
            state: "delivered".into(),
            route: Some("reticulum".into()),
            edited_at: None,
        };
        self.store.insert_message(&msg, true)?;
        self.publish("message.new", json!({ "message": msg }))?;
        self.publish(
            "conversation.updated",
            json!({ "conversation": self.store.get_conversation(&conv.id)? }),
        )?;
        Ok(())
    }
}

#[derive(Deserialize)]
struct AuthQuery {
    token: Option<String>,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/ws", get(ws_handler))
        .layer(CorsLayer::permissive())
        .with_state(state)
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
