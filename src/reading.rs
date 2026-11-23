//! Reading aids built on the reference graph ([`crate::graph`]): which
//! declarations a project ends in, why a declaration is there, a table of
//! contents that follows the logic, and the ledger of named statements.
//! Also their command-line forms.

use crate::cli::Args;
use crate::db::query::{self, DeclRow};
use crate::graph::{self, nf, Cone, Follow, Graph, BODY, NODE, STMT};
use crate::map::{MapLoad, ProjectMap};
use crate::model::DeclKind;
use crate::paths::Project;
use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// At most this many declarations are taken as a project's endpoints
/// without anyone having chosen them.
pub const MAX_AUTO_ROOTS: usize = 32;

/// An index opened for a command.
pub struct Opened {
    pub index: PathBuf,
    pub project: Option<Project>,
    pub conn: Connection,
    pub root: Option<PathBuf>,
    pub generation: String,
}

/// The index a command's first argument (a project directory) or
/// `--index FILE` names.
pub fn open(args: &Args, dir: Option<&str>) -> Result<Opened> {
    let (index, project) = match args.path("index") {
        Some(i) => (i, None),
        None => {
            let p = Project::open(Path::new(dir.unwrap_or(".")))?;
            let sem = p.semantic_index();
            let idx = if sem.exists() && !args.has("source") { sem } else { p.source_index() };
            if !idx.exists() {
                bail!("no index for {} yet; run `slim scan {}` or open it once", p.name, p.root.display());
            }
            (idx, Some(p))
        }
    };
    let conn = crate::db::open_read(&index)?;
    let generation = crate::db::get_meta(&conn, "name_index_generation")?.unwrap_or_default();
    let root = match &project {
        Some(p) => Some(p.root.clone()),
        None => args.path("source-root").or_else(|| crate::db::get_meta(&conn, "source_root").ok().flatten().map(PathBuf::from)),
    };
    Ok(Opened { index, project, conn, root, generation })
}

impl Opened {
    pub fn graph(&self) -> Result<Graph> {
        match Graph::load(&self.index, &self.generation)? {
            Some(g) => Ok(g),
            None if graph::graph_path(&self.index).exists() => bail!("the reference graph was built for an earlier state of the index; run `slim graph build` again"),
            None => bail!("no reference graph yet; run `slim graph build <PROJECT_DIR>` (it reads files and build artifacts, and runs nothing)"),
        }
    }
    pub fn map(&self, args: &Args) -> Option<std::sync::Arc<ProjectMap>> {
        let load = match (args.path("map"), &self.root) {
            (Some(p), _) => crate::map::load(&p),
            (None, Some(r)) => crate::map::load_for_root(r),
            _ => return None,
        };
        match load {
            MapLoad::Loaded(m) => Some(m),
            _ => None,
        }
    }
}

/// The roots a walk starts from, and who chose them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Roots {
    pub nodes: Vec<u32>,
    pub how: String,
}

/// Explicit names; else the objectives of the project map; else the
/// theorems of the files nothing imports, when they are few.
pub fn pick_roots(conn: &Connection, g: &Graph, map: Option<&ProjectMap>, explicit: &[String]) -> Result<Roots> {
    if !explicit.is_empty() {
        let mut nodes = Vec::new();
        for n in explicit {
            let id = find_decl(conn, n)?;
            nodes.push(g.node(id).with_context(|| format!("{n} is not in the reference graph"))?);
        }
        return Ok(Roots { nodes, how: "the goal".into() });
    }
    if let Some(m) = map {
        let nodes: Vec<u32> = m.objectives.iter().filter_map(|o| query::decl_by_name(conn, &o.decl).ok().flatten()).filter_map(|id| g.node(id)).collect();
        if !nodes.is_empty() {
            return Ok(Roots { nodes, how: "the objectives of the project map".into() });
        }
    }
    match graph::import_endpoints(conn, MAX_AUTO_ROOTS)? {
        Some(ids) => {
            let nodes: Vec<u32> = ids.into_iter().filter_map(|id| g.node(id)).collect();
            if nodes.is_empty() {
                Ok(Roots::default())
            } else {
                Ok(Roots { nodes, how: "the theorems of the files that nothing imports".into() })
            }
        }
        None => Ok(Roots { nodes: Vec::new(), how: String::new() }),
    }
}

/// The Lake packages that lie below the opened root (their directories),
/// when the root is not one itself: a repository of several projects.
pub fn packages_below(conn: &Connection) -> Vec<String> {
    let Ok(mut st) = conn.prepare("SELECT DISTINCT pkg FROM module_lake WHERE pkg != '' ORDER BY pkg LIMIT 24") else { return Vec::new() };
    let rows = st.query_map([], |r| r.get::<_, String>(0)).map(|it| it.flatten().collect::<Vec<_>>());
    rows.unwrap_or_default().into_iter().map(|p| p.trim_end_matches('/').to_string()).collect()
}

/// What to say when a project has no endpoints of its own.
pub fn no_endpoints_hint(conn: &Connection) -> String {
    let pkgs = packages_below(conn);
    if pkgs.len() > 1 {
        format!(
            "No endpoints: this directory holds {} Lake packages ({}{}). Open one of them and it has its own; or pin a goal (*) on a main theorem.",
            pkgs.len(),
            pkgs.iter().take(6).cloned().collect::<Vec<_>>().join(", "),
            if pkgs.len() > 6 { ", …" } else { "" }
        )
    } else {
        "No endpoints: too many files end the import order to guess what the project is for. Pin a goal (*) on a main theorem, or list objectives in the project map.".into()
    }
}

/// A declaration by its full name, or by the end of its name when that
/// names exactly one.
pub fn find_decl(conn: &Connection, name: &str) -> Result<i64> {
    if let Some(id) = query::decl_by_name(conn, name)? {
        return Ok(id);
    }
    // Distinct names: one name declared twice (a proof and its
    // restatement with `sorry`) is one name, and means the proof.
    let pat = format!("%.{}", name.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
    let mut st = conn.prepare("SELECT DISTINCT name FROM decls WHERE name LIKE ?1 ESCAPE '\\' LIMIT 6")?;
    let hits: Vec<String> = st.query_map([pat], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    match hits.as_slice() {
        [] => bail!("no declaration is named {name}"),
        [one] => query::decl_by_name(conn, one)?.with_context(|| format!("no declaration is named {name}")),
        many => bail!("{name} is the end of several names: {}", many.join(", ")),
    }
}

/// Rows of some nodes, by node.
pub fn rows(conn: &Connection, g: &Graph, nodes: &[u32]) -> Result<HashMap<u32, DeclRow>> {
    let ids: Vec<i64> = nodes.iter().map(|&v| g.id(v)).collect();
    let mut out = HashMap::with_capacity(nodes.len());
    for r in query::rows_by_ids(conn, &ids)? {
        if let Some(v) = g.node(r.id) {
            out.insert(v, r);
        }
    }
    Ok(out)
}

/// A count with thin separators: `1 133 440`.
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push('\u{202f}');
        }
        out.push(c);
    }
    out
}

/// A relay theorem followed to where its proof really starts: the chain of
/// names that only hand over (`a := b`, `b := c`), ending at the first
/// declaration that does something. Bounded; a cycle stops it.
pub fn relay_chain(g: &Graph, v: u32) -> Vec<u32> {
    let mut out = vec![v];
    let mut at = v;
    while let Some(t) = g.relay_target(at) {
        if out.contains(&t) || out.len() > 32 {
            break;
        }
        out.push(t);
        at = t;
    }
    out
}

// ---------------------------------------------------------------------------
// The ledger of named statements.

#[derive(Clone, Debug, PartialEq)]
pub struct LedgerRow {
    pub statement: u32,
    /// The theorems whose type is this statement.
    pub provers: Vec<u32>,
    /// Declarations that refer to the statement, the provers left out.
    pub users: u32,
    /// Steps from the nearest root, when a root reaches it.
    pub depth: Option<u16>,
}

/// Named statements (definitions of sort `Prop`) with the theorems that
/// prove them. `only`: the nodes to consider (a namespace or directory),
/// or all. Proved and used statements first, by how many use them.
pub fn ledger(g: &Graph, cone: Option<&Cone>, only: Option<&dyn Fn(u32) -> bool>) -> Vec<LedgerRow> {
    let mut out = Vec::new();
    for v in 0..g.len() as u32 {
        if !g.has(v, nf::PROP) || only.is_some_and(|f| !f(v)) {
            continue;
        }
        // The theorem that does the proving first: not one that hands
        // over to another, and the one most referred to.
        let mut provers = g.provers(v);
        provers.sort_by_key(|&p| (g.has(p, nf::SORRY), g.has(p, nf::RELAY), std::cmp::Reverse(g.users(p).len()), p));
        let users = g.users(v).iter().map(|e| e & NODE).filter(|u| !provers.contains(u)).count() as u32;
        let depth = cone.and_then(|c| c.contains(v).then(|| c.depth[v as usize]));
        out.push(LedgerRow { statement: v, provers, users, depth });
    }
    out.sort_by(|a, b| {
        (b.depth.is_some(), !b.provers.is_empty(), b.users).cmp(&(a.depth.is_some(), !a.provers.is_empty(), a.users)).then(a.statement.cmp(&b.statement))
    });
    out
}

// ---------------------------------------------------------------------------
// Commands.

fn progress_printer() -> impl Fn(&str, u64, u64) + Sync {
    let last = std::sync::Mutex::new(std::time::Instant::now());
    move |phase: &str, done: u64, total: u64| {
        let mut l = last.lock().unwrap();
        if l.elapsed().as_millis() >= 500 {
            *l = std::time::Instant::now();
            if total > 1 {
                eprintln!("  {phase}: {:.0}%", done as f64 * 100.0 / total as f64);
            } else {
                eprintln!("  {phase}…");
            }
        }
    }
}

/// One reason files have no Lean references, or no edges at all.
fn fallback_line(why: &str, n: u64) -> String {
    match why.strip_prefix("left out: ") {
        Some(w) => format!("{n} files left out: {w}"),
        None => format!("{n} files read lexically: {why}"),
    }
}

fn kind_counts(conn: &Connection, g: &Graph, cone: &Cone) -> Result<Vec<(DeclKind, u64, u64)>> {
    let mut st = conn.prepare("SELECT id, kind FROM decls")?;
    let mut by: HashMap<u8, (u64, u64)> = HashMap::new();
    for r in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, u8>(1)?)))? {
        let (id, k) = r?;
        let Some(v) = g.node(id) else { continue };
        let e = by.entry(k).or_default();
        e.1 += 1;
        e.0 += cone.contains(v) as u64;
    }
    let mut v: Vec<(DeclKind, u64, u64)> = by.into_iter().map(|(k, (a, b))| (DeclKind::from_u8(k), a, b)).collect();
    v.sort_by(|a, b| b.2.cmp(&a.2));
    Ok(v)
}

fn print_roots(conn: &Connection, g: &Graph, roots: &Roots) -> Result<()> {
    if roots.nodes.is_empty() {
        let pkgs = packages_below(conn);
        if pkgs.len() > 1 {
            println!("endpoints: none chosen: this directory holds {} Lake packages ({}); run the command on one of them, or name an endpoint with --goal NAME", pkgs.len(), pkgs.join(", "));
        } else {
            println!("endpoints: none chosen (too many files end the import order to guess); name one with --goal NAME, or list objectives in the project map");
        }
        return Ok(());
    }
    let names = rows(conn, g, &roots.nodes)?;
    println!("endpoints ({}): {}", roots.how, roots.nodes.iter().filter_map(|v| names.get(v)).map(|r| r.display.as_str()).collect::<Vec<_>>().join(", "));
    Ok(())
}

/// `slim graph build|status`.
pub fn cmd_graph(args: &Args) -> Result<i32> {
    let sub = args.positional.first().map(|s| s.as_str()).unwrap_or("");
    if !matches!(sub, "build" | "status" | "show") {
        bail!("usage: slim graph build <PROJECT_DIR> [--artifacts DIR] [--lexical] [--threads N] | slim graph status <PROJECT_DIR> | slim graph show <PROJECT_DIR> <NAME>");
    }
    // Building the graph of a project that was never opened: scan it first
    // (the same reading of `.lean` files as text that opening it does).
    if sub == "build" && args.path("index").is_none() {
        let p = Project::open(Path::new(args.positional.get(1).map(|s| s.as_str()).unwrap_or(".")))?;
        if !p.semantic_index().exists() && !p.source_index().exists() {
            let cfg = crate::source::index::ScanConfig::for_project(&p);
            let pp = progress_printer();
            let s = crate::source::index::build_source_index(&p, &p.source_index(), &cfg, &AtomicBool::new(false), Some(&pp))?;
            eprintln!("source index built: {} files, {} declaration candidates, {:.2} s", s.files, s.decls, s.elapsed.as_secs_f64());
        }
    }
    let o = open(args, args.positional.get(1).map(|s| s.as_str()))?;
    let goals = args.get_all("goal");
    if sub == "show" {
        let name = args.positional.get(2).context("usage: slim graph show <PROJECT_DIR> <NAME>")?;
        let g = o.graph()?;
        let v = g.node(find_decl(&o.conn, name)?).context("not in the reference graph")?;
        let limit = args.num::<usize>("limit")?.unwrap_or(60);
        for (title, list) in [("refers to", g.uses(v)), ("referred to by", g.users(v))] {
            println!("{title} ({}, {}):", list.len(), g.stats.label());
            let nodes: Vec<u32> = list.iter().take(limit).map(|e| e & NODE).collect();
            let names = rows(&o.conn, &g, &nodes)?;
            for e in list.iter().take(limit) {
                let part = match (e & STMT != 0, e & BODY != 0) {
                    (true, true) => "S+B",
                    (true, false) => "S  ",
                    _ => "  B",
                };
                if let Some(r) = names.get(&(e & NODE)) {
                    println!("  {part} {} {}", r.kind.short(), r.display);
                }
            }
            if list.len() > limit {
                println!("  … {} more", list.len() - limit);
            }
        }
        return Ok(0);
    }
    if sub == "status" {
        let g = o.graph()?;
        println!("graph: {}", graph::graph_path(&o.index).display());
        println!("  {}", g.stats.describe());
        if let Some(a) = &g.stats.artifacts {
            println!("  artifacts read from {a}");
        }
        for (why, n) in &g.stats.fallbacks {
            println!("  {}", fallback_line(why, *n));
        }
        if g.stats.unresolved > 0 {
            println!("  {} references name something the index does not hold (generated names) and were left out", g.stats.unresolved);
        }
        let roots = pick_roots(&o.conn, &g, o.map(args).as_deref(), &goals)?;
        print_roots(&o.conn, &g, &roots)?;
        if !roots.nodes.is_empty() {
            let c = g.cone(&roots.nodes, Follow::All);
            println!("reached from the endpoints: {} of {} declarations ({:.0} ms)", c.reached, g.len(), c.ms);
            for (k, a, b) in kind_counts(&o.conn, &g, &c)? {
                println!("  {:<11} {:>9} of {:>9}", k.as_str(), a, b);
            }
        }
        return Ok(0);
    }
    // An artifact directory given here is remembered beside the index.
    let recorded = crate::leanrefs::artifacts_file(&o.index);
    let artifacts = match args.path("artifacts") {
        Some(p) => {
            let p = p.canonicalize().with_context(|| format!("artifact directory {}", p.display()))?;
            if crate::leanrefs::external_lib_dir(&p).is_none() {
                bail!("{} has no lib/lean directory: it is not laid out like a Lake build directory", p.display());
            }
            std::fs::write(&recorded, format!("{}\n", p.display())).with_context(|| format!("writing {}", recorded.display()))?;
            eprintln!("artifact directory recorded in {}", recorded.display());
            Some(p)
        }
        None => crate::leanrefs::artifacts_for_index(&o.index),
    };
    let threads = args.num::<usize>("threads")?.unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(1, 4));
    let opts = graph::BuildOptions { threads, lexical_only: args.has("lexical"), artifacts };
    let pp = progress_printer();
    let g = graph::build(&o.index, o.root.as_deref(), &opts, &AtomicBool::new(false), Some(&pp))?;
    println!("{}", g.stats.describe());
    for (why, n) in &g.stats.fallbacks {
        println!("  {}", fallback_line(why, *n));
    }
    println!("built in {:.1} s with {threads} threads; {} ({:.1} MiB)", g.stats.ms as f64 / 1000.0, graph::graph_path(&o.index).display(), std::fs::metadata(graph::graph_path(&o.index)).map(|m| m.len()).unwrap_or(0) as f64 / (1 << 20) as f64);
    let roots = pick_roots(&o.conn, &g, o.map(args).as_deref(), &goals)?;
    print_roots(&o.conn, &g, &roots)?;
    if !roots.nodes.is_empty() {
        let c = g.cone(&roots.nodes, Follow::All);
        println!("reached from the endpoints: {} of {} declarations ({:.0} ms)", c.reached, g.len(), c.ms);
    }
    Ok(0)
}

fn where_of(conn: &Connection, id: i64) -> String {
    conn.query_row("SELECT m.path, d.sel_line FROM decls d JOIN modules m ON m.id = d.module_id WHERE d.id = ?1", [id], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<i64>>(1)?)))
        .ok()
        .and_then(|(p, l)| Some(format!("{}:{}", p?, l.unwrap_or(1))))
        .unwrap_or_default()
}

/// `slim why DIR NAME`: the chain from an endpoint down to a declaration.
pub fn cmd_why(args: &Args) -> Result<i32> {
    let o = open(args, args.positional.first().map(|s| s.as_str()))?;
    let name = args.positional.get(if args.get("index").is_some() { 0 } else { 1 }).context("usage: slim why <PROJECT_DIR> <NAME> [--goal NAME]")?;
    let g = o.graph()?;
    let roots = pick_roots(&o.conn, &g, o.map(args).as_deref(), &args.get_all("goal"))?;
    print_roots(&o.conn, &g, &roots)?;
    if roots.nodes.is_empty() {
        return Ok(1);
    }
    let id = find_decl(&o.conn, name)?;
    let v = g.node(id).context("the declaration is not in the reference graph")?;
    let c = g.cone(&roots.nodes, Follow::All);
    let chain = c.chain(v);
    if chain.is_empty() {
        println!("{name}: not reached from the endpoints in this data ({}): no chain of references leads to it", g.stats.label());
        let users = g.users(v).len();
        println!("  {users} declaration(s) refer to it");
        return Ok(1);
    }
    let names = rows(&o.conn, &g, &chain)?;
    println!("a shortest chain of references, {} steps ({}):", chain.len() - 1, g.stats.label());
    for (k, w) in chain.iter().enumerate() {
        let Some(r) = names.get(w) else { continue };
        let how = if k == 0 {
            "endpoint".to_string()
        } else {
            let e = g.uses(chain[k - 1]).iter().find(|e| *e & NODE == *w).copied().unwrap_or(0);
            match (e & STMT != 0, e & BODY != 0) {
                (true, true) => "stated and used by the one above",
                (true, false) => "in the statement of the one above",
                _ => "in the proof or body of the one above",
            }
            .to_string()
        };
        println!("  {:>3}  {} {}  [{}]  {}", k, r.kind.short(), r.display, where_of(&o.conn, r.id), how);
    }
    let d = c.dominators(v);
    if d.len() > 1 {
        let dn = rows(&o.conn, &g, &d)?;
        println!("every chain to it passes through: {}", d.iter().filter(|w| **w != v).filter_map(|w| dn.get(w)).map(|r| r.display.as_str()).collect::<Vec<_>>().join(" ▸ "));
    }
    println!("{} declaration(s) are reached only through it", c.excl[v as usize].saturating_sub(1));
    Ok(0)
}

/// `slim contents DIR [NAME]`: the dominator tree as a table of contents.
pub fn cmd_contents(args: &Args) -> Result<i32> {
    let o = open(args, args.positional.first().map(|s| s.as_str()))?;
    let g = o.graph()?;
    let roots = pick_roots(&o.conn, &g, o.map(args).as_deref(), &args.get_all("goal"))?;
    print_roots(&o.conn, &g, &roots)?;
    if roots.nodes.is_empty() {
        return Ok(1);
    }
    let c = g.cone(&roots.nodes, Follow::All);
    let depth = args.num::<usize>("depth")?.unwrap_or(4);
    let limit = args.num::<usize>("limit")?.unwrap_or(8);
    println!("contents by dependence ({}; {} of {} declarations reached)", g.stats.label(), c.reached, g.len());
    println!("a row's number: the declarations reached only through it, itself included");
    let start: Vec<u32> = match args.positional.get(if args.get("index").is_some() { 0 } else { 1 }) {
        Some(n) => {
            let v = g.node(find_decl(&o.conn, n)?).context("not in the reference graph")?;
            if !c.contains(v) {
                bail!("{n} is not reached from the endpoints");
            }
            vec![v]
        }
        None => c.top().to_vec(),
    };
    fn walk(o: &Opened, g: &Graph, c: &Cone, kids: &[u32], ind: usize, depth: usize, limit: usize) -> Result<()> {
        let shown = &kids[..kids.len().min(limit)];
        let names = rows(&o.conn, g, shown)?;
        for &k in shown {
            let Some(r) = names.get(&k) else { continue };
            // A relay hands over to one declaration: show the pair on one row.
            let chain = relay_chain(g, k);
            let via = if chain.len() > 1 {
                let rn = rows(&o.conn, g, &chain[1..])?;
                format!(" = {}", chain[1..].iter().filter_map(|w| rn.get(w)).map(|r| crate::app::state::short_name(&r.display)).collect::<Vec<_>>().join(" = "))
            } else {
                String::new()
            };
            println!("{:>9}  {}{} {}{}", thousands(c.excl[k as usize] as u64), "  ".repeat(ind), r.kind.short().trim_end(), r.display, via);
            if depth > 1 && c.excl[k as usize] > 1 {
                let mut at = k;
                // Skip relays that dominate only the next relay.
                while let [only] = c.children(at) {
                    if g.relay_target(at) == Some(*only) {
                        at = *only;
                    } else {
                        break;
                    }
                }
                walk(o, g, c, c.children(at), ind + 1, depth - 1, limit)?;
            }
        }
        if kids.len() > shown.len() {
            let rest: u64 = kids[shown.len()..].iter().map(|&k| c.excl[k as usize] as u64).sum();
            println!("{:>9}  {}… {} smaller ones", thousands(rest), "  ".repeat(ind), kids.len() - shown.len());
        }
        Ok(())
    }
    walk(&o, &g, &c, &start, 0, depth, limit)?;
    Ok(0)
}

/// `slim ledger DIR`: named statements and the theorems that prove them.
pub fn cmd_ledger(args: &Args) -> Result<i32> {
    let o = open(args, args.positional.first().map(|s| s.as_str()))?;
    let g = o.graph()?;
    let roots = pick_roots(&o.conn, &g, o.map(args).as_deref(), &args.get_all("goal"))?;
    let cone = (!roots.nodes.is_empty()).then(|| g.cone(&roots.nodes, Follow::All));
    // Restricted to a namespace: the ids of its declarations.
    let only: Option<std::collections::HashSet<u32>> = match args.get("namespace") {
        Some(ns) => {
            let ns = ns.trim_end_matches('.');
            let mut st = o.conn.prepare("SELECT id FROM decls WHERE name >= ?1 || '.' AND name < ?1 || '/'")?;
            let ids: Vec<i64> = st.query_map([ns], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            Some(ids.into_iter().filter_map(|id| g.node(id)).collect())
        }
        None => None,
    };
    let f = |v: u32| only.as_ref().is_none_or(|s| s.contains(&v));
    let all = ledger(&g, cone.as_ref(), Some(&f));
    let limit = args.num::<usize>("limit")?.unwrap_or(40);
    let proved = all.iter().filter(|r| !r.provers.is_empty()).count();
    let reached = all.iter().filter(|r| r.depth.is_some()).count();
    println!(
        "{} named statements{} ({}): {} have a theorem of exactly that type, {} are reached from the endpoints",
        all.len(),
        args.get("namespace").map(|n| format!(" under {n}")).unwrap_or_default(),
        g.stats.label(),
        proved,
        reached
    );
    println!("{:<6} {:>5}  statement  ⊢ proved by", "depth", "users");
    let shown = &all[..all.len().min(limit)];
    let mut want: Vec<u32> = shown.iter().map(|r| r.statement).collect();
    want.extend(shown.iter().flat_map(|r| r.provers.iter().take(1).copied()));
    let names = rows(&o.conn, &g, &want)?;
    for r in shown {
        let Some(s) = names.get(&r.statement) else { continue };
        let by = match r.provers.as_slice() {
            [] => "—".to_string(),
            [p, rest @ ..] => format!("{}{}", names.get(p).map(|x| x.display.as_str()).unwrap_or("?"), if rest.is_empty() { String::new() } else { format!(" (+{})", rest.len()) }),
        };
        println!("{:<6} {:>5}  {}  ⊢ {}", r.depth.map(|d| d.to_string()).unwrap_or_else(|| "·".into()), r.users, s.display, by);
    }
    if all.len() > shown.len() {
        println!("… {} more (--limit N)", all.len() - shown.len());
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// The contract of a statement, and a page about one declaration.

fn contract_opts(args: &Args, default_depth: u8) -> Result<crate::contract::Options> {
    let mut o = crate::contract::Options { depth: default_depth, ..Default::default() };
    if let Some(d) = args.num::<u8>("depth")? {
        o.depth = d.min(12);
    }
    if let Some(l) = args.num::<usize>("limit")? {
        o.max_items = l.clamp(1, 400);
    }
    Ok(o)
}

/// `slim contract DIR NAME [--depth N]`: the statement laid out as given
/// and then, with the project's vocabulary unfolded under it.
pub fn cmd_contract(args: &Args) -> Result<i32> {
    let o = open(args, args.positional.first().map(|s| s.as_str()))?;
    let name = args.positional.get(if args.get("index").is_some() { 0 } else { 1 }).context("usage: slim contract <PROJECT_DIR> <NAME> [--depth N]")?;
    let root = o.root.clone().context("the index has no source root to read the statement from")?;
    let id = find_decl(&o.conn, name)?;
    let mut cache = crate::sourceview::SourceCache::for_index(&o.index);
    let c = crate::contract::contract(&o.conn, &root, &mut cache, id, &contract_opts(args, 1)?)?;
    print!("{}", crate::contract::to_text(&crate::contract::render(&c)));
    println!("-- {}", crate::contract::footer(&c));
    Ok(0)
}

/// Statement headers of some declarations (as written, cut), by id.
fn statements(o: &Opened, cache: &mut crate::sourceview::SourceCache, ids: &[i64]) -> Vec<(DeclRow, String, String)> {
    let mut out = Vec::new();
    let Some(root) = o.root.as_deref() else { return out };
    for &id in ids {
        let Ok(Some(d)) = query::detail(&o.conn, id) else { continue };
        let Some(rel) = d.module_path.clone() else { continue };
        let Ok(f) = cache.get(root, &rel) else { continue };
        let (Some(kw), Some(he)) = (d.pos.kw_byte.or(d.pos.start_byte), d.pos.header_end_byte.or(d.pos.end_byte)) else { continue };
        let text = f.text.get(kw as usize..(he as usize).min(f.text.len())).unwrap_or("").trim_end();
        let (sig, _) = crate::skim::cut_statement(text);
        out.push((d.row.clone(), sig, format!("{rel}:{}", d.pos.sel_line.or(d.pos.start_line).unwrap_or(1))));
    }
    out
}

/// A page about one declaration, in Markdown: what it states with its
/// vocabulary unfolded, where it stands under the endpoints, what its
/// proof rests on, and what uses it. `base_url`: where the files can be
/// read online (`…/blob/<revision>/`), for links.
pub fn dossier(o: &Opened, g: Option<&Graph>, roots: &Roots, id: i64, opts: &crate::contract::Options, base_url: Option<&str>) -> Result<String> {
    use std::fmt::Write as _;
    let root = o.root.clone().context("the index has no source root to read from")?;
    let mut cache = crate::sourceview::SourceCache::for_index(&o.index);
    let c = crate::contract::contract(&o.conn, &root, &mut cache, id, opts)?;
    let it = &c.items[0];
    // A link is made below the Lake package the file belongs to: that is
    // where a repository of the project has it, wherever it was opened.
    let layout = std::cell::RefCell::new(crate::lake::Layout::new(&root));
    let link = |place: &str| -> String {
        match (base_url, place.rsplit_once(':')) {
            (Some(b), Some((p, l))) => {
                let pkg = layout.borrow_mut().package_of(p);
                format!("[{place}]({}/{}#L{l})", b.trim_end_matches('/'), p.strip_prefix(pkg.rel.as_str()).unwrap_or(p))
            }
            _ => format!("`{place}`"),
        }
    };
    let place = format!("{}:{}", it.path.clone().unwrap_or_default(), it.line.unwrap_or(1));
    let mut s = String::new();
    writeln!(s, "# {}\n", it.row.display)?;
    writeln!(s, "{} in {}. Written by slim {} from the source text; nothing here was checked by Lean for this page.\n", it.row.kind.as_str(), link(&place), env!("CARGO_PKG_VERSION"))?;
    writeln!(s, "## What it states\n")?;
    writeln!(s, "The project's own names are unfolded under the part that uses them; names from libraries are left as they are.\n")?;
    writeln!(s, "```lean\n{}```\n", crate::contract::to_text(&crate::contract::render(&c)))?;
    writeln!(s, "{}.\n", crate::contract::footer(&c))?;
    if let Some(g) = g {
        if let Some(v) = g.node(id) {
            let cone = (!roots.nodes.is_empty()).then(|| g.cone(&roots.nodes, Follow::All));
            writeln!(s, "## Where it stands\n")?;
            match cone.as_ref().map(|c| (c, c.chain(v))) {
                Some((cn, chain)) if !chain.is_empty() => {
                    let names = rows(&o.conn, g, &chain)?;
                    writeln!(s, "A shortest chain of references from an endpoint ({}), {} steps:\n", g.stats.label(), chain.len() - 1)?;
                    for (k, w) in chain.iter().enumerate() {
                        if let Some(r) = names.get(w) {
                            writeln!(s, "{}. `{}` ({})", k + 1, r.display, link(&where_of(&o.conn, r.id)))?;
                        }
                    }
                    writeln!(s, "\n{} declarations are reached only through it.\n", thousands(cn.excl[v as usize].saturating_sub(1) as u64))?;
                }
                Some(_) => writeln!(s, "No chain of references leads to it from the endpoints ({}).\n", g.stats.label())?,
                None => writeln!(s, "No endpoints are chosen for this project.\n")?,
            }
            // What the proof rests on: the references after the statement.
            let body: Vec<i64> = g.uses(v).iter().filter(|e| *e & BODY != 0 && !g.has(*e & NODE, nf::NESTED)).map(|e| g.id(e & NODE)).take(24).collect();
            let total = g.uses(v).iter().filter(|e| *e & BODY != 0).count();
            if !body.is_empty() && it.row.kind == DeclKind::Theorem {
                writeln!(s, "## What its proof refers to\n")?;
                writeln!(s, "{} declarations of the project are written in the proof ({}){}.\n", total, g.stats.label(), if total > body.len() { format!("; the first {}", body.len()) } else { String::new() })?;
                for (r, sig, at) in statements(o, &mut cache, &body) {
                    writeln!(s, "* `{}` ({})\n\n  ```lean\n  {}\n  ```", r.display, link(&at), sig.replace('\n', "\n  "))?;
                }
                writeln!(s)?;
            }
            let users: Vec<i64> = g.users(v).iter().map(|e| g.id(e & NODE)).collect();
            writeln!(s, "## What refers to it\n")?;
            if users.is_empty() {
                writeln!(s, "Nothing in the project refers to it ({}).\n", g.stats.label())?;
            } else {
                writeln!(s, "{} declarations ({}){}:\n", users.len(), g.stats.label(), if users.len() > 12 { "; the first 12" } else { "" })?;
                for r in query::rows_by_ids(&o.conn, &users[..users.len().min(12)])? {
                    writeln!(s, "* `{}` ({})", r.display, link(&where_of(&o.conn, r.id)))?;
                }
                writeln!(s)?;
            }
        }
    } else {
        writeln!(s, "## Where it stands\n\nNo reference graph was built for this project (`slim graph build`), so its place under the endpoints is not shown.\n")?;
    }
    Ok(s)
}

/// `slim dossier DIR NAME [-o FILE.md] [--depth N] [--from URL]`.
pub fn cmd_dossier(args: &Args) -> Result<i32> {
    let o = open(args, args.positional.first().map(|s| s.as_str()))?;
    let name = args.positional.get(if args.get("index").is_some() { 0 } else { 1 }).context("usage: slim dossier <PROJECT_DIR> <NAME> [-o FILE.md] [--depth N] [--from BASE_URL]")?;
    let id = find_decl(&o.conn, name)?;
    let g = o.graph().ok();
    let map = o.map(args);
    let roots = match &g {
        Some(g) => pick_roots(&o.conn, g, map.as_deref(), &args.get_all("goal"))?,
        None => Roots::default(),
    };
    let text = dossier(&o, g.as_ref(), &roots, id, &contract_opts(args, 3)?, args.get("from"))?;
    match args.path("o").or(args.path("out")) {
        Some(p) => {
            std::fs::write(&p, &text).with_context(|| format!("writing {}", p.display()))?;
            eprintln!("wrote {} ({} lines)", p.display(), text.lines().count());
        }
        None => print!("{text}"),
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// Rank and components, for search.

/// Directories that divide the project's declarations into readable
/// shares: the top-level directories, with any that holds more than two
/// fifths of everything divided again. Returns `(label, dotted prefix)` of
/// each and, by module id, which one a module belongs to.
pub fn components(conn: &Connection) -> Result<(Vec<(String, String)>, Vec<u16>)> {
    let mut st = conn.prepare("SELECT id, name, decl_count FROM modules WHERE is_local = 1")?;
    let mods: Vec<(i64, String, u64)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)?.max(0) as u64)))?.collect::<rusqlite::Result<_>>()?;
    let total: u64 = mods.iter().map(|m| m.2).sum::<u64>().max(1);
    // Declarations below every directory prefix, and each directory's
    // immediate sub-directories.
    let mut below: HashMap<&str, u64> = HashMap::new();
    let mut kids: HashMap<&str, std::collections::BTreeSet<&str>> = HashMap::new();
    for (_, name, n) in &mods {
        let mut parent = "";
        for (i, c) in name.char_indices() {
            if c == '.' {
                let dir = &name[..i];
                *below.entry(dir).or_default() += n;
                kids.entry(parent).or_default().insert(dir);
                parent = dir;
            }
        }
    }
    let mut groups: Vec<&str> = kids.get("").map(|s| s.iter().copied().collect()).unwrap_or_default();
    // Leading directories that hold nearly everything are not shown.
    let mut skip = "";
    for _ in 0..8 {
        let mut next = Vec::new();
        let mut changed = false;
        for g in &groups {
            let n = below.get(g).copied().unwrap_or(0);
            let sub = kids.get(g).map(|s| s.len()).unwrap_or(0);
            if sub >= 1 && n * 5 > total * 2 && next.len() + sub <= 400 {
                if n * 20 >= total * 19 && g.len() > skip.len() {
                    skip = g;
                }
                next.push(*g);
                next.extend(kids[g].iter().copied());
                changed = true;
            } else {
                next.push(*g);
            }
        }
        next.sort_unstable();
        next.dedup();
        groups = next;
        if !changed {
            break;
        }
    }
    let index: HashMap<&str, u16> = groups.iter().enumerate().map(|(i, g)| (*g, i as u16)).collect();
    let max_id = mods.iter().map(|m| m.0).max().unwrap_or(0).max(0) as usize;
    let mut of = vec![u16::MAX; max_id + 1];
    for (id, name, _) in &mods {
        // The longest group prefix that holds the module.
        let mut at = name.as_str();
        while let Some(i) = at.rfind('.') {
            at = &at[..i];
            if let Some(&g) = index.get(at) {
                of[*id as usize] = g;
                break;
            }
        }
    }
    let labels = groups
        .iter()
        .map(|g| {
            let shown = g.strip_prefix(skip).map(|s| s.trim_start_matches('.')).filter(|s| !s.is_empty()).unwrap_or(g);
            (shown.replace('.', "/"), g.to_string())
        })
        .collect();
    Ok((labels, of))
}

/// What search needs to rank and group: reach and nearness when there is a
/// graph and endpoints, components always.
pub fn rank(conn: &Connection, g: Option<&Graph>, depth: Option<&[u16]>) -> Result<crate::nameindex::Rank> {
    use crate::nameindex::Rank;
    let (components, component) = components(conn)?;
    let mut r = Rank { components, component, ..Default::default() };
    let Some(g) = g else { return Ok(r) };
    let max_id = g.ids.last().copied().unwrap_or(0).max(0) as usize;
    r.bonus = vec![0; max_id + 1];
    r.flags = vec![0; max_id + 1];
    r.has_cone = depth.is_some();
    for v in 0..g.len() as u32 {
        let id = g.id(v);
        if id < 0 {
            continue;
        }
        let users = g.users(v).len() as u32;
        let used = (80 * (32 - (users + 1).leading_zeros())).min(500);
        let named = if g.has(v, nf::PROP) { 300 } else { 0 };
        let mut b = used + named;
        let mut f = Rank::KNOWN;
        if let Some(d) = depth.map(|d| d[v as usize]).filter(|&d| d != u16::MAX) {
            f |= Rank::IN_CONE;
            b += 2000 + 1200u32.saturating_sub(60 * d as u32);
        }
        r.bonus[id as usize] = b.min(Rank::MAX_BONUS as u32) as u16;
        r.flags[id as usize] = f;
    }
    Ok(r)
}

/// The rank for an index as it is: its graph if one is built for it, and
/// reach from `goal` when one is given and the graph holds it, else from
/// the endpoints the map or the import order gives.
pub fn rank_for_index(index: &Path, conn: &Connection, map: Option<&ProjectMap>, goal: Option<&str>) -> Result<crate::nameindex::Rank> {
    let generation = crate::db::get_meta(conn, "name_index_generation")?.unwrap_or_default();
    let g = Graph::load(index, &generation).ok().flatten();
    let depth = match &g {
        Some(g) => {
            let explicit: Vec<String> = goal.map(|s| s.to_string()).into_iter().collect();
            let roots = match pick_roots(conn, g, map, &explicit) {
                Ok(r) => r,
                Err(_) => pick_roots(conn, g, map, &[])?,
            };
            (!roots.nodes.is_empty()).then(|| g.reach(&roots.nodes, Follow::All).1)
        }
        None => None,
    };
    rank(conn, g.as_ref(), depth.as_deref())
}
