//! The architecture map, the goal explorer and reading trails: three
//! navigable views over the same declaration identities, next to the
//! milestone-1 declaration browser.
//!
//! Each view is an outline (left) with a detail pane (right). Rows are
//! identified by stable keys, not indexes, so a selection survives reloads,
//! filtering and resizing. Like the rest of the UI thread, nothing here
//! touches SQLite or files: requests go to the workers and answers are
//! applied when their generation is still current.

use super::state::*;
use super::worker::*;
use crate::db::query::{DeclRow, Direction};
use crate::db::IndexMode;
use crate::imports::{DirSummary, ImportGraph, PathAnswer};
use crate::map::{MapLoad, ProjectMap, Unresolved};
use crate::skim::SkimScope;
use crate::relations::{ChildPage, ChildQuery, ConnOutcome, ConnQuery, ConnResult, EdgeLabel, Limits, RelMode, TypeBody};
use crate::userdata::{RefKind, Stop, StopStatus};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum View {
    /// The milestone-1 declaration browser (search, statement, neighbours).
    #[default]
    Browse,
    Map,
    Goal,
    Trails,
    /// Files by import depth, endpoints and hubs.
    Imports,
}

impl View {
    pub fn label(self) -> &'static str {
        match self {
            View::Browse => "Declarations",
            View::Map => "Architecture map",
            View::Goal => "Goal explorer",
            View::Trails => "Reading trails",
            View::Imports => "Import layers",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VFocus {
    Outline,
    Detail,
}

/// Below this width the map, goal and trail views show one pane at a time
/// (Tab switches between the outline and the detail).
pub const SINGLE_PANE_BELOW: u16 = 80;

#[derive(Clone, Debug)]
pub enum MapRowKind {
    Section { title: String, count: String, expanded: bool },
    Objective { decl: String, title: Option<String> },
    Group { idx: usize, expanded: bool },
    Landmark { group: usize, decl: String, role: Option<String> },
    Unresolved(Unresolved),
    Issue { text: String, error: bool },
    /// A node of the file/namespace tree (generated organization).
    Module { node: usize, expanded: bool },
    Note(String),
}

#[derive(Clone, Debug)]
pub struct MapRow {
    pub key: String,
    pub depth: u16,
    pub kind: MapRowKind,
}

#[derive(Default)]
pub struct MapView {
    pub bundle: Option<MapBundle>,
    pub rows: Vec<MapRow>,
    pub list: ListState,
    pub expanded: HashSet<String>,
    /// A row to select once the map (or the module tree) has loaded.
    pub pending_reveal: Option<String>,
    /// The reader asked for a reload: report its outcome.
    pub reload_requested: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GoalRef {
    pub name: String,
    pub display: String,
}

/// One node of the unfolded relation outline. The underlying data is a
/// graph; the same declaration can be several nodes.
#[derive(Clone, Debug)]
pub struct GNode {
    pub key: u64,
    pub name: String,
    pub row: Option<DeclRow>,
    pub depth: u16,
    pub parent: Option<usize>,
    pub labels: Vec<EdgeLabel>,
    pub note: Option<String>,
    pub expanded: bool,
    pub loaded: bool,
    pub loading: bool,
    /// Already on the path from the goal to here: shown as a back-reference
    /// and never unfolded again.
    pub backref: bool,
    /// Also appears elsewhere in the outline (it can still be unfolded here).
    pub repeat: bool,
    pub children: Vec<usize>,
    pub page: ChildPage,
}

#[derive(Clone, Debug)]
pub enum GRow {
    Node(usize),
    More { node: usize, offset: usize, remaining: u64, capped: bool },
    Note { node: usize, text: String, warn: bool },
    /// A connection query result, shown above the outline.
    ConnHeader { text: String, warn: bool },
    ConnStep(usize),
    /// An objective of the map offered as a goal.
    Pick(usize),
    Info(String),
}

pub struct ConnState {
    pub query: ConnQuery,
    pub running: bool,
    pub result: Option<ConnResult>,
}

pub struct GoalView {
    pub goal: Option<GoalRef>,
    pub mode: RelMode,
    pub dir: Direction,
    pub tb: TypeBody,
    /// Show everything the display filters would hide.
    pub reveal_hidden: bool,
    pub nodes: Vec<GNode>,
    pub rows: Vec<GRow>,
    pub list: ListState,
    pub next_key: u64,
    pub conn: Option<ConnState>,
    pub conn_latest: Arc<AtomicU64>,
    pub conn_thread: Option<std::thread::JoinHandle<()>>,
    /// The saved goal was applied (once per session).
    pub restored: bool,
}

impl Default for GoalView {
    fn default() -> Self {
        GoalView {
            goal: None,
            mode: RelMode::Curated,
            dir: Direction::Uses,
            tb: TypeBody::All,
            reveal_hidden: false,
            nodes: Vec::new(),
            rows: Vec::new(),
            list: ListState::default(),
            next_key: 0,
            conn: None,
            conn_latest: Arc::new(AtomicU64::new(0)),
            conn_thread: None,
            restored: false,
        }
    }
}

#[derive(Clone, Debug)]
pub enum TRow {
    Header(String),
    Trail(usize),
    Stop { trail: usize, stop: usize },
    /// A trail shipped with the map that the reader has not started.
    Shared(usize),
    Question(usize),
    Note(String),
}

#[derive(Default)]
pub struct TrailsView {
    pub reading: Reading,
    pub loaded: bool,
    pub rows: Vec<TRow>,
    pub list: ListState,
    pub expanded: HashSet<i64>,
}

impl TrailsView {
    pub fn active(&self) -> Option<i64> {
        let id = self.reading.prefs.get("active_trail")?.parse::<i64>().ok()?;
        self.reading.trails.iter().any(|t| t.id == id).then_some(id)
    }
}

/// Which list of the import view a file row belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IList {
    Endpoint,
    Hub,
    ImportOnly,
}

#[derive(Clone, Debug)]
pub enum IRowKind {
    Section { title: String, count: String, expanded: bool },
    /// A directory or file of the layer tree.
    Dir { dir: u32, expanded: bool },
    /// A file listed under endpoints, hubs or import-only files.
    Module { node: u32, what: IList },
    /// One module of an import path that was asked for.
    PathStep { name: String, step: usize },
    Note { text: String, warn: bool },
}

#[derive(Clone, Debug)]
pub struct IRow {
    pub key: String,
    pub depth: u16,
    pub kind: IRowKind,
}

/// The import view: files by import depth, endpoints and hubs.
pub struct ImportsView {
    pub graph: Option<Arc<ImportGraph>>,
    pub requested: bool,
    pub error: Option<String>,
    pub rows: Vec<IRow>,
    pub list: ListState,
    pub expanded: HashSet<String>,
    /// What the selected directory imports and what imports it (computed
    /// on the worker; `dir_want` is the directory asked about).
    pub dir: Option<DirSummary>,
    pub dir_want: Option<String>,
    /// The last "why does A import B" answer.
    pub path: Option<PathAnswer>,
    pub path_running: bool,
    /// A row to select once the graph has arrived.
    pub pending_key: Option<String>,
}

impl Default for ImportsView {
    fn default() -> Self {
        let expanded = ["sec:path", "sec:layers", "sec:endpoints", "sec:hubs"].iter().map(|s| s.to_string()).collect();
        ImportsView { graph: None, requested: false, error: None, rows: Vec::new(), list: ListState::default(), expanded, dir: None, dir_want: None, path: None, path_running: false, pending_key: None }
    }
}

/// Rows listed per section before a "… more" note.
const IMPORT_LIST_CAP: usize = 300;

/// What the detail pane describes.
#[derive(Clone, Debug)]
pub enum Subject {
    None,
    Section(String),
    Group(usize),
    Decl { name: String, role: Option<String>, stop: Option<(usize, usize)>, gnode: Option<usize>, conn_step: Option<usize> },
    Unresolved(Unresolved),
    Issue(String),
    Module(usize),
    Trail(usize),
    Shared(usize),
    Question(usize),
    GroupStop { trail: usize, stop: usize },
    /// A directory or file of the import view's layer tree.
    ImportDir(u32),
    /// A file of the import graph.
    ImportModule(u32),
}

impl Subject {
    fn decl(name: &str) -> Subject {
        Subject::Decl { name: name.to_string(), role: None, stop: None, gnode: None, conn_step: None }
    }
    pub fn decl_name(&self) -> Option<&str> {
        match self {
            Subject::Decl { name, .. } => Some(name),
            _ => None,
        }
    }
}

/// The shared detail pane of the three views.
pub struct ViewDetail {
    /// The declaration the pane wants a detail bundle for.
    pub want: Option<String>,
    pub bundle: Option<Box<DetailBundle>>,
    pub bundle_name: String,
    /// The wanted name is not in the index.
    pub missing: bool,
    pub scroll: usize,
    pub hscroll: usize,
    pub wrap: bool,
    /// Scroll position remembered per item.
    pub scrolls: HashMap<String, usize>,
    pub key: String,
    pub version: u64,
    pub cache: Option<(u16, u64, Vec<ratatui::text::Line<'static>>)>,
    pub height: usize,
}

impl Default for ViewDetail {
    fn default() -> Self {
        ViewDetail {
            want: None,
            bundle: None,
            bundle_name: String::new(),
            missing: false,
            scroll: 0,
            hscroll: 0,
            wrap: true,
            scrolls: HashMap::new(),
            key: String::new(),
            version: 0,
            cache: None,
            height: 10,
        }
    }
}

pub struct ViewsState {
    pub view: View,
    pub focus: VFocus,
    pub map: MapView,
    pub goal: GoalView,
    pub trails: TrailsView,
    pub imports: ImportsView,
    pub detail: ViewDetail,
    /// Set by the renderer: the last frame showed one pane only.
    pub single_pane: bool,
    pub list_height: usize,
}

impl Default for ViewsState {
    fn default() -> Self {
        let mut map = MapView::default();
        for k in ["sec:objectives", "sec:components", "sec:foundations", "sec:unresolved", "sec:issues"] {
            map.expanded.insert(k.to_string());
        }
        ViewsState { view: View::Browse, focus: VFocus::Outline, map, goal: GoalView::default(), trails: TrailsView::default(), imports: ImportsView::default(), detail: ViewDetail::default(), single_pane: false, list_height: 20 }
    }
}

fn clamp_sel(sel: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        (sel as isize + delta).clamp(0, len as isize - 1) as usize
    }
}

impl App {
    pub fn map(&self) -> Option<&Arc<ProjectMap>> {
        self.v.map.bundle.as_ref().and_then(|b| b.load.map())
    }

    fn q2(&self, r: QueryReq) {
        if let Some(t) = &self.ch.query {
            let _ = t.send(r);
        }
    }

    // ------------------------------------------------------------------
    // Switching views
    // ------------------------------------------------------------------

    /// Shows a view. Switching is a navigation step: Backspace returns.
    pub fn set_view(&mut self, v: View) {
        if self.v.view != v {
            let loc = self.location();
            self.history.back.push(loc);
            self.history.fwd.clear();
            self.v.view = v;
            self.v.focus = VFocus::Outline;
        }
        self.preview_due = None;
        self.preview_target = None;
        match v {
            View::Browse => {}
            View::Map => self.rebuild_map_rows(),
            View::Goal => {
                if self.v.goal.goal.is_some() && self.v.goal.nodes.is_empty() {
                    self.reset_goal_tree();
                }
                self.rebuild_goal_rows();
            }
            View::Trails => {
                self.q2(QueryReq::LoadReading);
                self.rebuild_trail_rows();
            }
            View::Imports => {
                self.request_imports();
                self.rebuild_import_rows();
            }
        }
        self.sync_view_detail();
        self.dirty = true;
    }

    /// Key of the selected row of the current view (for history).
    pub fn view_key(&self) -> Option<String> {
        match self.v.view {
            View::Map => self.v.map.rows.get(self.v.map.list.selected).map(|r| r.key.clone()),
            View::Imports => self.v.imports.rows.get(self.v.imports.list.selected).map(|r| r.key.clone()),
            _ => None,
        }
    }

    /// Restores a view recorded in the history.
    pub fn restore_view(&mut self, v: View, key: Option<String>) {
        self.v.view = v;
        match v {
            View::Map => {
                self.rebuild_map_rows();
                if let Some(k) = key {
                    self.map_select_key(&k);
                }
            }
            View::Goal => self.rebuild_goal_rows(),
            View::Trails => self.rebuild_trail_rows(),
            View::Imports => {
                self.request_imports();
                self.rebuild_import_rows();
                if let Some(k) = key {
                    if !self.imports_select_key(&k) {
                        self.v.imports.pending_key = Some(k);
                    }
                }
            }
            View::Browse => {}
        }
        self.sync_view_detail();
    }

    /// Opens a declaration by name in the declaration browser, recording
    /// where the reader came from.
    pub fn open_from_view(&mut self, name: &str, how: &str) {
        let loc = self.location();
        self.history.back.push(loc);
        self.history.fwd.clear();
        self.v.view = View::Browse;
        self.focus = Focus::Center;
        self.search.editing = false;
        self.preview_session = false;
        self.current_how = how.to_string();
        self.goto_name(name.to_string());
        self.dirty = true;
    }

    // ------------------------------------------------------------------
    // Architecture map
    // ------------------------------------------------------------------

    pub fn on_map_bundle(&mut self, b: MapBundle) {
        let first = self.v.map.bundle.is_none();
        let asked = std::mem::take(&mut self.v.map.reload_requested);
        match &b.load {
            MapLoad::Loaded(m) => {
                let n = m.issues.len();
                if asked {
                    self.msg(format!("project map reloaded: {} groups, {} curated landmarks{}", m.groups.len(), m.curated_landmark_count(), if n > 0 { format!(", {n} problems") } else { String::new() }));
                }
            }
            MapLoad::Malformed { path, issues } => self.warn(format!(
                "{} could not be used ({}); it was left unchanged and the browser works without it — M shows the details",
                path.display(),
                issues.first().map(|i| i.line()).unwrap_or_default()
            )),
            MapLoad::Absent { .. } => {
                if asked {
                    self.msg("no project map found (lean-view.toml)");
                }
            }
        }
        // A project with a map opens on it, unless the reader already did
        // something or a previous session is being resumed.
        let fresh = first && !self.user_acted && self.v.view == View::Browse && self.current.is_none() && self.pending_goto.is_none() && self.search.query.is_empty();
        if fresh && matches!(b.load, MapLoad::Loaded(_)) {
            self.v.view = View::Map;
            self.search.editing = false;
        }
        self.v.map.bundle = Some(b);
        self.rebuild_map_rows();
        if let Some(k) = self.v.map.pending_reveal.take() {
            self.map_reveal(&k);
        }
        // Search results may now include map entries.
        if !self.search.query.trim().is_empty() && !first {
            self.run_search();
        }
        // Ids may have changed with the index: ask for the detail again.
        self.v.detail.bundle_name.clear();
        self.v.detail.want = None;
        self.v.detail.version += 1;
        self.sync_view_detail();
        self.dirty = true;
    }

    fn map_expanded(&self, key: &str) -> bool {
        self.v.map.expanded.contains(key)
    }

    pub fn rebuild_map_rows(&mut self) {
        let keep = self.v.map.rows.get(self.v.map.list.selected).map(|r| r.key.clone());
        let mut rows: Vec<MapRow> = Vec::new();
        let sec = |rows: &mut Vec<MapRow>, key: &str, title: &str, count: String, exp: bool| {
            rows.push(MapRow { key: key.into(), depth: 0, kind: MapRowKind::Section { title: title.into(), count, expanded: exp } });
        };
        let note = |rows: &mut Vec<MapRow>, key: String, text: &str| rows.push(MapRow { key, depth: 1, kind: MapRowKind::Note(text.to_string()) });
        let bundle = self.v.map.bundle.clone();
        match bundle.as_ref().map(|b| &b.load) {
            None => note(&mut rows, "note:loading".into(), "loading the project map…"),
            Some(MapLoad::Absent { looked_at }) => {
                sec(&mut rows, "sec:nomap", "No project map", String::new(), true);
                let at = looked_at.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "the source root (none is known for this index)".into());
                for (i, t) in [
                    format!("No lean-view.toml at {at}."),
                    "Everything else works without one. A map adds main results, mathematical components and landmarks chosen by a person.".to_string(),
                    "`slim map init <PROJECT_DIR>` prints a starting file derived from directories (labelled generated).".to_string(),
                ]
                .iter()
                .enumerate()
                {
                    note(&mut rows, format!("note:nomap{i}"), t);
                }
            }
            Some(MapLoad::Malformed { path, issues }) => {
                sec(&mut rows, "sec:issues", "Map file could not be used", format!("{} problem(s)", issues.len()), true);
                note(&mut rows, "note:malformed".into(), &format!("{} was left unchanged. Fix it and press r to reload; the declaration browser works meanwhile.", path.display()));
                for (i, is) in issues.iter().enumerate() {
                    rows.push(MapRow { key: format!("issue:{i}"), depth: 1, kind: MapRowKind::Issue { text: is.line(), error: true } });
                }
            }
            Some(MapLoad::Loaded(m)) => {
                let res = &bundle.as_ref().unwrap().resolution;
                let e = self.map_expanded("sec:objectives");
                sec(&mut rows, "sec:objectives", "Main results", format!("{} chosen", m.objectives.len()), e);
                if e {
                    for o in &m.objectives {
                        rows.push(MapRow { key: format!("obj:{}", o.decl), depth: 1, kind: MapRowKind::Objective { decl: o.decl.clone(), title: o.title.clone() } });
                    }
                    if m.objectives.is_empty() {
                        note(&mut rows, "note:noobj".into(), "none chosen. Objectives are never inferred: add [[objective]] entries to the map.");
                    }
                }
                fn walk(app: &App, m: &ProjectMap, k: usize, depth: u16, rows: &mut Vec<MapRow>) {
                    let g = &m.groups[k];
                    let key = format!("grp:{}", g.id);
                    let exp = app.map_expanded(&key);
                    rows.push(MapRow { key, depth, kind: MapRowKind::Group { idx: k, expanded: exp } });
                    if !exp {
                        return;
                    }
                    for p in &g.landmarks {
                        rows.push(MapRow { key: format!("lm:{}:{}", g.id, p.decl), depth: depth + 1, kind: MapRowKind::Landmark { group: k, decl: p.decl.clone(), role: p.role.clone() } });
                    }
                    for &c in &g.children {
                        walk(app, m, c, depth + 1, rows);
                    }
                }
                let comps: Vec<usize> = m.roots.iter().copied().filter(|&k| m.groups[k].kind == crate::map::GroupKind::Component).collect();
                let founds: Vec<usize> = m.roots.iter().copied().filter(|&k| m.groups[k].kind == crate::map::GroupKind::Foundation).collect();
                let e = self.map_expanded("sec:components");
                sec(&mut rows, "sec:components", "Mathematical components", format!("{} curated", m.curated_landmark_count()), e);
                if e {
                    for &k in &comps {
                        walk(self, m, k, 1, &mut rows);
                    }
                    if comps.is_empty() {
                        note(&mut rows, "note:nogroups".into(), "no groups yet: add [[group]] entries to the map.");
                    }
                }
                let e = self.map_expanded("sec:foundations");
                let ext = m.decl_refs().iter().filter(|(n, _)| res.is_external(n)).count();
                sec(&mut rows, "sec:foundations", "External foundations", format!("{ext} listed"), e);
                if e {
                    for &k in &founds {
                        walk(self, m, k, 1, &mut rows);
                    }
                    if founds.is_empty() {
                        note(&mut rows, "note:nofound".into(), "no foundation group (kind = \"foundation\"). Library dependencies are not shown here unless listed; the goal explorer shows them per declaration.");
                    }
                }
                let un = res.unresolved(m);
                if !un.is_empty() {
                    let e = self.map_expanded("sec:unresolved");
                    sec(&mut rows, "sec:unresolved", "Unresolved references", format!("{}", un.len()), e);
                    if e {
                        for u in un {
                            rows.push(MapRow { key: format!("unres:{}", u.name), depth: 1, kind: MapRowKind::Unresolved(u) });
                        }
                    }
                }
                if !m.issues.is_empty() {
                    let e = self.map_expanded("sec:issues");
                    sec(&mut rows, "sec:issues", "Map problems", format!("{}", m.issues.len()), e);
                    if e {
                        for (i, is) in m.issues.iter().enumerate() {
                            rows.push(MapRow { key: format!("issue:{i}"), depth: 1, kind: MapRowKind::Issue { text: is.line(), error: is.severity == crate::map::Severity::Error } });
                        }
                    }
                }
            }
        }
        // The file/namespace tree is always available and always labelled.
        let e = self.map_expanded("sec:files");
        // Counts and their meaning are spelled out in the detail pane.
        sec(&mut rows, "sec:files", "Files and namespaces", "[generated]".into(), e);
        if e {
            if !self.outline.loaded {
                note(&mut rows, "note:files".into(), "loading modules…");
            } else {
                fn walk(app: &App, i: usize, depth: u16, rows: &mut Vec<MapRow>) {
                    let n = &app.outline.nodes[i];
                    let key = format!("mod:{}", n.full);
                    let exp = app.map_expanded(&key) && !n.children.is_empty();
                    rows.push(MapRow { key, depth, kind: MapRowKind::Module { node: i, expanded: exp } });
                    if exp {
                        for &c in app.outline.ordered(&n.children).iter() {
                            walk(app, c, depth + 1, rows);
                        }
                    }
                }
                for &r in self.outline.ordered(&self.outline.roots).iter() {
                    walk(self, r, 1, &mut rows);
                }
            }
        }
        let old = self.v.map.list.selected;
        self.v.map.rows = rows;
        if let Some(k) = keep {
            if let Some(i) = self.v.map.rows.iter().position(|r| r.key == k) {
                let delta = i as isize - old as isize;
                self.v.map.list.selected = i;
                self.v.map.list.scroll = (self.v.map.list.scroll as isize + delta).max(0) as usize;
            }
        }
        self.v.map.list.selected = self.v.map.list.selected.min(self.v.map.rows.len().saturating_sub(1));
        self.dirty = true;
    }

    fn map_select_key(&mut self, key: &str) -> bool {
        match self.v.map.rows.iter().position(|r| r.key == key) {
            Some(i) => {
                self.v.map.list.selected = i;
                self.v.map.list.scroll = i.saturating_sub(self.v.list_height / 3);
                true
            }
            None => false,
        }
    }

    /// Shows a map row by key, expanding whatever contains it.
    pub fn map_reveal(&mut self, key: &str) {
        let Some(m) = self.map().cloned() else {
            self.v.map.pending_reveal = Some(key.to_string());
            return;
        };
        let open_group = |app: &mut App, gid: &str, itself: bool| {
            if let Some(mut k) = m.group_by_id(gid) {
                if itself {
                    app.v.map.expanded.insert(format!("grp:{gid}"));
                }
                let mut top = k;
                while let Some(p) = m.groups[k].parent {
                    app.v.map.expanded.insert(format!("grp:{}", m.groups[p].id));
                    k = p;
                    top = p;
                }
                let sec = if m.groups[top].kind == crate::map::GroupKind::Foundation { "sec:foundations" } else { "sec:components" };
                app.v.map.expanded.insert(sec.to_string());
            }
        };
        if let Some(rest) = key.strip_prefix("lm:") {
            // `lm:<group id>:<decl>`; group ids contain no ':'.
            if let Some((gid, _)) = rest.split_once(':') {
                open_group(self, gid, true);
            }
        } else if let Some(gid) = key.strip_prefix("grp:") {
            open_group(self, gid, false);
        } else if key.starts_with("obj:") {
            self.v.map.expanded.insert("sec:objectives".into());
        } else if key.starts_with("unres:") {
            self.v.map.expanded.insert("sec:unresolved".into());
        }
        self.rebuild_map_rows();
        if !self.map_select_key(key) {
            self.msg("that entry is no longer in the project map");
        }
        self.sync_view_detail();
    }

    /// Map rows where a declaration is placed (objective first).
    pub fn map_placements(&self, decl: &str) -> Vec<String> {
        let Some(m) = self.map() else { return Vec::new() };
        let mut v = Vec::new();
        if m.is_objective(decl) {
            v.push(format!("obj:{decl}"));
        }
        for k in m.placements(decl) {
            v.push(format!("lm:{}:{decl}", m.groups[k].id));
        }
        v
    }

    /// `M`: open the map; from a declaration, reveal where it is placed, and
    /// cycle through its placements on repeated presses.
    pub fn cmd_map(&mut self) {
        let decl = match self.v.view {
            View::Browse => self.detail.as_ref().map(|d| d.d.row.name.clone()),
            _ => self.subject().decl_name().map(String::from),
        };
        let was_map = self.v.view == View::Map;
        self.set_view(View::Map);
        self.outline_for_map();
        let Some(decl) = decl else { return };
        let places = self.map_placements(&decl);
        if places.is_empty() {
            if !was_map && self.map().is_some() {
                self.msg(format!("{decl} is not placed in the project map (not a landmark)"));
            }
            return;
        }
        let cur = self.view_key();
        let next = match cur.and_then(|c| places.iter().position(|p| *p == c)) {
            Some(i) if was_map => (i + 1) % places.len(),
            _ => 0,
        };
        self.map_reveal(&places[next]);
        if places.len() > 1 {
            self.msg(format!("placement {} of {} of {decl} — M shows the next one", next + 1, places.len()));
        }
    }

    fn outline_for_map(&mut self) {
        if self.map_expanded("sec:files") && !self.outline.loaded {
            self.request_outline();
        }
    }

    fn map_toggle(&mut self, open: Option<bool>) {
        let Some(row) = self.v.map.rows.get(self.v.map.list.selected).cloned() else { return };
        let expandable = match &row.kind {
            MapRowKind::Section { .. } | MapRowKind::Group { .. } => true,
            MapRowKind::Module { node, .. } => self.outline.nodes.get(*node).is_some_and(|n| !n.children.is_empty()),
            _ => false,
        };
        if !expandable {
            if open == Some(false) {
                self.map_to_parent();
            }
            return;
        }
        let is_open = self.v.map.expanded.contains(&row.key);
        let want = open.unwrap_or(!is_open);
        if want == is_open {
            if !want {
                self.map_to_parent();
            }
            return;
        }
        if want {
            self.v.map.expanded.insert(row.key.clone());
        } else {
            self.v.map.expanded.remove(&row.key);
        }
        self.outline_for_map();
        self.rebuild_map_rows();
    }

    fn map_to_parent(&mut self) {
        let sel = self.v.map.list.selected;
        let Some(d) = self.v.map.rows.get(sel).map(|r| r.depth) else { return };
        if let Some(j) = (0..sel).rev().find(|&j| self.v.map.rows[j].depth < d) {
            self.v.map.list.selected = j;
        }
    }

    fn map_activate(&mut self) {
        let Some(row) = self.v.map.rows.get(self.v.map.list.selected).cloned() else { return };
        match row.kind {
            MapRowKind::Section { .. } | MapRowKind::Group { .. } => self.map_toggle(None),
            MapRowKind::Objective { decl, .. } | MapRowKind::Landmark { decl, .. } => {
                if self.v.map.bundle.as_ref().is_some_and(|b| b.resolution.is_missing(&decl)) {
                    self.warn(format!("{decl} is not in this index (see Unresolved references); nothing to open"));
                } else {
                    self.open_from_view(&decl, "map");
                }
            }
            MapRowKind::Unresolved(u) => {
                if u.suggestions.is_empty() {
                    self.warn(format!("{}: {}. Edit lean-view.toml and press r.", u.name, u.why));
                } else {
                    self.warn(format!(
                        "{}: {}. Similar names: {}. Nothing is re-attached automatically: edit lean-view.toml, then r.",
                        u.name,
                        u.why,
                        u.suggestions.join(", ")
                    ));
                }
            }
            MapRowKind::Module { node, .. } => {
                let Some(n) = self.outline.nodes.get(node) else { return };
                if n.module.is_some() {
                    let name = n.full.clone();
                    let loc = self.location();
                    self.history.back.push(loc);
                    self.history.fwd.clear();
                    self.v.view = View::Browse;
                    self.reveal_in_outline(name, None);
                } else {
                    self.map_toggle(None);
                }
            }
            MapRowKind::Issue { .. } | MapRowKind::Note(_) => {}
        }
    }

    // ------------------------------------------------------------------
    // Detail pane
    // ------------------------------------------------------------------

    /// What the selected row of the current view is about.
    pub fn subject(&self) -> Subject {
        match self.v.view {
            View::Browse => Subject::None,
            View::Map => match self.v.map.rows.get(self.v.map.list.selected).map(|r| &r.kind) {
                Some(MapRowKind::Section { title, .. }) => Subject::Section(title.clone()),
                Some(MapRowKind::Objective { decl, .. }) => Subject::decl(decl),
                Some(MapRowKind::Group { idx, .. }) => Subject::Group(*idx),
                Some(MapRowKind::Landmark { decl, role, .. }) => Subject::Decl { name: decl.clone(), role: role.clone(), stop: None, gnode: None, conn_step: None },
                Some(MapRowKind::Unresolved(u)) => Subject::Unresolved(u.clone()),
                Some(MapRowKind::Issue { text, .. }) => Subject::Issue(text.clone()),
                Some(MapRowKind::Module { node, .. }) => Subject::Module(*node),
                Some(MapRowKind::Note(t)) => Subject::Issue(t.clone()),
                None => Subject::None,
            },
            View::Goal => match self.v.goal.rows.get(self.v.goal.list.selected) {
                Some(GRow::Node(i)) => Subject::Decl { name: self.v.goal.nodes[*i].name.clone(), role: None, stop: None, gnode: Some(*i), conn_step: None },
                Some(GRow::More { node, .. }) | Some(GRow::Note { node, .. }) => {
                    Subject::Decl { name: self.v.goal.nodes[*node].name.clone(), role: None, stop: None, gnode: Some(*node), conn_step: None }
                }
                Some(GRow::ConnStep(i)) => match self.v.goal.conn.as_ref().and_then(|c| c.result.as_ref()).map(|r| &r.outcome) {
                    Some(ConnOutcome::Path(p)) if *i < p.len() => Subject::Decl { name: p[*i].name.clone(), role: None, stop: None, gnode: None, conn_step: Some(*i) },
                    _ => Subject::None,
                },
                Some(GRow::Pick(i)) => match self.map().and_then(|m| m.objectives.get(*i)) {
                    Some(o) => Subject::decl(&o.decl),
                    None => Subject::None,
                },
                Some(GRow::ConnHeader { text, .. }) | Some(GRow::Info(text)) => Subject::Section(text.clone()),
                None => Subject::None,
            },
            View::Trails => match self.v.trails.rows.get(self.v.trails.list.selected) {
                Some(TRow::Trail(t)) => Subject::Trail(*t),
                Some(TRow::Stop { trail, stop }) => match self.v.trails.reading.trails.get(*trail).and_then(|t| t.stops.get(*stop)) {
                    Some(s) if s.ref_kind == RefKind::Decl => Subject::Decl { name: s.name.clone(), role: None, stop: Some((*trail, *stop)), gnode: None, conn_step: None },
                    Some(_) => Subject::GroupStop { trail: *trail, stop: *stop },
                    None => Subject::None,
                },
                Some(TRow::Shared(i)) => Subject::Shared(*i),
                Some(TRow::Question(i)) => Subject::Question(*i),
                Some(TRow::Header(t)) | Some(TRow::Note(t)) => Subject::Section(t.clone()),
                None => Subject::None,
            },
            View::Imports => match self.v.imports.rows.get(self.v.imports.list.selected).map(|r| &r.kind) {
                Some(IRowKind::Section { title, .. }) => Subject::Section(title.clone()),
                Some(IRowKind::Dir { dir, .. }) => Subject::ImportDir(*dir),
                Some(IRowKind::Module { node, .. }) => Subject::ImportModule(*node),
                Some(IRowKind::PathStep { name, .. }) => match self.v.imports.graph.as_ref().and_then(|g| g.node(name)) {
                    Some(n) => Subject::ImportModule(n),
                    None => Subject::None,
                },
                Some(IRowKind::Note { text, .. }) => Subject::Section(text.clone()),
                None => Subject::None,
            },
        }
    }

    fn subject_key(&self) -> String {
        let row = match self.v.view {
            View::Map => self.view_key().unwrap_or_default(),
            View::Goal => format!("{}", self.v.goal.list.selected),
            View::Trails => format!("{}", self.v.trails.list.selected),
            View::Imports => self.view_key().unwrap_or_default(),
            View::Browse => String::new(),
        };
        format!("{:?}/{row}/{}", self.v.view, self.subject().decl_name().unwrap_or(""))
    }

    /// Called after the selection changed: remembers the scroll position of
    /// the item left behind, restores the one of the new item and asks for
    /// the declaration it needs.
    pub fn sync_view_detail(&mut self) {
        if self.v.view == View::Browse {
            return;
        }
        let key = self.subject_key();
        if key != self.v.detail.key {
            let old = std::mem::replace(&mut self.v.detail.key, key.clone());
            if self.v.detail.scroll > 0 {
                if self.v.detail.scrolls.len() > 500 {
                    self.v.detail.scrolls.clear();
                }
                self.v.detail.scrolls.insert(old, self.v.detail.scroll);
            } else {
                self.v.detail.scrolls.remove(&old);
            }
            self.v.detail.scroll = self.v.detail.scrolls.get(&key).copied().unwrap_or(0);
            self.v.detail.hscroll = 0;
        }
        self.v.detail.version += 1;
        if self.v.view == View::Imports {
            self.imports_want_dir();
        }
        let want = self.subject().decl_name().map(String::from);
        if want != self.v.detail.want {
            self.v.detail.want = want.clone();
            self.v.detail.missing = false;
            if let Some(name) = want {
                if self.v.detail.bundle_name != name {
                    let gen = self.gens.bump();
                    self.gens.vdetail = gen;
                    self.q2(QueryReq::ViewDetail { gen, name });
                }
            }
        }
        self.dirty = true;
    }

    pub fn on_view_detail(&mut self, gen: u64, name: String, res: Result<Option<Box<DetailBundle>>, String>) -> bool {
        if gen != self.gens.vdetail {
            return false;
        }
        match res {
            Ok(Some(b)) => {
                self.v.detail.bundle = Some(b);
                self.v.detail.bundle_name = name;
                self.v.detail.missing = false;
            }
            Ok(None) => {
                self.v.detail.bundle = None;
                self.v.detail.bundle_name = name;
                self.v.detail.missing = true;
            }
            Err(e) => {
                self.v.detail.bundle = None;
                self.v.detail.bundle_name.clear();
                self.warn(e);
            }
        }
        self.v.detail.version += 1;
        true
    }

    /// The detail bundle of the subject, when it has arrived.
    pub fn subject_bundle(&self) -> Option<&DetailBundle> {
        let want = self.v.detail.want.as_deref()?;
        self.v.detail.bundle.as_deref().filter(|_| self.v.detail.bundle_name == want)
    }

    // ------------------------------------------------------------------
    // Goal explorer
    // ------------------------------------------------------------------

    fn goal_filters(&self) -> (bool, bool, TypeBody) {
        let g = &self.v.goal;
        if g.reveal_hidden {
            (false, true, TypeBody::All)
        } else {
            (self.filters.local_only, self.filters.show_generated, g.tb)
        }
    }

    /// Pins a declaration as the goal (or clears it with `None`).
    pub fn set_goal(&mut self, goal: Option<GoalRef>) {
        self.cancel_connection();
        self.v.goal.conn = None;
        match &goal {
            Some(g) => {
                self.msg(format!("goal pinned: {} — A opens the goal explorer, * on it again clears", g.display));
                self.q2(QueryReq::SetPref { key: "goal".into(), value: Some(g.name.clone()) });
                self.q2(QueryReq::SetPref { key: "goal_display".into(), value: Some(g.display.clone()) });
            }
            None => {
                self.msg("goal cleared");
                self.q2(QueryReq::SetPref { key: "goal".into(), value: None });
                self.q2(QueryReq::SetPref { key: "goal_display".into(), value: None });
            }
        }
        self.v.goal.goal = goal;
        self.v.goal.restored = true;
        self.reset_goal_tree();
        self.v.detail.version += 1;
        self.center.version += 1;
        self.dirty = true;
    }

    /// `*`: pin the declaration at hand as the goal; on the goal itself, clear it.
    pub fn cmd_toggle_goal(&mut self) {
        let target: Option<GoalRef> = match self.v.view {
            View::Browse => self.detail.as_ref().map(|d| GoalRef { name: d.d.row.name.clone(), display: d.d.row.display.clone() }),
            _ => match self.subject() {
                Subject::Decl { name, .. } => {
                    let display = self.subject_bundle().map(|b| b.d.row.display.clone()).unwrap_or_else(|| name.clone());
                    Some(GoalRef { name, display })
                }
                _ => None,
            },
        };
        match target {
            Some(t) if self.v.goal.goal.as_ref().is_some_and(|g| g.name == t.name) => self.set_goal(None),
            Some(t) => self.set_goal(Some(t)),
            None if self.v.goal.goal.is_some() => self.msg("select a declaration to pin as the goal (the current goal is unchanged; * on the goal itself clears it)"),
            None => self.msg("select a declaration first: * pins it as the goal"),
        }
    }

    /// Drops the unfolded outline and loads the goal's direct relations.
    pub fn reset_goal_tree(&mut self) {
        let gen = self.gens.bump();
        self.gens.goal = gen;
        let g = &mut self.v.goal;
        g.nodes.clear();
        g.list = ListState::default();
        if let Some(goal) = g.goal.clone() {
            g.next_key += 1;
            g.nodes.push(GNode {
                key: g.next_key,
                name: goal.name,
                row: None,
                depth: 0,
                parent: None,
                labels: Vec::new(),
                note: None,
                expanded: false,
                loaded: false,
                loading: false,
                backref: false,
                repeat: false,
                children: Vec::new(),
                page: ChildPage::default(),
            });
            self.goal_expand(0);
        }
        self.rebuild_goal_rows();
    }

    fn goal_request(&mut self, node: usize, offset: usize) {
        let (local_only, show_generated, tb) = self.goal_filters();
        self.v.goal.nodes[node].loading = true;
        let (name, key) = (self.v.goal.nodes[node].name.clone(), self.v.goal.nodes[node].key);
        let q = ChildQuery { name, mode: self.v.goal.mode, dir: self.v.goal.dir, local_only, show_generated, tb, offset, limit: 50 };
        self.q2(QueryReq::GoalChildren { gen: self.gens.goal, node: key, q });
    }

    fn goal_expand(&mut self, node: usize) {
        let n = &self.v.goal.nodes[node];
        if n.backref {
            self.msg("already on the path from the goal to here (a cycle): shown as a back-reference, not unfolded again. Enter opens it.");
            return;
        }
        if n.depth >= 40 {
            self.msg("unfolding depth limit reached (40)");
            return;
        }
        self.v.goal.nodes[node].expanded = true;
        if !self.v.goal.nodes[node].loaded && !self.v.goal.nodes[node].loading {
            self.goal_request(node, 0);
        }
        self.rebuild_goal_rows();
    }

    pub fn on_goal_children(&mut self, gen: u64, key: u64, offset: usize, res: Result<ChildPage, String>) -> bool {
        if gen != self.gens.goal {
            return false;
        }
        let Some(pi) = self.v.goal.nodes.iter().position(|n| n.key == key) else { return false };
        self.v.goal.nodes[pi].loading = false;
        let page = match res {
            Ok(p) => p,
            Err(e) => {
                self.v.goal.nodes[pi].loaded = true;
                self.v.goal.nodes[pi].page.notes.push((format!("error: {e}"), true));
                self.rebuild_goal_rows();
                return true;
            }
        };
        // Path-local visited set: only the ancestors of this node.
        let mut path: HashSet<String> = HashSet::new();
        let mut cur = Some(pi);
        while let Some(c) = cur {
            path.insert(self.v.goal.nodes[c].name.clone());
            cur = self.v.goal.nodes[c].parent;
        }
        let mut seen: HashSet<String> = self.v.goal.nodes.iter().map(|n| n.name.clone()).collect();
        let depth = self.v.goal.nodes[pi].depth + 1;
        let mut children = std::mem::take(&mut self.v.goal.nodes[pi].children);
        let mut meta = page.clone();
        meta.children = Vec::new();
        for c in page.children {
            // A later page can repeat a declaration listed under another kind.
            if let Some(&ex) = children.iter().find(|&&i| self.v.goal.nodes[i].name == c.name) {
                for l in c.labels {
                    if !self.v.goal.nodes[ex].labels.contains(&l) {
                        self.v.goal.nodes[ex].labels.push(l);
                    }
                }
                continue;
            }
            self.v.goal.next_key += 1;
            let backref = path.contains(&c.name);
            let repeat = !backref && !seen.insert(c.name.clone());
            let idx = self.v.goal.nodes.len();
            self.v.goal.nodes.push(GNode {
                key: self.v.goal.next_key,
                name: c.name,
                row: c.row,
                depth,
                parent: Some(pi),
                labels: c.labels,
                note: c.note,
                expanded: false,
                loaded: false,
                loading: false,
                backref,
                repeat,
                children: Vec::new(),
                page: ChildPage::default(),
            });
            children.push(idx);
        }
        let n = &mut self.v.goal.nodes[pi];
        n.children = children;
        n.loaded = true;
        if offset == 0 {
            n.page = meta;
        } else {
            // Later pages only move the paging cursor.
            n.page.more = meta.more;
            n.page.remaining = meta.remaining;
            n.page.remaining_capped = meta.remaining_capped;
        }
        self.rebuild_goal_rows();
        self.v.detail.version += 1;
        true
    }

    pub fn rebuild_goal_rows(&mut self) {
        let mut rows: Vec<GRow> = Vec::new();
        if let Some(c) = &self.v.goal.conn {
            if c.running {
                rows.push(GRow::ConnHeader { text: format!("Connection: searching… {} (Esc cancels)", c.query.describe()), warn: false });
            } else if let Some(r) = &c.result {
                let n = r.explored;
                let (text, warn) = match &r.outcome {
                    ConnOutcome::Path(p) => (format!("Connection found: {} step(s) — {}", p.len().saturating_sub(1), r.query.describe()), false),
                    ConnOutcome::NoPath => (format!("No path in this data: all {n} reachable declarations were searched — {}", r.query.describe()), true),
                    ConnOutcome::Truncated { by_time } => (
                        format!(
                            "Query truncated after {n} declarations ({}): no conclusion either way — {}",
                            if *by_time { format!("time limit, {} ms", r.ms) } else { "work limit".into() },
                            r.query.describe()
                        ),
                        true,
                    ),
                    ConnOutcome::Unavailable(why) => (format!("Connection query unavailable: {why}"), true),
                    ConnOutcome::Cancelled => (format!("Connection query cancelled after {n} declarations: no conclusion"), true),
                    ConnOutcome::Error(e) => (format!("Connection query failed: {e}"), true),
                };
                rows.push(GRow::ConnHeader { text, warn });
                if let ConnOutcome::Path(p) = &r.outcome {
                    for i in 0..p.len() {
                        rows.push(GRow::ConnStep(i));
                    }
                    rows.push(GRow::Info("shows connectivity only: not a reading order, not necessity".into()));
                }
            }
        }
        if self.v.goal.goal.is_none() {
            rows.push(GRow::Info("No goal pinned: * pins a declaration.".into()));
            let n = self.map().map(|m| m.objectives.len()).unwrap_or(0);
            if n > 0 {
                rows.push(GRow::Info("Objectives in the map (Enter pins):".into()));
                for i in 0..n {
                    rows.push(GRow::Pick(i));
                }
            }
        } else {
            fn walk(g: &GoalView, i: usize, rows: &mut Vec<GRow>) {
                rows.push(GRow::Node(i));
                let n = &g.nodes[i];
                if !n.expanded {
                    return;
                }
                if n.loading && !n.loaded {
                    rows.push(GRow::Note { node: i, text: "loading…".into(), warn: false });
                }
                for &c in &n.children {
                    walk(g, c, rows);
                }
                if n.loaded {
                    if let Some(off) = n.page.more {
                        rows.push(GRow::More { node: i, offset: off, remaining: n.page.remaining, capped: n.page.remaining_capped });
                    }
                    let p = &n.page;
                    let mut hidden = Vec::new();
                    let cap = |x: u64| if x >= crate::relations::COUNT_CAP { format!("≥{x}") } else { x.to_string() };
                    if p.hidden_external > 0 {
                        hidden.push(format!("{} external", cap(p.hidden_external)));
                    }
                    if p.hidden_generated > 0 {
                        hidden.push(format!("{} generated", cap(p.hidden_generated)));
                    }
                    if p.hidden_kind > 0 {
                        hidden.push(format!("{} other-kind", p.hidden_kind));
                    }
                    if !hidden.is_empty() {
                        rows.push(GRow::Note { node: i, text: format!("hidden: {} (H)", hidden.join(", ")), warn: false });
                    }
                    for (t, w) in &p.notes {
                        rows.push(GRow::Note { node: i, text: t.clone(), warn: *w });
                    }
                    if n.children.is_empty() && p.notes.is_empty() && hidden.is_empty() {
                        rows.push(GRow::Note { node: i, text: "nothing recorded here for this relation kind and direction".into(), warn: false });
                    }
                }
            }
            if !self.v.goal.nodes.is_empty() {
                walk(&self.v.goal, 0, &mut rows);
            }
        }
        self.v.goal.rows = rows;
        self.v.goal.list.selected = self.v.goal.list.selected.min(self.v.goal.rows.len().saturating_sub(1));
        self.dirty = true;
    }

    fn goal_collapse(&mut self, node: usize) {
        // Descendants stay loaded in the arena (cheap); they are just not shown.
        self.v.goal.nodes[node].expanded = false;
        self.rebuild_goal_rows();
    }

    fn goal_select_node(&mut self, node: usize) {
        if let Some(i) = self.v.goal.rows.iter().position(|r| matches!(r, GRow::Node(k) if *k == node)) {
            self.v.goal.list.selected = i;
        }
    }

    fn goal_toggle(&mut self, open: Option<bool>) {
        match self.v.goal.rows.get(self.v.goal.list.selected).cloned() {
            Some(GRow::Node(i)) => {
                let exp = self.v.goal.nodes[i].expanded;
                match open {
                    Some(true) if !exp => self.goal_expand(i),
                    None if !exp => self.goal_expand(i),
                    Some(false) | None if exp => self.goal_collapse(i),
                    Some(false) => {
                        if let Some(p) = self.v.goal.nodes[i].parent {
                            self.goal_select_node(p);
                        }
                    }
                    _ => {}
                }
            }
            Some(GRow::More { node, offset, .. }) if open != Some(false) => {
                self.goal_request(node, offset);
                self.msg("loading more…");
            }
            Some(GRow::More { node, .. }) | Some(GRow::Note { node, .. }) => self.goal_select_node(node),
            _ => {}
        }
    }

    fn goal_activate(&mut self) {
        match self.v.goal.rows.get(self.v.goal.list.selected).cloned() {
            Some(GRow::Node(i)) => {
                let n = self.v.goal.nodes[i].clone();
                if n.row.is_none() && n.depth > 0 {
                    self.warn(format!("{} is not in this index: nothing to open", n.name));
                    return;
                }
                let how = match n.labels.first() {
                    Some(l) => format!("goal [{}]", l.tag()),
                    None => "goal".into(),
                };
                self.open_from_view(&n.name, &how);
            }
            Some(GRow::More { .. }) => self.goal_toggle(Some(true)),
            Some(GRow::ConnStep(i)) => {
                let name = match self.v.goal.conn.as_ref().and_then(|c| c.result.as_ref()).map(|r| &r.outcome) {
                    Some(ConnOutcome::Path(p)) => p.get(i).map(|s| s.name.clone()),
                    _ => None,
                };
                if let Some(n) = name {
                    self.open_from_view(&n, "connection");
                }
            }
            Some(GRow::Pick(i)) => {
                if let Some(o) = self.map().and_then(|m| m.objectives.get(i)).cloned() {
                    let display = self.subject_bundle().map(|b| b.d.row.display.clone()).unwrap_or_else(|| o.decl.clone());
                    self.set_goal(Some(GoalRef { name: o.decl, display }));
                    self.rebuild_goal_rows();
                }
            }
            _ => {}
        }
    }

    pub fn cancel_connection(&mut self) {
        self.v.goal.conn_latest.fetch_add(1, Ordering::Relaxed);
        if let Some(h) = self.v.goal.conn_thread.take() {
            // The search checks the token between expansions; this is brief.
            let _ = h.join();
        }
    }

    /// Starts a bounded connection search from the goal to `to`.
    pub fn start_connection(&mut self, to: String) {
        let Some(goal) = self.v.goal.goal.clone() else {
            self.msg("pin a goal first (*): a connection query runs from the goal to another declaration");
            return;
        };
        if to.trim().is_empty() {
            return;
        }
        if to == goal.name {
            self.msg("that is the goal itself");
            return;
        }
        self.cancel_connection();
        let (_, _, tb) = self.goal_filters();
        let q = ConnQuery { from: goal.name, to: to.trim().to_string(), mode: self.v.goal.mode, dir: self.v.goal.dir, tb };
        let gen = self.gens.bump();
        self.gens.conn = gen;
        self.v.goal.conn_latest.store(gen, Ordering::Relaxed);
        self.v.goal.conn = Some(ConnState { query: q.clone(), running: true, result: None });
        if let (Some(ctx), Some(tx)) = (self.ctx.index.clone(), self.ch.events.clone()) {
            self.v.goal.conn_thread = Some(spawn_connection(ctx, tx, gen, Arc::clone(&self.v.goal.conn_latest), q, Limits::default()));
        }
        if self.v.view != View::Goal {
            self.set_view(View::Goal);
        }
        self.rebuild_goal_rows();
        self.v.goal.list.selected = 0;
    }

    pub fn on_connection(&mut self, gen: u64, res: ConnResult) -> bool {
        if gen != self.gens.conn {
            return false;
        }
        if let Some(h) = self.v.goal.conn_thread.take() {
            let _ = h.join();
        }
        if let Some(c) = &mut self.v.goal.conn {
            c.running = false;
            c.result = Some(res);
        }
        self.rebuild_goal_rows();
        self.v.goal.list.selected = 0;
        self.sync_view_detail();
        true
    }

    fn goal_refilter(&mut self, what: String) {
        self.msg(what);
        self.reset_goal_tree();
        self.sync_view_detail();
    }

    // ------------------------------------------------------------------
    // Reading trails
    // ------------------------------------------------------------------

    pub fn on_reading(&mut self, r: Reading) {
        let keep = self.trail_sel_ids();
        let focus = r.focus;
        self.v.trails.reading = r;
        self.v.trails.loaded = true;
        if !self.v.goal.restored {
            self.v.goal.restored = true;
            // The reader's highlighting mode for this project, unless one
            // was given on the command line.
            if let (false, Some(m)) = (self.hl_mode_explicit, self.v.trails.reading.prefs.get("hl_mode").and_then(|m| crate::highlight::Mode::parse(m))) {
                if m != self.hl.mode {
                    self.hl.mode = m;
                    self.hl_reload();
                }
            }
            if let Some(name) = self.v.trails.reading.prefs.get("goal").cloned() {
                let display = self.v.trails.reading.prefs.get("goal_display").cloned().unwrap_or_else(|| name.clone());
                self.v.goal.goal = Some(GoalRef { name, display });
                if self.v.view == View::Goal {
                    self.reset_goal_tree();
                }
            }
        }
        if self.v.view == View::Goal && self.v.goal.goal.is_none() {
            self.rebuild_goal_rows();
        }
        if let Some((t, _)) = focus {
            self.v.trails.expanded.insert(t);
        }
        self.rebuild_trail_rows();
        let target = focus.or(keep);
        if let Some((tid, sid)) = target {
            let rd = &self.v.trails.reading;
            let pos = self.v.trails.rows.iter().position(|r| match (r, sid) {
                (TRow::Stop { trail, stop }, Some(s)) => rd.trails[*trail].id == tid && rd.trails[*trail].stops[*stop].id == s,
                (TRow::Trail(t), None) => rd.trails[*t].id == tid,
                _ => false,
            });
            let pos = pos.or_else(|| self.v.trails.rows.iter().position(|r| matches!(r, TRow::Trail(t) if rd.trails[*t].id == tid)));
            if let Some(p) = pos {
                self.v.trails.list.selected = p;
            }
        }
        self.v.detail.version += 1;
        self.center.version += 1;
        if self.v.view == View::Trails {
            self.sync_view_detail();
        }
        self.dirty = true;
    }

    fn trail_sel_ids(&self) -> Option<(i64, Option<i64>)> {
        let rd = &self.v.trails.reading;
        match self.v.trails.rows.get(self.v.trails.list.selected)? {
            TRow::Trail(t) => Some((rd.trails.get(*t)?.id, None)),
            TRow::Stop { trail, stop } => {
                let t = rd.trails.get(*trail)?;
                Some((t.id, Some(t.stops.get(*stop)?.id)))
            }
            _ => None,
        }
    }

    pub fn rebuild_trail_rows(&mut self) {
        let mut rows = Vec::new();
        let rd = &self.v.trails.reading;
        rows.push(TRow::Header(format!("Your trails ({})", rd.trails.len())));
        for (i, t) in rd.trails.iter().enumerate() {
            rows.push(TRow::Trail(i));
            if self.v.trails.expanded.contains(&t.id) {
                for s in 0..t.stops.len() {
                    rows.push(TRow::Stop { trail: i, stop: s });
                }
                if t.stops.is_empty() {
                    rows.push(TRow::Note("empty: a adds a declaration or group".into()));
                }
            }
        }
        if rd.trails.is_empty() {
            rows.push(TRow::Note(if self.v.trails.loaded { "none yet: a adds a stop, N names a trail".into() } else { "loading…".into() }));
        }
        if let Some(m) = self.map() {
            let fresh: Vec<usize> = (0..m.trails.len()).filter(|&i| !rd.trails.iter().any(|t| t.origin == format!("shared:{}", m.trails[i].id))).collect();
            if !fresh.is_empty() {
                rows.push(TRow::Header(format!("Shared trails in the project map ({})", fresh.len())));
                rows.extend(fresh.into_iter().map(TRow::Shared));
            }
        }
        if !rd.questions.is_empty() {
            let open = rd.questions.iter().filter(|q| !q.resolved).count();
            rows.push(TRow::Header(format!("Questions ({open} open, {} resolved)", rd.questions.len() - open)));
            rows.extend((0..rd.questions.len()).map(TRow::Question));
        }
        self.v.trails.rows = rows;
        let n = self.v.trails.rows.len();
        let l = &mut self.v.trails.list;
        l.selected = l.selected.min(n.saturating_sub(1));
        // Rest on something actionable rather than a header.
        if matches!(self.v.trails.rows.get(l.selected), Some(TRow::Header(_))) && l.selected + 1 < n {
            l.selected += 1;
        }
        self.dirty = true;
    }

    fn selected_stop(&self) -> Option<Stop> {
        match self.v.trails.rows.get(self.v.trails.list.selected)? {
            TRow::Stop { trail, stop } => self.v.trails.reading.trails.get(*trail)?.stops.get(*stop).cloned(),
            _ => None,
        }
    }

    fn selected_trail(&self) -> Option<usize> {
        match self.v.trails.rows.get(self.v.trails.list.selected)? {
            TRow::Trail(t) => Some(*t),
            TRow::Stop { trail, .. } => Some(*trail),
            _ => None,
        }
    }

    fn trails_toggle(&mut self, open: Option<bool>) {
        match self.v.trails.rows.get(self.v.trails.list.selected).cloned() {
            Some(TRow::Trail(t)) => {
                let id = self.v.trails.reading.trails[t].id;
                let is = self.v.trails.expanded.contains(&id);
                if open.unwrap_or(!is) {
                    self.v.trails.expanded.insert(id);
                } else {
                    self.v.trails.expanded.remove(&id);
                }
                self.rebuild_trail_rows();
            }
            Some(TRow::Stop { trail, .. }) if open == Some(false) => {
                if let Some(i) = self.v.trails.rows.iter().position(|r| matches!(r, TRow::Trail(t) if *t == trail)) {
                    self.v.trails.list.selected = i;
                }
            }
            _ => {}
        }
    }

    fn trails_activate(&mut self) {
        match self.v.trails.rows.get(self.v.trails.list.selected).cloned() {
            Some(TRow::Trail(_)) => self.trails_toggle(None),
            Some(TRow::Stop { trail, stop }) => {
                let s = self.v.trails.reading.trails[trail].stops[stop].clone();
                match (s.ref_kind, self.v.trails.reading.statuses.get(&s.id)) {
                    (_, Some(StopStatus::Unresolved(why))) => self.warn(format!("{}: {why}. The stop and its notes are kept.", s.name)),
                    (RefKind::Decl, _) => self.open_from_view(&s.name, "trail"),
                    (RefKind::Group, _) => {
                        self.set_view(View::Map);
                        self.map_reveal(&format!("grp:{}", s.name));
                    }
                }
            }
            Some(TRow::Shared(i)) => {
                if let Some(id) = self.map().and_then(|m| m.trails.get(i)).map(|t| t.id.clone()) {
                    self.q2(QueryReq::StartShared { id });
                }
            }
            Some(TRow::Question(i)) => {
                if let Some(q) = self.v.trails.reading.questions.get(i).cloned() {
                    if q.ref_kind == RefKind::Decl {
                        self.open_from_view(&q.name, "question");
                    }
                }
            }
            _ => {}
        }
    }

    /// The declaration or group at hand, as a trail stop.
    fn stop_at_hand(&self) -> Option<Stop> {
        let from_bundle = |b: &DetailBundle| Stop {
            ref_kind: RefKind::Decl,
            name: b.d.row.name.clone(),
            display: b.d.row.display.clone(),
            kind: b.d.row.kind.as_str().into(),
            module: b.d.row.module.clone(),
            path: b.d.module_path.clone(),
            line: b.d.pos.start_line,
            sha256: b.d.module_sha256.clone(),
            ..Default::default()
        };
        match self.v.view {
            View::Browse | View::Trails => self.detail.as_deref().map(from_bundle),
            _ => match self.subject() {
                Subject::Group(k) => self.map().map(|m| Stop { ref_kind: RefKind::Group, name: m.groups[k].id.clone(), display: m.groups[k].title.clone(), kind: "group".into(), ..Default::default() }),
                Subject::Decl { .. } => self.subject_bundle().map(from_bundle),
                _ => None,
            },
        }
    }

    /// `a`: add what is at hand to the active trail, with an optional note.
    pub fn cmd_add_to_trail(&mut self) {
        match self.stop_at_hand() {
            Some(stop) => {
                let to = match self.v.trails.active().and_then(|id| self.v.trails.reading.trails.iter().find(|t| t.id == id)) {
                    Some(t) => format!("“{}”", t.title),
                    None => "your reading trail".into(),
                };
                let prompt = format!("Add {} to {to} — why it is on the route (optional; Enter adds, Esc cancels):", stop.display);
                self.overlay = Overlay::Input { purpose: InputPurpose::StopAdd { stop: Box::new(stop) }, prompt, text: String::new() };
            }
            None => self.msg("nothing to add here: select a declaration or a map group (if it is still loading, try again)"),
        }
    }

    /// `Q`: ask a question about what is at hand (on a trail stop: the stop's question).
    pub fn cmd_question(&mut self) {
        if self.v.view == View::Trails {
            if let Some(s) = self.selected_stop() {
                self.overlay = Overlay::Input {
                    purpose: InputPurpose::StopQuestion { id: s.id },
                    prompt: format!("Question about {} at this stop (empty clears):", s.display),
                    text: s.question.unwrap_or_default(),
                };
                return;
            }
        }
        match self.stop_at_hand() {
            Some(s) => {
                self.overlay = Overlay::Input {
                    purpose: InputPurpose::AskQuestion { ref_kind: s.ref_kind, name: s.name, display: s.display.clone() },
                    prompt: format!("Your question about {} (kept in your data; T lists open questions):", s.display),
                    text: String::new(),
                };
            }
            None => self.msg("select a declaration or a map group to attach a question to"),
        }
    }

    pub fn cmd_export_trail(&mut self) {
        let rd = &self.v.trails.reading;
        let t = match self.v.view {
            View::Trails => self.selected_trail().and_then(|i| rd.trails.get(i)),
            _ => None,
        }
        .or_else(|| self.v.trails.active().and_then(|id| rd.trails.iter().find(|t| t.id == id)))
        .or(rd.trails.first());
        let Some(t) = t else {
            self.msg("no trail to export yet: press a on a declaration to start one");
            return;
        };
        let slug: String = t.title.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
        let default = std::env::current_dir().unwrap_or_default().join(format!("trail-{}.md", slug.trim_matches('-')));
        self.overlay = Overlay::Input {
            purpose: InputPurpose::ExportTrailTo { trail: t.id },
            prompt: format!("Export the trail “{}” as Markdown to file:", t.title),
            text: default.to_string_lossy().into(),
        };
    }

    fn trails_key(&mut self, k: KeyEvent) -> bool {
        let stop = self.selected_stop();
        let trail = self.selected_trail().map(|i| self.v.trails.reading.trails[i].clone());
        let question = match self.v.trails.rows.get(self.v.trails.list.selected) {
            Some(TRow::Question(i)) => self.v.trails.reading.questions.get(*i).cloned(),
            _ => None,
        };
        match k.code {
            KeyCode::Char('s') => match (&stop, &question) {
                (Some(s), _) => self.q2(QueryReq::StopSetState { id: s.id, state: s.state.next() }),
                (_, Some(q)) => self.q2(QueryReq::QuestionToggle { id: q.id }),
                _ => self.msg("s cycles the reading state of a stop (unread, read, understood, revisit) or resolves a question"),
            },
            KeyCode::Char('n') => match &stop {
                Some(s) => {
                    self.overlay = Overlay::Input {
                        purpose: InputPurpose::StopNote { id: s.id },
                        prompt: format!("Explanation for the stop {} (empty clears):", s.display),
                        text: s.note.clone().unwrap_or_default(),
                    }
                }
                None => self.msg("n edits the explanation of a stop: select a stop"),
            },
            KeyCode::Char('d') => match (&stop, &question, &trail) {
                (Some(s), _, _) => self.q2(QueryReq::StopRemove { id: s.id }),
                (_, Some(q), _) => self.q2(QueryReq::QuestionRemove { id: q.id }),
                (_, _, Some(t)) => {
                    self.overlay = Overlay::Input {
                        purpose: InputPurpose::DeleteTrail { id: t.id },
                        prompt: format!("Delete the trail “{}” and its {} stops? Type yes:", t.title, t.stops.len()),
                        text: String::new(),
                    }
                }
                _ => self.msg("d removes the selected stop, question or trail"),
            },
            KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Char('-') => match &stop {
                Some(s) => self.q2(QueryReq::StopMove { id: s.id, delta: if k.code == KeyCode::Char('-') { -1 } else { 1 } }),
                None => self.msg("- and + move a stop up and down within its trail: select a stop"),
            },
            KeyCode::Char('N') => {
                self.overlay = Overlay::Input { purpose: InputPurpose::NewTrail, prompt: "Title of the new trail:".into(), text: String::new() };
            }
            KeyCode::Char('e') => match &trail {
                Some(t) => self.overlay = Overlay::Input { purpose: InputPurpose::RenameTrail { id: t.id }, prompt: "New title of the trail:".into(), text: t.title.clone() },
                None => self.msg("e renames a trail: select one"),
            },
            KeyCode::Char('t') => match &trail {
                Some(t) => {
                    self.q2(QueryReq::SetPref { key: "active_trail".into(), value: Some(t.id.to_string()) });
                    self.msg(format!("“{}” is now the active trail: a adds stops to it", t.title));
                }
                None => self.msg("t makes a trail the target of a: select a trail"),
            },
            KeyCode::Char('r') => match &trail {
                Some(t) => match t.resume_at() {
                    Some(s) => {
                        self.v.trails.expanded.insert(t.id);
                        self.rebuild_trail_rows();
                        let rd = &self.v.trails.reading;
                        if let Some(p) = self.v.trails.rows.iter().position(|r| matches!(r, TRow::Stop { trail, stop } if rd.trails[*trail].id == t.id && *stop == s)) {
                            self.v.trails.list.selected = p;
                        }
                        self.msg(format!("resuming at stop {} of {}: the first one not yet read", s + 1, t.stops.len()));
                    }
                    None if t.stops.is_empty() => self.msg("this trail has no stops yet"),
                    None => self.msg("every stop of this trail is marked read or understood"),
                },
                None => self.msg("r resumes a trail at its first unread stop: select a trail"),
            },
            _ => return false,
        }
        true
    }

    // ------------------------------------------------------------------
    // Import layers
    // ------------------------------------------------------------------

    /// `I`: files by import depth, endpoints and hubs. From a declaration
    /// or a selected file it shows where that file sits.
    pub fn cmd_imports(&mut self) {
        let at = match self.v.view {
            View::Browse => self.detail.as_ref().and_then(|d| d.d.row.module.clone()),
            View::Imports => None,
            _ => self.view_files_at_hand().or_else(|| self.subject_bundle().and_then(|d| d.d.row.module.clone())),
        };
        self.set_view(View::Imports);
        if let Some(m) = at {
            self.imports_reveal(&m);
        }
    }

    pub(super) fn request_imports(&mut self) {
        if self.v.imports.requested {
            return;
        }
        self.v.imports.requested = true;
        self.v.imports.error = None;
        let gen = self.gens.bump();
        self.gens.imports = gen;
        self.q2(QueryReq::Imports { gen });
    }

    /// Forgets the import graph (the index was replaced).
    pub fn reset_imports(&mut self) {
        let expanded = std::mem::take(&mut self.v.imports.expanded);
        let key = self.v.imports.rows.get(self.v.imports.list.selected).map(|r| r.key.clone());
        self.v.imports = ImportsView { expanded, pending_key: key, ..Default::default() };
        if self.v.view == View::Imports {
            self.request_imports();
            self.rebuild_import_rows();
        }
    }

    pub fn on_imports(&mut self, gen: u64, res: Result<Arc<ImportGraph>, String>) -> bool {
        if gen != self.gens.imports {
            return false;
        }
        match res {
            Ok(g) => {
                super::timing::record("import graph: read from the index", g.load_ms);
                // One root is opened to its first level, like the outline.
                if let [r] = g.roots[..] {
                    self.v.imports.expanded.insert(format!("il:{}", g.dirs[r as usize].full));
                }
                self.v.imports.graph = Some(g);
                self.v.imports.error = None;
            }
            Err(e) => {
                self.v.imports.requested = false;
                self.v.imports.error = Some(e);
            }
        }
        self.rebuild_import_rows();
        if let Some(k) = self.v.imports.pending_key.take() {
            match k.strip_prefix("il:") {
                Some(full) => self.imports_reveal(&full.to_string()),
                None => {
                    self.imports_select_key(&k);
                }
            }
        }
        self.v.detail.version += 1;
        self.sync_view_detail();
        true
    }

    pub fn rebuild_import_rows(&mut self) {
        let keep = self.v.imports.rows.get(self.v.imports.list.selected).map(|r| r.key.clone());
        let mut rows: Vec<IRow> = Vec::new();
        let iv = &self.v.imports;
        let exp = |k: &str| iv.expanded.contains(k);
        let sec = |rows: &mut Vec<IRow>, key: &str, title: &str, count: String| -> bool {
            let e = exp(key);
            rows.push(IRow { key: key.into(), depth: 0, kind: IRowKind::Section { title: title.into(), count, expanded: e } });
            e
        };
        let note = |rows: &mut Vec<IRow>, key: String, text: String, warn: bool| rows.push(IRow { key, depth: 1, kind: IRowKind::Note { text, warn } });
        match (&iv.graph, &iv.error) {
            (None, Some(e)) => note(&mut rows, "note:error".into(), format!("the import graph could not be read: {e}"), true),
            (None, None) => note(&mut rows, "note:loading".into(), "reading the import graph…".into(), false),
            (Some(g), _) => {
                if iv.path.is_some() || iv.path_running {
                    let n = match &iv.path {
                        Some(PathAnswer::Path(p)) => format!("{} step{}", p.len().saturating_sub(1), if p.len() == 2 { "" } else { "s" }),
                        Some(PathAnswer::NoPath { .. }) => "no path".into(),
                        Some(PathAnswer::Unknown { .. }) => "module not found".into(),
                        None => "searching…".into(),
                    };
                    if sec(&mut rows, "sec:path", "Import path", n) {
                        match &iv.path {
                            Some(PathAnswer::Path(p)) => {
                                for (i, name) in p.iter().enumerate() {
                                    rows.push(IRow { key: format!("path:{i}"), depth: 1, kind: IRowKind::PathStep { name: name.clone(), step: i } });
                                }
                            }
                            Some(PathAnswer::NoPath { from, to, reached }) => note(
                                &mut rows,
                                "note:nopath".into(),
                                format!("{from} does not import {to}, directly or through other files (all {reached} modules it reaches were checked)"),
                                false,
                            ),
                            Some(PathAnswer::Unknown { text, examples, count }) => note(
                                &mut rows,
                                "note:unknown".into(),
                                if *count == 0 { format!("no module is named {text:?} or ends with it") } else { format!("{count} modules end with {text:?}: {} — give more of the name", examples.join(", ")) },
                                true,
                            ),
                            None => {}
                        }
                    }
                }
                if sec(&mut rows, "sec:layers", "Layers by import depth", format!("{} files, depth 0–{}", g.local_count, g.max_depth)) {
                    fn walk(g: &ImportGraph, iv: &ImportsView, d: u32, depth: u16, rows: &mut Vec<IRow>) {
                        let n = &g.dirs[d as usize];
                        let key = format!("il:{}", n.full);
                        let e = iv.expanded.contains(&key) && !n.children.is_empty();
                        rows.push(IRow { key, depth, kind: IRowKind::Dir { dir: d, expanded: e } });
                        if e {
                            for &c in &n.children {
                                walk(g, iv, c, depth + 1, rows);
                            }
                        }
                    }
                    for &r in &g.roots {
                        walk(g, iv, r, 1, &mut rows);
                    }
                    if g.roots.is_empty() {
                        note(&mut rows, "note:nofiles".into(), "this index has no local files".into(), false);
                    }
                }
                let list = |rows: &mut Vec<IRow>, prefix: &str, what: IList, nodes: &[u32]| {
                    for &v in nodes.iter().take(IMPORT_LIST_CAP) {
                        rows.push(IRow { key: format!("{prefix}:{}", g.nodes[v as usize].name), depth: 1, kind: IRowKind::Module { node: v, what } });
                    }
                    if nodes.len() > IMPORT_LIST_CAP {
                        rows.push(IRow { key: format!("note:{prefix}more"), depth: 1, kind: IRowKind::Note { text: format!("… {} more (the layer tree lists every file)", nodes.len() - IMPORT_LIST_CAP), warn: false } });
                    }
                };
                if sec(&mut rows, "sec:endpoints", "Endpoints", format!("{} files", g.endpoints.len())) {
                    list(&mut rows, "ep", IList::Endpoint, &g.endpoints);
                }
                if sec(&mut rows, "sec:hubs", "Hubs", format!("{} most imported", g.hubs.len())) {
                    list(&mut rows, "hub", IList::Hub, &g.hubs);
                }
                if !g.import_only.is_empty() && sec(&mut rows, "sec:importonly", "Import-only files", g.import_only.len().to_string()) {
                    list(&mut rows, "io", IList::ImportOnly, &g.import_only);
                }
                if g.cyclic_count > 0 {
                    note(&mut rows, "note:cycles".into(), format!("{} files are on or above an import cycle: their depth is a lower bound", g.cyclic_count), true);
                }
            }
        }
        let old = self.v.imports.list.selected;
        self.v.imports.rows = rows;
        if let Some(k) = keep {
            if let Some(i) = self.v.imports.rows.iter().position(|r| r.key == k) {
                let delta = i as isize - old as isize;
                self.v.imports.list.selected = i;
                self.v.imports.list.scroll = (self.v.imports.list.scroll as isize + delta).max(0) as usize;
            }
        }
        self.v.imports.list.selected = self.v.imports.list.selected.min(self.v.imports.rows.len().saturating_sub(1));
        self.dirty = true;
    }

    fn imports_select_key(&mut self, key: &str) -> bool {
        match self.v.imports.rows.iter().position(|r| r.key == key) {
            Some(i) => {
                self.v.imports.list.selected = i;
                self.v.imports.list.scroll = i.saturating_sub(self.v.list_height / 3);
                true
            }
            None => false,
        }
    }

    /// Shows a file or directory in the layer tree, opening what contains it.
    pub fn imports_reveal(&mut self, full: &String) {
        let Some(g) = self.v.imports.graph.clone() else {
            self.v.imports.pending_key = Some(format!("il:{full}"));
            return;
        };
        let Some(mut d) = g.dir(full) else {
            self.msg(format!("{full} is not a file of this project (an external module): the layer tree lists local files"));
            return;
        };
        self.v.imports.expanded.insert("sec:layers".into());
        while let Some(p) = g.dirs[d as usize].parent {
            self.v.imports.expanded.insert(format!("il:{}", g.dirs[p as usize].full));
            d = p;
        }
        self.rebuild_import_rows();
        self.imports_select_key(&format!("il:{full}"));
        self.sync_view_detail();
    }

    /// The module of the selected row of the import view, if it is one.
    fn imports_selected_module(&self) -> Option<String> {
        let g = self.v.imports.graph.as_ref()?;
        match &self.v.imports.rows.get(self.v.imports.list.selected)?.kind {
            IRowKind::Dir { dir, .. } => g.dirs[*dir as usize].module.map(|m| g.nodes[m as usize].name.clone()),
            IRowKind::Module { node, .. } => Some(g.nodes[*node as usize].name.clone()),
            IRowKind::PathStep { name, .. } => Some(name.clone()),
            _ => None,
        }
    }

    /// Asks the worker what the selected directory imports and what imports it.
    fn imports_want_dir(&mut self) {
        let want = match (self.v.imports.graph.as_ref(), self.v.imports.rows.get(self.v.imports.list.selected).map(|r| &r.kind)) {
            (Some(g), Some(IRowKind::Dir { dir, .. })) if !g.dirs[*dir as usize].children.is_empty() => Some(g.dirs[*dir as usize].full.clone()),
            _ => None,
        };
        if want == self.v.imports.dir_want {
            return;
        }
        self.v.imports.dir_want = want.clone();
        if let Some(full) = want {
            if self.v.imports.dir.as_ref().is_none_or(|d| d.full != full) {
                let gen = self.gens.bump();
                self.gens.idir = gen;
                self.q2(QueryReq::ImportsDir { gen, full });
            }
        }
    }

    pub fn on_imports_dir(&mut self, gen: u64, res: Result<DirSummary, String>) -> bool {
        if gen != self.gens.idir {
            return false;
        }
        match res {
            Ok(d) => self.v.imports.dir = Some(d),
            Err(e) => self.warn(e),
        }
        self.v.detail.version += 1;
        true
    }

    fn imports_toggle(&mut self, open: Option<bool>) {
        let Some(row) = self.v.imports.rows.get(self.v.imports.list.selected).cloned() else { return };
        let expandable = match &row.kind {
            IRowKind::Section { .. } => true,
            IRowKind::Dir { dir, .. } => self.v.imports.graph.as_ref().is_some_and(|g| !g.dirs[*dir as usize].children.is_empty()),
            _ => false,
        };
        let is_open = self.v.imports.expanded.contains(&row.key);
        let want = open.unwrap_or(!is_open);
        if !expandable || want == is_open {
            if open == Some(false) || (!expandable && open.is_none()) {
                let sel = self.v.imports.list.selected;
                if let Some(j) = (0..sel).rev().find(|&j| self.v.imports.rows[j].depth < row.depth) {
                    self.v.imports.list.selected = j;
                }
            }
            return;
        }
        if want {
            self.v.imports.expanded.insert(row.key.clone());
        } else {
            self.v.imports.expanded.remove(&row.key);
        }
        self.rebuild_import_rows();
    }

    fn imports_activate(&mut self) {
        let Some(row) = self.v.imports.rows.get(self.v.imports.list.selected).cloned() else { return };
        match row.kind {
            IRowKind::Section { .. } => self.imports_toggle(None),
            IRowKind::Dir { dir, .. } => {
                let is_file = self.v.imports.graph.as_ref().is_some_and(|g| g.dirs[dir as usize].module.is_some() && g.dirs[dir as usize].children.is_empty());
                if is_file {
                    self.imports_open_module();
                } else {
                    self.imports_toggle(None);
                }
            }
            IRowKind::Module { .. } | IRowKind::PathStep { .. } => self.imports_open_module(),
            IRowKind::Note { .. } => {}
        }
    }

    /// Shows the selected file in the declaration browser's outline.
    fn imports_open_module(&mut self) {
        let Some(name) = self.imports_selected_module() else { return };
        let local = self.v.imports.graph.as_ref().and_then(|g| g.node(&name).map(|n| g.nodes[n as usize].local)).unwrap_or(false);
        if !local {
            self.msg(format!("{name} is an external module: its files are not part of this project's outline"));
            return;
        }
        let loc = self.location();
        self.history.back.push(loc);
        self.history.fwd.clear();
        self.v.view = View::Browse;
        self.reveal_in_outline(name, None);
    }

    fn imports_key(&mut self, k: KeyEvent) -> bool {
        match k.code {
            KeyCode::Char('p') => {
                self.cmd_import_path();
                true
            }
            _ => false,
        }
    }

    /// `p`: ask which module to find an import path to.
    pub fn cmd_import_path(&mut self) {
        let from = match self.v.view {
            View::Imports => self.imports_selected_module(),
            View::Browse => self.detail.as_ref().and_then(|d| d.d.row.module.clone()),
            _ => self.subject_bundle().and_then(|d| d.d.row.module.clone()),
        };
        let Some(from) = from else {
            self.msg("select a file first: an import path runs from a file to a module it imports, directly or through others");
            return;
        };
        self.overlay = Overlay::Input {
            purpose: InputPurpose::ImportPathTo { from: from.clone() },
            prompt: format!("Why does {from} import… which module? (a module name, a File.lean path, or the end of a name)"),
            text: String::new(),
        };
    }

    pub fn start_import_path(&mut self, from: String, to: String) {
        if to.is_empty() {
            self.msg("no import path searched (the module name was empty)");
            return;
        }
        if self.v.view != View::Imports {
            self.set_view(View::Imports);
        }
        let gen = self.gens.bump();
        self.gens.ipath = gen;
        self.v.imports.path = None;
        self.v.imports.path_running = true;
        self.v.imports.expanded.insert("sec:path".into());
        self.q2(QueryReq::ImportPath { gen, from, to });
        self.rebuild_import_rows();
    }

    pub fn on_import_path(&mut self, gen: u64, res: Result<PathAnswer, String>) -> bool {
        if gen != self.gens.ipath {
            return false;
        }
        self.v.imports.path_running = false;
        match res {
            Ok(a) => {
                match &a {
                    PathAnswer::Path(p) if p.len() <= 1 => self.msg(format!("{} is the module itself: nothing to explain", p.first().cloned().unwrap_or_default())),
                    PathAnswer::Path(p) => self.msg(format!("import path: {} import line(s) from {} to {}", p.len() - 1, p[0], p[p.len() - 1])),
                    PathAnswer::NoPath { from, to, .. } => self.msg(format!("{from} does not import {to}, even through other files")),
                    PathAnswer::Unknown { text, .. } => self.warn(format!("no single module matches {text:?}")),
                }
                self.v.imports.path = Some(a);
            }
            Err(e) => self.warn(e),
        }
        self.rebuild_import_rows();
        self.imports_select_key("sec:path");
        self.sync_view_detail();
        true
    }

    // ------------------------------------------------------------------
    // What `S` and `f` act on in the map, goal, trail and import views
    // ------------------------------------------------------------------

    /// The file or directory selected in a view, as a module-name prefix.
    pub fn view_files_at_hand(&self) -> Option<String> {
        match self.v.view {
            View::Map => match self.v.map.rows.get(self.v.map.list.selected).map(|r| &r.kind) {
                Some(MapRowKind::Module { node, .. }) => self.outline.nodes.get(*node).map(|n| n.full.clone()),
                _ => None,
            },
            View::Imports => {
                let g = self.v.imports.graph.as_ref()?;
                match &self.v.imports.rows.get(self.v.imports.list.selected)?.kind {
                    IRowKind::Dir { dir, .. } => Some(g.dirs[*dir as usize].full.clone()),
                    IRowKind::Module { node, .. } => Some(g.nodes[*node as usize].name.clone()).filter(|_| g.nodes[*node as usize].local),
                    IRowKind::PathStep { name, .. } => g.node(name).filter(|&n| g.nodes[n as usize].local).map(|_| name.clone()),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// What `S` skims from a view: the selected map group, file or
    /// directory, else the file of the selected declaration.
    pub fn view_skim_at_hand(&self) -> Option<SkimScope> {
        if let Some(Subject::Group(k)) = Some(self.subject()).filter(|_| self.v.view == View::Map) {
            return self.map().and_then(|m| m.groups.get(k).map(|g| SkimScope::Group(g.id.clone())));
        }
        if let Some(prefix) = self.view_files_at_hand() {
            let is_dir = match self.v.view {
                View::Map => self.outline.by_name.get(&prefix).is_some_and(|&i| !self.outline.nodes[i].children.is_empty()),
                _ => self.v.imports.graph.as_ref().and_then(|g| g.dir(&prefix).map(|d| !g.dirs[d as usize].children.is_empty())).unwrap_or(false),
            };
            return Some(if is_dir { SkimScope::Dir(prefix) } else { SkimScope::Module(prefix) });
        }
        if let Some(b) = self.subject_bundle() {
            return b.d.row.module.clone().map(SkimScope::Module);
        }
        // A landmark whose detail has not arrived yet: the map knows its module.
        let name = self.subject().decl_name().map(String::from)?;
        self.v.map.bundle.as_ref()?.resolution.row(&name)?.module.clone().map(SkimScope::Module)
    }

    // ------------------------------------------------------------------
    // Input shared by the three views
    // ------------------------------------------------------------------

    fn view_list(&mut self) -> (&mut ListState, usize) {
        match self.v.view {
            View::Map => {
                let n = self.v.map.rows.len();
                (&mut self.v.map.list, n)
            }
            View::Goal => {
                let n = self.v.goal.rows.len();
                (&mut self.v.goal.list, n)
            }
            View::Imports => {
                let n = self.v.imports.rows.len();
                (&mut self.v.imports.list, n)
            }
            _ => {
                let n = self.v.trails.rows.len();
                (&mut self.v.trails.list, n)
            }
        }
    }

    pub fn view_move(&mut self, delta: isize) {
        if self.v.focus == VFocus::Detail {
            let len = self.v.detail.cache.as_ref().map(|c| c.2.len()).unwrap_or(0);
            let max = len.saturating_sub(self.v.detail.height.max(1));
            self.v.detail.scroll = (self.v.detail.scroll as isize + delta).clamp(0, max as isize) as usize;
            self.dirty = true;
            return;
        }
        let (l, n) = self.view_list();
        l.selected = clamp_sel(l.selected, delta, n);
        self.sync_view_detail();
    }

    /// Click in the outline of a view: select, or activate the selected row.
    pub fn view_click(&mut self, row: usize) {
        self.v.focus = VFocus::Outline;
        let (l, n) = self.view_list();
        let idx = l.scroll + row;
        if idx >= n {
            return;
        }
        if l.selected == idx {
            self.view_activate();
        } else {
            l.selected = idx;
            self.sync_view_detail();
        }
    }

    fn view_activate(&mut self) {
        match self.v.view {
            View::Map => self.map_activate(),
            View::Goal => self.goal_activate(),
            View::Trails => self.trails_activate(),
            View::Imports => self.imports_activate(),
            View::Browse => {}
        }
        self.sync_view_detail();
    }

    fn view_toggle(&mut self, open: Option<bool>) {
        match self.v.view {
            View::Map => self.map_toggle(open),
            View::Goal => self.goal_toggle(open),
            View::Trails => self.trails_toggle(open),
            View::Imports => self.imports_toggle(open),
            View::Browse => {}
        }
        self.sync_view_detail();
    }

    /// Keys of the map, goal and trail views. Returns false for keys that
    /// the global bindings should handle.
    pub fn views_key(&mut self, k: KeyEvent) -> bool {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let page = self.v.list_height.saturating_sub(2).max(3) as isize;
        match k.code {
            KeyCode::Down | KeyCode::Char('j') => self.view_move(1),
            KeyCode::Up | KeyCode::Char('k') => self.view_move(-1),
            KeyCode::PageDown => self.view_move(page),
            KeyCode::PageUp => self.view_move(-page),
            KeyCode::Char('d') if ctrl => self.view_move(page / 2),
            KeyCode::Char('u') if ctrl => self.view_move(-page / 2),
            KeyCode::Home => self.view_move(-1_000_000),
            KeyCode::End => self.view_move(1_000_000),
            KeyCode::Tab | KeyCode::BackTab => {
                self.v.focus = if self.v.focus == VFocus::Outline { VFocus::Detail } else { VFocus::Outline };
            }
            KeyCode::Char('w') => {
                self.v.detail.wrap = !self.v.detail.wrap;
                self.v.detail.hscroll = 0;
                self.v.detail.cache = None;
                self.msg(if self.v.detail.wrap { "detail: long lines wrap" } else { "detail: long lines are not wrapped — < and > scroll sideways" });
            }
            KeyCode::Char('<') | KeyCode::Char('>') | KeyCode::Char(',') | KeyCode::Char('.') => {
                if self.v.detail.wrap {
                    self.msg("sideways scrolling applies when wrapping is off (w)");
                } else if matches!(k.code, KeyCode::Char('<') | KeyCode::Char(',')) {
                    self.v.detail.hscroll = self.v.detail.hscroll.saturating_sub(8);
                } else {
                    self.v.detail.hscroll = (self.v.detail.hscroll + 8).min(2000);
                }
            }
            KeyCode::Esc => {
                if self.v.goal.conn.as_ref().is_some_and(|c| c.running) {
                    self.cancel_connection();
                    if let Some(c) = &mut self.v.goal.conn {
                        c.running = false;
                        let q = c.query.clone();
                        c.result = Some(ConnResult { query: q, outcome: ConnOutcome::Cancelled, explored: 0, ms: 0 });
                    }
                    self.rebuild_goal_rows();
                } else if self.v.focus == VFocus::Detail {
                    self.v.focus = VFocus::Outline;
                } else {
                    self.set_view(View::Browse);
                }
            }
            _ if self.v.focus == VFocus::Detail && matches!(k.code, KeyCode::Char(' ') | KeyCode::Enter | KeyCode::Left | KeyCode::Right | KeyCode::Char('h') | KeyCode::Char('l')) => {
                match k.code {
                    KeyCode::Char(' ') => self.view_move(page),
                    KeyCode::Enter => {
                        // The detail describes the selected row: Enter opens it.
                        self.v.focus = VFocus::Outline;
                        self.view_activate();
                    }
                    KeyCode::Left | KeyCode::Char('h') => self.v.focus = VFocus::Outline,
                    _ => {}
                }
            }
            KeyCode::Char(' ') => self.view_toggle(None),
            KeyCode::Right | KeyCode::Char('l') => self.view_toggle(Some(true)),
            KeyCode::Left | KeyCode::Char('h') => self.view_toggle(Some(false)),
            KeyCode::Enter => self.view_activate(),
            _ => {
                let handled = match self.v.view {
                    View::Map => self.map_key(k),
                    View::Goal => self.goal_key(k),
                    View::Trails => self.trails_key(k),
                    View::Imports => self.imports_key(k),
                    View::Browse => false,
                };
                if handled {
                    self.sync_view_detail();
                    self.dirty = true;
                    return true;
                }
                // Keys of the declaration browser do nothing useful here: say so.
                if matches!(k.code, KeyCode::Char('1'..='5') | KeyCode::Char('x') | KeyCode::Char('i') | KeyCode::Char('s') | KeyCode::Char('e') | KeyCode::Char('t') | KeyCode::Char('K') | KeyCode::Char('n')) {
                    self.msg("that key belongs to the declaration browser: Enter opens the selected declaration there (? lists this view's keys)");
                    return true;
                }
                return false;
            }
        }
        self.dirty = true;
        true
    }

    fn map_key(&mut self, k: KeyEvent) -> bool {
        match k.code {
            KeyCode::Char('r') => {
                self.v.map.reload_requested = true;
                self.q2(QueryReq::ReloadMap);
                self.msg("reloading lean-view.toml…");
                true
            }
            _ => false,
        }
    }

    fn goal_key(&mut self, k: KeyEvent) -> bool {
        match k.code {
            KeyCode::Char('r') => {
                self.v.goal.mode = self.v.goal.mode.next();
                let m = self.v.goal.mode;
                let mut s = format!("relation kind: {} — {}", m.label(), m.explain());
                if m == RelMode::Lean && self.ctx.mode == IndexMode::Source {
                    s = "relation kind: Lean dependencies — UNAVAILABLE in a source-only index (nothing was elaborated). r moves on; a semantic snapshot provides them.".into();
                }
                self.goal_refilter(s);
            }
            KeyCode::Char('u') => {
                self.v.goal.dir = if self.v.goal.dir == Direction::Uses { Direction::UsedBy } else { Direction::Uses };
                let s = if self.v.goal.dir == Direction::Uses {
                    "direction: uses — from a statement to its prerequisites (A uses B: unfolding A lists B)"
                } else {
                    "direction: used by — from a statement to what depends on it (the reverse of uses)"
                };
                self.goal_refilter(s.into());
            }
            KeyCode::Char('y') => {
                self.v.goal.tb = self.v.goal.tb.next();
                self.v.goal.reveal_hidden = false;
                let s = format!("Lean edges: {}", self.v.goal.tb.label());
                self.goal_refilter(s);
            }
            KeyCode::Char('H') => {
                self.v.goal.reveal_hidden = !self.v.goal.reveal_hidden;
                let s = if self.v.goal.reveal_hidden {
                    "showing everything: external, Lean-generated and all edge kinds (H hides again)"
                } else {
                    "display filters applied again (hidden counts are shown under each node)"
                };
                self.goal_refilter(s.into());
            }
            KeyCode::Char('L') => {
                self.filters.local_only = !self.filters.local_only;
                let s = if self.filters.local_only { "filter: local declarations only (hidden externals are counted)" } else { "filter: local and library declarations" };
                self.goal_refilter(s.into());
            }
            KeyCode::Char('G') => {
                self.filters.show_generated = !self.filters.show_generated;
                let s = if self.filters.show_generated { "showing Lean-generated declarations" } else { "hiding Lean-generated declarations (counted under each node)" };
                self.goal_refilter(s.into());
            }
            KeyCode::Char('c') => self.cmd_connect(),
            _ => return false,
        }
        true
    }

    /// `c`: ask for the other end of a connection query.
    pub fn cmd_connect(&mut self) {
        let Some(goal) = self.v.goal.goal.clone() else {
            self.msg("pin a goal first (*): a connection query runs from the goal to another declaration");
            return;
        };
        let default = match self.v.view {
            View::Browse => self.detail.as_ref().map(|d| d.d.row.name.clone()),
            View::Goal => self.detail.as_ref().map(|d| d.d.row.name.clone()),
            _ => self.subject().decl_name().map(String::from),
        }
        .filter(|n| *n != goal.name)
        .unwrap_or_default();
        self.overlay = Overlay::Input {
            purpose: InputPurpose::ConnectTo,
            prompt: format!("Show connection: {} {} which declaration? ({}; bounded search)", goal.display, if self.v.goal.dir == Direction::Uses { "uses" } else { "is used by" }, self.v.goal.mode.short()),
            text: default,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::theme::ThemeName;
    use crate::app::Event;
    use crate::model::{DeclKind, Origin};
    use crate::relations::Child;
    use crate::userdata::ReadState;
    use std::path::Path;
    use std::sync::atomic::AtomicBool;

    fn app() -> (App, std::sync::mpsc::Receiver<QueryReq>) {
        let ctx = Ctx { title: "t".into(), mode: IndexMode::Semantic, index: None, user_key: "lake:t".into(), editor: None, info: None, synthetic: false, project_root: None };
        let ch = Channels { query: None, search: None, jobs: None, events: None, search_latest: Arc::new(AtomicU64::new(0)), job_cancel: Arc::new(AtomicBool::new(false)) };
        let mut a = App::new(ctx, ThemeName::Mono, true, ch);
        let (tx, rx) = std::sync::mpsc::channel();
        a.ch.query = Some(tx);
        // Not typing into the search box: letters are commands.
        a.search.editing = false;
        a.user_acted = true;
        (a, rx)
    }

    const MAP: &str = r#"
schema = 1
[[objective]]
decl = "D.main"
title = "Main theorem"
[[group]]
id = "a"
title = "Alpha"
landmarks = ["D.x", "D.shared"]
[[group]]
id = "a1"
parent = "a"
title = "Inner"
landmarks = ["D.shared", "D.gone"]
[[group]]
id = "b"
title = "Beta"
landmarks = ["D.shared"]
[[trail]]
id = "intro"
title = "Introduction"
stops = [{ decl = "D.main" }]
"#;

    fn row(id: i64, name: &str) -> DeclRow {
        DeclRow { id, name: name.into(), display: name.into(), kind: DeclKind::Theorem, origin: Origin::Local, flags: 0, module: None }
    }

    fn load_map(a: &mut App, text: &str) {
        let m = crate::map::parse(text, Path::new("lean-view.toml")).unwrap();
        let mut res = crate::map::Resolution::default();
        for (i, (n, _)) in m.decl_refs().into_iter().enumerate() {
            let status = if n == "D.gone" { crate::map::RefStatus::Missing { suggestions: vec!["D.here".into()] } } else { crate::map::RefStatus::Resolved };
            let r = if n == "D.gone" { None } else { Some(row(i as i64 + 1, &n)) };
            res.refs.insert(n, crate::map::RefInfo { status, row: r, module_sha256: None });
        }
        a.handle(Event::Query(QueryResp::Map(Box::new(MapBundle { load: MapLoad::Loaded(Arc::new(m)), resolution: res }))));
    }

    fn keys(a: &App) -> Vec<String> {
        a.v.map.rows.iter().map(|r| r.key.clone()).collect()
    }

    fn press(a: &mut App, c: char) {
        a.handle(Event::Input(ratatui::crossterm::event::Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))));
    }

    fn key(a: &mut App, c: KeyCode) {
        a.handle(Event::Input(ratatui::crossterm::event::Event::Key(KeyEvent::new(c, KeyModifiers::NONE))));
    }

    fn import_graph() -> Arc<ImportGraph> {
        use crate::imports::Node;
        // In name order; `P` is an import-only umbrella file.
        let spec: [(&str, bool, u32, &[u32]); 7] = [
            ("Lib.Core", false, 0, &[]),
            ("P", true, 0, &[2, 3, 4, 5, 6]),
            ("P.Alg.Group", true, 9, &[4, 3]),
            ("P.Found.Basic", true, 5, &[0]),
            ("P.Found.Order", true, 3, &[3, 0]),
            ("P.Main.Other", true, 1, &[3]),
            ("P.Main.Result", true, 2, &[2]),
        ];
        let nodes = spec.iter().enumerate().map(|(i, s)| Node { id: i as i64 + 1, name: s.0.into(), local: s.1, decls: s.2 }).collect();
        let edges = spec.iter().enumerate().flat_map(|(i, s)| s.3.iter().map(move |&t| (i as u32, t))).collect();
        Arc::new(ImportGraph::build(nodes, edges))
    }

    fn ikeys(a: &App) -> Vec<String> {
        a.v.imports.rows.iter().map(|r| r.key.clone()).collect()
    }

    fn iselect(a: &mut App, key: &str) {
        let i = a.v.imports.rows.iter().position(|r| r.key == key).unwrap_or_else(|| panic!("no row {key}: {:?}", ikeys(a)));
        a.v.imports.list.selected = i;
        a.sync_view_detail();
    }

    fn detail_text(a: &App) -> String {
        crate::app::render_views::detail_lines(a, 100).iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn import_view_lists_layers_endpoints_and_hubs() {
        let (mut a, rx) = app();
        press(&mut a, 'I');
        assert_eq!(a.v.view, View::Imports);
        assert_eq!(rx.try_iter().filter(|r| matches!(r, QueryReq::Imports { .. })).count(), 1, "the graph is asked for once");
        assert_eq!(ikeys(&a), vec!["note:loading"]);
        // A stale answer is dropped.
        assert!(!a.on_imports(0, Err("old".into())));
        let gen = a.gens.imports;
        a.handle(Event::Query(QueryResp::Imports(gen, Ok(import_graph()))));
        // The single root is open; directories run from the bottom of the import order up.
        assert_eq!(
            ikeys(&a),
            vec![
                "sec:layers", "il:P", "il:P.Found", "il:P.Alg", "il:P.Main", "sec:endpoints", "ep:P.Main.Result", "ep:P.Main.Other", "sec:hubs", "hub:P.Found.Basic", "hub:P.Alg.Group",
                "hub:P.Found.Order", "sec:importonly"
            ]
        );
        // Selecting a directory asks the worker for its imports, once.
        iselect(&mut a, "il:P.Found");
        let asked: Vec<String> = rx.try_iter().filter_map(|r| if let QueryReq::ImportsDir { full, .. } = r { Some(full) } else { None }).collect();
        assert_eq!(asked, vec!["P.Found"]);
        assert!(detail_text(&a).contains("reading its imports"));
        let g = import_graph();
        let gen = a.gens.idir;
        a.handle(Event::Query(QueryResp::ImportsDir(gen, Ok(g.dir_summary(g.dir("P.Found").unwrap(), 12)))));
        let d = detail_text(&a);
        for n in ["directory / namespace prefix P.Found", "depth 0-1", "Imported by (5 import lines)", "P.Alg  (2)", "Lib  (2)", "1 import line between files inside it", "A build dependency, not a"] {
            assert!(d.contains(n), "detail lacks {n:?}:\n{d}");
        }
        a.sync_view_detail();
        assert_eq!(rx.try_iter().filter(|r| matches!(r, QueryReq::ImportsDir { .. })).count(), 0, "not asked again for the same directory");
        // Space opens it; a file shows its depth with a witness chain.
        key(&mut a, KeyCode::Char(' '));
        assert!(ikeys(&a).contains(&"il:P.Found.Order".to_string()));
        iselect(&mut a, "il:P.Alg");
        key(&mut a, KeyCode::Right);
        iselect(&mut a, "il:P.Alg.Group");
        let d = detail_text(&a);
        for n in ["file P.Alg.Group", "Depth 2", "P.Alg.Group -> P.Found.Order -> P.Found.Basic", "Imports (2)", "Imported by (2)", "P.Main.Result"] {
            assert!(d.contains(n), "detail lacks {n:?}:\n{d}");
        }
        // Enter shows the file in the browser's outline; Backspace returns to the same row.
        key(&mut a, KeyCode::Enter);
        assert_eq!((a.v.view, a.left_mode), (View::Browse, LeftMode::Outline));
        assert_eq!(a.outline.reveal.as_ref().map(|r| r.module.as_str()), Some("P.Alg.Group"));
        key(&mut a, KeyCode::Backspace);
        assert_eq!(a.v.view, View::Imports);
        assert_eq!(a.view_key().as_deref(), Some("il:P.Alg.Group"));
        // S skims the selected directory or file; f searches within it.
        iselect(&mut a, "il:P.Found");
        press(&mut a, 'S');
        assert!(rx.try_iter().any(|r| matches!(r, QueryReq::Skim { scope: SkimScope::Dir(d), .. } if d == "P.Found")));
        assert_eq!((a.v.view, a.center_mode), (View::Browse, CenterMode::Skim));
        key(&mut a, KeyCode::Backspace);
        assert_eq!(a.v.view, View::Imports);
        iselect(&mut a, "ep:P.Main.Result");
        press(&mut a, 'f');
        assert_eq!(a.search.scope.as_ref().map(|s| (s.kind, s.prefix.as_str())), Some((crate::nameindex::ScopeKind::Modules, "P.Main.Result")));
        assert_eq!(a.v.view, View::Browse);
    }

    #[test]
    fn import_path_questions_have_three_distinct_answers() {
        use crate::imports::PathAnswer;
        let (mut a, rx) = app();
        press(&mut a, 'I');
        let gen = a.gens.imports;
        a.handle(Event::Query(QueryResp::Imports(gen, Ok(import_graph()))));
        // Without a file selected, p says what it needs.
        iselect(&mut a, "sec:hubs");
        press(&mut a, 'p');
        assert!(matches!(a.overlay, Overlay::None));
        assert!(a.status.as_ref().unwrap().0.contains("select a file first"));
        iselect(&mut a, "ep:P.Main.Result");
        press(&mut a, 'p');
        assert!(matches!(&a.overlay, Overlay::Input { purpose: InputPurpose::ImportPathTo { from }, .. } if from == "P.Main.Result"));
        for c in "Basic".chars() {
            press(&mut a, c);
        }
        key(&mut a, KeyCode::Enter);
        let sent: Vec<(String, String)> = rx.try_iter().filter_map(|r| if let QueryReq::ImportPath { from, to, .. } = r { Some((from, to)) } else { None }).collect();
        assert_eq!(sent, vec![("P.Main.Result".to_string(), "Basic".to_string())]);
        assert!(ikeys(&a).starts_with(&["sec:path".to_string()]), "the question is shown while it is answered");
        let g = import_graph();
        let gen = a.gens.ipath;
        a.handle(Event::Query(QueryResp::ImportPath(gen, Ok(g.explain("P.Main.Result", "Basic")))));
        assert_eq!(&ikeys(&a)[..5], ["sec:path", "path:0", "path:1", "path:2", "sec:layers"]);
        assert_eq!(a.view_key().as_deref(), Some("sec:path"));
        // A step is a file: its detail, and Enter shows it in the outline.
        iselect(&mut a, "path:1");
        assert!(detail_text(&a).contains("file P.Alg.Group"));
        // No path and an unknown name are different answers.
        a.handle(Event::Query(QueryResp::ImportPath(gen, Ok(g.explain("P.Found.Basic", "P.Main.Result")))));
        assert!(matches!(&a.v.imports.rows[1].kind, IRowKind::Note { text, warn: false } if text.contains("does not import") && text.contains("were checked")));
        a.handle(Event::Query(QueryResp::ImportPath(gen, Ok(g.explain("P.Found.Basic", "Nope")))));
        assert!(matches!(&a.v.imports.rows[1].kind, IRowKind::Note { text, warn: true } if text.contains("no module is named")));
        assert!(matches!(a.v.imports.path, Some(PathAnswer::Unknown { .. })));
        // An answer to an older question is dropped.
        assert!(!a.on_import_path(gen - 1, Ok(PathAnswer::Path(vec!["x".into()]))));
    }

    #[test]
    fn import_view_follows_the_current_file_and_a_replaced_index() {
        let (mut a, rx) = app();
        // From a declaration, I shows where its file sits (when the graph arrives).
        a.detail = Some(Box::new(DetailBundle {
            d: crate::db::query::DeclDetail {
                row: DeclRow { module: Some("P.Found.Order".into()), ..row(7, "P.x") },
                module_id: Some(5),
                module_path: None,
                module_status: None,
                module_sha256: None,
                type_cov: crate::model::Coverage::Complete,
                value_cov: crate::model::Coverage::Complete,
                pos: Default::default(),
                type_text: None,
                sig: None,
                doc: None,
                attrs: None,
                generated: None,
                keyword: None,
                sorry_line: None,
                degrees: [0; 6],
                axioms: Vec::new(),
                lean_axioms: None,
                same_name: 0,
            },
            excerpt: Vec::new(),
            excerpt_truncated: false,
            decl_lines: None,
            source_status: SourceStatus::NoPath,
            source_sha256: None,
            statement_src: None,
            statement_at: None,
            bookmarked: false,
        }));
        press(&mut a, 'I');
        let gen = a.gens.imports;
        a.handle(Event::Query(QueryResp::Imports(gen, Ok(import_graph()))));
        assert_eq!(a.view_key().as_deref(), Some("il:P.Found.Order"));
        assert!(a.v.imports.expanded.contains("il:P.Found"));
        // The index is replaced: the graph is read again and the row found again.
        let _ = rx.try_iter().count();
        a.reset_imports();
        assert!(a.v.imports.graph.is_none());
        assert_eq!(rx.try_iter().filter(|r| matches!(r, QueryReq::Imports { .. })).count(), 1);
        let gen = a.gens.imports;
        a.handle(Event::Query(QueryResp::Imports(gen, Ok(import_graph()))));
        assert_eq!(a.view_key().as_deref(), Some("il:P.Found.Order"));
        // A session that ended in this view asks for the graph when it resumes.
        let js = a.session_json();
        let (mut b, rx2) = app();
        b.restore_session(&js);
        assert_eq!(b.v.view, View::Imports);
        assert!(rx2.try_iter().any(|r| matches!(r, QueryReq::Imports { .. })));
        // A failure is shown and can be retried by opening the view again.
        let gen = b.gens.imports;
        b.handle(Event::Query(QueryResp::Imports(gen, Err("locked".into()))));
        assert!(matches!(&b.v.imports.rows[0].kind, IRowKind::Note { text, warn: true } if text.contains("locked")));
        b.set_view(View::Browse);
        b.set_view(View::Imports);
        assert!(rx2.try_iter().any(|r| matches!(r, QueryReq::Imports { .. })));
    }

    #[test]
    fn map_outline_nests_groups_and_lists_unresolved_references() {
        let (mut a, _rx) = app();
        load_map(&mut a, MAP);
        press(&mut a, 'M');
        assert_eq!(a.v.view, View::Map);
        assert_eq!(keys(&a), ["sec:objectives", "obj:D.main", "sec:components", "grp:a", "grp:b", "sec:foundations", "note:nofound", "sec:unresolved", "unres:D.gone", "sec:files"]);
        // Space expands without leaving the row; nested groups follow landmarks.
        a.v.map.list.selected = 3;
        press(&mut a, ' ');
        assert_eq!(a.v.map.list.selected, 3);
        assert_eq!(&keys(&a)[3..8], ["grp:a", "lm:a:D.x", "lm:a:D.shared", "grp:a1", "grp:b"]);
        // The unresolved entry explains itself and never re-attaches.
        let i = keys(&a).iter().position(|k| k == "unres:D.gone").unwrap();
        a.v.map.list.selected = i;
        key(&mut a, KeyCode::Enter);
        let (msg, warn) = a.status.clone().unwrap();
        assert!(warn && msg.contains("D.here") && msg.contains("Nothing is re-attached automatically"));
        assert_eq!(a.v.view, View::Map);
    }

    #[test]
    fn selection_survives_reload_and_collapse_of_other_rows() {
        let (mut a, _rx) = app();
        load_map(&mut a, MAP);
        a.set_view(View::Map);
        a.map_reveal("lm:a1:D.shared");
        assert_eq!(a.view_key().as_deref(), Some("lm:a1:D.shared"));
        // A reload that adds rows above keeps the same item selected.
        let more = MAP.replace("[[group]]\nid = \"a\"", "[[group]]\nid = \"zero\"\norder = -1\nlandmarks = [\"D.x\"]\n[[group]]\nid = \"a\"");
        a.v.map.expanded.insert("grp:zero".into());
        load_map(&mut a, &more);
        assert_eq!(a.view_key().as_deref(), Some("lm:a1:D.shared"));
        // When the item disappears the selection stays in range.
        load_map(&mut a, "schema = 1\n");
        assert!(a.v.map.list.selected < a.v.map.rows.len());
    }

    #[test]
    fn one_declaration_many_placements_are_cycled_from_the_browser() {
        let (mut a, _rx) = app();
        load_map(&mut a, MAP);
        assert_eq!(a.map_placements("D.shared"), ["lm:a:D.shared", "lm:a1:D.shared", "lm:b:D.shared"]);
        assert_eq!(a.map_placements("D.main"), ["obj:D.main"]);
        a.set_view(View::Map);
        a.map_reveal("lm:a:D.shared");
        for want in ["lm:a1:D.shared", "lm:b:D.shared", "lm:a:D.shared"] {
            press(&mut a, 'M');
            assert_eq!(a.view_key().as_deref(), Some(want));
        }
    }

    #[test]
    fn views_are_history_steps_and_enter_opens_the_declaration_browser() {
        let (mut a, rx) = app();
        load_map(&mut a, MAP);
        press(&mut a, 'M');
        a.map_reveal("obj:D.main");
        key(&mut a, KeyCode::Enter);
        assert_eq!(a.v.view, View::Browse);
        assert!(rx.try_iter().any(|r| matches!(r, QueryReq::ResolveName { name, .. } if name == "D.main")));
        // Backspace returns to the map with the same row selected.
        key(&mut a, KeyCode::Backspace);
        assert_eq!(a.v.view, View::Map);
        assert_eq!(a.view_key().as_deref(), Some("obj:D.main"));
        key(&mut a, KeyCode::Backspace);
        assert_eq!(a.v.view, View::Browse, "back to where M was pressed");
        key(&mut a, KeyCode::Char(']'));
        assert_eq!(a.v.view, View::Map);
    }

    #[test]
    fn malformed_map_keeps_the_browser_and_reports_fields() {
        let (mut a, _rx) = app();
        let issues = crate::map::parse("schema = 1\n[[group]]\nid = \"a\"\npurpos = 1\n", Path::new("x")).unwrap_err();
        a.handle(Event::Query(QueryResp::Map(Box::new(MapBundle { load: MapLoad::Malformed { path: "lean-view.toml".into(), issues }, resolution: Default::default() }))));
        assert!(a.status.as_ref().is_some_and(|(m, w)| *w && m.contains("left unchanged")));
        press(&mut a, 'M');
        assert!(a.v.map.rows.iter().any(|r| matches!(&r.kind, MapRowKind::Issue { text, .. } if text.contains("purpos"))));
        // The declaration browser is one key away and unaffected.
        press(&mut a, '/');
        assert_eq!(a.v.view, View::Browse);
        assert!(a.search.editing);
    }

    fn page(names: &[&str]) -> ChildPage {
        ChildPage {
            children: names
                .iter()
                .enumerate()
                .map(|(i, n)| Child { name: n.to_string(), row: Some(row(100 + i as i64, n)), labels: vec![EdgeLabel::Lean { type_: false, body: true, ctor: false }], note: None })
                .collect(),
            ..Default::default()
        }
    }

    fn goal_names(a: &App) -> Vec<(String, u16, bool, bool)> {
        a.v.goal.rows.iter().filter_map(|r| if let GRow::Node(i) = r { let n = &a.v.goal.nodes[*i]; Some((n.name.clone(), n.depth, n.backref, n.repeat)) } else { None }).collect()
    }

    fn answer(a: &mut App, rx: &std::sync::mpsc::Receiver<QueryReq>, p: ChildPage) -> ChildQuery {
        let (gen, node, q) = rx.try_iter().filter_map(|r| if let QueryReq::GoalChildren { gen, node, q } = r { Some((gen, node, q)) } else { None }).last().expect("a children request");
        a.handle(Event::Query(QueryResp::GoalChildren(gen, node, q.offset, Ok(p))));
        q
    }

    #[test]
    fn goal_outline_unfolds_lazily_with_path_local_cycle_detection() {
        let (mut a, rx) = app();
        a.set_goal(Some(GoalRef { name: "G".into(), display: "G".into() }));
        press(&mut a, 'A');
        assert_eq!(a.v.view, View::Goal);
        let q = answer(&mut a, &rx, page(&["A", "B"]));
        assert_eq!((q.name.as_str(), q.mode, q.dir), ("G", RelMode::Curated, Direction::Uses));
        assert_eq!(goal_names(&a).iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["G", "A", "B"]);
        // Unfold A: it uses G (a cycle back to the goal) and B (seen elsewhere).
        a.v.goal.list.selected = 1;
        press(&mut a, ' ');
        answer(&mut a, &rx, page(&["G", "B", "C"]));
        let n = goal_names(&a);
        assert_eq!(n.iter().map(|x| (x.0.as_str(), x.1)).collect::<Vec<_>>(), [("G", 0), ("A", 1), ("G", 2), ("B", 2), ("C", 2), ("B", 1)]);
        assert!(n[2].2 && !n[2].3, "G under A is a back-reference");
        assert!(!n[3].2 && n[3].3, "B under A is a repeat with a link marker, not pruned");
        // The back-reference cannot be unfolded (finite), the repeat can.
        a.v.goal.list.selected = 2;
        press(&mut a, ' ');
        assert!(rx.try_iter().all(|r| !matches!(r, QueryReq::GoalChildren { .. })));
        assert!(a.status.as_ref().unwrap().0.contains("cycle"));
        a.v.goal.list.selected = 3;
        press(&mut a, ' ');
        let q = answer(&mut a, &rx, page(&["A"]));
        assert_eq!(q.name, "B");
        let n = goal_names(&a);
        assert_eq!((n[4].0.as_str(), n[4].1, n[4].2), ("A", 3, true), "A is on this path: G → A → B → A");
        // Collapsing and re-expanding does not ask again.
        a.v.goal.list.selected = 1;
        press(&mut a, ' ');
        assert_eq!(goal_names(&a).len(), 3);
        press(&mut a, ' ');
        assert!(rx.try_iter().all(|r| !matches!(r, QueryReq::GoalChildren { .. })));
        assert_eq!(goal_names(&a).len(), 7);
    }

    #[test]
    fn huge_fan_out_is_paged_and_hidden_counts_are_shown() {
        let (mut a, rx) = app();
        a.set_goal(Some(GoalRef { name: "Hub".into(), display: "Hub".into() }));
        a.set_view(View::Goal);
        let names: Vec<String> = (0..50).map(|i| format!("N{i}")).collect();
        let mut p = page(&names.iter().map(|s| s.as_str()).collect::<Vec<_>>());
        p.more = Some(50);
        p.remaining = 99_950;
        p.hidden_external = 12;
        p.hidden_generated = 3;
        answer(&mut a, &rx, p);
        assert_eq!(a.v.goal.nodes.len(), 51, "only one page is materialised");
        assert!(a.v.goal.rows.iter().any(|r| matches!(r, GRow::More { remaining: 99_950, .. })));
        assert!(a.v.goal.rows.iter().any(|r| matches!(r, GRow::Note { text, .. } if text.contains("12 external") && text.contains("3 generated") && text.contains("(H)"))));
        // Loading the next page appends; the cursor moves on.
        let i = a.v.goal.rows.iter().position(|r| matches!(r, GRow::More { .. })).unwrap();
        a.v.goal.list.selected = i;
        key(&mut a, KeyCode::Enter);
        let more: Vec<String> = (50..100).map(|i| format!("N{i}")).collect();
        let mut p2 = page(&more.iter().map(|s| s.as_str()).collect::<Vec<_>>());
        p2.more = Some(100);
        p2.remaining = 99_900;
        let q = answer(&mut a, &rx, p2);
        assert_eq!(q.offset, 50);
        assert_eq!(a.v.goal.nodes.len(), 101);
        assert!(a.v.goal.rows.iter().any(|r| matches!(r, GRow::More { offset: 100, remaining: 99_900, .. })));
        // H asks again with every filter off.
        press(&mut a, 'H');
        let q = rx.try_iter().filter_map(|r| if let QueryReq::GoalChildren { q, .. } = r { Some(q) } else { None }).last().unwrap();
        assert!(!q.local_only && q.show_generated && q.tb == TypeBody::All);
    }

    #[test]
    fn relation_modes_are_explicit_and_stale_answers_are_dropped() {
        let (mut a, rx) = app();
        a.ctx.mode = IndexMode::Source;
        a.set_goal(Some(GoalRef { name: "G".into(), display: "G".into() }));
        a.set_view(View::Goal);
        let old: Vec<(u64, u64)> = rx.try_iter().filter_map(|r| if let QueryReq::GoalChildren { gen, node, .. } = r { Some((gen, node)) } else { None }).collect();
        press(&mut a, 'r');
        assert_eq!(a.v.goal.mode, RelMode::Mentions);
        press(&mut a, 'r');
        assert_eq!(a.v.goal.mode, RelMode::Lean);
        assert!(a.status.as_ref().unwrap().0.contains("UNAVAILABLE"), "not silently skipped");
        // An answer for the earlier mode arrives late: ignored.
        let (g, n) = old[0];
        a.dirty = false;
        a.handle(Event::Query(QueryResp::GoalChildren(g, n, 0, Ok(page(&["Stale"])))));
        assert!(!a.dirty && a.v.goal.nodes.len() == 1);
        // Direction is spelled out.
        press(&mut a, 'u');
        assert!(a.status.as_ref().unwrap().0.contains("used by"));
        // * on the goal clears it; the explorer says what to do next.
        a.v.goal.list.selected = 0;
        press(&mut a, '*');
        assert!(a.v.goal.goal.is_none());
        assert!(matches!(&a.v.goal.rows[0], GRow::Info(t) if t.contains("No goal pinned")));
    }

    #[test]
    fn connection_results_distinguish_no_path_truncation_and_unavailable() {
        let (mut a, _rx) = app();
        a.set_goal(Some(GoalRef { name: "G".into(), display: "G".into() }));
        a.set_view(View::Goal);
        let q = ConnQuery { from: "G".into(), to: "X".into(), mode: RelMode::Lean, dir: Direction::Uses, tb: TypeBody::All };
        let show = |a: &mut App, outcome: ConnOutcome| -> String {
            let gen = a.gens.bump();
            a.gens.conn = gen;
            a.v.goal.conn = Some(ConnState { query: q.clone(), running: true, result: None });
            a.handle(Event::Query(QueryResp::Connection(gen, Box::new(ConnResult { query: q.clone(), outcome, explored: 7, ms: 3 }))));
            match &a.v.goal.rows[0] {
                GRow::ConnHeader { text, .. } => text.clone(),
                other => panic!("{other:?}"),
            }
        };
        assert!(show(&mut a, ConnOutcome::NoPath).starts_with("No path in this data"));
        assert!(show(&mut a, ConnOutcome::Truncated { by_time: true }).contains("truncated") );
        assert!(show(&mut a, ConnOutcome::Truncated { by_time: false }).contains("no conclusion"));
        assert!(show(&mut a, ConnOutcome::Unavailable("Semantic information unavailable: x".into())).contains("Semantic information unavailable"));
        assert!(show(&mut a, ConnOutcome::Cancelled).contains("cancelled"));
        let steps = vec![
            crate::relations::ConnStep { name: "G".into(), row: Some(row(1, "G")), next: Some(EdgeLabel::Lean { type_: false, body: true, ctor: false }) },
            crate::relations::ConnStep { name: "X".into(), row: Some(row(2, "X")), next: None },
        ];
        assert!(show(&mut a, ConnOutcome::Path(steps)).starts_with("Connection found: 1 step"));
        assert!(a.v.goal.rows.iter().any(|r| matches!(r, GRow::Info(t) if t.contains("connectivity only"))));
        // A result for a superseded query is ignored.
        a.dirty = false;
        a.handle(Event::Query(QueryResp::Connection(1, Box::new(ConnResult { query: q.clone(), outcome: ConnOutcome::NoPath, explored: 0, ms: 0 }))));
        assert!(!a.dirty);
    }

    fn reading() -> Reading {
        let stop = |id: i64, name: &str, state: ReadState| Stop { id, trail_id: 1, name: name.into(), display: name.into(), kind: "theorem".into(), state, ..Default::default() };
        let mut r = Reading::default();
        r.trails.push(crate::userdata::Trail {
            id: 1,
            title: "Main argument".into(),
            origin: "personal".into(),
            stops: vec![stop(10, "D.main", ReadState::Understood), stop(11, "D.x", ReadState::Unread), stop(12, "D.renamed", ReadState::Revisit)],
            ..Default::default()
        });
        r.statuses.insert(12, StopStatus::Unresolved("not in this index (renamed or removed?)".into()));
        r.prefs.insert("goal".into(), "D.main".into());
        r.prefs.insert("active_trail".into(), "1".into());
        r
    }

    #[test]
    fn trails_view_edits_stops_and_keeps_stale_references_visible() {
        let (mut a, rx) = app();
        load_map(&mut a, MAP);
        press(&mut a, 'T');
        assert!(rx.try_iter().any(|r| matches!(r, QueryReq::LoadReading)));
        a.handle(Event::Query(QueryResp::User(0, UserData::Reading(Box::new(reading())), String::new())));
        // The saved goal is restored with the reading data.
        assert_eq!(a.v.goal.goal.as_ref().map(|g| g.name.as_str()), Some("D.main"));
        // A shared trail that was not started is offered.
        assert!(a.v.trails.rows.iter().any(|r| matches!(r, TRow::Shared(0))));
        let ti = a.v.trails.rows.iter().position(|r| matches!(r, TRow::Trail(0))).unwrap();
        a.v.trails.list.selected = ti;
        // r resumes at the first stop that is not read.
        press(&mut a, 'r');
        assert!(matches!(a.v.trails.rows[a.v.trails.list.selected], TRow::Stop { stop: 1, .. }));
        press(&mut a, 's');
        assert!(rx.try_iter().any(|r| matches!(r, QueryReq::StopSetState { id: 11, state: ReadState::Read })));
        press(&mut a, '-');
        press(&mut a, '+');
        let moves: Vec<i64> = rx.try_iter().filter_map(|r| if let QueryReq::StopMove { id: 11, delta } = r { Some(delta) } else { None }).collect();
        assert_eq!(moves, [-1, 1]);
        // An unresolved stop is kept, explained, and does not navigate.
        press(&mut a, 'j');
        key(&mut a, KeyCode::Enter);
        assert_eq!(a.v.view, View::Trails);
        assert!(a.status.as_ref().is_some_and(|(m, w)| *w && m.contains("renamed or removed") && m.contains("kept")));
        // Notes and questions go through an input prompt.
        press(&mut a, 'Q');
        assert!(matches!(&a.overlay, Overlay::Input { purpose: InputPurpose::StopQuestion { id: 12 }, .. }));
        for c in "why?".chars() {
            press(&mut a, c);
        }
        key(&mut a, KeyCode::Enter);
        assert!(rx.try_iter().any(|r| matches!(r, QueryReq::StopSetQuestion { id: 12, question: Some(q) } if q == "why?")));
        // Starting the shared trail asks the worker for a personal copy.
        let si = a.v.trails.rows.iter().position(|r| matches!(r, TRow::Shared(0))).unwrap();
        a.v.trails.list.selected = si;
        key(&mut a, KeyCode::Enter);
        assert!(rx.try_iter().any(|r| matches!(r, QueryReq::StartShared { id } if id == "intro")));
        // After a reload the same stop stays selected.
        a.v.trails.list.selected = a.v.trails.rows.iter().position(|r| matches!(r, TRow::Stop { stop: 2, .. })).unwrap();
        let mut r2 = reading();
        r2.trails[0].stops.swap(0, 2);
        a.handle(Event::Query(QueryResp::User(0, UserData::Reading(Box::new(r2)), String::new())));
        assert!(matches!(a.v.trails.rows[a.v.trails.list.selected], TRow::Stop { stop: 0, .. }), "follows stop 12 to its new position");
    }

    #[test]
    fn browser_keys_in_a_view_are_explained_not_ignored() {
        let (mut a, _rx) = app();
        load_map(&mut a, MAP);
        press(&mut a, 'M');
        press(&mut a, '3');
        assert!(a.status.as_ref().unwrap().0.contains("declaration browser"));
        assert_eq!(a.v.view, View::Map);
        // Adding a group to a trail works from the map.
        a.map_reveal("grp:a");
        press(&mut a, 'a');
        assert!(matches!(&a.overlay, Overlay::Input { purpose: InputPurpose::StopAdd { stop }, .. } if stop.ref_kind == RefKind::Group && stop.name == "a"));
    }
}
