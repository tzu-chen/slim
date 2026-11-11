//! The explicit semantic-export action.
//!
//! `slim export <PROJECT>` (or the confirmed palette action in the UI):
//!
//! 1. inspects the project as text (`lean-toolchain`, `lakefile.toml`,
//!    `lake-manifest.json`) and its existing build artifacts
//!    (`.lake/build/lib/lean/**/*.olean`); it never runs Lake or the build;
//! 2. selects the exporter compatibility shim for the project's exact Lean
//!    toolchain, which must already be installed (slim never installs or
//!    updates toolchains);
//! 3. builds slim's own exporter (embedded in this binary) with that
//!    toolchain into the cache directory;
//! 4. runs the exporter on the project's build artifacts inside a sandbox
//!    (bubblewrap: no network, read-only project and toolchain, private
//!    `/tmp`, cleared environment, only the output directory writable) with
//!    resource limits, after printing exactly what will run and receiving
//!    an explicit confirmation;
//! 5. validates the exporter output and wraps it into a versioned snapshot
//!    with provenance and source fingerprints, optionally importing it.

use crate::cli::Args;
use crate::paths::{sha256_hex, Project};
use crate::process::{self, Limits, Outcome};
use crate::snapshot::{self, Counts, Footer, Header, LeanInfo, Line, ProjectInfo, Producer};
use crate::source::lexer::parse_lean_version;
use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const EXPORTER_MAIN: &str = include_str!("../exporter/Main.lean");
pub const EXPORTER_LAKEFILE: &str = include_str!("../exporter/lakefile.toml");
pub const COMPAT_LEGACY: &str = include_str!("../exporter/compat/legacy/Compat.lean");
pub const COMPAT_MID: &str = include_str!("../exporter/compat/mid/Compat.lean");
pub const COMPAT_MODERN: &str = include_str!("../exporter/compat/modern/Compat.lean");

/// Toolchains on which the exporter was built and run against the fixture,
/// with its axiom analysis compared to Lean's own report (see docs/design.md).
pub const TESTED_TOOLCHAINS: &[&str] = &[
    "leanprover/lean4:v4.16.0",
    "leanprover/lean4:v4.21.0",
    "leanprover/lean4:v4.28.0",
    "leanprover/lean4:v4.33.1",
    "leanprover/lean4:v4.35.0",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compat {
    Legacy,
    Mid,
    Modern,
}

impl Compat {
    pub fn for_toolchain(tc: &str) -> Option<Compat> {
        match parse_lean_version(tc)? {
            (4, m, _) if (16..21).contains(&m) => Some(Compat::Legacy),
            (4, m, _) if (21..28).contains(&m) => Some(Compat::Mid),
            (4, m, _) if m >= 28 => Some(Compat::Modern),
            _ => None,
        }
    }
    pub fn source(self) -> &'static str {
        match self {
            Compat::Legacy => COMPAT_LEGACY,
            Compat::Mid => COMPAT_MID,
            Compat::Modern => COMPAT_MODERN,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Compat::Legacy => "legacy (4.16–4.20)",
            Compat::Mid => "mid (4.21–4.27)",
            Compat::Modern => "modern (≥ 4.28)",
        }
    }
}

/// `leanprover/lean4:v4.33.1` → `leanprover--lean4---v4.33.1` (elan's directory naming).
pub fn elan_dir_name(tc: &str) -> String {
    tc.trim().replace('/', "--").replace(':', "---")
}

pub fn elan_home() -> PathBuf {
    std::env::var_os("ELAN_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".elan"))
}

pub fn toolchain_dir(tc: &str) -> PathBuf {
    elan_home().join("toolchains").join(elan_dir_name(tc))
}

#[derive(Clone, Debug)]
pub enum Sandbox {
    Bwrap { path: PathBuf, version: String },
    None,
}

impl Sandbox {
    pub fn describe(&self) -> String {
        match self {
            Sandbox::Bwrap { version, .. } => format!(
                "bubblewrap {version}: new user/pid/net/ipc/uts/cgroup namespaces (no network), read-only /usr, \
                 toolchain and project, private /tmp, cleared environment, only the output directory writable"
            ),
            Sandbox::None => "NONE: the exporter runs as your user with full access to your files and network".into(),
        }
    }
}

/// Detects a usable bubblewrap (user namespaces must be permitted).
pub fn detect_sandbox() -> Sandbox {
    for cand in ["/usr/bin/bwrap", "/usr/local/bin/bwrap", "/bin/bwrap"] {
        let p = Path::new(cand);
        if !p.exists() {
            continue;
        }
        let version = Command::new(p)
            .arg("--version")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().trim_start_matches("bubblewrap ").to_string())
            .unwrap_or_default();
        let ok = Command::new(p)
            .args(["--unshare-all", "--die-with-parent", "--ro-bind", "/", "/", "--", "/bin/true"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return Sandbox::Bwrap { path: p.to_path_buf(), version };
        }
    }
    Sandbox::None
}

/// Locally built modules: `.olean` files under the build directory whose
/// source exists in the project (module name ↔ relative path).
pub fn local_built_modules(root: &Path) -> Result<(PathBuf, Vec<(String, String)>)> {
    let candidates = [root.join(".lake/build/lib/lean"), root.join(".lake/build/lib"), root.join("build/lib")];
    let Some(build) = candidates.iter().find(|p| p.is_dir()).cloned() else {
        bail!(
            "no build artifacts found (looked for .lake/build/lib/lean). slim does not build projects: \
             run `lake build` yourself if you trust this project, then retry"
        );
    };
    let mut out = Vec::new();
    let mut stack = vec![build.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)?.flatten() {
            let ft = e.file_type()?;
            let p = e.path();
            if ft.is_dir() {
                stack.push(p);
            } else if ft.is_file() && p.extension().is_some_and(|x| x == "olean") {
                let rel = p.strip_prefix(&build).unwrap().to_string_lossy().to_string();
                let module = rel.trim_end_matches(".olean").replace('/', ".");
                let src = rel.trim_end_matches(".olean").to_string() + ".lean";
                if crate::paths::resolve_contained(root, &src).is_ok_and(|p| p.is_file()) {
                    out.push((module, src));
                }
            }
        }
    }
    out.sort();
    Ok((build, out))
}

fn mtime_ns(p: &Path) -> Option<i64> {
    std::fs::metadata(p).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok().map(|d| d.as_nanos() as i64)
}

/// UTC timestamp `YYYY-MM-DDTHH:MM:SSZ` without external crates.
pub fn utc_now() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    // civil-from-days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, (rem % 3600) / 60, rem % 60)
}

/// Reads the checked-out git revision from `.git` files (no git process).
pub fn git_revision(root: &Path) -> Option<String> {
    let head = std::fs::read_to_string(root.join(".git/HEAD")).ok()?;
    let head = head.trim();
    if let Some(r) = head.strip_prefix("ref: ") {
        if let Ok(s) = std::fs::read_to_string(root.join(".git").join(r)) {
            return Some(s.trim().to_string());
        }
        let packed = std::fs::read_to_string(root.join(".git/packed-refs")).ok()?;
        return packed.lines().find(|l| l.ends_with(r)).and_then(|l| l.split_whitespace().next()).map(String::from);
    }
    (head.len() == 40).then(|| head.to_string())
}

pub struct ExportPlan {
    pub project: Project,
    pub toolchain: String,
    pub toolchain_dir: PathBuf,
    pub compat: Compat,
    pub tested: bool,
    pub build_dir: PathBuf,
    pub modules: Vec<(String, String)>,
    pub search_path: Vec<PathBuf>,
    pub exporter_dir: PathBuf,
    pub sandbox: Sandbox,
    pub limits: Limits,
    pub library: String,
    pub library_types: String,
    pub pp: String,
    pub validate_axioms: bool,
    pub work_dir: PathBuf,
}

impl ExportPlan {
    pub fn exporter_binary(&self) -> PathBuf {
        self.exporter_dir.join(".lake/build/bin/slim-export")
    }

    /// Full command line of the sandboxed exporter run. `in_dir` (host
    /// written, bound read-only) holds the module list; `out_dir` is a fresh
    /// empty directory and the only writable path inside the sandbox.
    pub fn exporter_command(&self, in_dir: &Path, out_dir: &Path, out: &Path, modules_file: &Path) -> Command {
        let exe = self.exporter_binary();
        let mut args: Vec<String> = vec![
            "--out".into(),
            out.to_string_lossy().into(),
            "--modules-file".into(),
            modules_file.to_string_lossy().into(),
            "--library".into(),
            self.library.clone(),
            "--library-types".into(),
            self.library_types.clone(),
            "--pp".into(),
            self.pp.clone(),
        ];
        for sp in &self.search_path {
            args.push("--search-path".into());
            args.push(sp.to_string_lossy().into());
        }
        if self.validate_axioms {
            args.push("--validate-axioms".into());
        }
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
                c.args(["--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp"]);
                for ro in [&self.toolchain_dir, &self.exporter_dir, &self.project.root, &in_dir.to_path_buf()] {
                    c.arg("--ro-bind").arg(ro).arg(ro);
                }
                c.arg("--bind").arg(out_dir).arg(out_dir);
                c.args(["--clearenv", "--setenv", "PATH", "/usr/bin", "--setenv", "HOME", "/tmp", "--setenv", "LANG", "C.UTF-8"]);
                c.args(["--chdir", "/tmp", "--"]);
                c.arg(&exe);
                c.args(&args);
                c
            }
            Sandbox::None => {
                let mut c = process::with_cgroup(&self.limits, &exe, &[]);
                c.args(&args);
                c.env_clear();
                c.env("PATH", "/usr/bin:/bin");
                c.env("HOME", out_dir);
                c.current_dir(out_dir);
                c
            }
        }
    }

    pub fn describe(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("Semantic export of project `{}` at {}\n", self.project.name, self.project.root.display()));
        s.push_str(&format!(
            "  toolchain:   {} ({}){}\n",
            self.toolchain,
            self.toolchain_dir.display(),
            if self.tested { "" } else { "  [NOT IN TESTED LIST]" }
        ));
        s.push_str(&format!("  exporter:    slim-export {} with compat shim {}\n", env!("CARGO_PKG_VERSION"), self.compat.name()));
        s.push_str(&format!("               built into {}\n", self.exporter_dir.display()));
        s.push_str(&format!(
            "  input:       {} locally built modules from {} (existing artifacts; slim does not build)\n",
            self.modules.len(),
            self.build_dir.display()
        ));
        s.push_str(&format!("  library:     {} closure, types for {}\n", self.library, self.library_types));
        s.push_str(&format!(
            "  printing:    {}\n",
            if self.pp == "notation" {
                "notation — loads environment extensions and RUNS the imported modules' initializers and attribute handlers (project code) to print notation"
            } else {
                "raw — no code from the imported modules is run; types are printed without library notation"
            }
        ));
        s.push_str(&format!("  sandbox:     {}\n", self.sandbox.describe()));
        s.push_str(&format!("  limits:      {}\n", self.limits.describe()));
        s.push_str(&format!("  writes only: a fresh per-run directory under {}\n", self.work_dir.display()));
        s
    }
}

pub struct PlanOptions {
    pub toolchain: Option<String>,
    pub sandbox: String,
    pub trust_project: bool,
    pub allow_untested: bool,
    pub library: String,
    pub library_types: String,
    pub pp: String,
    pub validate_axioms: Option<bool>,
    pub limits: Limits,
}

impl Default for PlanOptions {
    fn default() -> Self {
        PlanOptions {
            toolchain: None,
            sandbox: "auto".into(),
            trust_project: false,
            allow_untested: false,
            library: "full".into(),
            library_types: "frontier".into(),
            pp: "notation".into(),
            validate_axioms: None,
            limits: Limits::default(),
        }
    }
}

pub fn plan(root: &Path, o: &PlanOptions) -> Result<ExportPlan> {
    let project = Project::open(root)?;
    let toolchain = o
        .toolchain
        .clone()
        .or_else(|| project.toolchain.clone())
        .context("project has no lean-toolchain file; pass --toolchain")?;
    let compat = Compat::for_toolchain(&toolchain)
        .with_context(|| format!("toolchain {toolchain} is not supported by slim-export (supported: Lean 4.16 or newer)"))?;
    let tested = TESTED_TOOLCHAINS.contains(&toolchain.as_str());
    if !tested && !o.allow_untested {
        bail!(
            "toolchain {toolchain} is not in the tested list ({}). Its compat shim would be {}. \
             Pass --allow-untested-toolchain to try anyway; results are then unverified",
            TESTED_TOOLCHAINS.join(", "),
            compat.name()
        );
    }
    let tc_dir = toolchain_dir(&toolchain);
    if !tc_dir.join("bin/lean").exists() {
        bail!(
            "toolchain {toolchain} is not installed at {}. slim never installs toolchains; \
             install it yourself (e.g. `elan toolchain install {toolchain}`) if you want a semantic export",
            tc_dir.display()
        );
    }
    let (build_dir, modules) = local_built_modules(&project.root)?;
    if modules.is_empty() {
        bail!("no built local modules found under {}", build_dir.display());
    }
    let mut search_path = vec![tc_dir.join("lib/lean"), build_dir.clone()];
    if let Ok(rd) = std::fs::read_dir(project.root.join(".lake/packages")) {
        let mut pkgs: Vec<PathBuf> = rd.flatten().map(|e| e.path().join(".lake/build/lib/lean")).filter(|p| p.is_dir()).collect();
        pkgs.sort();
        search_path.extend(pkgs);
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
            Sandbox::None => {
                if o.sandbox == "auto" && o.trust_project {
                    bail!(
                        "no usable sandbox (bubblewrap with user namespaces) was found. slim will not fall back to running \
                         project code unisolated; if you trust this project, pass --sandbox none --trust-project explicitly"
                    );
                }
                bail!("no usable sandbox (bubblewrap with user namespaces) was found; semantic export refused. Source-only browsing still works");
            }
        },
        other => bail!("--sandbox: expected auto|bwrap|none, got {other}"),
    };
    let cache = crate::paths::cache_dir();
    let exporter_dir = cache.join("exporter").join(format!("{}-{}", elan_dir_name(&toolchain), env!("CARGO_PKG_VERSION")));
    let work_dir = project.cache_dir().join("export-work");
    let validate_axioms = o.validate_axioms.unwrap_or(modules.len() <= 400);
    Ok(ExportPlan {
        project,
        toolchain,
        toolchain_dir: tc_dir,
        compat,
        tested,
        build_dir,
        modules,
        search_path,
        exporter_dir,
        sandbox,
        limits: o.limits.clone(),
        library: o.library.clone(),
        library_types: o.library_types.clone(),
        pp: o.pp.clone(),
        validate_axioms,
        work_dir,
    })
}

/// Builds slim's own exporter for the plan's toolchain (trusted code,
/// no project code involved). Cached by toolchain and slim version.
pub fn build_exporter(p: &ExportPlan, cancel: &AtomicBool, log: &dyn Fn(&str)) -> Result<()> {
    let dir = &p.exporter_dir;
    let main_src = EXPORTER_MAIN;
    let compat_src = p.compat.source();
    let stamp = sha256_hex(format!("{main_src}\n{compat_src}\n{EXPORTER_LAKEFILE}\n{}", p.toolchain).as_bytes());
    if p.exporter_binary().exists() && std::fs::read_to_string(dir.join(".slim-stamp")).ok().as_deref() == Some(&stamp) {
        return Ok(());
    }
    std::fs::create_dir_all(dir.join("SlimExport"))?;
    std::fs::write(dir.join("Main.lean"), main_src)?;
    std::fs::write(dir.join("SlimExport/Compat.lean"), compat_src)?;
    std::fs::write(dir.join("lakefile.toml"), EXPORTER_LAKEFILE)?;
    std::fs::write(dir.join("lean-toolchain"), format!("{}\n", p.toolchain))?;
    log(&format!("building slim-export with {} (one-time, cached)…", p.toolchain));
    let mut cmd = Command::new(p.toolchain_dir.join("bin/lake"));
    cmd.arg("build").current_dir(dir).env_clear();
    cmd.env("PATH", format!("{}:/usr/bin:/bin", p.toolchain_dir.join("bin").display()));
    cmd.env("HOME", std::env::var_os("HOME").unwrap_or_default());
    // Our own trusted code: time limits only (Lean's worker threads reserve
    // large virtual ranges, so an address-space limit would break the build).
    let limits = Limits {
        wall: Duration::from_secs(900),
        cpu_seconds: Some(3600),
        address_space_mib: None,
        memory_mib: None,
        merge_stdout: true,
        max_stderr_lines: 60,
        ..Limits::default()
    };
    let (st, tail) = process::run_bounded(&mut cmd, &limits, cancel)?;
    if !st.success() || !p.exporter_binary().exists() {
        bail!("building the exporter failed ({st}):\n{}", tail.join("\n"));
    }
    std::fs::write(dir.join(".slim-stamp"), stamp)?;
    Ok(())
}

#[derive(Debug, Default)]
pub struct ExportResult {
    pub snapshot: PathBuf,
    pub local_decls: u64,
    pub decls: u64,
    pub modules: u64,
    pub library_truncated: bool,
    pub elapsed: Duration,
    pub stderr_tail: Vec<String>,
}

/// Creates a fresh private directory (fails if anything already exists there).
fn fresh_private_dir(p: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new().mode(0o700).create(p).with_context(|| format!("creating {}", p.display()))?;
    let md = std::fs::symlink_metadata(p)?;
    if !md.is_dir() || md.permissions().mode() & 0o077 != 0 {
        bail!("{} is not a private directory", p.display());
    }
    Ok(())
}

/// Opens a file for reading only if it is a regular file (no symlinks).
fn open_regular_nofollow(p: &Path) -> Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let md = std::fs::symlink_metadata(p).with_context(|| format!("{} missing", p.display()))?;
    if !md.file_type().is_file() {
        bail!("{} is not a regular file; refusing to read it", p.display());
    }
    Ok(std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(p)?)
}

/// Holds an exclusive lock so that only one semantic extraction runs at a
/// time for this user (across slim processes).
pub struct ExportLock {
    _file: std::fs::File,
}

pub fn acquire_export_lock() -> Result<ExportLock> {
    use std::os::fd::AsRawFd;
    let dir = crate::paths::cache_dir();
    std::fs::create_dir_all(&dir)?;
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("semantic-export.lock"))?;
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        bail!("another semantic export is already running (one at a time); try again when it finishes");
    }
    Ok(ExportLock { _file: f })
}

/// Runs the exporter (sandboxed) and wraps its output into a snapshot at `out`.
pub fn run_export(p: &ExportPlan, out: &Path, cancel: &AtomicBool, log: &dyn Fn(&str)) -> Result<ExportResult> {
    use std::os::unix::fs::OpenOptionsExt;
    let _lock = acquire_export_lock()?;
    let t0 = std::time::Instant::now();
    std::fs::create_dir_all(&p.work_dir)?;
    let run_id = crate::db::new_generation();
    let in_dir = p.work_dir.join(format!("run-{run_id}-in"));
    let out_dir = p.work_dir.join(format!("run-{run_id}-out"));
    fresh_private_dir(&in_dir)?;
    fresh_private_dir(&out_dir)?;
    let cleanup = || {
        let _ = std::fs::remove_dir_all(&in_dir);
        let _ = std::fs::remove_dir_all(&out_dir);
    };
    let result = (|| -> Result<ExportResult> {
        let raw = out_dir.join("exporter-output.jsonl");
        let modules_file = in_dir.join("modules.txt");
        {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&modules_file)?;
            f.write_all(p.modules.iter().map(|(m, _)| m.clone() + "\n").collect::<String>().as_bytes())?;
        }
        // Fingerprints of local sources before the run.
        let canon_root = p.project.root.canonicalize()?;
        let hash_src = |src: &str| -> Option<String> {
            let path = crate::paths::resolve_in_canonical_root(&canon_root, src).ok()?;
            std::fs::read(path).ok().map(|b| sha256_hex(&b))
        };
        let before: Vec<Option<String>> = p.modules.iter().map(|(_, src)| hash_src(src)).collect();
        let mut cmd = p.exporter_command(&in_dir, &out_dir, &raw, &modules_file);
        cmd.stdout(std::process::Stdio::null());
        log("running slim-export in the sandbox…");
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let mut child = process::spawn(&mut cmd, &p.limits, Some(tx))?;
        let outcome = child.wait(cancel)?;
        for l in rx.try_iter() {
            log(&format!("  exporter: {l}"));
        }
        let tail = child.stderr_tail();
        match outcome {
            Outcome::Exited(st) if st.success() => {}
            Outcome::Exited(st) => bail!("exporter failed ({st}):\n{}", tail.join("\n")),
            Outcome::TimedOut => bail!("exporter timed out after {} s and was terminated", p.limits.wall.as_secs()),
            Outcome::Cancelled => bail!("export cancelled; the exporter was terminated"),
        }
        let file = open_regular_nofollow(&raw)?;
        let res = wrap_snapshot(p, file, out, &before, &canon_root)?;
        Ok(ExportResult { elapsed: t0.elapsed(), stderr_tail: tail, ..res })
    })();
    cleanup();
    result
}

fn wrap_snapshot(p: &ExportPlan, f: std::fs::File, out: &Path, before: &[Option<String>], canon_root: &Path) -> Result<ExportResult> {
    let mut r = BufReader::new(f);
    let mut first = String::new();
    r.read_line(&mut first)?;
    let eh: serde_json::Value = serde_json::from_str(&first).context("exporter header")?;
    if eh.get("record").and_then(|v| v.as_str()) != Some("exporter_header") {
        bail!("exporter output does not start with its header");
    }
    let lean_version = eh.get("lean_version").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if let Some((a, b, c)) = parse_lean_version(&p.toolchain) {
        if !lean_version.starts_with(&format!("{a}.{b}.{c}")) {
            bail!("exporter reports Lean {lean_version} but the toolchain is {}", p.toolchain);
        }
    }
    let local_paths: std::collections::HashMap<&str, &str> = p.modules.iter().map(|(m, s)| (m.as_str(), s.as_str())).collect();
    let tmp = out.with_extension("partial");
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut w = BufWriter::new(std::fs::File::create(&tmp)?);
    let header = Header {
        record: "header".into(),
        format: snapshot::FORMAT.into(),
        format_version: snapshot::FORMAT_VERSION,
        producer: Producer { name: "slim export".into(), version: env!("CARGO_PKG_VERSION").into() },
        exporter: Some(eh.clone()),
        lean: Some(LeanInfo {
            version: lean_version.clone(),
            githash: eh.get("lean_githash").and_then(|v| v.as_str()).unwrap_or("").into(),
            toolchain: p.toolchain.clone(),
        }),
        project: Some(ProjectInfo {
            name: p.project.name.clone(),
            root_hint: Some(p.project.root.to_string_lossy().into()),
            lean_toolchain: p.project.toolchain.clone(),
            lake_manifest_sha256: p.project.lake_manifest_sha256.clone(),
            revision: git_revision(&p.project.root),
        }),
        scope: Some(serde_json::json!({
            "local_modules": p.modules.len(),
            "library": p.library,
            "library_types": p.library_types,
            "pp": p.pp,
            "loads_extensions": eh.get("loads_extensions"),
            "olean_level": eh.get("olean_level"),
            "validate_axioms": p.validate_axioms,
            "compat": p.compat.name(),
        })),
        created_at: Some(utc_now()),
        build: Some(serde_json::json!({
            "artifacts": "present",
            "olean_dir": p.build_dir.strip_prefix(&p.project.root).map(|x| x.to_string_lossy().to_string()).unwrap_or_default(),
            "build_run_by_slim": false,
            "note": "slim read existing build artifacts; it did not run, observe or verify the build. \
                     The presence of .olean files is not a fresh kernel replay and says nothing about informal claims."
        })),
        isolation: Some(serde_json::json!({
            "sandbox": p.sandbox.describe(),
            "limits": p.limits.describe(),
        })),
        synthetic: false,
    };
    writeln!(w, "{}", snapshot::header_line(&header))?;
    let mut modules = 0u64;
    let mut decls = 0u64;
    let mut local_decls = 0u64;
    let mut footer_seen = None;
    let mut buf = String::new();
    let mut lineno = 1u64;
    loop {
        buf.clear();
        let n = r.by_ref().take(snapshot::limits::MAX_LINE_BYTES as u64 + 1).read_line(&mut buf)?;
        if n == 0 {
            break;
        }
        lineno += 1;
        if buf.len() > snapshot::limits::MAX_LINE_BYTES {
            bail!("exporter line {lineno} too long");
        }
        let l: Line = serde_json::from_str(buf.trim_end()).with_context(|| format!("exporter line {lineno}"))?;
        match l.record.as_str() {
            "module" => {
                let mut m = l.into_module()?;
                if m.local {
                    if let Some(src) = local_paths.get(m.name.as_str()) {
                        let path = crate::paths::resolve_in_canonical_root(canon_root, src)?;
                        let bytes = std::fs::read(&path)?;
                        m.path = Some(src.to_string());
                        m.sha256 = Some(sha256_hex(&bytes));
                        m.size = Some(bytes.len() as u64);
                        m.mtime_ns = mtime_ns(&path);
                        m.olean_mtime_ns = mtime_ns(&p.build_dir.join(src.trim_end_matches(".lean").to_string() + ".olean"));
                    }
                }
                writeln!(w, "{}", snapshot::module_line(&m))?;
                modules += 1;
            }
            "decl" => {
                let d = l.into_decl()?;
                if d.local {
                    local_decls += 1;
                }
                writeln!(w, "{}", snapshot::decl_line(&d))?;
                decls += 1;
            }
            "exporter_footer" => {
                let v: serde_json::Value = serde_json::from_str(buf.trim_end())?;
                footer_seen = Some(v);
            }
            other => bail!("unexpected exporter record {other:?}"),
        }
    }
    let Some(ef) = footer_seen else {
        bail!("exporter output has no footer (truncated); no snapshot written");
    };
    let truncated = ef.get("library_truncated").and_then(|v| v.as_bool()).unwrap_or(false);
    // Sources must not have changed while the export ran.
    for ((_, src), b) in p.modules.iter().zip(before) {
        let now = crate::paths::resolve_in_canonical_root(canon_root, src).ok().and_then(|x| std::fs::read(x).ok()).map(|x| sha256_hex(&x));
        if &now != b {
            bail!("{src} changed while the export was running; snapshot discarded");
        }
    }
    writeln!(w, "{}", snapshot::footer_line(&Footer { counts: Counts { modules, decls }, complete: !truncated }))?;
    w.flush()?;
    drop(w);
    std::fs::rename(&tmp, out)?;
    Ok(ExportResult {
        snapshot: out.to_path_buf(),
        local_decls,
        decls,
        modules,
        library_truncated: truncated,
        ..Default::default()
    })
}

fn confirm(prompt: &str) -> Result<bool> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    let mut s = String::new();
    std::io::stdin().read_line(&mut s)?;
    Ok(s.trim() == "yes")
}

pub fn cmd_export(args: &Args) -> Result<i32> {
    let root = PathBuf::from(args.positional.first().context("usage: slim export <PROJECT_DIR> [options]")?);
    let mut limits = Limits::default();
    if let Some(t) = args.num::<u64>("timeout")? {
        limits.wall = Duration::from_secs(t);
        limits.cpu_seconds = Some(t * 2);
    }
    if let Some(m) = args.num::<u64>("memory-mib")? {
        limits.memory_mib = Some(m);
        limits.address_space_mib = Some((m * 8).max(65536));
    }
    let o = PlanOptions {
        toolchain: args.get("toolchain").map(String::from),
        sandbox: args.get("sandbox").unwrap_or("auto").into(),
        trust_project: args.has("trust-project"),
        allow_untested: args.has("allow-untested-toolchain"),
        library: args.get("library").unwrap_or("full").into(),
        library_types: args.get("library-types").unwrap_or("frontier").into(),
        pp: match args.get("pp").unwrap_or("notation") {
            v @ ("raw" | "notation") => v.into(),
            v => bail!("--pp expects notation|raw, got {v}"),
        },
        validate_axioms: if args.has("validate-axioms") {
            Some(true)
        } else if args.has("no-validate-axioms") {
            Some(false)
        } else {
            None
        },
        limits,
    };
    let plan = plan(&root, &o)?;
    eprintln!("{}", plan.describe());
    // `--with-highlight`: the semantic-highlight indexing job as well, under
    // the same single confirmation (its plan is shown first).
    let hl_plan = if args.has("with-highlight") {
        let p = crate::highlight::indexer::plan(&root, &crate::highlight::cli::options(args)?)?;
        eprint!("{}", p.describe());
        Some(p)
    } else {
        None
    };
    let question = if hl_plan.is_some() { "Type `yes` to run this export and then the semantic-highlight indexing: " } else { "Type `yes` to run this export: " };
    if !args.has("yes") && !confirm(question)? {
        eprintln!("not confirmed; nothing was run");
        return Ok(3);
    }
    let cancel = AtomicBool::new(false);
    let log = |s: &str| eprintln!("{s}");
    build_exporter(&plan, &cancel, &log)?;
    let out = args.path("out").unwrap_or_else(|| plan.project.cache_dir().join("snapshot.jsonl"));
    let res = run_export(&plan, &out, &cancel, &log)?;
    println!(
        "snapshot {} written: {} declarations ({} local), {} modules{} in {:.1}s",
        res.snapshot.display(),
        res.decls,
        res.local_decls,
        res.modules,
        if res.library_truncated { ", library closure TRUNCATED" } else { "" },
        res.elapsed.as_secs_f64()
    );
    if args.has("import") {
        let pp = |phase: &str, _d: u64, _t: u64| eprintln!("  {phase}…");
        let opts = crate::db::import::ImportOptions {
            source_root: Some(plan.project.root.clone()),
            progress: Some(&pp),
            ..Default::default()
        };
        let s = crate::db::import::import_snapshot(&res.snapshot, &plan.project.semantic_index(), &opts)?;
        println!(
            "imported into {}: {} declarations, {} edges, {} axioms",
            plan.project.semantic_index().display(),
            s.decls,
            s.edges,
            s.axioms
        );
    }
    if let Some(p) = hl_plan {
        return crate::highlight::cli::run_confirmed(&p, args);
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolchain_names_and_compat() {
        assert_eq!(elan_dir_name("leanprover/lean4:v4.33.1"), "leanprover--lean4---v4.33.1");
        assert_eq!(Compat::for_toolchain("leanprover/lean4:v4.16.0"), Some(Compat::Legacy));
        assert_eq!(Compat::for_toolchain("leanprover/lean4:v4.21.0"), Some(Compat::Mid));
        assert_eq!(Compat::for_toolchain("leanprover/lean4:v4.35.0-rc3"), Some(Compat::Modern));
        assert_eq!(Compat::for_toolchain("leanprover/lean4:v4.9.0"), None);
        assert!(utc_now().ends_with('Z'));
        assert_eq!(utc_now().len(), 20);
    }

    #[test]
    fn work_dirs_refuse_planted_symlinks() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("victim.txt");
        std::fs::write(&target, "do not touch").unwrap();
        // A planted symlink where the run directory would go is refused.
        let planted = d.path().join("run-x-in");
        std::os::unix::fs::symlink(d.path(), &planted).unwrap();
        assert!(fresh_private_dir(&planted).is_err());
        let fresh = d.path().join("run-y-out");
        fresh_private_dir(&fresh).unwrap();
        // Output that is a symlink (to a host file) is not read back.
        let out = fresh.join("exporter-output.jsonl");
        std::os::unix::fs::symlink(&target, &out).unwrap();
        assert!(open_regular_nofollow(&out).is_err());
        std::fs::remove_file(&out).unwrap();
        std::fs::write(&out, "{}").unwrap();
        assert!(open_regular_nofollow(&out).is_ok());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "do not touch");
    }

    #[test]
    fn untested_or_missing_toolchains_are_refused() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("lean-toolchain"), "leanprover/lean4:v4.35.0-rc3\n").unwrap();
        let err = plan(d.path(), &PlanOptions::default()).err().unwrap().to_string();
        assert!(err.contains("not in the tested list"), "{err}");
        let o = PlanOptions { allow_untested: true, ..Default::default() };
        let err = plan(d.path(), &o).err().unwrap().to_string();
        assert!(err.contains("not installed") || err.contains("no build artifacts"), "{err}");
    }
}
