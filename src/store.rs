//! SQLite store: events and decisions, ordered by seq, times in UTC unix seconds.

use crate::config::Mode;
use crate::decide::{Kind, Sent};
use crate::mapping::{Alert, Status};
use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension, Params, Row, Statement};
use std::str::FromStr;
use std::sync::{Mutex, PoisonError};

pub struct Store {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub seq: i64,
    pub received_at: i64,
    pub alert: Alert,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecisionRow {
    pub event_seq: i64,
    pub decided_at: i64,
    pub mode: Mode,
    pub kind: Kind,
    pub probability: Option<f64>,
    pub reason: String,
    /// The team got it: a webhook that answered 2xx, or, for a digest, an entry queued.
    pub delivered: bool,
    pub backend_ms: Option<i64>,
    pub input_tokens: Option<i64>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS events (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  received_at INTEGER NOT NULL,
  source TEXT NOT NULL, identity TEXT NOT NULL, status TEXT NOT NULL,
  summary TEXT NOT NULL, details TEXT NOT NULL, env TEXT NOT NULL, severity TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS events_identity ON events(identity, seq);
CREATE TABLE IF NOT EXISTS decisions (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq),
  decided_at INTEGER NOT NULL, mode TEXT NOT NULL, kind TEXT NOT NULL,
  probability REAL, reason TEXT NOT NULL, delivered INTEGER NOT NULL,
  backend_ms INTEGER, input_tokens INTEGER);
CREATE TABLE IF NOT EXISTS digest_queue (event_seq INTEGER PRIMARY KEY REFERENCES events(seq));
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

/// Read by `event`, in this order; every query aliases `events` as `e`.
const EVENT_COLS: &str =
    "e.seq, e.received_at, e.source, e.identity, e.status, e.summary, e.details, e.env, e.severity";

fn event(r: &Row) -> rusqlite::Result<Event> {
    let status: String = r.get(4)?;
    Ok(Event {
        seq: r.get(0)?,
        received_at: r.get(1)?,
        alert: Alert {
            source: r.get(2)?,
            identity: r.get(3)?,
            status: Status::parse(&status).unwrap_or(Status::Firing),
            summary: r.get(5)?,
            details: r.get(6)?,
            env: r.get(7)?,
            severity: r.get(8)?,
        },
    })
}

/// Reads a column holding an enum's `as_str` name.
fn named<T: FromStr>(r: &Row, i: usize) -> rusqlite::Result<T>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let s: String = r.get(i)?;
    s.parse().map_err(|e| rusqlite::Error::FromSqlConversionFailure(i, Type::Text, Box::new(e)))
}

/// Runs a query that selects `EVENT_COLS`.
fn events(st: &mut Statement, p: impl Params) -> Vec<Event> {
    st.query_map(p, event).expect("query").map(|r| r.expect("row")).collect()
}

fn decision(r: &Row, o: usize) -> rusqlite::Result<DecisionRow> {
    Ok(DecisionRow {
        event_seq: r.get(o)?,
        decided_at: r.get(o + 1)?,
        mode: named(r, o + 2)?,
        kind: named(r, o + 3)?,
        probability: r.get(o + 4)?,
        reason: r.get(o + 5)?,
        delivered: r.get(o + 6)?,
        backend_ms: r.get(o + 7)?,
        input_tokens: r.get(o + 8)?,
    })
}

const DECISION_COLS: &str =
    "d.event_seq, d.decided_at, d.mode, d.kind, d.probability, d.reason, d.delivered, d.backend_ms, d.input_tokens";

impl Store {
    pub fn open(path: &str) -> rusqlite::Result<Store> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Store { conn: Mutex::new(conn) })
    }

    pub fn memory() -> Store {
        let conn = Connection::open_in_memory().expect("in-memory sqlite");
        conn.execute_batch(SCHEMA).expect("schema");
        Store { conn: Mutex::new(conn) }
    }

    /// A panic in an earlier call poisons the lock; the connection itself is still usable (an
    /// aborted statement is rolled back by SQLite), so recover it rather than failing every call.
    fn c(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn insert_event(&self, a: &Alert, received_at: i64) -> i64 {
        let c = self.c();
        c.execute(
            "INSERT INTO events (received_at, source, identity, status, summary, details, env, severity)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![received_at, a.source, a.identity, a.status.as_str(), a.summary, a.details, a.env, a.severity],
        )
        .expect("insert event");
        c.last_insert_rowid()
    }

    /// Returns the identity's events strictly after the last `resolved` event received before
    /// `since` (or from the very first event if there is none), and strictly before `before_seq`.
    /// An episode already open when the window starts is thus returned from its true first event,
    /// even if that event lies outside `[since, before_seq)`; see `history::facts`.
    pub fn events_for(&self, identity: &str, since: i64, before_seq: i64) -> Vec<Event> {
        let c = self.c();
        let mut st = c
            .prepare(&format!(
                "SELECT {EVENT_COLS} FROM events e
            WHERE e.identity = ?1 AND e.seq < ?2 AND e.seq > COALESCE(
              (SELECT MAX(seq) FROM events WHERE identity = ?1 AND status = 'resolved' AND received_at < ?3), 0)
            ORDER BY e.seq"
            ))
            .expect("prepare");
        events(&mut st, params![identity, before_seq, since])
    }

    pub fn events_since(&self, since: i64) -> Vec<Event> {
        let c = self.c();
        let mut st = c
            .prepare(&format!("SELECT {EVENT_COLS} FROM events e WHERE e.received_at >= ?1 ORDER BY e.seq"))
            .expect("prepare");
        events(&mut st, params![since])
    }

    pub fn insert_decision(&self, d: &DecisionRow) -> i64 {
        let c = self.c();
        c.execute("INSERT OR REPLACE INTO decisions (event_seq, decided_at, mode, kind, probability, reason, delivered, backend_ms, input_tokens)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![d.event_seq, d.decided_at, d.mode.as_str(), d.kind.as_str(), d.probability, d.reason, d.delivered, d.backend_ms, d.input_tokens])
            .expect("insert decision");
        c.last_insert_rowid()
    }

    pub fn decisions_since(&self, since: i64) -> Vec<(Event, DecisionRow)> {
        let c = self.c();
        let mut st = c
            .prepare(&format!(
                "SELECT {EVENT_COLS}, {DECISION_COLS} FROM decisions d JOIN events e ON e.seq = d.event_seq
            WHERE e.received_at >= ?1 ORDER BY e.seq"
            ))
            .expect("prepare");
        st.query_map(params![since], |r| Ok((event(r)?, decision(r, 9)?)))
            .expect("query")
            .map(|r| r.expect("row"))
            .collect()
    }

    /// The notifications the episode open at `before_seq` already sent (`delivered = 1`), for
    /// `decide::repeat_floor`. Only a digest entry older than `since` can still count, so older
    /// ones of other kinds are left out.
    pub fn episode_sent(&self, identity: &str, before_seq: i64, since: i64) -> Vec<Sent> {
        let c = self.c();
        let mut st = c
            .prepare(
                "SELECT d.decided_at, d.kind FROM decisions d JOIN events e ON e.seq = d.event_seq
             WHERE e.identity = ?1 AND e.seq < ?2 AND e.seq > COALESCE(
               (SELECT MAX(seq) FROM events WHERE identity = ?1 AND seq < ?2 AND status = 'resolved'), 0)
             AND d.kind IN ('ping', 'untriaged', 'escalate', 'digest') AND d.delivered = 1
             AND (d.decided_at >= ?3 OR d.kind = 'digest')
             ORDER BY d.seq",
            )
            .expect("prepare");
        st.query_map(params![identity, before_seq, since], |r| Ok(Sent { at: r.get(0)?, kind: named(r, 1)? }))
            .expect("query")
            .map(|r| r.expect("row"))
            .collect()
    }

    pub fn queue_digest(&self, event_seq: i64) {
        self.c()
            .execute("INSERT OR IGNORE INTO digest_queue (event_seq) VALUES (?1)", params![event_seq])
            .expect("queue");
    }

    pub fn take_digest(&self) -> Vec<Event> {
        let mut c = self.c();
        let tx = c.transaction().expect("tx");
        let evs = events(
            &mut tx
                .prepare(&format!(
                    "SELECT {EVENT_COLS} FROM digest_queue q JOIN events e ON e.seq = q.event_seq ORDER BY e.seq"
                ))
                .expect("prepare"),
            [],
        );
        tx.execute("DELETE FROM digest_queue", []).expect("clear");
        tx.commit().expect("commit");
        evs
    }

    pub fn meta_get(&self, key: &str) -> Option<String> {
        self.c()
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0))
            .optional()
            .expect("meta")
    }

    pub fn meta_set(&self, key: &str, value: &str) {
        self.c()
            .execute("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)", params![key, value])
            .expect("meta");
    }
}
