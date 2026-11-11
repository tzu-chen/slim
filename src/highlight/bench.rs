//! `slim highlight bench`: headless measurements of highlighting on a
//! real project. Everything runs in this process on one thread; nothing is
//! elaborated and the project is only read. Baseline spans are persisted to
//! a scratch cache, never to the project's own cache; the project's
//! semantic cache, if any, is copied and read from the copy.
//!
//! The pseudo-terminal sessions in `docs/bench/highlight.md` complement
//! this with whole-application numbers.

use super::baseline::BaselineBuilder;
use super::codec::Blob;
use super::service::{FileHighlight, HlResp, Service};
use super::store::Store;
use super::{Mode, SemTok};
use crate::cli::Args;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((p / 100.0) * (v.len() - 1) as f64).round() as usize]
}

fn dist(mut v: Vec<f64>) -> Value {
    if v.is_empty() {
        return json!({"n": 0});
    }
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    json!({"n": v.len(), "mean": mean, "p50": pct(&mut v, 50.0), "p95": pct(&mut v, 95.0), "p99": pct(&mut v, 99.0), "max": pct(&mut v, 100.0)})
}

fn rss_mib() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).and_then(|l| l.split_whitespace().nth(1).and_then(|v| v.parse::<f64>().ok())))
        .unwrap_or(0.0)
        / 1024.0
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

struct FileInfo {
    rel: String,
    size: u64,
    notation: bool,
}

/// Opens `rel` showing lines up to `last`; returns the time to the first
/// answer, the time to the last answer of the request, and the last snapshot.
fn open(svc: &mut Service, rel: &str, last: usize) -> (f64, f64, Option<Arc<FileHighlight>>) {
    let t = Instant::now();
    let mut first = None;
    let mut snap = None;
    svc.open(1, rel, last, &mut |r| {
        if let HlResp::File { hl, .. } = r {
            first.get_or_insert_with(|| ms(t));
            snap = Some(hl);
        }
    });
    (first.unwrap_or_else(|| ms(t)), ms(t), snap)
}

/// Draw times (ms) of the source view scrolling one line per frame.
fn render_times(text: &crate::sourceview::SourceFileText, rel: &str, hl: Option<&Arc<FileHighlight>>, mode: Mode, size: (u16, u16), frames: usize) -> Vec<f64> {
    use crate::app::state::{App, CenterMode, Channels, Ctx, Focus};
    use crate::app::theme::ThemeName;
    use std::sync::atomic::{AtomicBool, AtomicU64};
    let ctx = Ctx { title: "bench".into(), mode: crate::db::IndexMode::Source, index: None, user_key: "bench".into(), editor: None, info: None, synthetic: false, project_root: None };
    let ch = Channels { query: None, search: None, jobs: None, events: None, search_latest: Arc::new(AtomicU64::new(0)), job_cancel: Arc::new(AtomicBool::new(false)) };
    let mut a = App::new(ctx, ThemeName::LeanDark, false, ch);
    a.ctx.index = Some(crate::app::worker::IndexCtx { index: "x".into(), source_root: None, mode: crate::db::IndexMode::Source, user_key: "k".into(), encoding: crate::text::PosEncoding::Utf8, map_path: None });
    a.center_mode = CenterMode::Source;
    a.focus = Focus::Center;
    a.hl.mode = mode;
    let n = text.line_count();
    a.center.source = Some(crate::app::worker::FullSource {
        rel_path: rel.to_string(),
        sha256: text.sha256.clone(),
        lines: (1..=n).map(|i| text.line(i).to_string()).collect(),
        focus_line: 1,
        highlight: Some((n / 3, n / 3 + 30)),
        changed: false,
    });
    a.hl.wanted = Some(rel.to_string());
    a.hl.file = hl.cloned();
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(size.0, size.1)).unwrap();
    let mut out = Vec::with_capacity(frames);
    let step = (n / frames.max(1)).max(1);
    for i in 0..frames.min(n) {
        a.center.source_scroll = (i * step).min(n.saturating_sub(1));
        let t = Instant::now();
        term.draw(|f| crate::app::render::draw(f, &mut a)).unwrap();
        out.push(ms(t));
    }
    out
}

pub fn cmd_bench(args: &Args) -> Result<i32> {
    let root = PathBuf::from(args.positional.get(1).context("highlight bench: expected a project directory")?).canonicalize()?;
    let project = crate::paths::Project::open(&root)?;
    let scratch = tempfile_dir()?;
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let t_disc = Instant::now();
    let (found, _) = crate::source::index::discover(&root, &cancel)?;
    let discover_ms = ms(t_disc);
    let filters = args.get_all("files");
    let mut files: Vec<FileInfo> = found
        .iter()
        .filter(|f| filters.is_empty() || filters.iter().any(|x| f.rel.starts_with(x.as_str())))
        .map(|f| FileInfo { rel: f.rel.clone(), size: f.size, notation: false })
        .collect();
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    let total_files = files.len();
    let total_bytes: u64 = files.iter().map(|f| f.size).sum();
    // A deterministic sample when asked: every k-th file plus the largest.
    let sampled = args.num::<usize>("max-files")?.filter(|&m| m < files.len());
    if let Some(m) = sampled {
        let mut by_size: Vec<usize> = (0..files.len()).collect();
        by_size.sort_by_key(|&i| std::cmp::Reverse(files[i].size));
        let mut keep: std::collections::BTreeSet<usize> = by_size.iter().take(20).copied().collect();
        let k = (files.len() / m.max(1)).max(1);
        keep.extend((0..files.len()).step_by(k));
        let mut i = 0;
        files.retain(|_| {
            i += 1;
            keep.contains(&(i - 1))
        });
    }
    eprintln!("{} local Lean files, {:.1} MiB; measuring {}", total_files, total_bytes as f64 / 1048576.0, files.len());

    if args.has("navigation-only") {
        // A fresh process doing nothing but a long navigation, so that its
        // resident memory is the navigation's: every file opened at the
        // top, then tokenized to its end, `--repeat` times over.
        let budget = args.num::<usize>("highlight-memory-mib")?.unwrap_or(128) << 20;
        let rounds = args.num::<usize>("repeat")?.unwrap_or(1).max(1);
        let mut nav = Service::new(root.clone(), scratch.join("highlight.sqlite"), project.toolchain.clone());
        nav.set_budget(budget);
        nav.set_persist(false);
        let rss_before = rss_mib();
        let (mut peak_cache, mut peak_rss, mut times) = (0usize, 0f64, Vec::new());
        let mut rss_after_round = Vec::new();
        let t = Instant::now();
        for _ in 0..rounds {
            for (i, f) in files.iter().enumerate() {
                let (_, e, _) = open(&mut nav, &f.rel, 40);
                times.push(e);
                while !nav.background_slice(&f.rel) {}
                peak_cache = peak_cache.max(nav.cache_bytes());
                if i % 64 == 0 {
                    peak_rss = peak_rss.max(rss_mib());
                }
            }
            rss_after_round.push(rss_mib());
        }
        let report = json!({
            "project": project.name, "local_files": total_files, "measured_files": files.len(), "sampled": sampled.is_some(),
            "rounds": rounds, "seconds": t.elapsed().as_secs_f64(), "open_ms": dist(times),
            "budget_mib": budget >> 20, "peak_worker_cache_mib": peak_cache as f64 / 1048576.0, "files_in_cache_at_end": nav.cached_files(),
            "rss_before_mib": rss_before, "peak_rss_mib": peak_rss.max(rss_mib()), "rss_after_each_round_mib": rss_after_round,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
        let _ = std::fs::remove_dir_all(&scratch);
        return Ok(0);
    }

    // 1. Tokenization: whole files, and the first viewport (40 lines + margin).
    let rss0 = rss_mib();
    let mut b = BaselineBuilder::new();
    let mut toks = Vec::new();
    let (mut bytes, mut lines, mut tokens) = (0u64, 0u64, 0u64);
    let mut per_file = Vec::new();
    let mut first_view = Vec::new();
    let mut slow: Vec<(f64, String, u64)> = Vec::new();
    let t_all = Instant::now();
    for f in files.iter_mut() {
        let Ok(raw) = std::fs::read(root.join(&f.rel)) else { continue };
        let (text, _) = crate::text::decode_lossy(&raw);
        f.notation = text.contains("notation") || text.contains("macro") || text.contains("syntax ");
        b.reset();
        let t = Instant::now();
        let mut view_done = false;
        for (i, line) in super::baseline::lines(&text).enumerate() {
            if i == 240 {
                first_view.push(ms(t));
                view_done = true;
            }
            b.feed(line, &mut toks);
            tokens += toks.len() as u64;
            lines += 1;
        }
        let e = ms(t);
        if !view_done {
            first_view.push(e);
        }
        bytes += text.len() as u64;
        per_file.push(e);
        slow.push((e, f.rel.clone(), f.size));
    }
    let tok_s = t_all.elapsed().as_secs_f64();
    slow.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    slow.truncate(5);
    let rss_tok = rss_mib();

    // 2. Opening files through the worker's code path.
    let store_path = scratch.join("highlight.sqlite");
    let real_store = Store::path_for_project(&project);
    let mut semantic_note = "no semantic cache for this project".to_string();
    if real_store.is_file() {
        // Read the project's semantic records from a copy.
        for suffix in ["", "-wal"] {
            let src = PathBuf::from(format!("{}{suffix}", real_store.display()));
            if src.is_file() {
                std::fs::copy(&src, PathBuf::from(format!("{}{suffix}", store_path.display())))?;
            }
        }
        if let Ok(Some(s)) = Store::open_existing(&store_path) {
            s.clear(true, false)?;
            let st = s.stats()?;
            semantic_note = format!("{} complete, {} partial, {} failed, {} skipped records ({} tokens, {:.1} MiB of token blobs)", st.complete, st.partial, st.failed, st.skipped, st.semantic_tokens, st.semantic_bytes as f64 / 1048576.0);
        }
    }
    let mut by_size: Vec<&FileInfo> = files.iter().collect();
    by_size.sort_by_key(|f| f.size);
    let mid = by_size.len() / 2;
    let ordinary: Vec<String> = by_size[mid.saturating_sub(100)..(mid + 100).min(by_size.len())].iter().map(|f| f.rel.clone()).collect();
    let largest: Vec<String> = by_size.iter().rev().take(20).map(|f| f.rel.clone()).collect();
    let notation: Vec<String> = files.iter().filter(|f| f.notation).take(50).map(|f| f.rel.clone()).collect();
    let mut open_stats = serde_json::Map::new();
    let mut svc = Service::new(root.clone(), store_path.clone(), project.toolchain.clone());
    let measure = |svc: &mut Service, name: &str, set: &[String], last: usize, stats: &mut serde_json::Map<String, Value>| {
        let (mut first, mut total) = (Vec::new(), Vec::new());
        for rel in set {
            let (f, t, _) = open(svc, rel, last);
            first.push(f);
            total.push(t);
        }
        stats.insert(name.to_string(), json!({"first_answer_ms": dist(first), "all_answers_ms": dist(total)}));
    };
    // Cold: nothing in memory or on disk; the viewport is tokenized now.
    measure(&mut svc, "ordinary files, top of file, cold", &ordinary, 40, &mut open_stats);
    measure(&mut svc, "largest files, top of file, cold", &largest, 40, &mut open_stats);
    measure(&mut svc, "files with notation/macros, top of file, cold", &notation, 40, &mut open_stats);
    // Warm: the same files again (in memory unless evicted).
    measure(&mut svc, "ordinary files again, warm in memory", &ordinary, 40, &mut open_stats);
    // A jump to the end of a large file needs every line before it.
    let mut svc2 = Service::new(root.clone(), store_path.clone(), project.toolchain.clone());
    measure(&mut svc2, "largest files, jump to the last line, cold", &largest, usize::MAX - 1000, &mut open_stats);
    // Finish them so that they are persisted, then read them in a new worker.
    for rel in &largest {
        while !svc2.background_slice(rel) {}
    }
    let mut svc3 = Service::new(root.clone(), store_path.clone(), project.toolchain.clone());
    measure(&mut svc3, "largest files, jump to the last line, new worker with spans cached on disk", &largest, usize::MAX - 1000, &mut open_stats);
    // Alternating between two files many times.
    if ordinary.len() >= 2 {
        let alt: Vec<String> = (0..200).map(|i| ordinary[i % 2].clone()).collect();
        measure(&mut svc3, "alternating between two files 200 times", &alt, 40, &mut open_stats);
    }

    // 3. Rendering the source view, headless.
    let mut render = serde_json::Map::new();
    let mut cache = crate::sourceview::SourceCache::default();
    let render_files: Vec<String> = largest.iter().take(3).chain(ordinary.iter().take(3)).cloned().collect();
    for size in [(120u16, 34u16), (200, 60)] {
        let mut per_mode: std::collections::BTreeMap<&str, Vec<f64>> = Default::default();
        for rel in &render_files {
            let Ok(text) = cache.get(&root, rel) else { continue };
            let (_, _, snap) = open(&mut svc3, rel, usize::MAX - 1000);
            while !svc3.background_slice(rel) {}
            let (_, _, snap) = if snap.as_ref().is_some_and(|s| s.baseline_complete()) { (0.0, 0.0, snap) } else { open(&mut svc3, rel, usize::MAX - 1000) };
            for (name, mode) in [("off", Mode::Off), ("syntax", Mode::Syntax), ("syntax + semantic", Mode::Semantic)] {
                per_mode.entry(name).or_default().extend(render_times(&text, rel, snap.as_ref(), mode, size, 400));
            }
        }
        let mut o = serde_json::Map::new();
        for (k, v) in per_mode {
            o.insert(k.to_string(), dist(v));
        }
        render.insert(format!("{}x{}", size.0, size.1), Value::Object(o));
    }

    // 4. A long navigation: every measured file once, memory bounded.
    // The workers above are gone, so the memory figures are this one's.
    drop(svc);
    drop(svc2);
    drop(svc3);
    let budget = args.num::<usize>("highlight-memory-mib")?.unwrap_or(128) << 20;
    let mut nav = Service::new(root.clone(), store_path.clone(), project.toolchain.clone());
    nav.set_budget(budget);
    nav.set_persist(false);
    let rss_nav0 = rss_mib();
    let (mut peak_cache, mut nav_times) = (0usize, Vec::new());
    let t_nav = Instant::now();
    for f in &files {
        let (_, t, _) = open(&mut nav, &f.rel, 40);
        nav_times.push(t);
        while !nav.background_slice(&f.rel) {}
        peak_cache = peak_cache.max(nav.cache_bytes());
    }
    let nav_s = t_nav.elapsed().as_secs_f64();
    let rss_nav1 = rss_mib();
    let nav_cached_files = nav.cached_files();
    drop(nav);

    // 5. The semantic cache: reading and decoding every record.
    let mut sem = json!({"note": semantic_note});
    if let Ok(Some(s)) = Store::open_existing(&store_path) {
        let metas = s.all_semantic_meta()?;
        let (mut load, mut decode, mut blob_bytes, mut toks) = (Vec::new(), Vec::new(), Vec::new(), 0u64);
        for m in metas.iter().filter(|m| m.n_tokens > 0) {
            let t = Instant::now();
            let Some(rec) = s.get_semantic(&m.path)? else { continue };
            let Some(blob) = rec.blob else { continue };
            blob_bytes.push(blob.len() as f64);
            let Ok(b) = Blob::<SemTok>::parse(blob) else { continue };
            load.push(ms(t));
            let t = Instant::now();
            for c in 0..b.chunk_count() {
                if let Ok(l) = b.chunk(c) {
                    toks += l.token_count() as u64;
                }
            }
            decode.push(ms(t));
        }
        sem = json!({"note": sem["note"], "records_with_tokens": load.len(), "tokens_decoded": toks,
            "read_and_parse_ms": dist(load), "decode_all_chunks_ms": dist(decode), "blob_bytes": dist(blob_bytes)});
    }

    let report = json!({
        "project": project.name, "root": root.display().to_string(),
        "local_files": total_files, "local_mib": total_bytes as f64 / 1048576.0,
        "measured_files": files.len(), "sampled": sampled.is_some(),
        "discover_ms": discover_ms,
        "tokenize": {
            "bytes": bytes, "lines": lines, "tokens": tokens, "seconds": tok_s,
            "mib_per_s": bytes as f64 / 1048576.0 / tok_s, "lines_per_s": lines as f64 / tok_s,
            "per_file_ms": dist(per_file), "first_240_lines_ms": dist(first_view),
            "slowest": slow.iter().map(|(e, r, s)| json!({"ms": e, "file": r, "bytes": s})).collect::<Vec<_>>(),
            "rss_before_mib": rss0, "rss_after_mib": rss_tok,
        },
        "open_ms": open_stats,
        "navigation": {
            "files_opened_and_fully_tokenized": files.len(), "seconds": nav_s, "open_ms": dist(nav_times),
            "budget_mib": budget >> 20, "peak_worker_cache_mib": peak_cache as f64 / 1048576.0,
            "files_in_cache_at_end": nav_cached_files, "rss_before_mib": rss_nav0, "rss_after_mib": rss_nav1,
        },
        "render_frame_ms": render,
        "semantic_cache": sem,
        "baseline_provider": super::textmate::baseline_identity(),
    });
    let text = serde_json::to_string_pretty(&report)?;
    match args.get("report") {
        Some(p) => std::fs::write(p, &text).with_context(|| format!("writing {p}"))?,
        None => println!("{text}"),
    }
    let _ = std::fs::remove_dir_all(&scratch);
    Ok(0)
}

fn tempfile_dir() -> Result<PathBuf> {
    let d = std::env::temp_dir().join(format!("slim-hlbench-{}", std::process::id()));
    std::fs::create_dir_all(&d)?;
    Ok(d)
}
