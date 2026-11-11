//! Source-only index: discovery, parallel scanning, transactional writes,
//! freshness checks and incremental per-file updates.
//!
//! Nothing here executes project code: `lakefile.lean` is never evaluated,
//! Lake is never invoked, and only `*.lean` files under the project root are
//! read (hidden directories such as `.lake` and `.git` are skipped, and
//! symlinks are not followed out of the root).

use super::lexer::LexProfile;
use super::scan::{scan_source, FileScan};
use crate::db::query::ModStats;
use crate::db::{self, get_meta, set_meta, IndexMode};
use crate::model::{flags, Coverage, DeclKind, Origin};
use crate::paths::{sha256_hex, Project};
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant, UNIX_EPOCH};

pub const SCANNER_VERSION: &str = "3";

/// True when the index was written by this version of the scanner. An index
/// from another version is built again: its candidates may differ.
pub fn scanner_current(conn: &Connection) -> bool {
    crate::db::get_meta(conn, "scanner_version").ok().flatten().as_deref() == Some(SCANNER_VERSION)
}

#[derive(Clone, Debug)]
pub struct ScanConfig {
    pub threads: usize,
    pub max_file_bytes: u64,
    pub profile: LexProfile,
}

impl ScanConfig {
    pub fn for_project(p: &Project) -> ScanConfig {
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(1, 4);
        ScanConfig {
            threads,
            max_file_bytes: 64 << 20,
            profile: p.toolchain.as_deref().map(LexProfile::for_toolchain).unwrap_or(LexProfile::NEWEST),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceFile {
    pub rel: String,
    pub module: String,
    pub size: u64,
    pub mtime_ns: i64,
}

fn mtime_ns(md: &std::fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

fn skip_dir(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "lake-packages" | "build" | "target" | "node_modules" | "_target")
}

/// Lists `*.lean` files under `root` (excluding lakefiles and hidden or
/// build directories). Symlinked directories are not followed; symlinked
/// files are included only when they resolve inside the root.
pub fn discover(root: &Path, cancel: &AtomicBool) -> Result<(Vec<SourceFile>, Vec<String>)> {
    let canon_root = root.canonicalize()?;
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    let mut stack: Vec<PathBuf> = vec![canon_root.clone()];
    let mut dirs = 0usize;
    while let Some(dir) = stack.pop() {
        dirs += 1;
        if dirs % 1024 == 0 && cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) => {
                warnings.push(format!("cannot read {}: {e}", dir.display()));
                continue;
            }
        };
        for ent in rd.flatten() {
            let name = ent.file_name();
            let name = name.to_string_lossy();
            let ft = match ent.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            let path = ent.path();
            if ft.is_dir() {
                if !skip_dir(&name) {
                    stack.push(path);
                }
                continue;
            }
            if !name.ends_with(".lean") {
                continue;
            }
            let md = if ft.is_symlink() {
                match path.canonicalize() {
                    Ok(t) if t.starts_with(&canon_root) && t.is_file() => match std::fs::metadata(&t) {
                        Ok(m) => m,
                        Err(_) => continue,
                    },
                    _ => {
                        warnings.push(format!("skipped symlink leaving the root: {}", path.display()));
                        continue;
                    }
                }
            } else if ft.is_file() {
                match ent.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                }
            } else {
                continue;
            };
            let rel = match path.strip_prefix(&canon_root) {
                Ok(r) => r.to_string_lossy().replace('\\', "/"),
                Err(_) => continue,
            };
            if rel == "lakefile.lean" || rel.contains('\n') {
                continue;
            }
            if crate::snapshot::check_rel_path(&rel).is_err() {
                warnings.push(format!("skipped unusual path {rel:?}"));
                continue;
            }
            let module = rel.trim_end_matches(".lean").replace('/', ".");
            out.push(SourceFile { rel, module, size: md.len(), mtime_ns: mtime_ns(&md) });
        }
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok((out, warnings))
}

pub struct FileResult {
    pub file: SourceFile,
    pub sha256: Option<String>,
    pub status: &'static str,
    pub detail: Option<String>,
    pub scan: Option<FileScan>,
    pub src: Option<String>,
    /// Counts for the outline's roll-ups (lexical, like everything here).
    pub stats: ModStats,
}

/// Counts of one scanned file: candidates by kind, documented ones, `sorry`
/// tokens, and the lines its theorem proofs span (from the end of each
/// statement header to the end of the declaration).
pub fn file_stats(scan: &FileScan, text: &str) -> ModStats {
    let starts = crate::text::line_starts(text);
    let line_of = |b: usize| starts.partition_point(|&s| s <= b);
    let mut st = ModStats::default();
    for d in &scan.decls {
        let counted = match d.kind {
            DeclKind::Theorem => {
                st.thm += 1;
                let (a, z) = (d.header_end as usize, d.end as usize);
                if z > a && z <= text.len() {
                    st.proof_lines += (line_of(z - 1) - line_of(a) + 1) as u32;
                }
                true
            }
            DeclKind::Definition | DeclKind::Abbrev | DeclKind::Opaque => {
                st.def += 1;
                true
            }
            DeclKind::Structure | DeclKind::Class | DeclKind::Inductive => {
                st.typ += 1;
                true
            }
            DeclKind::Instance => {
                st.inst += 1;
                true
            }
            DeclKind::Axiom => {
                st.axiom += 1;
                true
            }
            _ => false,
        };
        if counted && d.flags & flags::HAS_DOC != 0 {
            st.doc += 1;
        }
        if d.flags & flags::LEX_SORRY != 0 {
            st.sorry += 1;
        }
    }
    st
}

pub fn process_file(root: &Path, f: &SourceFile, cfg: &ScanConfig, keep_src: bool) -> FileResult {
    let mut r = FileResult { file: f.clone(), sha256: None, status: "ok", detail: None, scan: None, src: None, stats: ModStats::default() };
    if f.size > cfg.max_file_bytes {
        r.status = "too-large";
        r.detail = Some(format!("{} bytes exceeds the {} byte limit; not scanned", f.size, cfg.max_file_bytes));
        return r;
    }
    let bytes = match std::fs::read(root.join(&f.rel)) {
        Ok(b) => b,
        Err(e) => {
            r.status = "read-error";
            r.detail = Some(e.to_string());
            return r;
        }
    };
    r.sha256 = Some(sha256_hex(&bytes));
    r.file.size = bytes.len() as u64;
    let (text, valid) = crate::text::decode_lossy(&bytes);
    let scan = scan_source(&text, cfg.profile);
    if !valid {
        r.status = "invalid-utf8";
    } else if !scan.lex_ok {
        r.status = "lex-error";
    } else if !scan.issues.is_empty() || scan.unknown_commands > 0 {
        r.status = "partial";
    }
    if !scan.issues.is_empty() {
        let items: Vec<serde_json::Value> = scan
            .issues
            .iter()
            .take(20)
            .map(|i| serde_json::json!({"line": i.line, "message": i.message}))
            .collect();
        r.detail = Some(serde_json::Value::Array(items).to_string());
    }
    r.stats = file_stats(&scan, &text);
    if keep_src {
        r.src = Some(text.into_owned());
    }
    r.scan = Some(scan);
    r
}

pub type Progress<'a> = &'a (dyn Fn(&str, u64, u64) + Sync);

#[derive(Debug, Default, Clone)]
pub struct SourceSummary {
    pub files: u64,
    pub decls: u64,
    pub bytes: u64,
    pub partial_files: u64,
    pub error_files: u64,
    pub ambiguous: u64,
    pub elapsed: Duration,
    pub warnings: Vec<String>,
}

/// Runs `process_file` on a bounded worker pool, delivering results in
/// arbitrary order to `sink` on the calling thread.
fn parallel_scan(
    root: &Path,
    files: Vec<SourceFile>,
    cfg: &ScanConfig,
    cancel: &AtomicBool,
    mut sink: impl FnMut(FileResult) -> Result<()>,
) -> Result<()> {
    let files = Arc::new(files);
    let next = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = mpsc::sync_channel::<FileResult>(256);
    let threads = cfg.threads.max(1).min(files.len().max(1));
    std::thread::scope(|s| -> Result<()> {
        for _ in 0..threads {
            let tx = tx.clone();
            let files = Arc::clone(&files);
            let next = Arc::clone(&next);
            let root = root.to_path_buf();
            let cfg = cfg.clone();
            s.spawn(move || loop {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= files.len() {
                    break;
                }
                let r = process_file(&root, &files[i], &cfg, false);
                if tx.send(r).is_err() {
                    break;
                }
            });
        }
        drop(tx);
        let mut result = Ok(());
        for r in rx.iter() {
            if result.is_ok() {
                if let Err(e) = sink(r) {
                    result = Err(e);
                    cancel.store(true, Ordering::Relaxed);
                }
            }
        }
        result
    })?;
    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled");
    }
    Ok(())
}

struct Writer<'c> {
    conn: &'c Connection,
    modules: HashMap<String, i64>,
    pending_imports: Vec<(i64, String, i64)>,
}

impl<'c> Writer<'c> {
    fn module_id(&mut self, name: &str) -> Result<i64> {
        if let Some(&id) = self.modules.get(name) {
            return Ok(id);
        }
        self.conn.execute("INSERT INTO modules(name, is_local, status) VALUES (?1, 0, 'external')", [name])?;
        let id = self.conn.last_insert_rowid();
        self.modules.insert(name.to_string(), id);
        Ok(id)
    }

    /// Writes one scanned file (module row, imports, candidates, FTS rows).
    fn write_file(&mut self, r: &FileResult, index_statements: bool) -> Result<u64> {
        let f = &r.file;
        let doc = r.scan.as_ref().and_then(|s| s.module_doc.clone());
        let lines = r.scan.as_ref().map(|s| s.line_count as i64);
        let id = match self.modules.get(&f.module) {
            Some(&id) => {
                self.conn.execute(
                    "UPDATE modules SET path=?2, is_local=1, sha256=?3, size=?4, mtime_ns=?5, status=?6, status_detail=?7, doc=?8, line_count=?9 WHERE id=?1",
                    params![id, f.rel, r.sha256, f.size as i64, f.mtime_ns, r.status, r.detail, doc, lines],
                )?;
                id
            }
            None => {
                self.conn.execute(
                    "INSERT INTO modules(name, path, is_local, sha256, size, mtime_ns, status, status_detail, doc, line_count)
                     VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![f.module, f.rel, r.sha256, f.size as i64, f.mtime_ns, r.status, r.detail, doc, lines],
                )?;
                let id = self.conn.last_insert_rowid();
                self.modules.insert(f.module.clone(), id);
                id
            }
        };
        let Some(scan) = &r.scan else {
            self.conn.execute(
                "UPDATE modules SET decl_count = 0, n_thm = 0, n_def = 0, n_type = 0, n_inst = 0, n_axiom = 0, n_doc = 0, n_sorry = 0, proof_lines = 0 WHERE id = ?1",
                [id],
            )?;
            return Ok(0);
        };
        for imp in &scan.imports {
            let fl = (imp.public as i64) | ((imp.all as i64) << 1) | ((imp.meta as i64) << 2);
            self.pending_imports.push((id, imp.module.clone(), fl));
        }
        let mut st = self.conn.prepare_cached(
            "INSERT INTO decls(name, display, kind, module_id, origin, flags, type_cov, value_cov, start_line, start_col,
               end_line, sel_line, sel_col, start_byte, kw_byte, header_end_byte, end_byte, doc, attrs, keyword, sorry_line)
             VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
        )?;
        let mut fts = self.conn.prepare_cached("INSERT INTO fts(rowid, doc, attrs, sig) VALUES (?1, ?2, ?3, ?4)")?;
        for d in &scan.decls {
            st.execute(params![
                d.name,
                d.kind as i64,
                id,
                Origin::SourceCandidate as i64,
                d.flags as i64,
                Coverage::SourceOnly as i64,
                d.kw_line,
                0i64,
                d.end_line,
                d.name_line,
                d.name_col,
                d.start,
                d.kw,
                d.header_end,
                d.end,
                d.doc,
                d.attrs,
                d.keyword,
                d.sorry_line,
            ])?;
            if d.doc.is_some() || d.attrs.is_some() || index_statements {
                let rowid = self.conn.last_insert_rowid();
                fts.execute(params![rowid, d.doc.as_deref().unwrap_or(""), d.attrs.as_deref().unwrap_or(""), ""])?;
            }
        }
        let st = &r.stats;
        self.conn.execute(
            "UPDATE modules SET decl_count = ?2, n_thm = ?3, n_def = ?4, n_type = ?5, n_inst = ?6, n_axiom = ?7, n_doc = ?8, n_sorry = ?9, proof_lines = ?10 WHERE id = ?1",
            params![id, scan.decls.len() as i64, st.thm, st.def, st.typ, st.inst, st.axiom, st.doc, st.sorry, st.proof_lines],
        )?;
        Ok(scan.decls.len() as u64)
    }

    fn flush_imports(&mut self) -> Result<()> {
        let pending = std::mem::take(&mut self.pending_imports);
        for (src, name, fl) in pending {
            let dst = self.module_id(&name)?;
            self.conn.execute(
                "INSERT OR IGNORE INTO module_imports(src, dst, flags) VALUES (?1, ?2, ?3)",
                params![src, dst, fl],
            )?;
        }
        Ok(())
    }
}

fn mark_ambiguous_all(conn: &Connection) -> Result<u64> {
    let amb = flags::AMBIGUOUS as i64;
    conn.execute(&format!("UPDATE decls SET flags = flags & ~{amb} WHERE flags & {amb}"), [])?;
    let n = conn.execute(
        &format!(
            "UPDATE decls SET flags = flags | {amb} WHERE name IN (SELECT name FROM decls GROUP BY name HAVING count(*) > 1)"
        ),
        [],
    )?;
    Ok(n as u64)
}

fn mark_ambiguous_names(conn: &Connection, names: &HashSet<String>) -> Result<()> {
    let amb = flags::AMBIGUOUS as i64;
    let mut cnt = conn.prepare_cached("SELECT count(*) FROM decls WHERE name = ?1")?;
    for n in names {
        let c: i64 = cnt.query_row([n], |r| r.get(0))?;
        if c > 1 {
            conn.execute(&format!("UPDATE decls SET flags = flags | {amb} WHERE name = ?1"), [n])?;
        } else {
            conn.execute(&format!("UPDATE decls SET flags = flags & ~{amb} WHERE name = ?1"), [n])?;
        }
    }
    Ok(())
}

fn write_project_meta(conn: &Connection, project: &Project, cfg: &ScanConfig) -> Result<()> {
    set_meta(conn, "project_name", &project.name)?;
    set_meta(conn, "source_root", &project.root.to_string_lossy())?;
    set_meta(conn, "scanner_version", SCANNER_VERSION)?;
    set_meta(conn, "position_encoding", "utf8")?;
    set_meta(conn, "lex_profile", &format!("latin_letters={},subscript_j={}", cfg.profile.latin_letters, cfg.profile.subscript_j))?;
    if let Some(t) = &project.toolchain {
        set_meta(conn, "project_lean_toolchain", t)?;
    }
    if let Some(m) = &project.lake_manifest_sha256 {
        set_meta(conn, "project_lake_manifest_sha256", m)?;
    }
    Ok(())
}

/// Builds a fresh source index for `project` at `index_path`. The previous
/// index (if any) is replaced only when the build completes.
pub fn build_source_index(
    project: &Project,
    index_path: &Path,
    cfg: &ScanConfig,
    cancel: &AtomicBool,
    progress: Option<Progress>,
) -> Result<SourceSummary> {
    let t0 = Instant::now();
    let (files, warnings) = discover(&project.root, cancel)?;
    let total = files.len() as u64;
    let total_bytes: u64 = files.iter().map(|f| f.size).sum();
    if let Some(dir) = index_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let staging = index_path.with_extension("building");
    let conn = db::create(&staging, IndexMode::Source)?;
    write_project_meta(&conn, project, cfg)?;
    let mut summary = SourceSummary { warnings, ..Default::default() };
    let mut w = Writer { conn: &conn, modules: HashMap::new(), pending_imports: Vec::new() };
    conn.execute_batch("BEGIN")?;
    let mut done = 0u64;
    let mut bytes_done = 0u64;
    let res = parallel_scan(&project.root, files, cfg, cancel, |r| {
        summary.decls += w.write_file(&r, false)?;
        summary.files += 1;
        summary.bytes += r.file.size;
        match r.status {
            "ok" => {}
            "partial" => summary.partial_files += 1,
            _ => summary.error_files += 1,
        }
        done += 1;
        bytes_done += r.file.size;
        if done % 2000 == 0 {
            conn.execute_batch("COMMIT; BEGIN")?;
            if let Some(p) = progress {
                p("scanning", bytes_done, total_bytes);
            }
        }
        Ok(())
    });
    if let Err(e) = res {
        drop(w);
        let _ = conn.execute_batch("ROLLBACK");
        drop(conn);
        let _ = std::fs::remove_file(&staging);
        return Err(e);
    }
    w.flush_imports()?;
    conn.execute_batch("COMMIT")?;
    if let Some(p) = progress {
        p("indexing", total, total);
    }
    db::finish_bulk_load(&conn)?;
    summary.ambiguous = mark_ambiguous_all(&conn)?;
    set_meta(&conn, "file_count", &summary.files.to_string())?;
    set_meta(&conn, "decl_count", &summary.decls.to_string())?;
    set_meta(&conn, "local_decl_count", &summary.decls.to_string())?;
    set_meta(&conn, "created_at", &db::now_unix().to_string())?;
    set_meta(&conn, "complete", "1")?;
    let generation = db::new_generation();
    set_meta(&conn, "name_index_generation", &generation)?;
    if let Some(p) = progress {
        p("name index", total, total);
    }
    let ni = crate::db::import::build_name_index(&conn, &generation)?;
    drop(conn);
    ni.save(&crate::db::import::names_path(index_path))?;
    db::install(&staging, index_path)?;
    summary.elapsed = t0.elapsed();
    Ok(summary)
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Freshness {
    pub checked: usize,
    pub unchanged: usize,
    /// Metadata changed but content hash identical.
    pub touched: Vec<String>,
    pub changed: Vec<String>,
    pub deleted: Vec<String>,
    pub added: Vec<String>,
    /// Project-level inputs that changed (`lean-toolchain`, `lake-manifest.json`).
    pub env_changed: Vec<String>,
    pub hashed_bytes: u64,
    pub elapsed: Duration,
}

impl Freshness {
    pub fn is_fresh(&self) -> bool {
        self.changed.is_empty() && self.deleted.is_empty() && self.added.is_empty() && self.env_changed.is_empty()
    }
}

/// Compares the files recorded in an index with the files on disk.
/// Size+mtime equality is used only as a hint to skip hashing (unless
/// `force_hash`); a mismatch is resolved by comparing SHA-256 content hashes.
pub fn check_freshness(conn: &Connection, root: &Path, discover_new: bool, force_hash: bool, cancel: &AtomicBool) -> Result<Freshness> {
    let t0 = Instant::now();
    let mut fr = Freshness::default();
    let mut st = conn.prepare("SELECT path, sha256, size, mtime_ns FROM modules WHERE is_local = 1 AND path IS NOT NULL")?;
    let rows: Vec<(String, Option<String>, Option<i64>, Option<i64>)> =
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<rusqlite::Result<_>>()?;
    let mut known: HashSet<String> = HashSet::with_capacity(rows.len());
    let canon_root = root.canonicalize().with_context(|| format!("source root {}", root.display()))?;
    for (i, (rel, sha, size, mt)) in rows.into_iter().enumerate() {
        if i % 1024 == 0 && cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        fr.checked += 1;
        known.insert(rel.clone());
        let path = match crate::paths::resolve_in_canonical_root(&canon_root, &rel) {
            Ok(p) => p,
            Err(_) => {
                fr.deleted.push(rel);
                continue;
            }
        };
        let md = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => {
                fr.deleted.push(rel);
                continue;
            }
        };
        let same_meta = size == Some(md.len() as i64) && mt == Some(mtime_ns(&md));
        if same_meta && !force_hash {
            fr.unchanged += 1;
            continue;
        }
        let Some(sha) = sha else {
            fr.changed.push(rel);
            continue;
        };
        match std::fs::read(&path) {
            Ok(b) => {
                fr.hashed_bytes += b.len() as u64;
                if sha256_hex(&b) == sha {
                    if same_meta {
                        fr.unchanged += 1;
                    } else {
                        fr.touched.push(rel);
                    }
                } else {
                    fr.changed.push(rel);
                }
            }
            Err(_) => fr.deleted.push(rel),
        }
    }
    if discover_new {
        let (files, _) = discover(root, cancel)?;
        for f in files {
            if !known.contains(&f.rel) {
                fr.added.push(f.rel);
            }
        }
    }
    // Project-level dependency state.
    if let Some(recorded) = get_meta(conn, "project_lean_toolchain")? {
        let now = std::fs::read_to_string(root.join("lean-toolchain")).map(|s| s.trim().to_string()).unwrap_or_default();
        if now != recorded {
            fr.env_changed.push(format!("lean-toolchain: {recorded} -> {}", if now.is_empty() { "(missing)" } else { &now }));
        }
    }
    if let Some(recorded) = get_meta(conn, "project_lake_manifest_sha256")? {
        let now = std::fs::read(root.join("lake-manifest.json")).map(|b| sha256_hex(&b)).unwrap_or_default();
        if now != recorded {
            fr.env_changed.push("lake-manifest.json changed (dependency versions)".into());
        }
    }
    fr.elapsed = t0.elapsed();
    Ok(fr)
}

/// Applies a freshness report to a source index in one transaction:
/// deleted files lose their candidates, changed and added files are
/// rescanned. Readers see either the old or the new state.
pub fn apply_incremental(index_path: &Path, project: &Project, fr: &Freshness, cfg: &ScanConfig, cancel: &AtomicBool) -> Result<SourceSummary> {
    let t0 = Instant::now();
    let conn = db::open_write(index_path)?;
    if db::mode_of(&conn)? != IndexMode::Source {
        bail!("incremental update only applies to source indexes");
    }
    let mut summary = SourceSummary::default();
    let mut touched_names: HashSet<String> = HashSet::new();
    let mut modules: HashMap<String, i64> = HashMap::new();
    {
        let mut st = conn.prepare("SELECT name, id FROM modules")?;
        for r in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (n, id) = r?;
            modules.insert(n, id);
        }
    }
    let mut to_scan: Vec<SourceFile> = Vec::new();
    for rel in fr.changed.iter().chain(fr.added.iter()) {
        let full = project.root.join(rel);
        if let Ok(md) = std::fs::metadata(&full) {
            to_scan.push(SourceFile {
                rel: rel.clone(),
                module: rel.trim_end_matches(".lean").replace('/', "."),
                size: md.len(),
                mtime_ns: mtime_ns(&md),
            });
        }
    }
    // Scan first (no DB lock held), then write everything in one transaction.
    let mut results = Vec::new();
    parallel_scan(&project.root, to_scan, cfg, cancel, |r| {
        results.push(r);
        Ok(())
    })?;
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let res = (|| -> Result<()> {
        let remove_module_contents = |mid: i64, names: &mut HashSet<String>| -> Result<()> {
            let mut st = conn.prepare_cached("SELECT name FROM decls WHERE module_id = ?1")?;
            for n in st.query_map([mid], |r| r.get::<_, String>(0))? {
                names.insert(n?);
            }
            conn.execute("DELETE FROM fts WHERE rowid IN (SELECT id FROM decls WHERE module_id = ?1)", [mid])?;
            conn.execute("DELETE FROM decls WHERE module_id = ?1", [mid])?;
            conn.execute("DELETE FROM module_imports WHERE src = ?1", [mid])?;
            Ok(())
        };
        for rel in &fr.deleted {
            let mid: Option<i64> = conn
                .query_row("SELECT id FROM modules WHERE path = ?1", [rel], |r| r.get(0))
                .ok();
            if let Some(mid) = mid {
                remove_module_contents(mid, &mut touched_names)?;
                conn.execute(
                    "UPDATE modules SET path = NULL, is_local = 0, sha256 = NULL, status = 'deleted', decl_count = 0, n_thm = 0, n_def = 0,
                       n_type = 0, n_inst = 0, n_axiom = 0, n_doc = 0, n_sorry = 0, proof_lines = 0 WHERE id = ?1",
                    [mid],
                )?;
            }
        }
        let mut w = Writer { conn: &conn, modules, pending_imports: Vec::new() };
        for r in &results {
            if let Some(&mid) = w.modules.get(&r.file.module) {
                remove_module_contents(mid, &mut touched_names)?;
            }
            summary.decls += w.write_file(r, false)?;
            summary.files += 1;
            if let Some(s) = &r.scan {
                for d in &s.decls {
                    touched_names.insert(d.name.clone());
                }
            }
        }
        w.flush_imports()?;
        for rel in &fr.touched {
            if let Ok(md) = std::fs::metadata(project.root.join(rel)) {
                conn.execute(
                    "UPDATE modules SET size = ?2, mtime_ns = ?3 WHERE path = ?1",
                    params![rel, md.len() as i64, mtime_ns(&md)],
                )?;
            }
        }
        mark_ambiguous_names(&conn, &touched_names)?;
        let total: i64 = conn.query_row("SELECT count(*) FROM decls", [], |r| r.get(0))?;
        set_meta(&conn, "decl_count", &total.to_string())?;
        set_meta(&conn, "local_decl_count", &total.to_string())?;
        set_meta(&conn, "updated_at", &db::now_unix().to_string())?;
        if let Some(t) = &project.toolchain {
            set_meta(&conn, "project_lean_toolchain", t)?;
        }
        if let Some(m) = &project.lake_manifest_sha256 {
            set_meta(&conn, "project_lake_manifest_sha256", m)?;
        }
        Ok(())
    })();
    if let Err(e) = res {
        let _ = conn.execute_batch("ROLLBACK");
        return Err(e);
    }
    let generation = db::new_generation();
    set_meta(&conn, "name_index_generation", &generation)?;
    conn.execute_batch("COMMIT")?;
    let ni = crate::db::import::build_name_index(&conn, &generation).context("rebuilding name index")?;
    ni.save(&crate::db::import::names_path(index_path))?;
    summary.elapsed = t0.elapsed();
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn names(idx: &Path) -> Vec<String> {
        let c = db::open_read(idx).unwrap();
        let mut st = c.prepare("SELECT name FROM decls ORDER BY name").unwrap();
        st.query_map([], |r| r.get(0)).unwrap().map(|x| x.unwrap()).collect()
    }

    #[test]
    fn build_then_incremental_with_delete_rename_and_add() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("proj");
        write(&root, "lakefile.toml", "name = \"p\"\n");
        write(&root, "P/A.lean", "namespace P\ntheorem a_one : True := trivial\ntheorem shared : True := trivial\nend P\n");
        write(&root, "P/B.lean", "import P.A\nnamespace P\ntheorem b_one : True := trivial\nend P\n");
        write(&root, ".lake/packages/x/X.lean", "theorem hidden : True := trivial\n");
        let project = Project::open(&root).unwrap();
        let idx = d.path().join("cache/source.sqlite");
        let cfg = ScanConfig { threads: 2, max_file_bytes: 1 << 20, profile: LexProfile::NEWEST };
        let cancel = AtomicBool::new(false);
        let s = build_source_index(&project, &idx, &cfg, &cancel, None).unwrap();
        assert_eq!(s.files, 2);
        assert_eq!(names(&idx), vec!["P.a_one", "P.b_one", "P.shared"]);
        let c = db::open_read(&idx).unwrap();
        let fr = check_freshness(&c, &root, true, false, &cancel).unwrap();
        assert!(fr.is_fresh(), "{fr:?}");
        drop(c);
        // delete B, rename a declaration in A, add C with a duplicate name
        std::fs::remove_file(root.join("P/B.lean")).unwrap();
        write(&root, "P/A.lean", "namespace P\ntheorem a_renamed : True := trivial\ntheorem shared : True := trivial\nend P\n");
        write(&root, "P/C.lean", "namespace P\ntheorem shared : True := trivial\nend P\n");
        let c = db::open_read(&idx).unwrap();
        let fr = check_freshness(&c, &root, true, false, &cancel).unwrap();
        assert_eq!(fr.deleted, vec!["P/B.lean"]);
        assert_eq!(fr.changed, vec!["P/A.lean"]);
        assert_eq!(fr.added, vec!["P/C.lean"]);
        drop(c);
        apply_incremental(&idx, &project, &fr, &cfg, &cancel).unwrap();
        assert_eq!(names(&idx), vec!["P.a_renamed", "P.shared", "P.shared"]);
        let c = db::open_read(&idx).unwrap();
        let amb: i64 = c
            .query_row(&format!("SELECT count(*) FROM decls WHERE flags & {} != 0", flags::AMBIGUOUS), [], |r| r.get(0))
            .unwrap();
        assert_eq!(amb, 2);
        let fr = check_freshness(&c, &root, true, false, &cancel).unwrap();
        assert!(fr.is_fresh(), "{fr:?}");
        // name index regenerated and consistent
        let generation = get_meta(&c, "name_index_generation").unwrap().unwrap();
        let ni = crate::nameindex::NameIndex::load(&crate::db::import::names_path(&idx), &generation).unwrap();
        assert_eq!(ni.len(), 3);
        // toolchain change is reported as an environment change
        write(&root, "lean-toolchain", "leanprover/lean4:v4.99.0\n");
        drop(c);
        let c = db::open_read(&idx).unwrap();
        // the index recorded no toolchain, so nothing to compare yet
        let fr = check_freshness(&c, &root, false, false, &cancel).unwrap();
        assert!(fr.env_changed.is_empty());
    }

    #[test]
    fn touch_without_content_change_is_not_a_change() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("proj");
        write(&root, "A.lean", "theorem a : True := trivial\n");
        let project = Project::open(&root).unwrap();
        let idx = d.path().join("i.sqlite");
        let cfg = ScanConfig { threads: 1, max_file_bytes: 1 << 20, profile: LexProfile::NEWEST };
        let cancel = AtomicBool::new(false);
        build_source_index(&project, &idx, &cfg, &cancel, None).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        write(&root, "A.lean", "theorem a : True := trivial\n");
        let c = db::open_read(&idx).unwrap();
        let fr = check_freshness(&c, &root, true, false, &cancel).unwrap();
        assert_eq!(fr.touched, vec!["A.lean"]);
        assert!(fr.is_fresh());
    }

    #[test]
    fn cancelled_build_leaves_previous_index() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("proj");
        for i in 0..50 {
            write(&root, &format!("M{i}.lean"), "theorem a : True := trivial\n");
        }
        let project = Project::open(&root).unwrap();
        let idx = d.path().join("i.sqlite");
        let cfg = ScanConfig { threads: 2, max_file_bytes: 1 << 20, profile: LexProfile::NEWEST };
        let cancel = AtomicBool::new(false);
        build_source_index(&project, &idx, &cfg, &cancel, None).unwrap();
        let before = std::fs::read(&idx).unwrap();
        let cancel = AtomicBool::new(true);
        assert!(build_source_index(&project, &idx, &cfg, &cancel, None).is_err());
        assert_eq!(std::fs::read(&idx).unwrap(), before);
        assert!(!idx.with_extension("building").exists());
    }
}
