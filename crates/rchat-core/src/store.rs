//! SQLite persistence.
//!
//! One connection guarded by a mutex is plenty for a single-user desktop app:
//! all writes are small and the daemon is the only writer. Everything the UI
//! shows is durable here, including a monotonic event log so a reconnecting
//! client can replay instead of refetching.

use anyhow::{Context, Result};
use rchat_ipc::{
    AnnounceSighting, AttachmentInfo, Caps, ConversationInfo, InterfaceRow, MessageInfo, PeerInfo,
};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::Mutex;

/// The network settings an older rchat kept outside the interface table. Read
/// once on upgrade and turned into rows with origin `legacy`.
#[derive(Debug, Clone, Default)]
pub struct LegacyNetwork {
    pub auto_discovery: Option<bool>,
    pub tcp_relay: Option<String>,
}

/// Meta key guarding the one-shot first-run interface setup. Distinct from
/// `mesh_config_imported`, which guards reading `rns/config`.
pub const NETWORK_IMPORTED: &str = "mesh_network_imported";

/// Name given to the discovery row every install starts with.
pub const DEFAULT_DISCOVERY_NAME: &str = "AutoInterface";
/// Name given to a relay carried over from the old settings field.
pub const LEGACY_RELAY_NAME: &str = "Relay";

/// Split an old relay string into host and port, preferring the string the user
/// gave so a DNS name stays preferred over the address it resolves to.
///
/// Accepts `host:port`, `[v6addr]:port`, and bare `host` only when the port is
/// missing from the string entirely, which is an error: the old field always
/// required a port, and guessing 4242 would hide a typo.
pub fn parse_relay(spec: &str) -> Result<(String, u16), String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("the relay address is empty".into());
    }
    if spec.contains("://") || spec.contains('/') {
        return Err(format!("\"{spec}\" is not a host and port"));
    }
    let (host, port) = if let Some(rest) = spec.strip_prefix('[') {
        let (host, tail) = rest
            .split_once(']')
            .ok_or_else(|| format!("\"{spec}\" has an unclosed address bracket"))?;
        let port = tail
            .strip_prefix(':')
            .ok_or_else(|| format!("\"{spec}\" has no port"))?;
        (host, port)
    } else {
        spec.rsplit_once(':')
            .ok_or_else(|| format!("\"{spec}\" has no port; write it as host:port"))?
    };
    let host = host.trim();
    if host.is_empty() || host.chars().any(char::is_whitespace) {
        return Err(format!("\"{spec}\" has no host name"));
    }
    let port: u16 = port
        .trim()
        .parse()
        .map_err(|_| format!("\"{spec}\" has a port that is not a number"))?;
    if port == 0 {
        return Err(format!("\"{spec}\" has port 0, which cannot be dialled"));
    }
    Ok((host.to_string(), port))
}

pub struct Store {
    conn: Mutex<Connection>,
    /// When announce pruning last ran, to keep it off the hot path.
    last_announce_prune: Mutex<Option<i64>>,
}

#[derive(Debug, Clone)]
pub struct IdentityRow {
    pub id: String,
    pub secret: Vec<u8>,
    pub display_name: String,
    pub address: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct EventRow {
    pub seq: i64,
    pub name: String,
    pub data: serde_json::Value,
    pub ts: i64,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening database at {}", path.display()))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=5000;",
        )?;
        let store = Self {
            conn: Mutex::new(conn),
            last_announce_prune: Mutex::new(None),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        let store = Self {
            conn: Mutex::new(conn),
            last_announce_prune: Mutex::new(None),
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS identity (
                slot         TEXT PRIMARY KEY CHECK (slot = 'me'),
                id           TEXT NOT NULL,
                secret       BLOB NOT NULL,
                display_name TEXT NOT NULL,
                address      TEXT,
                created_at   INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS peers (
                id              TEXT PRIMARY KEY,
                address         TEXT UNIQUE,
                nickname        TEXT,
                announced_name  TEXT,
                last_seen       INTEGER,
                bandwidth_class TEXT NOT NULL DEFAULT 'unknown',
                caps            TEXT NOT NULL,
                created_at      INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS conversations (
                id              TEXT PRIMARY KEY,
                kind            TEXT NOT NULL,
                title           TEXT,
                created_at      INTEGER NOT NULL,
                last_message_at INTEGER,
                unread          INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS members (
                conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
                peer_id         TEXT NOT NULL,
                role            TEXT NOT NULL DEFAULT 'member',
                joined_at       INTEGER NOT NULL,
                PRIMARY KEY (conversation_id, peer_id)
            );

            CREATE TABLE IF NOT EXISTS messages (
                id              TEXT PRIMARY KEY,
                conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
                author_id       TEXT NOT NULL,
                ts              INTEGER NOT NULL,
                body            TEXT NOT NULL,
                kind            TEXT NOT NULL DEFAULT 'text',
                reply_to        TEXT,
                state           TEXT NOT NULL,
                route           TEXT,
                edited_at       INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_messages_conv_ts
                ON messages(conversation_id, ts);

            -- One row per attached file. `message_id` is null between an upload
            -- and the send that claims it. Blobs themselves live in the
            -- content-addressed store, keyed by `hash`; a row without bytes
            -- carries `reason` instead.
            CREATE TABLE IF NOT EXISTS attachments (
                message_id TEXT REFERENCES messages(id) ON DELETE CASCADE,
                ordinal    INTEGER NOT NULL DEFAULT 0,
                name       TEXT NOT NULL,
                mime       TEXT NOT NULL,
                size       INTEGER NOT NULL,
                hash       TEXT NOT NULL,
                width      INTEGER,
                height     INTEGER,
                thumbnail  TEXT,
                reason     TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_attachments_message
                ON attachments(message_id, ordinal);
            CREATE INDEX IF NOT EXISTS idx_attachments_hash
                ON attachments(hash);

            CREATE TABLE IF NOT EXISTS events (
                seq  INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                data TEXT NOT NULL,
                ts   INTEGER NOT NULL
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts
                USING fts5(body, content='messages', content_rowid='rowid');

            CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
                INSERT INTO messages_fts(rowid, body) VALUES (new.rowid, new.body);
            END;
            CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
                INSERT INTO messages_fts(messages_fts, rowid, body) VALUES ('delete', old.rowid, old.body);
            END;
            CREATE TRIGGER IF NOT EXISTS messages_au AFTER UPDATE ON messages BEGIN
                INSERT INTO messages_fts(messages_fts, rowid, body) VALUES ('delete', old.rowid, old.body);
                INSERT INTO messages_fts(rowid, body) VALUES (new.rowid, new.body);
            END;

            -- Interfaces rchat manages. `config` is opaque JSON to the engine
            -- and validated only by the bridge, next to RNS.
            CREATE TABLE IF NOT EXISTS mesh_interfaces (
                id         TEXT PRIMARY KEY,
                name       TEXT NOT NULL UNIQUE,
                kind       TEXT NOT NULL,
                config     TEXT NOT NULL,
                enabled    INTEGER NOT NULL DEFAULT 1,
                origin     TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            -- Sections of rns/config rchat does not understand, kept verbatim.
            CREATE TABLE IF NOT EXISTS mesh_config_extra (
                section TEXT PRIMARY KEY,
                raw     TEXT NOT NULL
            );

            -- Coalesced announce sightings, one row per address per 10 minutes.
            CREATE TABLE IF NOT EXISTS mesh_announces (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                address    TEXT NOT NULL,
                name       TEXT,
                aspect     TEXT,
                interface  TEXT,
                hops       INTEGER,
                bucket     INTEGER NOT NULL,
                first_seen INTEGER NOT NULL,
                last_seen  INTEGER NOT NULL,
                count      INTEGER NOT NULL DEFAULT 1,
                UNIQUE(address, bucket)
            );
            CREATE INDEX IF NOT EXISTS idx_mesh_announces_last ON mesh_announces(last_seen);
            "#,
        )?;
        // `CREATE TABLE IF NOT EXISTS` never alters a table that already
        // exists, so columns added after the first release need their own
        // guarded migration. Checking the column list makes it safe on every
        // startup.
        // Schema 5 adds the reason an imported row cannot be used as written.
        ensure_column(&conn, "mesh_interfaces", "note", "TEXT")?;
        conn.execute(
            "INSERT OR IGNORE INTO meta(key, value) VALUES ('schema', '5')",
            [],
        )?;
        // An install that predates attachments carries schema 1 or 2; schema 3
        // added attachments and the mesh tables; 5 adds interface import notes.
        conn.execute("UPDATE meta SET value = '5' WHERE key = 'schema'", [])?;
        Ok(())
    }

    // ---- identity ---------------------------------------------------------

    pub fn identity(&self) -> Result<Option<IdentityRow>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT id, secret, display_name, address FROM identity WHERE slot = 'me'",
                [],
                |r| {
                    Ok(IdentityRow {
                        id: r.get(0)?,
                        secret: r.get(1)?,
                        display_name: r.get(2)?,
                        address: r.get(3)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    pub fn ensure_identity(&self) -> Result<IdentityRow> {
        if let Some(existing) = self.identity()? {
            return Ok(existing);
        }
        let ident = crate::identity::generate();
        let row = IdentityRow {
            id: ident.id.clone(),
            secret: ident.secret.to_vec(),
            display_name: "you".to_string(),
            address: None,
        };
        {
            let conn = self.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO identity(slot, id, secret, display_name, address, created_at)
                 VALUES ('me', ?1, ?2, ?3, NULL, ?4)",
                params![row.id, row.secret, row.display_name, crate::now_ms()],
            )?;
        }
        Ok(row)
    }

    pub fn set_display_name(&self, name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE identity SET display_name = ?1 WHERE slot = 'me'", [name])?;
        conn.execute(
            "INSERT INTO meta(key, value) VALUES ('display_name_set', '1')
             ON CONFLICT(key) DO UPDATE SET value = '1'",
            [],
        )?;
        Ok(())
    }

    /// Whether the user has explicitly chosen a display name. Kept in `meta`
    /// rather than inferred from the name so a user who picks "you" is not
    /// asked again.
    pub fn display_name_set(&self) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let value: Option<String> = conn
            .query_row("SELECT value FROM meta WHERE key = 'display_name_set'", [], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(value.as_deref() == Some("1"))
    }

    pub fn set_address(&self, address: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE identity SET address = ?1 WHERE slot = 'me'",
            params![address],
        )?;
        Ok(())
    }

    // ---- peers ------------------------------------------------------------

    pub fn upsert_peer(&self, peer: &PeerInfo) -> Result<()> {
        let caps = serde_json::to_string(&peer.caps)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"INSERT INTO peers(id, address, nickname, announced_name, last_seen,
                                 bandwidth_class, caps, created_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
               ON CONFLICT(id) DO UPDATE SET
                 address         = coalesce(excluded.address, peers.address),
                 nickname        = coalesce(excluded.nickname, peers.nickname),
                 announced_name  = coalesce(excluded.announced_name, peers.announced_name),
                 last_seen       = coalesce(excluded.last_seen, peers.last_seen),
                 bandwidth_class = excluded.bandwidth_class,
                 caps            = excluded.caps"#,
            params![
                peer.id,
                peer.address,
                peer.nickname,
                peer.announced_name,
                peer.last_seen,
                peer.bandwidth_class,
                caps,
                crate::now_ms()
            ],
        )?;
        Ok(())
    }

    pub fn peer(&self, id: &str) -> Result<Option<PeerInfo>> {
        let conn = self.conn.lock().unwrap();
        let peer = conn
            .query_row(
                "SELECT id, address, nickname, announced_name, last_seen, bandwidth_class, caps
                 FROM peers WHERE id = ?1",
                [id],
                map_peer,
            )
            .optional()?;
        Ok(peer)
    }

    pub fn peer_by_address(&self, address: &str) -> Result<Option<PeerInfo>> {
        let conn = self.conn.lock().unwrap();
        let peer = conn
            .query_row(
                "SELECT id, address, nickname, announced_name, last_seen, bandwidth_class, caps
                 FROM peers WHERE address = ?1",
                [address],
                map_peer,
            )
            .optional()?;
        Ok(peer)
    }

    pub fn list_peers(&self) -> Result<Vec<PeerInfo>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, address, nickname, announced_name, last_seen, bandwidth_class, caps
             FROM peers ORDER BY coalesce(last_seen, created_at) DESC",
        )?;
        let rows = stmt.query_map([], map_peer)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Set the coarse link class derived from the path table. Returns whether
    /// anything changed, so callers only publish on a real transition.
    pub fn update_peer_bandwidth(&self, id: &str, bandwidth_class: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE peers SET bandwidth_class = ?2
             WHERE id = ?1 AND bandwidth_class != ?2",
            params![id, bandwidth_class],
        )?;
        Ok(changed > 0)
    }

    // ---- conversations ----------------------------------------------------

    pub fn open_direct(&self, peer_id: &str) -> Result<ConversationInfo> {
        {
            let conn = self.conn.lock().unwrap();
            let existing: Option<String> = conn
                .query_row(
                    "SELECT c.id FROM conversations c
                     JOIN members m ON m.conversation_id = c.id
                     WHERE c.kind = 'direct' AND m.peer_id = ?1
                     LIMIT 1",
                    [peer_id],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(id) = existing {
                drop(conn);
                return self.get_conversation(&id);
            }
        }
        self.create_conversation("direct", None, &[peer_id.to_string()])
    }

    pub fn create_conversation(
        &self,
        kind: &str,
        title: Option<&str>,
        peer_ids: &[String],
    ) -> Result<ConversationInfo> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = crate::now_ms();
        {
            let mut conn = self.conn.lock().unwrap();
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO conversations(id, kind, title, created_at, last_message_at, unread)
                 VALUES (?1, ?2, ?3, ?4, NULL, 0)",
                params![id, kind, title, now],
            )?;
            for peer_id in peer_ids {
                tx.execute(
                    "INSERT OR IGNORE INTO members(conversation_id, peer_id, role, joined_at)
                     VALUES (?1, ?2, 'member', ?3)",
                    params![id, peer_id, now],
                )?;
            }
            tx.commit()?;
        }
        self.get_conversation(&id)
    }

    pub fn get_conversation(&self, id: &str) -> Result<ConversationInfo> {
        let (kind, title, created_at, last_message_at, unread): (
            String,
            Option<String>,
            i64,
            Option<i64>,
            i64,
        ) = {
            let conn = self.conn.lock().unwrap();
            conn.query_row(
                "SELECT c.kind, c.title, c.created_at, c.last_message_at, c.unread
                 FROM conversations c WHERE c.id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?
            .with_context(|| format!("conversation {id} not found"))?
        };
        Ok(ConversationInfo {
            id: id.to_string(),
            kind,
            title,
            created_at,
            last_message_at,
            unread,
            preview: self.preview(id)?,
            peer_ids: self.members(id)?,
        })
    }

    pub fn list_conversations(&self) -> Result<Vec<ConversationInfo>> {
        let rows: Vec<(String, String, Option<String>, i64, Option<i64>, i64)> = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare(
                "SELECT c.id, c.kind, c.title, c.created_at, c.last_message_at, c.unread
                 FROM conversations c
                 ORDER BY coalesce(c.last_message_at, c.created_at) DESC",
            )?;
            let collected = stmt
                .query_map([], |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            collected
        };

        let mut out = Vec::with_capacity(rows.len());
        for (id, kind, title, created_at, last_message_at, unread) in rows {
            let peer_ids = self.members(&id)?;
            out.push(ConversationInfo {
                preview: self.preview(&id)?,
                id,
                kind,
                title,
                created_at,
                last_message_at,
                unread,
                peer_ids,
            });
        }
        Ok(out)
    }

    /// One line for the conversation list. An attachment-only message still
    /// says something: an image reads as a photo, a lone file is named.
    fn preview(&self, conversation_id: &str) -> Result<Option<String>> {
        let latest: Option<(String, String)> = {
            let conn = self.conn.lock().unwrap();
            conn.query_row(
                "SELECT id, body FROM messages
                 WHERE conversation_id = ?1
                 ORDER BY ts DESC, rowid DESC LIMIT 1",
                [conversation_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
        };
        let Some((message_id, body)) = latest else {
            return Ok(None);
        };
        if !body.trim().is_empty() {
            return Ok(Some(body));
        }
        let attachments = self.attachments_for(&message_id)?;
        Ok(preview_text(&attachments))
    }

    pub fn members(&self, conversation_id: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT peer_id FROM members WHERE conversation_id = ?1 ORDER BY joined_at")?;
        let rows = stmt
            .query_map([conversation_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn mark_read(&self, conversation_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE conversations SET unread = 0 WHERE id = ?1",
            [conversation_id],
        )?;
        Ok(())
    }

    // ---- messages ---------------------------------------------------------

    pub fn insert_message(&self, msg: &MessageInfo, incoming: bool) -> Result<()> {
        self.insert_message_with_attachments(msg, incoming, &[])
    }

    /// Store a message and the attachment rows that belong to it in one
    /// transaction, so a crash never leaves a bubble pointing at nothing.
    pub fn insert_message_with_attachments(
        &self,
        msg: &MessageInfo,
        incoming: bool,
        attachments: &[AttachmentInfo],
    ) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO messages(id, conversation_id, author_id, ts, body, kind, reply_to,
                                   state, route, edited_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                msg.id,
                msg.conversation_id,
                msg.author_id,
                msg.ts,
                msg.body,
                msg.kind,
                msg.reply_to,
                msg.state,
                msg.route,
                msg.edited_at
            ],
        )?;
        for (ordinal, attachment) in attachments.iter().enumerate() {
            insert_attachment_row(&tx, Some(&msg.id), ordinal, attachment)?;
        }
        tx.execute(
            "UPDATE conversations
             SET last_message_at = max(coalesce(last_message_at, 0), ?2),
                 unread = unread + ?3
             WHERE id = ?1",
            params![msg.conversation_id, msg.ts, if incoming { 1 } else { 0 }],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Record an upload before any message refers to it. The send that uses the
    /// hash inserts its own rows; this one only keeps the bytes reachable.
    pub fn insert_upload(&self, attachment: &AttachmentInfo) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        insert_attachment_row(&conn, None, 0, attachment)?;
        Ok(())
    }

    /// Forget unclaimed upload rows for hashes that a send has just claimed.
    pub fn forget_unclaimed(&self, hashes: &[String]) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        for hash in hashes {
            conn.execute(
                "DELETE FROM attachments WHERE message_id IS NULL AND hash = ?1",
                [hash],
            )?;
        }
        Ok(())
    }

    /// Drop upload rows no send ever claimed. Safe at startup, when no shell is
    /// mid-composition: a row left by a refused or abandoned send would
    /// otherwise pin its bytes for the life of the install.
    pub fn discard_unclaimed_uploads(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let removed = conn.execute("DELETE FROM attachments WHERE message_id IS NULL", [])?;
        Ok(removed)
    }

    pub fn get_message(&self, id: &str) -> Result<Option<MessageInfo>> {
        let message = {
            let conn = self.conn.lock().unwrap();
            conn.query_row(
                "SELECT id, conversation_id, author_id, ts, body, kind, reply_to, state, route, edited_at
                 FROM messages WHERE id = ?1",
                [id],
                map_message,
            )
            .optional()?
        };
        let Some(message) = message else {
            return Ok(None);
        };
        let mut messages = [message];
        self.hydrate(&mut messages)?;
        Ok(messages.into_iter().next())
    }

    pub fn update_message_state(&self, id: &str, state: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE messages SET state = ?2 WHERE id = ?1", params![id, state])?;
        Ok(())
    }

    pub fn set_message_route(&self, id: &str, route: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE messages SET route = ?2 WHERE id = ?1", params![id, route])?;
        Ok(())
    }

    pub fn list_messages(
        &self,
        conversation_id: &str,
        before: Option<i64>,
        limit: u32,
    ) -> Result<Vec<MessageInfo>> {
        let limit = limit.clamp(1, 500) as i64;
        let mut rows = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare(
                "SELECT id, conversation_id, author_id, ts, body, kind, reply_to, state, route, edited_at
                 FROM messages
                 WHERE conversation_id = ?1 AND (?2 IS NULL OR ts < ?2)
                 ORDER BY ts DESC, rowid DESC
                 LIMIT ?3",
            )?;
            let rows = stmt
                .query_map(params![conversation_id, before, limit], map_message)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        rows.reverse();
        self.hydrate(&mut rows)?;
        Ok(rows)
    }

    /// Messages that still need to go out. Recomputed rather than stored in a
    /// separate outbox table so there is a single source of truth.
    pub fn queued_messages(&self) -> Result<Vec<MessageInfo>> {
        let mut rows = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare(
                "SELECT id, conversation_id, author_id, ts, body, kind, reply_to, state, route, edited_at
                 FROM messages WHERE state = 'queued' ORDER BY ts ASC LIMIT 200",
            )?;
            let rows = stmt
                .query_map([], map_message)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        self.hydrate(&mut rows)?;
        Ok(rows)
    }

    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<MessageInfo>> {
        let mut rows = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare(
                "SELECT m.id, m.conversation_id, m.author_id, m.ts, m.body, m.kind,
                        m.reply_to, m.state, m.route, m.edited_at
                 FROM messages_fts f
                 JOIN messages m ON m.rowid = f.rowid
                 WHERE messages_fts MATCH ?1
                 ORDER BY m.ts DESC LIMIT ?2",
            )?;
            let rows = stmt
                .query_map(params![query, limit.clamp(1, 200)], map_message)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        self.hydrate(&mut rows)?;
        Ok(rows)
    }

    // ---- attachments ------------------------------------------------------

    pub fn attachments_for(&self, message_id: &str) -> Result<Vec<AttachmentInfo>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT name, mime, size, hash, width, height, thumbnail, reason
             FROM attachments WHERE message_id = ?1 ORDER BY ordinal ASC",
        )?;
        let rows = stmt
            .query_map([message_id], map_attachment)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Attach descriptors to messages that were read without them. A single
    /// query per message: a page is at most a few hundred rows.
    pub fn hydrate(&self, messages: &mut [MessageInfo]) -> Result<()> {
        for message in messages.iter_mut() {
            message.attachments = self.attachments_for(&message.id)?;
        }
        Ok(())
    }

    /// The descriptor for one hash. Uploads leave an unclaimed row, so a send
    /// can only name bytes the engine actually stores.
    pub fn attachment_by_hash(&self, hash: &str) -> Result<Option<AttachmentInfo>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT name, mime, size, hash, width, height, thumbnail, reason
                 FROM attachments WHERE hash = ?1 ORDER BY rowid DESC LIMIT 1",
                [hash],
                map_attachment,
            )
            .optional()?;
        Ok(row)
    }

    /// The descriptor whose display thumbnail has this hash. Thumbnails are
    /// addressed by their own content, so a client can use the hash from a
    /// descriptor as an image URL without knowing the original.
    pub fn attachment_by_thumbnail(&self, hash: &str) -> Result<Option<AttachmentInfo>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT name, mime, size, hash, width, height, thumbnail, reason
                 FROM attachments WHERE thumbnail = ?1 ORDER BY rowid DESC LIMIT 1",
                [hash],
                map_attachment,
            )
            .optional()?;
        Ok(row)
    }

    pub fn has_attachment_hash(&self, hash: &str) -> Result<bool> {
        if hash.is_empty() {
            return Ok(false);
        }
        let conn = self.conn.lock().unwrap();
        let found: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM attachments WHERE hash = ?1 LIMIT 1",
                [hash],
                |r| r.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// Bytes used by attachments that still have a blob, counting identical
    /// content once.
    pub fn attachment_usage(&self) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        let total: i64 = conn.query_row(
            "SELECT coalesce(sum(size), 0) FROM
               (SELECT max(size) AS size FROM attachments
                WHERE reason IS NULL AND hash <> '' GROUP BY hash)",
            [],
            |r| r.get(0),
        )?;
        Ok(total.max(0) as u64)
    }

    /// Hashes eligible for eviction, oldest first. `received` selects the first
    /// pass; sent blobs are only considered once no received blob remains.
    pub fn evictable_hashes(&self, received: bool) -> Result<Vec<(String, u64)>> {
        let me = self
            .identity()?
            .map(|identity| identity.id)
            .unwrap_or_default();
        let comparison = if received { "<>" } else { "=" };
        let sql = format!(
            "SELECT a.hash, max(a.size)
             FROM attachments a JOIN messages m ON m.id = a.message_id
             WHERE a.reason IS NULL AND a.hash <> '' AND m.author_id {comparison} ?1
             GROUP BY a.hash
             ORDER BY min(m.ts) ASC, a.hash ASC"
        );
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map([me], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?.max(0) as u64))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Clear every row that shares an evicted hash. Returns how many rows were
    /// touched, so a caller can publish only when something changed.
    pub fn mark_hash_missing(&self, hash: &str, reason: &str) -> Result<usize> {
        if hash.is_empty() {
            return Ok(0);
        }
        let conn = self.conn.lock().unwrap();
        let touched = conn.execute(
            "UPDATE attachments SET reason = ?2, thumbnail = NULL
             WHERE hash = ?1 AND reason IS NULL",
            params![hash, reason],
        )?;
        Ok(touched)
    }

    /// The thumbnail hash recorded for an original, if it had one.
    pub fn thumbnail_for(&self, hash: &str) -> Result<Option<String>> {
        if hash.is_empty() {
            return Ok(None);
        }
        let conn = self.conn.lock().unwrap();
        let found: Option<String> = conn
            .query_row(
                "SELECT thumbnail FROM attachments
                 WHERE hash = ?1 AND thumbnail IS NOT NULL LIMIT 1",
                [hash],
                |r| r.get(0),
            )
            .optional()?;
        Ok(found)
    }

    pub fn has_attachment_thumbnail(&self, hash: &str) -> Result<bool> {
        if hash.is_empty() {
            return Ok(false);
        }
        let conn = self.conn.lock().unwrap();
        let found: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM attachments WHERE thumbnail = ?1 LIMIT 1",
                [hash],
                |r| r.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// Remove one attachment's bytes and thumbnail, then mark every row that
    /// shares the hash as missing. Returns whether a row changed.
    pub fn evict_attachment(
        &self,
        blobs: &crate::attachments::BlobStore,
        hash: &str,
        reason: &str,
    ) -> Result<bool> {
        let thumbnail = self.thumbnail_for(hash)?;
        blobs.remove(hash)?;
        if let Some(thumbnail) = thumbnail {
            blobs.remove_thumbnail(&thumbnail)?;
        }
        Ok(self.mark_hash_missing(hash, reason)? > 0)
    }

    /// Drop blobs and thumbnails no row references any more, which is what a
    /// crash between writing bytes and committing the row leaves behind.
    pub fn sweep_orphan_blobs(&self, blobs: &crate::attachments::BlobStore) -> Result<usize> {
        crate::attachments::sweep(
            blobs,
            |hash| self.has_attachment_hash(hash),
            |hash| self.has_attachment_thumbnail(hash),
        )
    }

    // ---- event log --------------------------------------------------------

    pub fn record_event(&self, name: &str, data: &serde_json::Value) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO events(name, data, ts) VALUES (?1, ?2, ?3)",
            params![name, data.to_string(), crate::now_ms()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn events_since(&self, seq: i64) -> Result<Vec<EventRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT seq, name, data, ts FROM events WHERE seq > ?1 ORDER BY seq ASC LIMIT 2000",
        )?;
        let rows = stmt
            .query_map([seq], |r| {
                let data: String = r.get(2)?;
                Ok(EventRow {
                    seq: r.get(0)?,
                    name: r.get(1)?,
                    data: serde_json::from_str(&data).unwrap_or(serde_json::Value::Null),
                    ts: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    // ---- meta -------------------------------------------------------------

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let value = conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?;
        Ok(value)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    // ---- announce history -------------------------------------------------

    /// The bucket size for coalesced announces: ten minutes.
    pub const ANNOUNCE_BUCKET_MS: i64 = 600_000;
    /// How long announce history is kept.
    pub const ANNOUNCE_RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1000;
    /// How many announce rows are kept.
    pub const ANNOUNCE_LIMIT: i64 = 5000;

    /// Record one announce sighting, coalescing repeats inside the same
    /// ten-minute bucket. Returns the row as it now stands.
    pub fn record_announce(
        &self,
        address: &str,
        name: Option<&str>,
        aspect: Option<&str>,
        interface: Option<&str>,
        hops: Option<u32>,
        ts: i64,
    ) -> Result<AnnounceSighting> {
        let bucket = ts.div_euclid(Self::ANNOUNCE_BUCKET_MS);
        {
            let conn = self.conn.lock().unwrap();
            conn.execute(
                r#"INSERT INTO mesh_announces
                     (address, name, aspect, interface, hops, bucket, first_seen, last_seen, count)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, 1)
                   ON CONFLICT(address, bucket) DO UPDATE SET
                     name      = coalesce(excluded.name, mesh_announces.name),
                     aspect    = coalesce(excluded.aspect, mesh_announces.aspect),
                     interface = coalesce(excluded.interface, mesh_announces.interface),
                     hops      = coalesce(excluded.hops, mesh_announces.hops),
                     first_seen = min(mesh_announces.first_seen, excluded.first_seen),
                     last_seen = max(mesh_announces.last_seen, excluded.last_seen),
                     count     = mesh_announces.count + 1"#,
                params![address, name, aspect, interface, hops, bucket, ts],
            )?;
        }
        self.maybe_prune_announces(ts)?;
        let sighting = self.announce_at(address, bucket)?;
        sighting.with_context(|| format!("announce for {address} vanished after insert"))
    }

    fn announce_at(&self, address: &str, bucket: i64) -> Result<Option<AnnounceSighting>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT address, name, aspect, interface, hops, bucket, first_seen, last_seen, count
                 FROM mesh_announces WHERE address = ?1 AND bucket = ?2",
                params![address, bucket],
                map_announce,
            )
            .optional()?;
        Ok(row)
    }

    /// Newest first, paged by `last_seen` so a live prepend cannot shift a page.
    pub fn list_announces(
        &self,
        before: Option<i64>,
        limit: u32,
    ) -> Result<Vec<AnnounceSighting>> {
        let limit = limit.clamp(1, 500) as i64;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT address, name, aspect, interface, hops, bucket, first_seen, last_seen, count
             FROM mesh_announces
             WHERE (?1 IS NULL OR last_seen < ?1)
             ORDER BY last_seen DESC, id DESC
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![before, limit], map_announce)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Prune at most once a minute so a busy mesh does not pay for it per
    /// announce.
    fn maybe_prune_announces(&self, now: i64) -> Result<()> {
        {
            let mut last = self.last_announce_prune.lock().unwrap();
            if let Some(previous) = *last {
                if now - previous < 60_000 {
                    return Ok(());
                }
            }
            *last = Some(now);
        }
        self.prune_announces(now)?;
        Ok(())
    }

    /// Delete rows past the retention window, then trim to the newest
    /// [`Self::ANNOUNCE_LIMIT`]. Returns how many rows were removed.
    pub fn prune_announces(&self, now: i64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let stale = conn.execute(
            "DELETE FROM mesh_announces WHERE last_seen < ?1",
            params![now - Self::ANNOUNCE_RETENTION_MS],
        )?;
        let excess = conn.execute(
            "DELETE FROM mesh_announces WHERE id NOT IN
               (SELECT id FROM mesh_announces ORDER BY last_seen DESC, id DESC LIMIT ?1)",
            params![Self::ANNOUNCE_LIMIT],
        )?;
        Ok(stale + excess)
    }

    // ---- interface configuration -----------------------------------------

    pub fn list_interfaces(&self) -> Result<Vec<InterfaceRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, kind, config, enabled, origin, note FROM mesh_interfaces
             ORDER BY created_at ASC, name ASC",
        )?;
        let rows = stmt
            .query_map([], map_interface)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn interface(&self, id: &str) -> Result<Option<InterfaceRow>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT id, name, kind, config, enabled, origin, note
                 FROM mesh_interfaces WHERE id = ?1",
                [id],
                map_interface,
            )
            .optional()?;
        Ok(row)
    }

    pub fn interface_by_name(&self, name: &str) -> Result<Option<InterfaceRow>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT id, name, kind, config, enabled, origin, note
                 FROM mesh_interfaces WHERE name = ?1",
                [name],
                map_interface,
            )
            .optional()?;
        Ok(row)
    }

    pub fn create_interface(
        &self,
        name: &str,
        kind: &str,
        config: &serde_json::Value,
        enabled: bool,
        origin: &str,
    ) -> Result<InterfaceRow> {
        let row = InterfaceRow {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            kind: kind.to_string(),
            config: config.clone(),
            enabled,
            origin: origin.to_string(),
            note: None,
        };
        self.insert_interface_row(&row)?;
        Ok(row)
    }

    /// Insert a row with a caller-chosen id, so an in-memory candidate can be
    /// sent to the bridge and only persisted once Reticulum accepted it.
    pub fn insert_interface_row(&self, row: &InterfaceRow) -> Result<()> {
        let now = crate::now_ms();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO mesh_interfaces
               (id, name, kind, config, enabled, origin, note, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
            params![
                row.id,
                row.name,
                row.kind,
                row.config.to_string(),
                if row.enabled { 1 } else { 0 },
                row.origin,
                row.note,
                now
            ],
        )?;
        Ok(())
    }

    pub fn update_interface_config(
        &self,
        id: &str,
        config: &serde_json::Value,
    ) -> Result<InterfaceRow> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE mesh_interfaces SET config = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, config.to_string(), crate::now_ms()],
        )?;
        drop(conn);
        if changed == 0 {
            anyhow::bail!("unknown interface: {id}");
        }
        self.interface(id)?
            .context("interface vanished right after update")
    }

    pub fn set_interface_enabled(&self, id: &str, enabled: bool) -> Result<InterfaceRow> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE mesh_interfaces SET enabled = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, if enabled { 1 } else { 0 }, crate::now_ms()],
        )?;
        drop(conn);
        if changed == 0 {
            anyhow::bail!("unknown interface: {id}");
        }
        self.interface(id)?
            .context("interface vanished right after update")
    }

    pub fn delete_interface(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM mesh_interfaces WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Sections of the generated config rchat does not own, verbatim.
    pub fn extra_config(&self) -> Result<Vec<(String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT section, raw FROM mesh_config_extra ORDER BY rowid ASC")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn replace_extra_config(&self, blocks: &[(String, String)]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM mesh_config_extra", [])?;
        for (section, raw) in blocks {
            tx.execute(
                "INSERT OR REPLACE INTO mesh_config_extra(section, raw) VALUES (?1, ?2)",
                params![section, raw],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// One-shot first-run network setup.
    ///
    /// Order matters: a hand-written `rns/config` is authoritative until the
    /// bridge has imported it, so when one is present and no rows exist this
    /// does nothing and leaves the flag unset. Otherwise discovery and the
    /// legacy relay become rows once, and every later start returns them
    /// untouched.
    pub fn import_network_once(
        &self,
        legacy: Option<&LegacyNetwork>,
        config_present: bool,
    ) -> Result<Vec<InterfaceRow>> {
        if self.meta(NETWORK_IMPORTED)?.as_deref() == Some("1") {
            return self.list_interfaces();
        }
        let existing = self.list_interfaces()?;
        if !existing.is_empty() {
            // The user already has interfaces (imported from a config, or
            // created by an earlier run): never add to or rewrite them.
            self.set_meta(NETWORK_IMPORTED, "1")?;
            return Ok(existing);
        }
        if config_present {
            return Ok(existing);
        }

        let discovery_wanted = legacy
            .and_then(|legacy| legacy.auto_discovery)
            .unwrap_or(true);
        let mut rows = Vec::new();
        if discovery_wanted {
            let origin = if legacy.and_then(|legacy| legacy.auto_discovery) == Some(true) {
                "legacy"
            } else {
                "default"
            };
            rows.push(InterfaceRow {
                id: uuid::Uuid::new_v4().to_string(),
                name: DEFAULT_DISCOVERY_NAME.to_string(),
                kind: "AutoInterface".to_string(),
                config: serde_json::json!({ "group_id": "reticulum" }),
                enabled: true,
                origin: origin.to_string(),
                note: None,
            });
        }

        if let Some(spec) = legacy
            .and_then(|legacy| legacy.tcp_relay.as_deref())
            .map(str::trim)
            .filter(|spec| !spec.is_empty())
        {
            rows.push(match parse_relay(spec) {
                Ok((host, port)) => InterfaceRow {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: LEGACY_RELAY_NAME.to_string(),
                    kind: "TCPClientInterface".to_string(),
                    config: serde_json::json!({ "target_host": host, "target_port": port }),
                    enabled: true,
                    origin: "legacy".to_string(),
                    note: Some(
                        "Imported from the old relay setting. Delete it if you no longer \
                         use that relay."
                            .to_string(),
                    ),
                },
                Err(reason) => InterfaceRow {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: LEGACY_RELAY_NAME.to_string(),
                    kind: "TCPClientInterface".to_string(),
                    config: serde_json::json!({ "target_host": spec, "target_port": 0 }),
                    enabled: false,
                    origin: "legacy".to_string(),
                    note: Some(format!(
                        "Imported from the old relay setting, which could not be read: \
                         {reason}. Correct the address and enable it."
                    )),
                },
            });
        }

        let mut stored = Vec::with_capacity(rows.len());
        for row in rows {
            // A same-named row is never overwritten. Rows were empty above, so
            // this only guards a race or a future caller.
            if self.interface_by_name(&row.name)?.is_some() {
                continue;
            }
            self.insert_interface_row(&row)?;
            stored.push(row);
        }
        self.set_meta(NETWORK_IMPORTED, "1")?;
        if stored.is_empty() {
            return self.list_interfaces();
        }
        Ok(stored)
    }
}

fn map_peer(r: &rusqlite::Row<'_>) -> rusqlite::Result<PeerInfo> {
    let caps: String = r.get(6)?;
    Ok(PeerInfo {
        id: r.get(0)?,
        address: r.get(1)?,
        nickname: r.get(2)?,
        announced_name: r.get(3)?,
        last_seen: r.get(4)?,
        bandwidth_class: r.get(5)?,
        caps: serde_json::from_str(&caps).unwrap_or_else(|_| Caps::default()),
    })
}

/// Add a column only when an existing table lacks it. Startup runs this every
/// time, so the column list is the guard rather than the schema version.
fn ensure_column(conn: &Connection, table: &str, column: &str, kind: &str) -> Result<()> {
    let present = {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let names = stmt
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        names.iter().any(|name| name == column)
    };
    if !present {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {kind}"),
            [],
        )?;
    }
    Ok(())
}

fn map_message(r: &rusqlite::Row<'_>) -> rusqlite::Result<MessageInfo> {
    Ok(MessageInfo {
        id: r.get(0)?,
        conversation_id: r.get(1)?,
        author_id: r.get(2)?,
        ts: r.get(3)?,
        body: r.get(4)?,
        kind: r.get(5)?,
        reply_to: r.get(6)?,
        state: r.get(7)?,
        route: r.get(8)?,
        edited_at: r.get(9)?,
        attachments: Vec::new(),
    })
}

fn insert_attachment_row(
    conn: &Connection,
    message_id: Option<&str>,
    ordinal: usize,
    attachment: &AttachmentInfo,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO attachments
           (message_id, ordinal, name, mime, size, hash, width, height, thumbnail, reason)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            message_id,
            ordinal as i64,
            attachment.name,
            attachment.mime,
            attachment.size as i64,
            attachment.hash,
            attachment.width,
            attachment.height,
            attachment.thumbnail,
            attachment.reason
        ],
    )?;
    Ok(())
}

fn map_attachment(r: &rusqlite::Row<'_>) -> rusqlite::Result<AttachmentInfo> {
    Ok(AttachmentInfo {
        name: r.get(0)?,
        mime: r.get(1)?,
        size: r.get::<_, i64>(2)?.max(0) as u64,
        hash: r.get(3)?,
        width: r.get(4)?,
        height: r.get(5)?,
        thumbnail: r.get(6)?,
        reason: r.get(7)?,
    })
}

/// The conversation-list line for a message whose body is only a caption.
fn preview_text(attachments: &[AttachmentInfo]) -> Option<String> {
    if attachments.is_empty() {
        return None;
    }
    if attachments.len() == 1 {
        let only = &attachments[0];
        return Some(if only.is_image() {
            "Photo".to_string()
        } else if only.name.is_empty() {
            "File".to_string()
        } else {
            only.name.clone()
        });
    }
    let images = attachments.iter().filter(|item| item.is_image()).count();
    if images == attachments.len() {
        Some(format!("{images} photos"))
    } else if images == 0 {
        Some(format!("{} files", attachments.len()))
    } else {
        Some(format!("{} attachments", attachments.len()))
    }
}

fn map_announce(r: &rusqlite::Row<'_>) -> rusqlite::Result<AnnounceSighting> {
    Ok(AnnounceSighting {
        address: r.get(0)?,
        name: r.get(1)?,
        aspect: r.get(2)?,
        interface: r.get(3)?,
        hops: r.get(4)?,
        bucket: r.get(5)?,
        first_seen: r.get(6)?,
        last_seen: r.get(7)?,
        count: r.get(8)?,
    })
}

fn map_interface(r: &rusqlite::Row<'_>) -> rusqlite::Result<InterfaceRow> {
    let config: String = r.get(3)?;
    Ok(InterfaceRow {
        id: r.get(0)?,
        name: r.get(1)?,
        kind: r.get(2)?,
        config: serde_json::from_str(&config).unwrap_or(serde_json::Value::Null),
        enabled: r.get::<_, i64>(4)? != 0,
        origin: r.get(5)?,
        note: r.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(conv: &str, author: &str, body: &str, ts: i64) -> MessageInfo {
        MessageInfo {
            id: uuid::Uuid::new_v4().to_string(),
            conversation_id: conv.to_string(),
            author_id: author.to_string(),
            ts,
            body: body.to_string(),
            kind: "text".into(),
            reply_to: None,
            state: "sent".into(),
            route: None,
            edited_at: None,
            attachments: Vec::new(),
        }
    }

    /// What the announce path does: read the stored peer, change what the
    /// announce carried, and upsert the whole row back.
    fn announce(store: &Store, id: &str, name: Option<&str>, ts: i64) {
        let peer = store.peer(id).unwrap().expect("a peer to announce");
        store
            .upsert_peer(&PeerInfo {
                announced_name: name.map(str::to_string),
                last_seen: Some(ts),
                ..peer
            })
            .unwrap();
    }

    fn attachment(name: &str, mime: &str, size: u64, hash: &str) -> AttachmentInfo {
        AttachmentInfo {
            name: name.to_string(),
            mime: mime.to_string(),
            size,
            hash: hash.to_string(),
            width: None,
            height: None,
            thumbnail: None,
            reason: None,
        }
    }

    #[test]
    fn identity_is_created_once() {
        let store = Store::open_in_memory().unwrap();
        let a = store.ensure_identity().unwrap();
        let b = store.ensure_identity().unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(store.list_peers().unwrap().len(), 0);
    }

    #[test]
    fn display_name_flag_only_set_when_chosen() {
        let store = Store::open_in_memory().unwrap();
        store.ensure_identity().unwrap();
        assert!(!store.display_name_set().unwrap());
        store.set_display_name("you").unwrap();
        assert!(store.display_name_set().unwrap());
        assert_eq!(store.identity().unwrap().unwrap().display_name, "you");
    }

    #[test]
    fn direct_conversations_are_reused() {
        let store = Store::open_in_memory().unwrap();
        store.ensure_identity().unwrap();
        let a = store.open_direct("peer-1").unwrap();
        let b = store.open_direct("peer-1").unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(store.list_conversations().unwrap().len(), 1);
        let c = store.open_direct("peer-2").unwrap();
        assert_ne!(a.id, c.id);
    }

    #[test]
    fn messages_order_and_unread() {
        let store = Store::open_in_memory().unwrap();
        store.ensure_identity().unwrap();
        let conv = store.open_direct("peer-1").unwrap();
        store.insert_message(&msg(&conv.id, "peer-1", "one", 10), true).unwrap();
        store.insert_message(&msg(&conv.id, "peer-1", "two", 20), true).unwrap();
        store.insert_message(&msg(&conv.id, "me", "three", 30), false).unwrap();

        let list = store.list_messages(&conv.id, None, 50).unwrap();
        assert_eq!(list.iter().map(|m| m.body.as_str()).collect::<Vec<_>>(), ["one", "two", "three"]);

        let fresh = store.get_conversation(&conv.id).unwrap();
        assert_eq!(fresh.unread, 2);
        assert_eq!(fresh.preview.as_deref(), Some("three"));

        store.mark_read(&conv.id).unwrap();
        assert_eq!(store.get_conversation(&conv.id).unwrap().unread, 0);
    }

    #[test]
    fn event_log_is_monotonic() {
        let store = Store::open_in_memory().unwrap();
        let a = store.record_event("a", &serde_json::json!({"x":1})).unwrap();
        let b = store.record_event("b", &serde_json::json!({"x":2})).unwrap();
        assert!(b > a);
        let rows = store.events_since(a).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "b");
    }

    #[test]
    fn full_text_search_works() {
        let store = Store::open_in_memory().unwrap();
        store.ensure_identity().unwrap();
        let conv = store.open_direct("peer-1").unwrap();
        store.insert_message(&msg(&conv.id, "peer-1", "the quick brown fox", 1), true).unwrap();
        store.insert_message(&msg(&conv.id, "peer-1", "lazy dog", 2), true).unwrap();
        let hits = store.search("quick", 10).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn attachments_are_ordered_and_hydrated_with_their_message() {
        let store = Store::open_in_memory().unwrap();
        store.ensure_identity().unwrap();
        let conv = store.open_direct("peer-1").unwrap();
        let mut message = msg(&conv.id, "peer-1", "look", 10);
        message.kind = "image".into();
        let rows = [
            attachment("one.png", "image/png", 10, &"aa".repeat(32)),
            attachment("two.pdf", "application/pdf", 20, &"bb".repeat(32)),
        ];
        store
            .insert_message_with_attachments(&message, true, &rows)
            .unwrap();

        let read = store.get_message(&message.id).unwrap().unwrap();
        assert_eq!(read.attachments, rows);
        let page = store.list_messages(&conv.id, None, 10).unwrap();
        assert_eq!(page[0].attachments.len(), 2);
        assert_eq!(page[0].attachments[1].name, "two.pdf");
        // The caption stays in the body and is what search sees.
        assert_eq!(page[0].body, "look");
        assert_eq!(store.search("look", 10).unwrap().len(), 1);
    }

    #[test]
    fn deleting_a_message_takes_its_attachment_rows() {
        let store = Store::open_in_memory().unwrap();
        store.ensure_identity().unwrap();
        let conv = store.open_direct("peer-1").unwrap();
        let mut message = msg(&conv.id, "peer-1", "", 10);
        let rows = [attachment("photo.png", "image/png", 10, &"cc".repeat(32))];
        store
            .insert_message_with_attachments(&message, true, &rows)
            .unwrap();
        assert!(store.has_attachment_hash(&rows[0].hash).unwrap());
        {
            let conn = store.conn.lock().unwrap();
            conn.execute("DELETE FROM messages WHERE id = ?1", [&message.id])
                .unwrap();
        }
        assert!(!store.has_attachment_hash(&rows[0].hash).unwrap());
        assert!(store.attachments_for(&message.id).unwrap().is_empty());
        message.body.clear();
    }

    #[test]
    fn an_upload_row_is_claimable_but_not_a_message() {
        let store = Store::open_in_memory().unwrap();
        store.ensure_identity().unwrap();
        store
            .insert_upload(&attachment("draft.png", "image/png", 5, &"dd".repeat(32)))
            .unwrap();

        let found = store
            .attachment_by_hash(&"dd".repeat(32))
            .unwrap()
            .expect("an upload leaves a row a send can name");
        assert_eq!(found.name, "draft.png");
        assert_eq!(store.attachment_usage().unwrap(), 5);
        assert!(store.evictable_hashes(true).unwrap().is_empty(), "unclaimed rows have no author");

        store.forget_unclaimed(&[found.hash]).unwrap();
        assert!(store.attachment_by_hash(&"dd".repeat(32)).unwrap().is_none());
    }

    #[test]
    fn usage_counts_identical_content_once() {
        let store = Store::open_in_memory().unwrap();
        store.ensure_identity().unwrap();
        let conv = store.open_direct("peer-1").unwrap();
        let shared = "ee".repeat(32);
        let mut first = msg(&conv.id, "peer-1", "", 1);
        first.kind = "image".into();
        let mut second = msg(&conv.id, "me", "", 2);
        second.kind = "image".into();
        store
            .insert_message_with_attachments(
                &first,
                true,
                &[attachment("a.png", "image/png", 100, &shared)],
            )
            .unwrap();
        store
            .insert_message_with_attachments(
                &second,
                false,
                &[attachment("a-again.png", "image/png", 100, &shared)],
            )
            .unwrap();
        assert_eq!(store.attachment_usage().unwrap(), 100);
    }

    #[test]
    fn eviction_offers_received_blobs_before_sent_ones() {
        let store = Store::open_in_memory().unwrap();
        let me = store.ensure_identity().unwrap();
        let conv = store.open_direct("peer-1").unwrap();
        let received = "11".repeat(32);
        let sent_old = "22".repeat(32);
        let sent_new = "33".repeat(32);

        let mut incoming = msg(&conv.id, "peer-1", "", 100);
        incoming.kind = "file".into();
        let mut mine = msg(&conv.id, &me.id, "", 50);
        mine.kind = "file".into();
        let mut mine_newer = msg(&conv.id, &me.id, "", 200);
        mine_newer.kind = "file".into();
        store
            .insert_message_with_attachments(
                &incoming,
                true,
                &[attachment("in.pdf", "application/pdf", 7, &received)],
            )
            .unwrap();
        store
            .insert_message_with_attachments(
                &mine,
                false,
                &[attachment("old.pdf", "application/pdf", 7, &sent_old)],
            )
            .unwrap();
        store
            .insert_message_with_attachments(
                &mine_newer,
                false,
                &[attachment("new.pdf", "application/pdf", 7, &sent_new)],
            )
            .unwrap();

        let received_first = store.evictable_hashes(true).unwrap();
        assert_eq!(
            received_first.iter().map(|(hash, _)| hash.as_str()).collect::<Vec<_>>(),
            [received.as_str()]
        );
        let sent_after = store.evictable_hashes(false).unwrap();
        assert_eq!(
            sent_after.iter().map(|(hash, _)| hash.as_str()).collect::<Vec<_>>(),
            [sent_old.as_str(), sent_new.as_str()],
            "oldest sent blob goes first"
        );
    }

    #[test]
    fn marking_a_hash_missing_keeps_the_row_and_its_metadata() {
        let store = Store::open_in_memory().unwrap();
        store.ensure_identity().unwrap();
        let conv = store.open_direct("peer-1").unwrap();
        let message = msg(&conv.id, "peer-1", "caption", 10);
        let hash = "44".repeat(32);
        store
            .insert_message_with_attachments(
                &message,
                true,
                &[attachment("gone.pdf", "application/pdf", 9, &hash)],
            )
            .unwrap();

        let touched = store
            .mark_hash_missing(&hash, crate::attachments::REASON_EVICTED)
            .unwrap();
        assert_eq!(touched, 1);

        let read = store.get_message(&message.id).unwrap().unwrap();
        assert_eq!(read.body, "caption");
        assert_eq!(read.ts, 10);
        let item = &read.attachments[0];
        assert_eq!(item.name, "gone.pdf");
        assert_eq!(item.size, 9);
        assert!(!item.has_bytes());
        assert_eq!(
            item.reason.as_deref(),
            Some(crate::attachments::REASON_EVICTED)
        );
        assert_eq!(store.attachment_usage().unwrap(), 0, "evicted bytes stop counting");
        assert!(
            store.evictable_hashes(true).unwrap().is_empty(),
            "an evicted blob is not evicted twice"
        );
    }

    #[test]
    fn conversation_previews_describe_attachments() {
        let store = Store::open_in_memory().unwrap();
        store.ensure_identity().unwrap();
        let conv = store.open_direct("peer-1").unwrap();

        let mut photo = msg(&conv.id, "peer-1", "", 10);
        photo.kind = "image".into();
        store
            .insert_message_with_attachments(
                &photo,
                true,
                &[attachment("photo.png", "image/png", 1, &"55".repeat(32))],
            )
            .unwrap();
        assert_eq!(
            store.get_conversation(&conv.id).unwrap().preview.as_deref(),
            Some("Photo")
        );

        let mut pdf = msg(&conv.id, "peer-1", "", 20);
        pdf.kind = "file".into();
        store
            .insert_message_with_attachments(
                &pdf,
                true,
                &[attachment("statement.pdf", "application/pdf", 2, &"66".repeat(32))],
            )
            .unwrap();
        assert_eq!(
            store.list_conversations().unwrap()[0].preview.as_deref(),
            Some("statement.pdf")
        );

        let captioned = msg(&conv.id, "peer-1", "look at this", 30);
        store.insert_message(&captioned, true).unwrap();
        assert_eq!(
            store.get_conversation(&conv.id).unwrap().preview.as_deref(),
            Some("look at this"),
            "a caption is still the preview"
        );
    }

    #[test]
    fn evicting_an_attachment_frees_both_halves() {
        let store = Store::open_in_memory().unwrap();
        store.ensure_identity().unwrap();
        let conv = store.open_direct("peer-1").unwrap();
        let dir = std::env::temp_dir().join(format!(
            "rchat-evict-{}-{}",
            std::process::id(),
            crate::now_ms()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let blobs = crate::attachments::BlobStore::new(&dir);

        let ingested = crate::attachments::ingest_bytes(
            &blobs,
            &{
                let mut picture = image::RgbImage::new(20, 20);
                picture.put_pixel(0, 0, image::Rgb([1, 2, 3]));
                let mut out = std::io::Cursor::new(Vec::new());
                image::DynamicImage::ImageRgb8(picture)
                    .write_to(&mut out, image::ImageFormat::Png)
                    .unwrap();
                out.into_inner()
            },
            "photo.png",
            None,
        )
        .unwrap();
        let thumbnail = ingested.thumbnail.clone().unwrap();

        let mut message = msg(&conv.id, "peer-1", "", 10);
        message.kind = "image".into();
        store
            .insert_message_with_attachments(&message, true, &[ingested.descriptor()])
            .unwrap();

        assert!(store.evict_attachment(&blobs, &ingested.hash, crate::attachments::REASON_EVICTED).unwrap());
        assert!(!blobs.contains(&ingested.hash));
        assert!(!blobs.contains_thumbnail(&thumbnail));
        let read = store.get_message(&message.id).unwrap().unwrap();
        assert_eq!(read.body, "", "an empty caption stays empty");
        assert_eq!(read.ts, 10);
        assert_eq!(
            read.attachments[0].reason.as_deref(),
            Some(crate::attachments::REASON_EVICTED)
        );
        assert!(store.attachment_usage().unwrap() == 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_orphan_sweep_keeps_referenced_blobs_only() {
        let store = Store::open_in_memory().unwrap();
        store.ensure_identity().unwrap();
        let conv = store.open_direct("peer-1").unwrap();

        let dir = std::env::temp_dir().join(format!(
            "rchat-sweep-{}-{}",
            std::process::id(),
            crate::now_ms()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let blobs = crate::attachments::BlobStore::new(&dir);
        let kept = blobs.write_bytes(b"kept bytes").unwrap();
        let orphan = blobs.write_bytes(b"orphan bytes").unwrap();

        let message = msg(&conv.id, "peer-1", "", 10);
        store
            .insert_message_with_attachments(
                &message,
                true,
                &[attachment("keep.png", "image/png", 10, &kept)],
            )
            .unwrap();

        let removed = store.sweep_orphan_blobs(&blobs).unwrap();
        assert_eq!(removed, 1);
        assert!(blobs.contains(&kept), "a referenced blob stays");
        assert!(!blobs.contains(&orphan), "a crash leftover goes");
        let _ = std::fs::remove_dir_all(&dir);
    }

    const HOUR: i64 = 3_600_000;

    #[test]
    fn announces_coalesce_inside_a_bucket() {
        let store = Store::open_in_memory().unwrap();
        let base = 1_760_000_000_000i64;
        for i in 0..1000 {
            store
                .record_announce("ab".repeat(16).as_str(), Some("alice"), None, Some("LAN"), Some(1), base + i)
                .unwrap();
        }
        let rows = store.list_announces(None, 100).unwrap();
        assert_eq!(rows.len(), 1, "one bucket must hold one row");
        assert_eq!(rows[0].count, 1000);
        assert_eq!(rows[0].first_seen, base);
        assert_eq!(rows[0].last_seen, base + 999);
        assert_eq!(rows[0].name.as_deref(), Some("alice"));

        // A new ten-minute bucket is a new row.
        store
            .record_announce("ab".repeat(16).as_str(), None, None, None, None, base + 600_000)
            .unwrap();
        assert_eq!(store.list_announces(None, 100).unwrap().len(), 2);
    }

    #[test]
    fn announces_prune_by_age_and_count() {
        let store = Store::open_in_memory().unwrap();
        let now = 1_760_000_000_000i64;
        store
            .record_announce("aa".repeat(16).as_str(), None, None, None, Some(1), now - 8 * 24 * HOUR)
            .unwrap();
        let pruned = store.prune_announces(now).unwrap();
        assert_eq!(pruned, 1);
        store
            .record_announce("bb".repeat(16).as_str(), None, None, None, Some(1), now)
            .unwrap();
        let rows = store.list_announces(None, 100).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].address, "bb".repeat(16));

        // Trim to the newest `ANNOUNCE_LIMIT` rows. One minute apart, so the
        // seven-day window never interferes with the count limit.
        for i in 0..6000 {
            store
                .record_announce(
                    format!("{:032x}", i + 1).as_str(),
                    None,
                    None,
                    None,
                    Some(1),
                    now + (i as i64 + 1) * 60_000,
                )
                .unwrap();
        }
        let total: i64 = store
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM mesh_announces", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            total,
            Store::ANNOUNCE_LIMIT,
            "history must settle at the retention limit, not keep growing"
        );
    }

    #[test]
    fn announce_pruning_is_throttled() {
        let store = Store::open_in_memory().unwrap();
        let base = 1_760_000_000_000i64;
        for i in 0..50 {
            store
                .record_announce("cc".repeat(16).as_str(), None, None, None, None, base + i)
                .unwrap();
        }
        // The first announce prunes; the rest, inside the same minute, do not.
        assert_eq!(*store.last_announce_prune.lock().unwrap(), Some(base));
    }

    #[test]
    fn announce_history_pages_newest_first() {
        let store = Store::open_in_memory().unwrap();
        let base = 1_760_000_000_000i64;
        for i in 0..5 {
            store
                .record_announce(
                    format!("{:032x}", i + 1).as_str(),
                    None,
                    None,
                    None,
                    Some(1),
                    base + i * 600_000,
                )
                .unwrap();
        }
        let page = store.list_announces(None, 2).unwrap();
        assert_eq!(page[0].address, format!("{:032x}", 5));
        assert_eq!(page[1].address, format!("{:032x}", 4));
        let next = store.list_announces(Some(page[1].last_seen), 2).unwrap();
        assert_eq!(next[0].address, format!("{:032x}", 3));
    }

    #[test]
    fn interfaces_crud_and_meta() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.meta("mesh_config_imported").unwrap(), None);
        store.set_meta("mesh_config_imported", "1").unwrap();
        assert_eq!(store.meta("mesh_config_imported").unwrap().as_deref(), Some("1"));

        let config = serde_json::json!({"target_host": "relay.example", "target_port": 4242});
        let row = store
            .create_interface("Relay", "TCPClientInterface", &config, true, "managed")
            .unwrap();
        assert_eq!(row.config, config);
        assert_eq!(store.list_interfaces().unwrap().len(), 1);
        assert_eq!(store.interface_by_name("Relay").unwrap().unwrap().id, row.id);

        let edited = store
            .update_interface_config(&row.id, &serde_json::json!({"target_port": 4243}))
            .unwrap();
        assert_eq!(edited.config["target_port"], 4243);

        let off = store.set_interface_enabled(&row.id, false).unwrap();
        assert!(!off.enabled);

        store.delete_interface(&row.id).unwrap();
        assert!(store.list_interfaces().unwrap().is_empty());
    }

    #[test]
    fn extra_config_round_trips_in_order() {
        let store = Store::open_in_memory().unwrap();
        let blocks = vec![
            ("transport".to_string(), "[transport]\n  enabled = Yes\n".to_string()),
            ("logging".to_string(), "[logging]\n  loglevel = 3\n".to_string()),
        ];
        store.replace_extra_config(&blocks).unwrap();
        assert_eq!(store.extra_config().unwrap(), blocks);
    }

    // ---- first-run network setup and the legacy import -------------------

    fn legacy(auto_discovery: Option<bool>, relay: Option<&str>) -> LegacyNetwork {
        LegacyNetwork {
            auto_discovery,
            tcp_relay: relay.map(str::to_string),
        }
    }

    #[test]
    fn a_fresh_install_starts_with_local_discovery_and_imports_once() {
        let store = Store::open_in_memory().unwrap();
        let rows = store.import_network_once(None, false).unwrap();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.name, DEFAULT_DISCOVERY_NAME);
        assert_eq!(row.kind, "AutoInterface");
        assert!(row.enabled, "discovery is on from first run");
        assert_eq!(row.origin, "default");
        assert_eq!(row.config["group_id"], "reticulum");
        assert_eq!(
            store.meta(NETWORK_IMPORTED).unwrap().as_deref(),
            Some("1")
        );

        // A second start returns the same row instead of seeding another.
        let again = store.import_network_once(None, false).unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].id, row.id);
    }

    #[test]
    fn legacy_settings_become_legacy_rows_and_keep_discovery_on() {
        let store = Store::open_in_memory().unwrap();
        let rows = store
            .import_network_once(Some(&legacy(Some(true), Some("relay.example:4242"))), false)
            .unwrap();
        let by_name: std::collections::HashMap<_, _> =
            rows.iter().map(|row| (row.name.as_str(), row)).collect();
        let discovery = by_name.get(DEFAULT_DISCOVERY_NAME).expect("discovery row");
        assert!(discovery.enabled);
        assert_eq!(discovery.origin, "legacy");

        let relay = by_name.get(LEGACY_RELAY_NAME).expect("relay row");
        assert!(relay.enabled);
        assert_eq!(relay.kind, "TCPClientInterface");
        assert_eq!(relay.origin, "legacy");
        assert_eq!(relay.config["target_host"], "relay.example");
        assert_eq!(relay.config["target_port"], 4242);
        assert!(
            relay.note.as_deref().unwrap_or("").contains("old relay setting"),
            "the row is labelled as carried over, so it can be retired deliberately"
        );
    }

    #[test]
    fn a_malformed_legacy_relay_is_imported_disabled_with_the_reason() {
        let store = Store::open_in_memory().unwrap();
        let rows = store
            .import_network_once(Some(&legacy(Some(false), Some("relay.example"))), false)
            .unwrap();
        assert_eq!(rows.len(), 1, "discovery was explicitly off and there is no default");
        let relay = &rows[0];
        assert_eq!(relay.name, LEGACY_RELAY_NAME);
        assert!(!relay.enabled, "a value rchat cannot read must not be dialled");
        assert_eq!(relay.origin, "legacy");
        assert_eq!(
            relay.config["target_host"], "relay.example",
            "the user's text is kept so it can be fixed, not retyped"
        );
        let note = relay.note.as_deref().expect("the row explains itself");
        assert!(note.contains("no port"), "the reason names the problem: {note}");
    }

    #[test]
    fn legacy_import_never_overwrites_an_existing_row() {
        let store = Store::open_in_memory().unwrap();
        let existing = store
            .create_interface(
                LEGACY_RELAY_NAME,
                "TCPClientInterface",
                &serde_json::json!({"target_host": "mine.example", "target_port": 1111}),
                true,
                "managed",
            )
            .unwrap();
        let rows = store
            .import_network_once(Some(&legacy(Some(true), Some("relay.example:4242"))), false)
            .unwrap();
        assert_eq!(rows.len(), 1, "rows already exist, so the import is skipped");
        assert_eq!(rows[0].id, existing.id);
        assert_eq!(rows[0].config["target_host"], "mine.example");
        assert_eq!(store.meta(NETWORK_IMPORTED).unwrap().as_deref(), Some("1"));
    }

    #[test]
    fn a_present_rns_config_is_imported_before_anything_is_seeded() {
        let store = Store::open_in_memory().unwrap();
        let rows = store.import_network_once(None, true).unwrap();
        assert!(
            rows.is_empty(),
            "the hand-written config owns the interface list until it is read"
        );
        assert_eq!(
            store.meta(NETWORK_IMPORTED).unwrap(),
            None,
            "nothing is marked done before the config import runs"
        );

        // The bridge import creates rows; the next attempt settles the flag.
        store
            .create_interface(
                "Relay",
                "TCPClientInterface",
                &serde_json::json!({"target_host": "relay.example", "target_port": 4242}),
                true,
                "imported",
            )
            .unwrap();
        let rows = store.import_network_once(None, false).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].origin, "imported");
        assert_eq!(store.meta(NETWORK_IMPORTED).unwrap().as_deref(), Some("1"));
    }

    #[test]
    fn a_legacy_toggle_that_was_off_does_not_seed_discovery() {
        let store = Store::open_in_memory().unwrap();
        let rows = store
            .import_network_once(Some(&legacy(Some(false), None)), false)
            .unwrap();
        assert!(rows.is_empty(), "the user's choice to stay local-only is kept");
        assert_eq!(store.meta(NETWORK_IMPORTED).unwrap().as_deref(), Some("1"));
    }

    #[test]
    fn relay_strings_are_split_without_guessing() {
        assert_eq!(
            parse_relay("relay.example:4242").unwrap(),
            ("relay.example".into(), 4242)
        );
        assert_eq!(
            parse_relay(" 192.168.1.5:1234 ").unwrap(),
            ("192.168.1.5".into(), 1234)
        );
        assert_eq!(
            parse_relay("[fd00::1]:4242").unwrap(),
            ("fd00::1".into(), 4242)
        );
        for bad in ["", "relay.example", "relay.example:", "relay.example:0",
                    "relay.example:70000", "tcp://relay.example:4242",
                    "relay.example:port", "two words:42"] {
            assert!(parse_relay(bad).is_err(), "{bad:?} should be refused");
        }
    }
}
