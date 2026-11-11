//! The interactive browser: target resolution, terminal lifecycle and the
//! event loop.
//!
//! The loop blocks on a single channel (no polling, no redraw timer), so an
//! idle slim uses no CPU. Input, POSIX signals, query results, search
//! results and background-job events all arrive on that channel. On quit,
//! panic or a termination signal the terminal is restored, background work
//! is cancelled, owned child processes are killed and reaped, and the
//! session is saved.

pub mod hl;
pub mod render;
pub mod render_views;
pub mod state;
pub mod theme;
pub mod views;
pub mod worker;

use crate::cli::Args;
use crate::db::{self, IndexMode};
use crate::paths::{sha256_hex, Project};
use crate::text::PosEncoding;
use anyhow::{bail, Context, Result};
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::{
    cursor, event,
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::Terminal;
use state::{App, Channels, Ctx};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::time::Duration;
use theme::ThemeName;
use worker::*;

/// Opt-in timing statistics (only collected when `SLIM_TIMING_LOG` is set).
pub mod timing {
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::sync::OnceLock;

    static ON: OnceLock<bool> = OnceLock::new();
    static DATA: Mutex<BTreeMap<&'static str, Vec<f64>>> = Mutex::new(BTreeMap::new());

    pub fn enabled() -> bool {
        *ON.get_or_init(|| std::env::var_os("SLIM_TIMING_LOG").is_some())
    }

    pub fn record(key: &'static str, ms: f64) {
        if enabled() {
            if let Ok(mut d) = DATA.lock() {
                d.entry(key).or_default().push(ms);
            }
        }
    }

    /// `{key: {n, p50, p95, p99, max}}` in milliseconds.
    pub fn summary() -> serde_json::Value {
        let d = DATA.lock().map(|d| d.clone()).unwrap_or_default();
        let mut out = serde_json::Map::new();
        for (k, mut v) in d {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let pick = |p: f64| v[((p / 100.0) * (v.len() - 1) as f64).round() as usize];
            out.insert(
                k.to_string(),
                serde_json::json!({"n": v.len(), "p50": pick(50.0), "p95": pick(95.0), "p99": pick(99.0), "max": v[v.len() - 1]}),
            );
        }
        serde_json::Value::Object(out)
    }
}

pub enum Event {
    Input(event::Event),
    Query(QueryResp),
    Search(SearchResp),
    Job(JobEvent),
    Highlight(crate::highlight::service::HlResp),
    SemIndex(hl::SemEvent),
    Signal(i32),
    InputClosed,
}

pub enum Target {
    Demo,
    Project(PathBuf),
    Index { index: PathBuf, source_root: Option<PathBuf> },
}

/// Embedded demo: an original miniature library and a semantic snapshot
/// generated from it by `slim export` (Lean 4.33.1). The demo needs no
/// Lean installation, network or compilation.
pub const DEMO_SNAPSHOT: &str = include_str!("../../fixtures/demo/snapshot/slim_demo.lean-v4.33.1.jsonl");
pub const DEMO_FILES: &[(&str, &str)] = &[
    ("lakefile.toml", include_str!("../../fixtures/demo/lakefile.toml")),
    ("lean-toolchain", include_str!("../../fixtures/demo/lean-toolchain")),
    ("lake-manifest.json", include_str!("../../fixtures/demo/lake-manifest.json")),
    ("lean-view.toml", include_str!("../../fixtures/demo/lean-view.toml")),
    ("SlimDemo.lean", include_str!("../../fixtures/demo/SlimDemo.lean")),
    ("SlimDemo/Relation/Basic.lean", include_str!("../../fixtures/demo/SlimDemo/Relation/Basic.lean")),
    ("SlimDemo/Relation/Closure.lean", include_str!("../../fixtures/demo/SlimDemo/Relation/Closure.lean")),
    ("SlimDemo/Relation/Equivalence.lean", include_str!("../../fixtures/demo/SlimDemo/Relation/Equivalence.lean")),
    ("SlimDemo/Graph/Basic.lean", include_str!("../../fixtures/demo/SlimDemo/Graph/Basic.lean")),
    ("SlimDemo/Graph/Search.lean", include_str!("../../fixtures/demo/SlimDemo/Graph/Search.lean")),
    ("SlimDemo/Fixtures/Trust.lean", include_str!("../../fixtures/demo/SlimDemo/Fixtures/Trust.lean")),
    ("SlimDemo/Fixtures/Special.lean", include_str!("../../fixtures/demo/SlimDemo/Fixtures/Special.lean")),
];

/// Writes the embedded demo sources and imports the embedded snapshot into
/// the cache (once per snapshot content). Returns (index, source root).
pub fn prepare_demo(progress: &dyn Fn(&str)) -> Result<(PathBuf, PathBuf)> {
    let h = sha256_hex(DEMO_SNAPSHOT.as_bytes());
    let dir = crate::paths::cache_dir().join("demo").join(&h[..16]);
    let src = dir.join("src");
    let idx = dir.join("semantic.sqlite");
    for (rel, text) in DEMO_FILES {
        let p = src.join(rel);
        if std::fs::read_to_string(&p).ok().as_deref() != Some(text) {
            std::fs::create_dir_all(p.parent().unwrap())?;
            std::fs::write(&p, text)?;
        }
    }
    let ok = idx.exists() && db::open_read(&idx).ok().and_then(|c| db::get_meta(&c, "snapshot_sha256").ok().flatten()).is_some();
    if !ok {
        progress("preparing the demo index (one-time import of the bundled snapshot)…");
        let snap = dir.join("snapshot.jsonl");
        std::fs::write(&snap, DEMO_SNAPSHOT)?;
        let opts = db::import::ImportOptions { source_root: Some(src.clone()), ..Default::default() };
        db::import::import_snapshot(&snap, &idx, &opts)?;
    }
    Ok((idx, src))
}

struct Opening {
    ctx: Option<IndexCtx>,
    project: Option<Project>,
    title: String,
    mode: IndexMode,
    needs_build: bool,
    /// Something the reader should know about how the index was opened.
    notice: Option<String>,
}

/// A semantic index that this slim cannot read (written with another
/// index format) is derived data: when the snapshot it was imported from is
/// still in the cache, it is imported again. Nothing in the project runs.
fn reimport_semantic(project: &Project, sem: &Path, why: &anyhow::Error) -> std::result::Result<(), String> {
    let snap = project.cache_dir().join("snapshot.jsonl");
    if !snap.is_file() {
        return Err(format!(
            "the semantic index cannot be used ({why:#}) and its snapshot is not in the cache; showing the source-only index. `slim import <SNAPSHOT> --source-root {}` rebuilds it",
            project.root.display()
        ));
    }
    eprintln!("slim: the semantic index was written with another index format; importing {} again (one time)…", snap.display());
    let opts = db::import::ImportOptions { source_root: Some(project.root.clone()), ..Default::default() };
    db::import::import_snapshot(&snap, sem, &opts).map(|_| ()).map_err(|e| format!("the semantic index cannot be used ({why:#}) and importing its snapshot again failed ({e:#}); showing the source-only index"))
}

fn resolve(target: Target) -> Result<Opening> {
    match target {
        Target::Demo => {
            let (idx, src) = prepare_demo(&|m| eprintln!("{m}"))?;
            Ok(Opening {
                ctx: Some(IndexCtx { index: idx, source_root: Some(src), mode: IndexMode::Semantic, user_key: "lake:slim_demo".into(), encoding: PosEncoding::CodePoint, map_path: None }),
                project: None,
                title: "demo: slim_demo (miniature library + labelled test fixtures)".into(),
                mode: IndexMode::Semantic,
                needs_build: false,
                notice: None,
            })
        }
        Target::Project(p) => {
            if p.is_file() {
                bail!("{} is a file: import snapshots with `slim import` first", p.display());
            }
            let project = Project::open(&p)?;
            let sem = project.semantic_index();
            let mut notice = None;
            let sem_ok = sem.exists()
                && match db::open_read(&sem) {
                    Ok(_) => true,
                    Err(e) => match reimport_semantic(&project, &sem, &e) {
                        Ok(()) => {
                            notice = Some("the semantic index was rebuilt from its cached snapshot for this slim version".to_string());
                            db::open_read(&sem).is_ok()
                        }
                        Err(msg) => {
                            notice = Some(msg);
                            false
                        }
                    },
                };
            if sem_ok {
                return Ok(Opening {
                    ctx: Some(IndexCtx {
                        index: sem,
                        source_root: Some(project.root.clone()),
                        mode: IndexMode::Semantic,
                        user_key: project.user_key(),
                        encoding: PosEncoding::CodePoint, map_path: None,
                    }),
                    title: format!("{} ({})", project.name, project.root.display()),
                    project: Some(project),
                    mode: IndexMode::Semantic,
                    needs_build: false,
                    notice,
                });
            }
            let src = project.source_index();
            let usable = src.exists() && db::open_read(&src).is_ok();
            Ok(Opening {
                ctx: Some(IndexCtx {
                    index: src,
                    source_root: Some(project.root.clone()),
                    mode: IndexMode::Source,
                    user_key: project.user_key(),
                    encoding: PosEncoding::Utf8, map_path: None,
                }),
                title: format!("{} ({})", project.name, project.root.display()),
                project: Some(project),
                mode: IndexMode::Source,
                needs_build: !usable,
                notice,
            })
        }
        Target::Index { index, source_root } => {
            let conn = db::open_read(&index)?;
            let mode = db::mode_of(&conn)?;
            let root = source_root.or_else(|| db::get_meta(&conn, "source_root").ok().flatten().map(PathBuf::from)).filter(|p| p.is_dir());
            let name = db::get_meta(&conn, "project_name")?.unwrap_or_else(|| "index".into());
            Ok(Opening {
                ctx: Some(IndexCtx {
                    index: index.clone(),
                    source_root: root,
                    mode,
                    user_key: format!("lake:{name}"),
                    encoding: if mode == IndexMode::Semantic { PosEncoding::CodePoint } else { PosEncoding::Utf8 },
                    map_path: None,
                }),
                project: None,
                title: format!("{name} ({})", index.display()),
                mode,
                needs_build: false,
                notice: None,
            })
        }
    }
}

fn restore_terminal() {
    let _ = terminal::disable_raw_mode();
    let mut out = std::io::stdout();
    let _ = execute!(out, DisableMouseCapture, LeaveAlternateScreen, cursor::Show);
    let _ = out.flush();
}

/// Collects a whole frame and writes it with a single `write` on flush, so
/// terminals never render half a frame. Unlike `BufWriter`, dropping it
/// discards unflushed bytes: after a panic in the middle of a frame nothing
/// is written onto the restored screen. Counts bytes for the timing log.
struct FrameWriter {
    buf: Vec<u8>,
    out: std::io::Stdout,
    bytes: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl std::io::Write for FrameWriter {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if !self.buf.is_empty() {
            let mut lock = self.out.lock();
            lock.write_all(&self.buf)?;
            lock.flush()?;
            self.bytes.fetch_add(self.buf.len() as u64, Ordering::Relaxed);
            self.buf.clear();
        }
        Ok(())
    }
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn spawn_input_thread(tx: Sender<Event>) {
    std::thread::Builder::new()
        .name("slim-input".into())
        .spawn(move || loop {
            match event::read() {
                Ok(e) => {
                    if tx.send(Event::Input(e)).is_err() {
                        return;
                    }
                }
                Err(_) => {
                    let _ = tx.send(Event::InputClosed);
                    return;
                }
            }
        })
        .expect("spawn input thread");
}

fn spawn_signal_thread(tx: Sender<Event>) -> Result<()> {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGQUIT, SIGTERM};
    let mut signals = signal_hook::iterator::Signals::new([SIGTERM, SIGHUP, SIGINT, SIGQUIT])?;
    std::thread::Builder::new().name("slim-signals".into()).spawn(move || {
        if let Some(sig) = signals.forever().next() {
            let _ = tx.send(Event::Signal(sig));
        }
    })?;
    Ok(())
}

struct Workers {
    query: Option<(Sender<QueryReq>, std::thread::JoinHandle<()>)>,
    search: Option<(Sender<SearchReq>, std::thread::JoinHandle<()>)>,
    jobs: Option<(Sender<JobReq>, std::thread::JoinHandle<()>)>,
}

fn attach(app: &mut App, workers: &mut Workers, ctx: &IndexCtx, tx: &Sender<Event>) {
    let (qtx, qh) = spawn_query_worker(ctx.clone(), tx.clone());
    let (stx, sh) = spawn_search_worker(ctx.clone(), tx.clone(), Arc::clone(&app.ch.search_latest));
    app.ch.query = Some(qtx.clone());
    app.ch.search = Some(stx.clone());
    workers.query = Some((qtx, qh));
    workers.search = Some((stx, sh));
    let gen = app.gens.bump();
    app.gens.info = gen;
    let _ = app.ch.query.as_ref().unwrap().send(QueryReq::Info { gen });
    let gen = app.gens.bump();
    let _ = app.ch.query.as_ref().unwrap().send(QueryReq::LoadBookmarks { gen });
    // Trails, questions and the pinned goal (the project map arrives by itself).
    let _ = app.ch.query.as_ref().unwrap().send(QueryReq::LoadReading);
    // Restore the previous session for this project, if any.
    if let Ok(store) = crate::userdata::UserStore::open() {
        if let Ok(Some(s)) = store.load_session(&session_key(ctx)) {
            app.restore_session(&s);
        }
    }
    if ctx.source_root.is_some() {
        app.job_queue.push(JobReq::Freshness { force_hash: false, discover_new: ctx.mode == IndexMode::Source });
        app.run_next_job();
    }
    // The highlight worker: tokenizes and reads the cache; starts no process.
    if let (Some(root), None) = (&ctx.source_root, &app.hl.tx) {
        let store_path = match &app.ctx.project_root {
            Some(p) => Project::open(p).map(|p| crate::highlight::store::Store::path_for_project(&p)).unwrap_or_else(|_| crate::highlight::store::Store::path_beside_index(&ctx.index)),
            None => crate::highlight::store::Store::path_beside_index(&ctx.index),
        };
        let toolchain = std::fs::read_to_string(root.join("lean-toolchain")).ok().map(|s| s.trim().to_string());
        let (htx, hh) = crate::highlight::service::spawn(root.clone(), store_path, toolchain, app.hl.budget_bytes, tx.clone(), Event::Highlight);
        app.hl.tx = Some(htx);
        app.hl.worker = Some(hh);
    }
}

fn session_key(ctx: &IndexCtx) -> String {
    format!("{}#{}", ctx.user_key, ctx.mode.as_str())
}

fn stop_workers(w: &mut Workers) {
    if let Some((t, h)) = w.query.take() {
        let _ = t.send(QueryReq::Shutdown);
        let _ = h.join();
    }
    if let Some((t, h)) = w.search.take() {
        let _ = t.send(SearchReq::Shutdown);
        let _ = h.join();
    }
}

/// Appends timing facts to `$SLIM_TIMING_LOG` (used by tests and the
/// measured-performance report).
fn timing_log(app: &App) {
    if let Some(p) = std::env::var_os("SLIM_TIMING_LOG") {
        let line = serde_json::json!({
            "first_frame_ms": app.first_frame_ms,
            "search_ready": app.search_ready.as_ref().and_then(|r| r.as_ref().ok()).map(|(n, ms, rebuilt)| serde_json::json!({"entries": n, "ms": ms, "rebuilt": rebuilt})),
            "search_ready_after_start_ms": app.search_ready_at_ms,
            "mode": app.ctx.mode.as_str(),
            "stats_ms": timing::summary(),
        });
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = writeln!(f, "{line}");
        }
    }
}

pub fn launch(target: Target, args: &Args) -> Result<i32> {
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        bail!("slim's browser needs an interactive terminal (try `slim search` or `slim info` for scripted use)");
    }
    let mut open = resolve(target)?;
    if let (Some(c), Some(m)) = (open.ctx.as_mut(), args.path("map")) {
        c.map_path = Some(m);
    }
    let known = "lean, lean-dark, lean-light, dark, light, contrast, mono";
    let theme = match args.get("theme").map(String::from).or_else(|| std::env::var("SLIM_THEME").ok()) {
        Some(t) => ThemeName::parse(&t).with_context(|| format!("unknown theme {t} ({known})"))?,
        None if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) => ThemeName::Mono,
        None => ThemeName::parse("lean").unwrap_or(ThemeName::LeanDark),
    };
    let ascii = args.has("ascii") || std::env::var("LANG").map(|l| !l.to_uppercase().contains("UTF")).unwrap_or(false) && std::env::var_os("LC_ALL").is_none();
    let threads = args.num::<usize>("threads")?;
    let (tx, rx) = channel::<Event>();
    let ch = Channels {
        query: None,
        search: None,
        jobs: None,
        events: Some(tx.clone()),
        search_latest: Arc::new(AtomicU64::new(0)),
        job_cancel: Arc::new(AtomicBool::new(false)),
    };
    let ctx = Ctx {
        title: open.title.clone(),
        mode: open.mode,
        index: open.ctx.clone(),
        user_key: open.ctx.as_ref().map(|c| c.user_key.clone()).unwrap_or_default(),
        editor: args.get("editor").map(String::from).or_else(|| std::env::var("SLIM_EDITOR").ok()),
        info: None,
        synthetic: false,
        project_root: open.project.as_ref().map(|p| p.root.clone()),
    };
    let mut app = App::new(ctx, theme, ascii, ch);
    if let Some(m) = args.get("highlight").map(String::from).or_else(|| std::env::var("SLIM_HIGHLIGHT").ok()) {
        app.hl.mode = crate::highlight::Mode::parse(&m).with_context(|| format!("--highlight: expected off, syntax or semantic, got {m}"))?;
        app.hl_mode_explicit = true;
    }
    if let Some(t) = args.get("syntax-theme").map(String::from).or_else(|| std::env::var("SLIM_SYNTAX_THEME").ok()) {
        crate::highlight::cli::load_theme(&t)?;
        if crate::highlight::theme::SyntaxTheme::builtin(&t).is_none() {
            app.hl.user_theme = Some(t.clone());
        }
        app.hl.theme_choice = Some(t);
        app.hl_apply_theme();
    }
    if let Some(m) = args.num::<usize>("highlight-memory-mib")?.or_else(|| std::env::var("SLIM_HL_MEMORY_BUDGET_MIB").ok().and_then(|v| v.parse().ok())) {
        app.hl.budget_bytes = m.max(8) << 20;
    }
    let mut workers = Workers { query: None, search: None, jobs: None };
    if let Some(ictx) = &open.ctx {
        let jc = JobCtx { project: open.project.clone(), ctx: ictx.clone(), threads };
        let (jtx, jh) = spawn_job_runner(jc, tx.clone(), Arc::clone(&app.ch.job_cancel));
        app.ch.jobs = Some(jtx.clone());
        workers.jobs = Some((jtx, jh));
    }

    // Terminal setup with restoration on every exit path.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        crate::process::kill_all_registered();
        default_hook(info);
    }));
    terminal::enable_raw_mode().context("enabling raw mode")?;
    let _guard = TerminalGuard;
    let mut out = std::io::stdout();
    // Clear with a plain escape sequence: ratatui's `Terminal::clear` queries
    // the cursor position, which some terminals never answer.
    execute!(out, EnterAlternateScreen, cursor::Hide, terminal::Clear(terminal::ClearType::All))?;
    // Mouse on by default (most terminals still select text with Shift+drag);
    // --no-mouse leaves the mouse entirely to the terminal.
    let mouse = !args.has("no-mouse");
    if mouse {
        execute!(out, EnableMouseCapture)?;
    }
    let bytes = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let mut terminal = Terminal::new(CrosstermBackend::new(FrameWriter {
        buf: Vec::with_capacity(1 << 16),
        out: std::io::stdout(),
        bytes: bytes.clone(),
    }))?;
    spawn_input_thread(tx.clone());
    spawn_signal_thread(tx.clone())?;

    if let Some(n) = open.notice.take() {
        app.warn(n);
    }
    if open.needs_build {
        app.building = true;
        app.job_queue.push(JobReq::BuildSource);
        app.run_next_job();
    } else if let Some(ictx) = &open.ctx {
        attach(&mut app, &mut workers, ictx, &tx);
    }

    // Event loop: block until something happens, then drain and redraw once.
    let result: Result<()> = (|| {
        // When the oldest not yet drawn keyboard or mouse event arrived.
        let mut input_at: Option<std::time::Instant> = None;
        loop {
            if app.dirty {
                let t = std::time::Instant::now();
                let b0 = bytes.load(Ordering::Relaxed);
                // Synchronized output (DEC mode 2026): supporting terminals show
                // the frame atomically; others ignore the sequences.
                ratatui::crossterm::queue!(terminal.backend_mut(), terminal::BeginSynchronizedUpdate)?;
                terminal.draw(|f| render::draw(f, &mut app))?;
                ratatui::crossterm::execute!(terminal.backend_mut(), terminal::EndSynchronizedUpdate)?;
                timing::record("frame (render + write)", t.elapsed().as_secs_f64() * 1000.0);
                timing::record("frame bytes (KiB)", (bytes.load(Ordering::Relaxed) - b0) as f64 / 1024.0);
                if let Some(t0) = input_at.take() {
                    timing::record("input event to frame written", t0.elapsed().as_secs_f64() * 1000.0);
                }
                app.dirty = false;
            }
            input_at = None;
            // Block until something happens; a timeout is armed only while a
            // debounced preview is pending (idle CPU stays at zero).
            let ev = match app.preview_due {
                Some(due) => {
                    let now = std::time::Instant::now();
                    if now >= due {
                        app.fire_preview();
                        continue;
                    }
                    match rx.recv_timeout(due - now) {
                        Ok(e) => e,
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            app.fire_preview();
                            continue;
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
                None => match rx.recv() {
                    Ok(e) => e,
                    Err(_) => break,
                },
            };
            let t = std::time::Instant::now();
            if matches!(ev, Event::Input(_)) {
                input_at = Some(t);
            }
            app.handle(ev);
            timing::record("ui event handling", t.elapsed().as_secs_f64() * 1000.0);
            while let Ok(ev) = rx.try_recv() {
                let t = std::time::Instant::now();
                if matches!(ev, Event::Input(_)) {
                    input_at.get_or_insert(t);
                }
                app.handle(ev);
                timing::record("ui event handling", t.elapsed().as_secs_f64() * 1000.0);
                if app.quit {
                    break;
                }
            }
            if app.needs_reattach {
                app.needs_reattach = false;
                if let Some(ictx) = open.ctx.clone() {
                    attach(&mut app, &mut workers, &ictx, &tx);
                }
            }
            if let Some(idx) = app.switch_to.take() {
                // Replace the source-only index by the new semantic one.
                if let Some(old) = open.ctx.clone() {
                    stop_workers(&mut workers);
                    let ictx = IndexCtx { index: idx, mode: IndexMode::Semantic, encoding: PosEncoding::CodePoint, ..old };
                    let keep = app.detail.as_ref().map(|d| d.d.row.name.clone());
                    app.reset_for_index(ictx.clone());
                    attach(&mut app, &mut workers, &ictx, &tx);
                    if let Some(n) = keep {
                        app.current_how = "semantic export".into();
                        app.goto_name(n);
                    }
                }
            }
            app.run_next_job();
            if app.quit {
                break;
            }
        }
        Ok(())
    })();

    // Shutdown: cancel background work first, then restore the terminal.
    app.ch.job_cancel.store(true, Ordering::Relaxed);
    app.ch.search_latest.fetch_add(1, Ordering::Relaxed);
    app.cancel_connection();
    if let Some(ictx) = &open.ctx {
        if app.detail.is_some() || app.v.view != views::View::Browse {
            if let Ok(store) = crate::userdata::UserStore::open() {
                let _ = store.save_session(&session_key(ictx), &app.session_json());
            }
        }
    }
    if let Some((t, _)) = &workers.query {
        let _ = t.send(QueryReq::Shutdown);
    }
    if let Some((t, _)) = &workers.search {
        let _ = t.send(SearchReq::Shutdown);
    }
    if let Some((t, _)) = &workers.jobs {
        let _ = t.send(JobReq::Shutdown);
    }
    let hl_threads = app.hl_shutdown();
    crate::process::kill_all_registered();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    for h in [workers.jobs.take().map(|x| x.1), workers.query.take().map(|x| x.1), workers.search.take().map(|x| x.1)].into_iter().flatten().chain(hl_threads) {
        while !h.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if h.is_finished() {
            let _ = h.join();
        }
    }
    drop(terminal);
    restore_terminal();
    timing_log(&app);
    let leftover = crate::process::registered_groups();
    if !leftover.is_empty() {
        eprintln!("slim: warning: child process groups still registered at exit: {leftover:?}");
    }
    result?;
    Ok(0)
}

#[allow(dead_code)]
fn exists(p: &Path) -> bool {
    p.exists()
}
