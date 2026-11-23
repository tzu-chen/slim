//! Reading views built on the reference graph: a start page, the contents
//! of a project by dependence, the ledger of named statements, and why a
//! declaration is there. Also what the declaration view borrows from the
//! graph: where the open declaration stands, and its statement unfolded.
//!
//! The graph is read once, off the query worker's thread, and what an
//! endpoint reaches is computed for the roots in force: the pinned goal,
//! else the objectives of the project map, else the theorems of the files
//! that nothing imports. Every screen says which data it shows.

use super::state::{App, Cmd, ListState};
use super::views::{Subject, View};
use super::worker::{IndexCtx, MapBundle, QueryReq};
use crate::contract::Contract;
use crate::db::query::DeclRow;
use crate::graph::{nf, Cone, Follow, Graph, BODY, NODE, STMT};
use crate::model::DeclKind;
use crate::reading::{pick_roots, relay_chain, rows, thousands, Roots};
use crate::sourceview::SourceCache;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

/// Rows sent per request.
pub const PAGE: usize = 200;

// ---------------------------------------------------------------------------
// Worker side.

/// The graph of the open index and the last analyses made of it.
pub struct GraphSlot {
    loading: Option<Receiver<Result<Option<Graph>, String>>>,
    graph: Option<Arc<Graph>>,
    /// Why there is none.
    pub why_not: Option<String>,
    cones: Vec<(Vec<u32>, Arc<Cone>)>,
}

impl GraphSlot {
    /// Starts reading the graph beside `index` on a thread of its own.
    pub fn new(index: &Path) -> GraphSlot {
        let (tx, rx) = std::sync::mpsc::channel();
        let index = index.to_path_buf();
        let _ = std::thread::Builder::new().name("slim-graph".into()).spawn(move || {
            let r = (|| -> anyhow::Result<Option<Graph>> {
                if !crate::graph::graph_path(&index).exists() {
                    return Ok(None);
                }
                let conn = crate::db::open_read(&index)?;
                let gen = crate::db::get_meta(&conn, "name_index_generation")?.unwrap_or_default();
                Graph::load(&index, &gen)
            })();
            let _ = tx.send(r.map_err(|e| format!("{e:#}")));
        });
        GraphSlot { loading: Some(rx), graph: None, why_not: None, cones: Vec::new() }
    }

    pub fn graph(&mut self) -> Option<Arc<Graph>> {
        if let Some(rx) = self.loading.take() {
            match rx.recv() {
                Ok(Ok(Some(g))) => self.graph = Some(Arc::new(g)),
                Ok(Ok(None)) => self.why_not = Some("no reference graph for this index as it is now: `slim graph build <PROJECT_DIR>` makes one (it reads files and build artifacts, and runs nothing)".into()),
                Ok(Err(e)) => self.why_not = Some(format!("the reference graph could not be read: {e}")),
                Err(_) => self.why_not = Some("the reference graph could not be read".into()),
            }
        }
        self.graph.clone()
    }

    /// The graph, the roots in force and what they reach.
    pub fn cone(&mut self, c: &Connection, map: &MapBundle, goal: Option<&str>) -> anyhow::Result<Option<(Arc<Graph>, Arc<Cone>, Roots)>> {
        let Some(g) = self.graph() else { return Ok(None) };
        let explicit: Vec<String> = goal.map(|s| s.to_string()).into_iter().collect();
        let roots = match pick_roots(c, &g, map.load.map().map(|m| m.as_ref()), &explicit) {
            Ok(r) => r,
            // A goal the graph does not hold: the project's own endpoints.
            Err(_) => pick_roots(c, &g, map.load.map().map(|m| m.as_ref()), &[])?,
        };
        if roots.nodes.is_empty() {
            return Ok(Some((g.clone(), Arc::new(g.cone(&[], Follow::All)), roots)));
        }
        let mut key = roots.nodes.clone();
        key.sort_unstable();
        if let Some(pos) = self.cones.iter().position(|x| x.0 == key) {
            let e = self.cones.remove(pos);
            self.cones.insert(0, e);
        } else {
            self.cones.insert(0, (key, Arc::new(g.cone(&roots.nodes, Follow::All))));
            self.cones.truncate(3);
        }
        Ok(Some((g, self.cones[0].1.clone(), roots)))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum GuideReq {
    Start,
    /// The children of a node of the contents (`None`: the top).
    Children { parent: Option<i64>, offset: usize },
    Ledger { namespace: Option<String>, offset: usize },
    Why { id: i64 },
}

#[derive(Clone, Debug, Default)]
pub struct StartData {
    /// Who chose the endpoints.
    pub how: String,
    /// The endpoints with their statements and what is reached only
    /// through each.
    pub endpoints: Vec<(DeclRow, String, u32)>,
    pub graph: Option<String>,
    pub reached: Option<(usize, usize)>,
    pub files: u64,
    pub decls: u64,
    pub theorems: u64,
    pub documented: u64,
    /// Named statements: all, with a theorem of that type, reached.
    pub named: Option<(usize, usize, usize)>,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ContentRow {
    pub row: DeclRow,
    /// Declarations reached only through it, itself included.
    pub excl: u32,
    pub kids: u32,
    /// The names a relay hands over to, in order.
    pub relay: Vec<String>,
    /// The declaration whose children continue the tree (the end of a
    /// relay chain).
    pub below: i64,
}

#[derive(Clone, Debug)]
pub struct ChildrenData {
    pub parent: Option<i64>,
    pub offset: usize,
    pub rows: Vec<ContentRow>,
    pub more: usize,
    pub rest_size: u64,
    pub label: String,
    pub how: String,
    pub reached: usize,
    pub total: usize,
}

#[derive(Clone, Debug)]
pub struct LedgerItem {
    pub statement: DeclRow,
    pub prover: Option<DeclRow>,
    pub provers: u32,
    pub users: u32,
    pub depth: Option<u16>,
}

#[derive(Clone, Debug)]
pub struct LedgerData {
    pub namespace: Option<String>,
    pub offset: usize,
    pub rows: Vec<LedgerItem>,
    pub total: usize,
    pub proved: usize,
    pub reached: usize,
    pub label: String,
}

#[derive(Clone, Debug)]
pub struct WhyData {
    pub id: i64,
    pub name: String,
    pub reached: bool,
    /// From an endpoint down to the declaration, with how each is used by
    /// the one before.
    pub chain: Vec<(DeclRow, &'static str)>,
    pub dominators: Vec<DeclRow>,
    pub excl: u32,
    pub users: u32,
    pub label: String,
    pub how: String,
}

#[derive(Clone, Debug)]
pub enum GuideData {
    Start(Box<StartData>),
    Children(ChildrenData),
    Ledger(LedgerData),
    Why(WhyData),
}

/// Where a declaration stands under the endpoints, for the line at the top
/// of the declaration view.
#[derive(Clone, Debug, PartialEq)]
pub struct Position {
    pub id: i64,
    pub reached: bool,
    pub depth: u16,
    pub excl: u32,
    pub users: u32,
    /// Display names from an endpoint down to the declaration.
    pub chain: Vec<String>,
    pub tag: &'static str,
    pub has_roots: bool,
}

pub struct GuideEnv<'a> {
    pub ctx: &'a IndexCtx,
    pub c: &'a Connection,
    pub cache: &'a mut SourceCache,
    pub map: &'a MapBundle,
    pub slot: &'a mut GraphSlot,
}

fn statement_of(env: &mut GuideEnv, id: i64) -> String {
    let Some(root) = env.ctx.source_root.as_deref() else { return String::new() };
    let Ok(Some(d)) = crate::db::query::detail(env.c, id) else { return String::new() };
    if let Some(t) = &d.type_text {
        return t.clone();
    }
    let Some(rel) = d.module_path.clone() else { return String::new() };
    let Ok(f) = env.cache.get(root, &rel) else { return String::new() };
    let (Some(kw), Some(he)) = (d.pos.kw_byte.or(d.pos.start_byte), d.pos.header_end_byte.or(d.pos.end_byte)) else { return String::new() };
    let text = f.text.get(kw as usize..(he as usize).min(f.text.len())).unwrap_or("").trim_end();
    crate::skim::cut_statement(text).0
}

fn how_used(g: &Graph, from: u32, to: u32) -> &'static str {
    let e = g.uses(from).iter().find(|e| *e & NODE == to).copied().unwrap_or(0);
    match (e & STMT != 0, e & BODY != 0) {
        (true, true) => "stated and used",
        (true, false) => "in its statement",
        _ => "in its proof",
    }
}

pub fn handle(env: &mut GuideEnv, goal: Option<&str>, req: &GuideReq) -> anyhow::Result<GuideData> {
    let found = env.slot.cone(env.c, env.map, goal)?;
    match req {
        GuideReq::Start => {
            let mut s = StartData::default();
            let (files, decls, thms, docs): (i64, i64, i64, i64) =
                env.c.query_row("SELECT count(*), coalesce(sum(decl_count),0), coalesce(sum(n_thm),0), coalesce(sum(n_doc),0) FROM modules WHERE is_local = 1", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
            (s.files, s.decls, s.theorems, s.documented) = (files as u64, decls as u64, thms as u64, docs as u64);
            match found {
                Some((g, cone, roots)) => {
                    s.graph = Some(g.stats.describe());
                    s.how = roots.how.clone();
                    let names = rows(env.c, &g, &roots.nodes)?;
                    for v in &roots.nodes {
                        if let Some(r) = names.get(v) {
                            let sig = statement_of(env, r.id);
                            s.endpoints.push((r.clone(), sig, cone.excl[*v as usize]));
                        }
                    }
                    if !roots.nodes.is_empty() {
                        s.reached = Some((cone.reached, g.len()));
                    } else {
                        s.notes.push(crate::reading::no_endpoints_hint(env.c));
                    }
                    let (mut all, mut proved, mut reached) = (0, 0, 0);
                    for v in 0..g.len() as u32 {
                        if g.has(v, nf::PROP) {
                            all += 1;
                            proved += (!g.provers(v).is_empty()) as usize;
                            reached += cone.contains(v) as usize;
                        }
                    }
                    s.named = Some((all, proved, reached));
                }
                None => {
                    // Without a graph: the import order still says where the project ends.
                    match crate::graph::import_endpoints(env.c, crate::reading::MAX_AUTO_ROOTS)? {
                        Some(ids) => {
                            s.how = "the theorems of the files that nothing imports".into();
                            for r in crate::db::query::rows_by_ids(env.c, &ids)? {
                                let sig = statement_of(env, r.id);
                                s.endpoints.push((r, sig, 0));
                            }
                        }
                        None => s.notes.push(crate::reading::no_endpoints_hint(env.c)),
                    }
                    s.notes.push(env.slot.why_not.clone().unwrap_or_default());
                }
            }
            Ok(GuideData::Start(Box::new(s)))
        }
        GuideReq::Children { parent, offset } => {
            let Some((g, cone, roots)) = found else { anyhow::bail!("{}", env.slot.why_not.clone().unwrap_or_default()) };
            if roots.nodes.is_empty() {
                anyhow::bail!("no endpoints to start from: pin a goal (*) on a main theorem, or list objectives in the project map");
            }
            let kids: &[u32] = match parent {
                None => cone.top(),
                Some(id) => {
                    let v = g.node(*id).ok_or_else(|| anyhow::anyhow!("not in the reference graph"))?;
                    cone.children(v)
                }
            };
            let page: Vec<u32> = kids.iter().skip(*offset).take(PAGE).copied().collect();
            let mut want = page.clone();
            let chains: Vec<Vec<u32>> = page.iter().map(|&k| relay_chain(&g, k)).collect();
            want.extend(chains.iter().flat_map(|c| c.iter().skip(1).copied()));
            let names = rows(env.c, &g, &want)?;
            let mut out = Vec::new();
            for (k, chain) in page.iter().zip(&chains) {
                let Some(r) = names.get(k) else { continue };
                // The tree continues below the last relay that dominates
                // nothing but the next one.
                let mut at = *k;
                while let [only] = cone.children(at) {
                    if g.relay_target(at) == Some(*only) {
                        at = *only;
                    } else {
                        break;
                    }
                }
                out.push(ContentRow {
                    row: r.clone(),
                    excl: cone.excl[*k as usize],
                    kids: cone.children(at).len() as u32,
                    relay: chain.iter().skip(1).filter_map(|w| names.get(w)).map(|x| super::state::short_name(&x.display)).collect(),
                    below: g.id(at),
                });
            }
            let shown = offset + page.len();
            let rest_size: u64 = kids.iter().skip(shown).map(|&k| cone.excl[k as usize] as u64).sum();
            Ok(GuideData::Children(ChildrenData { parent: *parent, offset: *offset, rows: out, more: kids.len().saturating_sub(shown), rest_size, label: g.stats.label().to_string(), how: roots.how.clone(), reached: cone.reached, total: g.len() }))
        }
        GuideReq::Ledger { namespace, offset } => {
            let Some((g, cone, roots)) = found else { anyhow::bail!("{}", env.slot.why_not.clone().unwrap_or_default()) };
            let only: Option<HashSet<u32>> = match namespace {
                Some(ns) => {
                    let mut st = env.c.prepare_cached("SELECT id FROM decls WHERE name >= ?1 || '.' AND name < ?1 || '/'")?;
                    let ids: Vec<i64> = st.query_map([ns.trim_end_matches('.')], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
                    Some(ids.into_iter().filter_map(|id| g.node(id)).collect())
                }
                None => None,
            };
            let f = |v: u32| only.as_ref().is_none_or(|s| s.contains(&v));
            let all = crate::reading::ledger(&g, (!roots.nodes.is_empty()).then_some(&*cone), Some(&f));
            let (total, proved, reached) = (all.len(), all.iter().filter(|r| !r.provers.is_empty()).count(), all.iter().filter(|r| r.depth.is_some()).count());
            let page: Vec<&crate::reading::LedgerRow> = all.iter().skip(*offset).take(PAGE).collect();
            let mut want: Vec<u32> = page.iter().map(|r| r.statement).collect();
            want.extend(page.iter().filter_map(|r| r.provers.first().copied()));
            let names = rows(env.c, &g, &want)?;
            let out = page
                .iter()
                .filter_map(|r| {
                    Some(LedgerItem { statement: names.get(&r.statement)?.clone(), prover: r.provers.first().and_then(|p| names.get(p)).cloned(), provers: r.provers.len() as u32, users: r.users, depth: r.depth })
                })
                .collect();
            Ok(GuideData::Ledger(LedgerData { namespace: namespace.clone(), offset: *offset, rows: out, total, proved, reached, label: g.stats.label().to_string() }))
        }
        GuideReq::Why { id } => {
            let Some((g, cone, roots)) = found else { anyhow::bail!("{}", env.slot.why_not.clone().unwrap_or_default()) };
            let v = g.node(*id).ok_or_else(|| anyhow::anyhow!("the declaration is not in the reference graph"))?;
            let name = crate::db::query::row(env.c, *id)?.map(|r| r.display).unwrap_or_default();
            let chain = cone.chain(v);
            let names = rows(env.c, &g, &chain)?;
            let doms = cone.dominators(v);
            let dn = rows(env.c, &g, &doms)?;
            Ok(GuideData::Why(WhyData {
                id: *id,
                name,
                reached: !chain.is_empty(),
                chain: chain.iter().enumerate().filter_map(|(k, w)| Some((names.get(w)?.clone(), if k == 0 { "endpoint" } else { how_used(&g, chain[k - 1], *w) }))).collect(),
                dominators: doms.iter().filter(|w| **w != v).filter_map(|w| dn.get(w).cloned()).collect(),
                excl: if cone.contains(v) { cone.excl[v as usize] } else { 0 },
                users: g.users(v).len() as u32,
                label: g.stats.label().to_string(),
                how: roots.how.clone(),
            }))
        }
    }
}

/// Where declaration `id` stands; `None` without a graph.
pub fn position(env: &mut GuideEnv, goal: Option<&str>, id: i64) -> Option<Position> {
    let (g, cone, roots) = env.slot.cone(env.c, env.map, goal).ok()??;
    let v = g.node(id)?;
    let chain = cone.chain(v);
    let names = rows(env.c, &g, &chain).ok()?;
    Some(Position {
        id,
        reached: !chain.is_empty(),
        depth: if cone.contains(v) { cone.depth[v as usize] } else { 0 },
        excl: if cone.contains(v) { cone.excl[v as usize] } else { 0 },
        users: g.users(v).len() as u32,
        chain: chain.iter().filter_map(|w| names.get(w)).map(|r| super::state::short_name(&r.display)).collect(),
        tag: g.stats.tag(),
        has_roots: !roots.nodes.is_empty(),
    })
}

/// "Used by" from the graph, when its edges are Lean's: the form the
/// tab already shows.
pub fn used_by(env: &mut GuideEnv, id: i64, limit: usize) -> Option<crate::leanrefs::UsedBy> {
    let g = env.slot.graph()?;
    if g.stats.source == "lexical" {
        return None;
    }
    let v = g.node(id)?;
    let ids: Vec<i64> = g.users(v).iter().take(limit).map(|e| g.id(e & NODE)).collect();
    let mut st = env.c.prepare_cached("SELECT coalesce(sel_line, start_line, 1) FROM decls WHERE id = ?1").ok()?;
    let mut users: Vec<crate::leanrefs::User> = Vec::new();
    for row in crate::db::query::rows_by_ids(env.c, &ids).ok()? {
        let line: u32 = st.query_row([row.id], |r| r.get(0)).unwrap_or(1);
        users.push(crate::leanrefs::User { row, uses: 1, line });
    }
    users.sort_by(|a, b| a.row.module.cmp(&b.row.module).then(a.line.cmp(&b.line)));
    let read = (g.stats.files_hashed + g.stats.files_consistent) as usize;
    Some(crate::leanrefs::UsedBy {
        users,
        loose: 0,
        more: g.users(v).len().saturating_sub(limit),
        local_files: g.stats.files as usize,
        read_files: read.max(1),
        unhashed_files: g.stats.files_consistent as usize,
        unavailable: None,
    })
}

// ---------------------------------------------------------------------------
// The view.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GuideMode {
    #[default]
    Start,
    Contents,
    Ledger,
    Why,
}

impl GuideMode {
    pub fn title(self) -> &'static str {
        match self {
            GuideMode::Start => " Start here ",
            GuideMode::Contents => " Contents by dependence ",
            GuideMode::Ledger => " Named statements ",
            GuideMode::Why => " Why is this here ",
        }
    }
}

#[derive(Clone, Debug)]
pub enum GuideKind {
    Section { title: String, right: String },
    Text { text: String, warn: bool },
    /// A way on from the start page.
    Door { title: String, hint: String, cmd: Cmd },
    Decl { name: String, kind: DeclKind, label: String, left: String, right: String, expandable: bool, expanded: bool, below: Option<i64>, note: Option<String>, dim: bool },
    More { parent: Option<i64>, offset: usize, text: String },
}

#[derive(Clone, Debug)]
pub struct GuideRow {
    pub key: String,
    pub depth: u16,
    pub kind: GuideKind,
}

#[derive(Default)]
pub struct GuideView {
    pub mode: GuideMode,
    pub rows: Vec<GuideRow>,
    pub list: ListState,
    pub start: Option<Box<StartData>>,
    /// Children of the contents tree by parent, with what is not loaded yet.
    pub kids: HashMap<Option<i64>, (Vec<ContentRow>, usize, u64)>,
    pub expanded: HashSet<i64>,
    pub asked: HashSet<Option<i64>>,
    pub contents_head: String,
    pub ledger: Option<LedgerData>,
    pub ledger_ns: Option<String>,
    pub why: Option<WhyData>,
    pub error: Option<String>,
    /// The goal the data on screen was computed for.
    pub for_goal: Option<String>,
    pub gen: u64,
    /// Where the open declaration stands (the declaration view).
    pub position: Option<Position>,
    /// The open declaration's statement unfolded, and to what depth.
    pub contract: Option<(i64, Box<Contract>)>,
    pub contract_depth: u8,
    pub contract_error: Option<String>,
}

impl App {
    fn guide_goal(&self) -> Option<String> {
        self.v.goal.goal.as_ref().map(|g| g.name.clone())
    }

    pub(super) fn guide_ask(&mut self, req: GuideReq) {
        self.v.guide.gen += 1;
        let (gen, goal) = (self.v.guide.gen, self.guide_goal());
        if let Some(t) = &self.ch.query {
            let _ = t.send(QueryReq::Guide { gen, goal, req });
        }
    }

    /// Shows one of the reading views; data that was computed for another
    /// goal is asked for again.
    pub fn cmd_guide(&mut self, mode: GuideMode) {
        if self.ctx.index.is_none() {
            return;
        }
        let goal = self.guide_goal();
        if self.v.guide.for_goal != goal {
            let g = &mut self.v.guide;
            g.start = None;
            g.kids.clear();
            g.asked.clear();
            g.expanded.clear();
            g.ledger = None;
            g.for_goal = goal;
        }
        // Why: of the declaration at hand (the row selected in a view,
        // else the one that is open).
        let why_id = if mode != GuideMode::Why {
            None
        } else if self.v.view != View::Browse {
            self.subject_bundle().map(|b| b.d.row.id).or(self.current)
        } else {
            self.current
        };
        self.v.guide.mode = mode;
        self.v.guide.error = None;
        self.v.guide.list = ListState::default();
        self.set_view(View::Guide);
        match (mode, why_id) {
            (GuideMode::Why, Some(id)) => {
                self.v.guide.why = None;
                self.guide_ask(GuideReq::Why { id });
            }
            (GuideMode::Why, None) => self.v.guide.error = Some("open a declaration first: this asks why it is in the project".into()),
            _ => self.guide_ensure(),
        }
        self.rebuild_guide_rows();
        self.sync_view_detail();
    }

    /// Asks for what the page on screen shows, when it is not there yet.
    pub fn guide_ensure(&mut self) {
        match self.v.guide.mode {
            GuideMode::Start => {
                if self.v.guide.start.is_none() {
                    self.guide_ask(GuideReq::Start);
                }
            }
            GuideMode::Contents => {
                if !self.v.guide.kids.contains_key(&None) && self.v.guide.asked.insert(None) {
                    self.guide_ask(GuideReq::Children { parent: None, offset: 0 });
                }
            }
            GuideMode::Ledger => {
                if self.v.guide.ledger.is_none() {
                    let ns = self.v.guide.ledger_ns.clone();
                    self.guide_ask(GuideReq::Ledger { namespace: ns, offset: 0 });
                }
            }
            GuideMode::Why => {}
        }
    }

    pub fn on_guide(&mut self, gen: u64, req: GuideReq, res: Result<GuideData, String>) -> bool {
        // Children arrive per parent and stay valid; the rest belongs to
        // the newest request.
        let g = &mut self.v.guide;
        match res {
            Err(e) => {
                if let GuideReq::Children { parent, .. } = &req {
                    g.asked.remove(parent);
                }
                if gen == g.gen || matches!(req, GuideReq::Children { .. }) {
                    g.error = Some(e);
                }
            }
            Ok(GuideData::Start(s)) => g.start = Some(s),
            Ok(GuideData::Children(c)) => {
                g.contents_head = format!("{} of {} reached from {}", thousands(c.reached as u64), thousands(c.total as u64), c.how);
                let e = g.kids.entry(c.parent).or_insert_with(|| (Vec::new(), 0, 0));
                if c.offset == e.0.len() {
                    e.0.extend(c.rows);
                    e.1 = c.more;
                    e.2 = c.rest_size;
                }
                g.asked.remove(&c.parent);
            }
            Ok(GuideData::Ledger(l)) => match &mut g.ledger {
                Some(old) if l.offset > 0 && l.offset == old.rows.len() && old.namespace == l.namespace => old.rows.extend(l.rows),
                _ => g.ledger = Some(l),
            },
            Ok(GuideData::Why(w)) => g.why = Some(w),
        }
        if self.v.view == View::Guide {
            self.rebuild_guide_rows();
            self.sync_view_detail();
        }
        true
    }

    fn guide_content_rows(&self, parent: Option<i64>, depth: u16, out: &mut Vec<GuideRow>) {
        let g = &self.v.guide;
        let Some((kids, more, rest)) = g.kids.get(&parent) else { return };
        for k in kids {
            let expandable = k.kids > 0;
            let expanded = expandable && g.expanded.contains(&k.row.id);
            out.push(GuideRow {
                key: format!("c:{}", k.row.id),
                depth,
                kind: GuideKind::Decl {
                    name: k.row.name.clone(),
                    kind: k.row.kind,
                    label: k.row.display.clone(),
                    left: thousands(k.excl as u64),
                    // A relay hands over to one declaration: both on one row.
                    right: if k.relay.is_empty() { String::new() } else { format!("= {}", k.relay.join(" = ")) },
                    expandable,
                    expanded,
                    below: Some(k.below),
                    note: None,
                    dim: k.excl <= 1,
                },
            });
            if expanded {
                if g.kids.contains_key(&Some(k.below)) {
                    self.guide_content_rows(Some(k.below), depth + 1, out);
                } else {
                    out.push(GuideRow { key: format!("load:{}", k.below), depth: depth + 1, kind: GuideKind::Text { text: "reading…".into(), warn: false } });
                }
            }
        }
        if *more > 0 {
            out.push(GuideRow {
                key: format!("more:{parent:?}"),
                depth,
                kind: GuideKind::More { parent, offset: kids.len(), text: format!("… {} smaller ones, {} declarations in all (Enter: more)", thousands(*more as u64), thousands(*rest)) },
            });
        }
    }

    pub fn rebuild_guide_rows(&mut self) {
        let keep = self.v.guide.rows.get(self.v.guide.list.selected).map(|r| r.key.clone());
        let mut rows: Vec<GuideRow> = Vec::new();
        let text = |t: &str, warn: bool| GuideRow { key: format!("t:{t}"), depth: 0, kind: GuideKind::Text { text: t.to_string(), warn } };
        let section = |t: &str, right: String| GuideRow { key: format!("s:{t}"), depth: 0, kind: GuideKind::Section { title: t.to_string(), right } };
        let g = &self.v.guide;
        if let Some(e) = &g.error {
            rows.push(text(e, true));
        }
        match g.mode {
            GuideMode::Start => match &g.start {
                None if g.error.is_none() => rows.push(text("reading…", false)),
                None => {}
                Some(s) => {
                    rows.push(section("What the project ends in", if s.how.is_empty() { String::new() } else { format!("{} endpoint{}", s.endpoints.len(), if s.endpoints.len() == 1 { "" } else { "s" }) }));
                    if !s.how.is_empty() {
                        rows.push(text(&format!("chosen by: {}", s.how), false));
                    }
                    // The page keeps the ways in on screen: of many endpoints
                    // the first few are listed.
                    const SHOWN: usize = 6;
                    for (r, sig, _) in s.endpoints.iter().take(SHOWN) {
                        rows.push(GuideRow {
                            key: format!("e:{}", r.id),
                            depth: 1,
                            kind: GuideKind::Decl {
                                name: r.name.clone(),
                                kind: r.kind,
                                label: r.display.clone(),
                                left: String::new(),
                                right: String::new(),
                                expandable: false,
                                expanded: false,
                                below: None,
                                note: None,
                                dim: false,
                            },
                        });
                        // What it states, under its name (the detail pane has all of it).
                        // (Without the keyword and the name, which the row above has.)
                        let stated = sig.split_whitespace().skip(2).collect::<Vec<_>>().join(" ");
                        if !stated.is_empty() {
                            rows.push(GuideRow { key: format!("es:{}", r.id), depth: 3, kind: GuideKind::Text { text: stated, warn: false } });
                        }
                    }
                    if s.endpoints.len() > SHOWN {
                        rows.push(text(&format!("      … {} more (I lists the files that nothing imports)", s.endpoints.len() - SHOWN), false));
                    }
                    for n in s.notes.iter().filter(|n| !n.is_empty()) {
                        rows.push(text(n, true));
                    }
                    rows.push(section("Ways in", String::new()));
                    let door = |title: &str, hint: String, cmd: Cmd| GuideRow { key: format!("d:{title}"), depth: 1, kind: GuideKind::Door { title: title.to_string(), hint, cmd } };
                    if self.map().is_some() {
                        let (trails, marks) = self.map().map(|m| (m.trails.len(), m.curated_landmark_count())).unwrap_or((0, 0));
                        rows.push(door("The project map", format!("{marks} landmarks, {trails} routes · M"), Cmd::Map));
                        if trails > 0 {
                            rows.push(door("Reading routes", format!("{trails} shipped with the map · T"), Cmd::Trail));
                        }
                    }
                    if s.graph.is_some() {
                        rows.push(door("Contents by dependence", match s.reached { Some((a, b)) => format!("{} of {} reached · D", thousands(a as u64), thousands(b as u64)), None => "D".into() }, Cmd::Contents));
                        if let Some((all, proved, _)) = s.named {
                            rows.push(door("Named statements and their proofs", format!("{} named, {} proved by name · N", thousands(all as u64), thousands(proved as u64)), Cmd::Ledger));
                        }
                    }
                    rows.push(door("Files by import order", "I".into(), Cmd::Imports));
                    rows.push(door("Files and namespaces", format!("{} files · O", thousands(s.files)), Cmd::Outline));
                    rows.push(door("Search names and statements", "/".into(), Cmd::FocusSearch));
                    rows.push(section("Size", String::new()));
                    rows.push(text(&format!("{} files · {} declarations · {} theorems · {} documented", thousands(s.files), thousands(s.decls), thousands(s.theorems), thousands(s.documented)), false));
                    if let Some(gd) = &s.graph {
                        rows.push(text(gd, false));
                    }
                    rows.push(text("Sizes are sizes, not progress. Nothing on this page was checked by slim.", false));
                }
            },
            GuideMode::Contents => {
                if !g.contents_head.is_empty() {
                    rows.push(section(&g.contents_head, String::new()));
                    rows.push(text("the number: what is reached only through the row", false));
                } else if g.error.is_none() {
                    rows.push(text("reading…", false));
                }
                self.guide_content_rows(None, 0, &mut rows);
            }
            GuideMode::Ledger => match &g.ledger {
                None if g.error.is_none() => rows.push(text("reading…", false)),
                None => {}
                Some(l) => {
                    rows.push(section(
                        &format!("{} named statements{}", thousands(l.total as u64), l.namespace.as_ref().map(|n| format!(" under {n}")).unwrap_or_default()),
                        format!("{} proved by name · {} reached", thousands(l.proved as u64), thousands(l.reached as u64)),
                    ));
                    rows.push(text("steps from an endpoint · name · by: the theorem of that type", false));
                    for it in &l.rows {
                        // A statement no theorem has as its type shows its uses only.
                        let by = match (&it.prover, it.provers) {
                            (Some(p), 1) => format!("by {} · ", super::state::short_name(&p.display)),
                            (Some(p), n) => format!("by {} (+{}) · ", super::state::short_name(&p.display), n - 1),
                            _ => String::new(),
                        };
                        rows.push(GuideRow {
                            key: format!("l:{}", it.statement.id),
                            depth: 0,
                            kind: GuideKind::Decl {
                                name: it.statement.name.clone(),
                                kind: it.statement.kind,
                                label: it.statement.display.clone(),
                                left: match it.depth {
                                    Some(d) => format!("{d:>3}"),
                                    None => "  ·".into(),
                                },
                                right: format!("{by}{} use{}", it.users, if it.users == 1 { "" } else { "s" }),
                                expandable: false,
                                expanded: false,
                                below: it.prover.as_ref().map(|p| p.id),
                                note: None,
                                dim: it.depth.is_none(),
                            },
                        });
                    }
                    if l.rows.len() < l.total {
                        rows.push(GuideRow { key: "more:ledger".into(), depth: 0, kind: GuideKind::More { parent: None, offset: l.rows.len(), text: format!("… {} more (Enter)", thousands((l.total - l.rows.len()) as u64)) } });
                    }
                }
            },
            GuideMode::Why => match &g.why {
                None if g.error.is_none() => rows.push(text("reading…", false)),
                None => {}
                Some(w) if !w.reached => {
                    rows.push(section(&w.name, String::new()));
                    rows.push(text(&format!("not reached from the endpoints ({}) in this data: {}", w.how, w.label), true));
                    rows.push(text(&format!("{} declaration(s) refer to it (tab 2 lists them)", w.users), false));
                }
                Some(w) => {
                    rows.push(section(&format!("A shortest chain of references to {}", super::state::short_name(&w.name)), format!("{} steps", w.chain.len().saturating_sub(1))));
                    rows.push(text(&format!("{} · endpoints: {}", w.label, w.how), false));
                    rows.push(text("each row is written in the one above it: in its statement, or in its proof or body", false));
                    for (k, (r, how)) in w.chain.iter().enumerate() {
                        rows.push(GuideRow {
                            key: format!("w:{k}"),
                            depth: 0,
                            kind: GuideKind::Decl { name: r.name.clone(), kind: r.kind, label: r.display.clone(), left: format!("{k:>3}"), right: how.to_string(), expandable: false, expanded: false, below: None, note: None, dim: false },
                        });
                    }
                    if !w.dominators.is_empty() {
                        rows.push(section("Every chain to it passes through", format!("{} declarations", w.dominators.len())));
                        for (k, r) in w.dominators.iter().enumerate() {
                            rows.push(GuideRow {
                                key: format!("wd:{k}"),
                                depth: 1,
                                kind: GuideKind::Decl { name: r.name.clone(), kind: r.kind, label: r.display.clone(), left: String::new(), right: String::new(), expandable: false, expanded: false, below: None, note: None, dim: false },
                            });
                        }
                    }
                    rows.push(text(&format!("{} declaration(s) are reached only through it · {} refer to it", thousands(w.excl.saturating_sub(1) as u64), w.users), false));
                }
            },
        }
        let g = &mut self.v.guide;
        g.rows = rows;
        // The selection stays on its row; a fresh list starts on its
        // first declaration or door.
        match keep.and_then(|k| g.rows.iter().position(|r| r.key == k)) {
            Some(i) => g.list.selected = i,
            // (A chain is asked about its last link: the detail pane shows that one.)
            None if g.mode == GuideMode::Why => g.list.selected = g.rows.iter().rposition(|r| r.key.starts_with("w:")).unwrap_or(0),
            None => g.list.selected = g.rows.iter().position(|r| matches!(r.kind, GuideKind::Decl { .. } | GuideKind::Door { .. })).unwrap_or(0),
        }
        self.dirty = true;
    }

    pub fn guide_subject(&self) -> Subject {
        match self.v.guide.rows.get(self.v.guide.list.selected).map(|r| &r.kind) {
            Some(GuideKind::Decl { name, .. }) => Subject::Decl { name: name.clone(), role: None, stop: None, gnode: None, conn_step: None },
            Some(GuideKind::Section { title, .. }) => Subject::Section(title.clone()),
            Some(GuideKind::Text { text, .. }) | Some(GuideKind::More { text, .. }) => Subject::Section(text.clone()),
            Some(GuideKind::Door { title, hint, .. }) => Subject::Section(format!("{title} — {hint}")),
            None => Subject::None,
        }
    }

    pub fn guide_activate(&mut self) {
        let Some(row) = self.v.guide.rows.get(self.v.guide.list.selected).cloned() else { return };
        match row.kind {
            GuideKind::Decl { name, .. } => self.open_from_view(&name, self.v.guide.mode.title().trim()),
            GuideKind::Door { cmd, .. } => self.run_cmd(cmd),
            GuideKind::More { parent, offset, .. } => match self.v.guide.mode {
                GuideMode::Ledger => {
                    let ns = self.v.guide.ledger_ns.clone();
                    self.guide_ask(GuideReq::Ledger { namespace: ns, offset });
                }
                _ => {
                    if self.v.guide.asked.insert(parent) {
                        self.guide_ask(GuideReq::Children { parent, offset });
                    }
                }
            },
            _ => {}
        }
    }

    pub fn guide_toggle(&mut self, open: Option<bool>) {
        if self.v.guide.mode != GuideMode::Contents {
            return;
        }
        let Some(row) = self.v.guide.rows.get(self.v.guide.list.selected).cloned() else { return };
        let GuideKind::Decl { expandable, expanded, below: Some(below), .. } = row.kind else { return };
        let Some(id) = row.key.strip_prefix("c:").and_then(|s| s.parse::<i64>().ok()) else { return };
        let want = open.unwrap_or(!expanded);
        if !expandable || want == expanded {
            // Left on a closed row: up to the row it hangs under.
            if open == Some(false) {
                let d = row.depth;
                if let Some(p) = (0..self.v.guide.list.selected).rev().find(|&i| self.v.guide.rows[i].depth < d && matches!(self.v.guide.rows[i].kind, GuideKind::Decl { .. })) {
                    self.v.guide.list.selected = p;
                }
            }
            return;
        }
        if want {
            self.v.guide.expanded.insert(id);
            if !self.v.guide.kids.contains_key(&Some(below)) && self.v.guide.asked.insert(Some(below)) {
                self.guide_ask(GuideReq::Children { parent: Some(below), offset: 0 });
            }
        } else {
            self.v.guide.expanded.remove(&id);
        }
        self.rebuild_guide_rows();
    }

    /// Keys of the reading views beyond moving and opening.
    pub fn guide_key(&mut self, k: KeyEvent) -> bool {
        match (self.v.guide.mode, k.code) {
            (GuideMode::Ledger, KeyCode::Char('f')) => {
                // The namespace of the selected statement, one level up each time.
                let cur = self.v.guide.ledger_ns.clone();
                let sel = match self.v.guide.rows.get(self.v.guide.list.selected).map(|r| &r.kind) {
                    Some(GuideKind::Decl { name, .. }) => Some(crate::model::namespace_of(name).to_string()),
                    _ => None,
                };
                let next = match (cur, sel) {
                    (None, Some(ns)) if !ns.is_empty() => Some(ns),
                    (Some(c), _) => {
                        let up = crate::model::namespace_of(&c).to_string();
                        (!up.is_empty()).then_some(up)
                    }
                    _ => None,
                };
                self.v.guide.ledger_ns = next.clone();
                self.v.guide.ledger = None;
                self.guide_ask(GuideReq::Ledger { namespace: next.clone(), offset: 0 });
                self.msg(match next {
                    Some(n) => format!("named statements under {n} (f: wider, F: all)"),
                    None => "all named statements".into(),
                });
                self.rebuild_guide_rows();
                true
            }
            (GuideMode::Ledger, KeyCode::Char('F')) => {
                self.v.guide.ledger_ns = None;
                self.v.guide.ledger = None;
                self.guide_ask(GuideReq::Ledger { namespace: None, offset: 0 });
                self.rebuild_guide_rows();
                true
            }
            (GuideMode::Ledger, KeyCode::Char('p')) => {
                // The theorem that proves the selected statement.
                if let Some(GuideKind::Decl { below: Some(_), .. }) = self.v.guide.rows.get(self.v.guide.list.selected).map(|r| &r.kind) {
                    let prover = self.v.guide.ledger.as_ref().and_then(|l| l.rows.iter().find(|it| format!("l:{}", it.statement.id) == self.v.guide.rows[self.v.guide.list.selected].key)).and_then(|it| it.prover.clone());
                    if let Some(p) = prover {
                        self.open_from_view(&p.name, "its proof");
                    }
                } else {
                    self.msg("no theorem has exactly this statement as its type");
                }
                true
            }
            _ => false,
        }
    }

    // ------------------------------------------------------------------
    // The declaration view: position and contract.

    /// Asks where the open declaration stands and, when the reader had a
    /// statement unfolded, for the new one's.
    pub fn guide_on_open(&mut self, id: i64) {
        let goal = self.guide_goal();
        self.v.guide.position = None;
        if let Some(t) = &self.ch.query {
            let _ = t.send(QueryReq::Position { gen: self.gens.detail, goal, id });
            if self.v.guide.contract_depth > 0 {
                let _ = t.send(QueryReq::Contract { gen: self.gens.detail, id, depth: self.v.guide.contract_depth });
            }
        }
    }

    /// `u` unfolds the statement one level more; `U` folds one back.
    pub fn cmd_unfold(&mut self, more: bool) {
        let Some(id) = self.current else {
            self.msg("open a declaration first");
            return;
        };
        if self.ctx.project_root.is_none() && self.ctx.index.as_ref().is_none_or(|i| i.source_root.is_none()) {
            self.msg("unfolding reads the source files: this index has no source root");
            return;
        }
        let g = &mut self.v.guide;
        if more {
            if g.contract.as_ref().is_some_and(|(i, c)| *i == id && !c.deeper && g.contract_depth > 0) {
                self.msg("nothing of the project is left folded in this statement");
                return;
            }
            g.contract_depth = (g.contract_depth + 1).min(8);
        } else if g.contract_depth == 0 {
            self.msg("the statement is shown as written (u unfolds its names)");
            return;
        } else {
            g.contract_depth -= 1;
        }
        let depth = g.contract_depth;
        if depth == 0 {
            g.contract = None;
            self.center.version += 1;
            self.msg("statement as written");
            self.dirty = true;
            return;
        }
        if let Some(t) = &self.ch.query {
            let _ = t.send(QueryReq::Contract { gen: self.gens.detail, id, depth });
        }
        self.msg(format!("unfolding the statement's names: level {depth} (u more, U less)"));
    }

    pub fn on_contract(&mut self, gen: u64, id: i64, res: Result<Box<Contract>, String>) -> bool {
        if gen != self.gens.detail || self.current != Some(id) {
            return false;
        }
        match res {
            Ok(c) => {
                self.v.guide.contract = Some((id, c));
                self.v.guide.contract_error = None;
            }
            Err(e) => {
                self.v.guide.contract = None;
                self.v.guide.contract_error = Some(e);
            }
        }
        self.center.version += 1;
        self.center.scroll = 0;
        true
    }

    pub fn on_position(&mut self, gen: u64, id: i64, p: Option<Position>) -> bool {
        if gen != self.gens.detail || self.current != Some(id) {
            return false;
        }
        self.v.guide.position = p;
        self.center.version += 1;
        true
    }

    /// The contract of the open declaration, when one is unfolded.
    pub fn contract_for(&self, id: i64) -> Option<&Contract> {
        self.v.guide.contract.as_ref().filter(|(i, _)| *i == id && self.v.guide.contract_depth > 0).map(|(_, c)| c.as_ref())
    }
}
