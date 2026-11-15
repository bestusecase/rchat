//! SQLite persistence.
//!
//! One connection guarded by a mutex is plenty for a single-user desktop app:
//! all writes are small and the daemon is the only writer. Everything the UI
//! shows is durable here, including a monotonic event log so a reconnecting
//! client can replay instead of refetching.

use anyhow::{Context, Result};
use rchat_ipc::{Caps, ConversationInfo, MessageInfo, PeerInfo};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::Mutex;

pub struct Store {
    conn: Mutex<Connection>,
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
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        let store = Self {
            conn: Mutex::new(conn),
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
            "#,
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO meta(key, value) VALUES ('schema', '1')",
            [],
        )?;
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
        let (kind, title, created_at, last_message_at, unread, preview): (
            String,
            Option<String>,
            i64,
            Option<i64>,
            i64,
            Option<String>,
        ) = {
            let conn = self.conn.lock().unwrap();
            conn.query_row(
                "SELECT c.kind, c.title, c.created_at, c.last_message_at, c.unread,
                        (SELECT m.body FROM messages m
                         WHERE m.conversation_id = c.id
                         ORDER BY m.ts DESC, m.rowid DESC LIMIT 1)
                 FROM conversations c WHERE c.id = ?1",
                [id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
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
            preview,
            peer_ids: self.members(id)?,
        })
    }

    pub fn list_conversations(&self) -> Result<Vec<ConversationInfo>> {
        let rows: Vec<(String, String, Option<String>, i64, Option<i64>, i64, Option<String>)> = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare(
                "SELECT c.id, c.kind, c.title, c.created_at, c.last_message_at, c.unread,
                        (SELECT m.body FROM messages m
                         WHERE m.conversation_id = c.id
                         ORDER BY m.ts DESC, m.rowid DESC LIMIT 1)
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
                        r.get(6)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            collected
        };

        let mut out = Vec::with_capacity(rows.len());
        for (id, kind, title, created_at, last_message_at, unread, preview) in rows {
            let peer_ids = self.members(&id)?;
            out.push(ConversationInfo {
                id,
                kind,
                title,
                created_at,
                last_message_at,
                unread,
                preview,
                peer_ids,
            });
        }
        Ok(out)
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

    pub fn get_message(&self, id: &str) -> Result<Option<MessageInfo>> {
        let conn = self.conn.lock().unwrap();
        let msg = conn
            .query_row(
                "SELECT id, conversation_id, author_id, ts, body, kind, reply_to, state, route, edited_at
                 FROM messages WHERE id = ?1",
                [id],
                map_message,
            )
            .optional()?;
        Ok(msg)
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
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, conversation_id, author_id, ts, body, kind, reply_to, state, route, edited_at
             FROM messages
             WHERE conversation_id = ?1 AND (?2 IS NULL OR ts < ?2)
             ORDER BY ts DESC, rowid DESC
             LIMIT ?3",
        )?;
        let mut rows = stmt
            .query_map(params![conversation_id, before, limit], map_message)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.reverse();
        Ok(rows)
    }

    /// Messages that still need to go out. Recomputed rather than stored in a
    /// separate outbox table so there is a single source of truth.
    pub fn queued_messages(&self) -> Result<Vec<MessageInfo>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, conversation_id, author_id, ts, body, kind, reply_to, state, route, edited_at
             FROM messages WHERE state = 'queued' ORDER BY ts ASC LIMIT 200",
        )?;
        let rows = stmt.query_map([], map_message)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<MessageInfo>> {
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
        Ok(rows)
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
}
