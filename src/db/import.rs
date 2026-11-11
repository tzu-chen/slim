//! Transactional, resumable import of semantic snapshots.
//!
//! Pass 1 streams the snapshot to validate its framing (header first, footer
//! last, counts), hash it, and collect every declaration and referenced name.
//! Identifiers are then assigned deterministically (local declarations
//! first, then library, then unresolved references, each sorted by name), so
//! that index order is a stable, meaningful sort order for neighbour lists.
//! Pass 2 loads rows into a staging database, committing in batches and
//! recording its byte offset so an interrupted import resumes where it
//! stopped. Finalisation computes degrees, the transitive axiom analysis,
//! full-text rows, secondary indexes and the name-search side file, then
//! atomically renames the staging file over the previous index. A failed or
//! cancelled import never touches the previous index.

use super::{create, finish_bulk_load, get_meta, install, new_generation, set_meta, IndexMode};
use crate::axioms;
use crate::model::{display_name, flags, AxiomCategory, Coverage, DeclKind, EdgeKind, Origin};
use crate::nameindex::{self, NameIndex};
use crate::snapshot::{self, limits, Header, Line};
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub type Progress<'a> = &'a (dyn Fn(&str, u64, u64) + Sync);

pub struct ImportOptions<'a> {
    pub allow_partial: bool,
    pub batch_records: usize,
    pub cancel: Option<&'a AtomicBool>,
    pub progress: Option<Progress<'a>>,
    /// Approved source root recorded in the index metadata.
    pub source_root: Option<PathBuf>,
    /// Stop after committing this many records (tests of resumption).
    pub stop_after_records: Option<u64>,
}

impl Default for ImportOptions<'_> {
    fn default() -> Self {
        ImportOptions {
            allow_partial: false,
            batch_records: 20_000,
            cancel: None,
            progress: None,
            source_root: None,
            stop_after_records: None,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct ImportSummary {
    pub modules: u64,
    pub decls: u64,
    pub local: u64,
    pub library: u64,
    pub unresolved: u64,
    pub edges: u64,
    pub axioms: u64,
    pub sha256: String,
    pub resumed: bool,
    pub complete_snapshot: bool,
    pub elapsed: Duration,
    pub warnings: Vec<String>,
}

/// Reads one `\n`-terminated line, refusing lines over the size limit.
fn read_line_bounded<R: BufRead>(r: &mut R, buf: &mut Vec<u8>) -> Result<usize> {
    buf.clear();
    let n = r.by_ref().take(limits::MAX_LINE_BYTES as u64 + 1).read_until(b'\n', buf)?;
    if buf.len() > limits::MAX_LINE_BYTES {
        bail!("snapshot line exceeds {} bytes", limits::MAX_LINE_BYTES);
    }
    Ok(n)
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct P1Line {
    record: String,
    name: Option<String>,
    local: Option<bool>,
    refs: Option<snapshot::Refs>,
    counts: Option<snapshot::Counts>,
    complete: Option<bool>,
}

struct Pass1 {
    header: Header,
    sha256: String,
    /// (rank, name): rank 0 local, 1 library, 2 unresolved.
    nodes: Vec<(u8, Box<str>)>,
    decl_records: u64,
    module_records: u64,
    footer_ok: bool,
    footer_complete: bool,
    bytes: u64,
}

fn pass1(path: &Path, opts: &ImportOptions) -> Result<Pass1> {
    let f = std::fs::File::open(path).with_context(|| format!("opening snapshot {}", path.display()))?;
    let total = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut r = BufReader::with_capacity(1 << 20, f);
    let mut hasher = Sha256::new();
    let mut buf = Vec::new();
    let mut lineno = 0u64;
    let mut header: Option<Header> = None;
    let mut decls: HashMap<Box<str>, bool> = HashMap::new();
    let mut refs: HashSet<Box<str>> = HashSet::new();
    let mut modules: HashSet<String> = HashSet::new();
    let mut decl_records = 0u64;
    let mut module_records = 0u64;
    let mut footer: Option<(snapshot::Counts, bool)> = None;
    let mut bytes = 0u64;
    loop {
        let n = read_line_bounded(&mut r, &mut buf)?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        hasher.update(&buf);
        lineno += 1;
        let line = buf.strip_suffix(b"\n").unwrap_or(&buf);
        if line.iter().all(|b| b.is_ascii_whitespace()) {
            continue;
        }
        if footer.is_some() {
            bail!("line {lineno}: data after footer record");
        }
        if lineno == 1 {
            let l: Line = serde_json::from_slice(line).with_context(|| format!("line 1: malformed JSON"))?;
            if l.record != "header" {
                bail!("line 1: first record must be the header");
            }
            header = Some(l.into_header()?);
            continue;
        }
        let l: P1Line = serde_json::from_slice(line).with_context(|| format!("line {lineno}: malformed JSON"))?;
        match l.record.as_str() {
            "module" => {
                module_records += 1;
                let name = l.name.unwrap_or_default();
                snapshot::check_name(&name).with_context(|| format!("line {lineno}"))?;
                if !modules.insert(name.clone()) {
                    bail!("line {lineno}: duplicate module record {name}");
                }
                if modules.len() > limits::MAX_MODULES {
                    bail!("too many modules");
                }
            }
            "decl" => {
                decl_records += 1;
                let name = l.name.unwrap_or_default();
                snapshot::check_name(&name).with_context(|| format!("line {lineno}"))?;
                if decls.insert(name.clone().into_boxed_str(), l.local.unwrap_or(false)).is_some() {
                    bail!("line {lineno}: duplicate declaration {name} (snapshots must deduplicate by canonical name)");
                }
                if decls.len() > limits::MAX_DECLS {
                    bail!("too many declarations");
                }
                if let Some(rf) = l.refs {
                    for r in rf.type_.into_iter().chain(rf.value).chain(rf.ctor) {
                        if !refs.contains(r.as_str()) {
                            refs.insert(r.into_boxed_str());
                        }
                    }
                }
            }
            "footer" => {
                footer = Some((l.counts.unwrap_or_default(), l.complete.unwrap_or(false)));
            }
            "header" => bail!("line {lineno}: second header record"),
            other => bail!("line {lineno}: unknown record type {other:?}"),
        }
        if lineno % 50_000 == 0 {
            if let Some(p) = opts.progress {
                p("validating", bytes, total);
            }
            if opts.cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                bail!("import cancelled");
            }
        }
    }
    let header = header.context("empty snapshot")?;
    let (footer_ok, footer_complete) = match &footer {
        Some((c, complete)) => {
            if c.decls != decl_records || c.modules != module_records {
                bail!(
                    "footer counts ({} decls, {} modules) do not match records ({decl_records}, {module_records}): snapshot is corrupt",
                    c.decls,
                    c.modules
                );
            }
            (true, *complete)
        }
        None => {
            if !opts.allow_partial {
                bail!("snapshot has no footer (truncated?); refusing to import (use --allow-partial to import anyway)");
            }
            (false, false)
        }
    };
    let mut nodes: Vec<(u8, Box<str>)> = Vec::with_capacity(decls.len() + 16);
    for (n, local) in decls.iter() {
        nodes.push((if *local { 0 } else { 1 }, n.clone()));
    }
    for r in refs {
        if !decls.contains_key(&r) {
            nodes.push((2, r));
        }
    }
    nodes.sort_unstable();
    Ok(Pass1 {
        header,
        sha256: hex(&hasher.finalize()),
        nodes,
        decl_records,
        module_records,
        footer_ok,
        footer_complete,
        bytes,
    })
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn parse_flags(list: &[String]) -> u32 {
    let mut f = 0;
    for s in list {
        f |= match s.as_str() {
            "private" => flags::PRIVATE,
            "protected" => flags::PROTECTED,
            "generated" => flags::GENERATED,
            "instance" => flags::INSTANCE,
            "unsafe" => flags::UNSAFE,
            "partial" => flags::PARTIAL,
            "noncomputable" => flags::NONCOMPUTABLE,
            "projection" => flags::PROJECTION,
            "class" => flags::CLASS,
            "structure" => flags::STRUCTURE,
            _ => 0,
        };
    }
    f
}

fn import_flag_bits(flags_list: &[String]) -> i64 {
    let mut f = 0;
    for s in flags_list {
        f |= match s.as_str() {
            "public" => 1,
            "all" => 2,
            "meta" => 4,
            _ => 0,
        };
    }
    f
}

struct ModuleCache {
    ids: HashMap<String, i64>,
}

impl ModuleCache {
    fn load(conn: &Connection) -> Result<ModuleCache> {
        let mut ids = HashMap::new();
        let mut st = conn.prepare("SELECT name, id FROM modules")?;
        for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (n, id) = row?;
            ids.insert(n, id);
        }
        Ok(ModuleCache { ids })
    }
    fn get_or_insert(&mut self, conn: &Connection, name: &str, local: bool) -> Result<i64> {
        if let Some(&id) = self.ids.get(name) {
            return Ok(id);
        }
        conn.execute("INSERT INTO modules(name, is_local) VALUES (?1, ?2)", params![name, local as i64])?;
        let id = conn.last_insert_rowid();
        self.ids.insert(name.to_string(), id);
        Ok(id)
    }
}

/// Imports `snapshot` into the index at `index_path`.
pub fn import_snapshot(snapshot_path: &Path, index_path: &Path, opts: &ImportOptions) -> Result<ImportSummary> {
    let t0 = Instant::now();
    let p1 = pass1(snapshot_path, opts)?;
    let mut summary = ImportSummary { sha256: p1.sha256.clone(), complete_snapshot: p1.footer_ok && p1.footer_complete, ..Default::default() };
    if !p1.footer_ok {
        summary.warnings.push("snapshot has no footer: imported as PARTIAL".into());
    } else if !p1.footer_complete {
        summary.warnings.push("snapshot footer reports an incomplete extraction".into());
    }
    let name_to_id: HashMap<&str, i64> =
        p1.nodes.iter().enumerate().map(|(i, (_, n))| (&**n, i as i64 + 1)).collect();
    let staging = index_path.with_extension("importing");
    if let Some(dir) = staging.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Resume?
    let mut resume_offset = 0u64;
    let mut records_done = 0u64;
    let conn = match try_resume(&staging, &p1.sha256)? {
        Some((c, off, rec)) => {
            resume_offset = off;
            records_done = rec;
            summary.resumed = true;
            c
        }
        None => {
            let c = create(&staging, IndexMode::Semantic)?;
            write_header_meta(&c, &p1.header, &p1.sha256, opts)?;
            set_state(&c, "sha256", &p1.sha256)?;
            set_state(&c, "phase", "loading")?;
            set_state(&c, "offset", "0")?;
            set_state(&c, "records", "0")?;
            c
        }
    };
    // ---- pass 2 ----
    let mut f = std::fs::File::open(snapshot_path)?;
    let total_bytes = p1.bytes;
    let mut offset = 0u64;
    if resume_offset > 0 {
        f.seek(SeekFrom::Start(resume_offset))?;
        offset = resume_offset;
    }
    let mut r = BufReader::with_capacity(1 << 20, f);
    let mut buf = Vec::new();
    let mut mods = ModuleCache::load(&conn)?;
    let mut in_batch = 0usize;
    let mut first_line = resume_offset == 0;
    let mut lean_axioms_present = get_meta(&conn, "lean_axioms_present")?.is_some();
    conn.execute_batch("BEGIN")?;
    {
        let mut ins_decl = conn.prepare(
            "INSERT INTO decls(id, name, display, kind, module_id, origin, flags, type_cov, value_cov,
               start_line, start_col, end_line, end_col, sel_line, sel_col, type_text, sig_json, doc, generated)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
        )?;
        let mut ins_edge = conn.prepare("INSERT OR IGNORE INTO edges(src, kind, dst) VALUES (?1, ?2, ?3)")?;
        let mut ins_lax = conn.prepare("INSERT OR IGNORE INTO lean_axioms(decl_id, axiom_name) VALUES (?1, ?2)")?;
        let mut upd_mod = conn.prepare(
            "UPDATE modules SET path = ?2, is_local = ?3, sha256 = ?4, size = ?5, mtime_ns = ?6, doc = ?7,
               status = CASE WHEN ?8 THEN 'artifact-older-than-source' ELSE 'ok' END WHERE id = ?1",
        )?;
        let mut ins_imp = conn.prepare("INSERT OR IGNORE INTO module_imports(src, dst, flags) VALUES (?1, ?2, ?3)")?;
        loop {
            let n = read_line_bounded(&mut r, &mut buf)?;
            if n == 0 {
                break;
            }
            offset += n as u64;
            if first_line {
                first_line = false;
                continue; // header handled via metadata
            }
            let line = buf.strip_suffix(b"\n").unwrap_or(&buf);
            if line.iter().all(|b| b.is_ascii_whitespace()) {
                continue;
            }
            let l: Line = serde_json::from_slice(line).context("malformed JSON in pass 2")?;
            match l.record.as_str() {
                "module" => {
                    let m = l.into_module()?;
                    let id = mods.get_or_insert(&conn, &m.name, m.local)?;
                    let older = matches!((m.olean_mtime_ns, m.mtime_ns), (Some(o), Some(s)) if o < s);
                    upd_mod.execute(params![id, m.path, m.local as i64, m.sha256, m.size.map(|x| x as i64), m.mtime_ns, m.doc, older])?;
                    for imp in &m.imports {
                        let dst = mods.get_or_insert(&conn, &imp.module, false)?;
                        ins_imp.execute(params![id, dst, import_flag_bits(&imp.flags)])?;
                    }
                    summary.modules += 1;
                }
                "decl" => {
                    let d = l.into_decl()?;
                    let id = *name_to_id.get(d.name.as_str()).context("internal: decl not assigned an id")?;
                    let kind = DeclKind::parse(&d.kind).unwrap_or(DeclKind::Unknown);
                    let module_id = match &d.module {
                        Some(m) => Some(mods.get_or_insert(&conn, m, d.local)?),
                        None => None,
                    };
                    let origin = if d.local { Origin::Local } else { Origin::Library };
                    let mut fl = parse_flags(&d.flags);
                    if d.generated.is_some() {
                        fl |= flags::GENERATED;
                    }
                    if d.doc.as_deref().is_some_and(|x| !x.trim().is_empty()) {
                        fl |= flags::HAS_DOC;
                    }
                    if kind == DeclKind::Instance {
                        fl |= flags::INSTANCE;
                    }
                    let tcov = Coverage::parse(&d.coverage.type_).unwrap_or(Coverage::Partial);
                    let mut vcov = Coverage::parse(&d.coverage.value).unwrap_or(Coverage::Partial);
                    if !kind.has_value() && vcov == Coverage::Complete && d.refs.value.is_empty() {
                        vcov = Coverage::NotApplicable;
                    }
                    let sig = match (&d.binders, &d.conclusion) {
                        (Some(b), Some(c)) => Some(serde_json::json!({"binders": b, "conclusion": c}).to_string()),
                        _ => None,
                    };
                    let (sl, sc, el, ec) = match &d.range {
                        Some(r) => (Some(r.start[0]), Some(r.start[1]), Some(r.end[0]), Some(r.end[1])),
                        None => (None, None, None, None),
                    };
                    let (sell, selc) = match &d.selection {
                        Some(r) => (Some(r.start[0]), Some(r.start[1])),
                        None => (None, None),
                    };
                    ins_decl.execute(params![
                        id,
                        d.name,
                        display_name(&d.name),
                        kind as i64,
                        module_id,
                        origin as i64,
                        fl as i64,
                        tcov as i64,
                        vcov as i64,
                        sl,
                        sc,
                        el,
                        ec,
                        sell,
                        selc,
                        d.type_,
                        sig,
                        d.doc,
                        d.generated,
                    ])?;
                    for (list, ek) in [(&d.refs.type_, EdgeKind::Type), (&d.refs.value, EdgeKind::Value), (&d.refs.ctor, EdgeKind::Ctor)] {
                        for r in list {
                            let dst = *name_to_id.get(r.as_str()).context("internal: ref not assigned an id")?;
                            ins_edge.execute(params![id, ek as i64, dst])?;
                        }
                    }
                    if let Some(ax) = &d.lean_axioms {
                        if !lean_axioms_present {
                            lean_axioms_present = true;
                            set_meta(&conn, "lean_axioms_present", "1")?;
                        }
                        for a in ax {
                            ins_lax.execute(params![id, a])?;
                        }
                    }
                }
                "footer" => {}
                other => bail!("unexpected record {other:?} in pass 2"),
            }
            records_done += 1;
            in_batch += 1;
            if in_batch >= opts.batch_records {
                in_batch = 0;
                set_state(&conn, "offset", &offset.to_string())?;
                set_state(&conn, "records", &records_done.to_string())?;
                conn.execute_batch("COMMIT; BEGIN")?;
                if let Some(p) = opts.progress {
                    p("loading", offset, total_bytes);
                }
                if opts.cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                    conn.execute_batch("COMMIT")?;
                    bail!("import cancelled after {records_done} records (resumable)");
                }
                if opts.stop_after_records.is_some_and(|s| records_done >= s) {
                    conn.execute_batch("COMMIT")?;
                    bail!("import stopped after {records_done} records (test hook; resumable)");
                }
            }
        }
    }
    set_state(&conn, "offset", &offset.to_string())?;
    set_state(&conn, "records", &records_done.to_string())?;
    set_state(&conn, "phase", "finalizing")?;
    conn.execute_batch("COMMIT")?;
    let _ = p1.decl_records;
    let _ = p1.module_records;
    // ---- unresolved nodes ----
    conn.execute_batch("BEGIN")?;
    {
        let existing: i64 = conn.query_row("SELECT count(*) FROM decls", [], |r| r.get(0))?;
        let _ = existing;
        let mut ins = conn.prepare(
            "INSERT OR IGNORE INTO decls(id, name, display, kind, module_id, origin, flags, type_cov, value_cov)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, 0, ?6, ?6)",
        )?;
        for (i, (rank, n)) in p1.nodes.iter().enumerate() {
            if *rank == 2 {
                ins.execute(params![
                    i as i64 + 1,
                    &**n,
                    display_name(n),
                    DeclKind::Unknown as i64,
                    Origin::Unresolved as i64,
                    Coverage::NotExtracted as i64
                ])?;
            }
        }
    }
    conn.execute_batch("COMMIT")?;
    if let Some(p) = opts.progress {
        p("analysing", 0, 1);
    }
    finalize_semantic(&conn, &mut summary)?;
    // Name search side file.
    let generation = new_generation();
    set_meta(&conn, "name_index_generation", &generation)?;
    let ni = build_name_index(&conn, &generation)?;
    set_meta(&conn, "complete", "1")?;
    set_state(&conn, "phase", "done")?;
    drop(conn);
    ni.save(&names_path(index_path))?;
    install(&staging, index_path)?;
    summary.elapsed = t0.elapsed();
    Ok(summary)
}

pub fn names_path(index_path: &Path) -> PathBuf {
    let mut s = index_path.as_os_str().to_owned();
    s.push(".names");
    PathBuf::from(s)
}

fn set_state(conn: &Connection, k: &str, v: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO import_state(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        (k, v),
    )?;
    Ok(())
}

fn get_state(conn: &Connection, k: &str) -> Result<Option<String>> {
    Ok(conn.query_row("SELECT value FROM import_state WHERE key = ?1", [k], |r| r.get(0)).optional()?)
}

fn try_resume(staging: &Path, sha: &str) -> Result<Option<(Connection, u64, u64)>> {
    if !staging.exists() {
        return Ok(None);
    }
    let attempt = || -> Result<Option<(Connection, u64, u64)>> {
        let conn = Connection::open(staging)?;
        conn.pragma_update(None, "synchronous", "OFF")?;
        super::check_schema(&conn)?;
        if get_state(&conn, "sha256")?.as_deref() != Some(sha) || get_state(&conn, "phase")?.as_deref() != Some("loading") {
            return Ok(None);
        }
        let off: u64 = get_state(&conn, "offset")?.unwrap_or_default().parse().unwrap_or(0);
        let rec: u64 = get_state(&conn, "records")?.unwrap_or_default().parse().unwrap_or(0);
        if off == 0 {
            return Ok(None);
        }
        Ok(Some((conn, off, rec)))
    };
    match attempt() {
        Ok(Some(x)) => Ok(Some(x)),
        _ => {
            let _ = std::fs::remove_file(staging);
            Ok(None)
        }
    }
}

fn write_header_meta(conn: &Connection, h: &Header, sha: &str, opts: &ImportOptions) -> Result<()> {
    set_meta(conn, "snapshot_sha256", sha)?;
    set_meta(conn, "snapshot_format_version", &h.format_version.to_string())?;
    set_meta(conn, "producer", &serde_json::to_string(&h.producer)?)?;
    set_meta(conn, "synthetic", if h.synthetic { "1" } else { "0" })?;
    if let Some(e) = &h.exporter {
        set_meta(conn, "exporter", &e.to_string())?;
    }
    if let Some(l) = &h.lean {
        set_meta(conn, "lean_version", &l.version)?;
        set_meta(conn, "lean_githash", &l.githash)?;
        set_meta(conn, "lean_toolchain", &l.toolchain)?;
    }
    if let Some(p) = &h.project {
        set_meta(conn, "project_name", &p.name)?;
        if let Some(r) = &p.root_hint {
            set_meta(conn, "project_root_hint", r)?;
        }
        if let Some(t) = &p.lean_toolchain {
            set_meta(conn, "project_lean_toolchain", t)?;
        }
        if let Some(m) = &p.lake_manifest_sha256 {
            set_meta(conn, "project_lake_manifest_sha256", m)?;
        }
        if let Some(rv) = &p.revision {
            set_meta(conn, "project_revision", rv)?;
        }
    }
    if let Some(s) = &h.scope {
        set_meta(conn, "scope", &s.to_string())?;
    }
    if let Some(c) = &h.created_at {
        set_meta(conn, "created_at", c)?;
    }
    if let Some(b) = &h.build {
        set_meta(conn, "build", &b.to_string())?;
    }
    if let Some(i) = &h.isolation {
        set_meta(conn, "isolation", &i.to_string())?;
    }
    if let Some(r) = &opts.source_root {
        set_meta(conn, "source_root", &r.to_string_lossy())?;
    }
    set_meta(conn, "position_encoding", "codepoint")?;
    set_meta(conn, "imported_at", &super::now_unix().to_string())?;
    Ok(())
}

/// Degrees, axiom analysis and full-text rows for a semantic index.
pub fn finalize_semantic(conn: &Connection, summary: &mut ImportSummary) -> Result<()> {
    let n: i64 = conn.query_row("SELECT coalesce(max(id), 0) FROM decls", [], |r| r.get(0))?;
    let n = n as usize;
    // Load node info.
    let mut kind = vec![DeclKind::Unknown; n];
    let mut origin = vec![Origin::Unresolved; n];
    let mut incomplete = vec![false; n];
    {
        let mut st = conn.prepare("SELECT id, kind, origin, type_cov, value_cov FROM decls")?;
        let rows = st.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, u8>(1)?, r.get::<_, u8>(2)?, r.get::<_, u8>(3)?, r.get::<_, u8>(4)?))
        })?;
        for row in rows {
            let (id, k, o, tc, vc) = row?;
            let i = id as usize - 1;
            kind[i] = DeclKind::from_u8(k);
            origin[i] = Origin::from_u8(o);
            let tc = Coverage::from_u8(tc);
            let vc = Coverage::from_u8(vc);
            incomplete[i] = tc.is_incomplete() || (kind[i].has_value() && vc.is_incomplete()) || origin[i] == Origin::Unresolved;
            match origin[i] {
                Origin::Local => summary.local += 1,
                Origin::Library => summary.library += 1,
                Origin::Unresolved => summary.unresolved += 1,
                Origin::SourceCandidate => {}
            }
        }
    }
    summary.decls = summary.local + summary.library + summary.unresolved;
    let mut edges: Vec<(u32, u8, u32)> = Vec::new();
    {
        let mut st = conn.prepare("SELECT src, kind, dst FROM edges")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, u8>(1)?, r.get::<_, i64>(2)?)))?;
        for row in rows {
            let (s, k, d) = row?;
            edges.push((s as u32 - 1, k, d as u32 - 1));
        }
    }
    summary.edges = edges.len() as u64;
    // Degrees.
    let mut deg = vec![[0u32; 6]; n];
    for &(s, k, d) in &edges {
        let ki = (k as usize).saturating_sub(1).min(2);
        deg[s as usize][ki] += 1;
        deg[d as usize][3 + ki] += 1;
    }
    conn.execute_batch("BEGIN")?;
    {
        let mut st = conn.prepare(
            "UPDATE decls SET out_type=?2, out_value=?3, out_ctor=?4, in_type=?5, in_value=?6, in_ctor=?7 WHERE id=?1",
        )?;
        for (i, d) in deg.iter().enumerate() {
            if d.iter().any(|&x| x > 0) {
                st.execute(params![i as i64 + 1, d[0], d[1], d[2], d[3], d[4], d[5]])?;
            }
        }
        conn.execute(
            "UPDATE modules SET decl_count = (SELECT count(*) FROM decls WHERE decls.module_id = modules.id)",
            [],
        )?;
    }
    conn.execute_batch("COMMIT")?;
    // Axioms.
    let graph = axioms::Graph::from_edges(n, edges);
    let is_axiom: Vec<bool> = kind.iter().zip(&origin).map(|(k, o)| *k == DeclKind::Axiom && *o != Origin::Unresolved).collect();
    let an = axioms::analyze(&graph, &is_axiom, &incomplete);
    conn.execute_batch("BEGIN")?;
    {
        conn.execute(
            "INSERT INTO axioms(id, decl_id, name, category) VALUES (0, NULL, '(coverage gap)', ?1)",
            [AxiomCategory::Incomplete as i64],
        )?;
        let mut ins_ax = conn.prepare("INSERT INTO axioms(id, decl_id, name, category) VALUES (?1, ?2, ?3, ?4)")?;
        let mut get_name = conn.prepare("SELECT name FROM decls WHERE id = ?1")?;
        for (k, &v) in an.axioms.iter().enumerate() {
            let name: String = get_name.query_row([v as i64 + 1], |r| r.get(0))?;
            ins_ax.execute(params![k as i64 + 1, v as i64 + 1, name, AxiomCategory::of_name(&name) as i64])?;
        }
        let mut ins = conn.prepare("INSERT INTO decl_axioms(decl_id, axiom_id, via, via_kind) VALUES (?1, ?2, ?3, ?4)")?;
        for w in &an.witnesses {
            let (via, vk) = match w.via {
                Some((x, k)) => (Some(x as i64 + 1), Some(k as i64)),
                None => (None, None),
            };
            ins.execute(params![w.node as i64 + 1, w.axiom as i64, via, vk])?;
        }
    }
    conn.execute_batch("COMMIT")?;
    summary.axioms = an.axioms.len() as u64;
    // Full-text rows for declarations with documentation or statement text.
    conn.execute_batch("BEGIN")?;
    conn.execute(
        "INSERT INTO fts(rowid, doc, attrs, sig) SELECT id, coalesce(doc, ''), '', coalesce(type_text, '')
           FROM decls WHERE doc IS NOT NULL OR type_text IS NOT NULL",
        [],
    )?;
    conn.execute_batch("COMMIT")?;
    finish_bulk_load(conn)?;
    write_module_stats(conn)?;
    set_meta(conn, "decl_count", &summary.decls.to_string())?;
    set_meta(conn, "local_decl_count", &summary.local.to_string())?;
    set_meta(conn, "edge_count", &summary.edges.to_string())?;
    let _ = get_meta(conn, "mode")?;
    Ok(())
}

/// Per-module counts for the outline's roll-ups: user-written declarations
/// by kind, documented ones, and those whose dependency closure reaches
/// `sorryAx`. Needs the secondary indexes and the axiom analysis.
fn write_module_stats(conn: &Connection) -> Result<()> {
    let g = flags::GENERATED;
    let hd = flags::HAS_DOC;
    let rows: Vec<[i64; 7]> = conn
        .prepare(&format!(
            "SELECT module_id, sum(kind = 0), sum(kind IN (1, 2, 10)), sum(kind IN (4, 5, 6)), sum(kind = 3), sum(kind = 9),
                    sum((flags & {hd}) != 0 AND kind IN (0, 1, 2, 3, 4, 5, 6, 9, 10))
               FROM decls WHERE module_id IS NOT NULL AND (flags & {g}) = 0 GROUP BY module_id"
        ))?
        .query_map([], |r| Ok([r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?]))?
        .collect::<rusqlite::Result<_>>()?;
    let sorry: Vec<(i64, i64)> = conn
        .prepare(&format!(
            "SELECT d.module_id, count(*) FROM decl_axioms da JOIN axioms a ON a.id = da.axiom_id JOIN decls d ON d.id = da.decl_id
              WHERE a.category = {} AND (d.flags & {g}) = 0 AND d.module_id IS NOT NULL GROUP BY d.module_id",
            AxiomCategory::Sorry as i64
        ))?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    conn.execute_batch("BEGIN")?;
    {
        let mut st = conn.prepare("UPDATE modules SET n_thm = ?2, n_def = ?3, n_type = ?4, n_inst = ?5, n_axiom = ?6, n_doc = ?7 WHERE id = ?1")?;
        for r in &rows {
            st.execute(params![r[0], r[1], r[2], r[3], r[4], r[5], r[6]])?;
        }
        let mut st = conn.prepare("UPDATE modules SET n_sorry = ?2 WHERE id = ?1")?;
        for (m, n) in &sorry {
            st.execute(params![m, n])?;
        }
    }
    conn.execute_batch("COMMIT")?;
    Ok(())
}

/// Builds the name-search index from the `decls` table.
pub fn build_name_index(conn: &Connection, generation: &str) -> Result<NameIndex> {
    let mut st = conn.prepare("SELECT id, display, kind, origin, flags, coalesce(module_id, 0) FROM decls")?;
    let rows = st.query_map([], |r| {
        Ok(nameindex::Entry {
            id: r.get::<_, i64>(0)? as u32,
            display: r.get(1)?,
            kind: DeclKind::from_u8(r.get::<_, u8>(2)?),
            origin: Origin::from_u8(r.get::<_, u8>(3)?),
            flags: r.get::<_, i64>(4)? as u32,
            module: r.get::<_, i64>(5)?.clamp(0, u32::MAX as i64) as u32,
        })
    })?;
    let mut v = Vec::new();
    for e in rows {
        v.push(e?);
    }
    NameIndex::build(v, generation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::*;

    pub fn tiny_snapshot(complete: bool) -> String {
        let h = Header {
            record: "header".into(),
            format: FORMAT.into(),
            format_version: FORMAT_VERSION,
            producer: Producer { name: "test".into(), version: "0".into() },
            exporter: None,
            lean: Some(LeanInfo { version: "4.33.1".into(), githash: "x".into(), toolchain: "leanprover/lean4:v4.33.1".into() }),
            project: Some(ProjectInfo { name: "t".into(), ..Default::default() }),
            scope: None,
            created_at: None,
            build: None,
            isolation: None,
            synthetic: true,
        };
        let mut lines = vec![header_line(&h)];
        lines.push(module_line(&ModuleRec { name: "T.A".into(), local: true, path: Some("T/A.lean".into()), ..Default::default() }));
        let d = |name: &str, kind: &str, ty: &[&str], val: &[&str], local: bool| DeclRec {
            name: name.into(),
            kind: kind.into(),
            module: Some(if local { "T.A".into() } else { "Init".into() }),
            local,
            refs: Refs { type_: ty.iter().map(|s| s.to_string()).collect(), value: val.iter().map(|s| s.to_string()).collect(), ctor: vec![] },
            coverage: if kind == "axiom" { CoverageRec { type_: "complete".into(), value: "not_applicable".into() } } else { CoverageRec::default() },
            ..Default::default()
        };
        lines.push(decl_line(&d("T.main", "theorem", &["T.P"], &["T.helper", "propext"], true)));
        lines.push(decl_line(&d("T.helper", "theorem", &["T.P"], &["sorryAx"], true)));
        lines.push(decl_line(&d("T.P", "def", &[], &["Nat"], true)));
        lines.push(decl_line(&d("propext", "axiom", &[], &[], false)));
        lines.push(decl_line(&d("sorryAx", "axiom", &[], &[], false)));
        if complete {
            lines.push(footer_line(&Footer { counts: Counts { modules: 1, decls: 5 }, complete: true }));
        }
        lines.join("\n") + "\n"
    }

    #[test]
    fn import_assigns_ids_and_computes_axioms() {
        let dir = tempfile::tempdir().unwrap();
        let snap = dir.path().join("s.jsonl");
        std::fs::write(&snap, tiny_snapshot(true)).unwrap();
        let idx = dir.path().join("i.sqlite");
        let s = import_snapshot(&snap, &idx, &ImportOptions::default()).unwrap();
        assert_eq!(s.local, 3);
        assert_eq!(s.library, 2);
        assert_eq!(s.unresolved, 1); // Nat
        let conn = super::super::open_read(&idx).unwrap();
        // local first, sorted by name
        let first: String = conn.query_row("SELECT name FROM decls WHERE id = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(first, "T.P");
        let axs: Vec<String> = conn
            .prepare("SELECT a.name FROM decl_axioms da JOIN axioms a ON a.id = da.axiom_id JOIN decls d ON d.id = da.decl_id WHERE d.name = 'T.main' ORDER BY a.name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|x| x.unwrap())
            .collect();
        // Nat is unresolved, so T.P (and everything using it) has a coverage gap.
        assert_eq!(axs, vec!["(coverage gap)", "propext", "sorryAx"]);
        assert!(names_path(&idx).exists());
    }

    #[test]
    fn truncated_snapshot_is_refused_and_previous_index_kept() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good.jsonl");
        std::fs::write(&good, tiny_snapshot(true)).unwrap();
        let idx = dir.path().join("i.sqlite");
        import_snapshot(&good, &idx, &ImportOptions::default()).unwrap();
        let before = std::fs::read(&idx).unwrap();
        let bad = dir.path().join("bad.jsonl");
        std::fs::write(&bad, tiny_snapshot(false)).unwrap();
        let err = import_snapshot(&bad, &idx, &ImportOptions::default()).unwrap_err();
        assert!(err.to_string().contains("no footer"), "{err}");
        assert_eq!(std::fs::read(&idx).unwrap(), before, "previous index untouched");
        // allow_partial imports with a warning
        let s = import_snapshot(&bad, &idx, &ImportOptions { allow_partial: true, ..Default::default() }).unwrap();
        assert!(!s.complete_snapshot);
    }

    #[test]
    fn malformed_and_hostile_input_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let idx = dir.path().join("i.sqlite");
        let base = tiny_snapshot(true);
        let cases = [
            base.replace("T/A.lean", "../../etc/passwd"),
            base.replace("\"T.helper\",\"kind\"", "\"T.main\",\"kind\""),
            base.replacen("{\"record\":\"module\"", "{\"record\":\"bogus\"", 1),
            base.replace("\"decls\":5", "\"decls\":6"),
            "not json\n".to_string(),
        ];
        for (i, c) in cases.iter().enumerate() {
            let p = dir.path().join(format!("c{i}.jsonl"));
            std::fs::write(&p, c).unwrap();
            assert!(import_snapshot(&p, &idx, &ImportOptions::default()).is_err(), "case {i} accepted");
        }
        assert!(!idx.exists());
    }

    #[test]
    fn interrupted_import_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let snap = dir.path().join("s.jsonl");
        std::fs::write(&snap, tiny_snapshot(true)).unwrap();
        let idx = dir.path().join("i.sqlite");
        let opts = ImportOptions { batch_records: 2, stop_after_records: Some(2), ..Default::default() };
        assert!(import_snapshot(&snap, &idx, &opts).is_err());
        assert!(!idx.exists());
        let s = import_snapshot(&snap, &idx, &ImportOptions { batch_records: 2, ..Default::default() }).unwrap();
        assert!(s.resumed);
        assert_eq!(s.local, 3);
        let conn = super::super::open_read(&idx).unwrap();
        let n: i64 = conn.query_row("SELECT count(*) FROM edges", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 6);
    }
}
