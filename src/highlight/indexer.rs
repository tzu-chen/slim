//! The explicit semantic-highlight indexing job.
//!
//! It starts the project's own pinned Lean language server (`lake serve`
//! from the toolchain named in `lean-toolchain`, which must already be
//! installed), opens documents one after another with their exact captured
//! contents, waits for a verified completion condition, takes the server's
//! semantic tokens and publishes them to the cache per file. It never
//! builds, fetches or installs anything: documents are opened with
//! `dependencyBuildMode: "never"`, and by default the server runs in a
//! sandbox without network where the project is read-only.
//!
//! Completion (checked for Lean 4.16–4.35, `docs/highlighting.md`): the
//! answer to `textDocument/waitForDiagnostics` for the opened version
//! *and* a `$/lean/fileProgress` notification with an empty `processing`
//! list for that version. Only then is `textDocument/semanticTokens/full`
//! requested, and requested again if the server asked for a refresh in
//! between. Anything less is recorded as partial or failed, never complete.
//! Finishing with elaboration errors is still complete coverage; coverage
//! says nothing about whether the file's proofs check.

use super::env::{Artifacts, Closure, DepTracker, EnvInfo, EnvRecord};
use super::lsp::{Client, Incoming, ServerInfo, ShutdownReport};
use super::semantic::{decode_lsp, encoding_name, line_endings};
use super::store::{SemMeta, SemRecord, Store};
use super::{Coverage, EXTRACTOR_VERSION};
use crate::export::{detect_sandbox, toolchain_dir, Sandbox};
use crate::paths::{sha256_hex, Project};
use crate::process::{self, Limits};
use crate::source::lexer::parse_lean_version;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// A file is stopped when the server's anonymous memory reaches this share
/// of its cgroup limit (sampled every second).
pub const MEMORY_STOP_FRACTION: f64 = 0.7;

/// Toolchains on which the whole procedure was run against the fixture and
/// its results checked (`scripts/test-highlight-toolchains.sh`).
pub const TESTED_TOOLCHAINS: &[&str] = &[
    "leanprover/lean4:v4.16.0",
    "leanprover/lean4:v4.21.0",
    "leanprover/lean4:v4.28.0",
    "leanprover/lean4:v4.33.1",
    "leanprover/lean4:v4.35.0",
];

#[derive(Clone, Debug)]
pub struct Options {
    /// Restrict to these relative paths or directory prefixes (empty: all).
    pub files: Vec<String>,
    pub max_files: Option<usize>,
    /// Wall-clock limit per file; a file over it is recorded as partial.
    pub file_timeout: Duration,
    /// Wall-clock limit for the whole run; the rest stays for a later run.
    pub budget: Option<Duration>,
    pub memory_mib: u64,
    /// `auto`, `bwrap` or `none`.
    pub sandbox: String,
    pub trust_project: bool,
    /// Apply the memory limit with a systemd user scope. Turn off when an
    /// outer resource guard already confines this process.
    pub cgroup: bool,
    /// Re-index files even if their cached record is current.
    pub refresh: bool,
    /// Try again files whose current record is partial, failed or skipped.
    pub retry: bool,
    /// Documents open at once (1–4).
    pub open_docs: usize,
    pub max_file_bytes: u64,
    pub allow_untested: bool,
    /// `LEAN_NUM_THREADS` for the server, if set.
    pub lean_threads: Option<usize>,
    /// How long to wait for the empty progress report after the wait
    /// request was answered before recording the file as partial.
    pub completion_grace: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            files: Vec::new(),
            max_files: None,
            file_timeout: Duration::from_secs(300),
            budget: None,
            memory_mib: 6144,
            sandbox: "auto".into(),
            trust_project: false,
            cgroup: true,
            refresh: false,
            retry: false,
            open_docs: 1,
            max_file_bytes: 4 << 20,
            allow_untested: false,
            lean_threads: None,
            completion_grace: Duration::from_secs(5),
        }
    }
}

pub struct Plan {
    pub project: Project,
    pub toolchain: String,
    pub toolchain_dir: PathBuf,
    pub tested: bool,
    /// `lakefile.toml`, `lakefile.lean` or none.
    pub config: Option<String>,
    pub sandbox: Sandbox,
    pub limits: Limits,
    pub files: Vec<String>,
    pub built: usize,
    pub store_path: PathBuf,
    pub opts: Options,
}

fn matches_filter(rel: &str, filters: &[String]) -> bool {
    filters.is_empty()
        || filters.iter().any(|f| {
            let f = f.trim_start_matches("./");
            rel == f || rel.strip_prefix(f.trim_end_matches('/')).is_some_and(|r| r.starts_with('/'))
        })
}

/// Inspects the project as data and decides whether indexing can run.
/// Nothing is executed here.
pub fn plan(root: &Path, o: &Options) -> Result<Plan> {
    let project = Project::open(root)?;
    let toolchain = project.toolchain.clone().context(
        "this directory has no lean-toolchain file, so its Lean version is unknown; \
         semantic highlighting needs the project's own pinned toolchain",
    )?;
    let tested = TESTED_TOOLCHAINS.contains(&toolchain.as_str());
    match parse_lean_version(&toolchain) {
        Some((4, m, _)) if m >= 16 => {}
        _ => bail!("toolchain {toolchain} is not supported for semantic highlighting (Lean 4.16 or newer)"),
    }
    if !tested && !o.allow_untested {
        bail!(
            "toolchain {toolchain} is not in the tested list ({}). Pass --allow-untested-toolchain to try anyway; \
             the completion protocol is then unverified for this version",
            TESTED_TOOLCHAINS.join(", ")
        );
    }
    let tc_dir = toolchain_dir(&toolchain);
    if !tc_dir.join("bin/lake").exists() || !tc_dir.join("bin/lean").exists() {
        bail!(
            "toolchain {toolchain} is not installed at {}. slim never installs toolchains; install it yourself \
             (e.g. `elan toolchain install {toolchain}`). Syntax colouring works without it",
            tc_dir.display()
        );
    }
    let config = ["lakefile.toml", "lakefile.lean"].iter().find(|n| project.root.join(n).is_file()).map(|s| s.to_string());
    // Dependencies must already be there: nothing is fetched.
    if let Ok(text) = std::fs::read_to_string(project.root.join("lake-manifest.json")) {
        let j: Value = serde_json::from_str(&text).context("lake-manifest.json is not valid JSON")?;
        let pkgs_dir = j.get("packagesDir").and_then(|v| v.as_str()).unwrap_or(".lake/packages");
        for p in j.get("packages").and_then(|p| p.as_array()).into_iter().flatten() {
            if p.get("type").and_then(|v| v.as_str()) == Some("path") {
                continue;
            }
            let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if !name.is_empty() && !project.root.join(pkgs_dir).join(name).is_dir() {
                bail!(
                    "dependency `{name}` is not present under {pkgs_dir}. slim does not fetch dependencies; \
                     run `lake update`/`lake build` yourself if you trust this project"
                );
            }
        }
    }
    if config.is_some() && super::env::build_lib_dir(&project.root).is_none() {
        bail!(
            "no build artifacts found (.lake/build/lib/lean). Imports could not be loaded and slim does not build \
             projects: run `lake build` yourself if you trust this project, then retry"
        );
    }
    let sandbox = match o.sandbox.as_str() {
        "none" => {
            if !o.trust_project {
                bail!("--sandbox none requires --trust-project (a deliberate decision to run this project's code unisolated)");
            }
            Sandbox::None
        }
        "bwrap" | "auto" => match detect_sandbox() {
            s @ Sandbox::Bwrap { .. } => s,
            Sandbox::None => bail!(
                "no usable sandbox (bubblewrap with user namespaces) was found; semantic indexing refused. \
                 If you trust this project, pass --sandbox none --trust-project explicitly"
            ),
        },
        other => bail!("--sandbox: expected auto|bwrap|none, got {other}"),
    };
    let cancel = AtomicBool::new(false);
    let (found, _) = crate::source::index::discover(&project.root, &cancel)?;
    let tracker = DepTracker::new(&project.root, Some(&toolchain));
    let mut built = Vec::new();
    let mut unbuilt = Vec::new();
    for f in found {
        if !matches_filter(&f.rel, &o.files) {
            continue;
        }
        if tracker.olean_path(&f.rel).is_file() {
            built.push(f.rel);
        } else {
            unbuilt.push(f.rel);
        }
    }
    built.sort();
    unbuilt.sort();
    let n_built = built.len();
    // Built modules first: their imports are known to be available.
    let mut files = built;
    files.extend(unbuilt);
    if let Some(m) = o.max_files {
        files.truncate(m);
    }
    if files.is_empty() {
        bail!("no Lean files match");
    }
    let limits = Limits {
        address_space_mib: Some(131072),
        memory_mib: o.cgroup.then_some(o.memory_mib),
        tasks_max: Some(2048),
        cpu_seconds: None,
        file_size_mib: Some(64),
        wall: Duration::from_secs(u32::MAX as u64),
        max_stderr_lines: 60,
        merge_stdout: false,
    };
    let store_path = Store::path_for_project(&project);
    Ok(Plan { project, toolchain, toolchain_dir: tc_dir, tested, config, sandbox, limits, built: n_built.min(files.len()), files, store_path, opts: o.clone() })
}

impl Plan {
    /// The exact server command.
    pub fn server_command(&self) -> Command {
        let lake = self.toolchain_dir.join("bin/lake");
        let root = &self.project.root;
        match &self.sandbox {
            Sandbox::Bwrap { path, .. } => {
                let mut c = process::with_cgroup(&self.limits, path, &[]);
                c.args(["--unshare-all", "--die-with-parent", "--new-session", "--cap-drop", "ALL"]);
                c.args(["--ro-bind", "/usr", "/usr"]);
                for d in ["/lib", "/lib64", "/bin", "/sbin"] {
                    match std::fs::read_link(d) {
                        Ok(target) => {
                            c.arg("--symlink").arg(target).arg(d);
                        }
                        Err(_) => {
                            c.arg("--ro-bind-try").arg(d).arg(d);
                        }
                    }
                }
                c.args(["--ro-bind-try", "/etc/ld.so.cache", "/etc/ld.so.cache"]);
                // The server's watchdog reads the time zone for its log.
                c.args(["--ro-bind-try", "/etc/localtime", "/etc/localtime"]);
                c.args(["--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp"]);
                for ro in [&self.toolchain_dir, root] {
                    c.arg("--ro-bind").arg(ro).arg(ro);
                }
                c.args(["--clearenv", "--setenv", "PATH", "/usr/bin", "--setenv", "HOME", "/tmp", "--setenv", "LANG", "C.UTF-8"]);
                if let Some(t) = self.opts.lean_threads {
                    c.args(["--setenv", "LEAN_NUM_THREADS", &t.to_string()]);
                }
                c.arg("--chdir").arg(root).arg("--");
                c.arg(&lake).args(["serve", "--"]);
                c
            }
            Sandbox::None => {
                let mut c = process::with_cgroup(&self.limits, &lake, &[]);
                c.args(["serve", "--"]);
                c.env_clear();
                c.env("PATH", "/usr/bin:/bin");
                if let Some(h) = std::env::var_os("HOME") {
                    c.env("HOME", h);
                }
                c.env("LANG", "C.UTF-8");
                if let Some(t) = self.opts.lean_threads {
                    c.env("LEAN_NUM_THREADS", t.to_string());
                }
                c.current_dir(root);
                c
            }
        }
    }

    pub fn describe(&self) -> String {
        let o = &self.opts;
        let mut s = String::new();
        s.push_str(&format!("Semantic highlighting index for `{}` at {}\n", self.project.name, self.project.root.display()));
        s.push_str(&format!(
            "  toolchain:   {} ({}){}\n",
            self.toolchain,
            self.toolchain_dir.display(),
            if self.tested { "" } else { "  [NOT IN TESTED LIST]" }
        ));
        s.push_str(&format!(
            "  runs:        `lake serve` of that toolchain (the Lean language server), which elaborates each file:\n               \
             this EXECUTES the project's macros, tactics and elaborators{}\n",
            match self.config.as_deref() {
                Some("lakefile.lean") => ", and Lake evaluates lakefile.lean (project code) at start",
                Some(_) => "; lakefile.toml is read by Lake as data",
                None => "; no Lake configuration was found, files are elaborated on their own",
            }
        ));
        s.push_str(&format!(
            "  input:       {} Lean files ({} with existing build artifacts first), one document at a time{}\n",
            self.files.len(),
            self.built,
            if o.open_docs > 1 { format!(" (up to {} open)", o.open_docs) } else { String::new() }
        ));
        s.push_str("  never:       no build, no `lake update`, no downloads, no toolchain install; imports must already be built\n");
        s.push_str("               (documents are opened with dependencyBuildMode \"never\"; a file with stale imports is recorded as failed)\n");
        s.push_str(&format!(
            "  sandbox:     {}\n",
            match &self.sandbox {
                Sandbox::Bwrap { version, .. } => format!(
                    "bubblewrap {version}: no network, read-only toolchain and project, private /tmp, cleared environment, nothing writable"
                ),
                Sandbox::None => "NONE: the server runs as your user with full access to your files and network".into(),
            }
        ));
        s.push_str(&format!(
            "  limits:      {}; {} s per file{}\n",
            match self.limits.memory_mib {
                Some(m) if process::cgroup_scope_available() => format!("memory ≤ {m} MiB, no swap (cgroup via systemd user scope)"),
                Some(m) => format!("memory budget {m} MiB NOT enforceable here (no systemd user scope)"),
                None => "memory limit left to the enclosing resource guard (--cgroup none)".into(),
            },
            o.file_timeout.as_secs(),
            o.budget.map(|b| format!("; stops after {} s overall", b.as_secs())).unwrap_or_default()
        ));
        s.push_str(&format!("  writes only: the highlight cache {}\n", self.store_path.display()));
        s.push_str("  resumable:   each file is published when done; cancelling keeps finished files\n");
        s
    }
}

#[derive(Clone, Debug, Default)]
pub struct Counts {
    /// Opened in the server during this run.
    pub attempted: usize,
    pub complete: usize,
    pub partial: usize,
    pub failed: usize,
    pub skipped: usize,
    /// Already had a current record; not opened.
    pub fresh: usize,
    pub total: usize,
}

impl Counts {
    pub fn done(&self) -> usize {
        self.complete + self.partial + self.failed + self.skipped + self.fresh
    }
}

#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub counts: Counts,
    pub current: Vec<String>,
    pub elapsed: Duration,
    /// Seconds the oldest open document has been processing.
    pub current_secs: u64,
    /// Resident memory of the owned server process tree, when measurable.
    pub tree_rss_kib: Option<u64>,
    pub tree_procs: usize,
    pub paused: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Finished,
    Cancelled,
    BudgetExhausted,
    ServerFailed(String),
}

impl Outcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Outcome::Finished => "finished",
            Outcome::Cancelled => "cancelled",
            Outcome::BudgetExhausted => "budget exhausted",
            Outcome::ServerFailed(_) => "server failed",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Summary {
    pub counts: Counts,
    pub outcome: Outcome,
    pub elapsed: Duration,
    pub peak_tree_rss_kib: u64,
    pub restarts: usize,
    /// From noticing the stop request to the server tree being gone.
    pub shutdown: ShutdownReport,
    pub lean_version: Option<String>,
}

impl Summary {
    pub fn remaining(&self) -> usize {
        self.counts.total.saturating_sub(self.counts.done())
    }
    pub fn to_json(&self) -> Value {
        let c = &self.counts;
        json!({
            "outcome": self.outcome.as_str(),
            "detail": match &self.outcome { Outcome::ServerFailed(e) => e.clone(), _ => String::new() },
            "total": c.total, "attempted": c.attempted, "complete": c.complete, "partial": c.partial, "failed": c.failed,
            "skipped": c.skipped, "fresh": c.fresh, "remaining": self.remaining(),
            "elapsed_s": self.elapsed.as_secs_f64(), "peak_tree_rss_mib": self.peak_tree_rss_kib / 1024,
            "restarts": self.restarts, "shutdown_ms": self.shutdown.elapsed.as_millis() as u64,
            "shutdown_clean": self.shutdown.clean, "lingering_children": self.shutdown.lingering,
            "lean_version": self.lean_version,
        })
    }
}

pub enum Event<'a> {
    Progress(&'a Progress),
    Published { rel: &'a str, status: Coverage, detail: &'a str, elapsed: Duration },
}

/// Stop and pause requests from another thread.
#[derive(Default)]
pub struct Control {
    pub cancel: AtomicBool,
    pub pause: AtomicBool,
}

/// The server connection the job drives (the real client, or a scripted
/// one in tests).
pub trait Conn {
    fn request(&mut self, method: &str, params: Value) -> Result<u64>;
    fn notify(&mut self, method: &str, params: Value) -> Result<()>;
    fn answer(&mut self, id: &Value, method: &str, params: &Value) -> Result<()>;
    fn recv(&mut self, timeout: Duration) -> Option<Incoming>;
    fn rss_kib(&self) -> Option<(u64, usize)>;
    /// Memory the kernel charges to the server's cgroup, and its limit.
    fn cgroup_memory(&self) -> Option<process::CgroupMemory>;
    fn stderr_tail(&self) -> Vec<String>;
    fn close(self: Box<Self>, polite: bool) -> ShutdownReport;
}

impl Conn for Client {
    fn request(&mut self, method: &str, params: Value) -> Result<u64> {
        Client::request(self, method, params)
    }
    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        Client::notify(self, method, params)
    }
    fn answer(&mut self, id: &Value, method: &str, params: &Value) -> Result<()> {
        self.answer_default(id, method, params)
    }
    fn recv(&mut self, timeout: Duration) -> Option<Incoming> {
        Client::recv(self, timeout)
    }
    fn rss_kib(&self) -> Option<(u64, usize)> {
        Some(process::tree_rss_kib(self.pid()))
    }
    fn cgroup_memory(&self) -> Option<process::CgroupMemory> {
        // The innermost process of the tree is in the cgroup that counts.
        let pid = process::descendants(self.pid()).into_iter().max().unwrap_or(self.pid());
        process::cgroup_memory(pid)
    }
    fn stderr_tail(&self) -> Vec<String> {
        Client::stderr_tail(self)
    }
    fn close(self: Box<Self>, polite: bool) -> ShutdownReport {
        self.shutdown(polite)
    }
}

/// What a job needs from its surroundings.
pub struct Job<'a> {
    pub root: &'a Path,
    pub toolchain: Option<&'a str>,
    pub files: &'a [String],
    pub opts: &'a Options,
    pub store: &'a Store,
    pub env: EnvRecord,
    /// Starts (or restarts) the server.
    pub connect: &'a mut dyn FnMut() -> Result<(Box<dyn Conn>, ServerInfo)>,
}

struct Doc {
    rel: String,
    uri: String,
    text: String,
    sha: String,
    version: i64,
    opened: Instant,
    wait_id: u64,
    wait_done: Option<Instant>,
    progress_done: bool,
    saw_progress: bool,
    tokens_id: Option<u64>,
    tokens_requested: Option<Instant>,
    refresh_after_request: bool,
    attempts: u8,
    diag_errors: usize,
    diag_warnings: usize,
    header_failure: Option<String>,
    processing_from: Option<u64>,
    /// When the server first reported a fatal error for the file.
    fatal_progress: Option<Instant>,
    first_error: Option<String>,
    stale_imports: Vec<String>,
    fatal: Option<String>,
    timed_out: bool,
    unverified: bool,
    /// Already closed in the server (stopped for memory).
    closed: bool,
    closure: Closure,
    artifacts: Artifacts,
}

fn build_note(a: &Artifacts, c: &Closure) -> String {
    let mut v = Vec::new();
    if c.files.is_empty() {
        return "no local imports".into();
    }
    if a.digest.is_empty() {
        return "no build directory: compiled imports unknown".into();
    }
    if !a.older_than_source.is_empty() {
        v.push(format!(
            "{} imported module(s) had sources newer than their .olean when indexed (e.g. {}): tokens reflect the compiled version",
            a.older_than_source.len(),
            a.older_than_source[0]
        ));
    }
    if !a.missing.is_empty() {
        v.push(format!("{} imported module(s) had no .olean (e.g. {})", a.missing.len(), a.missing[0]));
    }
    if v.is_empty() {
        format!("{} local imports, each .olean at least as new as its source when indexed", c.files.len())
    } else {
        v.join("; ")
    }
}

/// Runs the job against `job.connect`'s server until done, cancelled, out
/// of budget or the server keeps failing.
pub fn run_job(job: Job, ctl: &Control, on: &mut dyn FnMut(Event)) -> Result<Summary> {
    let Job { root, toolchain, files, opts, store, env, connect } = job;
    let start = Instant::now();
    let env_fp = store.put_env(&env)?;
    let run_id = store.begin_run(&env_fp)?;
    let mut tracker = DepTracker::new(root, toolchain);
    let mut counts = Counts { total: files.len(), ..Default::default() };
    let mut queue: VecDeque<&String> = files.iter().collect();
    let mut active: Vec<Doc> = Vec::new();
    let mut restarts = 0usize;
    let mut peak_rss = 0u64;
    let mut last_progress = Instant::now() - Duration::from_secs(1);
    let mut last_rss = Instant::now() - Duration::from_secs(10);
    let mut rss: Option<(u64, usize)> = None;
    let open_docs = opts.open_docs.clamp(1, 4);
    let mut version = 0i64;

    let (mut conn, mut info) = match connect() {
        Ok(x) => x,
        Err(e) => {
            let s = Summary {
                counts,
                outcome: Outcome::ServerFailed(format!("{e:#}")),
                elapsed: start.elapsed(),
                peak_tree_rss_kib: 0,
                restarts: 0,
                shutdown: ShutdownReport::default(),
                lean_version: env.lean_version.clone(),
            };
            store.finish_run(run_id, s.outcome.as_str(), &s.to_json())?;
            return Ok(s);
        }
    };

    let base_meta = |d: &Doc, status: Coverage, detail: String, n_lines: usize, info: &ServerInfo| SemMeta {
        path: d.rel.clone(),
        source_sha256: d.sha.clone(),
        source_size: d.text.len() as u64,
        line_endings: line_endings(&d.text).into(),
        status,
        detail,
        n_lines,
        n_tokens: 0,
        processed_lines: None,
        legend: info.legend.clone(),
        position_encoding: encoding_name(info.encoding).into(),
        diag_errors: d.diag_errors,
        diag_warnings: d.diag_warnings,
        env_fingerprint: env_fp.clone(),
        deps_digest: d.closure.digest.clone(),
        deps_count: d.closure.files.len(),
        artifacts_digest: d.artifacts.digest.clone(),
        build_note: build_note(&d.artifacts, &d.closure),
        extractor: EXTRACTOR_VERSION.into(),
        elapsed_ms: d.opened.elapsed().as_millis() as u64,
        created: crate::export::utc_now(),
    };

    let outcome = 'run: loop {
        if ctl.cancel.load(Ordering::Relaxed) {
            break Outcome::Cancelled;
        }
        let over_budget = opts.budget.is_some_and(|b| start.elapsed() >= b);
        if over_budget && active.is_empty() {
            break if queue.is_empty() { Outcome::Finished } else { Outcome::BudgetExhausted };
        }
        let paused = ctl.pause.load(Ordering::Relaxed);
        // Open documents up to the limit.
        while active.len() < open_docs && !paused && !over_budget {
            let Some(rel) = queue.pop_front() else { break };
            let path = root.join(rel);
            let skip = |why: String, text: &str, sha: String| SemRecord {
                meta: SemMeta {
                    path: rel.clone(),
                    source_sha256: sha,
                    source_size: text.len() as u64,
                    line_endings: line_endings(text).into(),
                    status: Coverage::Skipped,
                    detail: why,
                    n_lines: 0,
                    n_tokens: 0,
                    processed_lines: None,
                    legend: info.legend.clone(),
                    position_encoding: encoding_name(info.encoding).into(),
                    diag_errors: 0,
                    diag_warnings: 0,
                    env_fingerprint: env_fp.clone(),
                    deps_digest: String::new(),
                    deps_count: 0,
                    artifacts_digest: String::new(),
                    build_note: String::new(),
                    extractor: EXTRACTOR_VERSION.into(),
                    elapsed_ms: 0,
                    created: crate::export::utc_now(),
                },
                blob: None,
            };
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    let r = skip(format!("cannot read: {e}"), "", String::new());
                    store.put_semantic(&r)?;
                    counts.skipped += 1;
                    on(Event::Published { rel, status: Coverage::Skipped, detail: &r.meta.detail, elapsed: Duration::ZERO });
                    continue;
                }
            };
            let sha = sha256_hex(&bytes);
            let prior = store.get_semantic_meta(rel)?;
            let text = match String::from_utf8(bytes) {
                Ok(t) if t.len() as u64 <= opts.max_file_bytes => t,
                other => {
                    let why = match &other {
                        Ok(t) => format!("{} bytes is over the per-file size limit ({} MiB)", t.len(), opts.max_file_bytes >> 20),
                        Err(_) => "not valid UTF-8".to_string(),
                    };
                    if !prior.as_ref().is_some_and(|m| m.source_sha256 == sha && m.status == Coverage::Skipped && m.detail == why) {
                        store.put_semantic(&skip(why.clone(), "", sha))?;
                    }
                    counts.skipped += 1;
                    on(Event::Published { rel, status: Coverage::Skipped, detail: &why, elapsed: Duration::ZERO });
                    continue;
                }
            };
            let closure = tracker.closure(rel, Some(&text));
            if let Some(m) = prior.filter(|_| !opts.refresh) {
                let current = m.source_sha256 == sha && m.env_fingerprint == env_fp && m.deps_digest == closure.digest && m.extractor == EXTRACTOR_VERSION;
                if current && (m.status == Coverage::Complete || !opts.retry) {
                    counts.fresh += 1;
                    continue;
                }
            }
            let artifacts = tracker.artifacts(&closure.files);
            version += 1;
            let uri = super::lsp::path_to_uri(&path);
            let opened = conn
                .notify(
                    "textDocument/didOpen",
                    json!({ "textDocument": { "uri": uri, "languageId": "lean4", "version": version, "text": text }, "dependencyBuildMode": "never" }),
                )
                .and_then(|_| conn.request("textDocument/waitForDiagnostics", json!({ "uri": uri, "version": version })));
            let wait_id = match opened {
                Ok(id) => id,
                Err(_) => {
                    // The server is gone; the Closed message handles it.
                    queue.push_front(rel);
                    break;
                }
            };
            counts.attempted += 1;
            active.push(Doc {
                rel: rel.clone(),
                uri,
                text,
                sha,
                version,
                opened: Instant::now(),
                wait_id,
                wait_done: None,
                progress_done: false,
                saw_progress: false,
                tokens_id: None,
                tokens_requested: None,
                refresh_after_request: false,
                attempts: 0,
                diag_errors: 0,
                diag_warnings: 0,
                header_failure: None,
                processing_from: None,
                fatal_progress: None,
                first_error: None,
                stale_imports: Vec::new(),
                fatal: None,
                timed_out: false,
                unverified: false,
                closed: false,
                closure,
                artifacts,
            });
        }
        if active.is_empty() && queue.is_empty() {
            break Outcome::Finished;
        }

        let mut finished: Vec<(usize, Option<Value>)> = Vec::new();
        match conn.recv(Duration::from_millis(if active.is_empty() { 100 } else { 50 })) {
            None => {}
            Some(Incoming::Notification { method, params }) => match method.as_str() {
                "$/lean/fileProgress" => {
                    let uri = params.pointer("/textDocument/uri").and_then(|v| v.as_str()).unwrap_or("");
                    let ver = params.pointer("/textDocument/version").and_then(|v| v.as_i64());
                    if let Some(d) = active.iter_mut().find(|d| d.uri == uri && ver.is_none_or(|v| v == d.version)) {
                        let proc_ = params.get("processing").and_then(|p| p.as_array()).cloned().unwrap_or_default();
                        d.saw_progress = true;
                        d.progress_done = proc_.is_empty();
                        if !proc_.is_empty() {
                            d.processing_from = proc_.iter().filter_map(|r| r.pointer("/range/start/line").and_then(|l| l.as_u64())).min();
                            // A later non-empty report means earlier tokens may be stale.
                            if d.tokens_id.is_some() {
                                d.refresh_after_request = true;
                            }
                        }
                        if proc_.iter().any(|r| r.get("kind").and_then(|k| k.as_u64()) == Some(2)) {
                            // The reason follows as a diagnostic: wait for it.
                            d.fatal_progress.get_or_insert_with(Instant::now);
                        }
                    }
                }
                "textDocument/publishDiagnostics" => {
                    let uri = params.get("uri").and_then(|v| v.as_str()).unwrap_or("");
                    let ver = params.get("version").and_then(|v| v.as_i64());
                    if let Some(d) = active.iter_mut().find(|d| d.uri == uri && ver.is_none_or(|v| v == d.version)) {
                        let diags = params.get("diagnostics").and_then(|p| p.as_array()).cloned().unwrap_or_default();
                        d.diag_errors = diags.iter().filter(|x| x.get("severity").and_then(|s| s.as_u64()) == Some(1)).count();
                        d.diag_warnings = diags.iter().filter(|x| x.get("severity").and_then(|s| s.as_u64()) == Some(2)).count();
                        // Lake's own messages arrive one at a time as transient
                        // information: remember which imports it named.
                        for x in &diags {
                            if let Some(m) = x.get("message").and_then(|m| m.as_str()) {
                                if let Some(name) = m.strip_prefix("- ").map(str::trim).filter(|n| !n.is_empty() && !n.contains(' ')) {
                                    if d.stale_imports.len() < 6 && !d.stale_imports.iter().any(|s| s == name) {
                                        d.stale_imports.push(name.to_string());
                                    }
                                }
                                if x.get("severity").and_then(|s| s.as_u64()) == Some(1) && d.first_error.is_none() {
                                    d.first_error = Some(m.lines().next().unwrap_or("").chars().take(240).collect());
                                }
                            }
                        }
                        let found = diags.iter().find_map(|x| {
                            let m = x.get("message").and_then(|m| m.as_str())?;
                            let is_err = x.get("severity").and_then(|s| s.as_u64()) == Some(1);
                            if is_err && m.starts_with("Imports are out of date") {
                                Some("imports are out of date and must be rebuilt (slim never builds; build the project, then retry)".to_string())
                            } else if is_err && m.contains("setup-file") && m.contains("failed") {
                                // The message repeats the whole command line; the
                                // useful part is what Lake wrote, if anything.
                                let said: String = m.split("stderr:").nth(1).unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" ").chars().take(200).collect();
                                Some(format!(
                                    "`lake setup-file --no-build` failed for this file (its imports are not available as built){}",
                                    if said.is_empty() { String::new() } else { format!(": {said}") }
                                ))
                            } else {
                                None
                            }
                        });
                        // Later (cleared) diagnostic lists do not undo it.
                        if found.is_some() {
                            d.header_failure = found;
                        }
                    }
                }
                _ => {}
            },
            Some(Incoming::Request { id, method, params }) => {
                if method == "workspace/semanticTokens/refresh" {
                    for d in active.iter_mut().filter(|d| d.tokens_id.is_some()) {
                        d.refresh_after_request = true;
                    }
                }
                let _ = conn.answer(&id, &method, &params);
            }
            Some(Incoming::Response { id, result }) => {
                if let Some((i, d)) = active.iter_mut().enumerate().find(|(_, d)| d.wait_id == id) {
                    match result {
                        Ok(_) => d.wait_done = Some(Instant::now()),
                        // A reason already recorded (stopped for memory, which
                        // ends the worker and thereby this request) stays.
                        Err((code, msg)) => {
                            d.fatal.get_or_insert_with(|| format!("waiting for the file to be processed failed ({code}): {msg}"));
                        }
                    }
                    let _ = i;
                } else if let Some((i, d)) = active.iter_mut().enumerate().find(|(_, d)| d.tokens_id == Some(id)) {
                    match result {
                        Ok(v) => {
                            if d.refresh_after_request && d.attempts < 3 && !d.timed_out {
                                // The server changed its mind after we asked: ask again.
                                d.tokens_id = None;
                                d.refresh_after_request = false;
                            } else {
                                finished.push((i, Some(v)));
                            }
                        }
                        Err((code, msg)) => {
                            d.fatal.get_or_insert_with(|| format!("the semantic token request failed ({code}): {msg}"));
                        }
                    }
                }
            }
            Some(Incoming::Closed(why)) => {
                // A server that went away because the job is being stopped
                // (quitting kills its process group at once) is not a
                // failure of the file that happened to be open.
                if ctl.cancel.load(Ordering::Relaxed) {
                    break 'run Outcome::Cancelled;
                }
                let tail = conn.stderr_tail();
                let detail = format!("the language server stopped: {why}{}", tail.last().map(|l| format!(" (last log line: {l})")).unwrap_or_default());
                for d in active.drain(..) {
                    let n_lines = d.text.split('\n').count();
                    let m = base_meta(&d, Coverage::Failed, detail.clone(), n_lines, &info);
                    store.put_semantic(&SemRecord { meta: m, blob: None })?;
                    counts.failed += 1;
                    on(Event::Published { rel: &d.rel, status: Coverage::Failed, detail: &detail, elapsed: d.opened.elapsed() });
                }
                restarts += 1;
                if restarts > 3 {
                    // Leave the queue for a later run.
                    break 'run Outcome::ServerFailed(detail);
                }
                match connect() {
                    Ok((c, i)) => {
                        std::mem::replace(&mut conn, c).close(false);
                        info = i;
                    }
                    Err(e) => {
                        let shutdown = conn.close(false);
                        let s = Summary {
                            counts,
                            outcome: Outcome::ServerFailed(format!("{detail}; restart failed: {e:#}")),
                            elapsed: start.elapsed(),
                            peak_tree_rss_kib: peak_rss,
                            restarts,
                            shutdown,
                            lean_version: env.lean_version.clone(),
                        };
                        store.finish_run(run_id, s.outcome.as_str(), &s.to_json())?;
                        return Ok(s);
                    }
                }
                continue;
            }
        }

        // Advance each open document.
        for (i, d) in active.iter_mut().enumerate() {
            if finished.iter().any(|(j, _)| *j == i) {
                continue;
            }
            if let (None, Some(t)) = (&d.fatal, d.fatal_progress) {
                if d.wait_done.is_some() || d.header_failure.is_some() || t.elapsed() > Duration::from_secs(2) {
                    d.fatal = Some(match &d.first_error {
                        Some(e) => format!("the server reported a fatal error while processing this file: {e}"),
                        None => "the server reported a fatal error while processing this file".into(),
                    });
                }
            }
            if d.fatal.is_some() {
                finished.push((i, None));
                continue;
            }
            if !d.timed_out && d.opened.elapsed() >= opts.file_timeout {
                // Take what the server has finished so far, labelled partial.
                d.timed_out = true;
                if d.tokens_id.is_none() {
                    d.attempts = 0;
                }
            }
            if d.timed_out && d.tokens_requested.is_some_and(|t| t.elapsed() > Duration::from_secs(15)) {
                d.fatal = Some(format!("time limit of {} s reached and the server did not return its partial tokens", opts.file_timeout.as_secs()));
                finished.push((i, None));
                continue;
            }
            // The completion signal: the wait request answered *and* an empty
            // progress report for this version. If the answer came but no
            // empty report follows, the result is kept as partial.
            let ready = match (d.wait_done, d.progress_done) {
                (Some(_), true) => true,
                (Some(t), false) if t.elapsed() > opts.completion_grace => {
                    d.unverified = true;
                    true
                }
                _ => false,
            };
            if (ready || d.timed_out) && d.tokens_id.is_none() {
                // On a write error the Closed message follows and handles it.
                if let Ok(id) = conn.request("textDocument/semanticTokens/full", json!({ "textDocument": { "uri": d.uri } })) {
                    d.tokens_id = Some(id);
                    d.tokens_requested = Some(Instant::now());
                    d.attempts += 1;
                }
            }
        }

        // Publish finished documents (highest index first: removal is by index).
        finished.sort_by_key(|f| std::cmp::Reverse(f.0));
        for (i, result) in finished {
            let d = active.remove(i);
            let lean_lines: Vec<&str> = d.text.split('\n').collect();
            let n_lines = lean_lines.len();
            let (status, detail, blob, n_tokens, processed) = (|| -> (Coverage, String, Option<Vec<u8>>, usize, Option<usize>) {
                // The precise reason first: imports that are not built.
                if let Some(h) = &d.header_failure {
                    let named = if d.stale_imports.is_empty() { String::new() } else { format!("; Lake named: {}", d.stale_imports.join(", ")) };
                    return (Coverage::Failed, format!("{h}{named}"), None, 0, None);
                }
                if let Some(f) = &d.fatal {
                    return (Coverage::Failed, f.clone(), None, 0, None);
                }
                let Some(v) = result else { return (Coverage::Failed, "no result".into(), None, 0, None) };
                let data: Option<Vec<u64>> = v.get("data").and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|x| x.as_u64()).collect());
                let raw_len = v.get("data").and_then(|a| a.as_array()).map(|a| a.len());
                let Some(data) = data.filter(|x| Some(x.len()) == raw_len) else {
                    return (Coverage::Failed, "the semantic token response has no valid `data` array".into(), None, 0, None);
                };
                let dec = match decode_lsp(&data, &info.legend, info.encoding, &lean_lines) {
                    Ok(dec) => dec,
                    Err(e) => return (Coverage::Failed, format!("{e:#}"), None, 0, None),
                };
                // Inputs must be what they were when the document was opened.
                let now_closure = tracker.closure(&d.rel, Some(&d.text));
                let now_art = tracker.artifacts(&d.closure.files);
                if now_closure.digest != d.closure.digest || now_art.digest != d.artifacts.digest {
                    return (Coverage::Failed, "imported sources or their compiled artifacts changed while this file was being processed".into(), None, 0, None);
                }
                if EnvInfo::current(root).fingerprint() != env.info.fingerprint() {
                    return (Coverage::Failed, "the toolchain or project configuration changed while this file was being processed".into(), None, 0, None);
                }
                let blob = super::codec::encode_semantic(&dec.chunks, n_lines);
                if d.timed_out {
                    let from = d.processing_from.map(|l| l as usize).unwrap_or(0).min(n_lines);
                    return (
                        Coverage::Partial,
                        format!("time limit of {} s reached: the server had processed the file up to about line {}", opts.file_timeout.as_secs(), from),
                        Some(blob),
                        dec.tokens,
                        Some(from),
                    );
                }
                if dec.dropped > 0 {
                    return (
                        Coverage::Partial,
                        format!("{} token(s) with invalid ranges were dropped ({})", dec.dropped, dec.reasons.join("; ")),
                        Some(blob),
                        dec.tokens,
                        None,
                    );
                }
                if d.unverified || !d.saw_progress {
                    return (Coverage::Partial, "the server gave no complete-processing report for this version".into(), Some(blob), dec.tokens, None);
                }
                let detail = if d.diag_errors > 0 {
                    format!("processed completely; Lean reported {} error(s) in this file", d.diag_errors)
                } else {
                    "processed completely".into()
                };
                (Coverage::Complete, detail, Some(blob), dec.tokens, None)
            })();
            let mut m = base_meta(&d, status, detail.clone(), n_lines, &info);
            m.n_tokens = n_tokens;
            m.processed_lines = processed;
            store.put_semantic(&SemRecord { meta: m, blob })?;
            match status {
                Coverage::Complete => counts.complete += 1,
                Coverage::Partial => counts.partial += 1,
                Coverage::Failed => counts.failed += 1,
                Coverage::Skipped => counts.skipped += 1,
            }
            if !d.closed {
                let _ = conn.notify("textDocument/didClose", json!({ "textDocument": { "uri": d.uri } }));
            }
            on(Event::Published { rel: &d.rel, status, detail: &detail, elapsed: d.opened.elapsed() });
        }

        if last_rss.elapsed() >= Duration::from_secs(1) {
            last_rss = Instant::now();
            rss = conn.rss_kib();
            peak_rss = peak_rss.max(rss.map(|r| r.0).unwrap_or(0));
            // A file whose elaboration eats the memory limit would take the
            // whole server (or the job) down with it. Before that happens
            // the file is closed, which ends its worker, and recorded as
            // failed for that reason; the run goes on.
            if let Some(m) = conn.cgroup_memory() {
                if let Some(max) = m.max.filter(|&max| max > 0 && m.anon as f64 >= MEMORY_STOP_FRACTION * max as f64) {
                    for d in active.iter_mut().filter(|d| d.fatal.is_none()) {
                        d.fatal = Some(format!(
                            "stopped for memory: the server's heap reached {} MiB of its {} MiB limit while this file was being processed",
                            m.anon >> 20,
                            max >> 20
                        ));
                        let _ = conn.notify("textDocument/didClose", json!({ "textDocument": { "uri": d.uri } }));
                        d.closed = true;
                    }
                }
            }
        }
        if last_progress.elapsed() >= Duration::from_millis(250) {
            last_progress = Instant::now();
            on(Event::Progress(&Progress {
                counts: counts.clone(),
                current: active.iter().map(|d| d.rel.clone()).collect(),
                elapsed: start.elapsed(),
                current_secs: active.iter().map(|d| d.opened.elapsed().as_secs()).max().unwrap_or(0),
                tree_rss_kib: rss.map(|r| r.0),
                tree_procs: rss.map(|r| r.1).unwrap_or(0),
                paused,
            }));
        }
    };

    // Documents still open were not finished: they keep their old record
    // (or none) and are tried again next time.
    counts.attempted -= active.len().min(counts.attempted);
    let shutdown = conn.close(outcome == Outcome::Finished || outcome == Outcome::BudgetExhausted);
    let s = Summary { counts, outcome, elapsed: start.elapsed(), peak_tree_rss_kib: peak_rss, restarts, shutdown, lean_version: env.lean_version.clone() };
    store.finish_run(run_id, s.outcome.as_str(), &s.to_json())?;
    on(Event::Progress(&Progress { counts: s.counts.clone(), elapsed: s.elapsed, ..Default::default() }));
    Ok(s)
}

/// `lean --version` of the toolchain (one bounded run of the toolchain's
/// own binary; part of the confirmed job, never of browsing).
fn lean_version(tc_dir: &Path) -> Option<String> {
    let mut c = Command::new(tc_dir.join("bin/lean"));
    c.arg("--version").env_clear();
    let lim = Limits { memory_mib: None, wall: Duration::from_secs(20), merge_stdout: true, ..Default::default() };
    let (st, out) = process::run_bounded(&mut c, &lim, &AtomicBool::new(false)).ok()?;
    st.success().then(|| out.join(" ").trim().to_string()).filter(|s| !s.is_empty())
}

/// Runs a plan with the real server.
pub fn run(plan: &Plan, ctl: &Control, on: &mut dyn FnMut(Event)) -> Result<Summary> {
    let store = Store::open(&plan.store_path)?;
    store.set_meta("project_root", &plan.project.root.display().to_string())?;
    store.set_meta("project_name", &plan.project.name)?;
    let env = EnvRecord {
        info: EnvInfo::current(&plan.project.root),
        lean_version: lean_version(&plan.toolchain_dir),
        sandbox: match &plan.sandbox {
            Sandbox::Bwrap { version, .. } => format!("bubblewrap {version}"),
            Sandbox::None => "none".into(),
        },
        server_command: format!("{:?}", plan.server_command()),
        created: crate::export::utc_now(),
    };
    let mut connect = || -> Result<(Box<dyn Conn>, ServerInfo)> {
        let mut client = Client::start(&mut plan.server_command(), &plan.limits)?;
        match client.initialize(&plan.project.root, Duration::from_secs(120), &ctl.cancel) {
            Ok(info) => {
                if !info.full {
                    client.shutdown(true);
                    bail!("this server does not answer full semantic token requests");
                }
                Ok((Box::new(client) as Box<dyn Conn>, info))
            }
            Err(e) => {
                client.shutdown(false);
                Err(e)
            }
        }
    };
    run_job(
        Job { root: &plan.project.root, toolchain: Some(&plan.toolchain), files: &plan.files, opts: &plan.opts, store: &store, env, connect: &mut connect },
        ctl,
        on,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::highlight::codec::Blob;
    use crate::highlight::{Legend, SemTok};
    use crate::text::PosEncoding;
    use std::cell::RefCell;
    use std::rc::Rc;

    type Script = Box<dyn FnMut(&str, &Value, Option<u64>, &mut VecDeque<Incoming>)>;

    /// A scripted server: every message the job sends goes to the script,
    /// which queues what the "server" says back.
    struct Fake {
        script: Script,
        queue: VecDeque<Incoming>,
        next_id: u64,
        closed: Rc<RefCell<Vec<bool>>>,
        memory: Rc<RefCell<Option<process::CgroupMemory>>>,
    }

    thread_local! {
        /// What the scripted server's cgroup reports (per test thread).
        static MEMORY: Rc<RefCell<Option<process::CgroupMemory>>> = Rc::new(RefCell::new(None));
    }

    impl Conn for Fake {
        fn request(&mut self, method: &str, params: Value) -> Result<u64> {
            self.next_id += 1;
            (self.script)(method, &params, Some(self.next_id), &mut self.queue);
            Ok(self.next_id)
        }
        fn notify(&mut self, method: &str, params: Value) -> Result<()> {
            (self.script)(method, &params, None, &mut self.queue);
            Ok(())
        }
        fn answer(&mut self, _: &Value, method: &str, _: &Value) -> Result<()> {
            (self.script)(&format!("answer:{method}"), &Value::Null, None, &mut self.queue);
            Ok(())
        }
        fn recv(&mut self, timeout: Duration) -> Option<Incoming> {
            let m = self.queue.pop_front();
            if m.is_none() {
                std::thread::sleep(timeout.min(Duration::from_millis(5)));
            }
            m
        }
        fn rss_kib(&self) -> Option<(u64, usize)> {
            None
        }
        fn cgroup_memory(&self) -> Option<process::CgroupMemory> {
            *self.memory.borrow()
        }
        fn stderr_tail(&self) -> Vec<String> {
            vec!["last words".into()]
        }
        fn close(self: Box<Self>, polite: bool) -> ShutdownReport {
            self.closed.borrow_mut().push(polite);
            ShutdownReport { clean: polite, ..Default::default() }
        }
    }

    fn info() -> ServerInfo {
        ServerInfo {
            legend: Legend { types: vec!["keyword".into(), "variable".into()], modifiers: vec![] },
            encoding: PosEncoding::Utf16,
            full: true,
            range: true,
            name: None,
        }
    }

    fn uri_of(p: &Value) -> String {
        p.pointer("/textDocument/uri").or(p.get("uri")).and_then(|v| v.as_str()).unwrap_or("").to_string()
    }

    fn progress(uri: &str, version: i64, from: Option<u64>) -> Incoming {
        let processing: Vec<Value> = from.map(|l| json!({"range": {"start": {"line": l, "character": 0}, "end": {"line": 99, "character": 0}}, "kind": 1})).into_iter().collect();
        Incoming::Notification { method: "$/lean/fileProgress".into(), params: json!({"textDocument": {"uri": uri, "version": version}, "processing": processing}) }
    }

    fn diags(uri: &str, version: i64, items: Value) -> Incoming {
        Incoming::Notification { method: "textDocument/publishDiagnostics".into(), params: json!({"uri": uri, "version": version, "diagnostics": items}) }
    }

    /// A server that processes every file completely and returns one
    /// keyword token at the start of line 0.
    fn well_behaved() -> Script {
        let mut versions: std::collections::HashMap<String, i64> = Default::default();
        Box::new(move |method, p, id, q| match method {
            "textDocument/didOpen" => {
                let (uri, v) = (uri_of(p), p.pointer("/textDocument/version").and_then(|v| v.as_i64()).unwrap());
                versions.insert(uri.clone(), v);
                q.push_back(progress(&uri, v, Some(0)));
                q.push_back(diags(&uri, v, json!([])));
                q.push_back(progress(&uri, v, None));
            }
            "textDocument/waitForDiagnostics" => q.push_back(Incoming::Response { id: id.unwrap(), result: Ok(json!({})) }),
            "textDocument/semanticTokens/full" => q.push_back(Incoming::Response { id: id.unwrap(), result: Ok(json!({"data": [0, 0, 3, 0, 0]})) }),
            _ => {}
        })
    }

    struct Proj {
        dir: tempfile::TempDir,
        store: Store,
    }

    fn project(files: &[(&str, &str)]) -> Proj {
        let dir = tempfile::tempdir().unwrap();
        for (rel, text) in files {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        let store = Store::open(&dir.path().join(".cache/highlight.sqlite")).unwrap();
        Proj { dir, store }
    }

    fn run_with(p: &Proj, files: &[&str], opts: &Options, mut scripts: Vec<Script>, ctl: &Control) -> (Summary, Rc<RefCell<Vec<bool>>>, Vec<(String, Coverage)>) {
        let closed = Rc::new(RefCell::new(Vec::new()));
        let c2 = closed.clone();
        let mut connect = move || -> Result<(Box<dyn Conn>, ServerInfo)> {
            if scripts.is_empty() {
                bail!("no more servers");
            }
            Ok((Box::new(Fake { script: scripts.remove(0), queue: VecDeque::new(), next_id: 0, closed: c2.clone(), memory: MEMORY.with(|m| m.clone()) }) as Box<dyn Conn>, info()))
        };
        let files: Vec<String> = files.iter().map(|s| s.to_string()).collect();
        let mut published = Vec::new();
        let env = EnvRecord { info: EnvInfo::current(p.dir.path()), ..Default::default() };
        let s = run_job(
            Job { root: p.dir.path(), toolchain: None, files: &files, opts, store: &p.store, env, connect: &mut connect },
            ctl,
            &mut |e| {
                if let Event::Published { rel, status, .. } = e {
                    published.push((rel.to_string(), status));
                }
            },
        )
        .unwrap();
        (s, closed, published)
    }

    fn quick() -> Options {
        Options { file_timeout: Duration::from_secs(20), completion_grace: Duration::from_millis(60), ..Default::default() }
    }

    #[test]
    fn complete_files_are_published_and_later_runs_resume_at_file_boundaries() {
        let p = project(&[("P/A.lean", "import P.B\ndef a := b\n"), ("P/B.lean", "def b := 1\n"), ("P/C.lean", "def c := 1\n")]);
        let all = ["P/A.lean", "P/B.lean", "P/C.lean"];
        let ctl = Control::default();
        let (s, closed, _) = run_with(&p, &all, &quick(), vec![well_behaved()], &ctl);
        assert_eq!(s.outcome, Outcome::Finished);
        assert_eq!((s.counts.attempted, s.counts.complete, s.counts.fresh), (3, 3, 0));
        assert_eq!(*closed.borrow(), vec![true], "one server, shut down politely");
        let rec = p.store.get_semantic("P/A.lean").unwrap().unwrap();
        assert_eq!(rec.meta.status, Coverage::Complete);
        assert_eq!((rec.meta.n_lines, rec.meta.n_tokens, rec.meta.deps_count), (3, 1, 1));
        assert_eq!(rec.meta.position_encoding, "utf-16");
        let blob = Blob::<SemTok>::parse(rec.blob.unwrap()).unwrap();
        assert_eq!(blob.chunk(0).unwrap().line(0), &[SemTok { start: 0, end: 3, ty: 0, mods: 0 }]);
        // Nothing changed: nothing is opened again.
        let (s, _, _) = run_with(&p, &all, &quick(), vec![well_behaved()], &ctl);
        assert_eq!((s.counts.attempted, s.counts.fresh), (0, 3));
        // B changes: B itself and its importer A are redone; C is not.
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(p.dir.path().join("P/B.lean"), "def b := 2\n").unwrap();
        let (s, _, published) = run_with(&p, &all, &quick(), vec![well_behaved()], &ctl);
        assert_eq!((s.counts.attempted, s.counts.fresh), (2, 1));
        let mut names: Vec<_> = published.iter().map(|x| x.0.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["P/A.lean", "P/B.lean"]);
        // A run records its outcome.
        assert_eq!(p.store.last_run().unwrap().unwrap().2.as_deref(), Some("finished"));
    }

    #[test]
    fn errors_in_a_file_do_not_reduce_coverage_but_stale_imports_fail() {
        let p = project(&[("A.lean", "def a := 1\ntheorem bad : False := by simp\n"), ("B.lean", "import X\n")]);
        let script: Script = Box::new(|method, par, id, q| match method {
            "textDocument/didOpen" => {
                let (uri, v) = (uri_of(par), par.pointer("/textDocument/version").and_then(|v| v.as_i64()).unwrap());
                let d = if uri.ends_with("A.lean") {
                    json!([{"severity": 1, "message": "simp made no progress", "range": {"start": {"line": 1, "character": 0}}}, {"severity": 2, "message": "unused"}])
                } else {
                    json!([{"severity": 1, "message": "Imports are out of date and must be rebuilt; use the \"Restart File\" command in your editor.", "range": {"start": {"line": 0, "character": 0}}}])
                };
                q.push_back(diags(&uri, v, d));
                q.push_back(progress(&uri, v, None));
            }
            "textDocument/waitForDiagnostics" => q.push_back(Incoming::Response { id: id.unwrap(), result: Ok(json!({})) }),
            "textDocument/semanticTokens/full" => q.push_back(Incoming::Response { id: id.unwrap(), result: Ok(json!({"data": [0, 0, 3, 0, 0]})) }),
            _ => {}
        });
        let (s, _, _) = run_with(&p, &["A.lean", "B.lean"], &quick(), vec![script], &Control::default());
        assert_eq!((s.counts.complete, s.counts.failed), (1, 1));
        let a = p.store.get_semantic_meta("A.lean").unwrap().unwrap();
        assert_eq!((a.status, a.diag_errors, a.diag_warnings), (Coverage::Complete, 1, 1));
        assert!(a.detail.contains("1 error"), "{}", a.detail);
        let b = p.store.get_semantic("B.lean").unwrap().unwrap();
        assert_eq!(b.meta.status, Coverage::Failed);
        assert!(b.meta.detail.contains("out of date") && b.meta.detail.contains("never builds"), "{}", b.meta.detail);
        assert!(b.blob.is_none(), "a failed record carries no tokens");
        // A failed record with unchanged inputs is not retried unless asked.
        let (s, _, _) = run_with(&p, &["A.lean", "B.lean"], &quick(), vec![well_behaved()], &Control::default());
        assert_eq!((s.counts.attempted, s.counts.fresh), (0, 2));
        let (s, _, _) = run_with(&p, &["A.lean", "B.lean"], &Options { retry: true, ..quick() }, vec![well_behaved()], &Control::default());
        assert_eq!((s.counts.attempted, s.counts.fresh, s.counts.complete), (1, 1, 1));
    }

    #[test]
    fn a_fatal_header_error_is_recorded_with_its_reason() {
        let p = project(&[("A.lean", "import X.Y\ndef a := 1\n"), ("B.lean", "def b := 1\n")]);
        // The order the real server uses: Lake's lines as transient
        // information, the fatal progress report, then the error.
        let script: Script = Box::new(|method, par, id, q| {
            let uri = uri_of(par);
            match method {
                "textDocument/didOpen" if uri.ends_with("A.lean") => {
                    q.push_back(diags(&uri, 1, json!([{"severity": 3, "message": "error: target is out-of-date and needs to be rebuilt\n"}])));
                    q.push_back(diags(&uri, 1, json!([{"severity": 3, "message": "Some required targets logged failures:\n"}])));
                    q.push_back(diags(&uri, 1, json!([{"severity": 3, "message": "- X.Y\n"}])));
                    q.push_back(Incoming::Notification {
                        method: "$/lean/fileProgress".into(),
                        params: json!({"textDocument": {"uri": uri, "version": 1}, "processing": [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 1, "character": 0}}, "kind": 2}]}),
                    });
                    q.push_back(diags(&uri, 1, json!([{"severity": 1, "message": "Imports are out of date and must be rebuilt; use the \"Restart File\" command in your editor."}])));
                }
                "textDocument/didOpen" => {
                    // B: a fatal error with a message of its own, never a wait answer.
                    q.push_back(Incoming::Notification {
                        method: "$/lean/fileProgress".into(),
                        params: json!({"textDocument": {"uri": uri, "version": 2}, "processing": [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 1, "character": 0}}, "kind": 2}]}),
                    });
                    q.push_back(diags(&uri, 2, json!([{"severity": 1, "message": "unknown module prefix 'Q'\nmore lines"}])));
                }
                "textDocument/waitForDiagnostics" if uri.ends_with("A.lean") => q.push_back(Incoming::Response { id: id.unwrap(), result: Ok(json!({})) }),
                _ => {}
            }
        });
        let (s, _, _) = run_with(&p, &["A.lean", "B.lean"], &quick(), vec![script], &Control::default());
        assert_eq!(s.counts.failed, 2);
        let a = p.store.get_semantic_meta("A.lean").unwrap().unwrap();
        assert!(a.detail.contains("imports are out of date") && a.detail.contains("Lake named: X.Y"), "{}", a.detail);
        let b = p.store.get_semantic_meta("B.lean").unwrap().unwrap();
        assert!(b.detail.contains("fatal error") && b.detail.contains("unknown module prefix 'Q'") && !b.detail.contains("more lines"), "{}", b.detail);
    }

    #[test]
    fn a_file_over_its_time_limit_is_partial_and_does_not_stall_the_rest() {
        let p = project(&[("Slow.lean", "def a := 1\ndef b := 2\ndef c := 3\ndef d := 4\n"), ("Z.lean", "def z := 1\n")]);
        let mut good = well_behaved();
        let script: Script = Box::new(move |method, par, id, q| {
            let uri = uri_of(par);
            if !uri.ends_with("Slow.lean") {
                return good(method, par, id, q);
            }
            match method {
                // Processing stops at line 3 and never completes; the wait
                // request is never answered.
                "textDocument/didOpen" => q.push_back(progress(&uri, par.pointer("/textDocument/version").and_then(|v| v.as_i64()).unwrap(), Some(3))),
                // The finished prefix: tokens on lines 0..=2 only.
                "textDocument/semanticTokens/full" => q.push_back(Incoming::Response { id: id.unwrap(), result: Ok(json!({"data": [0, 0, 3, 0, 0, 1, 0, 3, 0, 0, 1, 0, 3, 0, 0]})) }),
                _ => {}
            }
        });
        let opts = Options { file_timeout: Duration::from_millis(150), ..quick() };
        let (s, _, published) = run_with(&p, &["Slow.lean", "Z.lean"], &opts, vec![script], &Control::default());
        assert_eq!(s.outcome, Outcome::Finished);
        assert_eq!(published, vec![("Slow.lean".to_string(), Coverage::Partial), ("Z.lean".to_string(), Coverage::Complete)]);
        let m = p.store.get_semantic_meta("Slow.lean").unwrap().unwrap();
        assert_eq!((m.status, m.n_tokens, m.processed_lines), (Coverage::Partial, 3, Some(3)));
        assert!(m.detail.contains("time limit"), "{}", m.detail);
    }

    #[test]
    fn a_refresh_after_the_token_request_makes_the_job_ask_again() {
        let p = project(&[("A.lean", "def a := 1\ndef b := a\n")]);
        let asked = Rc::new(RefCell::new(0));
        let a2 = asked.clone();
        let script: Script = Box::new(move |method, par, id, q| match method {
            "textDocument/didOpen" => q.push_back(progress(&uri_of(par), 1, None)),
            "textDocument/waitForDiagnostics" => q.push_back(Incoming::Response { id: id.unwrap(), result: Ok(json!({})) }),
            "textDocument/semanticTokens/full" => {
                *a2.borrow_mut() += 1;
                if *a2.borrow() == 1 {
                    // A late refresh overtakes the first, incomplete answer.
                    q.push_back(Incoming::Request { id: json!(7), method: "workspace/semanticTokens/refresh".into(), params: Value::Null });
                    q.push_back(Incoming::Response { id: id.unwrap(), result: Ok(json!({"data": [0, 0, 3, 0, 0]})) });
                } else {
                    q.push_back(Incoming::Response { id: id.unwrap(), result: Ok(json!({"data": [0, 0, 3, 0, 0, 1, 0, 3, 0, 0]})) });
                }
            }
            _ => {}
        });
        let (s, _, _) = run_with(&p, &["A.lean"], &quick(), vec![script], &Control::default());
        assert_eq!(s.counts.complete, 1);
        assert_eq!(*asked.borrow(), 2);
        assert_eq!(p.store.get_semantic_meta("A.lean").unwrap().unwrap().n_tokens, 2, "the final answer is the one stored");
    }

    #[test]
    fn reports_for_other_versions_and_missing_completion_reports_do_not_count() {
        let p = project(&[("A.lean", "def a := 1\n")]);
        // An empty progress report for an older version, then nothing.
        let script: Script = Box::new(|method, par, id, q| match method {
            "textDocument/didOpen" => q.push_back(progress(&uri_of(par), 0, None)),
            "textDocument/waitForDiagnostics" => q.push_back(Incoming::Response { id: id.unwrap(), result: Ok(json!({})) }),
            "textDocument/semanticTokens/full" => q.push_back(Incoming::Response { id: id.unwrap(), result: Ok(json!({"data": [0, 0, 3, 0, 0]})) }),
            _ => {}
        });
        let (s, _, _) = run_with(&p, &["A.lean"], &quick(), vec![script], &Control::default());
        assert_eq!((s.counts.complete, s.counts.partial), (0, 1));
        let m = p.store.get_semantic_meta("A.lean").unwrap().unwrap();
        assert!(m.detail.contains("no complete-processing report"), "{}", m.detail);
    }

    #[test]
    fn malformed_token_data_never_becomes_a_complete_record() {
        for (data, status, needle) in [
            (json!({"data": [0, 0, 1]}), Coverage::Failed, "multiple of 5"),
            (json!({"data": "nope"}), Coverage::Failed, "no valid `data`"),
            (json!({"data": [0, 0, 3, 0, -1]}), Coverage::Failed, "no valid `data`"),
            (json!({}), Coverage::Failed, "no valid `data`"),
            // One good token and one past the end of its line.
            (json!({"data": [0, 0, 3, 0, 0, 0, 50, 4, 1, 0]}), Coverage::Partial, "dropped"),
        ] {
            let p = project(&[("A.lean", "def a := 1\n")]);
            let d = data.clone();
            let script: Script = Box::new(move |method, par, id, q| match method {
                "textDocument/didOpen" => q.push_back(progress(&uri_of(par), 1, None)),
                "textDocument/waitForDiagnostics" => q.push_back(Incoming::Response { id: id.unwrap(), result: Ok(json!({})) }),
                "textDocument/semanticTokens/full" => q.push_back(Incoming::Response { id: id.unwrap(), result: Ok(d.clone()) }),
                _ => {}
            });
            run_with(&p, &["A.lean"], &quick(), vec![script], &Control::default());
            let m = p.store.get_semantic_meta("A.lean").unwrap().unwrap();
            assert_eq!(m.status, status, "{data}");
            assert!(m.detail.contains(needle), "{} / {needle}", m.detail);
        }
    }

    #[test]
    fn a_dying_server_fails_the_open_file_and_is_restarted_a_bounded_number_of_times() {
        let p = project(&[("A.lean", "def a := 1\n"), ("B.lean", "def b := 1\n")]);
        let dying = || -> Script {
            Box::new(|method, _, _, q| {
                if method == "textDocument/didOpen" {
                    q.push_back(Incoming::Closed("the server closed its output".into()));
                }
            })
        };
        // First server dies on A; the second finishes B.
        let (s, closed, published) = run_with(&p, &["A.lean", "B.lean"], &quick(), vec![dying(), well_behaved()], &Control::default());
        assert_eq!(s.outcome, Outcome::Finished);
        assert_eq!((s.restarts, s.counts.failed, s.counts.complete), (1, 1, 1));
        assert_eq!(published[0], ("A.lean".to_string(), Coverage::Failed));
        assert_eq!(closed.borrow().len(), 2, "both servers were closed");
        let m = p.store.get_semantic_meta("A.lean").unwrap().unwrap();
        assert!(m.detail.contains("stopped") && m.detail.contains("last words"), "{}", m.detail);
        // A server that always dies ends the run instead of looping.
        let p = project(&[("A.lean", "a"), ("B.lean", "b"), ("C.lean", "c"), ("D.lean", "d"), ("E.lean", "e"), ("F.lean", "f")]);
        let (s, _, _) = run_with(&p, &["A.lean", "B.lean", "C.lean", "D.lean", "E.lean", "F.lean"], &quick(), (0..10).map(|_| dying()).collect(), &Control::default());
        assert!(matches!(s.outcome, Outcome::ServerFailed(_)));
        assert_eq!(s.counts.failed, 4);
        assert_eq!(s.remaining(), 2, "the rest is left for a later run");
        // No server at all.
        let (s, _, _) = run_with(&p, &["A.lean"], &Options { retry: true, ..quick() }, Vec::new(), &Control::default());
        assert!(matches!(s.outcome, Outcome::ServerFailed(_)));
    }

    #[test]
    fn cancelling_keeps_finished_files_and_leaves_the_open_one_unrecorded() {
        let p = project(&[("A.lean", "def a := 1\n"), ("B.lean", "def b := 1\n"), ("C.lean", "def c := 1\n")]);
        let ctl = std::sync::Arc::new(Control::default());
        let c2 = ctl.clone();
        let mut good = well_behaved();
        let script: Script = Box::new(move |method, par, id, q| {
            if uri_of(par).ends_with("B.lean") {
                // The reader cancels while B is open; B never completes.
                c2.cancel.store(true, Ordering::Relaxed);
                return;
            }
            good(method, par, id, q)
        });
        let (s, closed, _) = run_with(&p, &["A.lean", "B.lean", "C.lean"], &quick(), vec![script], &ctl);
        assert_eq!(s.outcome, Outcome::Cancelled);
        assert_eq!((s.counts.complete, s.counts.attempted, s.remaining()), (1, 1, 2));
        assert_eq!(*closed.borrow(), vec![false], "a cancelled server is terminated, not waited for");
        assert!(p.store.get_semantic_meta("A.lean").unwrap().is_some());
        assert!(p.store.get_semantic_meta("B.lean").unwrap().is_none(), "no half record");
        assert_eq!(p.store.last_run().unwrap().unwrap().2.as_deref(), Some("cancelled"));
        // The next run continues with B and C only.
        let (s, _, _) = run_with(&p, &["A.lean", "B.lean", "C.lean"], &quick(), vec![well_behaved()], &Control::default());
        assert_eq!((s.counts.fresh, s.counts.complete), (1, 2));
    }

    #[test]
    fn a_server_killed_because_the_job_is_stopping_fails_no_file() {
        let p = project(&[("A.lean", "def a := 1\n"), ("B.lean", "def b := 1\n")]);
        let ctl = std::sync::Arc::new(Control::default());
        let c2 = ctl.clone();
        let mut good = well_behaved();
        let script: Script = Box::new(move |method, par, id, q| {
            if method == "textDocument/didOpen" && uri_of(par).ends_with("B.lean") {
                // Quitting: the flag is set and the process group is killed
                // before the job has looked at the flag.
                c2.cancel.store(true, Ordering::Relaxed);
                q.push_back(Incoming::Closed("the server closed its output".into()));
                return;
            }
            good(method, par, id, q)
        });
        let (s, _, published) = run_with(&p, &["A.lean", "B.lean"], &quick(), vec![script], &ctl);
        assert_eq!(s.outcome, Outcome::Cancelled);
        assert_eq!((s.counts.complete, s.counts.failed, s.restarts), (1, 0, 0));
        assert_eq!(published.len(), 1);
        assert!(p.store.get_semantic_meta("B.lean").unwrap().is_none(), "B is simply not done yet");
    }

    #[test]
    fn a_file_that_eats_the_memory_limit_is_stopped_and_the_run_goes_on() {
        let p = project(&[("Greedy.lean", "def g := 1\n"), ("Z.lean", "def z := 1\n")]);
        let closes = Rc::new(RefCell::new(Vec::new()));
        let c2 = closes.clone();
        let mem = MEMORY.with(|m| m.clone());
        let m2 = mem.clone();
        let mut good = well_behaved();
        let script: Script = Box::new(move |method, par, id, q| {
            let uri = uri_of(par);
            if method == "textDocument/didClose" {
                c2.borrow_mut().push(uri.rsplit('/').next().unwrap().to_string());
                if uri.ends_with("Greedy.lean") {
                    // The real server then fails the pending wait request.
                    q.push_back(Incoming::Response { id: 1, result: Err((-32801, "The file worker has been terminated.".into())) });
                }
                // Closing the document ends its worker: memory is back.
                *m2.borrow_mut() = Some(process::CgroupMemory { anon: 300 << 20, current: 900 << 20, max: Some(4096 << 20) });
            }
            if uri.ends_with("Greedy.lean") {
                if method == "textDocument/didOpen" {
                    // Never completes; its worker's heap is at 75 % of the limit.
                    *m2.borrow_mut() = Some(process::CgroupMemory { anon: 3072 << 20, current: 3500 << 20, max: Some(4096 << 20) });
                    q.push_back(progress(&uri, 1, Some(0)));
                }
                return;
            }
            good(method, par, id, q)
        });
        let (s, _, published) = run_with(&p, &["Greedy.lean", "Z.lean"], &quick(), vec![script], &Control::default());
        *mem.borrow_mut() = None;
        assert_eq!(s.outcome, Outcome::Finished);
        assert_eq!(published, vec![("Greedy.lean".to_string(), Coverage::Failed), ("Z.lean".to_string(), Coverage::Complete)]);
        assert_eq!(s.restarts, 0, "the server itself survived");
        let m = p.store.get_semantic_meta("Greedy.lean").unwrap().unwrap();
        assert!(m.detail.contains("stopped for memory") && m.detail.contains("3072 MiB of its 4096 MiB"), "{}", m.detail);
        assert_eq!(*closes.borrow(), vec!["Greedy.lean", "Z.lean"], "each document is closed exactly once");
        // File cache alone near the limit stops nothing.
        let p = project(&[("A.lean", "def a := 1\n")]);
        *mem.borrow_mut() = Some(process::CgroupMemory { anon: 500 << 20, current: 4000 << 20, max: Some(4096 << 20) });
        let (s, _, _) = run_with(&p, &["A.lean"], &quick(), vec![well_behaved()], &Control::default());
        *mem.borrow_mut() = None;
        assert_eq!(s.counts.complete, 1);
    }

    #[test]
    fn inputs_that_change_during_extraction_are_not_published_as_consistent() {
        let p = project(&[("lakefile.toml", "name = \"p\"\n"), ("P/A.lean", "import P.B\ndef a := b\n"), ("P/B.lean", "def b := 1\n")]);
        let root = p.dir.path().to_path_buf();
        let mut good = well_behaved();
        let script: Script = Box::new(move |method, par, id, q| {
            if method == "textDocument/didOpen" {
                // An import is edited while A is being processed.
                std::thread::sleep(Duration::from_millis(20));
                std::fs::write(root.join("P/B.lean"), "def b := 99\n").unwrap();
            }
            good(method, par, id, q)
        });
        run_with(&p, &["P/A.lean"], &quick(), vec![script], &Control::default());
        let m = p.store.get_semantic_meta("P/A.lean").unwrap().unwrap();
        assert_eq!(m.status, Coverage::Failed);
        assert!(m.detail.contains("changed while"), "{}", m.detail);
    }

    #[test]
    fn budget_pause_and_unreadable_files() {
        let p = project(&[("A.lean", "def a := 1\n"), ("Big.lean", "-- 0123456789012345678901234567890123456789\n")]);
        std::fs::write(p.dir.path().join("Bin.lean"), [0xff, 0xfe, 0x00]).unwrap();
        // No time at all: nothing is opened, and the run says so.
        let (s, _, _) = run_with(&p, &["A.lean"], &Options { budget: Some(Duration::ZERO), ..quick() }, vec![well_behaved()], &Control::default());
        assert_eq!((s.outcome.clone(), s.counts.attempted, s.remaining()), (Outcome::BudgetExhausted, 0, 1));
        // Too large and not UTF-8 are skipped with the reason, not opened.
        let opts = Options { max_file_bytes: 16, ..quick() };
        let (s, _, _) = run_with(&p, &["A.lean", "Big.lean", "Bin.lean", "Gone.lean"], &opts, vec![well_behaved()], &Control::default());
        assert_eq!((s.counts.complete, s.counts.skipped, s.counts.attempted), (1, 3, 1));
        assert!(p.store.get_semantic_meta("Big.lean").unwrap().unwrap().detail.contains("size limit"));
        assert!(p.store.get_semantic_meta("Bin.lean").unwrap().unwrap().detail.contains("UTF-8"));
        assert!(p.store.get_semantic_meta("Gone.lean").unwrap().unwrap().detail.contains("cannot read"));
        // Paused: no new document is opened until resumed (here: cancelled).
        let p = project(&[("A.lean", "def a := 1\n")]);
        let ctl = std::sync::Arc::new(Control::default());
        ctl.pause.store(true, Ordering::Relaxed);
        let c2 = ctl.clone();
        let h = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            c2.cancel.store(true, Ordering::Relaxed);
        });
        let (s, _, _) = run_with(&p, &["A.lean"], &quick(), vec![well_behaved()], &ctl);
        h.join().unwrap();
        assert_eq!((s.outcome.clone(), s.counts.attempted), (Outcome::Cancelled, 0));
    }

    #[test]
    fn several_open_documents_finish_independently() {
        let files: Vec<(String, String)> = (0..7).map(|i| (format!("F{i}.lean"), format!("def f{i} := {i}\n"))).collect();
        let refs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let p = project(&refs);
        let names: Vec<&str> = files.iter().map(|f| f.0.as_str()).collect();
        let open_now = Rc::new(RefCell::new((0usize, 0usize)));
        let o2 = open_now.clone();
        let mut good = well_behaved();
        let script: Script = Box::new(move |method, par, id, q| {
            match method {
                "textDocument/didOpen" => {
                    let mut o = o2.borrow_mut();
                    o.0 += 1;
                    o.1 = o.1.max(o.0);
                }
                "textDocument/didClose" => o2.borrow_mut().0 -= 1,
                _ => {}
            }
            good(method, par, id, q)
        });
        let (s, _, _) = run_with(&p, &names, &Options { open_docs: 3, ..quick() }, vec![script], &Control::default());
        assert_eq!(s.counts.complete, 7);
        assert!(open_now.borrow().1 <= 3 && open_now.borrow().1 >= 2, "bounded fan-out: {:?}", open_now.borrow());
        assert_eq!(open_now.borrow().0, 0, "every document was closed");
    }

    #[test]
    fn plan_refuses_what_it_must_not_do_silently() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        let o = Options::default();
        let err = |r: &Path, o: &Options| format!("{:#}", plan(r, o).err().unwrap());
        assert!(err(r, &o).contains("no lean-toolchain"));
        std::fs::write(r.join("lean-toolchain"), "leanprover/lean4:v4.99.0-not-installed\n").unwrap();
        assert!(err(r, &o).contains("not in the tested list"));
        let o2 = Options { allow_untested: true, ..Default::default() };
        let e = err(r, &o2);
        assert!(e.contains("is not installed") && e.contains("never installs"), "{e}");
        std::fs::write(r.join("lean-toolchain"), "leanprover/lean4:v3.51.1\n").unwrap();
        assert!(err(r, &o2).contains("not supported"));
        assert!(matches_filter("A/B.lean", &[]) && matches_filter("A/B.lean", &["A".into()]) && matches_filter("A/B.lean", &["./A/B.lean".into()]));
        assert!(!matches_filter("AB/C.lean", &["A".into()]));
    }
}
