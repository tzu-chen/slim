//! The persistent index: one SQLite file per (project, mode).
//!
//! The index is derived data. Its schema is versioned with
//! [`SCHEMA_VERSION`]; an index with a different version is never migrated
//! in place on the UI path; it is rebuilt from its source (snapshot or
//! source tree) by an explicit background job.

pub mod import;
pub mod query;

use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::path::Path;

/// Version 2 added the per-file counts on `modules` (`n_thm` … `proof_lines`),
/// which the outline sums over directories.
pub const SCHEMA_VERSION: u32 = 2;

pub const SCHEMA: &str = r#"
CREATE TABLE meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
) WITHOUT ROWID;

CREATE TABLE modules (
  id            INTEGER PRIMARY KEY,
  name          TEXT NOT NULL UNIQUE,
  path          TEXT,
  is_local      INTEGER NOT NULL,
  sha256        TEXT,
  size          INTEGER,
  mtime_ns      INTEGER,
  status        TEXT NOT NULL DEFAULT 'ok',
  status_detail TEXT,
  doc           TEXT,
  decl_count    INTEGER NOT NULL DEFAULT 0,
  line_count    INTEGER,
  n_thm         INTEGER NOT NULL DEFAULT 0,
  n_def         INTEGER NOT NULL DEFAULT 0,
  n_type        INTEGER NOT NULL DEFAULT 0,
  n_inst        INTEGER NOT NULL DEFAULT 0,
  n_axiom       INTEGER NOT NULL DEFAULT 0,
  n_doc         INTEGER NOT NULL DEFAULT 0,
  n_sorry       INTEGER NOT NULL DEFAULT 0,
  proof_lines   INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE module_imports (
  src   INTEGER NOT NULL,
  dst   INTEGER NOT NULL,
  flags INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (src, dst)
) WITHOUT ROWID;

CREATE TABLE decls (
  id              INTEGER PRIMARY KEY,
  name            TEXT NOT NULL,
  display         TEXT NOT NULL,
  kind            INTEGER NOT NULL,
  module_id       INTEGER,
  origin          INTEGER NOT NULL,
  flags           INTEGER NOT NULL DEFAULT 0,
  type_cov        INTEGER NOT NULL,
  value_cov       INTEGER NOT NULL,
  start_line      INTEGER,
  start_col       INTEGER,
  end_line        INTEGER,
  end_col         INTEGER,
  sel_line        INTEGER,
  sel_col         INTEGER,
  start_byte      INTEGER,
  kw_byte         INTEGER,
  header_end_byte INTEGER,
  end_byte        INTEGER,
  type_text       TEXT,
  sig_json        TEXT,
  doc             TEXT,
  attrs           TEXT,
  generated       TEXT,
  keyword         TEXT,
  sorry_line      INTEGER,
  out_type        INTEGER NOT NULL DEFAULT 0,
  out_value       INTEGER NOT NULL DEFAULT 0,
  out_ctor        INTEGER NOT NULL DEFAULT 0,
  in_type         INTEGER NOT NULL DEFAULT 0,
  in_value        INTEGER NOT NULL DEFAULT 0,
  in_ctor         INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE edges (
  src  INTEGER NOT NULL,
  kind INTEGER NOT NULL,
  dst  INTEGER NOT NULL,
  PRIMARY KEY (src, kind, dst)
) WITHOUT ROWID;

CREATE TABLE axioms (
  id       INTEGER PRIMARY KEY,
  decl_id  INTEGER,
  name     TEXT NOT NULL UNIQUE,
  category INTEGER NOT NULL
);

CREATE TABLE decl_axioms (
  decl_id  INTEGER NOT NULL,
  axiom_id INTEGER NOT NULL,
  via      INTEGER,
  via_kind INTEGER,
  PRIMARY KEY (decl_id, axiom_id)
) WITHOUT ROWID;

CREATE TABLE lean_axioms (
  decl_id    INTEGER NOT NULL,
  axiom_name TEXT NOT NULL,
  PRIMARY KEY (decl_id, axiom_name)
) WITHOUT ROWID;

CREATE TABLE import_state (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
) WITHOUT ROWID;
"#;

/// Indexes created after bulk loading (faster than maintaining them during
/// inserts).
pub const POST_LOAD_INDEXES: &str = r#"
CREATE INDEX IF NOT EXISTS decls_name ON decls(name);
CREATE INDEX IF NOT EXISTS decls_module ON decls(module_id, start_line);
CREATE INDEX IF NOT EXISTS edges_rev ON edges(dst, kind, src);
CREATE INDEX IF NOT EXISTS module_imports_rev ON module_imports(dst, src);
CREATE INDEX IF NOT EXISTS decl_axioms_axiom ON decl_axioms(axiom_id, decl_id);
"#;

pub const FTS_SCHEMA: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS fts USING fts5(
  doc, attrs, sig,
  content='',
  contentless_delete=1,
  tokenize='unicode61 remove_diacritics 2',
  prefix='2 3'
);
"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexMode {
    Semantic,
    Source,
}

impl IndexMode {
    pub fn as_str(self) -> &'static str {
        match self {
            IndexMode::Semantic => "semantic",
            IndexMode::Source => "source",
        }
    }
    pub fn parse(s: &str) -> Option<IndexMode> {
        match s {
            "semantic" => Some(IndexMode::Semantic),
            "source" => Some(IndexMode::Source),
            _ => None,
        }
    }
}

fn tune(conn: &Connection, writable: bool) -> Result<()> {
    conn.pragma_update(None, "cache_size", -32000i64)?; // 32 MiB page cache
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    if writable {
        conn.pragma_update(None, "foreign_keys", false)?;
    }
    conn.busy_timeout(std::time::Duration::from_millis(2000))?;
    Ok(())
}

/// Opens an index read-only and checks its schema version.
pub fn open_read(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI,
    )
    .with_context(|| format!("opening index {}", path.display()))?;
    tune(&conn, false)?;
    check_schema(&conn)?;
    Ok(conn)
}

/// Opens an existing index for writing (incremental source updates).
pub fn open_write(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening index {} for writing", path.display()))?;
    tune(&conn, true)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    check_schema(&conn)?;
    Ok(conn)
}

/// Creates a fresh index file (the caller renames it into place when done).
pub fn create(path: &Path, mode: IndexMode) -> Result<Connection> {
    if path.exists() {
        std::fs::remove_file(path).with_context(|| format!("removing stale {}", path.display()))?;
    }
    for ext in ["-wal", "-shm", "-journal"] {
        let p = path.with_file_name(format!("{}{}", path.file_name().unwrap().to_string_lossy(), ext));
        let _ = std::fs::remove_file(p);
    }
    let conn = Connection::open(path).with_context(|| format!("creating {}", path.display()))?;
    tune(&conn, true)?;
    conn.pragma_update(None, "journal_mode", "OFF")?;
    conn.pragma_update(None, "synchronous", "OFF")?;
    conn.pragma_update(None, "page_size", 8192)?;
    conn.execute_batch(SCHEMA)?;
    conn.execute_batch(FTS_SCHEMA)?;
    set_meta(&conn, "schema_version", &SCHEMA_VERSION.to_string())?;
    set_meta(&conn, "mode", mode.as_str())?;
    set_meta(&conn, "slim_version", env!("CARGO_PKG_VERSION"))?;
    set_meta(&conn, "complete", "0")?;
    Ok(conn)
}

pub fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        (key, value),
    )?;
    Ok(())
}

pub fn get_meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .optional()?)
}

pub fn check_schema(conn: &Connection) -> Result<()> {
    let v: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |r| r.get(0))
        .optional()
        .context("reading index metadata (is this a slim index?)")?;
    match v {
        Some(v) if v == SCHEMA_VERSION.to_string() => Ok(()),
        Some(v) => bail!(
            "index format {v} differs from this slim's format {SCHEMA_VERSION}; the index is derived data: opening the project or `slim scan <PROJECT_DIR>` rebuilds a source-only index, `slim import <SNAPSHOT>` a semantic one"
        ),
        None => bail!("not a slim index (no schema_version)"),
    }
}

pub fn mode_of(conn: &Connection) -> Result<IndexMode> {
    let m = get_meta(conn, "mode")?.unwrap_or_default();
    IndexMode::parse(&m).ok_or_else(|| anyhow::anyhow!("unknown index mode {m:?}"))
}

/// Builds post-load indexes and statistics, then switches to WAL so that
/// readers and an incremental writer can coexist.
pub fn finish_bulk_load(conn: &Connection) -> Result<()> {
    conn.execute_batch(POST_LOAD_INDEXES)?;
    conn.execute_batch("ANALYZE;")?;
    Ok(())
}

/// Atomically replaces `final_path` with the completed index at `tmp_path`.
/// The previous index (if any) stays usable until the rename succeeds.
pub fn install(tmp_path: &Path, final_path: &Path) -> Result<()> {
    {
        let conn = Connection::open(tmp_path)?;
        conn.pragma_update(None, "journal_mode", "DELETE")?;
        let ok: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if ok != "ok" {
            bail!("integrity check of new index failed: {ok}");
        }
    }
    let f = std::fs::File::open(tmp_path)?;
    f.sync_all()?;
    drop(f);
    for ext in ["-wal", "-shm"] {
        let p = final_path.with_file_name(format!("{}{}", final_path.file_name().unwrap().to_string_lossy(), ext));
        let _ = std::fs::remove_file(p);
    }
    std::fs::rename(tmp_path, final_path)
        .with_context(|| format!("installing index at {}", final_path.display()))?;
    if let Some(dir) = final_path.parent() {
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

/// Random 64-bit generation identifier for invalidating side caches.
pub fn new_generation() -> String {
    let mut buf = [0u8; 8];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut buf);
    }
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    format!("{:016x}", u64::from_le_bytes(buf) ^ t)
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
