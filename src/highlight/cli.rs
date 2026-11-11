//! `slim highlight …`: semantic-highlight indexing and inspection.

use super::baseline::{lines, BaselineBuilder};
use super::env::{DepTracker, EnvInfo};
use super::indexer::{self, Control, Event, Options, Outcome};
use super::store::Store;
use super::textmate::scopes;
use super::theme::SyntaxTheme;
use super::Coverage;
use crate::cli::Args;
use anyhow::{bail, Context, Result};
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

/// Loads a syntax theme by bundled name or from a VS Code theme file.
pub fn load_theme(spec: &str) -> Result<SyntaxTheme> {
    if let Some(t) = SyntaxTheme::builtin(spec) {
        return Ok(t);
    }
    let p = Path::new(spec);
    if p.is_file() {
        return SyntaxTheme::from_file(p);
    }
    bail!("unknown syntax theme {spec:?}: expected one of {} or a VS Code theme .json file", SyntaxTheme::BUILTIN_NAMES.join(", "))
}

/// `highlight dump`: scope stacks (and, with a theme, resolved styles) per
/// line as JSON Lines, in the format of `scripts/textmate-reference.mjs`.
fn cmd_dump(args: &Args) -> Result<i32> {
    let mut files: Vec<String> = args.positional.iter().skip(1).cloned().collect();
    if let Some(list) = args.get("list") {
        files.extend(std::fs::read_to_string(list)?.lines().filter(|l| !l.is_empty()).map(String::from));
    }
    let theme = args.get("syntax-theme").map(load_theme).transpose()?;
    let mut b = BaselineBuilder::new();
    let out = std::io::stdout();
    let mut out = std::io::BufWriter::new(out.lock());
    let mut toks = Vec::new();
    let t0 = std::time::Instant::now();
    let (mut bytes, mut nlines) = (0u64, 0u64);
    for f in &files {
        let raw = std::fs::read(f).with_context(|| format!("reading {f}"))?;
        let (text, _) = crate::text::decode_lossy(&raw);
        bytes += text.len() as u64;
        b.reset();
        let mut jl = Vec::new();
        let mut js = Vec::new();
        for line in lines(&text) {
            nlines += 1;
            b.feed(line, &mut toks);
            jl.push(serde_json::Value::Array(toks.iter().map(|&(s, id)| serde_json::json!([s, scopes().display(id)])).collect()));
            if let Some(t) = &theme {
                js.push(serde_json::Value::Array(
                    toks.iter()
                        .map(|&(s, id)| {
                            let st = t.scope_style(&scopes().path(id));
                            let fg = st.fg.map(|c| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]));
                            serde_json::json!([s, fg, st.font])
                        })
                        .collect(),
                ));
            }
        }
        let mut o = serde_json::json!({ "path": f, "lines": jl });
        if theme.is_some() {
            o["styles"] = serde_json::Value::Array(js);
        }
        writeln!(out, "{o}")?;
    }
    if args.has("stats") {
        let s = t0.elapsed().as_secs_f64();
        eprintln!("{} files, {nlines} lines, {bytes} bytes in {s:.3} s ({:.1} MiB/s, {:.0} lines/s)", files.len(), bytes as f64 / 1048576.0 / s, nlines as f64 / s);
    }
    Ok(0)
}

pub fn options(args: &Args) -> Result<Options> {
    let mut o = Options::default();
    o.files = args.get_all("files");
    o.max_files = args.num::<usize>("max-files")?;
    if let Some(t) = args.num::<u64>("file-timeout")? {
        o.file_timeout = Duration::from_secs(t.max(1));
    }
    o.budget = args.num::<u64>("budget")?.map(Duration::from_secs);
    if let Some(m) = args.num::<u64>("memory-mib")? {
        o.memory_mib = m.max(256);
    }
    if let Some(s) = args.get("sandbox") {
        o.sandbox = s.to_string();
    }
    o.trust_project = args.has("trust-project");
    o.cgroup = match args.get("cgroup") {
        None | Some("scope") | Some("auto") => true,
        Some("none") | Some("off") => false,
        Some(x) => bail!("--cgroup: expected scope|none, got {x}"),
    };
    o.refresh = args.has("refresh");
    o.retry = args.has("retry");
    o.allow_untested = args.has("allow-untested-toolchain");
    if let Some(n) = args.num::<usize>("open-docs")? {
        o.open_docs = n.clamp(1, 4);
    }
    if let Some(m) = args.num::<u64>("max-file-mib")? {
        o.max_file_bytes = m.max(1) << 20;
    }
    o.lean_threads = args.num::<usize>("threads")?;
    Ok(o)
}

fn project_dir(args: &Args) -> Result<std::path::PathBuf> {
    match args.positional.get(1) {
        Some(p) => Ok(std::path::PathBuf::from(p)),
        None => bail!("highlight {}: expected a project directory", args.positional.first().map(|s| s.as_str()).unwrap_or("")),
    }
}

/// `highlight build`: the explicit semantic indexing job.
fn cmd_build(args: &Args) -> Result<i32> {
    let root = project_dir(args)?;
    let pl = indexer::plan(&root, &options(args)?)?;
    eprint!("{}", pl.describe());
    eprintln!("  command:     {:?}", pl.server_command());
    if args.has("dry-run") {
        return Ok(0);
    }
    if !args.has("yes") {
        eprint!("\nThis runs Lean on the project. Type yes to continue: ");
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if line.trim() != "yes" {
            eprintln!("not confirmed; nothing was run");
            return Ok(1);
        }
    }
    run_confirmed(&pl, args)
}

/// Runs an already confirmed plan, reporting to stderr (and, with `--json`,
/// a summary line on stdout). Returns the process exit code.
pub fn run_confirmed(pl: &indexer::Plan, args: &Args) -> Result<i32> {
    let ctl = std::sync::Arc::new(Control::default());
    // Ctrl-C and termination stop the job cleanly: the server tree is
    // shut down and finished files stay published.
    {
        use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
        let mut signals = signal_hook::iterator::Signals::new([SIGINT, SIGTERM, SIGHUP])?;
        let c = ctl.clone();
        std::thread::spawn(move || {
            if signals.forever().next().is_some() {
                c.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        });
    }
    let quiet = args.has("quiet");
    let mut last = Instant::now();
    let summary = indexer::run(pl, &ctl, &mut |e| match e {
        Event::Published { rel, status, detail, elapsed } => {
            if !quiet {
                eprintln!("  {:<8} {:>7.1}s  {rel}{}", status.as_str(), elapsed.as_secs_f64(), if status == Coverage::Complete { String::new() } else { format!("  — {detail}") });
            }
        }
        Event::Progress(p) => {
            if last.elapsed() >= Duration::from_secs(10) {
                last = Instant::now();
                let c = &p.counts;
                eprintln!(
                    "  … {}/{} done ({} complete, {} partial, {} failed, {} skipped, {} already current) · {:.0}s · {}{}",
                    c.done(),
                    c.total,
                    c.complete,
                    c.partial,
                    c.failed,
                    c.skipped,
                    c.fresh,
                    p.elapsed.as_secs_f64(),
                    p.tree_rss_kib.map(|k| format!("server tree {} MiB in {} processes", k / 1024, p.tree_procs)).unwrap_or_default(),
                    p.current.first().map(|f| format!(" · {f} ({}s)", p.current_secs)).unwrap_or_default(),
                );
            }
        }
    })?;
    let c = &summary.counts;
    eprintln!(
        "{}: {} files — {} complete, {} partial, {} failed, {} skipped, {} already current, {} not reached; {:.1}s; peak server tree {} MiB; \
         shutdown {} ms ({}), {} lingering process(es)",
        summary.outcome.as_str(),
        c.total,
        c.complete,
        c.partial,
        c.failed,
        c.skipped,
        c.fresh,
        summary.remaining(),
        summary.elapsed.as_secs_f64(),
        summary.peak_tree_rss_kib / 1024,
        summary.shutdown.elapsed.as_millis(),
        if summary.shutdown.clean { "clean" } else { "terminated" },
        summary.shutdown.lingering,
    );
    if let Outcome::ServerFailed(e) = &summary.outcome {
        eprintln!("server failure: {e}");
    }
    if args.has("json") {
        println!("{}", summary.to_json());
    }
    Ok(match summary.outcome {
        Outcome::Finished => 0,
        Outcome::BudgetExhausted => 3,
        Outcome::Cancelled => 130,
        Outcome::ServerFailed(_) => 1,
    })
}

/// `highlight status`: what the cache holds and how it relates to the
/// project now. Reads files and the cache; runs nothing.
fn cmd_status(args: &Args) -> Result<i32> {
    let root = project_dir(args)?;
    let project = crate::paths::Project::open(&root)?;
    let path = Store::path_for_project(&project);
    let Some(store) = Store::open_existing(&path)? else {
        println!("no highlight cache for {} (expected at {})", project.root.display(), path.display());
        return Ok(0);
    };
    let st = store.stats()?;
    println!("highlight cache: {}", path.display());
    println!("  size on disk: {:.1} MiB", st.file_bytes as f64 / 1048576.0);
    println!("  baseline (syntax) spans: {} files, {:.1} MiB", st.baseline_files, st.baseline_bytes as f64 / 1048576.0);
    println!(
        "  semantic records: {} complete, {} partial, {} failed, {} skipped; {} tokens, {:.1} MiB",
        st.complete,
        st.partial,
        st.failed,
        st.skipped,
        st.semantic_tokens,
        st.semantic_bytes as f64 / 1048576.0
    );
    if let Some((started, finished, outcome, summary)) = store.last_run()? {
        println!(
            "  last run: started {started}, {}{}",
            match (&finished, &outcome) {
                (Some(f), Some(o)) => format!("{o} at {f}"),
                _ => "did not finish (interrupted); finished files were kept".into(),
            },
            summary.map(|s| format!("\n            {s}")).unwrap_or_default()
        );
    }
    // Freshness of every record against the files as they are now.
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let (found, _) = crate::source::index::discover(&project.root, &cancel)?;
    let now_env = EnvInfo::current(&project.root);
    let mut tracker = DepTracker::new(&project.root, project.toolchain.as_deref());
    let metas: std::collections::HashMap<String, _> = store.all_semantic_meta()?.into_iter().map(|m| (m.path.clone(), m)).collect();
    let (mut current, mut src_changed, mut env_changed, mut deps_changed, mut none, mut unusable) = (0, 0, 0, 0, 0, 0);
    let (mut local_bytes, mut covered_bytes) = (0u64, 0u64);
    let mut env_reasons: Vec<String> = Vec::new();
    for f in &found {
        local_bytes += f.size;
        let Some(m) = metas.get(&f.rel) else {
            none += 1;
            continue;
        };
        let Ok(bytes) = std::fs::read(project.root.join(&f.rel)) else { continue };
        if crate::paths::sha256_hex(&bytes) != m.source_sha256 {
            src_changed += 1;
            continue;
        }
        if !matches!(m.status, Coverage::Complete | Coverage::Partial) {
            unusable += 1;
            continue;
        }
        if now_env.fingerprint() != m.env_fingerprint {
            env_changed += 1;
            if env_reasons.is_empty() {
                if let Some(then) = store.get_env(&m.env_fingerprint)? {
                    env_reasons = now_env.changes_since(&then.info);
                }
            }
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        if tracker.closure(&f.rel, Some(&text)).digest != m.deps_digest {
            deps_changed += 1;
            continue;
        }
        current += 1;
        if m.status == Coverage::Complete {
            covered_bytes += f.size;
        }
    }
    println!("project now: {} local Lean files, {:.1} MiB (imported dependencies are not indexed)", found.len(), local_bytes as f64 / 1048576.0);
    println!("  current (source, environment and imports all as recorded): {current}");
    println!("    of which complete coverage: {:.1} MiB of source ({:.1}% of local bytes)", covered_bytes as f64 / 1048576.0, if local_bytes > 0 { covered_bytes as f64 * 100.0 / local_bytes as f64 } else { 0.0 });
    println!("  source changed since extraction (tokens not applied): {src_changed}");
    println!("  environment changed (stale meaning): {env_changed}{}", if env_reasons.is_empty() { String::new() } else { format!(" — {}", env_reasons.join("; ")) });
    println!("  an imported source changed (stale meaning): {deps_changed}");
    println!("  failed or skipped records: {unusable}");
    println!("  no record: {none}");
    if args.has("failures") {
        for m in metas.values().filter(|m| !matches!(m.status, Coverage::Complete)) {
            println!("  {:<8} {} — {}", m.status.as_str(), m.path, m.detail);
        }
    }
    Ok(0)
}

/// `highlight clean`: cache maintenance; touches only slim's own cache.
fn cmd_clean(args: &Args) -> Result<i32> {
    let root = project_dir(args)?;
    let project = crate::paths::Project::open(&root)?;
    let path = Store::path_for_project(&project);
    if !path.exists() {
        println!("nothing to clean: no highlight cache at {}", path.display());
        return Ok(0);
    }
    let (b, s) = match (args.has("baseline"), args.has("semantic")) {
        (false, false) => (true, true),
        x => x,
    };
    if !args.has("yes") {
        eprint!("Delete {} from {}? Type yes: ", match (b, s) { (true, true) => "all cached highlight data", (true, false) => "cached syntax spans", _ => "cached semantic tokens" }, path.display());
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if line.trim() != "yes" {
            eprintln!("not confirmed; nothing was deleted");
            return Ok(1);
        }
    }
    match Store::open_existing(&path) {
        Ok(Some(store)) if !(b && s) => {
            let n = store.clear(b, s)?;
            println!("removed {n} cached entries");
        }
        _ => {
            // Everything, or a cache this version cannot read: remove the files.
            for suffix in ["", "-wal", "-shm"] {
                let p = std::path::PathBuf::from(format!("{}{suffix}", path.display()));
                if p.exists() {
                    std::fs::remove_file(&p).with_context(|| format!("removing {}", p.display()))?;
                }
            }
            println!("removed {}", path.display());
        }
    }
    Ok(0)
}

pub const HELP: &str = "slim highlight — syntax and semantic highlighting data

  slim highlight build <PROJECT_DIR> [--yes] [--dry-run] [--files PATH]... [--max-files N]
        [--file-timeout S] [--budget S] [--memory-mib M] [--cgroup scope|none] [--open-docs N]
        [--threads N] [--refresh] [--retry] [--sandbox auto|bwrap|none [--trust-project]]
        [--allow-untested-toolchain] [--max-file-mib M] [--quiet] [--json]
      EXPLICIT: runs the project's pinned Lean language server on its files to extract semantic
      tokens into slim's cache. Prints what will run and asks to confirm. Never builds,
      fetches or installs. Resumable: finished files are kept; rerun to continue.
  slim highlight status <PROJECT_DIR> [--failures]
      Cache contents and their freshness against the project as it is now (runs nothing).
  slim highlight clean <PROJECT_DIR> [--baseline] [--semantic] [--yes]
      Delete cached highlight data (only slim's own cache).
  slim highlight dump [--syntax-theme NAME|FILE] [--stats] FILE... | --list FILES.txt
      Scope stacks per line as JSON Lines (for comparison with VS Code's tokenizer).
  slim highlight bench <PROJECT_DIR> [--report FILE] [--files PATH]...
      Headless measurements of highlighting on a project (see docs/bench/highlight.md).
";

pub fn cmd_highlight(args: &Args) -> Result<i32> {
    match args.positional.first().map(|s| s.as_str()) {
        Some("dump") => cmd_dump(args),
        Some("build") => cmd_build(args),
        Some("status") => cmd_status(args),
        Some("clean") => cmd_clean(args),
        Some("bench") => super::bench::cmd_bench(args),
        _ => {
            print!("{HELP}");
            Ok(2)
        }
    }
}
