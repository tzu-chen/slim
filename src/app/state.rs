//! Application state and input handling. Everything here runs on the UI
//! thread and only ever *sends* requests to workers; results arrive as
//! events and are applied if their generation is still current.

use super::theme::{Symbols, Theme, ThemeName};
use super::views::{View, ViewsState};
use super::worker::*;
use super::Event;
use crate::db::query::*;
use crate::db::IndexMode;
use crate::model::{flags, EdgeKind, KindFilter, Origin};
use crate::nameindex::{MatchTier, ScopeKind, ScopeSpec, SearchParams};
use crate::relations::Occurrence;
use crate::skim::{Skim, SkimItem, SkimScope};
use crate::source::index::Freshness;
use crate::userdata::{Mark, RefKind, Stop};
use ratatui::crossterm::event::{Event as CEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Focus {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LeftMode {
    Search,
    Outline,
    Bookmarks,
    Trail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RightTab {
    Uses,
    UsedBy,
    Axioms,
    Module,
    Info,
}

impl RightTab {
    pub const ALL: [RightTab; 5] = [RightTab::Uses, RightTab::UsedBy, RightTab::Axioms, RightTab::Module, RightTab::Info];
    /// Shorter labels for narrow panes (level 1 = medium, 2 = short).
    pub fn short_label(self, mode: IndexMode, level: u8) -> &'static str {
        match (self, level) {
            (_, 0) => self.label(mode),
            (RightTab::Uses, 1) => self.label(mode),
            (RightTab::Uses, _) => if mode == IndexMode::Source { "Ment" } else { "Uses" },
            (RightTab::UsedBy, _) => "By",
            (RightTab::Axioms, 1) => self.label(mode),
            (RightTab::Axioms, _) => if mode == IndexMode::Source { "Evid" } else { "Ax" },
            (RightTab::Module, 1) => "Module",
            (RightTab::Module, _) => "Mod",
            (RightTab::Info, 1) => "Index",
            (RightTab::Info, _) => "Idx",
        }
    }
    pub fn label(self, mode: IndexMode) -> &'static str {
        match (self, mode) {
            (RightTab::Uses, IndexMode::Semantic) => "Uses",
            (RightTab::Uses, IndexMode::Source) => "Mentions",
            (RightTab::UsedBy, _) => "Used by",
            (RightTab::Axioms, IndexMode::Semantic) => "Axioms",
            (RightTab::Axioms, IndexMode::Source) => "Evidence",
            (RightTab::Module, _) => "Module",
            (RightTab::Info, _) => "Index",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CenterMode {
    Decl,
    Source,
    /// Statements of a file, directory, namespace or map group.
    Skim,
}

/// How the children of an outline node are ordered; the column on the
/// right of each row shows the same quantity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum OutlineOrder {
    #[default]
    Name,
    Decls,
    Theorems,
    ProofLines,
    Sorry,
    Modified,
}

impl OutlineOrder {
    pub fn next(self, mode: IndexMode) -> OutlineOrder {
        match self {
            OutlineOrder::Name => OutlineOrder::Decls,
            OutlineOrder::Decls => OutlineOrder::Theorems,
            // Proof lines are recorded by the source scan only.
            OutlineOrder::Theorems if mode == IndexMode::Source => OutlineOrder::ProofLines,
            OutlineOrder::Theorems | OutlineOrder::ProofLines => OutlineOrder::Sorry,
            OutlineOrder::Sorry => OutlineOrder::Modified,
            OutlineOrder::Modified => OutlineOrder::Name,
        }
    }

    pub fn label(self, mode: IndexMode) -> &'static str {
        match (self, mode) {
            (OutlineOrder::Name, _) => "by name",
            (OutlineOrder::Decls, _) => "most declarations first",
            (OutlineOrder::Theorems, _) => "most theorems first",
            (OutlineOrder::ProofLines, _) => "most proof lines first",
            (OutlineOrder::Sorry, IndexMode::Source) => "most sorry tokens first",
            (OutlineOrder::Sorry, IndexMode::Semantic) => "most sorryAx dependents first",
            (OutlineOrder::Modified, _) => "last modified first",
        }
    }

    /// The quantity nodes are ordered by (larger first).
    pub fn key(self, n: &OutlineNode) -> i64 {
        match self {
            OutlineOrder::Name => 0,
            OutlineOrder::Decls => n.decls_total as i64,
            OutlineOrder::Theorems => n.roll.thm as i64,
            OutlineOrder::ProofLines => n.roll.proof_lines as i64,
            OutlineOrder::Sorry => n.roll.sorry as i64,
            OutlineOrder::Modified => n.roll.mtime_ns,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListState {
    pub selected: usize,
    pub scroll: usize,
}

impl ListState {
    /// Keeps the selection visible in a viewport of `height` rows.
    pub fn clamp(&mut self, len: usize, height: usize) {
        if len == 0 {
            self.selected = 0;
            self.scroll = 0;
            return;
        }
        self.selected = self.selected.min(len - 1);
        let h = height.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + h {
            self.scroll = self.selected + 1 - h;
        }
        self.scroll = self.scroll.min(len.saturating_sub(1));
    }
}

#[derive(Clone, Debug)]
pub enum LeftItem {
    Result { row: DeclRow, tier: Option<MatchTier> },
    /// A group, objective or landmark placement of the project map.
    MapHit { key: String, label: String, what: &'static str },
    Section(String),
}

#[derive(Default)]
pub struct SearchPane {
    /// Enter was pressed before results arrived: open the first one then.
    pub open_on_results: bool,
    pub query: String,
    pub cursor: usize,
    pub editing: bool,
    pub items: Vec<LeftItem>,
    pub list: ListState,
    pub summary: String,
    pub pending: bool,
    /// The search is restricted to a namespace or to the files below a
    /// directory (`f` sets it from what is selected, `F` clears it).
    pub scope: Option<ScopeSpec>,
}

#[derive(Clone, Debug)]
pub enum OutlineRow {
    Node(usize),
    Decl { item: OutlineItem, depth: u16 },
    Note { text: String, depth: u16 },
}

/// A pending "reveal in outline" (applied when the outline and the
/// module's declarations have loaded).
#[derive(Clone, Debug, PartialEq)]
pub struct Reveal {
    pub module: String,
    pub decl: Option<i64>,
}

#[derive(Default)]
pub struct OutlinePane {
    pub nodes: Vec<OutlineNode>,
    pub roots: Vec<usize>,
    pub by_name: HashMap<String, usize>,
    pub reveal: Option<Reveal>,
    pub rows: Vec<OutlineRow>,
    pub list: ListState,
    pub module_decls: HashMap<i64, (Vec<OutlineItem>, bool)>,
    /// Modules whose declarations were requested and have not arrived.
    pub pending_decls: HashSet<i64>,
    pub loaded: bool,
    pub requested: bool,
    pub order: OutlineOrder,
}

impl OutlinePane {
    /// Nodes in the order they are shown: as built (by name), or by the
    /// chosen quantity, largest first.
    pub fn ordered<'a>(&'a self, ids: &'a [usize]) -> std::borrow::Cow<'a, [usize]> {
        if self.order == OutlineOrder::Name {
            return std::borrow::Cow::Borrowed(ids);
        }
        let mut v = ids.to_vec();
        v.sort_by_key(|&i| std::cmp::Reverse(self.order.key(&self.nodes[i])));
        std::borrow::Cow::Owned(v)
    }
}

#[derive(Default)]
pub struct MarksPane {
    pub marks: Vec<Mark>,
    pub list: ListState,
    pub loaded: bool,
}

#[derive(Clone, Debug)]
pub enum RRow {
    Header { text: String, kind: Option<EdgeKind> },
    Item { key: u64, row: DeclRow, kind: EdgeKind, depth: u16, parent_id: i64, expanded: bool, cycle: bool, dir: Direction },
    More { kind: EdgeKind, parent: Option<u64>, parent_id: i64, depth: u16, offset: usize, dir: Direction, remaining: u64 },
    Text { text: String, warn: bool },
    Evidence { row: DeclRow, step: usize, kind_in: Option<EdgeKind>, kind_next: Option<EdgeKind>, axiom: String, category: crate::model::AxiomCategory },
    /// `occ`: where the identifier is written in the declaration's text.
    Mention { ident: String, row: DeclRow, occ: Arc<Vec<Occurrence>> },
    Module { module: ModuleRow, flags: i64, current: bool },
}

impl RRow {
    pub fn selectable(&self) -> bool {
        matches!(self, RRow::Item { .. } | RRow::More { .. } | RRow::Evidence { .. } | RRow::Mention { .. } | RRow::Module { .. })
    }
}

#[derive(Default)]
pub struct RightPane {
    /// Rows belong to the previous declaration; new ones are loading.
    pub stale: bool,
    pub rows: Vec<RRow>,
    pub list: ListState,
    pub loading: bool,
    pub error: Option<String>,
    pub next_key: u64,
    pub transitive: Option<(Direction, u64, bool)>,
    /// Expanded item keys and their decl ids (for cycle detection).
    pub expanded: HashMap<u64, i64>,
    pub module_doc: Option<String>,
    /// The reader moved through the mentions list: the occurrences of the
    /// selected identifier are highlighted in the centre pane.
    pub marks_on: bool,
}

#[derive(Default)]
pub struct CenterPane {
    pub scroll: usize,
    pub source: Option<FullSource>,
    pub source_scroll: usize,
    pub version: u64,
    pub cache: Option<(u16, u64, Vec<ratatui::text::Line<'static>>)>,
    pub height: usize,
    pub width: u16,
    /// For each source line of the excerpt: its first row in `cache`.
    pub excerpt_rows: Vec<(usize, usize)>,
    /// Source line to bring into view when the lines are next laid out.
    pub reveal_line: Option<usize>,
    pub skim: SkimPane,
}

/// The skim shown in the centre pane (`CenterMode::Skim`).
#[derive(Default)]
pub struct SkimPane {
    pub data: Option<Box<Skim>>,
    /// The scope asked for (shown while it loads, and kept for history).
    pub scope: Option<SkimScope>,
    pub loading: bool,
    pub error: Option<String>,
    /// Index into `data.items` of the selected declaration.
    pub sel: usize,
    pub scroll: usize,
    /// Selection to apply when the skim arrives (history).
    pub pending_sel: Option<usize>,
    pub version: u64,
    /// Laid-out rows for a width and version, and the rows of each item.
    pub cache: Option<(u16, u64, Vec<ratatui::text::Line<'static>>, Vec<(usize, usize)>)>,
    pub height: usize,
}

impl SkimPane {
    pub fn decl_items(&self) -> Vec<usize> {
        match &self.data {
            Some(s) => s.items.iter().enumerate().filter(|(_, i)| matches!(i, SkimItem::Decl(_))).map(|(k, _)| k).collect(),
            None => Vec::new(),
        }
    }
    pub fn selected(&self) -> Option<&crate::skim::SkimDecl> {
        match self.data.as_ref()?.items.get(self.sel)? {
            SkimItem::Decl(d) => Some(d),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Location {
    pub decl: Option<i64>,
    pub decl_name: String,
    pub focus: Focus,
    pub left_mode: LeftMode,
    pub query: String,
    pub left_sel: usize,
    pub left_scroll: usize,
    pub center_mode: CenterMode,
    pub center_scroll: usize,
    pub right_tab: RightTab,
    pub right_sel: usize,
    pub right_scroll: usize,
    /// How the reader arrived here (shown in the route).
    pub how: String,
    /// Which view was showing (the map, goal and trail views are history steps too).
    #[serde(default)]
    pub view: View,
    /// Selected row of that view, by key.
    #[serde(default)]
    pub vkey: Option<String>,
    /// The skim that was in the centre pane, and its selected item.
    #[serde(default)]
    pub skim: Option<SkimScope>,
    #[serde(default)]
    pub skim_sel: usize,
    /// The scope the search was restricted to.
    #[serde(default)]
    pub scope: Option<ScopeSpec>,
}

#[derive(Default)]
pub struct History {
    pub back: Vec<Location>,
    pub fwd: Vec<Location>,
}

const HISTORY_CAP: usize = 500;

#[derive(Clone, Copy, Debug)]
pub struct Filters {
    pub local_only: bool,
    pub show_generated: bool,
    pub kinds: KindFilter,
    pub show_implicit: bool,
}

#[derive(Clone, Debug)]
pub enum Fresh {
    Unknown,
    Checking,
    Fresh { checked: usize, ms: u128 },
    Stale(Freshness),
    Error(String),
}

#[derive(Clone, Debug)]
pub enum InputPurpose {
    BookmarkNote { name: String },
    StopAdd { stop: Box<Stop> },
    StopNote { id: i64 },
    StopQuestion { id: i64 },
    NewTrail,
    RenameTrail { id: i64 },
    DeleteTrail { id: i64 },
    ExportTrailTo { trail: i64 },
    AskQuestion { ref_kind: RefKind, name: String, display: String },
    /// The other end of a connection query from the goal.
    ConnectTo,
    GotoName,
    /// "Why does `from` import …": the module to look for.
    ImportPathTo { from: String },
}

pub enum Overlay {
    None,
    Help { scroll: usize },
    Palette { input: String, sel: usize },
    Input { purpose: InputPurpose, prompt: String, text: String },
    /// Explicit trust decision: the plan is shown and `yes` must be typed.
    ConfirmExport { plan: String, typed: String },
    /// The same for the semantic-highlight indexing job.
    ConfirmHighlight { plan: String, typed: String },
    /// Highlighting details: settings, freshness facts, indexing progress.
    HlStatus { scroll: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    FocusSearch,
    Outline,
    OutlineOrder,
    Skim,
    SearchScope,
    ClearScope,
    Imports,
    ImportPath,
    Bookmarks,
    Trail,
    Map,
    Goal,
    ToggleGoal,
    Connect,
    Question,
    ReloadMap,
    ToggleBookmark,
    AddToTrail,
    EditNote,
    ExportTrail,
    ToggleLocal,
    ToggleGenerated,
    CycleKinds,
    ToggleImplicit,
    ToggleSource,
    Back,
    Forward,
    TabUses,
    TabUsedBy,
    TabAxioms,
    TabModule,
    TabInfo,
    Transitive,
    CheckFreshness,
    Theme,
    Ascii,
    GotoName,
    OpenEditor,
    ExportHelp,
    SemanticExport,
    HlMode,
    HlTheme,
    HlStatus,
    HlBuild,
    HlBuildFile,
    HlPause,
    HlCancel,
    HlStale,
    Help,
    Quit,
}

pub const COMMANDS: &[(Cmd, &str, &str)] = &[
    (Cmd::FocusSearch, "Search declarations by name or documentation", "/"),
    (Cmd::Outline, "Browse modules and namespaces (outline)", "O"),
    (Cmd::OutlineOrder, "Outline: cycle order and column (name, declarations, theorems, proof lines, sorry, last modified)", "o"),
    (Cmd::Skim, "Skim: statements of this file, directory, namespace or map group, proofs left out", "S"),
    (Cmd::SearchScope, "Search within: restrict search to the selected directory or the current namespace (again: widen)", "f"),
    (Cmd::ClearScope, "Search everywhere again (clear the search scope)", "F"),
    (Cmd::Imports, "Import layers: files by import depth, endpoints, hubs", "I"),
    (Cmd::ImportPath, "Why does this file import…: show a chain of import lines to another module", "p in I"),
    (Cmd::Bookmarks, "Show bookmarks", "B"),
    (Cmd::Map, "Architecture map: main results, components, landmarks (reveals the current declaration)", "M"),
    (Cmd::Goal, "Goal explorer: unfold what the pinned goal rests on", "A"),
    (Cmd::Trail, "Reading trails, stops and questions", "T"),
    (Cmd::ToggleGoal, "Pin the selected declaration as the goal / clear the goal", "*"),
    (Cmd::Connect, "Show connection from the goal to a declaration (bounded search)", "c"),
    (Cmd::Question, "Ask a question about the selected declaration or group", "Q"),
    (Cmd::ReloadMap, "Reload the project map (lean-view.toml) after editing it", ""),
    (Cmd::ToggleBookmark, "Bookmark / unbookmark current declaration", "m"),
    (Cmd::AddToTrail, "Add the selected declaration or group to the active trail (with note)", "a"),
    (Cmd::EditNote, "Edit note of the selected bookmark", "n"),
    (Cmd::ExportTrail, "Export a reading trail as Markdown", "E"),
    (Cmd::ToggleLocal, "Filter: local only / library-inclusive", "L"),
    (Cmd::ToggleGenerated, "Filter: show / hide Lean-generated declarations", "G"),
    (Cmd::CycleKinds, "Filter: cycle declaration kinds", "K"),
    (Cmd::ToggleImplicit, "Statement: show / hide implicit and instance arguments", "i"),
    (Cmd::ToggleSource, "Center: declaration view / whole source file", "s"),
    (Cmd::Back, "History: back", "[  Backspace"),
    (Cmd::Forward, "History: forward", "]"),
    (Cmd::TabUses, "Right pane: what it uses (dependencies)", "1"),
    (Cmd::TabUsedBy, "Right pane: what uses it (dependents)", "2"),
    (Cmd::TabAxioms, "Right pane: axioms / evidence with paths", "3"),
    (Cmd::TabModule, "Right pane: module imports and importers", "4"),
    (Cmd::TabInfo, "Right pane: index provenance and coverage", "5"),
    (Cmd::Transitive, "Count the transitive neighbourhood (bounded)", "t"),
    (Cmd::CheckFreshness, "Check sources against the index now", "R"),
    (Cmd::Theme, "Cycle colour theme (dark/light/high contrast/mono)", "F2"),
    (Cmd::Ascii, "Toggle ASCII UI chrome", "F3"),
    (Cmd::GotoName, "Go to a fully qualified name", "g"),
    (Cmd::OpenEditor, "Open current declaration in the configured editor", "e"),
    (Cmd::ExportHelp, "How to build a semantic index (explains what would run)", ""),
    (Cmd::SemanticExport, "Semantic export… (shows exactly what would run; asks to confirm)", ""),
    (Cmd::HlMode, "Highlighting: cycle syntax + semantic / syntax / off", "F4"),
    (Cmd::HlTheme, "Highlighting: cycle syntax theme (follows the UI theme by default)", "F5"),
    (Cmd::HlStatus, "Highlighting: status details (freshness of this file, indexing progress)", "F6"),
    (Cmd::HlBuild, "Semantic highlighting: build index… (runs Lean on this project; shows what runs, asks to confirm)", ""),
    (Cmd::HlBuildFile, "Semantic highlighting: refresh this file… (the same for the file on screen)", ""),
    (Cmd::HlPause, "Semantic highlighting: pause / resume indexing", ""),
    (Cmd::HlCancel, "Semantic highlighting: cancel indexing (finished files are kept)", ""),
    (Cmd::HlStale, "Semantic highlighting: stale preview on / off (show outdated tokens, labelled)", ""),
    (Cmd::Help, "Help: keys, symbols and what edges mean", "?"),
    (Cmd::Quit, "Quit", "q"),
];

pub struct Ctx {
    pub title: String,
    pub mode: IndexMode,
    pub index: Option<IndexCtx>,
    pub user_key: String,
    pub editor: Option<String>,
    pub info: Option<IndexInfo>,
    pub synthetic: bool,
    pub project_root: Option<PathBuf>,
}

pub struct Channels {
    pub query: Option<Sender<QueryReq>>,
    pub search: Option<Sender<SearchReq>>,
    pub jobs: Option<Sender<JobReq>>,
    /// The UI event channel, for short-lived helper threads (connection search).
    pub events: Option<Sender<Event>>,
    pub search_latest: Arc<AtomicU64>,
    pub job_cancel: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct Gens {
    pub next: u64,
    pub detail: u64,
    pub right: u64,
    pub search: u64,
    pub outline: u64,
    pub source: u64,
    pub marks: u64,
    pub resolve: u64,
    pub info: u64,
    pub vdetail: u64,
    pub goal: u64,
    pub conn: u64,
    pub skim: u64,
    pub imports: u64,
    pub idir: u64,
    pub ipath: u64,
}

impl Gens {
    pub fn bump(&mut self) -> u64 {
        self.next += 1;
        self.next
    }
}

/// Where things were drawn in the last frame (for mouse hit-testing).
#[derive(Clone, Debug, Default)]
pub struct HitMap {
    pub left: ratatui::layout::Rect,
    pub left_input: ratatui::layout::Rect,
    pub left_list: ratatui::layout::Rect,
    pub center: ratatui::layout::Rect,
    pub right: ratatui::layout::Rect,
    pub right_list: ratatui::layout::Rect,
    pub right_tabs: Vec<(u16, u16, RightTab)>,
    /// For each rendered line of the right list: the row it belongs to.
    pub right_line_rows: Vec<Option<usize>>,
    /// Outline and detail panes of the map, goal and trail views.
    pub view_list: ratatui::layout::Rect,
    pub view_detail: ratatui::layout::Rect,
}

pub struct PendingRestore {
    pub right_sel: usize,
    pub right_scroll: usize,
    pub left_sel: usize,
    pub left_scroll: usize,
}

pub struct App {
    pub ctx: Ctx,
    pub theme: Theme,
    pub sym: Symbols,
    pub ascii: bool,
    pub focus: Focus,
    pub left_mode: LeftMode,
    pub search: SearchPane,
    pub outline: OutlinePane,
    pub bookmarks: MarksPane,
    /// The architecture map, goal explorer and reading trails.
    pub v: ViewsState,
    pub center: CenterPane,
    pub right: RightPane,
    pub right_tab: RightTab,
    pub center_mode: CenterMode,
    pub current: Option<i64>,
    pub detail: Option<Box<DetailBundle>>,
    pub detail_error: Option<String>,
    pub history: History,
    pub current_how: String,
    pub filters: Filters,
    pub fresh: Fresh,
    pub job: Option<(String, u64, u64)>,
    pub job_queue: Vec<JobReq>,
    pub search_ready: Option<Result<(usize, u64, bool), String>>,
    pub bookmarked: HashSet<String>,
    pub overlay: Overlay,
    pub status: Option<(String, bool)>,
    pub gens: Gens,
    pub ch: Channels,
    pub quit: bool,
    pub dirty: bool,
    pub preview_session: bool,
    pub pending_restore: Option<PendingRestore>,
    pub pending_goto: Option<String>,
    /// The declaration being opened is shown in the outline when it arrives
    /// (a restored session, a declaration opened from another view).
    pub reveal_on_arrival: bool,
    pub building: bool,
    pub started: Instant,
    pub first_frame_ms: Option<u128>,
    pub last_body_height: usize,
    pub needs_reattach: bool,
    /// A semantic index was just built; switch to it.
    pub switch_to: Option<PathBuf>,
    pub search_ready_at_ms: Option<u128>,
    pub hit: HitMap,
    /// Syntax and semantic highlighting of the source on screen.
    pub hl: super::hl::HlState,
    /// The mode was given on the command line (saved preferences do not apply).
    pub hl_mode_explicit: bool,
    /// When the whole-file source was asked for (timing only).
    pub source_asked_at: Option<Instant>,
    /// Debounced preview of the declaration under the list cursor.
    pub preview_due: Option<Instant>,
    pub preview_target: Option<(i64, String)>,
    /// Centre scroll to apply when the next detail arrives.
    pub pending_center_scroll: Option<usize>,
    /// The reader pressed a key or clicked (start-up defaults stop applying).
    pub user_acted: bool,
}

/// Holding j/k only moves the highlight; the declaration under the cursor
/// is loaded once the cursor has rested this long.
pub const PREVIEW_DELAY: std::time::Duration = std::time::Duration::from_millis(60);

impl App {
    pub fn new(ctx: Ctx, theme: ThemeName, ascii: bool, ch: Channels) -> App {
        App {
            ctx,
            theme: Theme::new(theme),
            sym: Symbols::new(ascii),
            ascii,
            focus: Focus::Left,
            left_mode: LeftMode::Outline,
            search: SearchPane::default(),
            outline: OutlinePane::default(),
            bookmarks: MarksPane::default(),
            v: ViewsState::default(),
            center: CenterPane::default(),
            right: RightPane::default(),
            right_tab: RightTab::Uses,
            center_mode: CenterMode::Decl,
            current: None,
            detail: None,
            detail_error: None,
            history: History::default(),
            current_how: String::new(),
            filters: Filters { local_only: false, show_generated: false, kinds: KindFilter::All, show_implicit: true },
            fresh: Fresh::Unknown,
            job: None,
            job_queue: Vec::new(),
            search_ready: None,
            bookmarked: HashSet::new(),
            overlay: Overlay::None,
            status: None,
            gens: Gens::default(),
            ch,
            quit: false,
            dirty: true,
            preview_session: false,
            pending_restore: None,
            pending_goto: None,
            reveal_on_arrival: false,
            building: false,
            started: crate::PROCESS_START.get().copied().unwrap_or_else(Instant::now),
            first_frame_ms: None,
            last_body_height: 20,
            needs_reattach: false,
            switch_to: None,
            search_ready_at_ms: None,
            hit: HitMap::default(),
            hl: super::hl::HlState::new(Theme::new(theme).default_syntax_theme()),
            hl_mode_explicit: false,
            source_asked_at: None,
            preview_due: None,
            preview_target: None,
            pending_center_scroll: None,
            user_acted: false,
        }
    }

    // ------------------------------------------------------------------
    // Requests
    // ------------------------------------------------------------------

    fn q(&self, r: QueryReq) {
        if let Some(t) = &self.ch.query {
            let _ = t.send(r);
        }
    }

    pub fn msg(&mut self, s: impl Into<String>) {
        self.status = Some((s.into(), false));
    }

    pub fn warn(&mut self, s: impl Into<String>) {
        self.status = Some((s.into(), true));
    }

    pub fn search_params(&self) -> SearchParams {
        SearchParams {
            query: self.search.query.clone(),
            local_only: self.filters.local_only,
            show_generated: self.filters.show_generated,
            kinds: self.filters.kinds,
            limit: 300,
            ..Default::default()
        }
    }

    pub fn run_search(&mut self) {
        self.search.open_on_results = false;
        let gen = self.gens.bump();
        self.gens.search = gen;
        self.ch.search_latest.store(gen, Ordering::Relaxed);
        // Without a query nothing is listed, except inside a scope.
        if self.search.query.trim().is_empty() && self.search.scope.is_none() {
            self.search.items.clear();
            self.search.summary = String::new();
            self.search.pending = false;
            return;
        }
        self.search.pending = true;
        if let Some(t) = &self.ch.search {
            let _ = t.send(SearchReq::Query { gen, params: self.search_params(), docs: true, scope: self.search.scope.clone() });
        }
    }

    /// What `f` would restrict the search to: the directory or file
    /// selected in an outline, else the namespace of the declaration at hand.
    fn scope_at_hand(&self) -> Option<ScopeSpec> {
        let files = |prefix: String| Some(ScopeSpec { kind: ScopeKind::Modules, prefix });
        let of_decl = |display: &str, module: Option<&String>| -> Option<ScopeSpec> {
            let ns = crate::model::namespace_of(display);
            if ns.is_empty() {
                module.and_then(|m| files(m.clone()))
            } else {
                Some(ScopeSpec { kind: ScopeKind::Namespace, prefix: ns.to_string() })
            }
        };
        match self.v.view {
            View::Browse => {
                if self.focus == Focus::Left && self.left_mode == LeftMode::Outline {
                    match self.outline.rows.get(self.outline.list.selected) {
                        Some(OutlineRow::Node(i)) => return files(self.outline.nodes[*i].full.clone()),
                        Some(OutlineRow::Decl { item, .. }) => return item.row.module.clone().and_then(files),
                        _ => {}
                    }
                }
                if self.center_mode == CenterMode::Skim {
                    if let Some(d) = self.center.skim.selected() {
                        return of_decl(&d.row.display, d.row.module.as_ref());
                    }
                }
                self.detail.as_ref().and_then(|d| of_decl(&d.d.row.display, d.d.row.module.as_ref()))
            }
            _ => match self.view_files_at_hand() {
                Some(prefix) => files(prefix),
                None => self.subject_bundle().and_then(|d| of_decl(&d.d.row.display, d.d.row.module.as_ref())),
            },
        }
    }

    /// `f`: search within what is at hand; pressed again, one level wider.
    pub fn cmd_search_scope(&mut self) {
        let hand = self.scope_at_hand();
        let covered = |cur: &ScopeSpec, h: &ScopeSpec| cur.kind == h.kind && ScopeSpec::covers(&cur.prefix, &h.prefix);
        let next = match (&self.search.scope, hand) {
            (Some(cur), Some(h)) if covered(cur, &h) => cur.widened(),
            (_, Some(h)) => Some(h),
            (Some(cur), None) => cur.widened(),
            (None, None) => {
                self.msg("nothing to search within: select a directory or file, or open a declaration");
                return;
            }
        };
        if self.v.view != View::Browse {
            self.set_view(View::Browse);
        }
        self.focus = Focus::Left;
        self.left_mode = LeftMode::Search;
        self.search.editing = true;
        self.search.cursor = self.search.query.chars().count();
        match &next {
            Some(sc) => self.msg(format!("searching within {}", sc.label())),
            None => self.msg("search scope cleared: searching everywhere"),
        }
        self.search.scope = next;
        self.run_search();
    }

    pub fn clear_search_scope(&mut self) {
        if self.search.scope.take().is_some() {
            self.msg("search scope cleared: searching everywhere");
            self.run_search();
        } else {
            self.msg("the search is not restricted");
        }
    }

    // ------------------------------------------------------------------
    // Skimming
    // ------------------------------------------------------------------

    /// Shows the statements of `scope` in the centre pane.
    pub fn request_skim(&mut self, scope: SkimScope, sel: Option<usize>) {
        let gen = self.gens.bump();
        self.gens.skim = gen;
        let sk = &mut self.center.skim;
        if sk.data.as_ref().is_some_and(|d| d.scope != scope) {
            sk.data = None;
            sk.cache = None;
        }
        sk.scope = Some(scope.clone());
        sk.loading = true;
        sk.error = None;
        sk.pending_sel = sel;
        self.center_mode = CenterMode::Skim;
        self.q(QueryReq::Skim { gen, scope, show_generated: self.filters.show_generated });
        self.dirty = true;
    }

    /// What `S` would skim: the selected file, directory or map group,
    /// else the file of the declaration at hand.
    fn skim_scope_at_hand(&self) -> Option<SkimScope> {
        let node_scope = |n: &OutlineNode| if n.children.is_empty() { SkimScope::Module(n.full.clone()) } else { SkimScope::Dir(n.full.clone()) };
        match self.v.view {
            View::Browse => {
                if self.focus == Focus::Left && self.left_mode == LeftMode::Outline {
                    match self.outline.rows.get(self.outline.list.selected) {
                        Some(OutlineRow::Node(i)) => return Some(node_scope(&self.outline.nodes[*i])),
                        Some(OutlineRow::Decl { item, .. }) => return item.row.module.clone().map(SkimScope::Module),
                        _ => {}
                    }
                }
                if self.focus == Focus::Right {
                    if let Some(RRow::Module { module, .. }) = self.right.rows.get(self.right.list.selected) {
                        return Some(SkimScope::Module(module.name.clone()));
                    }
                }
                self.detail.as_ref().and_then(|d| d.d.row.module.clone()).map(SkimScope::Module)
            }
            _ => self.view_skim_at_hand(),
        }
    }

    /// `S`: skim what is at hand. Inside a skim it switches between the
    /// file and the namespace of the selected declaration.
    pub fn cmd_skim(&mut self) {
        if self.v.view == View::Browse && self.center_mode == CenterMode::Skim && self.focus == Focus::Center {
            let Some(d) = self.center.skim.selected().cloned() else {
                self.msg("select a declaration first");
                return;
            };
            let next = match self.center.skim.scope {
                Some(SkimScope::Namespace(_)) => d.row.module.clone().map(SkimScope::Module),
                _ => Some(crate::model::namespace_of(&d.row.name)).filter(|ns| !ns.is_empty()).map(|ns| SkimScope::Namespace(ns.to_string())),
            };
            match next {
                Some(sc) => {
                    self.push_history();
                    self.msg(format!("skimming {}", sc.label()));
                    self.request_skim(sc, None);
                }
                None => self.msg(format!("{} is not inside a namespace", d.row.display)),
            }
            return;
        }
        let Some(scope) = self.skim_scope_at_hand() else {
            self.msg("nothing to skim: select a declaration, file, directory or map group");
            return;
        };
        self.push_history();
        self.v.view = View::Browse;
        self.show_outline();
        self.focus = Focus::Center;
        self.search.editing = false;
        self.preview_due = None;
        self.preview_target = None;
        self.request_skim(scope, None);
    }

    /// Records where the reader is, so that Backspace returns here.
    fn push_history(&mut self) {
        let loc = self.location();
        self.history.back.push(loc);
        if self.history.back.len() > HISTORY_CAP {
            self.history.back.remove(0);
        }
        self.history.fwd.clear();
    }

    /// Moves the skim's selection by `delta` declarations.
    fn skim_move(&mut self, delta: isize) {
        let items = self.center.skim.decl_items();
        if items.is_empty() {
            return;
        }
        let pos = items.partition_point(|&k| k < self.center.skim.sel).min(items.len() - 1);
        let new = (pos as isize + delta).clamp(0, items.len() as isize - 1) as usize;
        self.center.skim.sel = items[new];
    }

    fn on_skim(&mut self, res: Result<Box<Skim>, String>) {
        let sk = &mut self.center.skim;
        sk.loading = false;
        sk.version += 1;
        match res {
            Ok(s) => {
                let first = s.items.iter().position(|i| matches!(i, SkimItem::Decl(_))).unwrap_or(0);
                let same = sk.data.as_ref().is_some_and(|d| d.scope == s.scope);
                let keep = sk.pending_sel.take().or(same.then_some(sk.sel));
                sk.sel = keep.filter(|&k| matches!(s.items.get(k), Some(SkimItem::Decl(_)))).unwrap_or(first);
                if !same {
                    sk.scroll = 0;
                }
                sk.data = Some(s);
                sk.error = None;
            }
            Err(e) => {
                sk.data = None;
                sk.error = Some(e);
            }
        }
    }

    // ------------------------------------------------------------------
    // Outline order
    // ------------------------------------------------------------------

    pub fn cmd_outline_order(&mut self) {
        self.outline.order = self.outline.order.next(self.ctx.mode);
        self.msg(format!("outline: {}", self.outline.order.label(self.ctx.mode)));
        self.rebuild_outline_rows();
        if self.v.view == View::Map {
            self.rebuild_map_rows();
        }
        if !self.outline.loaded && self.v.view == View::Browse && self.left_mode == LeftMode::Outline {
            self.request_outline();
        }
    }

    // ------------------------------------------------------------------
    // Occurrences of the selected mention
    // ------------------------------------------------------------------

    /// The places to highlight in the centre pane: where the identifier of
    /// the selected row of the Mentions tab is written.
    pub fn mention_marks(&self) -> Option<&Arc<Vec<Occurrence>>> {
        if !self.right.marks_on || self.right.stale || self.right_tab != RightTab::Uses {
            return None;
        }
        match self.right.rows.get(self.right.list.selected) {
            Some(RRow::Mention { occ, .. }) if !occ.is_empty() => Some(occ),
            _ => None,
        }
    }

    /// Called after the selection in the right pane moved: highlight the
    /// selected mention's occurrences and bring the first one into view.
    fn sync_marks(&mut self) {
        let first = match self.right.rows.get(self.right.list.selected) {
            Some(RRow::Mention { occ, .. }) => occ.first().map(|o| o.line as usize),
            _ => {
                if std::mem::take(&mut self.right.marks_on) {
                    self.center.version += 1;
                }
                return;
            }
        };
        self.right.marks_on = true;
        // The declaration view caches its lines.
        self.center.version += 1;
        let Some(line) = first else { return };
        match self.center_mode {
            CenterMode::Source => self.reveal_in_source(line),
            CenterMode::Decl => self.center.reveal_line = Some(line),
            CenterMode::Skim => {}
        }
    }

    /// Scrolls the whole-file view so that `line` (1-based) is on screen.
    fn reveal_in_source(&mut self, line: usize) {
        let top = self.center.source_scroll;
        // Wrapped lines take more rows: aim well inside the pane.
        let h = self.last_body_height.saturating_sub(3).max(3) * 2 / 3;
        if line <= top || line > top + h {
            self.center.source_scroll = line.saturating_sub(1 + h / 3);
            self.hl_refresh_view();
        }
    }

    pub fn neighbor_filter(&self) -> NeighborFilter {
        NeighborFilter { local_only: self.filters.local_only, show_generated: self.filters.show_generated, kinds: self.filters.kinds }
    }

    /// (Re)loads the right pane for the current declaration and tab. Rows of
    /// the previous declaration stay visible (dimmed) until the new ones
    /// arrive, so the pane changes once instead of flashing.
    pub fn load_right(&mut self) {
        let gen = self.gens.bump();
        self.gens.right = gen;
        self.right.error = None;
        self.right.expanded.clear();
        self.right.transitive = None;
        self.right.loading = true;
        self.right.stale = !self.right.rows.is_empty();
        let mode = self.ctx.mode;
        match self.right_tab {
            RightTab::Info => {
                self.begin_right_content();
                self.build_info_rows();
                return;
            }
            RightTab::Module => {
                if let Some(mid) = self.detail.as_ref().and_then(|d| d.d.module_id) {
                    self.q(QueryReq::ModuleImports { gen, module_id: mid });
                } else {
                    self.begin_right_content();
                    self.right.rows.push(RRow::Text { text: "No module for this declaration.".into(), warn: false });
                }
                return;
            }
            _ => {}
        }
        let Some(id) = self.current else {
            self.begin_right_content();
            return;
        };
        match (self.right_tab, mode) {
            (RightTab::Uses, IndexMode::Semantic) => self.q(QueryReq::Neighbors {
                gen,
                id,
                dir: Direction::Uses,
                filter: self.neighbor_filter(),
                limit: 100,
                offset: 0,
                parent: None,
            }),
            (RightTab::UsedBy, IndexMode::Semantic) => self.q(QueryReq::Neighbors {
                gen,
                id,
                dir: Direction::UsedBy,
                filter: self.neighbor_filter(),
                limit: 100,
                offset: 0,
                parent: None,
            }),
            (RightTab::Axioms, IndexMode::Semantic) => self.q(QueryReq::Evidence { gen, id }),
            (RightTab::Uses, IndexMode::Source) => self.q(QueryReq::Mentions { gen, id }),
            (RightTab::UsedBy, IndexMode::Source) => {
                self.begin_right_content();
                self.right.rows.push(RRow::Header { text: "Used by: not available".into(), kind: None });
                self.right.rows.push(RRow::Text { text: "a source-only index has no dependency edges; a semantic snapshot has them".into(), warn: false });
            }
            (RightTab::Axioms, IndexMode::Source) => {
                self.begin_right_content();
                self.build_source_evidence_rows();
            }
            _ => {}
        }
    }

    /// New content for the right pane is about to be written: drop the
    /// previous rows and selection (unless a history restore is pending).
    fn begin_right_content(&mut self) {
        self.right.rows.clear();
        self.right.loading = false;
        self.right.stale = false;
        self.right.error = None;
        if std::mem::take(&mut self.right.marks_on) {
            self.center.version += 1;
        }
        self.center.reveal_line = None;
        if self.pending_restore.is_none() {
            self.right.list = ListState::default();
        }
    }

    pub fn load_detail(&mut self) {
        let Some(id) = self.current else { return };
        let gen = self.gens.bump();
        self.gens.detail = gen;
        self.q(QueryReq::Detail { gen, id });
    }

    /// Resolves a fully qualified name and opens it when found.
    pub fn goto_name(&mut self, name: String) {
        let gen = self.gens.bump();
        self.gens.resolve = gen;
        self.pending_goto = Some(name.clone());
        self.q(QueryReq::ResolveName { gen, name });
    }

    /// Resets panes for a different index (used after a semantic export).
    pub fn reset_for_index(&mut self, ictx: IndexCtx) {
        let name = self.detail.as_ref().map(|d| d.d.row.name.clone());
        self.ctx.mode = ictx.mode;
        self.ctx.index = Some(ictx);
        self.ctx.info = None;
        self.search = SearchPane { query: self.search.query.clone(), cursor: self.search.cursor, scope: self.search.scope.clone(), ..Default::default() };
        self.outline = OutlinePane { order: self.outline.order, ..Default::default() };
        self.center.skim = SkimPane::default();
        if self.center_mode == CenterMode::Skim {
            self.center_mode = CenterMode::Decl;
        }
        self.right = RightPane::default();
        self.history = History::default();
        self.cancel_connection();
        let goal = self.v.goal.goal.take();
        self.v = ViewsState::default();
        self.v.goal.goal = goal;
        self.v.goal.restored = true;
        self.current = None;
        self.detail = None;
        self.fresh = Fresh::Unknown;
        self.search_ready = None;
        self.center.version += 1;
        if let Some(n) = name {
            self.pending_goto = Some(n);
        }
    }

    // ------------------------------------------------------------------
    // Navigation and history
    // ------------------------------------------------------------------

    pub fn location(&self) -> Location {
        let (left_sel, left_scroll) = match self.left_mode {
            LeftMode::Search => (self.search.list.selected, self.search.list.scroll),
            LeftMode::Outline => (self.outline.list.selected, self.outline.list.scroll),
            LeftMode::Bookmarks => (self.bookmarks.list.selected, self.bookmarks.list.scroll),
            LeftMode::Trail => (0, 0),
        };
        Location {
            view: self.v.view,
            vkey: self.view_key(),
            decl: self.current,
            decl_name: self.detail.as_ref().map(|d| d.d.row.name.clone()).unwrap_or_default(),
            focus: self.focus,
            left_mode: self.left_mode,
            query: self.search.query.clone(),
            left_sel,
            left_scroll,
            center_mode: self.center_mode,
            center_scroll: self.center.scroll,
            right_tab: self.right_tab,
            right_sel: self.right.list.selected,
            right_scroll: self.right.list.scroll,
            how: self.current_how.clone(),
            skim: if self.center_mode == CenterMode::Skim { self.center.skim.scope.clone() } else { None },
            skim_sel: self.center.skim.sel,
            scope: self.search.scope.clone(),
        }
    }

    /// Opens a declaration. `push` records the previous location in the
    /// back history; previews from the left list replace instead.
    pub fn open(&mut self, id: i64, how: String, push: bool) {
        if self.current == Some(id) && !push {
            return;
        }
        // A skim is a place to come back to even with no declaration open.
        if push && (self.current.is_some() || self.center_mode == CenterMode::Skim) {
            self.push_history();
        }
        self.current = Some(id);
        self.current_how = how;
        self.pending_center_scroll = Some(0);
        self.center_mode = CenterMode::Decl;
        self.center.source = None;
        self.detail_error = None;
        self.preview_due = None;
        self.preview_target = None;
        self.load_detail();
        if self.right_tab != RightTab::Module && self.right_tab != RightTab::Info {
            self.load_right();
        }
        self.dirty = true;
    }

    fn restore(&mut self, loc: Location) {
        self.focus = loc.focus;
        // Milestone 1 had a trail list in the left pane; it is a view now.
        self.left_mode = if loc.left_mode == LeftMode::Trail { LeftMode::Search } else { loc.left_mode };
        self.restore_view(loc.view, loc.vkey.clone());
        self.show_outline();
        if self.search.query != loc.query || self.search.scope != loc.scope {
            self.search.query = loc.query.clone();
            self.search.scope = loc.scope.clone();
            self.search.cursor = self.search.query.chars().count();
            self.run_search();
        }
        self.right_tab = loc.right_tab;
        self.center_mode = loc.center_mode;
        if loc.center_mode == CenterMode::Skim {
            match loc.skim.clone() {
                Some(sc) => self.request_skim(sc, Some(loc.skim_sel)),
                None => self.center_mode = CenterMode::Decl,
            }
        }
        self.current_how = loc.how.clone();
        self.pending_restore = Some(PendingRestore {
            right_sel: loc.right_sel,
            right_scroll: loc.right_scroll,
            left_sel: loc.left_sel,
            left_scroll: loc.left_scroll,
        });
        match loc.left_mode {
            LeftMode::Search => {
                self.search.list = ListState { selected: loc.left_sel, scroll: loc.left_scroll };
                self.search.editing = false;
            }
            LeftMode::Outline => self.outline.list = ListState { selected: loc.left_sel, scroll: loc.left_scroll },
            LeftMode::Bookmarks => self.bookmarks.list = ListState { selected: loc.left_sel, scroll: loc.left_scroll },
            LeftMode::Trail => {}
        }
        match loc.decl {
            Some(id) => {
                self.current = Some(id);
                self.pending_center_scroll = Some(loc.center_scroll);
                self.center.source = None;
                self.preview_due = None;
                self.preview_target = None;
                self.load_detail();
                self.load_right();
            }
            None if !loc.decl_name.is_empty() => {
                let gen = self.gens.bump();
                self.gens.resolve = gen;
                self.pending_goto = Some(loc.decl_name.clone());
                self.q(QueryReq::ResolveName { gen, name: loc.decl_name });
            }
            None => {
                self.current = None;
                self.detail = None;
            }
        }
        self.dirty = true;
    }

    pub fn back(&mut self) {
        let Some(loc) = self.history.back.pop() else {
            self.msg("history: nothing further back");
            return;
        };
        let cur = self.location();
        self.history.fwd.push(cur);
        self.preview_session = false;
        self.restore(loc);
    }

    pub fn forward(&mut self) {
        let Some(loc) = self.history.fwd.pop() else {
            self.msg("history: nothing forward");
            return;
        };
        let cur = self.location();
        self.history.back.push(cur);
        self.preview_session = false;
        self.restore(loc);
    }

    /// The reading route: how the reader got here, newest last.
    pub fn route(&self, max: usize) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self
            .history
            .back
            .iter()
            .rev()
            // Steps through the map, goal or trail views have no declaration.
            .filter(|l| !l.decl_name.is_empty())
            .take(max.saturating_sub(1))
            .map(|l| (l.how.clone(), short_name(&l.decl_name)))
            .collect();
        v.reverse();
        if let Some(d) = &self.detail {
            v.push((self.current_how.clone(), short_name(&d.d.row.display)));
        }
        v
    }

    // ------------------------------------------------------------------
    // Events
    // ------------------------------------------------------------------

    pub fn handle(&mut self, ev: Event) {
        match ev {
            Event::Input(e) => self.on_input(e),
            Event::Query(r) => self.on_query(r),
            Event::Search(r) => self.on_search(r),
            Event::Job(j) => self.on_job(j),
            Event::Highlight(r) => self.on_highlight(r),
            Event::SemIndex(e) => self.on_sem_event(e),
            Event::Signal(_) => self.quit = true,
            Event::InputClosed => self.quit = true,
        }
    }

    fn on_search(&mut self, r: SearchResp) {
        match r {
            SearchResp::Ready { entries, ms, rebuilt, .. } => {
                self.dirty = true;
                self.search_ready = Some(Ok((entries, ms, rebuilt)));
                self.search_ready_at_ms = Some(self.started.elapsed().as_millis());
                if !self.search.query.is_empty() {
                    self.run_search();
                }
            }
            SearchResp::Failed(e) => {
                self.dirty = true;
                self.search_ready = Some(Err(e));
            }
            SearchResp::Results(res) => {
                if res.gen != self.gens.search {
                    return;
                }
                self.search.pending = false;
                // Search spans the project map too: groups, objectives and
                // landmark placements come first, under their own heading.
                let listing = res.query.trim().is_empty();
                let map_hits = if listing { Vec::new() } else { self.map().map(|m| m.search(&res.query, 8)).unwrap_or_default() };
                let mut items: Vec<LeftItem> = Vec::new();
                if !map_hits.is_empty() {
                    items.push(LeftItem::Section(format!("project map ({})", map_hits.len())));
                    items.extend(map_hits.iter().map(|h| LeftItem::MapHit { key: h.key.clone(), label: h.label.clone(), what: h.what }));
                    items.push(LeftItem::Section("declarations".into()));
                }
                let n_map = map_hits.len();
                items.extend(res.rows.iter().zip(&res.outcome.hits).map(|(r, h)| LeftItem::Result { row: r.clone(), tier: Some(h.tier) }));
                if !res.doc_rows.is_empty() {
                    items.push(LeftItem::Section(format!("documentation / statement text ({})", res.doc_rows.len())));
                    items.extend(res.doc_rows.iter().map(|r| LeftItem::Result { row: r.clone(), tier: None }));
                }
                let o = &res.outcome;
                let mut parts = vec![if res.scope_empty {
                    "no local file has that name".to_string()
                } else if listing {
                    format!("{}{} in scope", if o.total_capped { "≥" } else { "" }, o.total)
                } else {
                    format!("{}{} name match{}", if o.total_capped { "≥" } else { "" }, o.total, if o.total == 1 { "" } else { "es" })
                }];
                if o.hits.len() < o.total {
                    parts.push(format!("top {} shown", o.hits.len()));
                }
                if !res.doc_rows.is_empty() {
                    parts.push(format!("{} text", res.doc_rows.len()));
                }
                if o.filtered_out > 0 {
                    parts.push(format!("{} hidden by filters", o.filtered_out));
                }
                if o.fuzzy_truncated {
                    parts.push("fuzzy stopped at time budget".into());
                }
                if self.map().is_some() && !listing {
                    parts.push(format!("{n_map} in map"));
                }
                parts.push(format!("{:.0} ms", o.elapsed.as_secs_f64() * 1000.0 + res.doc_ms));
                self.search.summary = parts.join(" · ");
                self.search.items = items;
                if let Some(p) = self.pending_restore.as_ref().filter(|_| self.left_mode == LeftMode::Search) {
                    self.search.list = ListState { selected: p.left_sel, scroll: p.left_scroll };
                } else {
                    // Map entries are listed first, but Enter keeps opening the
                    // best declaration match: start on the first declaration.
                    let first = self.search.items.iter().position(|i| matches!(i, LeftItem::Result { .. }));
                    self.search.list = ListState { selected: first.unwrap_or(0), scroll: 0 };
                    self.skip_to_selectable_left(true);
                }
                if std::mem::take(&mut self.search.open_on_results) && !self.search.items.is_empty() {
                    self.open_left_selection(true);
                    self.focus = Focus::Center;
                }
                self.dirty = true;
            }
        }
    }

    fn on_job(&mut self, j: JobEvent) {
        match j {
            JobEvent::Progress { label, done, total } => self.job = Some((label, done, total)),
            JobEvent::Freshness(fr) => {
                self.job = None;
                if fr.is_fresh() {
                    self.fresh = Fresh::Fresh { checked: fr.checked, ms: fr.elapsed.as_millis() };
                    if !fr.touched.is_empty() && self.ctx.mode == IndexMode::Source {
                        // metadata only: record new mtimes quietly
                        self.job_queue.push(JobReq::Rescan { fr: fr.clone() });
                    }
                } else if self.ctx.mode == IndexMode::Source && fr.env_changed.is_empty() {
                    let n = fr.changed.len() + fr.deleted.len() + fr.added.len();
                    self.msg(format!("{n} source files changed since indexing; updating the source index in the background"));
                    self.fresh = Fresh::Stale(fr.clone());
                    self.job_queue.push(JobReq::Rescan { fr });
                } else {
                    self.fresh = Fresh::Stale(fr);
                }
                self.run_next_job();
            }
            JobEvent::IndexReplaced(s) => {
                self.job = None;
                self.msg(s);
                if self.building {
                    self.building = false;
                    self.needs_reattach = true;
                } else {
                    self.q(QueryReq::Reopen);
                    if let Some(t) = &self.ch.search {
                        let _ = t.send(SearchReq::Reload);
                    }
                    self.fresh = Fresh::Unknown;
                    // The worker resolves the map again by itself; stops and
                    // the unfolded goal outline refer to the old index.
                    self.q(QueryReq::LoadReading);
                    self.v.detail.bundle_name.clear();
                    self.v.detail.want = None;
                    if self.v.goal.goal.is_some() {
                        self.reset_goal_tree();
                    }
                    self.outline = OutlinePane { order: self.outline.order, ..Default::default() };
                    // The import graph and a skim describe the old index.
                    self.reset_imports();
                    if self.center_mode == CenterMode::Skim {
                        if let Some(sc) = self.center.skim.scope.clone() {
                            self.request_skim(sc, Some(self.center.skim.sel));
                        }
                    }
                    // Map rows of the file tree point into the outline just dropped.
                    self.rebuild_map_rows();
                    if self.v.map.expanded.contains("sec:files") {
                        self.request_outline();
                    }
                    if self.left_mode == LeftMode::Outline {
                        self.request_outline();
                    }
                    if let Some(name) = self.detail.as_ref().map(|d| d.d.row.name.clone()) {
                        let gen = self.gens.bump();
                        self.gens.resolve = gen;
                        self.pending_goto = Some(name.clone());
                        self.q(QueryReq::ResolveName { gen, name });
                    }
                }
                self.run_next_job();
            }
            JobEvent::ExportPlanned(r) => {
                self.job = None;
                match r {
                    Ok(plan) => self.overlay = Overlay::ConfirmExport { plan, typed: String::new() },
                    Err(e) => self.warn(format!("semantic export not possible: {e}")),
                }
            }
            JobEvent::ExportDone(idx, msg) => {
                self.job = None;
                self.msg(format!("{msg}; switching to the semantic index"));
                self.switch_to = Some(idx);
            }
            JobEvent::Failed(e) => {
                self.job = None;
                if matches!(self.fresh, Fresh::Checking) {
                    self.fresh = Fresh::Error(e.clone());
                }
                self.warn(format!("background job failed: {e}"));
                self.run_next_job();
            }
            JobEvent::Cancelled => {
                self.job = None;
                self.msg("background job cancelled");
            }
        }
        self.dirty = true;
    }

    pub fn run_next_job(&mut self) {
        if self.job.is_some() || self.job_queue.is_empty() {
            return;
        }
        let j = self.job_queue.remove(0);
        if let JobReq::Freshness { .. } = j {
            self.fresh = Fresh::Checking;
        }
        self.job = Some((
            match &j {
                JobReq::Freshness { .. } => "checking freshness".into(),
                JobReq::Rescan { .. } => "updating source index".into(),
                JobReq::BuildSource => "building source index".into(),
                JobReq::ExportPlan => "planning semantic export".into(),
                JobReq::ExportRun => "semantic export (sandboxed)".into(),
                JobReq::Shutdown => String::new(),
            },
            0,
            0,
        ));
        if let Some(t) = &self.ch.jobs {
            let _ = t.send(j);
        }
    }

    fn on_query(&mut self, r: QueryResp) {
        // Stale responses `return` early and do not cause a redraw.
        match r {
            QueryResp::Detail(gen, res) => {
                if gen != self.gens.detail {
                    return;
                }
                match res {
                    Ok(b) => {
                        self.detail = Some(b);
                        self.detail_error = None;
                        self.center.version += 1;
                        if let Some(sc) = self.pending_center_scroll.take() {
                            self.center.scroll = sc;
                        }
                        if std::mem::take(&mut self.reveal_on_arrival) && self.v.view == View::Browse && self.left_mode == LeftMode::Outline {
                            let row = self.detail.as_ref().map(|d| &d.d.row).filter(|r| self.ctx.mode == IndexMode::Source || r.origin == Origin::Local);
                            if let Some((module, id)) = row.and_then(|r| r.module.clone().map(|m| (m, r.id))) {
                                self.outline.reveal = Some(Reveal { module, decl: Some(id) });
                                self.apply_reveal();
                            }
                        }
                        if self.right_tab == RightTab::Module || self.right_tab == RightTab::Info {
                            self.load_right();
                        }
                        if self.center_mode == CenterMode::Source {
                            self.request_full_source();
                        } else {
                            self.hl_refresh_view();
                        }
                    }
                    Err(e) => {
                        self.detail_error = Some(e);
                        self.center.version += 1;
                    }
                }
            }
            QueryResp::Neighbors(gen, parent, offset, res) => {
                if gen != self.gens.right {
                    return;
                }
                if parent.is_none() && offset == 0 {
                    self.begin_right_content();
                }
                self.right.loading = false;
                match res {
                    Ok(groups) => self.apply_neighbors(parent, offset, groups),
                    Err(e) => self.right.error = Some(e),
                }
            }
            QueryResp::Mentions(gen, res) => {
                if gen != self.gens.right {
                    return;
                }
                self.begin_right_content();
                match res {
                    Ok(ms) => {
                        // What a mention is and is not is in the help and the docs.
                        self.right.rows.push(RRow::Header { text: "Mentions (lexical, not resolved by Lean)".into(), kind: None });
                        if ms.is_empty() {
                            self.right.rows.push(RRow::Text { text: "no identifier equals an indexed name".into(), warn: false });
                        }
                        for m in ms {
                            let occ = Arc::new(m.occurrences);
                            for row in m.candidates {
                                self.right.rows.push(RRow::Mention { ident: m.ident.clone(), row, occ: occ.clone() });
                            }
                        }
                        self.apply_right_restore();
                    }
                    Err(e) => self.right.error = Some(e),
                }
            }
            QueryResp::Evidence(gen, res) => {
                if gen != self.gens.right {
                    return;
                }
                self.begin_right_content();
                match res {
                    Ok(ev) => self.build_evidence_rows(ev),
                    Err(e) => self.right.error = Some(e),
                }
            }
            QueryResp::ModuleImports(gen, res) => {
                if gen != self.gens.right {
                    return;
                }
                self.begin_right_content();
                match res {
                    Ok((m, imps, nimp, by, nby, stats)) => {
                        if let Some(m) = &m {
                            self.right.rows.push(RRow::Header { text: format!("Module {}", m.name), kind: None });
                            if m.is_local {
                                self.right.rows.push(RRow::Module { module: m.clone(), flags: 0, current: true });
                            }
                            let mut info = format!("{} declarations", m.decl_count);
                            if let Some(p) = &m.path {
                                info.push_str(&format!(" · {p}"));
                            }
                            if m.status != "ok" {
                                info.push_str(&format!(" · status: {}", m.status));
                            }
                            self.right.rows.push(RRow::Text { text: info, warn: m.status != "ok" });
                            if let Some(st) = stats.filter(|_| m.is_local) {
                                let mut line = crate::skim::counts_line(&st, self.ctx.mode);
                                if st.lines > 0 {
                                    line.push_str(&format!(" · {} lines", st.lines));
                                }
                                if st.proof_lines > 0 {
                                    line.push_str(&format!(", {} of them in proofs", st.proof_lines));
                                }
                                self.right.rows.push(RRow::Text { text: line, warn: false });
                            }
                            if let Some(d) = &m.status_detail {
                                if let Ok(v) = serde_json::from_str::<Vec<serde_json::Value>>(d) {
                                    for it in v.iter().take(6) {
                                        self.right.rows.push(RRow::Text {
                                            text: format!(
                                                "line {}: {}",
                                                it.get("line").and_then(|x| x.as_u64()).unwrap_or(0),
                                                it.get("message").and_then(|x| x.as_str()).unwrap_or("")
                                            ),
                                            warn: true,
                                        });
                                    }
                                }
                            }
                            self.right.module_doc = m.doc.clone();
                            if let Some(doc) = &m.doc {
                                let first: String = doc.lines().filter(|l| !l.trim().is_empty()).take(3).collect::<Vec<_>>().join(" ");
                                self.right.rows.push(RRow::Text { text: first, warn: false });
                            }
                        }
                        self.right.rows.push(RRow::Header {
                            text: format!("imports ({nimp}) — module header `import` lines"),
                            kind: None,
                        });
                        for e in imps {
                            self.right.rows.push(RRow::Module { module: e.module, flags: e.flags, current: false });
                        }
                        self.right.rows.push(RRow::Header { text: format!("imported by ({nby})"), kind: None });
                        for e in by {
                            self.right.rows.push(RRow::Module { module: e.module, flags: e.flags, current: false });
                        }
                        self.apply_right_restore();
                    }
                    Err(e) => self.right.error = Some(e),
                }
            }
            QueryResp::FullSource(gen, res) => {
                if gen != self.gens.source {
                    return;
                }
                match res {
                    Ok(fs) => {
                        self.center.source_scroll = fs.focus_line.saturating_sub(4);
                        self.center.source = Some(fs);
                        // A mention selected before the file was opened is shown at once.
                        if let Some(line) = self.mention_marks().and_then(|o| o.first()).map(|o| o.line as usize) {
                            self.reveal_in_source(line);
                        }
                        if let Some(t) = self.source_asked_at.take() {
                            super::timing::record("source: file asked to text on the UI thread", t.elapsed().as_secs_f64() * 1000.0);
                        }
                        self.hl_refresh_view();
                    }
                    Err(e) => {
                        self.warn(format!("cannot show source: {e}"));
                        self.center_mode = CenterMode::Decl;
                    }
                }
            }
            QueryResp::ModuleTree(gen, res) => {
                if gen != self.gens.outline {
                    return;
                }
                match res {
                    Ok(tree) => {
                        let t = *tree;
                        self.outline.nodes = t.nodes;
                        self.outline.roots = t.roots;
                        self.outline.by_name = t.by_name;
                        self.outline.loaded = true;
                        self.rebuild_outline_rows();
                        self.apply_reveal();
                        if self.v.view == View::Map {
                            self.rebuild_map_rows();
                        }
                    }
                    Err(e) => {
                        self.outline.requested = false;
                        self.warn(format!("outline: {e}"));
                    }
                }
            }
            QueryResp::ModuleDecls(gen, mid, res) => {
                let _ = gen;
                self.outline.pending_decls.remove(&mid);
                match res {
                    Ok(v) => {
                        self.outline.module_decls.insert(mid, v);
                        self.rebuild_outline_rows();
                        self.apply_reveal();
                    }
                    Err(e) => self.warn(format!("outline: {e}")),
                }
            }
            QueryResp::Rows(_, _) => {}
            QueryResp::Resolved(gen, name, id) => {
                if gen != self.gens.resolve {
                    return;
                }
                if self.pending_goto.as_deref() == Some(name.as_str()) {
                    self.pending_goto = None;
                    match id {
                        Some(id) => self.open(id, if self.current_how.is_empty() { "go to".into() } else { self.current_how.clone() }, false),
                        None => self.warn(format!("`{name}` is not in this index (renamed or removed?)")),
                    }
                }
            }
            QueryResp::TransitiveCount(gen, dir, res) => {
                if gen != self.gens.right {
                    return;
                }
                match res {
                    Ok((n, capped)) => {
                        self.right.transitive = Some((dir, n, capped));
                        self.msg(format!(
                            "transitive {}: {}{} declarations (all edge kinds{})",
                            if dir == Direction::Uses { "dependencies" } else { "dependents" },
                            if capped { "≥" } else { "" },
                            n,
                            if capped { "; bounded walk stopped" } else { "" }
                        ));
                    }
                    Err(e) => self.warn(e),
                }
            }
            QueryResp::Info(gen, res) => {
                if gen == self.gens.info {
                    if let Ok(i) = res {
                        self.ctx.synthetic = i.get("synthetic") == Some("1");
                        self.ctx.info = Some(i);
                        if self.v.view == View::Map {
                            self.rebuild_map_rows();
                        }
                        if self.right_tab == RightTab::Info {
                            self.load_right();
                        }
                    }
                }
            }
            QueryResp::User(_, data, msg) => {
                if !msg.is_empty() {
                    self.msg(msg);
                }
                match data {
                    UserData::Bookmarks(v) => {
                        self.bookmarked = v.iter().map(|m| m.name.clone()).collect();
                        self.bookmarks.marks = v;
                        self.bookmarks.loaded = true;
                    }
                    UserData::Reading(r) => self.on_reading(*r),
                    UserData::BookmarkNames(v) => {
                        self.v.detail.version += 1;
                        self.bookmarked = v.into_iter().collect();
                        if let Some(d) = &mut self.detail {
                            d.bookmarked = self.bookmarked.contains(&d.d.row.name);
                        }
                        self.center.version += 1;
                        let gen = self.gens.bump();
                        self.q(QueryReq::LoadBookmarks { gen });
                    }
                }
            }
            QueryResp::Map(b) => self.on_map_bundle(*b),
            QueryResp::Skim(gen, res) => {
                if gen != self.gens.skim {
                    return;
                }
                self.on_skim(res);
            }
            QueryResp::Imports(gen, res) => {
                if !self.on_imports(gen, res) {
                    return;
                }
            }
            QueryResp::ImportsDir(gen, res) => {
                if !self.on_imports_dir(gen, res) {
                    return;
                }
            }
            QueryResp::ImportPath(gen, res) => {
                if !self.on_import_path(gen, res) {
                    return;
                }
            }
            QueryResp::ViewDetail(gen, name, res) => {
                if !self.on_view_detail(gen, name, res) {
                    return;
                }
            }
            QueryResp::GoalChildren(gen, node, offset, res) => {
                if !self.on_goal_children(gen, node, offset, res) {
                    return;
                }
            }
            QueryResp::Connection(gen, res) => {
                if !self.on_connection(gen, *res) {
                    return;
                }
            }
            QueryResp::Message(m) => {
                if !m.is_empty() {
                    self.msg(m);
                }
            }
            QueryResp::Error(e) => self.warn(e),
        }
        self.dirty = true;
    }

    fn apply_right_restore(&mut self) {
        if let Some(p) = self.pending_restore.take() {
            self.right.list = ListState { selected: p.right_sel, scroll: p.right_scroll };
            if self.left_mode != LeftMode::Search {
                // left list state was restored directly
            }
        } else {
            self.skip_to_selectable_right(true);
        }
    }

    fn apply_neighbors(&mut self, parent: Option<u64>, offset: usize, groups: Vec<NeighborGroup>) {
        let dir = if self.right_tab == RightTab::UsedBy { Direction::UsedBy } else { Direction::Uses };
        match parent {
            None if offset == 0 => {
                self.right.rows.clear();
                let id = self.current.unwrap_or(0);
                if groups.is_empty() {
                    let cov = self.detail.as_ref().map(|d| (d.d.type_cov, d.d.value_cov));
                    let txt = match (dir, cov) {
                        (Direction::Uses, Some((t, v))) if t.is_incomplete() || v.is_incomplete() => {
                            "none recorded; coverage is incomplete, so this is not evidence of none".to_string()
                        }
                        (Direction::Uses, _) => "no constants in its type or value (coverage complete)".into(),
                        (Direction::UsedBy, _) => "nothing in this snapshot refers to it".into(),
                    };
                    self.right.rows.push(RRow::Text { text: txt, warn: false });
                }
                for g in groups {
                    self.push_group(g, 0, id, None, dir);
                }
                self.apply_right_restore();
            }
            None => {
                // next page of a top-level group: replace its More row
                if let Some(g) = groups.into_iter().next() {
                    let pos = self.right.rows.iter().position(|r| matches!(r, RRow::More { parent: None, kind, offset: o, .. } if *kind == g.kind && *o == offset));
                    if let Some(pos) = pos {
                        self.right.rows.remove(pos);
                        let id = self.current.unwrap_or(0);
                        let new_rows = self.group_items(&g, 0, id, None, dir, offset);
                        for (k, r) in new_rows.into_iter().enumerate() {
                            self.right.rows.insert(pos + k, r);
                        }
                    }
                }
            }
            Some(pkey) => {
                let Some(pos) = self.right.rows.iter().position(|r| matches!(r, RRow::Item { key, .. } if *key == pkey)) else { return };
                let (depth, pid) = match &self.right.rows[pos] {
                    RRow::Item { depth, row, .. } => (*depth, row.id),
                    _ => return,
                };
                let mut ins = Vec::new();
                if groups.is_empty() {
                    ins.push(RRow::Text { text: format!("{}(no {} recorded)", "  ".repeat(depth as usize + 1), if dir == Direction::Uses { "dependencies" } else { "dependents" }), warn: false });
                }
                for g in groups {
                    ins.extend(self.group_items(&g, depth + 1, pid, Some(pkey), dir, 0));
                }
                for (k, r) in ins.into_iter().enumerate() {
                    self.right.rows.insert(pos + 1 + k, r);
                }
            }
        }
    }

    fn ancestors_of(&self, depth: u16, parent_id: i64) -> HashSet<i64> {
        let mut s: HashSet<i64> = self.right.expanded.values().copied().collect();
        s.insert(parent_id);
        if let Some(c) = self.current {
            s.insert(c);
        }
        let _ = depth;
        s
    }

    fn group_items(&mut self, g: &NeighborGroup, depth: u16, parent_id: i64, parent: Option<u64>, dir: Direction, offset: usize) -> Vec<RRow> {
        let anc = self.ancestors_of(depth, parent_id);
        let mut v = Vec::new();
        for r in &g.rows {
            self.right.next_key += 1;
            let cycle = depth > 0 && anc.contains(&r.id);
            v.push(RRow::Item { key: self.right.next_key, row: r.clone(), kind: g.kind, depth, parent_id, expanded: false, cycle, dir });
        }
        if g.truncated {
            let shown = offset + g.rows.len();
            v.push(RRow::More {
                kind: g.kind,
                parent,
                parent_id,
                depth,
                offset: shown,
                dir,
                remaining: g.matching.saturating_sub(shown as u64),
            });
        }
        v
    }

    fn push_group(&mut self, g: NeighborGroup, depth: u16, parent_id: i64, parent: Option<u64>, dir: Direction) {
        let what = match (dir, g.kind) {
            (Direction::Uses, EdgeKind::Type) => "in its statement",
            (Direction::Uses, EdgeKind::Value) => "in its proof / body",
            (Direction::Uses, EdgeKind::Ctor) => "its constructors",
            (Direction::UsedBy, EdgeKind::Type) => "mention it in their statement",
            (Direction::UsedBy, EdgeKind::Value) => "use it in a proof / body",
            (Direction::UsedBy, EdgeKind::Ctor) => "its inductive type",
        };
        let count = if g.matching == g.total {
            format!("{}", g.total)
        } else {
            format!("{}{} of {} after filters", if g.matching_capped { "≥" } else { "" }, g.matching, g.total)
        };
        self.right.rows.push(RRow::Header { text: format!("[{}] {what} — {count}", g.kind.tag()), kind: Some(g.kind) });
        let items = self.group_items(&g, depth, parent_id, parent, dir, 0);
        self.right.rows.extend(items);
    }

    pub fn build_evidence_rows(&mut self, ev: Vec<AxiomEvidence>) {
        use crate::model::AxiomCategory as C;
        let d = self.detail.as_ref().map(|b| b.d.clone());
        self.right.rows.clear();
        if ev.is_empty() {
            self.right.rows.push(RRow::Header { text: "No axioms reachable".into(), kind: None });
            self.right.rows.push(RRow::Text { text: "through T, V and C edges; no coverage gap".into(), warn: false });
        }
        for e in ev {
            let mark = if e.axiom.category == C::Standard { self.sym.bullet } else { self.sym.warn };
            let name = if e.axiom.category == C::Incomplete { "coverage gap".to_string() } else { e.axiom.name.clone() };
            self.right.rows.push(RRow::Header { text: format!("{mark} {name} · {}", e.axiom.category.label()), kind: None });
            let mut prev: Option<EdgeKind> = None;
            for (i, st) in e.path.iter().enumerate() {
                self.right.rows.push(RRow::Evidence {
                    row: st.row.clone(),
                    step: i,
                    kind_in: prev,
                    kind_next: st.next_kind,
                    axiom: e.axiom.name.clone(),
                    category: e.axiom.category,
                });
                prev = st.next_kind;
            }
            if e.path_truncated {
                self.right.rows.push(RRow::Text { text: "path truncated".into(), warn: true });
            }
        }
        if let Some(d) = d {
            if let Some(la) = &d.lean_axioms {
                let mut ours: Vec<String> = d.axioms.iter().filter(|a| a.category != C::Incomplete).map(|a| a.name.clone()).collect();
                ours.sort();
                let mut la_s = la.clone();
                la_s.sort();
                let same = ours == la_s;
                self.right.rows.push(RRow::Header { text: "Lean's own report".into(), kind: None });
                self.right.rows.push(RRow::Text {
                    text: if same {
                        format!("{} collectAxioms at export: same set", self.sym.ok)
                    } else {
                        format!("{} collectAxioms at export: [{}] (Lean < 4.28 skips axiom types)", self.sym.warn, la.join(", "))
                    },
                    warn: !same,
                });
            }
        }
        self.apply_right_restore();
    }

    fn build_source_evidence_rows(&mut self) {
        self.right.rows.clear();
        self.right.rows.push(RRow::Header { text: "Source-only observations (lexical)".into(), kind: None });
        let Some(b) = &self.detail else { return };
        let d = &b.d;
        let mut lines: Vec<(String, bool)> = Vec::new();
        if d.row.kind == crate::model::DeclKind::Axiom {
            lines.push((format!("{} declared with the `axiom` keyword: assumed, not proved", self.sym.warn), true));
        }
        if d.row.flags & flags::LEX_SORRY != 0 {
            lines.push((
                format!("{} a `sorry`/`admit` token occurs in its text (line {})", self.sym.warn, d.sorry_line.map(|l| l.to_string()).unwrap_or_default()),
                true,
            ));
        } else {
            lines.push((
"no `sorry` token in its text (proves nothing)".into(),
                false,
            ));
        }
        if d.row.flags & flags::LEX_NATIVE != 0 {
            lines.push((format!("{} uses `native_decide` (trusts the compiler)", self.sym.warn), true));
        }
        lines.push(("dependencies and axioms: unknown without a semantic snapshot".into(), false));
        for (t, w) in lines {
            self.right.rows.push(RRow::Text { text: t, warn: w });
        }
    }

    pub fn build_info_rows(&mut self) {
        self.right.rows.clear();
        let mode = self.ctx.mode;
        self.right.rows.push(RRow::Header { text: "Index".into(), kind: None });
        let Some(info) = self.ctx.info.clone() else {
            self.right.rows.push(RRow::Text { text: "loading…".into(), warn: false });
            return;
        };
        let g = |k: &str| info.get(k).map(String::from);
        let mut t = vec![
            format!("mode: {}", if mode == IndexMode::Semantic { "semantic snapshot" } else { "source-only scan" }),
            format!("declarations: {} ({} local) · modules: {} ({} local)", info.decls, info.local, info.modules, info.local_modules),
        ];
        if mode == IndexMode::Semantic {
            t.push(format!("edges: {}", info.edges));
            if let Some(v) = g("lean_version") {
                t.push(format!("Lean {v} ({})", g("lean_toolchain").unwrap_or_default()));
            }
            if let Some(e) = g("exporter") {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&e) {
                    t.push(format!(
                        "exporter: {} {} · compat: {}",
                        v.get("exporter").and_then(|x| x.as_str()).unwrap_or("?"),
                        v.get("exporter_version").and_then(|x| x.as_str()).unwrap_or("?"),
                        v.get("compat").and_then(|x| x.as_str()).unwrap_or("?")
                    ));
                }
            }
            if let Some(c) = g("created_at") {
                t.push(format!("snapshot created: {c}"));
            }
            if let Some(sc) = g("scope") {
                t.push(format!("scope: {sc}"));
            }
            if let Some(b) = g("build") {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&b) {
                    if let Some(n) = v.get("note").and_then(|x| x.as_str()) {
                        t.push(format!("build: {n}"));
                    }
                }
            }
            if let Some(i) = g("isolation") {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&i) {
                    t.push(format!("sandbox: {}", v.get("sandbox").and_then(|x| x.as_str()).unwrap_or("?")));
                }
            }
            if let Some(s) = g("snapshot_sha256") {
                t.push(format!("snapshot sha256: {}", &s[..16.min(s.len())]));
            }
            if g("synthetic").as_deref() == Some("1") {
                t.push("SYNTHETIC benchmark data — not produced by Lean".into());
            }
        } else {
            t.push(format!("scanner v{} · lexer profile {}", g("scanner_version").unwrap_or_default(), g("lex_profile").unwrap_or_default()));
            t.push(format!("project toolchain: {}", g("project_lean_toolchain").unwrap_or_else(|| "(none)".into())));
            t.push("candidates: lexical scan (no elaboration, macros or Lake)".into());
        }
        if let Some(r) = g("source_root") {
            t.push(format!("source root: {r}"));
        }
        t.push(match &self.fresh {
            Fresh::Unknown => "freshness: not checked yet".into(),
            Fresh::Checking => "freshness: checking…".into(),
            Fresh::Fresh { checked, ms } => format!("freshness: {checked} files match the index ({ms} ms; size+mtime, SHA-256 on mismatch)"),
            Fresh::Stale(fr) => format!(
                "freshness: STALE — {} changed, {} deleted, {} added{}",
                fr.changed.len(),
                fr.deleted.len(),
                fr.added.len(),
                if fr.env_changed.is_empty() { String::new() } else { format!("; {}", fr.env_changed.join("; ")) }
            ),
            Fresh::Error(e) => format!("freshness: check failed: {e}"),
        });
        if let Some(Ok((n, ms, rebuilt))) = &self.search_ready {
            t.push(format!("name search: {n} entries loaded in {ms} ms{}", if *rebuilt { " (rebuilt)" } else { "" }));
        }
        if let Some(ms) = self.first_frame_ms {
            t.push(format!("first frame after {ms} ms"));
        }
        for x in t {
            let warn = x.contains("STALE") || x.contains("SYNTHETIC");
            self.right.rows.push(RRow::Text { text: x, warn });
        }
    }

    // ------------------------------------------------------------------
    // Outline
    // ------------------------------------------------------------------

    /// Loads the outline when the left pane is showing it.
    pub fn show_outline(&mut self) {
        if self.v.view == View::Browse && self.left_mode == LeftMode::Outline {
            self.request_outline();
        }
    }

    pub fn request_outline(&mut self) {
        if self.outline.requested {
            return;
        }
        self.outline.requested = true;
        let gen = self.gens.bump();
        self.gens.outline = gen;
        self.q(QueryReq::ModuleList { gen, local_only: true });
    }

    /// Requests a module's declarations for the outline (once).
    fn request_module_decls(&mut self, module_id: i64) {
        if self.outline.module_decls.contains_key(&module_id) || !self.outline.pending_decls.insert(module_id) {
            return;
        }
        let gen = self.gens.bump();
        self.q(QueryReq::ModuleDecls { gen, module_id, show_generated: self.filters.show_generated });
    }

    /// Shows `module` in the outline: expands its ancestors and itself,
    /// loads its declarations, selects it (or `decl` inside it when given),
    /// scrolls it into view and focuses the outline. Works before the
    /// outline or the declarations have loaded (applied on arrival).
    pub fn reveal_in_outline(&mut self, module: String, decl: Option<i64>) {
        self.left_mode = LeftMode::Outline;
        self.focus = Focus::Left;
        self.preview_session = false;
        self.outline.reveal = Some(Reveal { module, decl });
        if self.outline.loaded {
            self.apply_reveal();
        } else {
            self.request_outline();
        }
        self.dirty = true;
    }

    fn apply_reveal(&mut self) {
        let Some(r) = self.outline.reveal.clone() else { return };
        if !self.outline.loaded {
            return;
        }
        let Some(&i) = self.outline.by_name.get(&r.module) else {
            self.outline.reveal = None;
            self.msg(format!(
                "{} is not in this project's source tree (an external/library module), so the outline does not list it",
                r.module
            ));
            return;
        };
        // Expand the path; ancestors that are modules themselves (`A` and
        // `A/…`) also need their declarations loaded.
        let mut a = self.outline.nodes[i].parent;
        while let Some(k) = a {
            self.outline.nodes[k].expanded = true;
            if let Some(m) = self.outline.nodes[k].module.clone() {
                self.request_module_decls(m.id);
            }
            a = self.outline.nodes[k].parent;
        }
        self.outline.nodes[i].expanded = true;
        let mut waiting = false;
        if let Some(m) = self.outline.nodes[i].module.clone() {
            if !self.outline.module_decls.contains_key(&m.id) {
                self.request_module_decls(m.id);
                waiting = r.decl.is_some();
            }
        }
        self.rebuild_outline_rows();
        let node_row = self.outline.rows.iter().position(|x| matches!(x, OutlineRow::Node(k) if *k == i));
        let decl_row = r.decl.and_then(|d| {
            self.outline.rows.iter().position(|x| matches!(x, OutlineRow::Decl { item, .. } if item.row.id == d))
        });
        if let Some(row) = decl_row.or(node_row) {
            self.outline.list.selected = row;
            self.outline.list.scroll = row.saturating_sub(self.last_body_height.saturating_sub(4) / 3);
        }
        if !waiting {
            self.outline.reveal = None;
            if r.decl.is_some() && decl_row.is_none() {
                self.msg(format!("module {} shown; the declaration is hidden by the current filters", r.module));
            }
        }
        self.dirty = true;
    }

    pub fn rebuild_outline_rows(&mut self) {
        // Remember what is selected (not its index): rows inserted above it
        // (e.g. declarations arriving for an expanded ancestor) must not move
        // the highlight to a different item.
        #[derive(PartialEq)]
        enum Ident {
            Node(usize),
            Decl(i64),
        }
        let ident_of = |r: &OutlineRow| match r {
            OutlineRow::Node(i) => Some(Ident::Node(*i)),
            OutlineRow::Decl { item, .. } => Some(Ident::Decl(item.row.id)),
            OutlineRow::Note { .. } => None,
        };
        let old_sel = self.outline.list.selected;
        let keep = self.outline.rows.get(old_sel).and_then(ident_of);
        let mut rows = Vec::new();
        fn walk(o: &OutlinePane, i: usize, rows: &mut Vec<OutlineRow>) {
            rows.push(OutlineRow::Node(i));
            let n = &o.nodes[i];
            if !n.expanded {
                return;
            }
            if let Some(m) = &n.module {
                match o.module_decls.get(&m.id) {
                    Some((items, trunc)) => {
                        for it in items {
                            rows.push(OutlineRow::Decl { item: it.clone(), depth: n.depth + 1 });
                        }
                        if *trunc {
                            rows.push(OutlineRow::Note { text: "… more declarations (use search)".into(), depth: n.depth + 1 });
                        }
                    }
                    None => rows.push(OutlineRow::Note { text: "loading…".into(), depth: n.depth + 1 }),
                }
            }
            for &c in o.ordered(&n.children).iter() {
                walk(o, c, rows);
            }
        }
        for &r in self.outline.ordered(&self.outline.roots).iter() {
            walk(&self.outline, r, &mut rows);
        }
        if let Some(id) = keep {
            if let Some(new_sel) = rows.iter().position(|r| ident_of(r).as_ref() == Some(&id)) {
                let delta = new_sel as isize - old_sel as isize;
                self.outline.list.selected = new_sel;
                self.outline.list.scroll = (self.outline.list.scroll as isize + delta).max(0) as usize;
            }
        }
        self.outline.rows = rows;
        self.dirty = true;
    }

    fn toggle_outline_node(&mut self, i: usize, expand: Option<bool>) {
        let n = &mut self.outline.nodes[i];
        let new = expand.unwrap_or(!n.expanded);
        if n.expanded == new {
            return;
        }
        n.expanded = new;
        if new {
            if let Some(m) = n.module.clone() {
                self.request_module_decls(m.id);
            }
        }
        self.rebuild_outline_rows();
    }

    // ------------------------------------------------------------------
    // Input
    // ------------------------------------------------------------------

    fn on_input(&mut self, e: CEvent) {
        match e {
            CEvent::Key(k) if k.kind != KeyEventKind::Release => {
                self.user_acted = true;
                self.status = None;
                self.on_key(k);
            }
            CEvent::Resize(_, _) => {
                self.center.cache = None;
                self.v.detail.cache = None;
            }
            CEvent::Mouse(m) => match m.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    self.user_acted = true;
                    self.status = None;
                    self.on_click(m.column, m.row);
                }
                MouseEventKind::ScrollDown => self.on_wheel(m.column, m.row, 3),
                MouseEventKind::ScrollUp => self.on_wheel(m.column, m.row, -3),
                _ => {}
            },
            _ => {}
        }
        self.dirty = true;
    }

    fn on_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(k.code, KeyCode::Char('c') | KeyCode::Char('q')) {
            self.quit = true;
            return;
        }
        if self.building {
            // Only the progress screen is shown; q / Esc quit (and cancel the scan).
            if matches!(k.code, KeyCode::Char('q') | KeyCode::Esc) {
                self.quit = true;
            }
            return;
        }
        // Overlays first.
        match &mut self.overlay {
            Overlay::Help { scroll } => {
                match k.code {
                    KeyCode::Down | KeyCode::Char('j') => *scroll += 1,
                    KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                    KeyCode::PageDown | KeyCode::Char(' ') => *scroll += 10,
                    KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                    _ => self.overlay = Overlay::None,
                }
                return;
            }
            Overlay::HlStatus { scroll } => {
                match k.code {
                    KeyCode::Down | KeyCode::Char('j') => *scroll += 1,
                    KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                    KeyCode::PageDown | KeyCode::Char(' ') => *scroll += 10,
                    KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                    KeyCode::F(4) => self.hl_cycle_mode(),
                    KeyCode::F(5) => self.hl_cycle_theme(),
                    _ => self.overlay = Overlay::None,
                }
                return;
            }
            Overlay::ConfirmHighlight { typed, .. } => {
                match k.code {
                    KeyCode::Esc => {
                        self.overlay = Overlay::None;
                        self.hl_cancel();
                    }
                    KeyCode::Enter => {
                        let ok = typed.trim() == "yes";
                        self.overlay = Overlay::None;
                        if ok {
                            self.hl_confirmed();
                        } else {
                            self.hl_cancel();
                            self.msg("semantic indexing not started (type yes to confirm)");
                        }
                    }
                    KeyCode::Backspace => {
                        typed.pop();
                    }
                    KeyCode::Char(c) if !ctrl && typed.len() < 8 => typed.push(c),
                    _ => {}
                }
                return;
            }
            Overlay::Palette { input, sel } => {
                let matches = palette_matches(input);
                match k.code {
                    KeyCode::Esc => self.overlay = Overlay::None,
                    KeyCode::Enter => {
                        let cmd = matches.get(*sel).map(|(c, _, _)| *c);
                        self.overlay = Overlay::None;
                        if let Some(c) = cmd {
                            self.run_cmd(c);
                        }
                    }
                    KeyCode::Down | KeyCode::Tab => *sel = (*sel + 1).min(matches.len().saturating_sub(1)),
                    KeyCode::Up | KeyCode::BackTab => *sel = sel.saturating_sub(1),
                    KeyCode::Backspace => {
                        input.pop();
                        *sel = 0;
                    }
                    KeyCode::Char(c) if !ctrl => {
                        input.push(c);
                        *sel = 0;
                    }
                    _ => {}
                }
                return;
            }
            Overlay::Input { text, .. } => {
                match k.code {
                    KeyCode::Esc => self.overlay = Overlay::None,
                    KeyCode::Enter => {
                        if let Overlay::Input { purpose, text, .. } = std::mem::replace(&mut self.overlay, Overlay::None) {
                            self.finish_input(purpose, text);
                        }
                    }
                    KeyCode::Backspace => {
                        text.pop();
                    }
                    KeyCode::Char('u') if ctrl => text.clear(),
                    KeyCode::Char(c) if !ctrl && text.chars().count() < 2000 => text.push(c),
                    _ => {}
                }
                return;
            }
            Overlay::ConfirmExport { typed, .. } => {
                match k.code {
                    KeyCode::Esc => {
                        self.overlay = Overlay::None;
                        self.msg("semantic export not started");
                    }
                    KeyCode::Enter => {
                        let ok = typed.trim() == "yes";
                        self.overlay = Overlay::None;
                        if ok {
                            self.job_queue.push(JobReq::ExportRun);
                            self.run_next_job();
                            self.msg("semantic export started (q quits and terminates it)");
                        } else {
                            self.msg("semantic export not started (type yes to confirm)");
                        }
                    }
                    KeyCode::Backspace => {
                        typed.pop();
                    }
                    KeyCode::Char(c) if !ctrl && typed.len() < 8 => typed.push(c),
                    _ => {}
                }
                return;
            }
            Overlay::None => {}
        }
        // The map, goal and trail views have their own keys; what they do
        // not handle falls through to the global bindings.
        if self.v.view != View::Browse && self.views_key(k) {
            return;
        }
        // Search input editing.
        if self.v.view == View::Browse && self.focus == Focus::Left && self.left_mode == LeftMode::Search && self.search.editing {
            match k.code {
                KeyCode::Esc => {
                    self.search.editing = false;
                    return;
                }
                KeyCode::Enter => {
                    self.search.editing = false;
                    if self.search.pending {
                        self.search.open_on_results = true;
                        return;
                    }
                    self.skip_to_selectable_left(true);
                    self.activate();
                    return;
                }
                KeyCode::Down => {
                    self.search.editing = false;
                    self.skip_to_selectable_left(true);
                    self.open_left_selection(false);
                    return;
                }
                KeyCode::Backspace => {
                    if self.search.cursor > 0 {
                        let mut chars: Vec<char> = self.search.query.chars().collect();
                        chars.remove(self.search.cursor - 1);
                        self.search.query = chars.into_iter().collect();
                        self.search.cursor -= 1;
                        self.run_search();
                    } else if self.search.query.is_empty() && self.search.scope.is_some() {
                        // The scope sits in front of the query: one more Backspace removes it.
                        self.clear_search_scope();
                    }
                    return;
                }
                KeyCode::Delete => {
                    let mut chars: Vec<char> = self.search.query.chars().collect();
                    if self.search.cursor < chars.len() {
                        chars.remove(self.search.cursor);
                        self.search.query = chars.into_iter().collect();
                        self.run_search();
                    }
                    return;
                }
                KeyCode::Left => {
                    self.search.cursor = self.search.cursor.saturating_sub(1);
                    return;
                }
                KeyCode::Right => {
                    self.search.cursor = (self.search.cursor + 1).min(self.search.query.chars().count());
                    return;
                }
                KeyCode::Home => {
                    self.search.cursor = 0;
                    return;
                }
                KeyCode::End => {
                    self.search.cursor = self.search.query.chars().count();
                    return;
                }
                KeyCode::Char('u') if ctrl => {
                    self.search.query.clear();
                    self.search.cursor = 0;
                    self.run_search();
                    return;
                }
                // `/` (habitual "start search") is never part of a Lean name.
                KeyCode::Char('/') => return,
                KeyCode::Char(c) if !ctrl && !k.modifiers.contains(KeyModifiers::ALT) => {
                    if self.search.query.chars().count() < 256 {
                        let mut chars: Vec<char> = self.search.query.chars().collect();
                        chars.insert(self.search.cursor, c);
                        self.search.query = chars.into_iter().collect();
                        self.search.cursor += 1;
                        self.run_search();
                    }
                    return;
                }
                KeyCode::Tab | KeyCode::BackTab => {
                    self.search.editing = false;
                }
                _ => {}
            }
        }
        // Global keys.
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        match k.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') | KeyCode::F(1) => self.overlay = Overlay::Help { scroll: 0 },
            KeyCode::Char(':') => self.overlay = Overlay::Palette { input: String::new(), sel: 0 },
            KeyCode::Char('p') if ctrl => self.overlay = Overlay::Palette { input: String::new(), sel: 0 },
            KeyCode::Char('/') => self.run_cmd(Cmd::FocusSearch),
            KeyCode::Char('M') => self.run_cmd(Cmd::Map),
            KeyCode::Char('A') => self.run_cmd(Cmd::Goal),
            KeyCode::Char('*') => self.run_cmd(Cmd::ToggleGoal),
            KeyCode::Char('Q') => self.run_cmd(Cmd::Question),
            KeyCode::Char('c') => self.run_cmd(Cmd::Connect),
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Left => Focus::Center,
                    Focus::Center => Focus::Right,
                    Focus::Right => Focus::Left,
                }
            }
            KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Left => Focus::Right,
                    Focus::Center => Focus::Left,
                    Focus::Right => Focus::Center,
                }
            }
            KeyCode::Char('[') | KeyCode::Backspace => self.back(),
            KeyCode::Left if alt => self.back(),
            KeyCode::Char(']') => self.forward(),
            KeyCode::Right if alt => self.forward(),
            KeyCode::Char('O') => self.run_cmd(Cmd::Outline),
            KeyCode::Char('o') => self.run_cmd(Cmd::OutlineOrder),
            KeyCode::Char('S') => self.run_cmd(Cmd::Skim),
            KeyCode::Char('f') => self.run_cmd(Cmd::SearchScope),
            KeyCode::Char('F') => self.run_cmd(Cmd::ClearScope),
            KeyCode::Char('I') => self.run_cmd(Cmd::Imports),
            KeyCode::Char('B') => self.run_cmd(Cmd::Bookmarks),
            KeyCode::Char('T') => self.run_cmd(Cmd::Trail),
            KeyCode::Char('m') => self.run_cmd(Cmd::ToggleBookmark),
            KeyCode::Char('a') => self.run_cmd(Cmd::AddToTrail),
            KeyCode::Char('n') => self.run_cmd(Cmd::EditNote),
            KeyCode::Char('E') => self.run_cmd(Cmd::ExportTrail),
            KeyCode::Char('L') => self.run_cmd(Cmd::ToggleLocal),
            KeyCode::Char('G') => self.run_cmd(Cmd::ToggleGenerated),
            KeyCode::Char('K') => self.run_cmd(Cmd::CycleKinds),
            KeyCode::Char('i') => self.run_cmd(Cmd::ToggleImplicit),
            KeyCode::Char('s') => self.run_cmd(Cmd::ToggleSource),
            KeyCode::Char('1') => self.run_cmd(Cmd::TabUses),
            KeyCode::Char('2') => self.run_cmd(Cmd::TabUsedBy),
            KeyCode::Char('3') | KeyCode::Char('x') => self.run_cmd(Cmd::TabAxioms),
            KeyCode::Char('4') => self.run_cmd(Cmd::TabModule),
            KeyCode::Char('5') => self.run_cmd(Cmd::TabInfo),
            KeyCode::Char('t') => self.run_cmd(Cmd::Transitive),
            KeyCode::Char('R') => self.run_cmd(Cmd::CheckFreshness),
            KeyCode::F(2) => self.run_cmd(Cmd::Theme),
            KeyCode::F(3) => self.run_cmd(Cmd::Ascii),
            KeyCode::F(4) => self.run_cmd(Cmd::HlMode),
            KeyCode::F(5) => self.run_cmd(Cmd::HlTheme),
            KeyCode::F(6) => self.run_cmd(Cmd::HlStatus),
            KeyCode::Char('g') => self.run_cmd(Cmd::GotoName),
            KeyCode::Char('e') => self.run_cmd(Cmd::OpenEditor),
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1),
            KeyCode::PageDown | KeyCode::Char(' ') => self.move_sel(self.page() as isize),
            KeyCode::PageUp => self.move_sel(-(self.page() as isize)),
            KeyCode::Char('d') if ctrl => self.move_sel(self.page() as isize / 2),
            KeyCode::Char('u') if ctrl => self.move_sel(-(self.page() as isize / 2)),
            KeyCode::Home => self.move_sel(-1_000_000),
            KeyCode::End => self.move_sel(1_000_000),
            KeyCode::Enter => self.activate(),
            KeyCode::Right | KeyCode::Char('l') => self.expand(true),
            KeyCode::Left | KeyCode::Char('h') => self.expand(false),
            KeyCode::Esc => match self.center_mode {
                CenterMode::Source => self.run_cmd(Cmd::ToggleSource),
                CenterMode::Skim => {
                    self.center_mode = CenterMode::Decl;
                    self.hl_refresh_view();
                }
                CenterMode::Decl => {}
            },
            _ => {}
        }
    }

    fn page(&self) -> usize {
        self.last_body_height.saturating_sub(4).max(3)
    }

    fn pane_at(&self, x: u16, y: u16) -> Option<Focus> {
        let p = ratatui::layout::Position { x, y };
        if self.hit.left.contains(p) {
            Some(Focus::Left)
        } else if self.hit.right.contains(p) {
            Some(Focus::Right)
        } else if self.hit.center.contains(p) {
            Some(Focus::Center)
        } else {
            None
        }
    }

    /// Mouse wheel scrolls the pane under the pointer.
    fn on_wheel(&mut self, x: u16, y: u16, delta: isize) {
        if !matches!(self.overlay, Overlay::None) {
            return;
        }
        if self.v.view != View::Browse {
            let p = ratatui::layout::Position { x, y };
            if self.hit.view_detail.contains(p) {
                self.v.focus = super::views::VFocus::Detail;
            } else if self.hit.view_list.contains(p) {
                self.v.focus = super::views::VFocus::Outline;
            }
            self.view_move(delta);
            return;
        }
        if let Some(f) = self.pane_at(x, y) {
            self.focus = f;
            if f == Focus::Left {
                self.search.editing = false;
            }
        }
        self.move_sel(delta);
    }

    /// Mouse click: focus the pane, select the clicked row; clicking the
    /// already selected row activates it (like Enter); tabs switch tabs;
    /// outline nodes expand/collapse.
    pub fn on_click(&mut self, x: u16, y: u16) {
        let p = ratatui::layout::Position { x, y };
        match self.overlay {
            Overlay::None => {}
            Overlay::Help { .. } => {
                self.overlay = Overlay::None;
                return;
            }
            _ => return,
        }
        if self.v.view != View::Browse {
            if self.hit.view_list.contains(p) {
                self.view_click((y - self.hit.view_list.y) as usize);
            } else if self.hit.view_detail.contains(p) {
                self.v.focus = super::views::VFocus::Detail;
            }
            return;
        }
        match self.pane_at(x, y) {
            Some(Focus::Right) => {
                if y == self.hit.right.y {
                    if let Some(&(_, _, tab)) = self.hit.right_tabs.iter().find(|(x0, x1, _)| x >= *x0 && x < *x1) {
                        self.set_tab(tab);
                    }
                    return;
                }
                self.focus = Focus::Right;
                if self.hit.right_list.contains(p) {
                    let line = (y - self.hit.right_list.y) as usize;
                    if let Some(Some(row)) = self.hit.right_line_rows.get(line).copied() {
                        if self.right.rows.get(row).is_some_and(|r| r.selectable()) {
                            if self.right.list.selected == row {
                                self.activate();
                            } else {
                                self.right.list.selected = row;
                                self.sync_marks();
                            }
                        }
                    }
                }
            }
            Some(Focus::Left) => {
                self.focus = Focus::Left;
                if self.left_mode == LeftMode::Search && self.hit.left_input.contains(p) {
                    self.search.editing = true;
                    self.search.cursor = self.search.query.chars().count();
                    return;
                }
                if !self.hit.left_list.contains(p) {
                    return;
                }
                self.search.editing = false;
                let off = (y - self.hit.left_list.y) as usize;
                let (sel, scroll) = match self.left_mode {
                    LeftMode::Search => (self.search.list.selected, self.search.list.scroll),
                    LeftMode::Outline => (self.outline.list.selected, self.outline.list.scroll),
                    LeftMode::Bookmarks | LeftMode::Trail => (self.bookmarks.list.selected, self.bookmarks.list.scroll),
                };
                let idx = scroll + off;
                if idx >= self.left_len() {
                    return;
                }
                if idx == sel {
                    self.activate();
                    return;
                }
                match self.left_mode {
                    LeftMode::Search => {
                        if matches!(self.search.items.get(idx), Some(LeftItem::Result { .. }) | Some(LeftItem::MapHit { .. })) {
                            self.search.list.selected = idx;
                            self.open_left_selection(false);
                        }
                    }
                    LeftMode::Outline => {
                        self.outline.list.selected = idx;
                        match self.outline.rows.get(idx).cloned() {
                            Some(OutlineRow::Node(i)) => self.toggle_outline_node(i, None),
                            Some(OutlineRow::Decl { .. }) => self.open_left_selection(false),
                            _ => {}
                        }
                    }
                    LeftMode::Bookmarks | LeftMode::Trail => self.bookmarks.list.selected = idx,
                }
            }
            Some(Focus::Center) => self.focus = Focus::Center,
            None => {}
        }
    }

    pub fn run_cmd(&mut self, c: Cmd) {
        // Commands of the declaration browser act there, visibly.
        if self.v.view != View::Browse
            && matches!(
                c,
                Cmd::TabUses | Cmd::TabUsedBy | Cmd::TabAxioms | Cmd::TabModule | Cmd::TabInfo | Cmd::ToggleSource | Cmd::Transitive | Cmd::OpenEditor | Cmd::ToggleImplicit | Cmd::EditNote | Cmd::GotoName
            )
        {
            self.set_view(View::Browse);
        }
        match c {
            Cmd::FocusSearch => {
                self.set_view(View::Browse);
                self.focus = Focus::Left;
                self.left_mode = LeftMode::Search;
                self.search.editing = true;
                self.search.cursor = self.search.query.chars().count();
            }
            Cmd::Map => self.cmd_map(),
            Cmd::OutlineOrder => self.cmd_outline_order(),
            Cmd::Skim => self.cmd_skim(),
            Cmd::SearchScope => self.cmd_search_scope(),
            Cmd::ClearScope => self.clear_search_scope(),
            Cmd::Imports => self.cmd_imports(),
            Cmd::ImportPath => self.cmd_import_path(),
            Cmd::Goal => self.set_view(View::Goal),
            Cmd::ToggleGoal => self.cmd_toggle_goal(),
            Cmd::Connect => self.cmd_connect(),
            Cmd::Question => self.cmd_question(),
            Cmd::ReloadMap => {
                self.v.map.reload_requested = true;
                self.q(QueryReq::ReloadMap);
                self.msg("reloading lean-view.toml…");
            }
            Cmd::Outline if self.v.view != View::Browse => {
                self.set_view(View::Browse);
                self.focus = Focus::Left;
                self.left_mode = LeftMode::Outline;
                self.request_outline();
            }
            Cmd::Outline => match self.detail.as_ref().and_then(|d| d.d.row.module.clone().map(|m| (m, d.d.row.id))) {
                Some((m, id)) if self.ctx.mode == IndexMode::Source || self.detail.as_ref().is_some_and(|d| d.d.row.origin == Origin::Local) => {
                    self.reveal_in_outline(m, Some(id));
                }
                _ => {
                    self.focus = Focus::Left;
                    self.left_mode = LeftMode::Outline;
                    self.request_outline();
                }
            },
            Cmd::Bookmarks => {
                self.set_view(View::Browse);
                self.focus = Focus::Left;
                self.left_mode = LeftMode::Bookmarks;
                let gen = self.gens.bump();
                self.q(QueryReq::LoadBookmarks { gen });
            }
            Cmd::Trail => self.set_view(View::Trails),
            Cmd::ToggleBookmark => match self.mark_at_hand() {
                Some(m) => self.q(QueryReq::ToggleBookmark { mark: m }),
                None => self.msg("no declaration selected"),
            },
            Cmd::AddToTrail => self.cmd_add_to_trail(),
            Cmd::EditNote => match (self.focus, self.left_mode) {
                (Focus::Left, LeftMode::Bookmarks) => {
                    if let Some(m) = self.bookmarks.marks.get(self.bookmarks.list.selected).cloned() {
                        self.overlay = Overlay::Input {
                            purpose: InputPurpose::BookmarkNote { name: m.name.clone() },
                            prompt: format!("Note for bookmark {}:", m.display),
                            text: m.note.unwrap_or_default(),
                        };
                    }
                }
                _ => match self.detail.as_ref() {
                    Some(d) if d.bookmarked => {
                        let name = d.d.row.name.clone();
                        let existing = self.bookmarks.marks.iter().find(|m| m.name == name).and_then(|m| m.note.clone()).unwrap_or_default();
                        self.overlay = Overlay::Input {
                            purpose: InputPurpose::BookmarkNote { name },
                            prompt: format!("Note for bookmark {}:", d.d.row.display),
                            text: existing,
                        };
                    }
                    _ => self.msg("no bookmark here (notes of trail stops are edited in the trails view)"),
                },
            },
            Cmd::ExportTrail => self.cmd_export_trail(),
            Cmd::ToggleLocal => {
                self.filters.local_only = !self.filters.local_only;
                self.msg(if self.filters.local_only { "filter: local declarations only" } else { "filter: local and library declarations" });
                self.refilter();
            }
            Cmd::ToggleGenerated => {
                self.filters.show_generated = !self.filters.show_generated;
                self.msg(if self.filters.show_generated { "showing Lean-generated declarations" } else { "hiding Lean-generated declarations" });
                self.outline.module_decls.clear();
                self.outline.pending_decls.clear();
                // A skim on screen lists them or not, like the outline.
                if self.center_mode == CenterMode::Skim {
                    if let Some(sc) = self.center.skim.scope.clone() {
                        self.request_skim(sc, Some(self.center.skim.sel));
                    }
                }
                self.rebuild_outline_rows();
                for i in 0..self.outline.nodes.len() {
                    if self.outline.nodes[i].expanded {
                        if let Some(m) = self.outline.nodes[i].module.clone() {
                            self.request_module_decls(m.id);
                        }
                    }
                }
                self.refilter();
            }
            Cmd::CycleKinds => {
                self.filters.kinds = self.filters.kinds.next();
                self.msg(format!("filter: {}", self.filters.kinds.label()));
                self.refilter();
            }
            Cmd::ToggleImplicit => {
                self.filters.show_implicit = !self.filters.show_implicit;
                self.center.version += 1;
                self.msg(if self.filters.show_implicit { "statement: all arguments shown" } else { "statement: implicit and instance arguments hidden" });
            }
            Cmd::ToggleSource => match self.center_mode {
                CenterMode::Decl | CenterMode::Skim => {
                    if self.detail.as_ref().and_then(|d| d.d.module_path.clone()).is_none() {
                        self.msg("no source file for this declaration");
                        return;
                    }
                    self.center_mode = CenterMode::Source;
                    self.focus = Focus::Center;
                    self.request_full_source();
                }
                CenterMode::Source => {
                    self.center_mode = CenterMode::Decl;
                    self.hl_refresh_view();
                }
            },
            Cmd::Back => self.back(),
            Cmd::Forward => self.forward(),
            Cmd::TabUses => self.set_tab(RightTab::Uses),
            Cmd::TabUsedBy => self.set_tab(RightTab::UsedBy),
            Cmd::TabAxioms => self.set_tab(RightTab::Axioms),
            Cmd::TabModule => self.set_tab(RightTab::Module),
            Cmd::TabInfo => self.set_tab(RightTab::Info),
            Cmd::Transitive => {
                if self.ctx.mode != IndexMode::Semantic {
                    self.msg("transitive counts need semantic edges");
                    return;
                }
                if let Some(id) = self.current {
                    let dir = if self.right_tab == RightTab::UsedBy { Direction::UsedBy } else { Direction::Uses };
                    self.q(QueryReq::TransitiveCount { gen: self.gens.right, id, dir });
                    self.msg("counting (bounded to 200,000 declarations / 400 ms)…");
                }
            }
            Cmd::CheckFreshness => {
                if self.ctx.index.as_ref().and_then(|c| c.source_root.as_ref()).is_none() {
                    self.msg("no source root: nothing to compare against");
                    return;
                }
                self.job_queue.push(JobReq::Freshness { force_hash: true, discover_new: self.ctx.mode == IndexMode::Source });
                self.run_next_job();
                // The same key checks the highlight cache's inputs exactly.
                self.hl_recheck();
            }
            Cmd::Theme => {
                let n = self.theme.name.next();
                self.theme = Theme::new(n);
                self.center.cache = None;
                self.v.detail.cache = None;
                self.hl_apply_theme();
                self.msg(format!("theme: {}", n.label()));
            }
            Cmd::HlMode => self.hl_cycle_mode(),
            Cmd::HlTheme => self.hl_cycle_theme(),
            Cmd::HlStatus => self.overlay = Overlay::HlStatus { scroll: 0 },
            Cmd::HlBuild => self.hl_build(false),
            Cmd::HlBuildFile => self.hl_build(true),
            Cmd::HlPause => self.hl_pause(),
            Cmd::HlCancel => self.hl_cancel(),
            Cmd::HlStale => self.hl_toggle_stale(),
            Cmd::Ascii => {
                self.ascii = !self.ascii;
                self.sym = Symbols::new(self.ascii);
                self.center.cache = None;
                self.v.detail.cache = None;
                self.center.version += 1;
                self.msg(if self.ascii { "ASCII chrome (mathematical text unchanged)" } else { "Unicode chrome" });
            }
            Cmd::GotoName => {
                self.overlay = Overlay::Input { purpose: InputPurpose::GotoName, prompt: "Go to fully qualified name:".into(), text: String::new() };
            }
            Cmd::OpenEditor => self.open_editor(),
            Cmd::ExportHelp => {
                let root = self.ctx.project_root.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "<PROJECT>".into());
                self.msg(format!(
                    "semantic index: `slim export {root}` after `lake build` (sandboxed; prints what runs and asks to confirm)"
                ));
            }
            Cmd::SemanticExport => {
                if self.ctx.project_root.is_none() {
                    self.msg("semantic export works on a project directory (slim <PROJECT_DIR>)");
                    return;
                }
                if self.job.is_some() {
                    self.msg("another background job is running; try again when it finishes");
                    return;
                }
                self.job_queue.push(JobReq::ExportPlan);
                self.run_next_job();
            }
            Cmd::Help => self.overlay = Overlay::Help { scroll: 0 },
            Cmd::Quit => self.quit = true,
        }
        self.dirty = true;
    }

    fn refilter(&mut self) {
        self.run_search();
        if matches!(self.right_tab, RightTab::Uses | RightTab::UsedBy) {
            self.load_right();
        }
    }

    pub fn set_tab(&mut self, t: RightTab) {
        if self.right_tab == t && !self.right.rows.is_empty() {
            self.focus = Focus::Right;
            return;
        }
        self.right_tab = t;
        self.right.list = ListState::default();
        self.focus = Focus::Right;
        if t == RightTab::Info {
            let gen = self.gens.bump();
            self.gens.info = gen;
            self.q(QueryReq::Info { gen });
        }
        self.load_right();
    }

    fn request_full_source(&mut self) {
        let Some(d) = &self.detail else { return };
        let Some(rel) = d.d.module_path.clone() else { return };
        self.source_asked_at = Some(Instant::now());
        let gen = self.gens.bump();
        self.gens.source = gen;
        let focus = d.decl_lines.map(|x| x.0).unwrap_or(1);
        self.q(QueryReq::FullSource { gen, rel_path: rel, focus_line: focus, highlight: d.decl_lines, sha256: d.d.module_sha256.clone() });
    }

    /// The declaration a bookmark toggle applies to: the open declaration,
    /// or the one selected in the map, goal or trail view.
    fn mark_at_hand(&self) -> Option<Mark> {
        match self.v.view {
            View::Browse => self.current_mark(),
            _ => self.subject_bundle().map(Self::mark_of),
        }
    }

    fn current_mark(&self) -> Option<Mark> {
        self.detail.as_deref().map(Self::mark_of)
    }

    fn mark_of(b: &DetailBundle) -> Mark {
        let d = &b.d;
        Mark {
            seq: 0,
            name: d.row.name.clone(),
            display: d.row.display.clone(),
            kind: d.row.kind.as_str().into(),
            module: d.row.module.clone(),
            path: d.module_path.clone(),
            line: d.pos.start_line,
            note: None,
            created: 0,
        }
    }

    fn finish_input(&mut self, purpose: InputPurpose, text: String) {
        let note = if text.trim().is_empty() { None } else { Some(text.trim().to_string()) };
        match purpose {
            InputPurpose::BookmarkNote { name } => self.q(QueryReq::SetBookmarkNote { name, note }),
            InputPurpose::StopAdd { mut stop } => {
                stop.note = note;
                self.q(QueryReq::StopAdd { trail: None, stop: *stop });
            }
            InputPurpose::StopNote { id } => self.q(QueryReq::StopSetNote { id, note }),
            InputPurpose::StopQuestion { id } => self.q(QueryReq::StopSetQuestion { id, question: note }),
            InputPurpose::NewTrail => match note {
                Some(title) => self.q(QueryReq::TrailCreate { title }),
                None => self.msg("no trail created (the title was empty)"),
            },
            InputPurpose::RenameTrail { id } => match note {
                Some(title) => self.q(QueryReq::TrailRename { id, title }),
                None => self.msg("not renamed (the title was empty)"),
            },
            InputPurpose::DeleteTrail { id } => {
                if text.trim() == "yes" {
                    self.q(QueryReq::TrailDelete { id });
                } else {
                    self.msg("trail not deleted (type yes to confirm)");
                }
            }
            InputPurpose::ExportTrailTo { trail } => {
                let p = PathBuf::from(text.trim());
                if p.as_os_str().is_empty() {
                    return;
                }
                let desc = self.index_desc();
                self.q(QueryReq::ExportTrail { trail, path: p, index_desc: desc });
            }
            InputPurpose::AskQuestion { ref_kind, name, display } => match note {
                Some(text) => self.q(QueryReq::QuestionAdd { ref_kind, name, display, text }),
                None => self.msg("no question saved (it was empty)"),
            },
            InputPurpose::ConnectTo => self.start_connection(text.trim().to_string()),
            InputPurpose::ImportPathTo { from } => self.start_import_path(from, text.trim().to_string()),
            InputPurpose::GotoName => {
                let name = text.trim().to_string();
                if !name.is_empty() {
                    let gen = self.gens.bump();
                    self.gens.resolve = gen;
                    self.pending_goto = Some(name.clone());
                    self.current_how = "go to".into();
                    if let Some(cur) = self.current {
                        let _ = cur;
                        let loc = self.location();
                        self.history.back.push(loc);
                        self.history.fwd.clear();
                    }
                    self.q(QueryReq::ResolveName { gen, name });
                }
            }
        }
    }

    pub fn index_desc(&self) -> String {
        match (&self.ctx.info, self.ctx.mode) {
            (Some(i), IndexMode::Semantic) => format!(
                "a semantic snapshot (Lean {}, created {})",
                i.get("lean_version").unwrap_or("?"),
                i.get("created_at").unwrap_or("?")
            ),
            (_, IndexMode::Source) => "a source-only index".into(),
            _ => "an index".into(),
        }
    }

    fn open_editor(&mut self) {
        let Some(ed) = self.ctx.editor.clone() else {
            self.msg("no editor configured: start with --editor CMD (it receives +LINE FILE; no shell is involved)");
            return;
        };
        let (Some(d), Some(ix)) = (&self.detail, &self.ctx.index) else { return };
        let (Some(root), Some(rel)) = (&ix.source_root, &d.d.module_path) else {
            self.msg("no source file for this declaration");
            return;
        };
        let Ok(path) = crate::paths::resolve_contained(root, rel) else {
            self.warn("source path rejected (outside the approved root)");
            return;
        };
        let line = d.decl_lines.map(|x| x.0).unwrap_or(1);
        // The editor runs detached from our terminal session; no shell is used.
        let r = std::process::Command::new(&ed)
            .arg(format!("+{line}"))
            .arg(&path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match r {
            Ok(mut c) => {
                std::thread::spawn(move || {
                    let _ = c.wait();
                });
                self.msg(format!("opened {}:{} with {}", rel, line, ed));
            }
            Err(e) => self.warn(format!("could not start editor {ed}: {e}")),
        }
    }

    fn left_len(&self) -> usize {
        match self.left_mode {
            LeftMode::Search => self.search.items.len(),
            LeftMode::Outline => self.outline.rows.len(),
            LeftMode::Bookmarks | LeftMode::Trail => self.bookmarks.marks.len(),
        }
    }

    fn skip_to_selectable_left(&mut self, forward: bool) {
        if self.left_mode != LeftMode::Search {
            return;
        }
        let items = &self.search.items;
        let mut i = self.search.list.selected;
        while i < items.len() && matches!(items[i], LeftItem::Section(_)) {
            if forward {
                i += 1;
            } else if i == 0 {
                break;
            } else {
                i -= 1;
            }
        }
        if i < items.len() {
            self.search.list.selected = i;
        }
    }

    fn skip_to_selectable_right(&mut self, forward: bool) {
        let rows = &self.right.rows;
        if rows.is_empty() {
            return;
        }
        let mut i = self.right.list.selected.min(rows.len() - 1);
        let start = i;
        loop {
            if rows[i].selectable() {
                self.right.list.selected = i;
                return;
            }
            if forward {
                if i + 1 >= rows.len() {
                    break;
                }
                i += 1;
            } else {
                if i == 0 {
                    break;
                }
                i -= 1;
            }
        }
        self.right.list.selected = start;
    }

    pub fn move_sel(&mut self, delta: isize) {
        let apply = |sel: usize, len: usize| -> usize {
            if len == 0 {
                return 0;
            }
            (sel as isize + delta).clamp(0, len as isize - 1) as usize
        };
        match self.focus {
            Focus::Left => {
                let len = self.left_len();
                match self.left_mode {
                    LeftMode::Search => {
                        self.search.list.selected = apply(self.search.list.selected, len);
                        self.skip_to_selectable_left(delta > 0);
                        self.skip_to_selectable_left(delta <= 0);
                        self.open_left_selection(false);
                    }
                    LeftMode::Outline => {
                        self.outline.list.selected = apply(self.outline.list.selected, len);
                        self.open_left_selection(false);
                    }
                    LeftMode::Bookmarks | LeftMode::Trail => self.bookmarks.list.selected = apply(self.bookmarks.list.selected, len),
                }
            }
            Focus::Center => {
                if self.center_mode == CenterMode::Skim {
                    // Page keys arrive as a number of rows; a statement takes a few.
                    self.skim_move(if delta.abs() > 1 { delta / 3 } else { delta });
                } else if self.center_mode == CenterMode::Source {
                    let len = self.center.source.as_ref().map(|s| s.lines.len()).unwrap_or(0);
                    self.center.source_scroll = apply(self.center.source_scroll, len);
                    self.hl_refresh_view();
                } else {
                    let len = self.center.cache.as_ref().map(|c| c.2.len()).unwrap_or(0);
                    let max = len.saturating_sub(self.center.height.max(1));
                    self.center.scroll = (self.center.scroll as isize + delta).clamp(0, max as isize) as usize;
                }
            }
            Focus::Right => {
                let len = self.right.rows.len();
                self.right.list.selected = apply(self.right.list.selected, len);
                self.skip_to_selectable_right(delta > 0);
                self.skip_to_selectable_right(delta <= 0);
                self.sync_marks();
            }
        }
    }

    /// Opens the declaration under the left cursor. Cursor moves preview
    /// (replacing the current history entry while previewing); Enter commits.
    fn open_left_selection(&mut self, commit: bool) {
        let target: Option<(i64, String)> = match self.left_mode {
            LeftMode::Search => match self.search.items.get(self.search.list.selected) {
                Some(LeftItem::Result { row, tier }) => Some((
                    row.id,
                    match tier {
                        Some(_) => format!("search “{}”", self.search.query),
                        None => format!("text search “{}”", self.search.query),
                    },
                )),
                _ => None,
            },
            LeftMode::Outline => match self.outline.rows.get(self.outline.list.selected) {
                Some(OutlineRow::Decl { item, .. }) => Some((item.row.id, "outline".into())),
                _ => None,
            },
            _ => None,
        };
        let Some((id, how)) = target else {
            self.preview_due = None;
            self.preview_target = None;
            return;
        };
        if commit {
            self.preview_due = None;
            self.preview_target = None;
            if self.current == Some(id) {
                self.preview_session = false;
                return;
            }
            self.open(id, how, true);
            self.preview_session = false;
        } else if self.current == Some(id) {
            self.preview_due = None;
            self.preview_target = None;
        } else {
            // Debounced: only the highlight moves until the cursor rests.
            self.preview_target = Some((id, how));
            self.preview_due = Some(Instant::now() + PREVIEW_DELAY);
        }
    }

    /// Loads the debounced preview (called by the event loop when due).
    pub fn fire_preview(&mut self) {
        self.preview_due = None;
        if let Some((id, how)) = self.preview_target.take() {
            let push = !self.preview_session;
            self.open(id, how, push);
            self.preview_session = true;
            self.dirty = true;
        }
    }

    pub fn activate(&mut self) {
        match self.focus {
            Focus::Left => match self.left_mode {
                LeftMode::Search => {
                    if let Some(LeftItem::MapHit { key, .. }) = self.search.items.get(self.search.list.selected).cloned() {
                        self.set_view(View::Map);
                        self.map_reveal(&key);
                        return;
                    }
                    self.open_left_selection(true);
                    if self.current.is_some() {
                        self.focus = Focus::Center;
                    }
                }
                LeftMode::Outline => match self.outline.rows.get(self.outline.list.selected).cloned() {
                    Some(OutlineRow::Node(i)) => self.toggle_outline_node(i, None),
                    Some(OutlineRow::Decl { .. }) => {
                        self.open_left_selection(true);
                        self.focus = Focus::Center;
                    }
                    _ => {}
                },
                LeftMode::Bookmarks | LeftMode::Trail => {
                    let m = self.bookmarks.marks.get(self.bookmarks.list.selected).cloned();
                    if let Some(m) = m {
                        let gen = self.gens.bump();
                        self.gens.resolve = gen;
                        self.pending_goto = Some(m.name.clone());
                        if self.current.is_some() {
                            let loc = self.location();
                            self.history.back.push(loc);
                            self.history.fwd.clear();
                        }
                        self.current_how = "bookmark".into();
                        self.preview_session = false;
                        self.q(QueryReq::ResolveName { gen, name: m.name });
                        self.focus = Focus::Center;
                    }
                }
            },
            Focus::Center => match self.center_mode {
                CenterMode::Source => self.center_mode = CenterMode::Decl,
                CenterMode::Skim => {
                    if let Some(d) = self.center.skim.selected().cloned() {
                        self.preview_session = false;
                        self.open(d.row.id, "skim".into(), true);
                    }
                }
                CenterMode::Decl => {}
            },
            Focus::Right => {
                if self.right.stale {
                    self.msg("updating…");
                    return;
                }
                let sel = self.right.list.selected;
                match self.right.rows.get(sel).cloned() {
                    Some(RRow::Item { row, kind, dir, parent_id, .. }) => {
                        let from = if Some(parent_id) == self.current { "here".to_string() } else { "a neighbour".to_string() };
                        let how = match dir {
                            Direction::Uses => format!("uses[{}]", kind.tag()),
                            Direction::UsedBy => format!("used by[{}]", kind.tag()),
                        };
                        let _ = from;
                        self.preview_session = false;
                        self.open(row.id, how, true);
                    }
                    Some(RRow::More { kind, parent, parent_id, offset, dir, .. }) => {
                        self.q(QueryReq::Neighbors {
                            gen: self.gens.right,
                            id: parent_id,
                            dir,
                            filter: self.neighbor_filter(),
                            limit: 200,
                            offset,
                            parent,
                        });
                        let _ = kind;
                        self.msg("loading more…");
                    }
                    Some(RRow::Evidence { row, .. }) => {
                        self.preview_session = false;
                        self.open(row.id, "evidence path".into(), true);
                    }
                    Some(RRow::Mention { row, .. }) => {
                        self.preview_session = false;
                        self.open(row.id, "lexical mention".into(), true);
                    }
                    Some(RRow::Module { module, current, .. }) => {
                        let decl = if current { self.current } else { None };
                        self.reveal_in_outline(module.name.clone(), decl);
                    }
                    _ => {}
                }
            }
        }
    }

    fn expand(&mut self, open: bool) {
        match self.focus {
            Focus::Left if self.left_mode == LeftMode::Outline => {
                match self.outline.rows.get(self.outline.list.selected).cloned() {
                    Some(OutlineRow::Node(i)) => {
                        if !open && !self.outline.nodes[i].expanded {
                            // move to parent
                            let d = self.outline.nodes[i].depth;
                            if d > 0 {
                                let mut j = self.outline.list.selected;
                                while j > 0 {
                                    j -= 1;
                                    if let OutlineRow::Node(k) = self.outline.rows[j] {
                                        if self.outline.nodes[k].depth < d {
                                            self.outline.list.selected = j;
                                            break;
                                        }
                                    }
                                }
                            }
                        } else {
                            self.toggle_outline_node(i, Some(open));
                        }
                    }
                    _ if !open => {
                        // from a declaration row, go to its module node
                        let mut j = self.outline.list.selected;
                        while j > 0 {
                            j -= 1;
                            if matches!(self.outline.rows[j], OutlineRow::Node(_)) {
                                self.outline.list.selected = j;
                                break;
                            }
                        }
                    }
                    _ => {}
                }
            }
            Focus::Right => {
                let sel = self.right.list.selected;
                let Some(RRow::Item { key, row, expanded, cycle, depth, dir, .. }) = self.right.rows.get(sel).cloned() else { return };
                if open {
                    if expanded || cycle {
                        return;
                    }
                    if depth >= 8 {
                        self.msg("expansion depth limit reached (8)");
                        return;
                    }
                    if let Some(RRow::Item { expanded, .. }) = self.right.rows.get_mut(sel) {
                        *expanded = true;
                    }
                    self.right.expanded.insert(key, row.id);
                    self.q(QueryReq::Neighbors {
                        gen: self.gens.right,
                        id: row.id,
                        dir,
                        filter: self.neighbor_filter(),
                        limit: 50,
                        offset: 0,
                        parent: Some(key),
                    });
                } else if expanded {
                    if let Some(RRow::Item { expanded, .. }) = self.right.rows.get_mut(sel) {
                        *expanded = false;
                    }
                    self.right.expanded.remove(&key);
                    // remove descendants
                    let mut end = sel + 1;
                    while end < self.right.rows.len() {
                        let deeper = match &self.right.rows[end] {
                            RRow::Item { depth: d, key: k, .. } => {
                                if *d > depth {
                                    self.right.expanded.remove(k);
                                    true
                                } else {
                                    false
                                }
                            }
                            RRow::More { depth: d, .. } => *d > depth,
                            RRow::Text { text, .. } => text.starts_with("  ") && text.trim_start().starts_with("(no"),
                            _ => false,
                        };
                        if !deeper {
                            break;
                        }
                        end += 1;
                    }
                    self.right.rows.drain(sel + 1..end);
                }
            }
            Focus::Center if !open => self.back(),
            _ => {}
        }
    }

    /// Session state saved on exit (names, not ids, so it survives re-indexing).
    pub fn session_json(&self) -> String {
        let mut back: Vec<Location> = self.history.back.iter().rev().take(50).cloned().collect();
        back.reverse();
        for l in back.iter_mut() {
            l.decl = None;
        }
        let cur = self.detail.as_ref().map(|d| d.d.row.name.clone());
        serde_json::json!({
            "current": cur,
            "how": self.current_how,
            "query": self.search.query,
            "scope": self.search.scope,
            "outline_order": self.outline.order,
            "left_mode": self.left_mode,
            "right_tab": self.right_tab,
            "view": self.v.view,
            "goal_mode": self.v.goal.mode,
            "back": back,
        })
        .to_string()
    }

    pub fn restore_session(&mut self, json: &str) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return };
        if let Some(q) = v.get("query").and_then(|x| x.as_str()) {
            self.search.query = q.to_string();
            self.search.cursor = q.chars().count();
        }
        if let Some(sc) = v.get("scope").and_then(|x| serde_json::from_value::<Option<ScopeSpec>>(x.clone()).ok()) {
            self.search.scope = sc;
        }
        if let Some(o) = v.get("outline_order").and_then(|x| serde_json::from_value::<OutlineOrder>(x.clone()).ok()) {
            self.outline.order = o;
        }
        if let Some(t) = v.get("right_tab").and_then(|x| serde_json::from_value::<RightTab>(x.clone()).ok()) {
            self.right_tab = t;
        }
        if let Some(b) = v.get("back").and_then(|x| serde_json::from_value::<Vec<Location>>(x.clone()).ok()) {
            self.history.back = b;
        }
        if let Some(m) = v.get("goal_mode").and_then(|x| serde_json::from_value(x.clone()).ok()) {
            self.v.goal.mode = m;
        }
        let view = v.get("view").and_then(|x| serde_json::from_value::<View>(x.clone()).ok()).unwrap_or_default();
        if let Some(cur) = v.get("current").and_then(|x| x.as_str()) {
            self.current_how = v.get("how").and_then(|x| x.as_str()).unwrap_or("restored session").to_string();
            let gen = self.gens.bump();
            self.gens.resolve = gen;
            self.pending_goto = Some(cur.to_string());
            self.q(QueryReq::ResolveName { gen, name: cur.to_string() });
            self.focus = Focus::Center;
            self.search.editing = false;
            self.reveal_on_arrival = true;
        }
        if !self.search.query.is_empty() || self.search.scope.is_some() {
            self.run_search();
        }
        if view != View::Browse {
            // Resume in the view the reader left (not a history step).
            self.v.view = view;
            self.search.editing = false;
            // The map and the trails arrive by themselves; the import
            // graph is read only when asked for.
            if view == View::Imports {
                self.request_imports();
                self.rebuild_import_rows();
            }
        }
    }
}

pub fn short_name(n: &str) -> String {
    let comps = crate::model::name_components(n);
    if comps.len() <= 2 {
        n.to_string()
    } else {
        comps[comps.len() - 2..].join(".")
    }
}

pub fn palette_matches(input: &str) -> Vec<(Cmd, &'static str, &'static str)> {
    let q = input.to_lowercase();
    let words: Vec<&str> = q.split_whitespace().collect();
    COMMANDS
        .iter()
        .filter(|(_, name, _)| {
            let n = name.to_lowercase();
            words.iter().all(|w| n.contains(w))
        })
        .copied()
        .collect()
}

pub fn origin_label(o: Origin) -> &'static str {
    match o {
        Origin::Local => "local",
        Origin::Library => "lib",
        Origin::Unresolved => "unres",
        Origin::SourceCandidate => "src",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Event;

    fn app() -> App {
        let ctx = Ctx {
            title: "t".into(),
            mode: IndexMode::Semantic,
            index: None,
            user_key: "lake:t".into(),
            editor: None,
            info: None,
            synthetic: false,
            project_root: None,
        };
        let ch = Channels {
            query: None,
            search: None,
            jobs: None,
            events: None,
            search_latest: Arc::new(AtomicU64::new(0)),
            job_cancel: Arc::new(AtomicBool::new(false)),
        };
        App::new(ctx, ThemeName::Mono, true, ch)
    }

    #[test]
    fn history_restores_declaration_panes_query_and_scroll() {
        let mut a = app();
        a.search.query = "reach".into();
        a.open(1, "search “reach”".into(), true);
        a.center.scroll = 7;
        a.right_tab = RightTab::Axioms;
        a.right.list = ListState { selected: 3, scroll: 1 };
        a.focus = Focus::Right;
        a.open(2, "uses[V]".into(), true);
        a.search.query = "other".into();
        a.right_tab = RightTab::Uses;
        a.center.scroll = 0;
        a.open(3, "used by[T]".into(), true);
        assert_eq!(a.history.back.len(), 2);
        a.back();
        assert_eq!(a.current, Some(2));
        a.back();
        assert_eq!(a.current, Some(1));
        assert_eq!(a.pending_center_scroll, Some(7), "applied when the detail arrives");
        assert_eq!(a.right_tab, RightTab::Axioms);
        assert_eq!(a.focus, Focus::Right);
        assert_eq!(a.search.query, "reach");
        let p = a.pending_restore.as_ref().expect("right pane selection restored when data arrives");
        assert_eq!((p.right_sel, p.right_scroll), (3, 1));
        a.forward();
        assert_eq!(a.current, Some(2));
        a.forward();
        assert_eq!(a.current, Some(3));
        a.forward();
        assert_eq!(a.current, Some(3), "nothing further forward");
        // Navigating after going back drops the forward branch.
        a.back();
        a.open(9, "go to".into(), true);
        assert!(a.history.fwd.is_empty());
        // The route shows how the reader got here.
        assert_eq!(a.current_how, "go to");
    }

    #[test]
    fn previews_do_not_flood_history() {
        let mut a = app();
        a.open(1, "start".into(), true);
        // Three cursor moves in a result list = one history entry.
        a.preview_session = false;
        for id in [2, 3, 4] {
            let push = !a.preview_session;
            a.open(id, "search".into(), push);
            a.preview_session = true;
        }
        assert_eq!(a.history.back.len(), 1);
        a.back();
        assert_eq!(a.current, Some(1));
    }

    #[test]
    fn session_round_trip_uses_names() {
        let mut a = app();
        a.search.query = "trans".into();
        a.right_tab = RightTab::UsedBy;
        let js = a.session_json();
        let mut b = app();
        b.restore_session(&js);
        assert_eq!(b.search.query, "trans");
        assert_eq!(b.right_tab, RightTab::UsedBy);
    }

    fn app_with_queue() -> (App, std::sync::mpsc::Receiver<QueryReq>) {
        let mut a = app();
        let (tx, rx) = std::sync::mpsc::channel();
        a.ch.query = Some(tx);
        (a, rx)
    }

    fn module(id: i64, name: &str) -> ModuleRow {
        ModuleRow { id, name: name.into(), path: Some(format!("{}.lean", name.replace('.', "/"))), is_local: true, decl_count: 2, status: "ok".into(), status_detail: None, doc: None }
    }

    fn outline_item(id: i64, name: &str, line: u32) -> OutlineItem {
        OutlineItem {
            row: DeclRow { id, name: name.into(), display: name.into(), kind: crate::model::DeclKind::Theorem, origin: Origin::Local, flags: 0, module: None },
            line: Some(line),
        }
    }

    fn tree(a: &mut App, mods: Vec<ModuleRow>) {
        let mods = mods.into_iter().map(|m| (m, ModStats { files: 1, ..Default::default() })).collect();
        let gen = a.gens.outline;
        a.handle(Event::Query(QueryResp::ModuleTree(gen, Ok(Box::new(crate::app::worker::build_outline_tree(mods))))));
    }

    fn press(a: &mut App, c: char) {
        a.handle(Event::Input(CEvent::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))));
    }

    fn key(a: &mut App, c: KeyCode) {
        a.handle(Event::Input(CEvent::Key(KeyEvent::new(c, KeyModifiers::NONE))));
    }

    #[test]
    fn outline_order_sorts_children_by_the_chosen_count_and_keeps_the_selection() {
        let (mut a, _rx) = app_with_queue();
        a.ctx.mode = IndexMode::Source;
        a.left_mode = LeftMode::Outline;
        a.focus = Focus::Left;
        a.search.editing = false;
        a.request_outline();
        let st = |thm: u32, proof: u32, sorry: u32, mtime: i64| ModStats { files: 1, thm, proof_lines: proof, sorry, mtime_ns: mtime, ..Default::default() };
        let mods = vec![(module(1, "P.A"), st(1, 500, 0, 30)), (module(2, "P.B"), st(9, 20, 2, 10)), (module(3, "P.C.X"), st(4, 90, 0, 20)), (module(4, "P.C.Y"), st(0, 0, 1, 40))];
        let gen = a.gens.outline;
        a.handle(Event::Query(QueryResp::ModuleTree(gen, Ok(Box::new(crate::app::worker::build_outline_tree(mods))))));
        let labels = |a: &App| -> Vec<String> {
            a.outline.rows.iter().filter_map(|r| if let OutlineRow::Node(i) = r { Some(a.outline.nodes[*i].label.clone()) } else { None }).collect()
        };
        assert_eq!(labels(&a), vec!["P", "A", "B", "C"], "by name; the single root is open");
        // A directory sums what is below it.
        let c = a.outline.by_name["P.C"];
        assert_eq!((a.outline.nodes[c].roll.files, a.outline.nodes[c].roll.thm, a.outline.nodes[c].roll.proof_lines, a.outline.nodes[c].roll.mtime_ns), (2, 4, 90, 40));
        assert_eq!(a.outline.nodes[a.outline.by_name["P"]].roll.sorry, 3);
        // Select B, then cycle: the selection stays on B wherever it moves.
        a.outline.list.selected = 2;
        press(&mut a, 'o'); // declarations: all files have 2, the directory 4
        assert_eq!(labels(&a), vec!["P", "C", "A", "B"]);
        press(&mut a, 'o'); // theorems
        assert_eq!(labels(&a), vec!["P", "B", "C", "A"]);
        assert!(matches!(a.outline.rows[a.outline.list.selected], OutlineRow::Node(i) if a.outline.nodes[i].label == "B"));
        press(&mut a, 'o'); // proof lines (source-only indexes record them)
        assert_eq!(a.outline.order, OutlineOrder::ProofLines);
        assert_eq!(labels(&a), vec!["P", "A", "C", "B"]);
        press(&mut a, 'o'); // sorry
        assert_eq!(labels(&a), vec!["P", "B", "C", "A"]);
        press(&mut a, 'o'); // last modified
        assert_eq!(labels(&a), vec!["P", "C", "A", "B"]);
        press(&mut a, 'o');
        assert_eq!(a.outline.order, OutlineOrder::Name);
        assert_eq!(labels(&a), vec!["P", "A", "B", "C"]);
        // A semantic index has no proof lines: that column is skipped.
        a.ctx.mode = IndexMode::Semantic;
        a.outline.order = OutlineOrder::Theorems;
        press(&mut a, 'o');
        assert_eq!(a.outline.order, OutlineOrder::Sorry);
    }

    fn skim_of(scope: SkimScope, names: &[&str]) -> Box<Skim> {
        use crate::skim::SkimDecl;
        let mut items = vec![SkimItem::Header { title: "P.A".into(), sub: "P/A.lean".into(), doc: None, warn: None }];
        for (i, n) in names.iter().enumerate() {
            let row = DeclRow { id: 10 + i as i64, name: n.to_string(), display: n.to_string(), kind: crate::model::DeclKind::Theorem, origin: Origin::Local, flags: 0, module: Some("P.A".into()) };
            items.push(SkimItem::Decl(SkimDecl { row, line: Some(i as u32 + 1), sig: format!("theorem {n} : True"), sig_cut: false, doc: None, sorry: false, folded: 1 }));
        }
        Box::new(Skim { title: scope.label(), scope, decls: names.len(), files: 1, stats: None, truncated: None, items })
    }

    fn sent_skim(rx: &std::sync::mpsc::Receiver<QueryReq>) -> Option<SkimScope> {
        rx.try_iter().filter_map(|r| if let QueryReq::Skim { scope, .. } = r { Some(scope) } else { None }).last()
    }

    #[test]
    fn skim_is_a_history_step_with_its_own_selection() {
        let (mut a, rx) = app_with_queue();
        a.search.editing = false;
        // Nothing at hand: said, not ignored.
        press(&mut a, 'S');
        assert!(a.status.as_ref().unwrap().0.contains("nothing to skim"));
        assert_eq!(a.center_mode, CenterMode::Decl);
        // From an outline directory: its files; from a file: that file.
        a.left_mode = LeftMode::Outline;
        a.focus = Focus::Left;
        a.request_outline();
        tree(&mut a, vec![module(1, "P.A"), module(2, "P.B")]);
        a.outline.list.selected = 0;
        press(&mut a, 'S');
        assert_eq!(sent_skim(&rx), Some(SkimScope::Dir("P".into())));
        assert_eq!((a.center_mode, a.focus), (CenterMode::Skim, Focus::Center));
        assert_eq!(a.history.back.len(), 1, "Backspace returns to the outline");
        a.back();
        assert_eq!((a.center_mode, a.focus, a.left_mode), (CenterMode::Decl, Focus::Left, LeftMode::Outline));
        a.outline.list.selected = 1;
        press(&mut a, 'S');
        assert_eq!(sent_skim(&rx), Some(SkimScope::Module("P.A".into())));
        let gen = a.gens.skim;
        a.handle(Event::Query(QueryResp::Skim(gen, Ok(skim_of(SkimScope::Module("P.A".into()), &["N.a", "N.b", "N.c"])))));
        assert_eq!(a.center.skim.selected().unwrap().row.name, "N.a", "the first declaration, not the file header");
        // j/k move by declaration and stop at the ends.
        press(&mut a, 'j');
        press(&mut a, 'j');
        press(&mut a, 'j');
        assert_eq!(a.center.skim.selected().unwrap().row.name, "N.c");
        press(&mut a, 'k');
        assert_eq!(a.center.skim.selected().unwrap().row.name, "N.b");
        // Enter opens the declaration; Backspace returns to the same statement.
        key(&mut a, KeyCode::Enter);
        assert_eq!((a.current, a.center_mode), (Some(11), CenterMode::Decl));
        a.back();
        assert_eq!(a.center_mode, CenterMode::Skim);
        assert_eq!(sent_skim(&rx), Some(SkimScope::Module("P.A".into())));
        let gen = a.gens.skim;
        a.handle(Event::Query(QueryResp::Skim(gen, Ok(skim_of(SkimScope::Module("P.A".into()), &["N.a", "N.b", "N.c"])))));
        assert_eq!(a.center.skim.selected().unwrap().row.name, "N.b");
        // S inside a skim: the namespace of the selected declaration, and back to its file.
        press(&mut a, 'S');
        assert_eq!(sent_skim(&rx), Some(SkimScope::Namespace("N".into())));
        let gen = a.gens.skim;
        a.handle(Event::Query(QueryResp::Skim(gen, Ok(skim_of(SkimScope::Namespace("N".into()), &["N.a", "N.b"])))));
        press(&mut a, 'S');
        assert_eq!(sent_skim(&rx), Some(SkimScope::Module("P.A".into())));
        // A stale answer is dropped; Esc closes the skim.
        a.handle(Event::Query(QueryResp::Skim(gen, Err("old".into()))));
        assert!(a.center.skim.error.is_none());
        key(&mut a, KeyCode::Esc);
        assert_eq!(a.center_mode, CenterMode::Decl);
    }

    fn sent_scope(rx: &std::sync::mpsc::Receiver<SearchReq>) -> Option<Option<ScopeSpec>> {
        rx.try_iter().filter_map(|r| if let SearchReq::Query { scope, .. } = r { Some(scope) } else { None }).last()
    }

    #[test]
    fn search_scope_comes_from_what_is_selected_widens_and_clears() {
        let (mut a, _q) = app_with_queue();
        let (stx, srx) = std::sync::mpsc::channel();
        a.ch.search = Some(stx);
        a.search.editing = false;
        press(&mut a, 'f');
        assert!(a.search.scope.is_none());
        assert!(a.status.as_ref().unwrap().0.contains("nothing to search within"));
        // An outline directory scopes to its files, even with an empty query.
        a.left_mode = LeftMode::Outline;
        a.focus = Focus::Left;
        a.request_outline();
        tree(&mut a, vec![module(1, "P.A.X"), module(2, "P.B")]);
        a.outline.nodes[1].expanded = true;
        a.rebuild_outline_rows();
        let x = a.outline.rows.iter().position(|r| matches!(r, OutlineRow::Node(i) if a.outline.nodes[*i].full == "P.A.X")).unwrap();
        a.outline.list.selected = x;
        press(&mut a, 'f');
        let want = ScopeSpec { kind: ScopeKind::Modules, prefix: "P.A.X".into() };
        assert_eq!(a.search.scope, Some(want.clone()));
        assert_eq!(sent_scope(&srx), Some(Some(want)), "an empty query lists the scope");
        assert!(a.search.editing && a.left_mode == LeftMode::Search);
        // Typing keeps the scope; Backspace in the empty box removes it.
        press(&mut a, 'x');
        assert_eq!(sent_scope(&srx).unwrap().unwrap().prefix, "P.A.X");
        key(&mut a, KeyCode::Backspace);
        assert!(a.search.scope.is_some() && sent_scope(&srx).unwrap().is_some());
        key(&mut a, KeyCode::Backspace);
        assert!(a.search.scope.is_none());
        assert_eq!(sent_scope(&srx), None, "no query and no scope: nothing is searched");
        // From a declaration: its namespace; f again widens one level at a time, then clears.
        key(&mut a, KeyCode::Esc);
        a.focus = Focus::Center;
        a.left_mode = LeftMode::Search;
        a.detail = Some(Box::new(DetailBundle {
            d: DeclDetail {
                row: DeclRow { id: 5, name: "Demo.Graph.reach".into(), display: "Demo.Graph.reach".into(), kind: crate::model::DeclKind::Theorem, origin: Origin::Local, flags: 0, module: Some("P.B".into()) },
                module_id: Some(2),
                module_path: None,
                module_status: None,
                module_sha256: None,
                type_cov: crate::model::Coverage::SourceOnly,
                value_cov: crate::model::Coverage::SourceOnly,
                pos: Pos::default(),
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
        press(&mut a, 'f');
        assert_eq!(a.search.scope, Some(ScopeSpec { kind: ScopeKind::Namespace, prefix: "Demo.Graph".into() }));
        key(&mut a, KeyCode::Esc);
        press(&mut a, 'f');
        assert_eq!(a.search.scope.as_ref().unwrap().prefix, "Demo");
        key(&mut a, KeyCode::Esc);
        press(&mut a, 'f');
        assert!(a.search.scope.is_none(), "widening past the top clears the scope");
        // F clears at once; the scope is part of the session and of history.
        a.search.scope = Some(ScopeSpec { kind: ScopeKind::Namespace, prefix: "Demo".into() });
        let js = a.session_json();
        let loc = a.location();
        key(&mut a, KeyCode::Esc);
        press(&mut a, 'F');
        assert!(a.search.scope.is_none());
        let mut b = app();
        b.restore_session(&js);
        assert_eq!(b.search.scope.as_ref().unwrap().prefix, "Demo");
        assert_eq!(loc.scope.unwrap().prefix, "Demo");
    }

    #[test]
    fn mentions_highlight_the_selected_identifier_and_bring_it_into_view() {
        use crate::relations::Mention;
        let (mut a, _rx) = app_with_queue();
        a.ctx.mode = IndexMode::Source;
        a.search.editing = false;
        a.current = Some(1);
        a.right_tab = RightTab::Uses;
        a.load_right();
        let row = |id: i64, n: &str| DeclRow { id, name: n.into(), display: n.into(), kind: crate::model::DeclKind::Theorem, origin: Origin::Local, flags: 0, module: None };
        let occ = |line: u32, start: u32| Occurrence { line, start, end: start + 3 };
        let ms = vec![
            Mention { ident: "foo".into(), candidates: vec![row(2, "A.foo"), row(3, "B.foo")], occurrences: vec![occ(40, 4), occ(41, 9)] },
            Mention { ident: "bar".into(), candidates: vec![row(4, "A.bar")], occurrences: vec![occ(12, 0)] },
        ];
        let gen = a.gens.right;
        a.handle(Event::Query(QueryResp::Mentions(gen, Ok(ms))));
        assert!(matches!(a.right.rows[a.right.list.selected], RRow::Mention { .. }));
        assert!(a.mention_marks().is_none(), "opening a declaration highlights nothing by itself");
        // Moving in the list turns the highlight on and asks for the line.
        a.focus = Focus::Right;
        let v0 = a.center.version;
        press(&mut a, 'j');
        assert_eq!(a.mention_marks().unwrap().len(), 2, "both candidates of `foo` share its occurrences");
        assert_eq!(a.center.reveal_line, Some(40));
        assert!(a.center.version > v0, "the declaration view lays its lines out again");
        press(&mut a, 'j');
        assert_eq!((a.mention_marks().unwrap()[0].line, a.center.reveal_line), (12, Some(12)));
        // The whole-file view scrolls to the first occurrence when it is off screen.
        a.center_mode = CenterMode::Source;
        a.last_body_height = 20;
        a.center.source_scroll = 0;
        press(&mut a, 'k');
        assert!(a.center.source_scroll > 0 && a.center.source_scroll < 40, "line 40 is on screen: scroll {}", a.center.source_scroll);
        let at = a.center.source_scroll;
        press(&mut a, 'k');
        assert_eq!(a.center.source_scroll, at, "the other candidate has the same occurrences: no jump");
        // The highlight stays while the reader looks at the centre, and
        // goes with the declaration.
        a.focus = Focus::Center;
        assert!(a.mention_marks().is_some());
        a.load_right();
        let gen = a.gens.right;
        a.handle(Event::Query(QueryResp::Mentions(gen, Ok(Vec::new()))));
        assert!(a.mention_marks().is_none() && !a.right.marks_on);
    }

    #[test]
    fn module_row_reveals_module_in_outline() {
        let (mut a, rx) = app_with_queue();
        a.current = Some(7);
        a.right_tab = RightTab::Module;
        a.right.rows = vec![
            RRow::Header { text: "Module A.B.C".into(), kind: None },
            RRow::Module { module: module(3, "A.B.C"), flags: 0, current: true },
            RRow::Module { module: module(4, "X.Y"), flags: 1, current: false },
        ];
        a.focus = Focus::Right;
        a.right.list.selected = 2;
        // Outline not loaded yet: the reveal waits for it.
        a.activate();
        assert_eq!(a.left_mode, LeftMode::Outline);
        assert_eq!(a.focus, Focus::Left);
        assert!(rx.try_iter().any(|r| matches!(r, QueryReq::ModuleList { .. })));
        tree(&mut a, vec![module(1, "A.B"), module(3, "A.B.C"), module(2, "A.E"), module(4, "X.Y"), module(5, "X.Z")]);
        // Ancestor X and the module itself are expanded, the module row is selected.
        let i = a.outline.by_name["X.Y"];
        assert!(a.outline.nodes[a.outline.by_name["X"]].expanded);
        assert!(a.outline.nodes[i].expanded);
        assert!(matches!(a.outline.rows[a.outline.list.selected], OutlineRow::Node(k) if k == i));
        assert!(rx.try_iter().any(|r| matches!(r, QueryReq::ModuleDecls { module_id: 4, .. })));
        assert!(a.outline.reveal.is_none(), "no declaration requested: done");
    }

    #[test]
    fn current_module_row_selects_the_current_declaration() {
        let (mut a, rx) = app_with_queue();
        tree(&mut a, vec![module(3, "A.B.C"), module(4, "A.D")]);
        a.outline.loaded = true;
        a.current = Some(42);
        a.right.rows = vec![RRow::Module { module: module(3, "A.B.C"), flags: 0, current: true }];
        a.focus = Focus::Right;
        a.activate();
        assert!(a.outline.reveal.is_some(), "waits for the module's declarations");
        assert!(rx.try_iter().any(|r| matches!(r, QueryReq::ModuleDecls { module_id: 3, .. })));
        a.handle(Event::Query(QueryResp::ModuleDecls(0, 3, Ok((vec![outline_item(41, "A.x", 1), outline_item(42, "A.y", 9)], false)))));
        match &a.outline.rows[a.outline.list.selected] {
            OutlineRow::Decl { item, .. } => assert_eq!(item.row.id, 42),
            other => panic!("selected {other:?}"),
        }
        assert!(a.outline.reveal.is_none());
    }

    #[test]
    fn selection_stays_on_item_when_rows_arrive_above_it() {
        let (mut a, rx) = app_with_queue();
        // `A` is both a module and a directory containing `A.B`.
        tree(&mut a, vec![module(1, "A"), module(2, "A.B")]);
        a.reveal_in_outline("A.B".into(), None);
        let target = a.outline.by_name["A.B"];
        assert!(matches!(a.outline.rows[a.outline.list.selected], OutlineRow::Node(k) if k == target));
        // Both A's and A.B's declarations were requested exactly once.
        let reqs: Vec<i64> = rx.try_iter().filter_map(|r| if let QueryReq::ModuleDecls { module_id, .. } = r { Some(module_id) } else { None }).collect();
        assert_eq!(reqs.iter().filter(|&&m| m == 1).count(), 1);
        assert_eq!(reqs.iter().filter(|&&m| m == 2).count(), 1);
        // A's declarations arrive and are inserted above A.B.
        a.handle(Event::Query(QueryResp::ModuleDecls(0, 1, Ok((vec![outline_item(10, "A.x", 1), outline_item(11, "A.y", 2)], false)))));
        assert!(matches!(a.outline.rows[a.outline.list.selected], OutlineRow::Node(k) if k == target), "highlight stays on A.B");
        assert!(!a.outline.rows.iter().any(|r| matches!(r, OutlineRow::Note { text, .. } if text.contains("loading") && false)));
    }

    #[test]
    fn external_module_is_explained_not_silently_ignored() {
        let (mut a, _rx) = app_with_queue();
        tree(&mut a, vec![module(3, "A.B.C")]);
        a.reveal_in_outline("Mathlib.Algebra.Group.Defs".into(), None);
        assert!(a.status.as_ref().is_some_and(|(m, _)| m.contains("external")));
    }

    #[test]
    fn previews_are_debounced_and_commit_is_immediate() {
        let (mut a, rx) = app_with_queue();
        a.search.query = "x".into();
        a.search.items = (1..=5)
            .map(|i| LeftItem::Result { row: outline_item(i, &format!("N.n{i}"), 1).row, tier: Some(MatchTier::Prefix) })
            .collect();
        a.left_mode = LeftMode::Search;
        a.search.editing = false;
        a.focus = Focus::Left;
        for _ in 0..4 {
            a.move_sel(1);
        }
        assert_eq!(a.current, None, "holding j does not load anything");
        assert!(!rx.try_iter().any(|r| matches!(r, QueryReq::Detail { .. })));
        assert!(a.preview_due.is_some());
        a.fire_preview();
        assert_eq!(a.current, Some(5));
        assert_eq!(rx.try_iter().filter(|r| matches!(r, QueryReq::Detail { .. })).count(), 1);
        // Enter opens at once and cancels any pending preview.
        a.move_sel(-1);
        a.activate();
        assert_eq!(a.current, Some(4));
        assert!(a.preview_due.is_none());
    }

    #[test]
    fn right_pane_keeps_rows_until_new_ones_arrive() {
        let (mut a, _rx) = app_with_queue();
        a.current = Some(1);
        a.right_tab = RightTab::Uses;
        a.right.rows = vec![RRow::Text { text: "old".into(), warn: false }];
        a.load_right();
        assert!(a.right.stale && a.right.rows.len() == 1, "no loading flash");
        let gen = a.gens.right;
        a.handle(Event::Query(QueryResp::Neighbors(gen, None, 0, Ok(vec![]))));
        assert!(!a.right.stale);
        assert!(a.right.rows.iter().all(|r| !matches!(r, RRow::Text { text, .. } if text == "old")));
    }

    #[test]
    fn stale_responses_do_not_redraw() {
        let (mut a, _rx) = app_with_queue();
        a.dirty = false;
        a.handle(Event::Query(QueryResp::Evidence(999, Ok(vec![]))));
        assert!(!a.dirty);
    }

    #[test]
    fn mouse_clicks_select_then_activate_and_switch_tabs() {
        use ratatui::backend::TestBackend;
        let (mut a, _rx) = app_with_queue();
        a.ctx.index = Some(crate::app::worker::IndexCtx {
            index: "x".into(),
            source_root: None,
            mode: IndexMode::Semantic,
            user_key: "k".into(),
            encoding: crate::text::PosEncoding::Utf8, map_path: None,
        });
        tree(&mut a, vec![module(3, "A.B.C"), module(4, "X.Y")]);
        a.current = Some(1);
        a.right_tab = RightTab::Module;
        a.right.rows = vec![
            RRow::Header { text: "imports (1)".into(), kind: None },
            RRow::Module { module: module(4, "X.Y"), flags: 0, current: false },
        ];
        let mut t = ratatui::Terminal::new(TestBackend::new(110, 30)).unwrap();
        t.draw(|f| crate::app::render::draw(f, &mut a)).unwrap();
        let line = a.hit.right_line_rows.iter().position(|r| *r == Some(1)).unwrap() as u16;
        let (x, y) = (a.hit.right_list.x + 3, a.hit.right_list.y + line);
        a.on_click(x, y);
        assert_eq!(a.focus, Focus::Right);
        assert_eq!(a.right.list.selected, 1);
        a.on_click(x, y);
        assert_eq!(a.left_mode, LeftMode::Outline, "second click activates");
        assert!(matches!(a.outline.rows[a.outline.list.selected], OutlineRow::Node(k) if k == a.outline.by_name["X.Y"]));
        // Clicking a tab label switches tabs.
        t.draw(|f| crate::app::render::draw(f, &mut a)).unwrap();
        let (x0, _, _) = *a.hit.right_tabs.iter().find(|(_, _, tab)| *tab == RightTab::Info).unwrap();
        a.on_click(x0, a.hit.right.y);
        assert_eq!(a.right_tab, RightTab::Info);
    }

    #[test]
    fn palette_filters_by_words() {
        let m = palette_matches("trail export");
        assert!(m.iter().any(|(c, _, _)| *c == Cmd::ExportTrail));
        assert!(palette_matches("zzz").is_empty());
    }
}

