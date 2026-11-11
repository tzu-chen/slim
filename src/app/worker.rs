//! Background workers. The UI thread never touches SQLite, the file system
//! (beyond terminal I/O) or the name index: it sends requests tagged with a
//! generation number and drops responses that are no longer the newest.
//!
//! * the query worker owns a read-only index connection and the user-data
//!   store; it coalesces superseded requests of the same kind;
//! * the search worker owns the in-memory name index; an atomic "latest
//!   generation" lets it abandon an obsolete scan mid-way;
//! * the job runner executes one long task at a time (freshness check,
//!   incremental rescan, initial source build, snapshot import) with a
//!   cancellation flag and throttled progress events.

use super::Event;
use crate::db::{self, query::*, IndexMode};
use crate::map::{MapLoad, Resolution};
use crate::model::flags;
use crate::imports::{DirSummary, ImportGraph, PathAnswer};
use crate::nameindex::{ModuleSet, NameIndex, Scope, ScopeKind, ScopeSpec, SearchOutcome, SearchParams};
use crate::skim::{Skim, SkimScope};
use crate::paths::Project;
use crate::source::index::{self as sindex, Freshness, ScanConfig};
use crate::sourceview::{excerpt, SourceCache};
use crate::text::PosEncoding;
pub use crate::relations::Mention;
use crate::relations::{ChildPage, ChildQuery, ConnQuery, ConnResult, Env, Limits};
use crate::userdata::{Mark, Question, ReadState, RefKind, Stop, StopStatus, Trail, UserStore};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct IndexCtx {
    pub index: PathBuf,
    pub source_root: Option<PathBuf>,
    pub mode: IndexMode,
    pub user_key: String,
    pub encoding: PosEncoding,
    /// Explicit project-map file (`--map`); otherwise `lean-view.toml` in the
    /// source root.
    pub map_path: Option<PathBuf>,
}

/// The project map as loaded, and how its names resolve in the open index.
#[derive(Clone, Debug)]
pub struct MapBundle {
    pub load: MapLoad,
    pub resolution: Resolution,
}

/// The reader's trails and questions, with every stop checked against the
/// open index.
#[derive(Clone, Debug, Default)]
pub struct Reading {
    pub trails: Vec<Trail>,
    pub statuses: HashMap<i64, StopStatus>,
    pub questions: Vec<Question>,
    pub prefs: HashMap<String, String>,
    /// Trail (and stop) to select after this change.
    pub focus: Option<(i64, Option<i64>)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SourceStatus {
    Ok,
    /// File content differs from the fingerprint recorded in the index.
    Changed,
    Missing(String),
    NoPath,
    NoRoot,
    Rejected(String),
}

#[derive(Clone, Debug)]
pub struct DetailBundle {
    pub d: DeclDetail,
    pub excerpt: Vec<(usize, String)>,
    pub excerpt_truncated: bool,
    pub decl_lines: Option<(usize, usize)>,
    pub source_status: SourceStatus,
    /// SHA-256 of the file the excerpt was read from.
    pub source_sha256: Option<String>,
    /// Source-only mode: the statement header text read from the file.
    pub statement_src: Option<String>,
    /// Where that text starts in the file: 1-based line and byte column.
    pub statement_at: Option<(usize, usize)>,
    pub bookmarked: bool,
}

#[derive(Clone, Debug)]
pub struct FullSource {
    pub rel_path: String,
    /// SHA-256 of the text these lines come from (highlight data is only
    /// applied to text with the same hash).
    pub sha256: String,
    pub lines: Vec<String>,
    pub focus_line: usize,
    pub highlight: Option<(usize, usize)>,
    pub changed: bool,
}

/// Module outline as a tree of dotted-name components, built on the query
/// worker (not on the UI thread: CFSG has 229k modules).
#[derive(Clone, Debug)]
pub struct OutlineNode {
    pub label: String,
    pub full: String,
    pub module: Option<ModuleRow>,
    pub children: Vec<usize>,
    pub depth: u16,
    pub expanded: bool,
    pub decls_total: u64,
    pub parent: Option<usize>,
    /// Counts of every file at or below this node.
    pub roll: ModStats,
}

#[derive(Clone, Debug, Default)]
pub struct OutlineTree {
    pub nodes: Vec<OutlineNode>,
    pub roots: Vec<usize>,
    /// Full dotted name → node index.
    pub by_name: HashMap<String, usize>,
}

pub fn build_outline_tree(mods: Vec<(ModuleRow, ModStats)>) -> OutlineTree {
    let mut t = OutlineTree::default();
    for (m, st) in mods {
        let comps: Vec<&str> = crate::model::name_components(&m.name);
        let mut parent: Option<usize> = None;
        let count = m.decl_count;
        let mut full = String::new();
        for (k, c) in comps.iter().enumerate() {
            if k > 0 {
                full.push('.');
            }
            full.push_str(c);
            let idx = match t.by_name.get(&full) {
                Some(&i) => i,
                None => {
                    t.nodes.push(OutlineNode {
                        label: c.to_string(),
                        full: full.clone(),
                        module: None,
                        children: Vec::new(),
                        depth: k as u16,
                        expanded: false,
                        decls_total: 0,
                        parent,
                        roll: ModStats::default(),
                    });
                    let i = t.nodes.len() - 1;
                    t.by_name.insert(full.clone(), i);
                    match parent {
                        Some(p) => t.nodes[p].children.push(i),
                        None => t.roots.push(i),
                    }
                    i
                }
            };
            t.nodes[idx].decls_total += count;
            t.nodes[idx].roll.add(&st);
            parent = Some(idx);
        }
        if let Some(p) = parent {
            t.nodes[p].module = Some(m);
        }
    }
    if t.roots.len() == 1 {
        let r = t.roots[0];
        t.nodes[r].expanded = true;
    }
    t
}

#[derive(Clone, Debug)]
pub enum UserData {
    Bookmarks(Vec<Mark>),
    BookmarkNames(Vec<String>),
    Reading(Box<Reading>),
}

pub enum QueryReq {
    Detail { gen: u64, id: i64 },
    Neighbors { gen: u64, id: i64, dir: Direction, filter: NeighborFilter, limit: usize, offset: usize, parent: Option<u64> },
    Mentions { gen: u64, id: i64 },
    Evidence { gen: u64, id: i64 },
    ModuleList { gen: u64, local_only: bool },
    ModuleDecls { gen: u64, module_id: i64, show_generated: bool },
    ModuleImports { gen: u64, module_id: i64 },
    FullSource { gen: u64, rel_path: String, focus_line: usize, highlight: Option<(usize, usize)>, sha256: Option<String> },
    Rows { gen: u64, ids: Vec<i64> },
    ResolveName { gen: u64, name: String },
    TransitiveCount { gen: u64, id: i64, dir: Direction },
    Info { gen: u64 },
    LoadBookmarks { gen: u64 },
    ToggleBookmark { mark: Mark },
    SetBookmarkNote { name: String, note: Option<String> },
    /// Detail of a declaration (by name) for the map, goal and trail views.
    ViewDetail { gen: u64, name: String },
    GoalChildren { gen: u64, node: u64, q: ChildQuery },
    ReloadMap,
    LoadReading,
    TrailCreate { title: String },
    TrailRename { id: i64, title: String },
    TrailDelete { id: i64 },
    /// Adds a stop to `trail`, or to the active trail (created on demand).
    StopAdd { trail: Option<i64>, stop: Stop },
    StopSetNote { id: i64, note: Option<String> },
    StopSetQuestion { id: i64, question: Option<String> },
    StopSetState { id: i64, state: ReadState },
    StopRemove { id: i64 },
    StopMove { id: i64, delta: i64 },
    /// Copies a trail shipped with the map into the reader's data.
    StartShared { id: String },
    QuestionAdd { ref_kind: RefKind, name: String, display: String, text: String },
    QuestionToggle { id: i64 },
    QuestionRemove { id: i64 },
    SetPref { key: String, value: Option<String> },
    ExportTrail { trail: i64, path: PathBuf, index_desc: String },
    SaveSession { state: String },
    /// Statements of a file, directory, namespace or map group.
    Skim { gen: u64, scope: SkimScope, show_generated: bool },
    /// The `variable` and `open` lines in force at a declaration.
    Scope { gen: u64, id: i64 },
    /// The card of a declaration: statement, lead of its documentation.
    Peek { gen: u64, id: i64 },
    /// The section headings of a module's doc blocks.
    ModuleSections { module_id: i64 },
    /// The import graph (read once, then kept by the worker).
    Imports { gen: u64 },
    /// What a directory imports from the rest of the project, and what imports it.
    ImportsDir { gen: u64, full: String },
    /// "Why does `from` import `to`": a chain of import lines.
    ImportPath { gen: u64, from: String, to: String },
    Reopen,
    Shutdown,
}

impl QueryReq {
    fn timing_key(&self) -> &'static str {
        match self {
            QueryReq::Detail { .. } => "worker: detail",
            QueryReq::Neighbors { .. } => "worker: neighbours",
            QueryReq::Mentions { .. } => "worker: lexical mentions",
            QueryReq::Evidence { .. } => "worker: axiom evidence",
            QueryReq::ModuleList { .. } => "worker: outline tree",
            QueryReq::ModuleDecls { .. } => "worker: module declarations",
            QueryReq::ModuleImports { .. } => "worker: module imports",
            QueryReq::FullSource { .. } => "worker: full source",
            QueryReq::ViewDetail { .. } => "worker: view detail",
            QueryReq::GoalChildren { .. } => "worker: goal children",
            QueryReq::Skim { .. } => "worker: skim",
            QueryReq::Scope { .. } => "worker: scope of a declaration",
            QueryReq::Peek { .. } => "worker: card",
            QueryReq::ModuleSections { .. } => "worker: module sections",
            QueryReq::Imports { .. } => "worker: import graph",
            QueryReq::ImportsDir { .. } => "worker: imports of a directory",
            QueryReq::ImportPath { .. } => "worker: import path",
            QueryReq::ReloadMap => "worker: other",
            _ => "worker: other",
        }
    }

    /// Requests of the same kind supersede each other (newest wins).
    fn coalesce_key(&self) -> Option<u8> {
        Some(match self {
            QueryReq::Detail { .. } => 1,
            QueryReq::Evidence { .. } => 2,
            QueryReq::Mentions { .. } => 3,
            QueryReq::FullSource { .. } => 4,
            QueryReq::ModuleImports { .. } => 5,
            QueryReq::TransitiveCount { .. } => 6,
            QueryReq::Neighbors { parent: None, .. } => 7,
            QueryReq::ViewDetail { .. } => 8,
            QueryReq::Skim { .. } => 9,
            QueryReq::ImportsDir { .. } => 10,
            QueryReq::Scope { .. } => 11,
            QueryReq::Peek { .. } => 12,
            _ => return None,
        })
    }
}

pub enum QueryResp {
    Detail(u64, Result<Box<DetailBundle>, String>),
    Neighbors(u64, Option<u64>, usize, Result<Vec<NeighborGroup>, String>),
    Mentions(u64, Result<Vec<Mention>, String>),
    Evidence(u64, Result<Vec<AxiomEvidence>, String>),
    ModuleTree(u64, Result<Box<OutlineTree>, String>),
    ModuleDecls(u64, i64, Result<(Vec<OutlineItem>, bool), String>),
    ModuleImports(u64, Result<(Option<ModuleRow>, Vec<ImportEdge>, u64, Vec<ImportEdge>, u64, Option<ModStats>), String>),
    FullSource(u64, Result<FullSource, String>),
    Rows(u64, Result<Vec<DeclRow>, String>),
    Resolved(u64, String, Option<i64>),
    TransitiveCount(u64, Direction, Result<(u64, bool), String>),
    Info(u64, Result<IndexInfo, String>),
    User(u64, UserData, String),
    Map(Box<MapBundle>),
    ViewDetail(u64, String, Result<Option<Box<DetailBundle>>, String>),
    GoalChildren(u64, u64, usize, Result<ChildPage, String>),
    Connection(u64, Box<ConnResult>),
    Skim(u64, Result<Box<Skim>, String>),
    Scope(u64, i64, Result<crate::scope::DeclScope, String>),
    Peek(u64, i64, Result<Option<Box<crate::skim::Peek>>, String>),
    ModuleSections(i64, Vec<(u8, String, u32)>),
    Imports(u64, Result<Arc<ImportGraph>, String>),
    ImportsDir(u64, Result<DirSummary, String>),
    ImportPath(u64, Result<PathAnswer, String>),
    Message(String),
    Error(String),
}

pub fn spawn_query_worker(ctx: IndexCtx, tx: Sender<Event>) -> (Sender<QueryReq>, std::thread::JoinHandle<()>) {
    let (qtx, qrx) = std::sync::mpsc::channel::<QueryReq>();
    let h = std::thread::Builder::new()
        .name("slim-query".into())
        .spawn(move || query_loop(ctx, qrx, tx))
        .expect("spawn query worker");
    (qtx, h)
}

fn query_loop(ctx: IndexCtx, rx: Receiver<QueryReq>, tx: Sender<Event>) {
    let mut conn = match db::open_read(&ctx.index) {
        Ok(c) => Some(c),
        Err(e) => {
            let _ = tx.send(Event::Query(QueryResp::Error(format!("cannot open index: {e:#}"))));
            None
        }
    };
    let users = UserStore::open().map_err(|e| format!("{e:#}"));
    if let Err(e) = &users {
        let _ = tx.send(Event::Query(QueryResp::Error(format!("user data unavailable: {e}"))));
    }
    let mut cache = SourceCache::default();
    // Loaded when the import view first asks; dropped with the index.
    let mut imports: Option<Arc<ImportGraph>> = None;
    let mut map = MapBundle { load: MapLoad::Absent { looked_at: None }, resolution: Resolution::default() };
    if !publish_map(&ctx, conn.as_ref(), &mut map, &tx) {
        return;
    }
    while let Ok(first) = rx.recv() {
        // Drain the queue and keep only the newest request of each
        // coalescable kind (order of the remaining requests is preserved).
        let mut batch = vec![first];
        while let Ok(r) = rx.try_recv() {
            batch.push(r);
        }
        let mut seen: HashSet<u8> = HashSet::new();
        let mut keep = vec![true; batch.len()];
        for i in (0..batch.len()).rev() {
            if let Some(k) = batch[i].coalesce_key() {
                if !seen.insert(k) {
                    keep[i] = false;
                }
            }
        }
        for (req, k) in batch.into_iter().zip(keep) {
            if !k {
                continue;
            }
            match req {
                QueryReq::Shutdown => return,
                QueryReq::Reopen | QueryReq::ReloadMap => {
                    if matches!(req, QueryReq::Reopen) {
                        conn = db::open_read(&ctx.index).ok();
                        cache = SourceCache::default();
                        imports = None;
                    }
                    // The index or the sidecar changed: resolve the map again.
                    if !publish_map(&ctx, conn.as_ref(), &mut map, &tx) {
                        return;
                    }
                }
                req => {
                    let key = req.timing_key();
                    let t = Instant::now();
                    let resp = match (&conn, &users) {
                        (Some(c), u) => handle(&ctx, c, u.as_ref().ok(), &mut cache, &map, &mut imports, req),
                        (None, _) => QueryResp::Error("index not available".into()),
                    };
                    super::timing::record(key, t.elapsed().as_secs_f64() * 1000.0);
                    if tx.send(Event::Query(resp)).is_err() {
                        return;
                    }
                }
            }
        }
    }
}

fn s<T>(r: anyhow::Result<T>) -> Result<T, String> {
    r.map_err(|e| format!("{e:#}"))
}

/// Loads the map and sends it to the UI: at once with plain resolution, and
/// again with rename hints when some name is missing (the hints scan the
/// declaration table, which takes a moment on a multi-million-name index).
fn publish_map(ctx: &IndexCtx, conn: Option<&rusqlite::Connection>, map: &mut MapBundle, tx: &Sender<Event>) -> bool {
    let t = Instant::now();
    *map = load_map_with(ctx, conn, false);
    super::timing::record("worker: map load + resolve", t.elapsed().as_secs_f64() * 1000.0);
    if tx.send(Event::Query(QueryResp::Map(Box::new(map.clone())))).is_err() {
        return false;
    }
    if let (Some(m), Some(c), true) = (map.load.map().cloned(), conn, map.resolution.has_missing()) {
        let t = Instant::now();
        if let Ok(r) = crate::map::resolve_with(&m, c, true) {
            map.resolution = r;
            super::timing::record("worker: map rename hints", t.elapsed().as_secs_f64() * 1000.0);
            return tx.send(Event::Query(QueryResp::Map(Box::new(map.clone())))).is_ok();
        }
    }
    true
}

/// Reads the sidecar (never writes it) and resolves its names in the index.
pub fn load_map(ctx: &IndexCtx, conn: Option<&rusqlite::Connection>) -> MapBundle {
    load_map_with(ctx, conn, true)
}

fn load_map_with(ctx: &IndexCtx, conn: Option<&rusqlite::Connection>, hints: bool) -> MapBundle {
    let load = match (&ctx.map_path, &ctx.source_root) {
        (Some(p), _) => crate::map::load(p),
        (None, Some(root)) => crate::map::load_for_root(root),
        (None, None) => MapLoad::Absent { looked_at: None },
    };
    let resolution = match (load.map(), conn) {
        (Some(m), Some(c)) => crate::map::resolve_with(m, c, hints).unwrap_or_default(),
        _ => Resolution::default(),
    };
    MapBundle { load, resolution }
}

/// A trail stop for a declaration name, with the locators the index has now.
pub fn stop_for_name(c: &rusqlite::Connection, name: &str, index_desc: &str) -> Stop {
    let mut s = Stop { name: name.to_string(), display: name.to_string(), kind: "unknown".into(), index_desc: Some(index_desc.to_string()), ..Default::default() };
    if let Ok(Some((row, _, sha))) = crate::map::resolve_name(c, name) {
        s.display = row.display.clone();
        s.kind = row.kind.as_str().into();
        s.module = row.module.clone();
        s.sha256 = sha;
        if let Ok(Some(d)) = detail(c, row.id) {
            s.path = d.module_path;
            s.line = d.pos.start_line;
        }
    }
    s
}

fn index_desc(ctx: &IndexCtx, c: &rusqlite::Connection) -> String {
    match ctx.mode {
        IndexMode::Source => "source-only index".into(),
        IndexMode::Semantic => format!(
            "semantic snapshot {}",
            db::get_meta(c, "snapshot_sha256").ok().flatten().map(|s| s[..s.len().min(16)].to_string()).unwrap_or_else(|| "?".into())
        ),
    }
}

fn reading(ctx: &IndexCtx, c: &rusqlite::Connection, u: &UserStore, map: &MapBundle, focus: Option<(i64, Option<i64>)>) -> anyhow::Result<Reading> {
    let trails = u.trails(&ctx.user_key)?;
    let mut statuses = HashMap::new();
    for t in &trails {
        statuses.extend(crate::userdata::stop_statuses(c, map.load.map().map(|m| m.as_ref()), t));
    }
    Ok(Reading { trails, statuses, questions: u.questions(&ctx.user_key)?, prefs: u.prefs(&ctx.user_key)?, focus })
}

/// The trail new stops go to: the reader's active trail, else their only
/// trail, else a new "Reading trail".
fn active_trail(ctx: &IndexCtx, u: &UserStore) -> anyhow::Result<i64> {
    let trails = u.trails(&ctx.user_key)?;
    if let Some(id) = u.pref(&ctx.user_key, "active_trail")?.and_then(|s| s.parse::<i64>().ok()) {
        if trails.iter().any(|t| t.id == id) {
            return Ok(id);
        }
    }
    let id = match trails.first() {
        Some(t) => t.id,
        None => u.trail_create(&ctx.user_key, "Reading trail", None, "personal")?,
    };
    u.set_pref(&ctx.user_key, "active_trail", Some(&id.to_string()))?;
    Ok(id)
}

/// The import graph, read from the index the first time it is needed.
fn import_graph(c: &rusqlite::Connection, slot: &mut Option<Arc<ImportGraph>>) -> anyhow::Result<Arc<ImportGraph>> {
    if let Some(g) = slot {
        return Ok(g.clone());
    }
    let g = Arc::new(ImportGraph::load(c)?);
    *slot = Some(g.clone());
    Ok(g)
}

fn handle(ctx: &IndexCtx, c: &rusqlite::Connection, users: Option<&UserStore>, cache: &mut SourceCache, map: &MapBundle, imports: &mut Option<Arc<ImportGraph>>, req: QueryReq) -> QueryResp {
    // Changes to trails and questions answer with the whole reading state.
    let read_op = |f: &dyn Fn(&UserStore) -> anyhow::Result<(String, Option<(i64, Option<i64>)>)>| -> QueryResp {
        let Some(u) = users else { return QueryResp::Error("user data unavailable".into()) };
        match f(u).and_then(|(msg, focus)| Ok((msg, reading(ctx, c, u, map, focus)?))) {
            Ok((msg, r)) => QueryResp::User(0, UserData::Reading(Box::new(r)), msg),
            Err(e) => QueryResp::Error(format!("{e:#}")),
        }
    };
    match req {
        QueryReq::ViewDetail { gen, name } => {
            let r = (|| -> anyhow::Result<Option<Box<DetailBundle>>> {
                match decl_by_name(c, &name)? {
                    Some(id) => Ok(Some(Box::new(detail_bundle(ctx, c, users, cache, id)?))),
                    None => Ok(None),
                }
            })();
            QueryResp::ViewDetail(gen, name, s(r))
        }
        QueryReq::GoalChildren { gen, node, q } => {
            let mut env = Env { conn: c, mode: ctx.mode, map: map.load.map().map(|m| m.as_ref()), source_root: ctx.source_root.as_deref(), cache };
            QueryResp::GoalChildren(gen, node, q.offset, s(crate::relations::children(&mut env, &q)))
        }
        QueryReq::Skim { gen, scope, show_generated } => {
            let mut env = crate::skim::Env { conn: c, mode: ctx.mode, source_root: ctx.source_root.as_deref(), cache, map: map.load.map().map(|m| m.as_ref()), show_generated };
            QueryResp::Skim(gen, s(crate::skim::skim(&mut env, &scope)).map(Box::new))
        }
        QueryReq::Scope { gen, id } => {
            let r = (|| -> anyhow::Result<crate::scope::DeclScope> {
                let d = detail(c, id)?.ok_or_else(|| anyhow::anyhow!("declaration {id} not found"))?;
                crate::scope::decl_scope(c, ctx.source_root.as_deref(), cache, &d, ctx.mode == IndexMode::Source)
            })();
            QueryResp::Scope(gen, id, s(r))
        }
        QueryReq::Peek { gen, id } => {
            let mut env = crate::skim::Env { conn: c, mode: ctx.mode, source_root: ctx.source_root.as_deref(), cache, map: None, show_generated: true };
            QueryResp::Peek(gen, id, s(crate::skim::peek(&mut env, id)).map(|p| p.map(Box::new)))
        }
        QueryReq::ModuleSections { module_id } => {
            QueryResp::ModuleSections(module_id, crate::scope::sections(c, ctx.source_root.as_deref(), cache, module_id).unwrap_or_default())
        }
        QueryReq::Imports { gen } => QueryResp::Imports(gen, s(import_graph(c, imports))),
        QueryReq::ImportsDir { gen, full } => QueryResp::ImportsDir(
            gen,
            s(import_graph(c, imports).and_then(|g| {
                let d = g.dir(&full).ok_or_else(|| anyhow::anyhow!("{full} is not in the import graph any more"))?;
                Ok(g.dir_summary(d, 12))
            })),
        ),
        QueryReq::ImportPath { gen, from, to } => QueryResp::ImportPath(gen, s(import_graph(c, imports).map(|g| g.explain(&from, &to)))),
        QueryReq::LoadReading => read_op(&|_| Ok((String::new(), None))),
        QueryReq::TrailCreate { title } => read_op(&|u| {
            let id = u.trail_create(&ctx.user_key, &title, None, "personal")?;
            u.set_pref(&ctx.user_key, "active_trail", Some(&id.to_string()))?;
            Ok((format!("created trail “{}” (now the active trail)", title.trim()), Some((id, None))))
        }),
        QueryReq::TrailRename { id, title } => read_op(&|u| {
            u.trail_rename(&ctx.user_key, id, &title)?;
            Ok(("trail renamed".into(), Some((id, None))))
        }),
        QueryReq::TrailDelete { id } => read_op(&|u| {
            u.trail_delete(&ctx.user_key, id)?;
            Ok(("trail deleted".into(), None))
        }),
        QueryReq::StopAdd { trail, stop } => read_op(&|u| {
            let tid = match trail {
                Some(t) => t,
                None => active_trail(ctx, u)?,
            };
            let mut st = stop.clone();
            if st.index_desc.is_none() {
                st.index_desc = Some(index_desc(ctx, c));
            }
            let sid = u.stop_add(&ctx.user_key, tid, &st)?;
            let title = u.trail(&ctx.user_key, tid)?.map(|t| t.title).unwrap_or_default();
            Ok((format!("added {} to the trail “{title}”", st.display), Some((tid, Some(sid)))))
        }),
        QueryReq::StopSetNote { id, note } => read_op(&|u| {
            u.stop_set_note(&ctx.user_key, id, note.as_deref())?;
            Ok(("note saved".into(), None))
        }),
        QueryReq::StopSetQuestion { id, question } => read_op(&|u| {
            u.stop_set_question(&ctx.user_key, id, question.as_deref())?;
            Ok(("question saved".into(), None))
        }),
        QueryReq::StopSetState { id, state } => read_op(&|u| {
            u.stop_set_state(&ctx.user_key, id, state)?;
            Ok((format!("marked {} (your annotation, not Lean verification)", state.as_str()), None))
        }),
        QueryReq::StopRemove { id } => read_op(&|u| {
            u.stop_remove(&ctx.user_key, id)?;
            Ok(("removed from the trail".into(), None))
        }),
        QueryReq::StopMove { id, delta } => read_op(&|u| {
            let moved = u.stop_move(&ctx.user_key, id, delta)?;
            Ok((if moved { String::new() } else { "already at the end of the trail".into() }, None))
        }),
        QueryReq::StartShared { id } => read_op(&|u| {
            let m = map.load.map().ok_or_else(|| anyhow::anyhow!("no project map is loaded"))?;
            let sh = m.trails.iter().find(|t| t.id == id).ok_or_else(|| anyhow::anyhow!("the map has no trail {id:?} any more"))?;
            let desc = index_desc(ctx, c);
            let stops: Vec<Stop> = sh
                .stops
                .iter()
                .map(|x| {
                    let mut st = match &x.target {
                        crate::map::StopRef::Decl(n) => stop_for_name(c, n, &desc),
                        crate::map::StopRef::Group(g) => Stop {
                            ref_kind: RefKind::Group,
                            name: g.clone(),
                            display: m.group_by_id(g).map(|k| m.groups[k].title.clone()).unwrap_or_else(|| g.clone()),
                            kind: "group".into(),
                            index_desc: Some(desc.clone()),
                            ..Default::default()
                        },
                    };
                    st.note = x.note.clone();
                    st.question = x.question.clone();
                    st
                })
                .collect();
            let (tid, new) = u.start_shared_trail(&ctx.user_key, sh, &stops)?;
            u.set_pref(&ctx.user_key, "active_trail", Some(&tid.to_string()))?;
            Ok((
                if new { format!("started “{}”: a personal copy of the shared trail ({} stops)", sh.title, stops.len()) } else { format!("“{}” was already started; showing your copy", sh.title) },
                Some((tid, None)),
            ))
        }),
        QueryReq::QuestionAdd { ref_kind, name, display, text } => read_op(&|u| {
            u.question_add(&ctx.user_key, ref_kind, &name, &display, &text)?;
            Ok((format!("question saved for {display}"), None))
        }),
        QueryReq::QuestionToggle { id } => read_op(&|u| {
            let r = u.question_toggle(&ctx.user_key, id)?;
            Ok((if r { "question marked resolved".into() } else { "question reopened".into() }, None))
        }),
        QueryReq::QuestionRemove { id } => read_op(&|u| {
            u.question_remove(&ctx.user_key, id)?;
            Ok(("question removed".into(), None))
        }),
        QueryReq::SetPref { key, value } => read_op(&|u| {
            u.set_pref(&ctx.user_key, &key, value.as_deref())?;
            Ok((String::new(), None))
        }),
        QueryReq::ExportTrail { trail, path, index_desc } => user_op(users, |u| {
            let t = u.trail(&ctx.user_key, trail)?.ok_or_else(|| anyhow::anyhow!("that trail does not exist any more"))?;
            let info = crate::userdata::ExportInfo {
                project: ctx.user_key.clone(),
                revision: ctx.source_root.as_deref().and_then(crate::paths::git_revision),
                index_desc,
                statuses: crate::userdata::stop_statuses(c, map.load.map().map(|m| m.as_ref()), &t),
            };
            crate::userdata::write_atomic(&path, crate::userdata::trail_export_markdown(&t, &info).as_bytes())?;
            let bad = info.statuses.values().filter(|x| matches!(x, StopStatus::Stale(_) | StopStatus::Unresolved(_))).count();
            Ok((
                format!("exported {} stops of “{}” to {}{}", t.stops.len(), t.title, path.display(), if bad > 0 { format!(" ({bad} stale or unresolved, marked in the file)") } else { String::new() }),
                None,
            ))
        }),
        QueryReq::Detail { gen, id } => QueryResp::Detail(gen, s(detail_bundle(ctx, c, users, cache, id)).map(Box::new)),
        QueryReq::Neighbors { gen, id, dir, filter, limit, offset, parent } => {
            QueryResp::Neighbors(gen, parent, offset, s(neighbors(c, id, dir, &filter, limit, offset)))
        }
        QueryReq::Mentions { gen, id } => QueryResp::Mentions(gen, s(crate::relations::lexical_mentions(c, ctx.source_root.as_deref(), cache, id))),
        QueryReq::Evidence { gen, id } => QueryResp::Evidence(gen, s(axiom_evidence(c, id))),
        QueryReq::ModuleList { gen, local_only } => {
            QueryResp::ModuleTree(gen, s(modules_with_stats(c, local_only)).map(|m| Box::new(build_outline_tree(m))))
        }
        QueryReq::ModuleDecls { gen, module_id, show_generated } => {
            QueryResp::ModuleDecls(gen, module_id, s(module_decls(c, module_id, show_generated, 20_000)))
        }
        QueryReq::ModuleImports { gen, module_id } => QueryResp::ModuleImports(
            gen,
            s((|| {
                let m = module(c, module_id)?;
                let (a, na, b, nb) = module_import_edges(c, module_id, 200)?;
                Ok((m, a, na, b, nb, module_stats(c, module_id)?))
            })()),
        ),
        QueryReq::FullSource { gen, rel_path, focus_line, highlight, sha256 } => {
            QueryResp::FullSource(gen, s(full_source(ctx, cache, rel_path, focus_line, highlight, sha256)))
        }
        QueryReq::Rows { gen, ids } => QueryResp::Rows(gen, s(rows_by_ids(c, &ids))),
        QueryReq::ResolveName { gen, name } => {
            let id = decl_by_name(c, &name).ok().flatten();
            QueryResp::Resolved(gen, name, id)
        }
        QueryReq::TransitiveCount { gen, id, dir } => QueryResp::TransitiveCount(gen, dir, s(transitive_count(c, id, dir, 200_000, 400))),
        QueryReq::Info { gen } => QueryResp::Info(gen, s(index_info(c))),
        QueryReq::LoadBookmarks { gen } => match users {
            Some(u) => match u.bookmarks(&ctx.user_key) {
                Ok(v) => QueryResp::User(gen, UserData::Bookmarks(v), String::new()),
                Err(e) => QueryResp::Error(format!("{e:#}")),
            },
            None => QueryResp::Error("user data unavailable".into()),
        },
        QueryReq::ToggleBookmark { mark } => user_op(users, |u| {
            let on = u.toggle_bookmark(&ctx.user_key, &mark)?;
            let names = u.bookmark_names(&ctx.user_key)?;
            Ok((
                format!("{} {}", if on { "bookmarked" } else { "removed bookmark" }, mark.display),
                Some(UserData::BookmarkNames(names)),
            ))
        }),
        QueryReq::SetBookmarkNote { name, note } => user_op(users, |u| {
            u.set_bookmark_note(&ctx.user_key, &name, note.as_deref())?;
            Ok(("note saved".into(), Some(UserData::Bookmarks(u.bookmarks(&ctx.user_key)?))))
        }),
        QueryReq::SaveSession { state } => user_op(users, |u| {
            u.save_session(&ctx.user_key, &state)?;
            Ok((String::new(), None))
        }),
        QueryReq::Reopen | QueryReq::ReloadMap | QueryReq::Shutdown => QueryResp::Message(String::new()),
    }
}

fn user_op(users: Option<&UserStore>, f: impl FnOnce(&UserStore) -> anyhow::Result<(String, Option<UserData>)>) -> QueryResp {
    let Some(u) = users else { return QueryResp::Error("user data unavailable".into()) };
    match f(u) {
        Ok((msg, Some(data))) => QueryResp::User(0, data, msg),
        Ok((msg, None)) => QueryResp::Message(msg),
        Err(e) => QueryResp::Error(format!("{e:#}")),
    }
}

fn detail_bundle(ctx: &IndexCtx, c: &rusqlite::Connection, users: Option<&UserStore>, cache: &mut SourceCache, id: i64) -> anyhow::Result<DetailBundle> {
    let d = detail(c, id)?.ok_or_else(|| anyhow::anyhow!("declaration {id} not found"))?;
    let bookmarked = users.map(|u| u.is_bookmarked(&ctx.user_key, &d.row.name).unwrap_or(false)).unwrap_or(false);
    let mut b = DetailBundle {
        d,
        excerpt: Vec::new(),
        excerpt_truncated: false,
        decl_lines: None,
        source_status: SourceStatus::NoPath,
        source_sha256: None,
        statement_src: None,
        statement_at: None,
        bookmarked,
    };
    let (Some(root), Some(rel)) = (ctx.source_root.as_ref(), b.d.module_path.clone()) else {
        b.source_status = if b.d.module_path.is_some() { SourceStatus::NoRoot } else { SourceStatus::NoPath };
        return Ok(b);
    };
    let f = match cache.get(root, &rel) {
        Ok(f) => f,
        Err(e) => {
            let msg = format!("{e:#}");
            b.source_status = if msg.contains("outside the approved") || msg.contains("rejected") {
                SourceStatus::Rejected(msg)
            } else {
                SourceStatus::Missing(msg)
            };
            return Ok(b);
        }
    };
    b.source_status = match &b.d.module_sha256 {
        Some(h) if *h != f.sha256 => SourceStatus::Changed,
        _ => SourceStatus::Ok,
    };
    b.source_sha256 = Some(f.sha256.clone());
    // Declaration line range.
    let range = match ctx.mode {
        IndexMode::Source => match (b.d.pos.start_byte, b.d.pos.end_byte) {
            (Some(sb), Some(eb)) if b.source_status == SourceStatus::Ok => {
                let l0 = f.line_of_byte(sb as usize);
                let l1 = f.line_of_byte((eb as usize).saturating_sub(1).max(sb as usize));
                if let (Some(kw), Some(he)) = (b.d.pos.kw_byte, b.d.pos.header_end_byte) {
                    let (kw, he) = (kw as usize, he as usize);
                    if kw <= he && he <= f.text.len() && f.text.is_char_boundary(kw) && f.text.is_char_boundary(he) {
                        b.statement_src = Some(f.text[kw..he].trim_end().to_string());
                        let line = f.line_of_byte(kw);
                        b.statement_at = Some((line, kw - f.line_starts.get(line.saturating_sub(1)).copied().unwrap_or(0)));
                    }
                }
                Some((l0, l1))
            }
            _ => b.d.pos.start_line.map(|l| (l as usize, b.d.pos.end_line.unwrap_or(l) as usize)),
        },
        IndexMode::Semantic => b.d.pos.start_line.map(|l| (l as usize, b.d.pos.end_line.unwrap_or(l) as usize)),
    };
    if let Some((l0, l1)) = range {
        b.decl_lines = Some((l0, l1));
        let from = l0.saturating_sub(2).max(1);
        let to = (l1 + 1).min(f.line_count());
        let (ex, trunc) = excerpt(&f, from, to, 400);
        b.excerpt = ex;
        b.excerpt_truncated = trunc;
    }
    Ok(b)
}

fn full_source(ctx: &IndexCtx, cache: &mut SourceCache, rel: String, focus: usize, highlight: Option<(usize, usize)>, sha: Option<String>) -> anyhow::Result<FullSource> {
    let root = ctx.source_root.as_ref().ok_or_else(|| anyhow::anyhow!("no approved source root for this index"))?;
    let f = cache.get(root, &rel)?;
    let lines = (1..=f.line_count()).map(|n| f.line(n).to_string()).collect();
    Ok(FullSource { rel_path: rel, sha256: f.sha256.clone(), lines, focus_line: focus, highlight, changed: sha.is_some_and(|h| h != f.sha256) })
}

/// Runs one bounded connection search on its own thread, so the query
/// worker stays free. `latest` is the cancellation token: the search stops
/// as soon as it no longer holds `gen`.
pub fn spawn_connection(ctx: IndexCtx, tx: Sender<Event>, gen: u64, latest: Arc<AtomicU64>, q: ConnQuery, lim: Limits) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("slim-connection".into())
        .spawn(move || {
            let cancel = || latest.load(Ordering::Relaxed) != gen;
            let t = Instant::now();
            let res = match db::open_read(&ctx.index) {
                Ok(conn) => {
                    let map = load_map(&ctx, None).load;
                    let mut cache = SourceCache::default();
                    let mut env = Env { conn: &conn, mode: ctx.mode, map: map.map().map(|m| m.as_ref()), source_root: ctx.source_root.as_deref(), cache: &mut cache };
                    crate::relations::connection(&mut env, &q, &lim, &cancel)
                }
                Err(e) => ConnResult { query: q, outcome: crate::relations::ConnOutcome::Error(format!("{e:#}")), explored: 0, ms: 0 },
            };
            super::timing::record("connection search", t.elapsed().as_secs_f64() * 1000.0);
            let _ = tx.send(Event::Query(QueryResp::Connection(gen, Box::new(res))));
        })
        .expect("spawn connection search")
}

// ---------------------------------------------------------------------------
// Search worker
// ---------------------------------------------------------------------------

pub enum SearchReq {
    /// `scope` restricts the search; module names are resolved here.
    Query { gen: u64, params: SearchParams, docs: bool, scope: Option<ScopeSpec> },
    Reload,
    Shutdown,
}

#[derive(Clone, Debug)]
pub struct SearchResults {
    pub gen: u64,
    pub query: String,
    pub outcome: SearchOutcome,
    pub rows: Vec<DeclRow>,
    pub doc_rows: Vec<DeclRow>,
    pub doc_ms: f64,
    /// A file scope names no local file (the reader is told).
    pub scope_empty: bool,
}

/// Ids of the modules a file scope covers.
fn module_set(conn: &rusqlite::Connection, prefix: &str) -> anyhow::Result<ModuleSet> {
    let mut st = conn.prepare_cached("SELECT id FROM modules WHERE name = ?1 OR (name >= ?1 || '.' AND name < ?1 || '/')")?;
    let ids: Vec<i64> = st.query_map([prefix], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(ModuleSet::from_ids(ids.into_iter().filter(|&i| i > 0 && i <= u32::MAX as i64).map(|i| i as u32)))
}

pub enum SearchResp {
    Ready { entries: usize, ms: u64, rebuilt: bool, memory: usize },
    Failed(String),
    Results(Box<SearchResults>),
}

pub fn spawn_search_worker(ctx: IndexCtx, tx: Sender<Event>, latest: Arc<AtomicU64>) -> (Sender<SearchReq>, std::thread::JoinHandle<()>) {
    let (stx, srx) = std::sync::mpsc::channel::<SearchReq>();
    let h = std::thread::Builder::new()
        .name("slim-search".into())
        .spawn(move || search_loop(ctx, srx, tx, latest))
        .expect("spawn search worker");
    (stx, h)
}

fn load_index(ctx: &IndexCtx) -> anyhow::Result<(NameIndex, rusqlite::Connection, bool)> {
    let conn = db::open_read(&ctx.index)?;
    let gen = db::get_meta(&conn, "name_index_generation")?.unwrap_or_default();
    let path = db::import::names_path(&ctx.index);
    match NameIndex::load(&path, &gen) {
        Ok(ni) => Ok((ni, conn, false)),
        Err(_) => {
            let ni = db::import::build_name_index(&conn, &gen)?;
            let _ = ni.save(&path);
            Ok((ni, conn, true))
        }
    }
}

fn search_loop(ctx: IndexCtx, rx: Receiver<SearchReq>, tx: Sender<Event>, latest: Arc<AtomicU64>) {
    let mut state: Option<(NameIndex, rusqlite::Connection)> = None;
    let mut reload = true;
    // The module set of the scope in use (resolved once per scope).
    let mut scope_cache: Option<(String, Arc<ModuleSet>)> = None;
    loop {
        if reload {
            reload = false;
            scope_cache = None;
            let t = Instant::now();
            match load_index(&ctx) {
                Ok((ni, conn, rebuilt)) => {
                    let _ = tx.send(Event::Search(SearchResp::Ready {
                        entries: ni.len(),
                        ms: t.elapsed().as_millis() as u64,
                        rebuilt,
                        memory: ni.memory_bytes(),
                    }));
                    state = Some((ni, conn));
                }
                Err(e) => {
                    let _ = tx.send(Event::Search(SearchResp::Failed(format!("{e:#}"))));
                    state = None;
                }
            }
        }
        let Ok(mut req) = rx.recv() else { return };
        // Only the newest query matters.
        while let Ok(next) = rx.try_recv() {
            match (&req, &next) {
                (SearchReq::Shutdown, _) => break,
                _ => req = next,
            }
        }
        match req {
            SearchReq::Shutdown => return,
            SearchReq::Reload => reload = true,
            SearchReq::Query { gen, mut params, docs, scope } => {
                let Some((ni, conn)) = &state else { continue };
                if latest.load(Ordering::Relaxed) != gen {
                    continue;
                }
                let mut scope_empty = false;
                params.scope = match &scope {
                    None => None,
                    Some(sp) if sp.kind == ScopeKind::Namespace => Some(Scope::Namespace(sp.prefix.clone())),
                    Some(sp) => {
                        let set = match &scope_cache {
                            Some((p, set)) if *p == sp.prefix => set.clone(),
                            _ => {
                                let set = Arc::new(module_set(conn, &sp.prefix).unwrap_or_default());
                                scope_cache = Some((sp.prefix.clone(), set.clone()));
                                set
                            }
                        };
                        scope_empty = set.is_empty();
                        Some(Scope::Modules(set))
                    }
                };
                let cancel = || latest.load(Ordering::Relaxed) != gen;
                let outcome = ni.search(&params, &cancel);
                if outcome.cancelled {
                    continue;
                }
                let ids: Vec<i64> = outcome.hits.iter().map(|h| h.id as i64).collect();
                let rows = rows_by_ids(conn, &ids).unwrap_or_default();
                let t = Instant::now();
                let doc_rows = if docs && params.query.trim().chars().count() >= 3 {
                    // A scope is applied after ranking, so more candidates are read.
                    let ids = text_search(conn, &params.query, if scope.is_some() { 600 } else { 60 }).unwrap_or_default();
                    let hit_ids: HashSet<i64> = rows.iter().map(|r| r.id).collect();
                    let ids: Vec<i64> = ids.into_iter().filter(|i| !hit_ids.contains(i)).collect();
                    rows_by_ids(conn, &ids)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|r| (params.show_generated || r.flags & flags::GENERATED == 0)
                            && (!params.local_only || r.origin.is_local())
                            && params.kinds.accepts(r.kind)
                            && scope.as_ref().is_none_or(|sp| sp.accepts(&r.display, r.module.as_deref())))
                        .take(60)
                        .collect()
                } else {
                    Vec::new()
                };
                let doc_ms = t.elapsed().as_secs_f64() * 1000.0;
                if latest.load(Ordering::Relaxed) != gen {
                    continue;
                }
                let res = SearchResults { gen, query: params.query.clone(), outcome, rows, doc_rows, doc_ms, scope_empty };
                if tx.send(Event::Search(SearchResp::Results(Box::new(res)))).is_err() {
                    return;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Job runner
// ---------------------------------------------------------------------------

pub enum JobReq {
    Freshness { force_hash: bool, discover_new: bool },
    Rescan { fr: Freshness },
    BuildSource,
    /// Compute (but do not run) a semantic-export plan for the project.
    ExportPlan,
    /// Run the previously computed plan (after explicit confirmation).
    ExportRun,
    Shutdown,
}

#[derive(Debug)]
pub enum JobEvent {
    Progress { label: String, done: u64, total: u64 },
    Freshness(Freshness),
    IndexReplaced(String),
    ExportPlanned(Result<String, String>),
    /// Semantic index built and imported at this path.
    ExportDone(PathBuf, String),
    Failed(String),
    Cancelled,
}

pub struct JobCtx {
    pub project: Option<Project>,
    pub ctx: IndexCtx,
    pub threads: Option<usize>,
}

pub fn spawn_job_runner(jc: JobCtx, tx: Sender<Event>, cancel: Arc<AtomicBool>) -> (Sender<JobReq>, std::thread::JoinHandle<()>) {
    let (jtx, jrx) = std::sync::mpsc::channel::<JobReq>();
    let h = std::thread::Builder::new()
        .name("slim-jobs".into())
        .spawn(move || job_loop(jc, jrx, tx, cancel))
        .expect("spawn job runner");
    (jtx, h)
}

fn job_loop(jc: JobCtx, rx: Receiver<JobReq>, tx: Sender<Event>, cancel: Arc<AtomicBool>) {
    let send = |e: JobEvent| {
        let _ = tx.send(Event::Job(e));
    };
    let mut plan: Option<crate::export::ExportPlan> = None;
    while let Ok(req) = rx.recv() {
        if matches!(req, JobReq::Shutdown) {
            return;
        }
        cancel.store(false, Ordering::Relaxed);
        let last = std::sync::Mutex::new(Instant::now() - Duration::from_secs(1));
        let progress = |label: &str, done: u64, total: u64| {
            let mut l = last.lock().unwrap();
            if l.elapsed() >= Duration::from_millis(150) {
                *l = Instant::now();
                let _ = tx.send(Event::Job(JobEvent::Progress { label: label.to_string(), done, total }));
            }
        };
        let result: anyhow::Result<()> = (|| {
            match req {
                JobReq::Freshness { force_hash, discover_new } => {
                    let Some(root) = jc.ctx.source_root.as_ref() else { anyhow::bail!("no source root") };
                    progress("checking freshness", 0, 1);
                    let conn = db::open_read(&jc.ctx.index)?;
                    let fr = sindex::check_freshness(&conn, root, discover_new, force_hash, &cancel)?;
                    send(JobEvent::Freshness(fr));
                }
                JobReq::Rescan { fr } => {
                    let Some(p) = jc.project.as_ref() else { anyhow::bail!("no project") };
                    let mut cfg = ScanConfig::for_project(p);
                    if let Some(t) = jc.threads {
                        cfg.threads = t;
                    }
                    progress("rescanning changed files", 0, 1);
                    let s = sindex::apply_incremental(&jc.ctx.index, p, &fr, &cfg, &cancel)?;
                    send(JobEvent::IndexReplaced(format!("rescanned {} files ({} candidates) in {:.1}s", s.files, s.decls, s.elapsed.as_secs_f64())));
                }
                JobReq::BuildSource => {
                    let Some(p) = jc.project.as_ref() else { anyhow::bail!("no project") };
                    let mut cfg = ScanConfig::for_project(p);
                    if let Some(t) = jc.threads {
                        cfg.threads = t;
                    }
                    let s = sindex::build_source_index(p, &jc.ctx.index, &cfg, &cancel, Some(&progress))?;
                    send(JobEvent::IndexReplaced(format!(
                        "indexed {} files, {} declaration candidates in {:.1}s",
                        s.files,
                        s.decls,
                        s.elapsed.as_secs_f64()
                    )));
                }
                JobReq::ExportPlan => {
                    let Some(p) = jc.project.as_ref() else { anyhow::bail!("semantic export needs a project directory") };
                    match crate::export::plan(&p.root, &crate::export::PlanOptions::default()) {
                        Ok(pl) => {
                            send(JobEvent::ExportPlanned(Ok(pl.describe())));
                            plan = Some(pl);
                        }
                        Err(e) => send(JobEvent::ExportPlanned(Err(format!("{e:#}")))),
                    }
                }
                JobReq::ExportRun => {
                    let Some(pl) = plan.take() else { anyhow::bail!("no confirmed export plan") };
                    let log = |m: &str| progress(m, 0, 0);
                    crate::export::build_exporter(&pl, &cancel, &log)?;
                    let out = pl.project.cache_dir().join("snapshot.jsonl");
                    let res = crate::export::run_export(&pl, &out, &cancel, &log)?;
                    progress("importing snapshot", 0, 0);
                    let idx = pl.project.semantic_index();
                    let opts = crate::db::import::ImportOptions {
                        source_root: Some(pl.project.root.clone()),
                        cancel: Some(&cancel),
                        ..Default::default()
                    };
                    let s = crate::db::import::import_snapshot(&res.snapshot, &idx, &opts)?;
                    send(JobEvent::ExportDone(
                        idx,
                        format!("semantic index ready: {} local declarations, {} edges, {} axioms", s.local, s.edges, s.axioms),
                    ));
                }
                JobReq::Shutdown => {}
            }
            Ok(())
        })();
        if let Err(e) = result {
            if cancel.load(Ordering::Relaxed) {
                send(JobEvent::Cancelled);
            } else {
                send(JobEvent::Failed(format!("{e:#}")));
            }
        }
    }
}
