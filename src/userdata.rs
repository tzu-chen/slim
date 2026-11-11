//! Personal reading data: bookmarks, reading trails (ordered stops with
//! notes, questions and reading states), standalone questions, per-project
//! preferences and the last session. Stored in the user data directory,
//! never in the inspected project, keyed by project identity (Lake package
//! name) and canonical declaration names so that they survive re-indexing
//! and moved checkouts.
//!
//! SQLite gives atomic, crash-safe writes; every multi-statement change runs
//! in an immediate transaction, so two running slim instances cannot
//! interleave half an update. The store's schema is versioned with
//! `PRAGMA user_version`; data written by a newer slim is refused, not
//! reinterpreted.

use crate::cli::Args;
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Version of the user-data schema (`PRAGMA user_version`). Version 1 had
/// one unnamed trail per project (table `trail`); it is migrated on open and
/// left in place.
pub const USER_SCHEMA_VERSION: i64 = 2;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS bookmarks (
  project TEXT NOT NULL, name TEXT NOT NULL, display TEXT NOT NULL, kind TEXT NOT NULL,
  module TEXT, path TEXT, line INTEGER, note TEXT, created INTEGER NOT NULL,
  PRIMARY KEY (project, name)
);
CREATE TABLE IF NOT EXISTS trail (
  project TEXT NOT NULL, seq INTEGER NOT NULL, name TEXT NOT NULL, display TEXT NOT NULL, kind TEXT NOT NULL,
  module TEXT, path TEXT, line INTEGER, note TEXT, created INTEGER NOT NULL,
  PRIMARY KEY (project, seq)
);
CREATE TABLE IF NOT EXISTS sessions (
  project TEXT PRIMARY KEY, state TEXT NOT NULL, updated INTEGER NOT NULL
);
"#;

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Mark {
    pub seq: i64,
    pub name: String,
    pub display: String,
    pub kind: String,
    pub module: Option<String>,
    pub path: Option<String>,
    pub line: Option<u32>,
    pub note: Option<String>,
    pub created: i64,
}


const SCHEMA_V2: &str = r#"
CREATE TABLE IF NOT EXISTS trails (
  id INTEGER PRIMARY KEY AUTOINCREMENT, project TEXT NOT NULL, title TEXT NOT NULL, description TEXT,
  origin TEXT NOT NULL DEFAULT 'personal', created INTEGER NOT NULL, updated INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS stops (
  id INTEGER PRIMARY KEY AUTOINCREMENT, trail_id INTEGER NOT NULL, pos INTEGER NOT NULL,
  ref_kind TEXT NOT NULL, name TEXT NOT NULL, display TEXT NOT NULL, kind TEXT NOT NULL,
  module TEXT, path TEXT, line INTEGER, note TEXT, question TEXT, state TEXT NOT NULL DEFAULT 'unread',
  sha256 TEXT, index_desc TEXT, created INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS stops_trail ON stops(trail_id, pos);
CREATE TABLE IF NOT EXISTS questions (
  id INTEGER PRIMARY KEY AUTOINCREMENT, project TEXT NOT NULL, ref_kind TEXT NOT NULL, name TEXT NOT NULL,
  display TEXT NOT NULL, text TEXT NOT NULL, resolved INTEGER NOT NULL DEFAULT 0, created INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS prefs (
  project TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY (project, key)
);
"#;

/// A reader's own annotation of a stop. None of these is a statement about
/// Lean: "understood" means the reader says so.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ReadState {
    #[default]
    Unread,
    Read,
    Understood,
    Revisit,
}

impl ReadState {
    pub fn as_str(self) -> &'static str {
        match self {
            ReadState::Unread => "unread",
            ReadState::Read => "read",
            ReadState::Understood => "understood",
            ReadState::Revisit => "revisit",
        }
    }
    pub fn parse(s: &str) -> ReadState {
        match s {
            "read" => ReadState::Read,
            "understood" => ReadState::Understood,
            "revisit" => ReadState::Revisit,
            _ => ReadState::Unread,
        }
    }
    pub fn next(self) -> ReadState {
        match self {
            ReadState::Unread => ReadState::Read,
            ReadState::Read => ReadState::Understood,
            ReadState::Understood => ReadState::Revisit,
            ReadState::Revisit => ReadState::Unread,
        }
    }
    /// Fixed-width tag for lists.
    pub fn tag(self) -> &'static str {
        match self {
            ReadState::Unread => "[      ]",
            ReadState::Read => "[read  ]",
            ReadState::Understood => "[unders]",
            ReadState::Revisit => "[revis.]",
        }
    }
    /// Counts as progress when resuming a trail.
    pub fn done(self) -> bool {
        matches!(self, ReadState::Read | ReadState::Understood)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RefKind {
    #[default]
    Decl,
    /// A group of the project map, by id.
    Group,
}

impl RefKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RefKind::Decl => "decl",
            RefKind::Group => "group",
        }
    }
    pub fn parse(s: &str) -> RefKind {
        if s == "group" {
            RefKind::Group
        } else {
            RefKind::Decl
        }
    }
}

/// One stop of a reading trail. `name` is the identity (a fully qualified
/// Lean name, or a map group id); path, line and fingerprint are locators
/// recorded when the stop was added.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Stop {
    pub id: i64,
    pub trail_id: i64,
    pub ref_kind: RefKind,
    pub name: String,
    pub display: String,
    pub kind: String,
    pub module: Option<String>,
    pub path: Option<String>,
    pub line: Option<u32>,
    pub note: Option<String>,
    pub question: Option<String>,
    pub state: ReadState,
    /// SHA-256 of the source file as the index recorded it when the stop was added.
    pub sha256: Option<String>,
    /// The index the stop was taken from ("source-only index", snapshot id, …).
    pub index_desc: Option<String>,
    pub created: i64,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Trail {
    pub id: i64,
    pub title: String,
    pub description: Option<String>,
    /// `personal`, or `shared:<id>` for a copy of a trail shipped with the map.
    pub origin: String,
    pub created: i64,
    pub updated: i64,
    pub stops: Vec<Stop>,
}

impl Trail {
    pub fn done(&self) -> usize {
        self.stops.iter().filter(|s| s.state.done()).count()
    }
    /// The first stop that is not read or understood.
    pub fn resume_at(&self) -> Option<usize> {
        self.stops.iter().position(|s| !s.state.done())
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Question {
    pub id: i64,
    pub ref_kind: RefKind,
    pub name: String,
    pub display: String,
    pub text: String,
    pub resolved: bool,
    pub created: i64,
}

/// How a stop relates to the index it is being shown or exported against.
#[derive(Clone, Debug, PartialEq)]
pub enum StopStatus {
    Current,
    /// Still resolves, but its source file changed since the stop was added.
    Stale(String),
    /// No longer resolves in this index.
    Unresolved(String),
    /// Not checked (no index at hand).
    Unknown,
}

pub struct UserStore {
    conn: Connection,
    pub path: PathBuf,
}

impl UserStore {
    pub fn default_path() -> PathBuf {
        crate::paths::data_dir().join("userdata.sqlite")
    }

    pub fn open() -> Result<UserStore> {
        Self::open_at(&Self::default_path())
    }

    pub fn open_at(path: &Path) -> Result<UserStore> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.busy_timeout(std::time::Duration::from_millis(2000))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(SCHEMA)?;
        let store = UserStore { conn, path: path.to_path_buf() };
        store.migrate()?;
        Ok(store)
    }

    pub fn is_bookmarked(&self, project: &str, name: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row("SELECT 1 FROM bookmarks WHERE project = ?1 AND name = ?2", params![project, name], |_| Ok(()))
            .optional()?
            .is_some())
    }

    /// Toggles a bookmark; returns whether it is now bookmarked.
    pub fn toggle_bookmark(&self, project: &str, m: &Mark) -> Result<bool> {
        if self.is_bookmarked(project, &m.name)? {
            self.conn.execute("DELETE FROM bookmarks WHERE project = ?1 AND name = ?2", params![project, m.name])?;
            return Ok(false);
        }
        self.conn.execute(
            "INSERT INTO bookmarks(project, name, display, kind, module, path, line, note, created)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![project, m.name, m.display, m.kind, m.module, m.path, m.line, m.note, crate::db::now_unix()],
        )?;
        Ok(true)
    }

    pub fn set_bookmark_note(&self, project: &str, name: &str, note: Option<&str>) -> Result<()> {
        self.conn.execute("UPDATE bookmarks SET note = ?3 WHERE project = ?1 AND name = ?2", params![project, name, note])?;
        Ok(())
    }

    pub fn bookmarks(&self, project: &str) -> Result<Vec<Mark>> {
        self.marks("SELECT 0, name, display, kind, module, path, line, note, created FROM bookmarks WHERE project = ?1 ORDER BY created, name", project)
    }

    pub fn bookmark_names(&self, project: &str) -> Result<Vec<String>> {
        let mut st = self.conn.prepare("SELECT name FROM bookmarks WHERE project = ?1")?;
        let v = st.query_map([project], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    pub fn save_session(&self, project: &str, state: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sessions(project, state, updated) VALUES (?1, ?2, ?3)
             ON CONFLICT(project) DO UPDATE SET state = excluded.state, updated = excluded.updated",
            params![project, state, crate::db::now_unix()],
        )?;
        Ok(())
    }

    pub fn load_session(&self, project: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT state FROM sessions WHERE project = ?1", [project], |r| r.get(0)).optional()?)
    }

    fn marks(&self, sql: &str, project: &str) -> Result<Vec<Mark>> {
        let mut st = self.conn.prepare(sql)?;
        let v = st
            .query_map([project], |r| {
                Ok(Mark {
                    seq: r.get(0)?,
                    name: r.get(1)?,
                    display: r.get(2)?,
                    kind: r.get(3)?,
                    module: r.get(4)?,
                    path: r.get(5)?,
                    line: r.get(6)?,
                    note: r.get(7)?,
                    created: r.get(8)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    pub fn projects(&self) -> Result<Vec<String>> {
        let mut st = self.conn.prepare(
            "SELECT project FROM bookmarks UNION SELECT project FROM trails UNION SELECT project FROM sessions ORDER BY 1",
        )?;
        let v = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }
}

fn md_escape_inline(s: &str) -> String {
    // Inline code spans cannot contain backticks safely; fall back to a
    // double-backtick span with padding when needed.
    if s.contains('`') {
        format!("`` {s} ``")
    } else {
        format!("`{s}`")
    }
}

/// Renders a reading trail as Markdown with stable identities (fully
/// qualified names) and source locations relative to the project root.
pub fn trail_markdown(project: &str, title: &str, marks: &[Mark], index_desc: &str) -> String {
    let mut s = String::new();
    s.push_str(&format!("# {title}\n\n"));
    s.push_str(&format!(
        "Project `{project}`. Exported {} by slim {} from {index_desc}.\n\n",
        crate::export::utc_now(),
        env!("CARGO_PKG_VERSION")
    ));
    s.push_str("Identities are fully qualified Lean names; locations are relative to the project root. ");
    s.push_str("The order is the reader's own and is not a dependency order.\n\n");
    if marks.is_empty() {
        s.push_str("_(empty)_\n");
    }
    for (i, m) in marks.iter().enumerate() {
        let loc = match (&m.path, m.line) {
            (Some(p), Some(l)) => format!(" — {}", md_escape_inline(&format!("{p}:{l}"))),
            (Some(p), None) => format!(" — {}", md_escape_inline(p)),
            _ => String::new(),
        };
        let module = m.module.as_deref().map(|x| format!(", module {}", md_escape_inline(x))).unwrap_or_default();
        s.push_str(&format!("{}. **{}** ({}){}{}\n", i + 1, md_escape_inline(&m.name), m.kind, loc, module));
        if let Some(n) = m.note.as_deref().filter(|n| !n.trim().is_empty()) {
            for line in n.lines() {
                s.push_str(&format!("   > {line}\n"));
            }
        }
    }
    s
}


const STOP_COLS: &str = "id, trail_id, ref_kind, name, display, kind, module, path, line, note, question, state, sha256, index_desc, created";

fn stop_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<Stop> {
    Ok(Stop {
        id: r.get(0)?,
        trail_id: r.get(1)?,
        ref_kind: RefKind::parse(&r.get::<_, String>(2)?),
        name: r.get(3)?,
        display: r.get(4)?,
        kind: r.get(5)?,
        module: r.get(6)?,
        path: r.get(7)?,
        line: r.get(8)?,
        note: r.get(9)?,
        question: r.get(10)?,
        state: ReadState::parse(&r.get::<_, String>(11)?),
        sha256: r.get(12)?,
        index_desc: r.get(13)?,
        created: r.get(14)?,
    })
}

/// Trails, questions and preferences (schema version 2).
impl UserStore {
    fn tx(&self) -> Result<rusqlite::Transaction<'_>> {
        Ok(rusqlite::Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?)
    }

    /// Brings the store to the current schema. Data from a newer slim is
    /// refused; version-1 trails are copied into named trails (the old table
    /// is left in place, so an older slim still sees its data).
    fn migrate(&self) -> Result<()> {
        let v: i64 = self.conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if v > USER_SCHEMA_VERSION {
            bail!(
                "{} was written by a newer slim (user-data schema {v}, this version reads up to {USER_SCHEMA_VERSION}); it was not changed",
                self.path.display()
            );
        }
        if v == USER_SCHEMA_VERSION {
            return Ok(());
        }
        let tx = self.tx()?;
        // Another instance may have migrated while we waited for the lock.
        let v: i64 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if v < USER_SCHEMA_VERSION {
            tx.execute_batch(SCHEMA_V2)?;
            let projects: Vec<String> = {
                let mut st = tx.prepare("SELECT DISTINCT project FROM trail ORDER BY project")?;
                let v = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
                v
            };
            let now = crate::db::now_unix();
            for p in projects {
                tx.execute(
                    "INSERT INTO trails(project, title, description, origin, created, updated) VALUES (?1, 'Reading trail', NULL, 'personal', ?2, ?2)",
                    params![p, now],
                )?;
                let tid = tx.last_insert_rowid();
                tx.execute(
                    "INSERT INTO stops(trail_id, pos, ref_kind, name, display, kind, module, path, line, note, state, created)
                     SELECT ?1, seq, 'decl', name, display, kind, module, path, line, note, 'unread', created FROM trail WHERE project = ?2 ORDER BY seq",
                    params![tid, p],
                )?;
            }
            tx.pragma_update(None, "user_version", USER_SCHEMA_VERSION)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn trails(&self, project: &str) -> Result<Vec<Trail>> {
        let mut st = self.conn.prepare("SELECT id, title, description, origin, created, updated FROM trails WHERE project = ?1 ORDER BY id")?;
        let mut trails: Vec<Trail> = st
            .query_map([project], |r| {
                Ok(Trail { id: r.get(0)?, title: r.get(1)?, description: r.get(2)?, origin: r.get(3)?, created: r.get(4)?, updated: r.get(5)?, stops: Vec::new() })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut st = self.conn.prepare(&format!("SELECT {STOP_COLS} FROM stops WHERE trail_id = ?1 ORDER BY pos, id"))?;
        for t in trails.iter_mut() {
            t.stops = st.query_map([t.id], stop_from)?.collect::<rusqlite::Result<_>>()?;
        }
        Ok(trails)
    }

    pub fn trail(&self, project: &str, id: i64) -> Result<Option<Trail>> {
        Ok(self.trails(project)?.into_iter().find(|t| t.id == id))
    }

    fn owns(&self, project: &str, trail_id: i64) -> Result<()> {
        let ok = self.conn.query_row("SELECT 1 FROM trails WHERE id = ?1 AND project = ?2", params![trail_id, project], |_| Ok(())).optional()?;
        if ok.is_none() {
            bail!("trail #{trail_id} does not exist any more (deleted in another slim instance?)");
        }
        Ok(())
    }

    pub fn trail_create(&self, project: &str, title: &str, description: Option<&str>, origin: &str) -> Result<i64> {
        let title = title.trim();
        if title.is_empty() {
            bail!("a trail needs a title");
        }
        let now = crate::db::now_unix();
        self.conn.execute(
            "INSERT INTO trails(project, title, description, origin, created, updated) VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![project, title, description, origin, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn trail_rename(&self, project: &str, id: i64, title: &str) -> Result<()> {
        self.owns(project, id)?;
        if title.trim().is_empty() {
            bail!("a trail needs a title");
        }
        self.conn.execute("UPDATE trails SET title = ?2, updated = ?3 WHERE id = ?1", params![id, title.trim(), crate::db::now_unix()])?;
        Ok(())
    }

    pub fn trail_delete(&self, project: &str, id: i64) -> Result<()> {
        self.owns(project, id)?;
        let tx = self.tx()?;
        tx.execute("DELETE FROM stops WHERE trail_id = ?1", [id])?;
        tx.execute("DELETE FROM trails WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(())
    }

    /// Appends a stop; returns its id.
    pub fn stop_add(&self, project: &str, trail_id: i64, s: &Stop) -> Result<i64> {
        self.owns(project, trail_id)?;
        let tx = self.tx()?;
        let pos: i64 = tx.query_row("SELECT coalesce(max(pos), 0) + 1 FROM stops WHERE trail_id = ?1", [trail_id], |r| r.get(0))?;
        let now = crate::db::now_unix();
        tx.execute(
            "INSERT INTO stops(trail_id, pos, ref_kind, name, display, kind, module, path, line, note, question, state, sha256, index_desc, created)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![trail_id, pos, s.ref_kind.as_str(), s.name, s.display, s.kind, s.module, s.path, s.line, s.note, s.question, s.state.as_str(), s.sha256, s.index_desc, now],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute("UPDATE trails SET updated = ?2 WHERE id = ?1", params![trail_id, now])?;
        tx.commit()?;
        Ok(id)
    }

    fn stop_trail(&self, project: &str, stop_id: i64) -> Result<i64> {
        self.conn
            .query_row("SELECT s.trail_id FROM stops s JOIN trails t ON t.id = s.trail_id WHERE s.id = ?1 AND t.project = ?2", params![stop_id, project], |r| r.get(0))
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("that stop does not exist any more (removed in another slim instance?)"))
    }

    pub fn stop_set_note(&self, project: &str, stop_id: i64, note: Option<&str>) -> Result<()> {
        self.stop_trail(project, stop_id)?;
        self.conn.execute("UPDATE stops SET note = ?2 WHERE id = ?1", params![stop_id, note])?;
        Ok(())
    }

    pub fn stop_set_question(&self, project: &str, stop_id: i64, question: Option<&str>) -> Result<()> {
        self.stop_trail(project, stop_id)?;
        self.conn.execute("UPDATE stops SET question = ?2 WHERE id = ?1", params![stop_id, question])?;
        Ok(())
    }

    pub fn stop_set_state(&self, project: &str, stop_id: i64, state: ReadState) -> Result<()> {
        self.stop_trail(project, stop_id)?;
        self.conn.execute("UPDATE stops SET state = ?2 WHERE id = ?1", params![stop_id, state.as_str()])?;
        Ok(())
    }

    pub fn stop_remove(&self, project: &str, stop_id: i64) -> Result<()> {
        self.stop_trail(project, stop_id)?;
        self.conn.execute("DELETE FROM stops WHERE id = ?1", [stop_id])?;
        Ok(())
    }

    /// Moves a stop up (`delta < 0`) or down within its trail. The whole
    /// trail is renumbered in one transaction.
    pub fn stop_move(&self, project: &str, stop_id: i64, delta: i64) -> Result<bool> {
        let trail_id = self.stop_trail(project, stop_id)?;
        let tx = self.tx()?;
        let mut ids: Vec<i64> = {
            let mut st = tx.prepare("SELECT id FROM stops WHERE trail_id = ?1 ORDER BY pos, id")?;
            let v = st.query_map([trail_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            v
        };
        let Some(i) = ids.iter().position(|&x| x == stop_id) else { return Ok(false) };
        let j = (i as i64 + delta).clamp(0, ids.len() as i64 - 1) as usize;
        if i == j {
            return Ok(false);
        }
        let x = ids.remove(i);
        ids.insert(j, x);
        for (k, id) in ids.iter().enumerate() {
            tx.execute("UPDATE stops SET pos = ?2 WHERE id = ?1", params![id, k as i64 + 1])?;
        }
        tx.commit()?;
        Ok(true)
    }

    pub fn questions(&self, project: &str) -> Result<Vec<Question>> {
        let mut st = self.conn.prepare("SELECT id, ref_kind, name, display, text, resolved, created FROM questions WHERE project = ?1 ORDER BY resolved, id")?;
        let v = st
            .query_map([project], |r| {
                Ok(Question {
                    id: r.get(0)?,
                    ref_kind: RefKind::parse(&r.get::<_, String>(1)?),
                    name: r.get(2)?,
                    display: r.get(3)?,
                    text: r.get(4)?,
                    resolved: r.get::<_, i64>(5)? != 0,
                    created: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    pub fn question_add(&self, project: &str, ref_kind: RefKind, name: &str, display: &str, text: &str) -> Result<i64> {
        if text.trim().is_empty() {
            bail!("an empty question was not saved");
        }
        self.conn.execute(
            "INSERT INTO questions(project, ref_kind, name, display, text, resolved, created) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)",
            params![project, ref_kind.as_str(), name, display, text.trim(), crate::db::now_unix()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Toggles a question between open and resolved; returns the new state.
    pub fn question_toggle(&self, project: &str, id: i64) -> Result<bool> {
        let n = self.conn.execute("UPDATE questions SET resolved = 1 - resolved WHERE id = ?1 AND project = ?2", params![id, project])?;
        if n == 0 {
            bail!("that question does not exist any more");
        }
        Ok(self.conn.query_row("SELECT resolved FROM questions WHERE id = ?1", [id], |r| r.get::<_, i64>(0))? != 0)
    }

    pub fn question_remove(&self, project: &str, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM questions WHERE id = ?1 AND project = ?2", params![id, project])?;
        Ok(())
    }

    pub fn pref(&self, project: &str, key: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM prefs WHERE project = ?1 AND key = ?2", params![project, key], |r| r.get(0)).optional()?)
    }

    pub fn set_pref(&self, project: &str, key: &str, value: Option<&str>) -> Result<()> {
        match value {
            Some(v) => self.conn.execute(
                "INSERT INTO prefs(project, key, value) VALUES (?1, ?2, ?3) ON CONFLICT(project, key) DO UPDATE SET value = excluded.value",
                params![project, key, v],
            )?,
            None => self.conn.execute("DELETE FROM prefs WHERE project = ?1 AND key = ?2", params![project, key])?,
        };
        Ok(())
    }

    pub fn prefs(&self, project: &str) -> Result<HashMap<String, String>> {
        let mut st = self.conn.prepare("SELECT key, value FROM prefs WHERE project = ?1")?;
        let v = st.query_map([project], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    /// Copies a trail shipped with the project map into the reader's own
    /// data (once: an existing copy is returned instead).
    pub fn start_shared_trail(&self, project: &str, shared: &crate::map::SharedTrail, stops: &[Stop]) -> Result<(i64, bool)> {
        let origin = format!("shared:{}", shared.id);
        if let Some(id) = self.conn.query_row("SELECT id FROM trails WHERE project = ?1 AND origin = ?2", params![project, origin], |r| r.get(0)).optional()? {
            return Ok((id, false));
        }
        let id = self.trail_create(project, &shared.title, shared.description.as_deref(), &origin)?;
        for s in stops {
            self.stop_add(project, id, s)?;
        }
        Ok((id, true))
    }
}

/// Checks every stop of a trail against an index (and the map, for group stops).
pub fn stop_statuses(conn: &Connection, map: Option<&crate::map::ProjectMap>, trail: &Trail) -> HashMap<i64, StopStatus> {
    let mut out = HashMap::new();
    for s in &trail.stops {
        let st = match s.ref_kind {
            RefKind::Group => match map {
                Some(m) if m.group_by_id(&s.name).is_some() => StopStatus::Current,
                Some(_) => StopStatus::Unresolved("this group is no longer in the project map".into()),
                None => StopStatus::Unresolved("no project map is loaded, so the group cannot be shown".into()),
            },
            RefKind::Decl => match crate::map::resolve_name(conn, &s.name) {
                Ok(Some((row, _, _))) if row.origin == crate::model::Origin::Unresolved => {
                    StopStatus::Unresolved("the index only refers to this name; it has no declaration for it".into())
                }
                Ok(Some((_, _, sha))) => match (&s.sha256, &sha) {
                    (Some(a), Some(b)) if a != b => StopStatus::Stale("its source file changed since this stop was added".into()),
                    _ => StopStatus::Current,
                },
                Ok(None) => StopStatus::Unresolved("not in this index (renamed or removed?)".into()),
                Err(_) => StopStatus::Unknown,
            },
        };
        out.insert(s.id, st);
    }
    out
}

/// What an exported trail is pinned to.
#[derive(Clone, Debug, Default)]
pub struct ExportInfo {
    pub project: String,
    /// Commit read from `.git` as text, if any. It does not say whether the
    /// worktree was clean; the per-stop file fingerprints do that job.
    pub revision: Option<String>,
    pub index_desc: String,
    pub statuses: HashMap<i64, StopStatus>,
}

/// Renders a trail as Markdown: project identity, source revision, stable
/// identities, the reader's notes, questions and states, and stale or
/// unresolved references called out.
pub fn trail_export_markdown(t: &Trail, info: &ExportInfo) -> String {
    let mut s = String::new();
    s.push_str(&format!("# {}\n\n", t.title.replace('\n', " ")));
    if let Some(d) = t.description.as_deref().filter(|d| !d.trim().is_empty()) {
        s.push_str(&format!("{}\n\n", d.trim()));
    }
    s.push_str(&format!("- Project: `{}`\n", info.project));
    s.push_str(&match &info.revision {
        Some(r) => format!("- Source revision: git `{r}` (read from `.git`; uncommitted changes are not detected, see the per-stop fingerprints)\n"),
        None => "- Source revision: unknown (no git metadata found)\n".to_string(),
    });
    s.push_str(&format!("- Index: {}\n", info.index_desc));
    s.push_str(&format!("- Exported {} by slim {}", crate::export::utc_now(), env!("CARGO_PKG_VERSION")));
    if let Some(id) = t.origin.strip_prefix("shared:") {
        s.push_str(&format!("; started from the shared trail `{id}` of the project map"));
    }
    s.push_str("\n\n");
    s.push_str("Identities are fully qualified Lean names (or map group ids); paths and lines are locators. ");
    s.push_str("The order is the reader's own and is not a dependency order. ");
    s.push_str("States (unread, read, understood, revisit) are the reader's annotations, not Lean verification.\n\n");
    let problems: Vec<&Stop> = t.stops.iter().filter(|x| matches!(info.statuses.get(&x.id), Some(StopStatus::Stale(_)) | Some(StopStatus::Unresolved(_)))).collect();
    if !problems.is_empty() {
        s.push_str(&format!("**{} of {} stops need attention** (stale or unresolved references are marked below).\n\n", problems.len(), t.stops.len()));
    }
    if t.stops.is_empty() {
        s.push_str("_(empty)_\n");
    }
    for (i, m) in t.stops.iter().enumerate() {
        let loc = match (&m.path, m.line) {
            (Some(p), Some(l)) => format!(" — {}", md_escape_inline(&format!("{p}:{l}"))),
            (Some(p), None) => format!(" — {}", md_escape_inline(p)),
            _ => String::new(),
        };
        let module = m.module.as_deref().map(|x| format!(", module {}", md_escape_inline(x))).unwrap_or_default();
        let what = if m.ref_kind == RefKind::Group { "map group".to_string() } else { m.kind.clone() };
        s.push_str(&format!("{}. **{}** ({what}){loc}{module} · state: {}\n", i + 1, md_escape_inline(&m.name), m.state.as_str()));
        if m.ref_kind == RefKind::Group && m.display != m.name {
            s.push_str(&format!("   - Group title: {}\n", m.display.replace('\n', " ")));
        }
        if let Some(n) = m.note.as_deref().filter(|n| !n.trim().is_empty()) {
            for line in n.lines() {
                s.push_str(&format!("   > {line}\n"));
            }
        }
        if let Some(q) = m.question.as_deref().filter(|q| !q.trim().is_empty()) {
            s.push_str(&format!("   - Question: {}\n", q.replace('\n', " ")));
        }
        if m.name.starts_with("_private.") {
            s.push_str("   - Private declaration: this name embeds its module path and is only stable while the file is not moved.\n");
        }
        match info.statuses.get(&m.id) {
            Some(StopStatus::Stale(why)) => s.push_str(&format!("   - **STALE**: {why}\n")),
            Some(StopStatus::Unresolved(why)) => s.push_str(&format!("   - **UNRESOLVED**: {why}\n")),
            Some(StopStatus::Unknown) | None => s.push_str("   - not checked against an index\n"),
            Some(StopStatus::Current) => {}
        }
        if let Some(h) = &m.sha256 {
            s.push_str(&format!("   - file fingerprint when added: sha256 `{}`\n", &h[..h.len().min(16)]));
        }
    }
    s
}

fn project_from_args(args: &Args) -> Result<(String, Option<crate::paths::Project>)> {
    if let Some(dir) = args.positional.get(1).or(args.positional.first()) {
        if Path::new(dir).is_dir() {
            let p = crate::paths::Project::open(Path::new(dir))?;
            return Ok((p.user_key(), Some(p)));
        }
    }
    if let Some(p) = args.get("project") {
        return Ok((if p.contains(':') { p.to_string() } else { format!("lake:{p}") }, None));
    }
    let store = UserStore::open()?;
    let ps = store.projects()?;
    match ps.as_slice() {
        [one] => Ok((one.clone(), None)),
        [] => anyhow::bail!("no reading data yet"),
        many => anyhow::bail!("several projects have reading data; pass --project NAME ({})", many.join(", ")),
    }
}

/// `slim trail list|export [PROJECT_DIR | --project NAME] [--trail ID|TITLE] [-o FILE.md]`
pub fn cmd_trail(args: &Args) -> Result<i32> {
    let sub = args.positional.first().map(|s| s.as_str()).unwrap_or("export");
    if !matches!(sub, "export" | "show" | "list") {
        anyhow::bail!("usage: slim trail list|export [PROJECT_DIR | --project NAME] [--trail ID|TITLE] [-o FILE.md]");
    }
    let (project, proj) = project_from_args(args)?;
    let store = UserStore::open()?;
    let trails = store.trails(&project)?;
    if sub == "list" {
        for t in &trails {
            println!("#{}\t{}\t{} stops, {} read or understood\t{}", t.id, t.title, t.stops.len(), t.done(), t.origin);
        }
        if trails.is_empty() {
            println!("no trails for {project}");
        }
        return Ok(0);
    }
    let t = match args.get("trail") {
        Some(sel) => trails.iter().find(|t| t.id.to_string() == sel || t.title == sel).with_context(|| format!("no trail {sel:?} for {project} (see `slim trail list`)"))?,
        None => match trails.as_slice() {
            [one] => one,
            [] => anyhow::bail!("no trails for {project}"),
            _ => anyhow::bail!("several trails exist; pass --trail ID or --trail TITLE (see `slim trail list`)"),
        },
    };
    let mut info = ExportInfo { project: project.clone(), index_desc: "not checked against an index (pass the project directory to check)".into(), ..Default::default() };
    if let Some(p) = &proj {
        info.revision = crate::paths::git_revision(&p.root);
        let (idx, desc) = if p.semantic_index().exists() { (p.semantic_index(), "semantic snapshot") } else { (p.source_index(), "source-only index") };
        if let Ok(conn) = crate::db::open_read(&idx) {
            let map = crate::map::load_for_root(&p.root);
            info.statuses = stop_statuses(&conn, map.map().map(|m| m.as_ref()), t);
            info.index_desc = format!("the project's {desc}");
        }
    }
    let md = trail_export_markdown(t, &info);
    match args.path("o").or(args.path("out")) {
        Some(p) => {
            write_atomic(&p, md.as_bytes())?;
            println!("wrote {} stops to {}", t.stops.len(), p.display());
        }
        None => print!("{md}"),
    }
    Ok(0)
}

/// Writes through a temporary file in the same directory and renames it
/// into place, so a reader never sees half a file.
pub fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path.file_name().with_context(|| format!("{} is not a file path", path.display()))?.to_string_lossy().to_string();
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let r = (|| -> Result<()> {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
        f.write_all(data)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

pub fn cmd_bookmarks(args: &Args) -> Result<i32> {
    let (project, _) = project_from_args(args)?;
    let store = UserStore::open()?;
    let marks = store.bookmarks(&project)?;
    print!("{}", trail_markdown(&project, "Bookmarks", &marks, "the user's slim data"));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(name: &str) -> Mark {
        Mark {
            name: name.into(),
            display: name.into(),
            kind: "theorem".into(),
            module: Some("M".into()),
            path: Some("M.lean".into()),
            line: Some(3),
            ..Default::default()
        }
    }

    fn stop(name: &str) -> Stop {
        Stop { name: name.into(), display: name.into(), kind: "theorem".into(), module: Some("M".into()), path: Some("M.lean".into()), line: Some(3), ..Default::default() }
    }

    #[test]
    fn bookmarks_and_sessions_persist() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("u.sqlite");
        {
            let s = UserStore::open_at(&p).unwrap();
            assert!(s.toggle_bookmark("lake:x", &mark("A.b")).unwrap());
            s.set_bookmark_note("lake:x", "A.b", Some("key lemma")).unwrap();
            s.save_session("lake:x", "{\"current\":\"A.b\"}").unwrap();
        }
        let s = UserStore::open_at(&p).unwrap();
        assert!(s.is_bookmarked("lake:x", "A.b").unwrap());
        assert!(!s.is_bookmarked("lake:y", "A.b").unwrap());
        assert_eq!(s.bookmarks("lake:x").unwrap()[0].note.as_deref(), Some("key lemma"));
        let md = trail_markdown("lake:x", "Bookmarks", &s.bookmarks("lake:x").unwrap(), "test");
        assert!(md.contains("1. **`A.b`** (theorem) — `M.lean:3`, module `M`"));
        assert_eq!(s.load_session("lake:x").unwrap().as_deref(), Some("{\"current\":\"A.b\"}"));
        assert!(!s.toggle_bookmark("lake:x", &mark("A.b")).unwrap());
    }

    #[test]
    fn trails_keep_order_notes_questions_and_states_across_reopen() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("u.sqlite");
        let (t1, ids) = {
            let s = UserStore::open_at(&p).unwrap();
            let t1 = s.trail_create("lake:x", "Main argument", Some("why it holds"), "personal").unwrap();
            let t2 = s.trail_create("lake:x", "Introduction", None, "personal").unwrap();
            assert_ne!(t1, t2);
            let a = s.stop_add("lake:x", t1, &Stop { note: Some("start here\nthen this".into()), ..stop("A.b") }).unwrap();
            let b = s.stop_add("lake:x", t1, &stop("A.`c")).unwrap();
            let c = s.stop_add("lake:x", t1, &Stop { ref_kind: RefKind::Group, kind: "group".into(), ..stop("closure") }).unwrap();
            s.stop_set_question("lake:x", b, Some("why is fuel needed?")).unwrap();
            s.stop_set_state("lake:x", a, ReadState::Understood).unwrap();
            assert!(s.stop_move("lake:x", c, -2).unwrap());
            assert!(!s.stop_move("lake:x", c, -1).unwrap(), "already first");
            // Another project cannot touch these.
            assert!(s.stop_add("lake:y", t1, &stop("Z")).is_err());
            assert!(s.stop_set_state("lake:y", a, ReadState::Read).is_err());
            (t1, [a, b, c])
        };
        let s = UserStore::open_at(&p).unwrap();
        let ts = s.trails("lake:x").unwrap();
        assert_eq!(ts.len(), 2);
        let t = ts.iter().find(|t| t.id == t1).unwrap();
        assert_eq!(t.stops.iter().map(|x| x.id).collect::<Vec<_>>(), [ids[2], ids[0], ids[1]]);
        assert_eq!(t.stops[1].state, ReadState::Understood);
        assert_eq!(t.stops[2].question.as_deref(), Some("why is fuel needed?"));
        assert_eq!(t.stops[0].ref_kind, RefKind::Group);
        assert_eq!(t.done(), 1);
        assert_eq!(t.resume_at(), Some(0));
        let mut info = ExportInfo { project: "lake:x".into(), revision: Some("abc123".into()), index_desc: "test index".into(), ..Default::default() };
        info.statuses.insert(ids[0], StopStatus::Current);
        info.statuses.insert(ids[1], StopStatus::Unresolved("not in this index (renamed or removed?)".into()));
        info.statuses.insert(ids[2], StopStatus::Stale("its source file changed since this stop was added".into()));
        let md = trail_export_markdown(t, &info);
        assert!(md.contains("# Main argument") && md.contains("- Project: `lake:x`") && md.contains("git `abc123`"));
        assert!(md.contains("1. **`closure`** (map group)"));
        assert!(md.contains("2. **`A.b`** (theorem) — `M.lean:3`, module `M` · state: understood"));
        assert!(md.contains("   > start here\n   > then this"));
        assert!(md.contains("3. **`` A.`c ``**") && md.contains("   - Question: why is fuel needed?") && md.contains("**UNRESOLVED**"));
        assert!(md.contains("**STALE**") && md.contains("**2 of 3 stops need attention**"));
        assert!(md.contains("not Lean verification"));
        s.stop_remove("lake:x", ids[0]).unwrap();
        s.trail_delete("lake:x", t1).unwrap();
        assert_eq!(s.trails("lake:x").unwrap().len(), 1);
        assert!(s.stop_set_note("lake:x", ids[1], Some("x")).is_err(), "stops of a deleted trail are gone");
    }

    #[test]
    fn version_one_trails_are_migrated_and_newer_stores_refused() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("u.sqlite");
        {
            // A store as written by milestone 1.
            let c = Connection::open(&p).unwrap();
            c.execute_batch(SCHEMA).unwrap();
            c.execute_batch(
                "INSERT INTO trail(project, seq, name, display, kind, module, path, line, note, created) VALUES
                   ('lake:x', 1, 'A.b', 'A.b', 'theorem', 'M', 'M.lean', 3, 'old note', 5),
                   ('lake:x', 2, 'A.c', 'A.c', 'def', NULL, NULL, NULL, NULL, 6),
                   ('lake:y', 1, 'B', 'B', 'def', NULL, NULL, NULL, NULL, 7);",
            )
            .unwrap();
        }
        for _ in 0..2 {
            // Opening twice migrates once.
            let s = UserStore::open_at(&p).unwrap();
            let ts = s.trails("lake:x").unwrap();
            assert_eq!(ts.len(), 1);
            assert_eq!(ts[0].title, "Reading trail");
            assert_eq!(ts[0].stops.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["A.b", "A.c"]);
            assert_eq!(ts[0].stops[0].note.as_deref(), Some("old note"));
            assert_eq!(s.trails("lake:y").unwrap()[0].stops.len(), 1);
        }
        // The version-1 table is still there for an older slim.
        let c = Connection::open(&p).unwrap();
        assert_eq!(c.query_row("SELECT count(*) FROM trail", [], |r| r.get::<_, i64>(0)).unwrap(), 3);
        c.pragma_update(None, "user_version", 99).unwrap();
        drop(c);
        let e = UserStore::open_at(&p).err().expect("newer schema refused").to_string();
        assert!(e.contains("newer slim"), "{e}");
        let c = Connection::open(&p).unwrap();
        assert_eq!(c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), 99, "left untouched");
    }

    #[test]
    fn two_instances_do_not_lose_each_others_writes() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("u.sqlite");
        let a = UserStore::open_at(&p).unwrap();
        let b = UserStore::open_at(&p).unwrap();
        let t = a.trail_create("lake:x", "T", None, "personal").unwrap();
        let mut ids = Vec::new();
        for i in 0..20 {
            let s = if i % 2 == 0 { &a } else { &b };
            ids.push(s.stop_add("lake:x", t, &stop(&format!("N{i}"))).unwrap());
        }
        a.stop_move("lake:x", ids[19], -19).unwrap();
        b.stop_set_state("lake:x", ids[3], ReadState::Revisit).unwrap();
        let seen = b.trail("lake:x", t).unwrap().unwrap();
        assert_eq!(seen.stops.len(), 20, "interleaved appends all landed, each at its own position");
        assert_eq!(seen.stops[0].id, ids[19]);
        assert_eq!(a.trail("lake:x", t).unwrap().unwrap().stops.iter().find(|s| s.id == ids[3]).unwrap().state, ReadState::Revisit);
        // One instance deletes the trail; the other gets an error, not a silent no-op.
        a.trail_delete("lake:x", t).unwrap();
        assert!(b.stop_add("lake:x", t, &stop("late")).is_err());
        // Preferences and questions.
        a.set_pref("lake:x", "goal", Some("A.b")).unwrap();
        assert_eq!(b.pref("lake:x", "goal").unwrap().as_deref(), Some("A.b"));
        a.set_pref("lake:x", "goal", None).unwrap();
        assert!(b.prefs("lake:x").unwrap().is_empty());
        let q = a.question_add("lake:x", RefKind::Decl, "A.b", "A.b", "why?").unwrap();
        assert!(b.question_toggle("lake:x", q).unwrap());
        assert!(b.questions("lake:x").unwrap()[0].resolved);
        assert!(a.question_add("lake:x", RefKind::Decl, "A.b", "A.b", "  ").is_err());
    }

    #[test]
    fn atomic_write_replaces_whole_files() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("out.md");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "two");
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1, "no temporary file left behind");
        assert!(write_atomic(&d.path().join("missing/dir/x.md"), b"x").is_err());
    }
}
