//! Deterministic benchmark of the index and browser back end.
//!
//! The synthetic snapshot is generated from a fixed seed: declarations in
//! nested namespaces with names drawn from a mathematical vocabulary,
//! docstrings for a fraction of them, and typed edges to earlier
//! declarations with a heavy-tailed target distribution (so there are hubs
//! with tens of thousands of dependents, as in real libraries), plus
//! standard, custom and `sorryAx` axioms. It exercises the browser and index
//! only; it says nothing about Lean extraction or mathematical usefulness,
//! and its header is marked `"synthetic": true`.

use crate::cli::Args;
use crate::db::{self, query};
use crate::model::KindFilter;
use crate::nameindex::{NameIndex, SearchParams};
use crate::snapshot::*;
use anyhow::{bail, Result};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// SplitMix64: tiny, deterministic, good enough for workload generation.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next_u64() % n
        }
    }
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

const WORDS: &[&str] = &[
    "reach", "trans", "closure", "refl", "symm", "comp", "inv", "subgroup", "normal", "sylow", "simple", "order", "card",
    "finite", "prime", "cyclic", "abelian", "quotient", "kernel", "image", "hom", "iso", "mono", "epi", "orbit",
    "stabilizer", "centralizer", "normalizer", "commutator", "nilpotent", "solvable", "series", "chain", "lattice",
    "module", "ring", "field", "ideal", "basis", "rank", "dim", "trace", "det", "eigen", "graph", "path", "cycle",
    "tree", "edge", "vertex", "degree", "measure", "integral", "limit", "cont", "deriv", "bound", "sum", "prod", "mul",
    "add", "zero", "one", "pow", "succ", "le", "lt", "max", "min", "sup", "inf", "map", "fold", "filter", "mem",
];

fn name_for(rng: &mut Rng, i: usize, modules: usize) -> (String, usize) {
    let m = (i * 7919) % modules.max(1);
    let a = WORDS[rng.below(WORDS.len() as u64) as usize];
    let b = WORDS[rng.below(WORDS.len() as u64) as usize];
    let c = WORDS[rng.below(WORDS.len() as u64) as usize];
    let ns = format!("Synth.{}{}", capitalize(a), m % 97);
    (format!("{ns}.{b}_{c}_{i}"), m)
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

pub struct GenStats {
    pub decls: usize,
    pub edges: usize,
    pub bytes: u64,
}

/// Writes a synthetic snapshot with `n` declarations and about `e` edges.
pub fn generate(path: &Path, n: usize, e: usize, seed: u64) -> Result<GenStats> {
    let mut rng = Rng::new(seed);
    let modules = (n / 100).max(1);
    let f = std::fs::File::create(path)?;
    let mut w = BufWriter::with_capacity(1 << 20, f);
    let h = Header {
        record: "header".into(),
        format: FORMAT.into(),
        format_version: FORMAT_VERSION,
        producer: Producer { name: "slim bench generator".into(), version: env!("CARGO_PKG_VERSION").into() },
        exporter: None,
        lean: None,
        project: Some(ProjectInfo { name: format!("synthetic-{n}-{e}-{seed}"), ..Default::default() }),
        scope: Some(serde_json::json!({"synthetic": true, "decls": n, "edges": e, "seed": seed})),
        created_at: None,
        build: None,
        isolation: None,
        synthetic: true,
    };
    writeln!(w, "{}", header_line(&h))?;
    for m in 0..modules {
        let mr = ModuleRec { name: format!("Synth.Mod{m}"), local: true, ..Default::default() };
        writeln!(w, "{}", module_line(&mr))?;
    }
    writeln!(w, "{}", module_line(&ModuleRec { name: "Init.Core".into(), local: false, ..Default::default() }))?;
    // Library hubs and axioms.
    let lib: Vec<(&str, &str)> = vec![
        ("propext", "axiom"),
        ("Classical.choice", "axiom"),
        ("Quot.sound", "axiom"),
        ("sorryAx", "axiom"),
        ("Synth.oracle", "axiom"),
        ("Nat", "inductive"),
        ("Eq", "inductive"),
        ("Iff", "structure"),
        ("And", "structure"),
        ("Exists", "inductive"),
        ("funext", "theorem"),
        ("Classical.em", "theorem"),
    ];
    let mut names: Vec<String> = Vec::with_capacity(n + lib.len());
    for (nm, _) in &lib {
        names.push(nm.to_string());
    }
    let nlib = lib.len();
    for i in 0..n {
        names.push(name_for(&mut rng, i, modules).0);
    }
    let edges_per = (e as f64 / n.max(1) as f64).max(0.0);
    let mut edges = 0usize;
    for (k, (nm, kind)) in lib.iter().enumerate() {
        let mut d = DeclRec {
            name: nm.to_string(),
            kind: kind.to_string(),
            module: Some("Init.Core".into()),
            local: false,
            coverage: CoverageRec { type_: "complete".into(), value: if *kind == "theorem" { "complete".into() } else { "not_applicable".into() } },
            ..Default::default()
        };
        if *nm == "funext" {
            d.refs.value = vec!["Quot.sound".into()];
        }
        if *nm == "Classical.em" {
            d.refs.value = vec!["propext".into(), "Classical.choice".into()];
        }
        let _ = k;
        writeln!(w, "{}", decl_line(&d))?;
    }
    for i in 0..n {
        let idx = nlib + i;
        let kind = match rng.below(100) {
            0..=59 => "theorem",
            60..=84 => "def",
            85..=89 => "structure",
            90..=94 => "instance",
            _ => "abbrev",
        };
        let deg = {
            // geometric-ish around edges_per
            let u = rng.unit();
            ((-u.max(1e-9).ln()) * edges_per).round() as usize
        };
        let mut tys = Vec::new();
        let mut vals = Vec::new();
        for j in 0..deg.min(200) {
            // heavy-tailed preference for low indices (hubs)
            let u = rng.unit();
            let target = ((idx as f64) * u.powi(3)) as usize;
            if target >= idx {
                continue;
            }
            if j % 3 == 0 {
                tys.push(names[target].clone());
            } else {
                vals.push(names[target].clone());
            }
        }
        if rng.below(2000) == 0 {
            vals.push("sorryAx".into());
        }
        if rng.below(5000) == 0 {
            vals.push("Synth.oracle".into());
        }
        tys.sort();
        tys.dedup();
        vals.sort();
        vals.dedup();
        if kind == "structure" {
            vals.clear();
        }
        edges += tys.len() + vals.len();
        let m = (i * 7919) % modules;
        let doc = if rng.below(10) < 3 {
            let k = 6 + rng.below(10) as usize;
            Some((0..k).map(|_| WORDS[rng.below(WORDS.len() as u64) as usize]).collect::<Vec<_>>().join(" "))
        } else {
            None
        };
        let d = DeclRec {
            name: names[idx].clone(),
            kind: kind.into(),
            module: Some(format!("Synth.Mod{m}")),
            local: true,
            type_: Some(format!("∀ (x : Nat), {} x = {} x", WORDS[i % WORDS.len()], WORDS[(i / 3) % WORDS.len()])),
            doc,
            refs: Refs { type_: tys, value: vals, ctor: vec![] },
            coverage: CoverageRec {
                type_: "complete".into(),
                value: if kind == "structure" { "not_applicable".into() } else { "complete".into() },
            },
            ..Default::default()
        };
        writeln!(w, "{}", decl_line(&d))?;
    }
    writeln!(w, "{}", footer_line(&Footer { counts: Counts { modules: modules as u64 + 1, decls: (n + nlib) as u64 }, complete: true }))?;
    w.flush()?;
    drop(w);
    let bytes = std::fs::metadata(path)?.len();
    Ok(GenStats { decls: n + nlib, edges, bytes })
}

fn peak_rss_mib() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmHWM:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|k| k.parse::<f64>().ok()))
        .map(|kb| kb / 1024.0)
        .unwrap_or(0.0)
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let i = ((p / 100.0) * (v.len() - 1) as f64).round() as usize;
    v[i.min(v.len() - 1)]
}

fn stats(name: &str, mut v: Vec<f64>) -> String {
    let n = v.len();
    let p50 = percentile(&mut v, 50.0);
    let p95 = percentile(&mut v, 95.0);
    let max = v.iter().cloned().fold(0.0, f64::max);
    format!("| {name} | {n} | {p50:.2} | {p95:.2} | {max:.2} |")
}

fn hardware() -> Vec<String> {
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("model name")).map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string()))
        .unwrap_or_else(|| "unknown".into());
    let mem = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| s.lines().next().map(|l| l.split_whitespace().nth(1).unwrap_or("0").parse::<f64>().unwrap_or(0.0) / 1048576.0))
        .unwrap_or(0.0);
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    let par = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0);
    vec![
        format!("CPU: {cpu}; CPUs available to this process: {par}"),
        format!("RAM: {mem:.1} GiB; kernel {}", kernel.trim()),
        format!("build: {}", if cfg!(debug_assertions) { "debug" } else { "release" }),
    ]
}

pub struct BenchConfig {
    pub decls: usize,
    pub edges: usize,
    pub seed: u64,
    pub repeat: usize,
    pub max_seconds: u64,
    pub dir: PathBuf,
}

pub fn run(cfg: &BenchConfig, out: &mut dyn Write) -> Result<()> {
    let t_all = Instant::now();
    let deadline = t_all + Duration::from_secs(cfg.max_seconds);
    let check = |phase: &str| -> Result<()> {
        if Instant::now() > deadline {
            bail!("benchmark exceeded --max-seconds {} during {phase}; partial results above", cfg.max_seconds);
        }
        Ok(())
    };
    std::fs::create_dir_all(&cfg.dir)?;
    writeln!(out, "# slim benchmark\n")?;
    writeln!(out, "Synthetic snapshot: {} declarations, ~{} edges, seed {}. Synthetic data tests the index and browser back end only.\n", cfg.decls, cfg.edges, cfg.seed)?;
    for h in hardware() {
        writeln!(out, "- {h}")?;
    }
    writeln!(out)?;
    let snap = cfg.dir.join(format!("synthetic-{}-{}-{}.jsonl", cfg.decls, cfg.edges, cfg.seed));
    let idx = cfg.dir.join(format!("synthetic-{}-{}-{}.sqlite", cfg.decls, cfg.edges, cfg.seed));
    let t = Instant::now();
    let g = generate(&snap, cfg.decls, cfg.edges, cfg.seed)?;
    writeln!(out, "- generated snapshot: {} declarations, {} edges, {:.1} MiB in {:.2} s", g.decls, g.edges, g.bytes as f64 / 1048576.0, t.elapsed().as_secs_f64())?;
    out.flush()?;
    check("generation")?;
    // Cold import.
    let _ = std::fs::remove_file(&idx);
    let t = Instant::now();
    let s = db::import::import_snapshot(&snap, &idx, &db::import::ImportOptions::default())?;
    let import_s = t.elapsed().as_secs_f64();
    let size = std::fs::metadata(&idx)?.len() as f64 / 1048576.0;
    let names_size = std::fs::metadata(db::import::names_path(&idx)).map(|m| m.len()).unwrap_or(0) as f64 / 1048576.0;
    writeln!(
        out,
        "- cold import (validate, load, axiom analysis, indexes, FTS, name index): {import_s:.2} s; {} edges, {} axioms; index {size:.1} MiB + name index {names_size:.1} MiB; peak RSS so far {:.0} MiB",
        s.edges,
        s.axioms,
        peak_rss_mib()
    )?;
    out.flush()?;
    check("import")?;
    // Warm open: connection + metadata + name index (search-ready).
    let mut opens = Vec::new();
    let mut ni_opt: Option<NameIndex> = None;
    for _ in 0..cfg.repeat.max(3) {
        let t = Instant::now();
        let conn = db::open_read(&idx)?;
        let _info = query::index_info(&conn)?;
        let gen = db::get_meta(&conn, "name_index_generation")?.unwrap_or_default();
        let ni = NameIndex::load(&db::import::names_path(&idx), &gen)?;
        let _ = query::modules(&conn, true)?;
        opens.push(t.elapsed().as_secs_f64() * 1000.0);
        ni_opt = Some(ni);
    }
    let ni = ni_opt.unwrap();
    let conn = db::open_read(&idx)?;
    writeln!(out, "\n| warm path (ms) | n | p50 | p95 | max |\n|---|---|---|---|---|")?;
    writeln!(out, "{}", stats("open index + metadata + modules + name index load", opens))?;
    // Search.
    let mut rng = Rng::new(cfg.seed ^ 0xABCDEF);
    let mut queries: Vec<String> = Vec::new();
    for _ in 0..10 {
        let id = 13 + rng.below(cfg.decls as u64) as i64;
        if let Some(r) = query::row(&conn, id)? {
            queries.push(r.name.clone());
            queries.push(r.name[r.name.rfind('.').map(|x| x + 1).unwrap_or(0)..].to_string());
        }
    }
    for w in ["reach", "sylow", "trans", "closure_", "Synth.Reach", "normal sub", "cent norm", "sylw", "rch_trns", "x"] {
        queries.push(w.to_string());
    }
    let mut lat = Vec::new();
    let mut doc_lat = Vec::new();
    for _ in 0..cfg.repeat {
        for q in &queries {
            let p = SearchParams { query: q.clone(), limit: 300, kinds: KindFilter::All, ..Default::default() };
            let t = Instant::now();
            let o = ni.search(&p, &|| false);
            let ids: Vec<i64> = o.hits.iter().take(60).map(|h| h.id as i64).collect();
            let _rows = query::rows_by_ids(&conn, &ids)?;
            lat.push(t.elapsed().as_secs_f64() * 1000.0);
            let t = Instant::now();
            let _ = query::text_search(&conn, q, 60)?;
            doc_lat.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        check("search")?;
    }
    writeln!(out, "{}", stats("name search incl. fetching the visible page (20 query shapes)", lat))?;
    writeln!(out, "{}", stats("documentation full-text search", doc_lat))?;
    // Neighbours: random declarations plus the biggest hubs.
    let mut ids: Vec<i64> = (0..100).map(|_| 13 + rng.below(cfg.decls as u64) as i64).collect();
    let mut st = conn.prepare("SELECT id FROM decls ORDER BY in_type + in_value DESC LIMIT 10")?;
    let hubs: Vec<i64> = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    ids.extend(&hubs);
    let filt = query::NeighborFilter { local_only: false, show_generated: false, kinds: KindFilter::All };
    let filt_local = query::NeighborFilter { local_only: true, show_generated: false, kinds: KindFilter::Theorems };
    let mut nb = Vec::new();
    let mut nb_rev = Vec::new();
    let mut nb_rev_f = Vec::new();
    let mut det = Vec::new();
    let mut ev = Vec::new();
    for _ in 0..cfg.repeat {
        for &id in &ids {
            let t = Instant::now();
            query::neighbors(&conn, id, query::Direction::Uses, &filt, 100, 0)?;
            nb.push(t.elapsed().as_secs_f64() * 1000.0);
            let t = Instant::now();
            query::neighbors(&conn, id, query::Direction::UsedBy, &filt, 100, 0)?;
            nb_rev.push(t.elapsed().as_secs_f64() * 1000.0);
            let t = Instant::now();
            query::neighbors(&conn, id, query::Direction::UsedBy, &filt_local, 100, 0)?;
            nb_rev_f.push(t.elapsed().as_secs_f64() * 1000.0);
            let t = Instant::now();
            query::detail(&conn, id)?;
            det.push(t.elapsed().as_secs_f64() * 1000.0);
            let t = Instant::now();
            query::axiom_evidence(&conn, id)?;
            ev.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        check("neighbours")?;
    }
    let hub_in: i64 = conn.query_row("SELECT max(in_type + in_value) FROM decls", [], |r| r.get(0))?;
    writeln!(out, "{}", stats("direct dependencies (3 groups, ≤100 each, with counts)", nb))?;
    writeln!(out, "{}", stats(&format!("direct dependents (incl. 10 hubs, largest fan-in {hub_in})"), nb_rev))?;
    writeln!(out, "{}", stats("direct dependents, filtered (local theorems only)", nb_rev_f))?;
    writeln!(out, "{}", stats("declaration detail", det))?;
    writeln!(out, "{}", stats("axiom evidence with witness paths", ev))?;
    out.flush()?;
    // Responsiveness while ingesting: query latency on this index while a
    // second import runs concurrently.
    let idx2 = cfg.dir.join("concurrent-import.sqlite");
    let snap2 = snap.clone();
    let idx2c = idx2.clone();
    let handle = std::thread::spawn(move || db::import::import_snapshot(&snap2, &idx2c, &db::import::ImportOptions::default()));
    let mut during = Vec::new();
    let t0 = Instant::now();
    while !handle.is_finished() && t0.elapsed() < Duration::from_secs(120) {
        let q = &queries[during.len() % queries.len()];
        let t = Instant::now();
        let o = ni.search(&SearchParams { query: q.clone(), ..Default::default() }, &|| false);
        let id = o.hits.first().map(|h| h.id as i64).unwrap_or(1);
        query::neighbors(&conn, id, query::Direction::UsedBy, &filt, 100, 0)?;
        during.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let _ = handle.join();
    let _ = std::fs::remove_file(&idx2);
    let _ = std::fs::remove_file(db::import::names_path(&idx2));
    writeln!(out, "{}", stats("search + dependents while another import runs", during))?;
    writeln!(out, "\n- peak RSS of the benchmark process: {:.0} MiB", peak_rss_mib())?;
    writeln!(out, "- name index in memory: {:.1} MiB for {} entries", ni.memory_bytes() as f64 / 1048576.0, ni.len())?;
    writeln!(out, "- total benchmark time: {:.1} s", t_all.elapsed().as_secs_f64())?;
    Ok(())
}

/// Warm-path measurements on an existing (real) index: name-index load,
/// search with page fetch, documentation search, detail, outline and
/// neighbour queries for deterministic samples of its declarations.
pub fn run_on_index(idx: &Path, repeat: usize, seed: u64, out: &mut dyn Write) -> Result<()> {
    let conn = db::open_read(idx)?;
    let info = query::index_info(&conn)?;
    writeln!(out, "# slim measurement on an existing index\n")?;
    writeln!(out, "- index: {} ({:.1} MiB, mode {})", idx.display(), std::fs::metadata(idx)?.len() as f64 / 1048576.0, info.mode)?;
    for k in ["project_name", "source_root", "project_lean_toolchain", "file_count", "decl_count", "lean_version"] {
        if let Some(v) = info.get(k) {
            writeln!(out, "- {k}: {v}")?;
        }
    }
    for h in hardware() {
        writeln!(out, "- {h}")?;
    }
    let gen = db::get_meta(&conn, "name_index_generation")?.unwrap_or_default();
    let mut loads = Vec::new();
    let mut ni = None;
    for _ in 0..repeat.max(3) {
        let t = Instant::now();
        let c = db::open_read(idx)?;
        let _ = query::index_info(&c)?;
        let n = NameIndex::load(&db::import::names_path(idx), &gen)?;
        loads.push(t.elapsed().as_secs_f64() * 1000.0);
        ni = Some(n);
    }
    let ni = ni.unwrap();
    writeln!(out, "\n| warm path (ms) | n | p50 | p95 | max |\n|---|---|---|---|---|")?;
    writeln!(out, "{}", stats(&format!("open index + metadata + name index load ({} names)", ni.len()), loads))?;
    let max_id: i64 = conn.query_row("SELECT max(id) FROM decls", [], |r| r.get(0))?;
    let mut rng = Rng::new(seed);
    let mut sample = Vec::new();
    while sample.len() < 200 {
        let id = 1 + rng.below(max_id as u64) as i64;
        if let Some(r) = query::row(&conn, id)? {
            sample.push(r);
        }
    }
    let mut queries: Vec<String> = Vec::new();
    for r in sample.iter().take(20) {
        queries.push(r.name.clone());
        let last = &r.name[crate::model::last_component_start(&r.name)..];
        queries.push(last.to_string());
        queries.push(last.chars().take(5).collect());
    }
    for w in ["sylow", "normal subgroup", "card", "simple", "orbit", "thm", "lemma_", "cent norm"] {
        queries.push(w.to_string());
    }
    let mut lat = Vec::new();
    let mut doc = Vec::new();
    for _ in 0..repeat {
        for q in &queries {
            let t = Instant::now();
            let o = ni.search(&SearchParams { query: q.clone(), ..Default::default() }, &|| false);
            let ids: Vec<i64> = o.hits.iter().take(60).map(|h| h.id as i64).collect();
            let _ = query::rows_by_ids(&conn, &ids)?;
            lat.push(t.elapsed().as_secs_f64() * 1000.0);
            let t = Instant::now();
            let _ = query::text_search(&conn, q, 60)?;
            doc.push(t.elapsed().as_secs_f64() * 1000.0);
        }
    }
    writeln!(out, "{}", stats(&format!("name search incl. page fetch ({} query shapes)", queries.len()), lat))?;
    writeln!(out, "{}", stats("documentation full-text search", doc))?;
    let filt = query::NeighborFilter { local_only: false, show_generated: false, kinds: KindFilter::All };
    let (mut det, mut outl, mut nb) = (Vec::new(), Vec::new(), Vec::new());
    for _ in 0..repeat {
        for r in &sample {
            let t = Instant::now();
            let d = query::detail(&conn, r.id)?;
            det.push(t.elapsed().as_secs_f64() * 1000.0);
            if let Some(mid) = d.and_then(|d| d.module_id) {
                let t = Instant::now();
                let _ = query::module_decls(&conn, mid, false, 20_000)?;
                outl.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            let t = Instant::now();
            let _ = query::neighbors(&conn, r.id, query::Direction::UsedBy, &filt, 100, 0)?;
            nb.push(t.elapsed().as_secs_f64() * 1000.0);
        }
    }
    writeln!(out, "{}", stats("declaration detail", det))?;
    writeln!(out, "{}", stats("module outline (declarations of the module)", outl))?;
    writeln!(out, "{}", stats("direct dependents (semantic edges; none in source mode)", nb))?;
    let rss_before_overview = peak_rss_mib();

    // The overview features: outline with roll-ups, scoped search, skim,
    // the import graph, and mentions with their positions.
    let mut tree = Vec::new();
    let mut nodes = 0;
    for _ in 0..repeat.max(3) {
        let t = Instant::now();
        let tr = crate::app::worker::build_outline_tree(query::modules_with_stats(&conn, true)?);
        tree.push(t.elapsed().as_secs_f64() * 1000.0);
        nodes = tr.nodes.len();
    }
    writeln!(out, "{}", stats(&format!("outline tree with per-directory counts ({nodes} nodes)"), tree))?;
    let mode = db::mode_of(&conn)?;
    let (mut ns_lat, mut mod_lat, mut set_lat) = (Vec::new(), Vec::new(), Vec::new());
    let mut scopes: Vec<(String, std::sync::Arc<crate::nameindex::ModuleSet>)> = Vec::new();
    for r in sample.iter().take(20) {
        // A file, its directory and the top-level directory.
        let Some(m) = &r.module else { continue };
        let comps = crate::model::name_components(m);
        for k in [comps.len(), comps.len().saturating_sub(1).max(1), 1] {
            let prefix = comps[..k].join(".");
            if scopes.iter().any(|s| s.0 == prefix) {
                continue;
            }
            let t = Instant::now();
            let ids: Vec<i64> = conn
                .prepare_cached("SELECT id FROM modules WHERE name = ?1 OR (name >= ?1 || '.' AND name < ?1 || '/')")?
                .query_map([&prefix], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let set = crate::nameindex::ModuleSet::from_ids(ids.into_iter().map(|i| i as u32));
            set_lat.push(t.elapsed().as_secs_f64() * 1000.0);
            scopes.push((prefix, std::sync::Arc::new(set)));
        }
    }
    for _ in 0..repeat {
        for (i, q) in queries.iter().enumerate() {
            let r = &sample[i % sample.len().min(20)];
            let ns = crate::model::namespace_of(&r.display);
            if !ns.is_empty() {
                let t = Instant::now();
                let p = SearchParams { query: q.clone(), scope: Some(crate::nameindex::Scope::Namespace(ns.to_string())), ..Default::default() };
                let o = ni.search(&p, &|| false);
                let ids: Vec<i64> = o.hits.iter().take(60).map(|h| h.id as i64).collect();
                let _ = query::rows_by_ids(&conn, &ids)?;
                ns_lat.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            if !scopes.is_empty() {
                let (_, set) = &scopes[i % scopes.len()];
                let t = Instant::now();
                let p = SearchParams { query: q.clone(), scope: Some(crate::nameindex::Scope::Modules(set.clone())), ..Default::default() };
                let o = ni.search(&p, &|| false);
                let ids: Vec<i64> = o.hits.iter().take(60).map(|h| h.id as i64).collect();
                let _ = query::rows_by_ids(&conn, &ids)?;
                mod_lat.push(t.elapsed().as_secs_f64() * 1000.0);
            }
        }
    }
    writeln!(out, "{}", stats("name search within a namespace incl. page fetch", ns_lat))?;
    writeln!(out, "{}", stats(&format!("name search within files (a file, its directory, a top-level directory; {} scopes) incl. page fetch", scopes.len()), mod_lat))?;
    writeln!(out, "{}", stats("resolving a file scope to module ids (once per scope)", set_lat))?;
    let root = info.get("source_root").map(std::path::PathBuf::from).filter(|p| p.is_dir());
    let mut cache = crate::sourceview::SourceCache::default();
    let (mut sk_file, mut sk_dir, mut sk_ns, mut ment) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut dir_decls, mut dir_n) = (0usize, 0usize);
    for _ in 0..repeat.min(2) {
        for r in sample.iter().take(60) {
            let Some(m) = &r.module else { continue };
            let mut env = crate::skim::Env { conn: &conn, mode, source_root: root.as_deref(), cache: &mut cache, map: None, show_generated: false };
            let t = Instant::now();
            let _ = crate::skim::skim(&mut env, &crate::skim::SkimScope::Module(m.clone()))?;
            sk_file.push(t.elapsed().as_secs_f64() * 1000.0);
            let comps = crate::model::name_components(m);
            let dir = comps[..comps.len().saturating_sub(1).max(1)].join(".");
            let t = Instant::now();
            let s = crate::skim::skim(&mut env, &crate::skim::SkimScope::Dir(dir))?;
            sk_dir.push(t.elapsed().as_secs_f64() * 1000.0);
            dir_decls += s.decls;
            dir_n += 1;
            let ns = crate::model::namespace_of(&r.name);
            if !ns.is_empty() {
                let t = Instant::now();
                let _ = crate::skim::skim(&mut env, &crate::skim::SkimScope::Namespace(ns.to_string()))?;
                sk_ns.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            if mode == db::IndexMode::Source {
                let t = Instant::now();
                let _ = crate::relations::lexical_mentions(&conn, root.as_deref(), &mut cache, r.id)?;
                ment.push(t.elapsed().as_secs_f64() * 1000.0);
            }
        }
    }
    writeln!(out, "{}", stats("skim of a file (statements read from the source)", sk_file))?;
    writeln!(out, "{}", stats(&format!("skim of a directory (bounded: {} declarations, {} files; {} declarations on average here)", crate::skim::MAX_DECLS, crate::skim::MAX_FILES, dir_decls / dir_n.max(1)), sk_dir))?;
    writeln!(out, "{}", stats("skim of a namespace", sk_ns))?;
    writeln!(out, "{}", stats("lexical mentions of a declaration, with the position of every occurrence", ment))?;
    let mut gl = Vec::new();
    let mut graph = None;
    for _ in 0..repeat.max(3) {
        let t = Instant::now();
        graph = Some(crate::imports::ImportGraph::load(&conn)?);
        gl.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let g = graph.unwrap();
    writeln!(out, "{}", stats(&format!("import graph: read, depths, directory tree ({} local files, {} import lines between them)", g.local_count, g.local_edges), gl))?;
    let (mut ds, mut pl) = (Vec::new(), Vec::new());
    for _ in 0..repeat {
        for &r in &g.roots {
            let t = Instant::now();
            let _ = g.dir_summary(r, 12);
            ds.push(t.elapsed().as_secs_f64() * 1000.0);
            for &c in g.dirs[r as usize].children.iter().take(40) {
                let t = Instant::now();
                let _ = g.dir_summary(c, 12);
                ds.push(t.elapsed().as_secs_f64() * 1000.0);
            }
        }
        // From the deepest endpoints down to a file with no local imports.
        let bottoms: Vec<u32> = (0..g.nodes.len() as u32).filter(|&v| g.nodes[v as usize].local && g.depth[v as usize] == 0).take(8).collect();
        for &a in g.endpoints.iter().take(8) {
            for &b in &bottoms {
                let t = Instant::now();
                let _ = g.path(a, b);
                pl.push(t.elapsed().as_secs_f64() * 1000.0);
            }
        }
    }
    writeln!(out, "{}", stats("imports of a directory, grouped (whole project and its first-level directories)", ds))?;
    writeln!(out, "{}", stats("import path between two files (exhaustive search)", pl))?;
    writeln!(
        out,
        "\n- import graph: {} endpoints, {} import-only files, deepest chain {}, about {:.1} MiB in memory",
        g.endpoints.len(),
        g.import_only.len(),
        g.max_depth,
        g.memory_bytes() as f64 / 1048576.0
    )?;
    writeln!(
        out,
        "- peak RSS: {:.0} MiB before the overview measurements, {:.0} MiB after; name index in memory {:.1} MiB",
        rss_before_overview,
        peak_rss_mib(),
        ni.memory_bytes() as f64 / 1048576.0
    )?;
    Ok(())
}

pub fn cmd_bench(args: &Args) -> Result<i32> {
    if let Some(idx) = args.path("index") {
        let mut stdout = std::io::stdout();
        let mut buf: Vec<u8> = Vec::new();
        run_on_index(&idx, args.num::<usize>("repeat")?.unwrap_or(5), args.num::<u64>("seed")?.unwrap_or(42), &mut buf)?;
        stdout.write_all(&buf)?;
        if let Some(p) = args.path("report") {
            std::fs::write(&p, &buf)?;
        }
        return Ok(0);
    }
    let full = args.has("full");
    let (d, e) = if full { (100_000, 1_000_000) } else { (10_000, 100_000) };
    let cfg = BenchConfig {
        decls: args.num::<usize>("decls")?.unwrap_or(d),
        edges: args.num::<usize>("edges")?.unwrap_or(e),
        seed: args.num::<u64>("seed")?.unwrap_or(42),
        repeat: args.num::<usize>("repeat")?.unwrap_or(5),
        max_seconds: args.num::<u64>("max-seconds")?.unwrap_or(if full { 900 } else { 180 }),
        dir: args.path("out").unwrap_or_else(|| crate::paths::cache_dir().join("bench")),
    };
    let mut buf: Vec<u8> = Vec::new();
    struct Tee<'a>(&'a mut Vec<u8>);
    impl Write for Tee<'_> {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            std::io::stdout().write_all(b)?;
            self.0.extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            std::io::stdout().flush()
        }
    }
    let r = run(&cfg, &mut Tee(&mut buf));
    if let Some(p) = args.path("report") {
        std::fs::write(&p, &buf)?;
    }
    r?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generator_is_deterministic_and_importable() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a.jsonl");
        let b = d.path().join("b.jsonl");
        generate(&a, 500, 3000, 7).unwrap();
        generate(&b, 500, 3000, 7).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
        let idx = d.path().join("i.sqlite");
        let s = db::import::import_snapshot(&a, &idx, &db::import::ImportOptions::default()).unwrap();
        assert_eq!(s.local, 500);
        assert!(s.edges > 1000);
        let conn = db::open_read(&idx).unwrap();
        assert_eq!(db::get_meta(&conn, "synthetic").unwrap().as_deref(), Some("1"));
    }
}
