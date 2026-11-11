use anyhow::{bail, Context, Result};
use slim::cli::{Args, HELP};
use slim::db::{self, query};
use slim::paths::Project;
use slim::source::index::{self as sindex, ScanConfig};
use slim::{app, nameindex};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn main() {
    let _ = slim::PROCESS_START.set(Instant::now());
    let args = match Args::parse(std::env::args_os()) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n\n{HELP}");
            std::process::exit(2);
        }
    };
    if let Some(dir) = args.path("cache-dir") {
        std::env::set_var("SLIM_CACHE_DIR", dir);
    }
    let code = match run(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e:#}");
            1
        }
    };
    std::process::exit(code);
}

fn progress_printer() -> impl Fn(&str, u64, u64) + Sync {
    let last = std::sync::Mutex::new(Instant::now());
    move |phase: &str, done: u64, total: u64| {
        let mut l = last.lock().unwrap();
        if l.elapsed().as_millis() >= 500 {
            *l = Instant::now();
            if total > 0 {
                eprintln!("  {phase}: {:.0}%", done as f64 * 100.0 / total as f64);
            } else {
                eprintln!("  {phase}…");
            }
        }
    }
}

fn scan_config(args: &Args, project: &Project) -> Result<ScanConfig> {
    let mut cfg = ScanConfig::for_project(project);
    if let Some(t) = args.num::<usize>("threads")? {
        cfg.threads = t.clamp(1, 64);
    }
    Ok(cfg)
}

fn run(args: &Args) -> Result<i32> {
    if args.has("help") || args.has("h") || args.command.as_deref() == Some("help") {
        print!("{HELP}");
        return Ok(0);
    }
    if args.has("version") || args.command.as_deref() == Some("version") {
        println!("slim {}", env!("CARGO_PKG_VERSION"));
        return Ok(0);
    }
    match args.command.as_deref() {
        Some("scan") => cmd_scan(args),
        Some("info") => cmd_info(args),
        Some("search") => cmd_search(args),
        Some("skim") => cmd_skim(args),
        Some("imports") => cmd_imports(args),
        Some("import") => cmd_import(args),
        Some("export") => slim::export::cmd_export(args),
        Some("check-axioms") => cmd_check_axioms(args),
        Some("trail") => slim::userdata::cmd_trail(args),
        Some("map") => cmd_map(args),
        Some("bookmarks") => slim::userdata::cmd_bookmarks(args),
        Some("bench") => slim::bench::cmd_bench(args),
        Some("highlight") => slim::highlight::cli::cmd_highlight(args),
        Some("demo") => app::launch(app::Target::Demo, args),
        Some("open") | None => {
            if let Some(idx) = args.path("index") {
                return app::launch(app::Target::Index { index: idx, source_root: args.path("source-root") }, args);
            }
            let p = args.positional.first().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
            if args.command.is_none() && args.positional.is_empty() {
                // `slim` with no arguments: open the current directory if it
                // looks like a Lean project, otherwise show help.
                if !p.join("lean-toolchain").exists() && !p.join("lakefile.toml").exists() && !p.join("lakefile.lean").exists() {
                    print!("{HELP}");
                    return Ok(0);
                }
            }
            app::launch(app::Target::Project(p), args)
        }
        Some(other) => bail!("unknown command {other}"),
    }
}

/// `slim map init|check`: the only place a map file is ever written is
/// `init -o FILE`, and only when FILE does not exist.
fn cmd_map(args: &Args) -> Result<i32> {
    use slim::map::{self, MapLoad, Severity};
    let sub = args.positional.first().map(|s| s.as_str()).unwrap_or("");
    if !matches!(sub, "init" | "check") {
        bail!("usage: slim map init <PROJECT_DIR> [-o FILE] | slim map check <PROJECT_DIR> [--map FILE] [--fingerprints]");
    }
    let dir = args.positional.get(1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let project = Project::open(&dir)?;
    let (idx, what) = if project.semantic_index().exists() { (project.semantic_index(), "semantic snapshot") } else { (project.source_index(), "source-only index") };
    let conn = db::open_read(&idx).with_context(|| format!("no index for {} yet: run `slim scan {}` first (it only reads .lean files)", project.name, dir.display()))?;
    if sub == "init" {
        let mods: Vec<(String, u64)> = query::modules(&conn, true)?.into_iter().map(|m| (m.name, m.decl_count)).collect();
        let text = map::skeleton_toml(&project.name, &mods);
        match args.path("o").or(args.path("out")) {
            Some(p) => {
                use std::io::Write;
                let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&p).with_context(|| format!("{} exists or cannot be created; nothing was overwritten", p.display()))?;
                f.write_all(text.as_bytes())?;
                eprintln!("wrote a generated starting map to {} ({} modules read from the {what})", p.display(), mods.len());
            }
            None => print!("{text}"),
        }
        return Ok(0);
    }
    let load = match args.path("map") {
        Some(p) => map::load(&p),
        None => map::load_for_root(&project.root),
    };
    let m = match load {
        MapLoad::Absent { looked_at } => {
            println!("no project map at {}", looked_at.map(|p| p.display().to_string()).unwrap_or_default());
            return Ok(1);
        }
        MapLoad::Malformed { path, issues } => {
            println!("{}: cannot be used (left unchanged)", path.display());
            for i in issues {
                println!("  {}", i.line());
            }
            return Ok(1);
        }
        MapLoad::Loaded(m) => m,
    };
    let res = map::resolve(&m, &conn)?;
    println!("{}: schema {}, checked against the {what} of {}", m.path.display(), map::SCHEMA_VERSION, project.name);
    println!(
        "  {} chosen objectives, {} groups, {} curated landmarks, {} curated relations, {} arguments, {} shared trails",
        m.objectives.len(),
        m.groups.len(),
        m.curated_landmark_count(),
        m.relations.len(),
        m.arguments.len(),
        m.trails.len()
    );
    if let Some(n) = m.project_name.as_ref().filter(|n| **n != project.name) {
        println!("  warning: the map says project {n:?}, this project is {:?}", project.name);
    }
    for i in &m.issues {
        println!("  {}", i.line());
    }
    let un = res.unresolved(&m);
    for u in &un {
        println!("  unresolved: {} — {} (used in: {})", u.name, u.why, u.used_in.join(", "));
        if !u.suggestions.is_empty() {
            println!("    same last component (hints only, never applied): {}", u.suggestions.join(", "));
        }
    }
    let stale: Vec<&str> = m.landmarks.iter().filter(|l| res.is_stale(l)).map(|l| l.decl.as_str()).collect();
    for s in &stale {
        println!("  stale: the source file of {s} changed since its annotation recorded `source_sha256`");
    }
    if args.has("fingerprints") {
        println!("  fingerprints of annotated declarations' source files (paste as `source_sha256` to detect later changes):");
        for l in &m.landmarks {
            if let Some(h) = res.refs.get(&l.decl).and_then(|i| i.module_sha256.as_ref()) {
                println!("    {} = \"{}\"", l.decl, &h[..h.len().min(16)]);
            }
        }
    }
    let errors = m.issues.iter().filter(|i| i.severity == Severity::Error).count();
    println!("  {} error(s), {} warning(s), {} unresolved, {} stale", errors, m.issues.len() - errors, un.len(), stale.len());
    Ok(if errors > 0 { 1 } else { 0 })
}

fn project_arg(args: &Args) -> Result<Project> {
    let p = args.positional.first().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    Project::open(&p)
}

fn cmd_scan(args: &Args) -> Result<i32> {
    let project = project_arg(args)?;
    let cfg = scan_config(args, &project)?;
    let idx = project.source_index();
    let cancel = AtomicBool::new(false);
    let pp = progress_printer();
    eprintln!("project {} at {}", project.name, project.root.display());
    eprintln!("index   {}", idx.display());
    if idx.exists() && !args.has("force") {
        if let Some(conn) = db::open_read(&idx).ok().filter(sindex::scanner_current) {
            let t = Instant::now();
            let fr = sindex::check_freshness(&conn, &project.root, true, false, &cancel)?;
            drop(conn);
            println!(
                "freshness: {} files checked in {:.2}s ({} unchanged, {} touched, {} changed, {} deleted, {} added, {} MiB hashed)",
                fr.checked,
                t.elapsed().as_secs_f64(),
                fr.unchanged,
                fr.touched.len(),
                fr.changed.len(),
                fr.deleted.len(),
                fr.added.len(),
                fr.hashed_bytes >> 20
            );
            for e in &fr.env_changed {
                println!("  environment: {e}");
            }
            if fr.changed.is_empty() && fr.deleted.is_empty() && fr.added.is_empty() && fr.touched.is_empty() {
                println!("index is up to date");
                return Ok(0);
            }
            let s = sindex::apply_incremental(&idx, &project, &fr, &cfg, &cancel)?;
            println!("incremental update: {} files rescanned, {} candidates written, {:.2}s", s.files, s.decls, s.elapsed.as_secs_f64());
            return Ok(0);
        }
    }
    let s = sindex::build_source_index(&project, &idx, &cfg, &cancel, Some(&pp))?;
    let size = std::fs::metadata(&idx).map(|m| m.len()).unwrap_or(0);
    let names = std::fs::metadata(slim::db::import::names_path(&idx)).map(|m| m.len()).unwrap_or(0);
    println!(
        "source index built: {} files ({:.1} MiB), {} declaration candidates, {} files partially parsed, {} unreadable, {} ambiguous names",
        s.files,
        s.bytes as f64 / 1048576.0,
        s.decls,
        s.partial_files,
        s.error_files,
        s.ambiguous
    );
    println!(
        "elapsed {:.2}s with {} threads; index {:.1} MiB + name index {:.1} MiB",
        s.elapsed.as_secs_f64(),
        cfg.threads,
        size as f64 / 1048576.0,
        names as f64 / 1048576.0
    );
    for w in s.warnings.iter().take(10) {
        println!("  warning: {w}");
    }
    Ok(0)
}

/// Finds the index to use for a CLI command: `--index FILE`, else the
/// project's semantic index, else its source index.
fn resolve_index(args: &Args) -> Result<(PathBuf, Option<Project>)> {
    if let Some(i) = args.path("index") {
        return Ok((i, None));
    }
    let project = project_arg(args)?;
    let sem = project.semantic_index();
    if sem.exists() && !args.has("source") {
        return Ok((sem, Some(project)));
    }
    let src = project.source_index();
    if !src.exists() {
        bail!("no index for {} yet; run `slim scan {}` or open it once", project.name, project.root.display());
    }
    Ok((src, Some(project)))
}

fn load_names(conn: &rusqlite::Connection, idx: &Path) -> Result<nameindex::NameIndex> {
    let gen = db::get_meta(conn, "name_index_generation")?.unwrap_or_default();
    match nameindex::NameIndex::load(&db::import::names_path(idx), &gen) {
        Ok(n) => Ok(n),
        Err(_) => db::import::build_name_index(conn, &gen),
    }
}

fn cmd_info(args: &Args) -> Result<i32> {
    let (idx, project) = resolve_index(args)?;
    let conn = db::open_read(&idx)?;
    let info = query::index_info(&conn)?;
    println!("index: {}", idx.display());
    println!("mode: {}", info.mode);
    println!("declarations: {} ({} local), modules: {} ({} local), edges: {}", info.decls, info.local, info.modules, info.local_modules, info.edges);
    for (k, v) in &info.meta {
        if k == "name_index_generation" {
            continue;
        }
        println!("  {k}: {v}");
    }
    let root = project.as_ref().map(|p| p.root.clone()).or_else(|| info.get("source_root").map(PathBuf::from));
    if let Some(root) = root {
        if root.exists() {
            let cancel = AtomicBool::new(false);
            let fr = sindex::check_freshness(&conn, &root, info.mode == "source", false, &cancel)?;
            println!(
                "freshness vs {}: {} checked, {} changed, {} deleted, {} added, {} touched{}",
                root.display(),
                fr.checked,
                fr.changed.len(),
                fr.deleted.len(),
                fr.added.len(),
                fr.touched.len(),
                if fr.is_fresh() { " — FRESH" } else { " — STALE" }
            );
            for e in &fr.env_changed {
                println!("  environment changed: {e}");
            }
        }
    }
    // The highlight cache, if this project has one (read only).
    if let Some(p) = &project {
        if let Ok(Some(store)) = slim::highlight::store::Store::open_existing(&slim::highlight::store::Store::path_for_project(p)) {
            if let Ok(st) = store.stats() {
                println!(
                    "highlight cache: semantic tokens for {} files complete, {} partial, {} failed, {} skipped; syntax spans cached for {} files; {:.1} MiB \
                     (`slim highlight status` compares it with the project as it is now)",
                    st.complete,
                    st.partial,
                    st.failed,
                    st.skipped,
                    st.baseline_files,
                    st.file_bytes as f64 / 1048576.0
                );
            }
        }
    }
    Ok(0)
}

fn cmd_search(args: &Args) -> Result<i32> {
    let (idx, _p, words) = if args.get("index").is_some() {
        let (i, p) = resolve_index(args)?;
        (i, p, args.positional.join(" "))
    } else {
        let (i, p) = resolve_index(args)?;
        (i, p, args.positional[1..].join(" "))
    };
    let conn = db::open_read(&idx)?;
    let t = Instant::now();
    let ni = load_names(&conn, &idx)?;
    let load = t.elapsed();
    // `--namespace NS` or `--within FILES` (a dotted module-name prefix).
    let spec = match (args.get("namespace"), args.get("within")) {
        (Some(ns), _) => Some(nameindex::ScopeSpec { kind: nameindex::ScopeKind::Namespace, prefix: ns.trim_end_matches('.').to_string() }),
        (None, Some(w)) => Some(nameindex::ScopeSpec { kind: nameindex::ScopeKind::Modules, prefix: w.trim_end_matches(".lean").replace('/', ".") }),
        (None, None) => None,
    };
    let scope = match &spec {
        None => None,
        Some(s) if s.kind == nameindex::ScopeKind::Namespace => Some(nameindex::Scope::Namespace(s.prefix.clone())),
        Some(s) => {
            let ids: Vec<i64> = conn
                .prepare("SELECT id FROM modules WHERE name = ?1 OR (name >= ?1 || '.' AND name < ?1 || '/')")?
                .query_map([&s.prefix], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            if ids.is_empty() {
                bail!("no module is named {} or lies below it", s.prefix);
            }
            Some(nameindex::Scope::Modules(std::sync::Arc::new(nameindex::ModuleSet::from_ids(ids.into_iter().map(|i| i as u32)))))
        }
    };
    if let Some(s) = &spec {
        println!("within {}", s.label());
    }
    let params = nameindex::SearchParams {
        query: words.clone(),
        local_only: args.has("local"),
        show_generated: args.has("generated"),
        limit: args.num::<usize>("limit")?.unwrap_or(20),
        scope,
        ..Default::default()
    };
    let out = ni.search(&params, &|| false);
    let ids: Vec<i64> = out.hits.iter().map(|h| h.id as i64).collect();
    let rows = query::rows_by_ids(&conn, &ids)?;
    println!(
        "{} name matches for {:?} ({} entries indexed, loaded in {:.0} ms; search {:.1} ms{}{})",
        out.total,
        words,
        ni.len(),
        load.as_secs_f64() * 1000.0,
        out.elapsed.as_secs_f64() * 1000.0,
        if out.filtered_out > 0 { format!("; {} hidden by filters", out.filtered_out) } else { String::new() },
        if out.fuzzy_truncated { "; fuzzy stopped at time budget" } else { "" }
    );
    for (h, r) in out.hits.iter().zip(&rows) {
        println!("  {:<11} {} {}  [{}]", h.tier.label(), r.kind.short(), r.display, r.module.as_deref().unwrap_or("-"));
    }
    let t = Instant::now();
    let docs = query::text_search(&conn, &words, if spec.is_some() { 600 } else { 10 })?;
    let drows: Vec<query::DeclRow> = query::rows_by_ids(&conn, &docs)?
        .into_iter()
        .filter(|r| spec.as_ref().is_none_or(|s| s.accepts(&r.display, r.module.as_deref())))
        .take(10)
        .collect();
    println!("{} documentation/statement matches ({:.1} ms)", drows.len(), t.elapsed().as_secs_f64() * 1000.0);
    for r in drows {
        println!("  doc         {} {}", r.kind.short(), r.display);
    }
    Ok(0)
}

/// `slim skim`: statements without proofs, as text.
fn cmd_skim(args: &Args) -> Result<i32> {
    use slim::skim::{self, SkimItem, SkimScope};
    let (idx, project) = resolve_index(args)?;
    let conn = db::open_read(&idx)?;
    let mode = db::mode_of(&conn)?;
    let what = if args.get("index").is_some() { args.positional.first() } else { args.positional.get(1) };
    let scope = match (args.get("namespace"), args.get("group"), what) {
        (Some(ns), _, _) => SkimScope::Namespace(ns.trim_end_matches('.').to_string()),
        (None, Some(g), _) => SkimScope::Group(g.to_string()),
        (None, None, Some(w)) => {
            let name = w.trim_end_matches(".lean").trim_end_matches('/').replace('/', ".");
            let is_file: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM modules WHERE name = ?1 AND is_local = 1)", [&name], |r| r.get(0))?;
            let has_below: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM modules WHERE name >= ?1 || '.' AND name < ?1 || '/')", [&name], |r| r.get(0))?;
            if is_file && !has_below { SkimScope::Module(name) } else { SkimScope::Dir(name) }
        }
        (None, None, None) => bail!("usage: slim skim <PROJECT_DIR> <MODULE_OR_DIR> | --namespace NS | --group ID"),
    };
    let root = project.as_ref().map(|p| p.root.clone()).or_else(|| db::get_meta(&conn, "source_root").ok().flatten().map(PathBuf::from)).filter(|p| p.is_dir());
    let map = match (&scope, &root) {
        (SkimScope::Group(_), Some(r)) => match args.path("map").map(|p| slim::map::load(&p)).unwrap_or_else(|| slim::map::load_for_root(r)) {
            slim::map::MapLoad::Loaded(m) => Some(m),
            _ => bail!("--group needs a project map (lean-view.toml, or --map FILE)"),
        },
        _ => None,
    };
    let mut cache = slim::sourceview::SourceCache::default();
    let mut env = skim::Env { conn: &conn, mode, source_root: root.as_deref(), cache: &mut cache, map: map.as_deref(), show_generated: args.has("generated") };
    let t = Instant::now();
    let s = skim::skim(&mut env, &scope)?;
    println!("# {} — {} declarations in {} files, proofs left out ({:.1} ms)", s.title, s.decls, s.files, t.elapsed().as_secs_f64() * 1000.0);
    if mode == db::IndexMode::Source {
        println!("# statements are the header text as written (lexical scan, not elaborated)");
    }
    if let Some(st) = &s.stats {
        println!("# {} · {} lines, {} in theorem proofs · {} of {} documented", skim::counts_line(st, mode), st.lines, st.proof_lines, st.doc, st.named());
    }
    if let Some(w) = &s.truncated {
        println!("# NOT THE WHOLE SCOPE: {w}");
    }
    let clean = |t: &str| slim::text::sanitize(t, false).into_owned();
    for item in &s.items {
        match item {
            SkimItem::Header { title, sub, doc, warn } => {
                println!("\n## {}", clean(title));
                if !sub.is_empty() {
                    println!("   {}", clean(sub));
                }
                if let Some(d) = doc {
                    println!("   {}", clean(d));
                }
                if let Some(w) = warn {
                    println!("   !! {}", clean(w));
                }
            }
            SkimItem::Decl(d) => {
                for l in d.sig.lines() {
                    println!("{}", clean(l));
                }
                if d.sig_cut {
                    println!("    … (statement cut)");
                }
                let mut meta = Vec::new();
                if let Some(doc) = &d.doc {
                    meta.push(clean(doc));
                }
                if d.sorry {
                    meta.push("[sorry token]".into());
                }
                if d.folded > 0 {
                    meta.push(format!("[{} line{} folded]", d.folded, if d.folded == 1 { "" } else { "s" }));
                }
                if !meta.is_empty() {
                    println!("    -- {}", meta.join(" "));
                }
            }
            SkimItem::Note { text, .. } => println!("   ({})", clean(text)),
            SkimItem::Prose(p) => {
                for part in &p.parts {
                    if let Some((level, title, _)) = &part.heading {
                        println!("\n{} {}", "#".repeat(*level as usize + 1), clean(title));
                    }
                    if let Some(l) = &part.lead {
                        println!("   {}{}", clean(l), if part.lead_cut { " …" } else { "" });
                    }
                }
                if p.folded > 0 {
                    println!("   [{} lines folded]", p.folded);
                }
            }
            SkimItem::Scope { text } => println!("{}", clean(text)),
        }
    }
    Ok(0)
}

/// `slim imports`: the import order as text, or one import path.
fn cmd_imports(args: &Args) -> Result<i32> {
    use slim::imports::{ImportGraph, PathAnswer};
    let (idx, _project) = resolve_index(args)?;
    let conn = db::open_read(&idx)?;
    let g = ImportGraph::load(&conn)?;
    let rest: Vec<&String> = if args.get("index").is_some() { args.positional.iter().collect() } else { args.positional.iter().skip(1).collect() };
    if let [from, to] = rest[..] {
        return Ok(match g.explain(from, to) {
            PathAnswer::Path(p) => {
                println!("{} import lines from {} to {}:", p.len() - 1, p[0], p[p.len() - 1]);
                for (i, m) in p.iter().enumerate() {
                    println!("  {}{m}", if i == 0 { "" } else { "imports " });
                }
                0
            }
            PathAnswer::NoPath { from, to, reached } => {
                println!("{from} does not import {to}, directly or through other files (all {reached} modules it reaches were checked)");
                1
            }
            PathAnswer::Unknown { text, examples, count } => {
                if count == 0 {
                    println!("no module is named {text:?} or ends with it");
                } else {
                    println!("{count} modules end with {text:?}: {}", examples.join(", "));
                }
                2
            }
        });
    }
    if !rest.is_empty() {
        bail!("usage: slim imports <PROJECT_DIR> [FROM TO]");
    }
    let limit = args.num::<usize>("limit")?.unwrap_or(20);
    println!("import graph of {} (from import lines: build order, not mathematical dependency)", idx.display());
    println!(
        "{} local files, {} import lines between them, {} imports of external modules; deepest chain {}; read in {:.0} ms",
        g.local_count, g.local_edges, g.external_edges, g.max_depth, g.load_ms
    );
    if g.cyclic_count > 0 {
        println!("!! {} files are on or above an import cycle: their depth is a lower bound", g.cyclic_count);
    }
    println!("\nlayers (depth = longest chain of local imports below a file; bottom of the import order first):");
    let row = |d: u32, indent: usize| {
        let n = &g.dirs[d as usize];
        let range = if n.dmin == n.dmax { format!("{}", n.dmin) } else { format!("{}-{}", n.dmin, n.dmax) };
        println!("  {:indent$}depth {:<9} {:>7} files {:>8} decl  {}", "", range, n.files, n.decls, n.full, indent = indent);
    };
    for &r in &g.roots {
        row(r, 0);
        for &c in g.dirs[r as usize].children.iter().take(limit.max(1) * 3) {
            row(c, 2);
        }
        let more = g.dirs[r as usize].children.len().saturating_sub(limit.max(1) * 3);
        if more > 0 {
            println!("    … {more} more");
        }
    }
    println!("\nendpoints: {} files with declarations that no declaring file imports (top of the import order first)", g.endpoints.len());
    for &v in g.endpoints.iter().take(limit) {
        println!("  depth {:<4} {:>6} decl  {}", g.depth[v as usize], g.nodes[v as usize].decls, g.nodes[v as usize].name);
    }
    println!("\nhubs: the most imported local files");
    for &v in g.hubs.iter().take(limit) {
        println!("  imported by {:<6} depth {:<4} {}", g.local_importers[v as usize], g.depth[v as usize], g.nodes[v as usize].name);
    }
    println!("\nimport-only files (declare nothing): {}", g.import_only.len());
    for &v in g.import_only.iter().take(limit.min(10)) {
        println!("  imports {:<6} {}", g.imports_of(v).len(), g.nodes[v as usize].name);
    }
    Ok(0)
}

fn cmd_import(args: &Args) -> Result<i32> {
    let snap = PathBuf::from(args.positional.first().context("usage: slim import <SNAPSHOT> [--source-root DIR]")?);
    let source_root = args.path("source-root").map(|p| p.canonicalize()).transpose().context("--source-root")?;
    let idx = match args.path("index") {
        Some(i) => i,
        None => {
            let root = source_root.clone().context("--source-root DIR (the project checkout) or --index FILE is required")?;
            Project::open(&root)?.semantic_index()
        }
    };
    let pp = progress_printer();
    let cancel = AtomicBool::new(false);
    let opts = db::import::ImportOptions {
        allow_partial: args.has("allow-partial"),
        cancel: Some(&cancel),
        progress: Some(&pp),
        source_root,
        ..Default::default()
    };
    let s = db::import::import_snapshot(&snap, &idx, &opts)?;
    println!(
        "imported {} declarations ({} local, {} library, {} unresolved references kept as nodes), {} edges, {} axioms, {} modules in {:.2}s{}",
        s.decls,
        s.local,
        s.library,
        s.unresolved,
        s.edges,
        s.axioms,
        s.modules,
        s.elapsed.as_secs_f64(),
        if s.resumed { " (resumed)" } else { "" }
    );
    println!("snapshot sha256 {}", s.sha256);
    println!("index {}", idx.display());
    for w in &s.warnings {
        println!("  warning: {w}");
    }
    Ok(0)
}

fn cmd_check_axioms(args: &Args) -> Result<i32> {
    let (idx, _) = resolve_index(args)?;
    let conn = db::open_read(&idx)?;
    let r = slim::axioms_check::compare_with_lean(&conn)?;
    println!(
        "{} local declarations compared with Lean {}'s collectAxioms{}",
        r.checked,
        r.lean_version,
        if r.pre_428_semantics { " (pre-4.28 rule: axiom types not traversed)" } else { "" }
    );
    for m in &r.mismatches {
        println!("  MISMATCH {}: slim {:?} vs Lean {:?}", m.0, m.1, m.2);
    }
    for (n, extra) in &r.explained_by_rule_change {
        println!("  note {n}: slim additionally reports {extra:?}, reachable only through an axiom's type (Lean >= 4.28 reports these too)");
    }
    if r.checked == 0 {
        println!("no Lean axiom report in this index (export with --validate-axioms)");
        return Ok(1);
    }
    if r.mismatches.is_empty() {
        println!("all match (ignoring slim's coverage-gap marker)");
    }
    Ok(if r.mismatches.is_empty() { 0 } else { 1 })
}
