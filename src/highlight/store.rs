//! The persistent highlight cache: one SQLite file per project in the cache
//! directory (never in the project). It is separate from the index, which
//! is replaced wholesale when it is rebuilt.
//!
//! * `baseline`: TextMate scope spans per file, valid for one exact source
//!   hash and one baseline provider identity;
//! * `semantic`: Lean's semantic tokens per file with everything needed to
//!   judge them later: exact source hash, coverage, token legend, position
//!   encoding, environment fingerprint, digests of the imported sources and
//!   their compiled artifacts, diagnostics counts, extractor version;
//! * `env`: the environments records were made in; `run`: indexing runs.
//!
//! A record is replaced in one transaction, so a reader sees the old record
//! or the new one. WAL mode lets the browser read while an indexer writes.

use super::env::EnvRecord;
use super::{Coverage, Legend};
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS baseline(
  path TEXT PRIMARY KEY, source_sha256 TEXT NOT NULL, provider TEXT NOT NULL,
  n_lines INTEGER NOT NULL, blob BLOB NOT NULL, created TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS semantic(
  path TEXT PRIMARY KEY, source_sha256 TEXT NOT NULL, source_size INTEGER NOT NULL, line_endings TEXT NOT NULL,
  status TEXT NOT NULL, detail TEXT NOT NULL, n_lines INTEGER NOT NULL, n_tokens INTEGER NOT NULL,
  processed_lines INTEGER, legend TEXT NOT NULL, position_encoding TEXT NOT NULL, blob BLOB,
  diag_errors INTEGER NOT NULL, diag_warnings INTEGER NOT NULL,
  env_fingerprint TEXT NOT NULL, deps_digest TEXT NOT NULL, deps_count INTEGER NOT NULL,
  artifacts_digest TEXT NOT NULL, build_note TEXT NOT NULL,
  extractor TEXT NOT NULL, elapsed_ms INTEGER NOT NULL, created TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS env(fingerprint TEXT PRIMARY KEY, json TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS run(
  id INTEGER PRIMARY KEY, started TEXT NOT NULL, finished TEXT, env_fingerprint TEXT NOT NULL,
  outcome TEXT, summary TEXT);
";

/// A semantic record without its token blob.
#[derive(Clone, Debug, PartialEq)]
pub struct SemMeta {
    pub path: String,
    pub source_sha256: String,
    pub source_size: u64,
    pub line_endings: String,
    pub status: Coverage,
    /// Why the status is what it is, in words.
    pub detail: String,
    pub n_lines: usize,
    pub n_tokens: usize,
    /// For partial records: lines the server had processed.
    pub processed_lines: Option<usize>,
    pub legend: Legend,
    pub position_encoding: String,
    pub diag_errors: usize,
    pub diag_warnings: usize,
    pub env_fingerprint: String,
    pub deps_digest: String,
    pub deps_count: usize,
    pub artifacts_digest: String,
    /// What was known about compiled imports versus their sources.
    pub build_note: String,
    pub extractor: String,
    pub elapsed_ms: u64,
    pub created: String,
}

#[derive(Clone, Debug)]
pub struct SemRecord {
    pub meta: SemMeta,
    pub blob: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoreStats {
    pub baseline_files: u64,
    pub baseline_bytes: u64,
    pub complete: u64,
    pub partial: u64,
    pub failed: u64,
    pub skipped: u64,
    pub semantic_bytes: u64,
    pub semantic_tokens: u64,
    pub file_bytes: u64,
}

pub struct Store {
    conn: Connection,
    path: PathBuf,
}

const META_COLS: &str = "path, source_sha256, source_size, line_endings, status, detail, n_lines, n_tokens, processed_lines, legend, \
                         position_encoding, diag_errors, diag_warnings, env_fingerprint, deps_digest, deps_count, artifacts_digest, \
                         build_note, extractor, elapsed_ms, created";

fn meta_from_row(r: &rusqlite::Row) -> rusqlite::Result<SemMeta> {
    let status: String = r.get(4)?;
    let legend: String = r.get(9)?;
    Ok(SemMeta {
        path: r.get(0)?,
        source_sha256: r.get(1)?,
        source_size: r.get::<_, i64>(2)? as u64,
        line_endings: r.get(3)?,
        // An unknown status is treated as failed: never as usable tokens.
        status: Coverage::parse(&status).unwrap_or(Coverage::Failed),
        detail: r.get(5)?,
        n_lines: r.get::<_, i64>(6)? as usize,
        n_tokens: r.get::<_, i64>(7)? as usize,
        processed_lines: r.get::<_, Option<i64>>(8)?.map(|v| v as usize),
        legend: serde_json::from_str(&legend).unwrap_or_default(),
        position_encoding: r.get(10)?,
        diag_errors: r.get::<_, i64>(11)? as usize,
        diag_warnings: r.get::<_, i64>(12)? as usize,
        env_fingerprint: r.get(13)?,
        deps_digest: r.get(14)?,
        deps_count: r.get::<_, i64>(15)? as usize,
        artifacts_digest: r.get(16)?,
        build_note: r.get(17)?,
        extractor: r.get(18)?,
        elapsed_ms: r.get::<_, i64>(19)? as u64,
        created: r.get(20)?,
    })
}

impl Store {
    /// Where a project's cache lives.
    pub fn path_for_project(project: &crate::paths::Project) -> PathBuf {
        project.cache_dir().join("highlight.sqlite")
    }

    /// For an index opened without a project directory (the demo, `--index`).
    pub fn path_beside_index(index: &Path) -> PathBuf {
        index.with_file_name("highlight.sqlite")
    }

    fn init(conn: Connection, path: &Path, create: bool) -> Result<Store> {
        conn.busy_timeout(std::time::Duration::from_millis(5000))?;
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if v == 0 && create {
            conn.pragma_update(None, "journal_mode", "WAL")?;
            conn.pragma_update(None, "synchronous", "NORMAL")?;
            conn.execute_batch(SCHEMA)?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        } else if v != SCHEMA_VERSION {
            bail!(
                "highlight cache {} has schema version {v}, this slim uses {SCHEMA_VERSION}; \
                 remove it with `slim highlight clean`",
                path.display()
            );
        }
        Ok(Store { conn, path: path.to_path_buf() })
    }

    /// Opens the cache, creating it if needed.
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        Self::init(conn, path, true)
    }

    /// Opens the cache only if it exists (browsing never creates it just
    /// to look).
    pub fn open_existing(path: &Path) -> Result<Option<Store>> {
        if !path.is_file() {
            return Ok(None);
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        Self::init(conn, path, false).map(Some)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute("INSERT OR REPLACE INTO meta(key, value) VALUES (?1, ?2)", params![key, value])?;
        Ok(())
    }

    pub fn get_meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0)).optional()?)
    }

    // --- baseline ---

    pub fn get_baseline(&self, rel: &str, sha256: &str, provider: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn
            .query_row(
                "SELECT blob FROM baseline WHERE path = ?1 AND source_sha256 = ?2 AND provider = ?3",
                params![rel, sha256, provider],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn put_baseline(&self, rel: &str, sha256: &str, provider: &str, n_lines: usize, blob: &[u8]) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO baseline(path, source_sha256, provider, n_lines, blob, created) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![rel, sha256, provider, n_lines as i64, blob, crate::export::utc_now()],
        )?;
        Ok(())
    }

    // --- semantic ---

    pub fn get_semantic_meta(&self, rel: &str) -> Result<Option<SemMeta>> {
        Ok(self.conn.query_row(&format!("SELECT {META_COLS} FROM semantic WHERE path = ?1"), params![rel], meta_from_row).optional()?)
    }

    pub fn get_semantic(&self, rel: &str) -> Result<Option<SemRecord>> {
        Ok(self
            .conn
            .query_row(&format!("SELECT {META_COLS}, blob FROM semantic WHERE path = ?1"), params![rel], |r| {
                Ok(SemRecord { meta: meta_from_row(r)?, blob: r.get(21)? })
            })
            .optional()?)
    }

    pub fn all_semantic_meta(&self) -> Result<Vec<SemMeta>> {
        let mut st = self.conn.prepare(&format!("SELECT {META_COLS} FROM semantic ORDER BY path"))?;
        let rows = st.query_map([], meta_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Publishes a record, replacing any earlier one for the path, in one
    /// transaction.
    pub fn put_semantic(&self, rec: &SemRecord) -> Result<()> {
        let m = &rec.meta;
        self.conn.execute(
            &format!("INSERT OR REPLACE INTO semantic({META_COLS}, blob) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22)"),
            params![
                m.path,
                m.source_sha256,
                m.source_size as i64,
                m.line_endings,
                m.status.as_str(),
                m.detail,
                m.n_lines as i64,
                m.n_tokens as i64,
                m.processed_lines.map(|v| v as i64),
                serde_json::to_string(&m.legend)?,
                m.position_encoding,
                m.diag_errors as i64,
                m.diag_warnings as i64,
                m.env_fingerprint,
                m.deps_digest,
                m.deps_count as i64,
                m.artifacts_digest,
                m.build_note,
                m.extractor,
                m.elapsed_ms as i64,
                m.created,
                rec.blob,
            ],
        )?;
        Ok(())
    }

    // --- environments and runs ---

    pub fn put_env(&self, rec: &EnvRecord) -> Result<String> {
        let fp = rec.info.fingerprint();
        self.conn.execute("INSERT OR REPLACE INTO env(fingerprint, json) VALUES (?1, ?2)", params![fp, serde_json::to_string(rec)?])?;
        Ok(fp)
    }

    pub fn get_env(&self, fingerprint: &str) -> Result<Option<EnvRecord>> {
        let j: Option<String> = self.conn.query_row("SELECT json FROM env WHERE fingerprint = ?1", params![fingerprint], |r| r.get(0)).optional()?;
        Ok(j.and_then(|j| serde_json::from_str(&j).ok()))
    }

    pub fn begin_run(&self, env_fingerprint: &str) -> Result<i64> {
        self.conn.execute("INSERT INTO run(started, env_fingerprint) VALUES (?1, ?2)", params![crate::export::utc_now(), env_fingerprint])?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn finish_run(&self, id: i64, outcome: &str, summary: &serde_json::Value) -> Result<()> {
        self.conn.execute(
            "UPDATE run SET finished = ?1, outcome = ?2, summary = ?3 WHERE id = ?4",
            params![crate::export::utc_now(), outcome, summary.to_string(), id],
        )?;
        Ok(())
    }

    /// (started, finished, outcome, summary) of the latest run.
    pub fn last_run(&self) -> Result<Option<(String, Option<String>, Option<String>, Option<String>)>> {
        Ok(self
            .conn
            .query_row("SELECT started, finished, outcome, summary FROM run ORDER BY id DESC LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .optional()?)
    }

    pub fn stats(&self) -> Result<StoreStats> {
        let mut s = StoreStats::default();
        (s.baseline_files, s.baseline_bytes) =
            self.conn.query_row("SELECT COUNT(*), COALESCE(SUM(LENGTH(blob)), 0) FROM baseline", [], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64)))?;
        let mut st = self.conn.prepare("SELECT status, COUNT(*), COALESCE(SUM(LENGTH(blob)), 0), COALESCE(SUM(n_tokens), 0) FROM semantic GROUP BY status")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64, r.get::<_, i64>(2)? as u64, r.get::<_, i64>(3)? as u64)))?;
        for row in rows {
            let (status, n, bytes, toks) = row?;
            match Coverage::parse(&status) {
                Some(Coverage::Complete) => s.complete += n,
                Some(Coverage::Partial) => s.partial += n,
                Some(Coverage::Skipped) => s.skipped += n,
                _ => s.failed += n,
            }
            s.semantic_bytes += bytes;
            s.semantic_tokens += toks;
        }
        s.file_bytes = ["", "-wal"].iter().filter_map(|x| std::fs::metadata(format!("{}{x}", self.path.display())).ok()).map(|m| m.len()).sum();
        Ok(s)
    }

    /// Deletes cached data (`baseline`, `semantic` or both); returns rows removed.
    pub fn clear(&self, baseline: bool, semantic: bool) -> Result<usize> {
        let mut n = 0;
        if baseline {
            n += self.conn.execute("DELETE FROM baseline", [])?;
        }
        if semantic {
            n += self.conn.execute("DELETE FROM semantic", [])?;
            self.conn.execute("DELETE FROM env", [])?;
            self.conn.execute("DELETE FROM run", [])?;
        }
        Ok(n)
    }

    pub fn delete_semantic(&self, rel: &str) -> Result<()> {
        self.conn.execute("DELETE FROM semantic WHERE path = ?1", params![rel])?;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn test_meta(path: &str, sha: &str, status: Coverage) -> SemMeta {
    SemMeta {
        path: path.into(),
        source_sha256: sha.into(),
        source_size: 10,
        line_endings: "lf".into(),
        status,
        detail: String::new(),
        n_lines: 1,
        n_tokens: 0,
        processed_lines: None,
        legend: Legend { types: vec!["keyword".into(), "variable".into()], modifiers: vec![] },
        position_encoding: "utf-16".into(),
        diag_errors: 0,
        diag_warnings: 0,
        env_fingerprint: "env".into(),
        deps_digest: "deps".into(),
        deps_count: 0,
        artifacts_digest: String::new(),
        build_note: String::new(),
        extractor: super::EXTRACTOR_VERSION.into(),
        elapsed_ms: 1,
        created: "2026-01-01T00:00:00Z".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip_and_replace_atomically() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c/highlight.sqlite");
        assert!(Store::open_existing(&p).unwrap().is_none(), "looking does not create the cache");
        assert!(!p.exists());
        let s = Store::open(&p).unwrap();
        let mut rec = SemRecord { meta: test_meta("A.lean", "h1", Coverage::Complete), blob: Some(vec![1, 2, 3]) };
        rec.meta.n_tokens = 7;
        s.put_semantic(&rec).unwrap();
        // A second connection (the browser) sees whole records only.
        let reader = Store::open_existing(&p).unwrap().unwrap();
        let got = reader.get_semantic("A.lean").unwrap().unwrap();
        assert_eq!(got.meta, rec.meta);
        assert_eq!(got.blob, Some(vec![1, 2, 3]));
        rec.meta.source_sha256 = "h2".into();
        rec.meta.status = Coverage::Partial;
        rec.blob = None;
        s.put_semantic(&rec).unwrap();
        let got = reader.get_semantic("A.lean").unwrap().unwrap();
        assert_eq!((got.meta.source_sha256.as_str(), got.meta.status, got.blob), ("h2", Coverage::Partial, None));
        assert_eq!(reader.all_semantic_meta().unwrap().len(), 1);
        assert!(reader.get_semantic("B.lean").unwrap().is_none());
        let st = s.stats().unwrap();
        assert_eq!((st.complete, st.partial, st.semantic_tokens), (0, 1, 7));
    }

    #[test]
    fn baseline_is_keyed_by_hash_and_provider() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::open(&d.path().join("h.sqlite")).unwrap();
        s.put_baseline("A.lean", "h1", "p1", 3, b"blob").unwrap();
        assert_eq!(s.get_baseline("A.lean", "h1", "p1").unwrap().as_deref(), Some(&b"blob"[..]));
        assert!(s.get_baseline("A.lean", "h2", "p1").unwrap().is_none(), "source changed");
        assert!(s.get_baseline("A.lean", "h1", "p2").unwrap().is_none(), "grammar changed");
        assert_eq!(s.clear(true, false).unwrap(), 1);
        assert!(s.get_baseline("A.lean", "h1", "p1").unwrap().is_none());
    }

    #[test]
    fn foreign_schema_versions_are_refused() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("h.sqlite");
        {
            let c = Connection::open(&p).unwrap();
            c.pragma_update(None, "user_version", 99).unwrap();
        }
        assert!(Store::open(&p).is_err());
        assert!(Store::open_existing(&p).is_err());
        std::fs::write(d.path().join("junk.sqlite"), b"not a database at all").unwrap();
        assert!(Store::open_existing(&d.path().join("junk.sqlite")).is_err());
    }

    #[test]
    fn environments_and_runs() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::open(&d.path().join("h.sqlite")).unwrap();
        let rec = EnvRecord { lean_version: Some("Lean (version 4.33.1)".into()), ..Default::default() };
        let fp = s.put_env(&rec).unwrap();
        assert_eq!(s.get_env(&fp).unwrap().unwrap().lean_version, rec.lean_version);
        let id = s.begin_run(&fp).unwrap();
        assert_eq!(s.last_run().unwrap().unwrap().2, None, "an interrupted run has no outcome");
        s.finish_run(id, "cancelled", &serde_json::json!({"complete": 3})).unwrap();
        assert_eq!(s.last_run().unwrap().unwrap().2.as_deref(), Some("cancelled"));
    }
}
