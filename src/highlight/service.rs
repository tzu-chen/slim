//! The browser's highlight worker.
//!
//! One thread owns the tokenizer, a bounded cache of recently viewed files
//! and read access to the persistent cache. The UI asks for a file and the
//! lines it is showing; answers arrive as immutable snapshots, possibly
//! several per request (visible lines first, then the whole file, then
//! freshness facts). The worker blocks when it has nothing to do, starts
//! no process and never elaborates anything.

use super::baseline::{BaselineBuilder, Suspended};
use super::codec::Blob;
use super::env::{DepTracker, EnvInfo};
use super::store::{SemMeta, Store};
use super::textmate::baseline_identity;
use super::{BaseTok, Coverage, Layer, Legend, Lines, SemTok, CHUNK_LINES};
use crate::sourceview::{SourceCache, SourceFileText};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

/// Lines tokenized beyond the requested ones before the first answer.
pub const VIEW_MARGIN: usize = 200;
/// Lines per background slice (the worker looks for new requests between slices).
const SLICE_LINES: usize = 2048;
/// Baselines of files at least this large are written to the cache.
pub const PERSIST_MIN_BYTES: usize = 32 << 10;
pub const DEFAULT_BUDGET_BYTES: usize = 128 << 20;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum EnvState {
    /// Not compared (no record, or the record's environment is unknown).
    #[default]
    Unknown,
    Same,
    Changed(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Upstream {
    /// No record to compare.
    #[default]
    NotApplicable,
    /// The check runs in the background; not yet known.
    Checking,
    /// Every imported source has the content it had when indexed.
    /// `content_verified` is false when some file was accepted from its
    /// size and modification time since an earlier check in this session.
    Unchanged { content_verified: bool },
    Changed,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum BuildState {
    #[default]
    Unknown,
    /// The imports' compiled artifacts are the ones present when indexed.
    Same,
    /// They were rebuilt or removed since.
    Changed,
}

/// Separate facts about the cached semantic tokens of the shown file. No
/// single flag: each line of the status display comes from one of these.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SemFacts {
    /// The cached record, if any (for any content of this path).
    pub record: Option<Arc<SemMeta>>,
    /// Cached positions were computed for exactly the displayed bytes.
    pub aligned: bool,
    pub env: EnvState,
    pub upstream: Upstream,
    pub build: BuildState,
    /// `lean --version` recorded by the run that made the record.
    pub lean_version: Option<String>,
    /// The record's tokens could not be read back.
    pub damaged: bool,
}

/// What may be shown, derived from the facts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Usable {
    /// Aligned, same environment, imports unchanged.
    Current,
    /// Positions align but the meaning may be outdated (environment or
    /// imports changed): shown only in the explicit stale-preview mode.
    StaleMeaning,
    /// Aligned; the import check has not finished.
    Checking,
    No,
}

impl SemFacts {
    pub fn usable(&self) -> Usable {
        let Some(r) = &self.record else { return Usable::No };
        if !self.aligned || self.damaged || !matches!(r.status, Coverage::Complete | Coverage::Partial) {
            return Usable::No;
        }
        match (&self.env, &self.upstream) {
            (EnvState::Changed(_), _) | (_, Upstream::Changed) => Usable::StaleMeaning,
            (_, Upstream::Checking) => Usable::Checking,
            (EnvState::Unknown, _) => Usable::StaleMeaning,
            _ => Usable::Current,
        }
    }

    /// A few words for the status line.
    pub fn short(&self) -> String {
        let Some(r) = &self.record else { return "no semantic tokens cached".into() };
        if !self.aligned {
            return "semantic tokens are for an older version of this file".into();
        }
        if self.damaged {
            return "cached semantic tokens unreadable".into();
        }
        match r.status {
            Coverage::Failed => return "semantic extraction failed".to_string(),
            Coverage::Skipped => return "semantic extraction skipped".to_string(),
            _ => {}
        }
        let cov = if r.status == Coverage::Partial { "partial" } else { "complete" };
        match self.usable() {
            Usable::Current => format!("semantic {cov}, current"),
            Usable::Checking => format!("semantic {cov}, checking imports"),
            Usable::StaleMeaning => format!("semantic {cov}, STALE inputs"),
            Usable::No => "semantic unavailable".into(),
        }
    }

    /// The facts, one per line, for the details overlay.
    pub fn lines(&self) -> Vec<(String, bool)> {
        let mut v: Vec<(String, bool)> = Vec::new();
        let Some(r) = &self.record else {
            v.push(("Semantic tokens: none cached for this file (syntax colouring only).".into(), false));
            return v;
        };
        v.push((
            if self.aligned {
                "Source alignment: cached token positions were computed for exactly these bytes (SHA-256 equal).".into()
            } else {
                "Source alignment: the file CHANGED since its tokens were extracted; they are not applied.".into()
            },
            !self.aligned,
        ));
        v.push((
            match r.status {
                Coverage::Complete => format!("Extraction coverage: complete — {} ({} tokens).", r.detail, r.n_tokens),
                Coverage::Partial => format!("Extraction coverage: PARTIAL — {} ({} tokens).", r.detail, r.n_tokens),
                Coverage::Failed => format!("Extraction coverage: FAILED — {}.", r.detail),
                Coverage::Skipped => format!("Extraction coverage: skipped — {}.", r.detail),
            },
            r.status != Coverage::Complete,
        ));
        if r.diag_errors > 0 {
            v.push((format!("  Lean reported {} error(s) in this file when it was processed; coverage is not verification.", r.diag_errors), true));
        }
        v.push(match &self.env {
            EnvState::Same => ("Semantic environment: toolchain, Lake configuration and dependency manifest are as recorded.".into(), false),
            EnvState::Changed(why) => (format!("Semantic environment: CHANGED since extraction — {}.", why.join("; ")), true),
            EnvState::Unknown => ("Semantic environment: unknown (the recorded environment is not available).".into(), true),
        });
        v.push(match &self.upstream {
            Upstream::Unchanged { content_verified: true } => (format!("Imported sources: unchanged ({} local modules, contents hashed now).", r.deps_count), false),
            Upstream::Unchanged { content_verified: false } => (
                format!("Imported sources: unchanged ({} local modules; some accepted by size and time since an earlier check — R hashes all again).", r.deps_count),
                false,
            ),
            Upstream::Changed => ("Imported sources: CHANGED since extraction — positions still align, the meaning may not.".into(), true),
            Upstream::Checking => ("Imported sources: being checked…".into(), false),
            Upstream::NotApplicable => ("Imported sources: not compared.".into(), false),
        });
        v.push(match &self.build {
            BuildState::Same => (format!("Build relation: compiled imports are the ones present at extraction. At extraction: {}.", r.build_note), false),
            BuildState::Changed => (format!("Build relation: compiled imports were rebuilt or removed since extraction. At extraction: {}.", r.build_note), true),
            BuildState::Unknown => (format!("Build relation: not verifiable now. At extraction: {}.", r.build_note), false),
        });
        v.push((
            format!(
                "Extracted {} with {} ({}, positions {}), in {:.1} s.",
                r.created,
                self.lean_version.as_deref().unwrap_or("an unrecorded Lean version"),
                r.extractor,
                r.position_encoding,
                r.elapsed_ms as f64 / 1000.0
            ),
            false,
        ));
        v
    }
}

pub struct SemLayer {
    pub layer: Arc<Layer<SemTok>>,
    pub legend: Arc<Legend>,
    /// For partial records: lines the server had processed.
    pub processed_lines: Option<usize>,
}

/// An immutable snapshot of one file's highlight data.
pub struct FileHighlight {
    pub rel: String,
    /// Hash of the text the spans belong to; the UI applies a snapshot only
    /// to text with the same hash.
    pub sha256: String,
    pub n_lines: usize,
    pub baseline: Arc<Layer<BaseTok>>,
    /// Lines `0..baseline_lines_done` have baseline tokens.
    pub baseline_lines_done: usize,
    pub baseline_from_cache: bool,
    /// Present when a record's tokens align with this text. Whether they
    /// are shown is decided from `facts` and the reader's settings.
    pub semantic: Option<SemLayer>,
    pub facts: SemFacts,
    /// Memory held by the worker's file cache when this was made.
    pub cache_bytes: usize,
}

impl FileHighlight {
    pub fn baseline_complete(&self) -> bool {
        self.baseline_lines_done >= self.n_lines
    }
}

pub enum HlReq {
    /// Highlight data for a file, lines `first..=last` (0-based) first.
    Open { gen: u64, rel: String, first: usize, last: usize },
    /// Compare environment and imports again; `exact` forgets remembered
    /// hashes so every file is read.
    Recheck { exact: bool },
    /// The persistent cache changed for this path (or for all).
    Invalidate { rel: Option<String> },
    SetBudget(usize),
    Shutdown,
}

pub enum HlResp {
    File { gen: u64, hl: Arc<FileHighlight> },
    Error { gen: u64, message: String },
}

struct Entry {
    text: SourceFileText,
    layer: Layer<BaseTok>,
    frozen: Option<Arc<Layer<BaseTok>>>,
    /// Tokenizer position; `None` once complete.
    resume: Option<Suspended>,
    cur: Lines<BaseTok>,
    from_cache: bool,
    semantic: Option<(Arc<Layer<SemTok>>, Arc<Legend>, Option<usize>)>,
    facts: SemFacts,
    sem_loaded: bool,
    last_used: u64,
}

impl Entry {
    fn done(&self) -> usize {
        match &self.resume {
            Some(s) => s.lines_done(),
            None => self.text.line_count(),
        }
    }
    fn bytes(&self) -> usize {
        self.text.text.len()
            + self.frozen.as_ref().map(|l| l.heap_bytes()).unwrap_or_else(|| self.layer.heap_bytes())
            + self.cur.heap_bytes()
            + self.semantic.as_ref().map(|s| s.0.heap_bytes()).unwrap_or(0)
            + 512
    }
}

pub struct Service {
    root: PathBuf,
    store_path: PathBuf,
    store: Option<Store>,
    store_failed: Option<String>,
    sources: SourceCache,
    builder: BaselineBuilder,
    cache: HashMap<String, Entry>,
    clock: u64,
    budget: usize,
    tracker: DepTracker,
    env_now: Option<EnvInfo>,
    persist: bool,
}

impl Service {
    pub fn new(root: PathBuf, store_path: PathBuf, toolchain: Option<String>) -> Service {
        Service {
            tracker: DepTracker::new(&root, toolchain.as_deref()),
            root,
            store_path,
            store: None,
            store_failed: None,
            sources: SourceCache::default(),
            builder: BaselineBuilder::new(),
            cache: HashMap::new(),
            clock: 0,
            budget: DEFAULT_BUDGET_BYTES,
            env_now: None,
            persist: true,
        }
    }

    pub fn set_budget(&mut self, bytes: usize) {
        self.budget = bytes;
    }

    /// Turns off writing baselines to the persistent cache.
    pub fn set_persist(&mut self, on: bool) {
        self.persist = on;
    }

    pub fn cache_bytes(&self) -> usize {
        self.cache.values().map(|e| e.bytes()).sum()
    }

    pub fn cached_files(&self) -> usize {
        self.cache.len()
    }

    fn store(&mut self, create: bool) -> Option<&Store> {
        if self.store.is_none() && self.store_failed.is_none() {
            let r = if create { Store::open(&self.store_path).map(Some) } else { Store::open_existing(&self.store_path) };
            match r {
                Ok(s) => self.store = s,
                Err(e) => self.store_failed = Some(format!("{e:#}")),
            }
        }
        self.store.as_ref()
    }

    fn evict(&mut self, keep: &str) {
        while self.cache.len() > 1 && self.cache_bytes() > self.budget {
            let Some(victim) = self.cache.iter().filter(|(k, _)| k.as_str() != keep).min_by_key(|(_, e)| e.last_used).map(|(k, _)| k.clone()) else { break };
            self.cache.remove(&victim);
        }
    }

    fn snapshot(&self, rel: &str) -> Option<Arc<FileHighlight>> {
        let e = self.cache.get(rel)?;
        let baseline = match &e.frozen {
            Some(l) => l.clone(),
            None => {
                let mut l = e.layer.clone();
                if !e.cur.is_empty() {
                    l.set_chunk(e.done() / CHUNK_LINES, Arc::new(e.cur.clone()));
                }
                Arc::new(l)
            }
        };
        Some(Arc::new(FileHighlight {
            rel: rel.to_string(),
            sha256: e.text.sha256.clone(),
            n_lines: e.text.line_count(),
            baseline,
            baseline_lines_done: e.done(),
            baseline_from_cache: e.from_cache,
            semantic: e.semantic.as_ref().map(|(l, lg, p)| SemLayer { layer: l.clone(), legend: lg.clone(), processed_lines: *p }),
            facts: e.facts.clone(),
            cache_bytes: self.cache_bytes(),
        }))
    }

    /// Makes sure `rel` has an entry for the file's current content.
    fn ensure_entry(&mut self, rel: &str) -> anyhow::Result<()> {
        let text = self.sources.get(&self.root, rel)?;
        self.clock += 1;
        if let Some(e) = self.cache.get_mut(rel) {
            if e.text.sha256 == text.sha256 {
                e.last_used = self.clock;
                return Ok(());
            }
        }
        let n = text.line_count();
        let mut entry = Entry {
            layer: Layer::pending(n),
            frozen: None,
            resume: Some(Suspended::default()),
            cur: Lines::default(),
            from_cache: false,
            semantic: None,
            facts: SemFacts::default(),
            sem_loaded: false,
            last_used: self.clock,
            text,
        };
        let provider = baseline_identity();
        if entry.text.text.len() >= PERSIST_MIN_BYTES {
            if let Some(blob) = self.store(false).and_then(|s| s.get_baseline(rel, &entry.text.sha256, provider).ok().flatten()) {
                if let Ok(b) = Blob::<BaseTok>::parse(blob) {
                    if b.n_lines() == n {
                        entry.frozen = Some(Arc::new(Layer::from_blob(b)));
                        entry.resume = None;
                        entry.from_cache = true;
                    }
                }
            }
        }
        self.cache.insert(rel.to_string(), entry);
        Ok(())
    }

    /// Loads the semantic record and the facts that need no file reading
    /// beyond the record itself.
    fn load_semantic(&mut self, rel: &str) {
        if self.cache.get(rel).is_none_or(|e| e.sem_loaded) {
            return;
        }
        let rec = self.store(false).and_then(|s| s.get_semantic(rel).ok().flatten());
        let mut facts = SemFacts::default();
        let mut layer = None;
        if let Some(rec) = rec {
            let (sha, n_lines, valid) = {
                let e = &self.cache[rel];
                (e.text.sha256.clone(), e.text.line_count(), e.text.valid_utf8)
            };
            facts.aligned = rec.meta.source_sha256 == sha && valid;
            let env_rec = self.store(false).and_then(|s| s.get_env(&rec.meta.env_fingerprint).ok().flatten());
            facts.lean_version = env_rec.as_ref().and_then(|e| e.lean_version.clone());
            if self.env_now.is_none() {
                self.env_now = Some(EnvInfo::current(&self.root));
            }
            let now = self.env_now.as_ref().unwrap();
            facts.env = match env_rec {
                Some(then) => {
                    let ch = now.changes_since(&then.info);
                    if ch.is_empty() && now.fingerprint() == rec.meta.env_fingerprint {
                        EnvState::Same
                    } else if ch.is_empty() {
                        EnvState::Changed(vec!["recorded environment differs".into()])
                    } else {
                        EnvState::Changed(ch)
                    }
                }
                None => EnvState::Unknown,
            };
            if facts.aligned && matches!(rec.meta.status, Coverage::Complete | Coverage::Partial) {
                facts.upstream = Upstream::Checking;
                match rec.blob.map(Blob::<SemTok>::parse) {
                    Some(Ok(b)) if b.n_lines() == n_lines => {
                        layer = Some((Arc::new(Layer::from_blob(b)), Arc::new(rec.meta.legend.clone()), rec.meta.processed_lines));
                    }
                    _ => facts.damaged = true,
                }
            }
            facts.record = Some(Arc::new(rec.meta));
        }
        let e = self.cache.get_mut(rel).unwrap();
        e.semantic = layer;
        e.facts = facts;
        e.sem_loaded = true;
    }

    /// The part of the freshness check that reads imported files.
    fn check_upstream(&mut self, rel: &str) -> bool {
        let Some(e) = self.cache.get(rel) else { return false };
        if e.facts.upstream != Upstream::Checking {
            return false;
        }
        let Some(rec) = e.facts.record.clone() else { return false };
        let text = e.text.text.clone();
        let closure = self.tracker.closure(rel, Some(&text));
        let art = self.tracker.artifacts(&closure.files);
        let e = self.cache.get_mut(rel).unwrap();
        e.facts.upstream = if closure.digest == rec.deps_digest { Upstream::Unchanged { content_verified: closure.content_verified } } else { Upstream::Changed };
        e.facts.build = if art.digest.is_empty() || rec.artifacts_digest.is_empty() {
            BuildState::Unknown
        } else if art.digest == rec.artifacts_digest {
            BuildState::Same
        } else {
            BuildState::Changed
        };
        true
    }

    /// Tokenizes `rel` up to line `upto` (exclusive) or `max_lines` more
    /// lines, whichever is less. Returns true when the file is complete.
    fn tokenize(&mut self, rel: &str, upto: usize, max_lines: usize) -> bool {
        let Some(e) = self.cache.get_mut(rel) else { return true };
        let Some(sus) = e.resume.take() else { return true };
        let n = e.text.line_count();
        let target = upto.min(n).min(sus.lines_done() + max_lines);
        self.builder.resume(sus);
        let mut toks = Vec::new();
        while self.builder.lines_done() < target {
            let i = self.builder.lines_done();
            self.builder.feed(e.text.line(i + 1), &mut toks);
            e.cur.push_line(toks.drain(..));
            if e.cur.len() == CHUNK_LINES {
                e.layer.set_chunk(i / CHUNK_LINES, Arc::new(std::mem::take(&mut e.cur)));
            }
        }
        if self.builder.lines_done() >= n {
            if !e.cur.is_empty() {
                e.layer.set_chunk((n - 1) / CHUNK_LINES, Arc::new(std::mem::take(&mut e.cur)));
            }
            self.builder.reset();
            let layer = Arc::new(std::mem::replace(&mut e.layer, Layer::pending(0)));
            e.frozen = Some(layer.clone());
            let (len, sha) = (e.text.text.len(), e.text.sha256.clone());
            if self.persist && len >= PERSIST_MIN_BYTES {
                if let Some(chunks) = layer.all_chunks() {
                    let blob = super::codec::encode_baseline(&chunks, n);
                    if let Some(s) = self.store(true) {
                        let _ = s.put_baseline(rel, &sha, baseline_identity(), n, &blob);
                    }
                }
            }
            true
        } else {
            e.resume = Some(self.builder.suspend());
            false
        }
    }

    /// Handles one `Open`: sends at least one answer through `send`.
    /// Returns the path if background work (the rest of the file) remains.
    pub fn open(&mut self, gen: u64, rel: &str, last: usize, send: &mut dyn FnMut(HlResp)) -> Option<String> {
        if let Err(e) = self.ensure_entry(rel) {
            send(HlResp::Error { gen, message: format!("{e:#}") });
            return None;
        }
        self.load_semantic(rel);
        let complete = self.tokenize(rel, last.saturating_add(1 + VIEW_MARGIN), usize::MAX);
        if let Some(hl) = self.snapshot(rel) {
            send(HlResp::File { gen, hl });
        }
        if self.check_upstream(rel) {
            if let Some(hl) = self.snapshot(rel) {
                send(HlResp::File { gen, hl });
            }
        }
        self.evict(rel);
        (!complete).then(|| rel.to_string())
    }

    /// One slice of background tokenization; true when the file is done.
    pub fn background_slice(&mut self, rel: &str) -> bool {
        let done = self.tokenize(rel, usize::MAX, SLICE_LINES);
        // The file grew in the cache: keep the others within the budget.
        self.evict(rel);
        done
    }

    pub fn recheck(&mut self, exact: bool) {
        self.env_now = None;
        if exact {
            self.tracker.forget();
        }
        self.sources = SourceCache::default();
        for e in self.cache.values_mut() {
            e.sem_loaded = false;
        }
    }

    pub fn invalidate(&mut self, rel: Option<&str>) {
        // The cache file may have been created since the last look.
        if self.store.is_none() {
            self.store_failed = None;
        }
        match rel {
            Some(r) => {
                if let Some(e) = self.cache.get_mut(r) {
                    e.sem_loaded = false;
                }
            }
            None => {
                for e in self.cache.values_mut() {
                    e.sem_loaded = false;
                }
            }
        }
    }
}

/// Starts the worker thread. It ends at `Shutdown` or when either channel
/// closes.
pub fn spawn<E: Send + 'static>(
    root: PathBuf,
    store_path: PathBuf,
    toolchain: Option<String>,
    budget: usize,
    tx: Sender<E>,
    wrap: fn(HlResp) -> E,
) -> (Sender<HlReq>, std::thread::JoinHandle<()>) {
    let (rtx, rrx) = std::sync::mpsc::channel::<HlReq>();
    let h = std::thread::Builder::new()
        .name("slim-highlight".into())
        .spawn(move || {
            let mut svc = Service::new(root, store_path, toolchain);
            svc.set_budget(budget);
            run(&mut svc, rrx, &mut |r| tx.send(wrap(r)).is_ok());
        })
        .expect("spawn highlight worker");
    (rtx, h)
}

fn run(svc: &mut Service, rx: Receiver<HlReq>, send: &mut dyn FnMut(HlResp) -> bool) {
    // (generation, path) of the file whose remaining lines are being tokenized.
    let mut background: Option<(u64, String)> = None;
    loop {
        // Block when idle; while tokenizing, only look.
        let req = match &background {
            None => match rx.recv() {
                Ok(r) => Some(r),
                Err(_) => return,
            },
            Some(_) => match rx.try_recv() {
                Ok(r) => Some(r),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
            },
        };
        match req {
            Some(HlReq::Shutdown) => return,
            Some(HlReq::Open { gen, rel, first: _, last }) => {
                let t = Instant::now();
                let mut ok = true;
                let rest = svc.open(gen, &rel, last, &mut |r| ok &= send(r));
                crate::app::timing::record("highlight: open to first spans", t.elapsed().as_secs_f64() * 1000.0);
                if !ok {
                    return;
                }
                // The newest request decides what is finished in the background.
                background = rest.map(|r| (gen, r));
            }
            Some(HlReq::Recheck { exact }) => svc.recheck(exact),
            Some(HlReq::Invalidate { rel }) => svc.invalidate(rel.as_deref()),
            Some(HlReq::SetBudget(b)) => svc.set_budget(b),
            None => {
                let (gen, rel) = background.clone().unwrap();
                if svc.background_slice(&rel) {
                    background = None;
                    if let Some(hl) = svc.snapshot(&rel) {
                        if !send(HlResp::File { gen, hl }) {
                            return;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::highlight::store::{test_meta, SemRecord};
    use crate::highlight::textmate::scopes;

    fn write(root: &std::path::Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn open(svc: &mut Service, rel: &str, last: usize) -> (Vec<Arc<FileHighlight>>, Option<String>) {
        let mut out = Vec::new();
        let rest = svc.open(1, rel, last, &mut |r| match r {
            HlResp::File { hl, .. } => out.push(hl),
            HlResp::Error { message, .. } => panic!("{message}"),
        });
        (out, rest)
    }

    #[test]
    fn visible_lines_first_then_the_rest_and_state_is_carried() {
        let d = tempfile::tempdir().unwrap();
        // A block comment opened on line 1 and closed far below the first
        // viewport: lines after it must still know they are code.
        let mut text = String::from("/- start\n");
        for i in 0..3000 {
            text.push_str(&format!("comment line {i}\n"));
        }
        text.push_str("-/\ndef x := 1\n");
        write(d.path(), "A.lean", &text);
        let mut svc = Service::new(d.path().to_path_buf(), d.path().join("cache/highlight.sqlite"), None);
        let (snaps, rest) = open(&mut svc, "A.lean", 30);
        let first = &snaps[0];
        assert_eq!(first.baseline_lines_done, 31 + VIEW_MARGIN);
        assert!(!first.baseline_complete());
        assert_eq!(rest.as_deref(), Some("A.lean"));
        let scope_of = |hl: &FileHighlight, n: usize| scopes().display(hl.baseline.line(n).unwrap()[0].1);
        assert!(scope_of(first, 100).contains("comment.block.lean4"));
        // Background slices finish the file; a line inside the comment far
        // down and the code after it are both right.
        while !svc.background_slice("A.lean") {}
        let done = svc.snapshot("A.lean").unwrap();
        assert!(done.baseline_complete());
        assert!(scope_of(&done, 2999).contains("comment.block.lean4"));
        let def_line = done.baseline.line(3002).unwrap();
        assert!(scopes().display(def_line[0].1).contains("keyword"), "{:?}", scopes().display(def_line[0].1));
        // The earlier snapshot is unchanged (snapshots are immutable).
        assert_eq!(first.baseline_lines_done, 31 + VIEW_MARGIN);
        // It was large enough to be persisted: a new worker reads it back
        // without tokenizing.
        assert!(text.len() >= PERSIST_MIN_BYTES);
        let mut svc2 = Service::new(d.path().to_path_buf(), d.path().join("cache/highlight.sqlite"), None);
        let (snaps2, rest2) = open(&mut svc2, "A.lean", 10);
        assert!(snaps2[0].baseline_from_cache && snaps2[0].baseline_complete() && rest2.is_none());
        for n in [0usize, 100, 2999, 3001, 3002] {
            assert_eq!(snaps2[0].baseline.line(n).unwrap(), done.baseline.line(n).unwrap(), "line {n}");
        }
    }

    #[test]
    fn edited_file_gets_new_spans_and_old_semantic_tokens_are_not_applied() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "A.lean", "def a := 1\n");
        let store_path = d.path().join("cache/highlight.sqlite");
        let sha = crate::paths::sha256_hex(b"def a := 1\n");
        {
            let s = Store::open(&store_path).unwrap();
            let mut m = test_meta("A.lean", &sha, Coverage::Complete);
            m.n_lines = 2;
            m.n_tokens = 1;
            let mut l = Lines::default();
            l.push_line([SemTok { start: 4, end: 5, ty: 1, mods: 0 }]);
            l.push_line([]);
            let blob = crate::highlight::codec::encode_semantic(&[Arc::new(l)], 2);
            s.put_semantic(&SemRecord { meta: m, blob: Some(blob) }).unwrap();
        }
        let mut svc = Service::new(d.path().to_path_buf(), store_path, None);
        let (snaps, _) = open(&mut svc, "A.lean", 5);
        let last = snaps.last().unwrap();
        assert!(last.facts.aligned);
        assert!(last.semantic.is_some());
        assert_eq!(last.semantic.as_ref().unwrap().layer.line(0).unwrap().len(), 1);
        // The recorded environment is not in the cache: never called current.
        assert_eq!(last.facts.env, EnvState::Unknown);
        assert_eq!(last.facts.usable(), Usable::StaleMeaning);
        // The source is edited: a token shift would put colours on the
        // wrong characters, so the old tokens are dropped entirely.
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(d.path(), "A.lean", "def longer_name := 1\n");
        let (snaps, _) = open(&mut svc, "A.lean", 5);
        let last = snaps.last().unwrap();
        assert_ne!(last.sha256, sha);
        assert!(!last.facts.aligned);
        assert!(last.semantic.is_none());
        assert_eq!(last.facts.usable(), Usable::No);
        assert!(last.facts.short().contains("older version"));
        assert!(last.baseline_complete());
    }

    #[test]
    fn damaged_semantic_blob_falls_back_to_baseline() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "A.lean", "def a := 1\n");
        let store_path = d.path().join("highlight.sqlite");
        let sha = crate::paths::sha256_hex(b"def a := 1\n");
        let s = Store::open(&store_path).unwrap();
        s.put_semantic(&SemRecord { meta: test_meta("A.lean", &sha, Coverage::Complete), blob: Some(b"LNS1\x01garbage".to_vec()) }).unwrap();
        let mut svc = Service::new(d.path().to_path_buf(), store_path, None);
        let (snaps, _) = open(&mut svc, "A.lean", 5);
        let last = snaps.last().unwrap();
        assert!(last.facts.damaged && last.semantic.is_none());
        assert_eq!(last.facts.usable(), Usable::No);
        assert!(last.baseline_complete());
    }

    #[test]
    fn memory_is_bounded_by_evicting_least_recent_files() {
        let d = tempfile::tempdir().unwrap();
        let body = "theorem t (x : Nat) : x = x := rfl\n".repeat(400);
        for i in 0..12 {
            write(d.path(), &format!("F{i}.lean"), &body);
        }
        let mut svc = Service::new(d.path().to_path_buf(), d.path().join("highlight.sqlite"), None);
        svc.set_persist(false);
        svc.set_budget(200 << 10);
        for i in 0..12 {
            let (_, rest) = open(&mut svc, &format!("F{i}.lean"), 400);
            assert!(rest.is_none());
            assert!(svc.cache_bytes() <= (200 << 10) + body.len() * 6, "{}", svc.cache_bytes());
        }
        assert!(svc.cached_files() < 12, "older files were evicted");
        assert!(svc.cache.contains_key("F11.lean"), "the file being shown stays");
        // An evicted file simply tokenizes again.
        let (snaps, _) = open(&mut svc, "F0.lean", 400);
        assert!(snaps[0].baseline_complete());
    }

    #[test]
    fn facts_distinguish_alignment_environment_imports_and_coverage() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(r, "lean-toolchain", "leanprover/lean4:v0.0.0-none\n");
        write(r, "lakefile.toml", "name = \"p\"\n");
        write(r, "P/A.lean", "import P.B\ndef a := b\n");
        write(r, "P/B.lean", "def b := 1\n");
        let store_path = r.join(".cache/highlight.sqlite");
        let store = Store::open(&store_path).unwrap();
        let env = crate::highlight::env::EnvRecord { info: EnvInfo::current(r), lean_version: Some("Lean (version 0)".into()), ..Default::default() };
        let fp = store.put_env(&env).unwrap();
        let mut tracker = DepTracker::new(r, None);
        let text = std::fs::read_to_string(r.join("P/A.lean")).unwrap();
        let mut m = test_meta("P/A.lean", &crate::paths::sha256_hex(text.as_bytes()), Coverage::Partial);
        m.env_fingerprint = fp;
        m.deps_digest = tracker.closure("P/A.lean", None).digest;
        m.deps_count = 1;
        m.n_lines = 3;
        m.detail = "time limit".into();
        let mut l = Lines::default();
        for _ in 0..3 {
            l.push_line([]);
        }
        store.put_semantic(&SemRecord { meta: m, blob: Some(crate::highlight::codec::encode_semantic(&[Arc::new(l)], 3)) }).unwrap();

        let mut svc = Service::new(r.to_path_buf(), store_path, None);
        let (snaps, _) = open(&mut svc, "P/A.lean", 5);
        // First answer: aligned and same environment, imports still being
        // checked, so not yet called current.
        assert_eq!(snaps[0].facts.upstream, Upstream::Checking);
        assert_eq!(snaps[0].facts.usable(), Usable::Checking);
        let f = &snaps.last().unwrap().facts;
        assert_eq!((f.aligned, &f.env), (true, &EnvState::Same));
        assert_eq!(f.upstream, Upstream::Unchanged { content_verified: true });
        assert_eq!(f.usable(), Usable::Current);
        assert!(f.short().contains("partial"), "coverage is its own fact: {}", f.short());
        assert_eq!(f.build, BuildState::Unknown);

        // An imported module changes: positions still align, meaning may not.
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(r, "P/B.lean", "def b := 2\n");
        svc.recheck(false);
        let (snaps, _) = open(&mut svc, "P/A.lean", 5);
        let f = &snaps.last().unwrap().facts;
        assert!(f.aligned);
        assert_eq!(f.upstream, Upstream::Changed);
        assert_eq!(f.usable(), Usable::StaleMeaning);
        assert!(snaps.last().unwrap().semantic.is_some(), "available for the explicit stale preview only");

        // The toolchain file changes as well.
        write(r, "P/B.lean", "def b := 1\n");
        write(r, "lean-toolchain", "leanprover/lean4:v9.9.9-none\n");
        svc.recheck(true);
        let (snaps, _) = open(&mut svc, "P/A.lean", 5);
        let f = &snaps.last().unwrap().facts;
        assert!(matches!(&f.env, EnvState::Changed(w) if w[0].contains("lean-toolchain")));
        assert_eq!(f.upstream, Upstream::Unchanged { content_verified: true });
        assert_eq!(f.usable(), Usable::StaleMeaning);
        let text: Vec<String> = f.lines().into_iter().map(|l| l.0).collect();
        assert!(text.iter().any(|l| l.starts_with("Source alignment")));
        assert!(text.iter().any(|l| l.contains("PARTIAL")));
        assert!(text.iter().any(|l| l.contains("Semantic environment: CHANGED")));
    }
}
