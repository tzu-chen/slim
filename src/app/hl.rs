//! Highlighting in the browser: what the reader chose, the data for the
//! file on screen, and the explicit semantic indexing job.
//!
//! The UI thread only keeps an immutable snapshot of the shown file's
//! spans and resolves styles for visible lines. Tokenizing and cache reads
//! happen on the highlight worker; Lean runs only in the indexing job,
//! which the reader starts and confirms.

use super::state::{App, CenterMode, Overlay};
use super::Event;
use crate::highlight::indexer::{self, Control, Options, Outcome, Plan, Progress, Summary};
use crate::highlight::render::{styled_rows, Styler};
use crate::highlight::service::{FileHighlight, HlReq, HlResp, Usable, DEFAULT_BUDGET_BYTES};
use crate::highlight::theme::SyntaxTheme;
use crate::highlight::{Coverage, Mode};
use crate::text::{sanitize, wrap_line};
use ratatui::style::Style;
use ratatui::text::Span;
use std::cell::RefCell;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub enum SemEvent {
    Planned(Result<Box<Plan>, String>),
    Progress(Progress),
    Published { rel: String, status: Coverage },
    Finished(Result<Summary, String>),
}

pub enum SemJob {
    Idle,
    Planning,
    /// The plan is on screen, waiting for `yes`.
    Confirm(Box<Plan>),
    Running { progress: Progress, ctl: Arc<Control>, handle: Option<std::thread::JoinHandle<()>>, cancel_at: Option<Instant> },
}

pub struct HlState {
    pub mode: Mode,
    /// Show aligned semantic tokens whose inputs changed, labelled stale.
    pub stale_preview: bool,
    /// Explicit syntax theme (bundled name or file); `None` follows the UI theme.
    pub theme_choice: Option<String>,
    /// A theme file given on the command line, kept in the cycle.
    pub user_theme: Option<String>,
    pub styler: RefCell<Styler>,
    /// Snapshot for the file on screen.
    pub file: Option<Arc<FileHighlight>>,
    pub wanted: Option<String>,
    pub tx: Option<Sender<HlReq>>,
    pub worker: Option<std::thread::JoinHandle<()>>,
    pub gen: u64,
    pub sem: SemJob,
    pub last_summary: Option<String>,
    pub budget_bytes: usize,
    pub error: Option<String>,
    /// When the data for the file on screen was asked for (timing only).
    pub asked_at: Option<Instant>,
    /// When more lines of the same file were asked for, and through which
    /// line (timing only).
    pub more_at: Option<(Instant, usize)>,
}

impl HlState {
    pub fn new(default_theme: &str) -> HlState {
        let theme = SyntaxTheme::builtin(default_theme).or_else(|| SyntaxTheme::builtin("dark-plus")).expect("bundled theme");
        HlState {
            mode: Mode::Semantic,
            stale_preview: false,
            theme_choice: None,
            user_theme: None,
            styler: RefCell::new(Styler::new(Arc::new(theme))),
            file: None,
            wanted: None,
            tx: None,
            worker: None,
            gen: 0,
            sem: SemJob::Idle,
            last_summary: None,
            budget_bytes: DEFAULT_BUDGET_BYTES,
            error: None,
            asked_at: None,
            more_at: None,
        }
    }

    pub fn running(&self) -> Option<&Progress> {
        match &self.sem {
            SemJob::Running { progress, .. } => Some(progress),
            _ => None,
        }
    }
}

fn load_theme(spec: &str) -> Result<SyntaxTheme, String> {
    crate::highlight::cli::load_theme(spec).map_err(|e| format!("{e:#}"))
}

fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

impl App {
    /// Applies the syntax theme choice (or the UI theme's default).
    pub fn hl_apply_theme(&mut self) {
        let spec = self.hl.theme_choice.clone().unwrap_or_else(|| self.theme.default_syntax_theme().to_string());
        match load_theme(&spec) {
            Ok(t) => {
                self.hl.styler = RefCell::new(Styler::new(Arc::new(t)));
            }
            Err(e) => {
                self.hl.theme_choice = None;
                self.warn(format!("syntax theme not loaded: {e}"));
            }
        }
        // Resolved styles are rebuilt; cached tokens are untouched.
        self.center.cache = None;
        self.center.version += 1;
    }

    /// Asks the worker for highlight data of `rel`, lines `first..=last`
    /// (0-based) first, unless what is already here covers them.
    pub fn hl_view(&mut self, rel: &str, first: usize, last: usize) {
        if self.hl.mode == Mode::Off {
            return;
        }
        let Some(tx) = &self.hl.tx else { return };
        if self.hl.wanted.as_deref() == Some(rel) {
            if let Some(f) = self.hl.file.as_ref().filter(|f| f.rel == rel) {
                if f.baseline_complete() || f.baseline_lines_done > last + 1 {
                    return;
                }
            }
        }
        self.hl.gen += 1;
        if self.hl.wanted.as_deref() != Some(rel) || self.hl.file.is_none() {
            self.hl.asked_at = Some(Instant::now());
            self.hl.more_at = None;
        } else if self.hl.more_at.is_none() {
            self.hl.more_at = Some((Instant::now(), last));
        }
        self.hl.wanted = Some(rel.to_string());
        let _ = tx.send(HlReq::Open { gen: self.hl.gen, rel: rel.to_string(), first, last });
    }

    /// Requests data for whatever source the centre pane shows.
    pub fn hl_refresh_view(&mut self) {
        if self.center_mode == CenterMode::Source {
            if let Some(src) = &self.center.source {
                let rel = src.rel_path.clone();
                let first = self.center.source_scroll;
                let last = first + self.last_body_height.max(40);
                self.hl_view(&rel, first, last);
                return;
            }
        }
        if let Some(b) = &self.detail {
            if let (Some(rel), Some((_, last))) = (b.d.module_path.clone(), b.excerpt.first().map(|f| f.0).zip(b.excerpt.last().map(|l| l.0))) {
                let first = b.excerpt.first().map(|f| f.0).unwrap_or(1);
                self.hl_view(&rel, first.saturating_sub(1), last);
            }
        }
    }

    /// Forgets the snapshot and asks again (after settings or cache changes).
    pub fn hl_reload(&mut self) {
        self.hl.file = None;
        self.hl.wanted = None;
        self.center.version += 1;
        self.hl_refresh_view();
    }

    pub fn on_highlight(&mut self, r: HlResp) {
        match r {
            HlResp::File { hl, .. } => {
                if self.hl.wanted.as_deref() != Some(hl.rel.as_str()) {
                    return;
                }
                self.hl.error = None;
                if let Some(t) = self.hl.asked_at.take() {
                    super::timing::record("highlight: file asked to spans on the UI thread", t.elapsed().as_secs_f64() * 1000.0);
                }
                if let Some((t, last)) = self.hl.more_at {
                    if hl.baseline_complete() || hl.baseline_lines_done > last {
                        super::timing::record("highlight: lines further down asked to spans on the UI thread", t.elapsed().as_secs_f64() * 1000.0);
                        self.hl.more_at = None;
                    }
                }
                self.hl.file = Some(hl);
                // The declaration view caches its lines.
                self.center.version += 1;
                self.dirty = true;
            }
            HlResp::Error { message, .. } => self.hl.error = Some(message),
        }
    }

    /// The snapshot, if it belongs to exactly this text.
    pub fn hl_for(&self, rel: &str, sha256: &str) -> Option<&Arc<FileHighlight>> {
        if self.hl.mode == Mode::Off {
            return None;
        }
        self.hl.file.as_ref().filter(|f| f.rel == rel && f.sha256 == sha256)
    }

    /// Whether semantic tokens are applied to this snapshot now.
    pub fn hl_use_semantic(&self, hl: &FileHighlight) -> bool {
        if self.hl.mode != Mode::Semantic || hl.semantic.is_none() {
            return false;
        }
        match hl.facts.usable() {
            Usable::Current => true,
            Usable::StaleMeaning => self.hl.stale_preview,
            Usable::Checking | Usable::No => false,
        }
    }

    /// What the source pane's title says about the colours on screen: the
    /// full wording, a short form for narrow panes, and whether it is a
    /// warning. Neither form ever calls outdated or partial data current.
    pub fn hl_label(&self, hl: Option<&FileHighlight>) -> (String, &'static str, bool) {
        match (self.hl.mode, hl) {
            (Mode::Off, _) => ("plain".into(), "plain", false),
            (_, None) => ("plain (colours loading)".into(), "plain", false),
            (Mode::Syntax, Some(_)) => ("syntax".into(), "syntax", false),
            (Mode::Semantic, Some(h)) => {
                let Some(r) = &h.facts.record else { return ("syntax (no semantic tokens cached)".into(), "syntax", false) };
                let partial = r.status == Coverage::Partial;
                let cov = if partial { "partial" } else { "complete" };
                match (h.facts.usable(), self.hl.stale_preview) {
                    (Usable::Current, _) => (format!("syntax + semantic ({cov})"), if partial { "syntax + PARTIAL semantic" } else { "syntax + semantic" }, partial),
                    (Usable::StaleMeaning, true) => (format!("syntax + STALE semantic preview ({cov}; inputs changed)"), "syntax + STALE semantic", true),
                    (Usable::StaleMeaning, false) => ("syntax (semantic tokens stale: inputs changed)".into(), "syntax, semantic STALE", true),
                    (Usable::Checking, _) => ("syntax (semantic: checking imports)".into(), "syntax (checking)", false),
                    (Usable::No, _) => (format!("syntax ({})", h.facts.short()), if h.facts.aligned { "syntax, no semantic" } else { "syntax, semantic OUTDATED" }, !h.facts.aligned),
                }
            }
        }
    }

    /// One source line as styled spans per display row. Falls back to
    /// plain text when there are no spans for the line (yet).
    pub fn hl_rows(&self, hl: Option<&FileHighlight>, line0: usize, text: &str, width: usize, base: Style, on_line: bool) -> Vec<Vec<Span<'static>>> {
        let mut spans = Vec::new();
        if let Some(h) = hl {
            self.hl.styler.borrow_mut().line_spans(h, line0, self.hl_use_semantic(h), &mut spans);
        }
        if spans.is_empty() {
            return wrap_line(&sanitize(text, self.ascii), width, 2).into_iter().map(|r| vec![Span::styled(r, base)]).collect();
        }
        if let [(_, only)] = spans[..] {
            // One style for the whole line (a comment line, or a line too
            // long to tokenize): no per-character work.
            let st = self.theme.tok_style(base, only, on_line);
            return wrap_line(&sanitize(text, self.ascii), width, 2).into_iter().map(|r| vec![Span::styled(r, st)]).collect();
        }
        styled_rows(text, &spans, width, 2, self.ascii)
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|(s, st)| match st {
                        Some(tok) => Span::styled(s, self.theme.tok_style(base, tok, on_line)),
                        None => Span::styled(s, base),
                    })
                    .collect()
            })
            .collect()
    }

    /// [`App::hl_rows`] with byte ranges of the line to point at (where an
    /// identifier selected elsewhere occurs): they are drawn in the
    /// selection style, over whatever colours the tokens have.
    pub fn hl_rows_marked(&self, hl: Option<&FileHighlight>, line0: usize, text: &str, width: usize, base: Style, on_line: bool, marks: &[(u32, u32)]) -> Vec<Vec<Span<'static>>> {
        if marks.is_empty() {
            return self.hl_rows(hl, line0, text, width, base, on_line);
        }
        let mut spans = Vec::new();
        if let Some(h) = hl {
            self.hl.styler.borrow_mut().line_spans(h, line0, self.hl_use_semantic(h), &mut spans);
        }
        crate::highlight::render::styled_rows_marked(text, &spans, marks, width, 2, self.ascii)
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|(s, st, marked)| {
                        let style = match st {
                            Some(tok) => self.theme.tok_style(base, tok, on_line),
                            None => base,
                        };
                        Span::styled(s, if marked { self.theme.selected } else { style })
                    })
                    .collect()
            })
            .collect()
    }

    // --- commands ---

    pub fn hl_cycle_mode(&mut self) {
        self.hl.mode = self.hl.mode.next();
        let name = match self.hl.mode {
            Mode::Off => "off",
            Mode::Syntax => "syntax",
            Mode::Semantic => "semantic",
        };
        self.set_pref("hl_mode", Some(name.to_string()));
        self.msg(format!("highlighting: {}", self.hl.mode.label()));
        self.hl_reload();
    }

    pub fn hl_cycle_theme(&mut self) {
        let mut order: Vec<Option<String>> = vec![None];
        order.extend(SyntaxTheme::BUILTIN_NAMES.iter().map(|n| Some(n.to_string())));
        if let Some(u) = &self.hl.user_theme {
            order.push(Some(u.clone()));
        }
        let i = order.iter().position(|o| *o == self.hl.theme_choice).unwrap_or(0);
        self.hl.theme_choice = order[(i + 1) % order.len()].clone();
        self.hl_apply_theme();
        let name = self.hl.styler.borrow().theme().name.clone();
        self.msg(match &self.hl.theme_choice {
            None => format!("syntax theme: {name} (follows the UI theme)"),
            Some(_) => format!("syntax theme: {name}"),
        });
    }

    pub fn hl_toggle_stale(&mut self) {
        self.hl.stale_preview = !self.hl.stale_preview;
        self.center.version += 1;
        self.msg(if self.hl.stale_preview {
            "stale preview ON: semantic colours whose inputs changed are shown, labelled STALE"
        } else {
            "stale preview off: only current semantic colours are shown"
        });
    }

    pub fn hl_recheck(&mut self) {
        if let Some(tx) = &self.hl.tx {
            let _ = tx.send(HlReq::Recheck { exact: true });
        }
        self.hl_reload();
    }

    fn hl_options(&self) -> Options {
        let mut o = Options::default();
        // Under an outer resource guard the limit must stay the guard's.
        if std::env::var("SLIM_HL_CGROUP").ok().as_deref() == Some("none") {
            o.cgroup = false;
        }
        if let Some(m) = std::env::var("SLIM_HL_MEMORY_MIB").ok().and_then(|v| v.parse::<u64>().ok()) {
            o.memory_mib = m.max(256);
        }
        if let Some(t) = std::env::var("SLIM_HL_FILE_TIMEOUT").ok().and_then(|v| v.parse::<u64>().ok()) {
            o.file_timeout = Duration::from_secs(t.max(1));
        }
        o
    }

    /// Plans the indexing job (for the project, or only the file on screen)
    /// and shows the plan for confirmation. Nothing runs before `yes`.
    pub fn hl_build(&mut self, only_current_file: bool) {
        let Some(root) = self.ctx.project_root.clone() else {
            self.msg("semantic highlighting needs a project directory (slim <PROJECT_DIR>)");
            return;
        };
        if !matches!(self.hl.sem, SemJob::Idle) {
            self.msg("semantic indexing is already planned or running (F6 shows it)");
            return;
        }
        let mut opts = self.hl_options();
        if only_current_file {
            let rel = match (self.center_mode, &self.center.source, &self.detail) {
                (CenterMode::Source, Some(s), _) => Some(s.rel_path.clone()),
                (_, _, Some(d)) => d.d.module_path.clone(),
                _ => None,
            };
            let Some(rel) = rel else {
                self.msg("no source file is open");
                return;
            };
            opts.files = vec![rel];
            opts.refresh = true;
        }
        let Some(tx) = self.ch.events.clone() else { return };
        self.hl.sem = SemJob::Planning;
        self.msg("planning semantic indexing (nothing runs yet)…");
        std::thread::Builder::new()
            .name("slim-semhl-plan".into())
            .spawn(move || {
                let r = indexer::plan(&root, &opts).map(Box::new).map_err(|e| format!("{e:#}"));
                let _ = tx.send(Event::SemIndex(SemEvent::Planned(r)));
            })
            .ok();
    }

    /// The reader typed `yes`: start the job on its own thread.
    pub fn hl_confirmed(&mut self) {
        let SemJob::Confirm(plan) = std::mem::replace(&mut self.hl.sem, SemJob::Idle) else { return };
        let Some(tx) = self.ch.events.clone() else { return };
        let ctl = Arc::new(Control::default());
        let c2 = ctl.clone();
        let handle = std::thread::Builder::new()
            .name("slim-semhl".into())
            .spawn(move || {
                let r = indexer::run(&plan, &c2, &mut |e| {
                    let _ = match e {
                        indexer::Event::Progress(p) => tx.send(Event::SemIndex(SemEvent::Progress(p.clone()))),
                        indexer::Event::Published { rel, status, .. } => tx.send(Event::SemIndex(SemEvent::Published { rel: rel.to_string(), status })),
                    };
                });
                let _ = tx.send(Event::SemIndex(SemEvent::Finished(r.map_err(|e| format!("{e:#}")))));
            })
            .ok();
        self.hl.sem = SemJob::Running { progress: Progress::default(), ctl, handle, cancel_at: None };
        self.msg("semantic indexing started (browsing stays available; F6 details; the palette cancels it)");
    }

    pub fn hl_cancel(&mut self) {
        match &mut self.hl.sem {
            SemJob::Running { ctl, cancel_at, .. } => {
                ctl.cancel.store(true, Ordering::Relaxed);
                cancel_at.get_or_insert_with(Instant::now);
                self.msg("cancelling semantic indexing: stopping the Lean server (finished files are kept)…");
            }
            SemJob::Confirm(_) | SemJob::Planning => {
                self.hl.sem = SemJob::Idle;
                self.msg("semantic indexing not started");
            }
            SemJob::Idle => self.msg("no semantic indexing is running"),
        }
    }

    pub fn hl_pause(&mut self) {
        let msg = match &self.hl.sem {
            SemJob::Running { ctl, .. } => {
                let p = !ctl.pause.load(Ordering::Relaxed);
                ctl.pause.store(p, Ordering::Relaxed);
                if p {
                    "semantic indexing paused after the current file (the same command resumes)"
                } else {
                    "semantic indexing resumed"
                }
            }
            _ => "no semantic indexing is running",
        };
        self.msg(msg);
    }

    pub fn on_sem_event(&mut self, e: SemEvent) {
        self.dirty = true;
        match e {
            SemEvent::Planned(Ok(plan)) => {
                if matches!(self.hl.sem, SemJob::Planning) {
                    self.overlay = Overlay::ConfirmHighlight { plan: plan.describe(), typed: String::new() };
                    self.hl.sem = SemJob::Confirm(plan);
                }
            }
            SemEvent::Planned(Err(e)) => {
                self.hl.sem = SemJob::Idle;
                self.warn(format!("semantic highlighting unavailable: {e}"));
            }
            SemEvent::Progress(p) => {
                if let SemJob::Running { progress, .. } = &mut self.hl.sem {
                    *progress = p;
                }
            }
            SemEvent::Published { rel, .. } => {
                if let Some(tx) = &self.hl.tx {
                    let _ = tx.send(HlReq::Invalidate { rel: Some(rel.clone()) });
                }
                if self.hl.wanted.as_deref() == Some(rel.as_str()) {
                    // New tokens for the file on screen: scroll and selection stay.
                    self.hl.gen += 1;
                    if let Some(tx) = &self.hl.tx {
                        let first = self.center.source_scroll;
                        let _ = tx.send(HlReq::Open { gen: self.hl.gen, rel, first, last: first + self.last_body_height.max(40) });
                    }
                }
            }
            SemEvent::Finished(res) => {
                let cancel_at = match std::mem::replace(&mut self.hl.sem, SemJob::Idle) {
                    SemJob::Running { handle, cancel_at, .. } => {
                        if let Some(h) = handle {
                            let _ = h.join();
                        }
                        cancel_at
                    }
                    _ => None,
                };
                match res {
                    Ok(s) => {
                        let c = &s.counts;
                        let text = format!(
                            "semantic indexing {}: {} complete, {} partial, {} failed, {} skipped, {} already current, {} not reached, in {}{}{}",
                            s.outcome.as_str(),
                            c.complete,
                            c.partial,
                            c.failed,
                            c.skipped,
                            c.fresh,
                            s.remaining(),
                            fmt_duration(s.elapsed),
                            cancel_at.map(|t| format!("; stopped {} ms after the request", t.elapsed().as_millis())).unwrap_or_default(),
                            if s.shutdown.lingering > 0 { format!("; WARNING: {} server process(es) still alive", s.shutdown.lingering) } else { String::new() },
                        );
                        self.hl.last_summary = Some(match &s.outcome {
                            Outcome::ServerFailed(e) => format!("{text} — {e}"),
                            _ => text.clone(),
                        });
                        if matches!(s.outcome, Outcome::ServerFailed(_)) || s.shutdown.lingering > 0 {
                            self.warn(self.hl.last_summary.clone().unwrap());
                        } else {
                            self.msg(text);
                        }
                    }
                    Err(e) => {
                        self.hl.last_summary = Some(format!("semantic indexing failed: {e}"));
                        self.warn(format!("semantic indexing failed: {e}"));
                    }
                }
                if let Some(tx) = &self.hl.tx {
                    let _ = tx.send(HlReq::Invalidate { rel: None });
                }
                self.hl_reload();
            }
        }
    }

    /// Short progress text for the title bar while the job runs.
    pub fn hl_job_label(&self) -> Option<String> {
        match &self.hl.sem {
            SemJob::Planning => Some("planning semantic indexing".into()),
            SemJob::Confirm(_) => None,
            SemJob::Idle => None,
            SemJob::Running { progress: p, cancel_at, .. } => {
                let c = &p.counts;
                Some(format!(
                    "semantic index {}/{}{}{} {}{}",
                    c.done(),
                    c.total,
                    if c.failed + c.partial > 0 { format!(" ({} failed, {} partial)", c.failed, c.partial) } else { String::new() },
                    if p.paused { " PAUSED" } else { "" },
                    fmt_duration(p.elapsed),
                    if cancel_at.is_some() { " cancelling…" } else { "" },
                ))
            }
        }
    }

    /// Everything known about highlighting right now, for the details overlay.
    pub fn hl_status_lines(&self) -> Vec<(String, bool)> {
        let mut v: Vec<(String, bool)> = Vec::new();
        let head = |s: &str| (s.to_string(), false);
        v.push(head("SETTINGS"));
        v.push((format!("  Highlighting mode: {} (F4 cycles: syntax + semantic, syntax, off)", self.hl.mode.label()), false));
        {
            let st = self.hl.styler.borrow();
            let t = st.theme();
            v.push((
                format!(
                    "  Syntax theme: {}{} (F5 cycles; a theme change re-resolves styles, cached tokens stay)",
                    t.name,
                    if self.hl.theme_choice.is_none() { ", following the UI theme" } else { "" }
                ),
                false,
            ));
            if self.theme.syntax.mono {
                v.push(("  The monochrome UI theme shows only bold, italic and underline of the syntax theme.".into(), false));
            } else if !self.theme.syntax.truecolor {
                v.push(("  This terminal does not advertise 24-bit colour: colours are the nearest xterm-256 ones.".into(), false));
            }
        }
        v.push((format!("  Stale preview: {} (palette: Semantic highlighting: stale preview)", if self.hl.stale_preview { "ON" } else { "off" }), self.hl.stale_preview));
        v.push((format!("  Baseline provider: {}", crate::highlight::textmate::baseline_identity()), false));
        v.push(head(""));
        v.push(head("THIS FILE"));
        match (&self.hl.file, &self.hl.wanted) {
            (Some(f), _) => {
                v.push((format!("  {} — {} lines", f.rel, f.n_lines), false));
                v.push((
                    format!(
                        "  Syntax spans: {} of {} lines{}",
                        f.baseline_lines_done.min(f.n_lines),
                        f.n_lines,
                        if f.baseline_from_cache { " (read from the cache)" } else if f.baseline_complete() { " (tokenized now)" } else { " (the rest is being tokenized)" }
                    ),
                    false,
                ));
                let (label, _, warn) = self.hl_label(Some(f));
                v.push((format!("  Shown now: {label}"), warn));
                for (l, warn) in f.facts.lines() {
                    v.push((format!("  {l}"), warn));
                }
                v.push((format!("  Worker cache: {:.1} MiB of {} MiB", f.cache_bytes as f64 / 1048576.0, self.hl.budget_bytes >> 20), false));
            }
            (None, Some(w)) => v.push((format!("  {w}: waiting for the highlight worker"), false)),
            (None, None) => v.push(("  No source file is shown (s opens the whole file of the current declaration).".into(), false)),
        }
        if let Some(e) = &self.hl.error {
            v.push((format!("  Last error: {e}"), true));
        }
        v.push(head(""));
        v.push(head("SEMANTIC INDEXING"));
        match &self.hl.sem {
            SemJob::Idle => v.push(("  Not running. Browsing never starts Lean; the palette (:) has \"Semantic highlighting: build index…\".".into(), false)),
            SemJob::Planning => v.push(("  Planning (reading the project configuration; nothing runs yet).".into(), false)),
            SemJob::Confirm(_) => v.push(("  Waiting for your confirmation.".into(), false)),
            SemJob::Running { progress: p, cancel_at, .. } => {
                let c = &p.counts;
                v.push((format!("  Running for {}{}{}.", fmt_duration(p.elapsed), if p.paused { ", PAUSED" } else { "" }, if cancel_at.is_some() { ", cancelling" } else { "" }), false));
                v.push((
                    format!(
                        "  Files: {} of {} done — {} attempted, {} complete, {} partial, {} failed, {} skipped, {} already current.",
                        c.done(),
                        c.total,
                        c.attempted,
                        c.complete,
                        c.partial,
                        c.failed,
                        c.skipped,
                        c.fresh
                    ),
                    c.failed > 0,
                ));
                for f in &p.current {
                    v.push((format!("  Current file: {f} ({} s)", p.current_secs), false));
                }
                match p.tree_rss_kib {
                    Some(k) => v.push((format!("  Lean server: {} MiB resident over {} processes (shared pages counted per process).", k / 1024, p.tree_procs), false)),
                    None => v.push(("  Lean server memory: not measured yet.".into(), false)),
                }
                if c.attempted >= 20 && c.total > c.done() {
                    let per = p.elapsed.as_secs_f64() / c.attempted as f64;
                    v.push((format!("  At {per:.1} s per attempted file so far, the remaining {} would take about {} (files differ widely).", c.total - c.done(), fmt_duration(Duration::from_secs_f64(per * (c.total - c.done()) as f64))), false));
                } else {
                    v.push(("  No time estimate yet (too few files done).".into(), false));
                }
            }
        }
        if let Some(s) = &self.hl.last_summary {
            v.push((format!("  Last run: {s}"), false));
        }
        v.push(head(""));
        v.push(head("COMMANDS (palette, :)"));
        for l in [
            "  Semantic highlighting: build index…         plan, show what runs, ask for yes",
            "  Semantic highlighting: refresh this file…   the same for the file on screen",
            "  Semantic highlighting: pause / resume, cancel",
            "  Semantic highlighting: stale preview        show aligned but outdated tokens, labelled",
            "  R                                           compare sources, environment and imports again (hashing everything)",
            "  slim highlight status|clean <DIR>       cache contents, freshness, maintenance",
        ] {
            v.push(head(l));
        }
        v
    }

    fn set_pref(&mut self, key: &str, value: Option<String>) {
        if let Some(q) = &self.ch.query {
            let _ = q.send(super::worker::QueryReq::SetPref { key: key.to_string(), value });
        }
    }

    /// Stops the worker and any indexing job; returns threads to join.
    pub fn hl_shutdown(&mut self) -> Vec<std::thread::JoinHandle<()>> {
        let mut v = Vec::new();
        if let SemJob::Running { ctl, handle, .. } = &mut self.hl.sem {
            ctl.cancel.store(true, Ordering::Relaxed);
            v.extend(handle.take());
        }
        if let Some(tx) = self.hl.tx.take() {
            let _ = tx.send(HlReq::Shutdown);
        }
        v.extend(self.hl.worker.take());
        v
    }
}

#[cfg(test)]
mod tests {
    use super::super::state::*;
    use super::super::theme::{contrast, Theme, ThemeName, LEAN_DARK, LEAN_LIGHT};
    use super::super::worker::FullSource;
    use super::*;
    use crate::db::IndexMode;
    use crate::highlight::service::{EnvState, Service, Upstream};
    use crate::highlight::store::{SemMeta, SemRecord, Store};
    use crate::highlight::{Legend, Lines, SemTok};
    use ratatui::backend::TestBackend;
    use ratatui::style::{Color, Modifier};
    use ratatui::Terminal;
    use std::sync::atomic::{AtomicBool, AtomicU64};

    const SRC: &str = "/-- Doc with `code`. -/\ntheorem foo (h : P) : P := by\n\texact h -- done\ndef s := \"str\" ++ 𝔽α.field\nexample : True := sorry\n";

    fn app(theme: ThemeName) -> App {
        let ctx = Ctx { title: "t".into(), mode: IndexMode::Source, index: None, user_key: "lake:t".into(), editor: None, info: None, synthetic: false, project_root: None };
        let ch = Channels { query: None, search: None, jobs: None, events: None, search_latest: Arc::new(AtomicU64::new(0)), job_cancel: Arc::new(AtomicBool::new(false)) };
        let mut a = App::new(ctx, theme, false, ch);
        // Independent of the environment the tests run in.
        a.theme = match theme {
            ThemeName::LeanDark => Theme::from_palette(theme, &LEAN_DARK, true),
            ThemeName::LeanLight => Theme::from_palette(theme, &LEAN_LIGHT, true),
            _ => {
                let mut t = Theme::new(theme);
                t.syntax.truecolor = true;
                t
            }
        };
        a.hl_apply_theme();
        a.ctx.index = Some(crate::app::worker::IndexCtx { index: "x".into(), source_root: None, mode: IndexMode::Source, user_key: "k".into(), encoding: crate::text::PosEncoding::Utf8, map_path: None });
        a
    }

    /// A project on disk with `SRC` and, optionally, cached semantic tokens
    /// marking `h` (variable) and `field` (property); returns the snapshot
    /// the worker would send.
    fn snapshot(with_semantic: bool, text: &str) -> (tempfile::TempDir, Arc<FileHighlight>) {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("A.lean"), text).unwrap();
        let store_path = d.path().join("cache/highlight.sqlite");
        if with_semantic {
            let store = Store::open(&store_path).unwrap();
            let env = crate::highlight::env::EnvRecord { info: crate::highlight::env::EnvInfo::current(d.path()), ..Default::default() };
            let fp = store.put_env(&env).unwrap();
            let lines: Vec<&str> = text.split('\n').collect();
            let mut l = Lines::default();
            for line in &lines {
                let mut toks = Vec::new();
                for (word, ty) in [(" h", 1u16), ("field", 2)] {
                    if let Some(i) = line.find(word) {
                        let start = i + word.len() - word.trim_start().len();
                        toks.push(SemTok { start: start as u32, end: (i + word.len()) as u32, ty, mods: 0 });
                    }
                }
                toks.sort_by_key(|t| t.start);
                l.push_line(toks);
            }
            let mut tracker = crate::highlight::env::DepTracker::new(d.path(), None);
            let meta = SemMeta {
                path: "A.lean".into(),
                source_sha256: crate::paths::sha256_hex(text.as_bytes()),
                source_size: text.len() as u64,
                line_endings: "lf".into(),
                status: Coverage::Complete,
                detail: "processed completely".into(),
                n_lines: lines.len(),
                n_tokens: l.token_count(),
                processed_lines: None,
                legend: Legend { types: vec!["keyword".into(), "variable".into(), "property".into()], modifiers: vec![] },
                position_encoding: "utf-16".into(),
                diag_errors: 0,
                diag_warnings: 0,
                env_fingerprint: fp,
                deps_digest: tracker.closure("A.lean", None).digest,
                deps_count: 0,
                artifacts_digest: String::new(),
                build_note: "no local imports".into(),
                extractor: crate::highlight::EXTRACTOR_VERSION.into(),
                elapsed_ms: 5,
                created: "2026-01-01T00:00:00Z".into(),
            };
            let blob = crate::highlight::codec::encode_semantic(&[Arc::new(l)], lines.len());
            store.put_semantic(&SemRecord { meta, blob: Some(blob) }).unwrap();
        }
        let mut svc = Service::new(d.path().to_path_buf(), store_path, None);
        let mut last = None;
        svc.open(1, "A.lean", 100, &mut |r| {
            if let HlResp::File { hl, .. } = r {
                last = Some(hl);
            }
        });
        (d, last.unwrap())
    }

    fn show(a: &mut App, hl: &Arc<FileHighlight>, text: &str, highlight: Option<(usize, usize)>) {
        a.center_mode = CenterMode::Source;
        a.focus = Focus::Center;
        a.center.source = Some(FullSource {
            rel_path: "A.lean".into(),
            sha256: hl.sha256.clone(),
            lines: text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_string()).collect(),
            focus_line: 1,
            highlight,
            changed: false,
        });
        a.hl.wanted = Some("A.lean".into());
        a.on_highlight(HlResp::File { gen: 1, hl: hl.clone() });
    }

    fn draw(a: &mut App, w: u16, h: u16) -> Terminal<TestBackend> {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| super::super::render::draw(f, a)).unwrap();
        t
    }

    /// Foreground, background and modifiers of the first cell showing `needle`.
    fn cell_of(t: &Terminal<TestBackend>, needle: &str) -> (Color, Color, Modifier) {
        let b = t.backend().buffer();
        for y in 0..b.area.height {
            let row: Vec<&str> = (0..b.area.width).map(|x| b[(x, y)].symbol()).collect();
            let line: String = row.concat();
            if let Some(byte) = line.find(needle) {
                // Column of that byte (wide glyphs occupy a cell plus an empty one).
                let mut acc = 0;
                for (x, s) in row.iter().enumerate() {
                    if acc == byte {
                        let c = &b[(x as u16, y)];
                        return (c.fg, c.bg, c.modifier);
                    }
                    acc += s.len();
                }
            }
        }
        panic!("{needle:?} is not on screen:\n{}", text_of(t));
    }

    fn text_of(t: &Terminal<TestBackend>) -> String {
        let b = t.backend().buffer();
        (0..b.area.height).map(|y| (0..b.area.width).map(|x| b[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
    }

    fn rgb(h: &str) -> Color {
        let c = crate::highlight::theme::parse_hex(h).unwrap();
        Color::Rgb(c[0], c[1], c[2])
    }

    #[test]
    fn source_view_shows_syntax_then_semantic_colours_with_the_overlay_on_top() {
        let (_d, hl) = snapshot(true, SRC);
        assert_eq!(hl.facts.usable(), Usable::Current);
        let mut a = app(ThemeName::LeanDark);
        // Lines 2–3 are the current declaration: the line overlay.
        show(&mut a, &hl, SRC, Some((2, 3)));
        assert!(text_of(&draw(&mut a, 190, 20)).contains("syntax + semantic (complete)"));
        let t = draw(&mut a, 120, 20);
        assert!(text_of(&t).contains("│ syntax + semantic "), "{}", text_of(&t));
        let bg = rgb(LEAN_DARK.bg);
        let line_bg = rgb(LEAN_DARK.highlight_bg);
        // Baseline: comment, keyword, declaration name, string, sorry.
        assert_eq!(cell_of(&t, "/-- Doc").0, rgb("#6a9955"));
        assert_eq!(cell_of(&t, "theorem").0, rgb("#569cd6"));
        assert_eq!(cell_of(&t, "foo").0, rgb("#dcdcaa"));
        assert_eq!(cell_of(&t, "\"str\"").0, rgb("#ce9178"));
        assert_eq!(cell_of(&t, "sorry").0, rgb("#f44747"));
        // Semantic over baseline: the use of the hypothesis `h` and the projection.
        assert_eq!(cell_of(&t, "h -- done").0, rgb("#9cdcfe"));
        assert_eq!(cell_of(&t, "field").0, rgb("#9cdcfe"));
        // Unstyled text keeps the view's own colours.
        assert_eq!(cell_of(&t, ": True").0, a.theme.text.fg.unwrap());
        // The overlay supplies the background; token colours stay on it.
        assert_eq!(cell_of(&t, "theorem").1, line_bg);
        assert_eq!(cell_of(&t, "exact h").1, line_bg);
        assert_eq!(cell_of(&t, "-- done").0, rgb("#6a9955"));
        assert_eq!(cell_of(&t, "/-- Doc").1, bg);
        // Semantic off: `h` falls back to plain text; syntax stays.
        a.hl.mode = Mode::Syntax;
        let t = draw(&mut a, 120, 20);
        assert_ne!(cell_of(&t, "h -- done").0, rgb("#9cdcfe"));
        assert_eq!(cell_of(&t, "theorem").0, rgb("#569cd6"));
        // Off: no token colour anywhere.
        a.hl.mode = Mode::Off;
        let t = draw(&mut a, 120, 20);
        assert_ne!(cell_of(&t, "theorem").0, rgb("#569cd6"));
        assert!(text_of(&t).contains("│ plain"));
    }

    #[test]
    fn highlighting_never_changes_the_text_or_its_wrapping() {
        // Tabs, non-BMP letters, wide glyphs, combining marks, a control
        // character and long lines, at several widths.
        let text = format!("{SRC}\tdef 中文 := e{}x {}[31m 𝔽𝕢\n-- {}\n", '\u{301}', '\u{1b}', "long comment words ".repeat(20));
        let (_d, hl) = snapshot(true, &text);
        for (w, h) in [(40u16, 30u16), (63, 30), (120, 40), (200, 50)] {
            let mut a = app(ThemeName::LeanDark);
            show(&mut a, &hl, &text, Some((2, 2)));
            let coloured = text_of(&draw(&mut a, w, h));
            a.hl.mode = Mode::Off;
            let plain = text_of(&draw(&mut a, w, h));
            // Only the label in the pane title differs.
            let body = |s: &str| s.lines().skip(3).collect::<Vec<_>>().join("\n");
            assert_eq!(body(&coloured), body(&plain), "{w}x{h}");
        }
    }

    #[test]
    fn spans_for_other_text_are_never_applied() {
        let (_d, hl) = snapshot(true, SRC);
        let mut a = app(ThemeName::LeanDark);
        show(&mut a, &hl, SRC, None);
        // The file on screen is a different version than the snapshot's.
        a.center.source.as_mut().unwrap().sha256 = "another".into();
        a.center.source.as_mut().unwrap().lines[1] = "xx theorem foo".into();
        let t = draw(&mut a, 120, 20);
        assert_ne!(cell_of(&t, "theorem").0, rgb("#569cd6"), "shifted text must not get old colours");
        assert!(text_of(&t).contains("colours loading"));
    }

    #[test]
    fn stale_semantic_tokens_need_the_explicit_preview_and_are_labelled() {
        let (_d, hl) = snapshot(true, SRC);
        // The same snapshot with an import changed since extraction.
        let stale = Arc::new(FileHighlight {
            rel: hl.rel.clone(),
            sha256: hl.sha256.clone(),
            n_lines: hl.n_lines,
            baseline: hl.baseline.clone(),
            baseline_lines_done: hl.baseline_lines_done,
            baseline_from_cache: false,
            semantic: hl.semantic.as_ref().map(|s| crate::highlight::service::SemLayer { layer: s.layer.clone(), legend: s.legend.clone(), processed_lines: None }),
            facts: crate::highlight::service::SemFacts { upstream: Upstream::Changed, ..hl.facts.clone() },
            cache_bytes: 0,
        });
        let mut a = app(ThemeName::LeanDark);
        show(&mut a, &stale, SRC, None);
        let t = draw(&mut a, 100, 20);
        assert!(text_of(&t).contains("syntax, semantic STALE"), "narrow panes keep the warning: {}", text_of(&t));
        let t = draw(&mut a, 190, 20);
        assert_ne!(cell_of(&t, "h -- done").0, rgb("#9cdcfe"), "stale tokens are not shown by default");
        assert_eq!(cell_of(&t, "theorem").0, rgb("#569cd6"), "current baseline colours are");
        assert!(text_of(&t).contains("semantic tokens stale: inputs changed"), "{}", text_of(&t));
        a.hl_toggle_stale();
        let t = draw(&mut a, 190, 20);
        assert_eq!(cell_of(&t, "h -- done").0, rgb("#9cdcfe"));
        assert!(text_of(&t).contains("STALE semantic preview"), "never labelled current: {}", text_of(&t));
        // While imports are still being checked nothing is called current.
        let checking = Arc::new(FileHighlight {
            rel: hl.rel.clone(),
            sha256: hl.sha256.clone(),
            n_lines: hl.n_lines,
            baseline: hl.baseline.clone(),
            baseline_lines_done: hl.baseline_lines_done,
            baseline_from_cache: false,
            semantic: hl.semantic.as_ref().map(|s| crate::highlight::service::SemLayer { layer: s.layer.clone(), legend: s.legend.clone(), processed_lines: None }),
            facts: crate::highlight::service::SemFacts { upstream: Upstream::Checking, env: EnvState::Same, ..hl.facts.clone() },
            cache_bytes: 0,
        });
        a.hl.stale_preview = false;
        a.on_highlight(HlResp::File { gen: 2, hl: checking });
        let t = draw(&mut a, 190, 20);
        assert!(text_of(&t).contains("checking imports"));
        assert_ne!(cell_of(&t, "h -- done").0, rgb("#9cdcfe"));
        // The details overlay states each fact on its own line.
        a.overlay = Overlay::HlStatus { scroll: 0 };
        let s = text_of(&draw(&mut a, 130, 44));
        for needle in ["Source alignment:", "Extraction coverage: complete", "Semantic environment:", "Imported sources: being checked", "Build relation:"] {
            assert!(s.contains(needle), "{needle} missing:\n{s}");
        }
    }

    #[test]
    fn theme_change_resolves_styles_again_without_new_tokens() {
        let (_d, hl) = snapshot(true, SRC);
        let mut a = app(ThemeName::LeanDark);
        show(&mut a, &hl, SRC, None);
        let scroll = a.center.source_scroll;
        // The UI theme changes to light: Light+ follows.
        a.theme = Theme::from_palette(ThemeName::LeanLight, &LEAN_LIGHT, true);
        a.hl_apply_theme();
        let t = draw(&mut a, 120, 20);
        assert_eq!(cell_of(&t, "theorem").0, rgb("#0000ff"));
        assert_eq!(cell_of(&t, "/-- Doc").0, rgb("#008000"));
        assert_eq!(cell_of(&t, "h -- done").0, rgb("#001080"));
        // An explicit syntax theme that does not suit the background: colours
        // too close to it are dropped, the text stays readable.
        a.hl.theme_choice = Some("dark-plus".into());
        a.hl_apply_theme();
        let t = draw(&mut a, 120, 20);
        let light_bg = super::super::theme::hex_rgb(LEAN_LIGHT.bg);
        let Color::Rgb(r, g, b) = cell_of(&t, "foo").0 else { panic!() };
        assert!(contrast((r, g, b), light_bg) >= 3.0, "Dark+ yellow on a light background was replaced");
        assert_eq!(cell_of(&t, "foo").0, a.theme.text.fg.unwrap());
        // The same snapshot object throughout; scroll untouched.
        assert!(Arc::ptr_eq(a.hl.file.as_ref().unwrap(), &hl));
        assert_eq!(a.center.source_scroll, scroll);
    }

    #[test]
    fn monochrome_and_256_colour_terminals() {
        let text = "/-- # Heading\n-/\ndef x := 1\n";
        let (_d, hl) = snapshot(false, text);
        let mut a = app(ThemeName::Mono);
        show(&mut a, &hl, text, None);
        let t = draw(&mut a, 100, 12);
        let (fg, _, m) = cell_of(&t, "Heading");
        assert_eq!(fg, Color::Reset, "no colours in the monochrome theme");
        assert!(m.contains(Modifier::BOLD), "font styles carry over");
        assert_eq!(cell_of(&t, "def").0, Color::Reset);
        // 256 colours: indexed approximations that keep their contrast.
        let mut a = app(ThemeName::LeanDark);
        a.theme = Theme::from_palette(ThemeName::LeanDark, &LEAN_DARK, false);
        show(&mut a, &hl, text, None);
        let t = draw(&mut a, 100, 12);
        let Color::Indexed(i) = cell_of(&t, "def").0 else { panic!("expected an indexed colour") };
        let bg = super::super::theme::xterm_rgb(super::super::theme::nearest_256(0x18, 0x18, 0x18));
        assert!(contrast(super::super::theme::xterm_rgb(i), bg) >= 4.5);
    }

    #[test]
    fn default_theme_pairs_lose_no_colour_and_leave_no_default_cells() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/highlight");
        for (theme, palette, syntax) in [(ThemeName::LeanDark, &LEAN_DARK, "dark-plus"), (ThemeName::LeanLight, &LEAN_LIGHT, "light-plus")] {
            // Every colour the grammar's scopes resolve to on the fixtures is
            // legible on the theme's background and on the highlighted line.
            let st = crate::highlight::theme::SyntaxTheme::builtin(syntax).unwrap();
            let mut b = crate::highlight::baseline::BaselineBuilder::new();
            let mut worst = (f64::MAX, String::new());
            for name in ["Basics.lean", "Unicode.lean", "Meta.lean", "Markdown.lean", "Edge.lean"] {
                let text = std::fs::read_to_string(root.join(name)).unwrap();
                b.reset();
                let mut toks = Vec::new();
                for line in crate::highlight::baseline::lines(&text) {
                    b.feed(line, &mut toks);
                    for &(_, id) in &toks {
                        if let Some(c) = st.scope_style(&crate::highlight::textmate::scopes().path(id)).fg {
                            for bg in [palette.bg, palette.highlight_bg] {
                                let k = contrast((c[0], c[1], c[2]), super::super::theme::hex_rgb(bg));
                                if k < worst.0 {
                                    worst = (k, format!("#{:02x}{:02x}{:02x} on {bg} ({})", c[0], c[1], c[2], crate::highlight::textmate::scopes().display(id)));
                                }
                            }
                        }
                    }
                }
            }
            assert!(worst.0 >= super::super::theme::MIN_SYNTAX_CONTRAST, "{syntax}: {} has contrast {:.2}", worst.1, worst.0);
            for ty in ["keyword", "variable", "property", "function"] {
                let c = st.semantic_style(ty, &[]).fg.unwrap();
                for bg in [palette.bg, palette.highlight_bg] {
                    assert!(contrast((c[0], c[1], c[2]), super::super::theme::hex_rgb(bg)) >= super::super::theme::MIN_SYNTAX_CONTRAST, "{syntax} {ty}");
                }
            }
            // The themed background is painted under every highlighted cell
            // and under the highlighting overlays.
            let (_d, hl) = snapshot(true, SRC);
            for overlay in [0, 1, 2] {
                let mut a = app(theme);
                show(&mut a, &hl, SRC, Some((2, 3)));
                a.overlay = match overlay {
                    0 => Overlay::None,
                    1 => Overlay::HlStatus { scroll: 0 },
                    _ => Overlay::ConfirmHighlight { plan: "plan\n  line".into(), typed: "y".into() },
                };
                let t = draw(&mut a, 110, 30);
                let buf = t.backend().buffer();
                for y in 0..30 {
                    for x in 0..110 {
                        let c = &buf[(x, y)];
                        assert!(c.bg != Color::Reset && c.fg != Color::Reset, "{theme:?} overlay {overlay}: cell ({x},{y}) {:?} uses the terminal default", c.symbol());
                    }
                }
            }
        }
    }

    #[test]
    fn a_new_snapshot_keeps_scroll_and_only_applies_to_the_wanted_file() {
        let body: String = (0..400).map(|i| format!("def d{i} := {i}\n")).collect();
        let (_d, hl) = snapshot(false, &body);
        let mut a = app(ThemeName::LeanDark);
        show(&mut a, &hl, &body, None);
        a.center.source_scroll = 250;
        a.on_highlight(HlResp::File { gen: 9, hl: hl.clone() });
        assert_eq!(a.center.source_scroll, 250);
        let t = draw(&mut a, 100, 20);
        assert_eq!(cell_of(&t, "def d250").0, rgb("#569cd6"));
        // A late answer for a file that is no longer shown is ignored.
        a.hl.wanted = Some("Other.lean".into());
        a.hl.file = None;
        a.on_highlight(HlResp::File { gen: 10, hl });
        assert!(a.hl.file.is_none());
        // Resizing only re-wraps.
        for (w, h) in [(30u16, 8u16), (20, 5), (250, 70)] {
            draw(&mut a, w, h);
        }
    }
}
