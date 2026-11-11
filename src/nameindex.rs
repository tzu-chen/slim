//! In-memory declaration-name search.
//!
//! All display names are lower-cased (ASCII only, so byte offsets are
//! preserved) and stored in one contiguous buffer sorted by name. Exact and
//! prefix matches use binary search; substring matches use a vectorised
//! `memmem` scan of the whole buffer (names are newline-separated, so a hit
//! never spans two names); fuzzy subsequence matching runs last, only when
//! the earlier tiers did not fill the page, under a time budget. A top-k
//! heap avoids sorting the full candidate set on every keystroke.
//!
//! The index is persisted next to the SQLite file (`<index>.names`) and
//! tied to it by a generation id, so warm opening does not rebuild it.
//!
//! A search can be restricted to a [`Scope`]: a namespace (names are sorted,
//! so that is one contiguous range of the buffer) or a set of modules (each
//! entry carries its module id, tested against a bit set).

use crate::model::{flags, DeclKind, KindFilter, Origin};
use anyhow::{bail, Context, Result};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::{Duration, Instant};

const MAGIC: &[u8; 8] = b"LNAVNIX\x01";
/// Version 2 added the module id of every entry (for scoped search).
const VERSION: u32 = 2;

/// Packed per-entry metadata.
#[inline]
fn pack(kind: DeclKind, origin: Origin, fl: u32) -> u32 {
    let mut m = (kind as u32) | ((origin as u32) << 4);
    if fl & flags::GENERATED != 0 {
        m |= 1 << 8;
    }
    if fl & flags::PRIVATE != 0 {
        m |= 1 << 9;
    }
    if fl & flags::ANONYMOUS != 0 {
        m |= 1 << 10;
    }
    m
}
#[inline]
fn m_kind(m: u32) -> DeclKind {
    DeclKind::from_u8((m & 0xf) as u8)
}
#[inline]
fn m_origin(m: u32) -> Origin {
    Origin::from_u8(((m >> 4) & 0xf) as u8)
}
#[inline]
fn m_generated(m: u32) -> bool {
    m & (1 << 8) != 0
}

pub struct NameIndex {
    pub generation: String,
    buf: Vec<u8>,
    starts: Vec<u32>,
    ids: Vec<u32>,
    last: Vec<u16>,
    meta: Vec<u32>,
    by_last: Vec<u32>,
    /// Module id of each entry in the index database (0: none).
    module: Vec<u32>,
}

/// A set of module ids.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModuleSet {
    bits: Vec<u64>,
    count: usize,
}

impl ModuleSet {
    pub fn from_ids(ids: impl IntoIterator<Item = u32>) -> ModuleSet {
        let mut s = ModuleSet::default();
        for id in ids {
            let (w, b) = (id as usize / 64, id % 64);
            if w >= s.bits.len() {
                s.bits.resize(w + 1, 0);
            }
            if s.bits[w] >> b & 1 == 0 {
                s.bits[w] |= 1 << b;
                s.count += 1;
            }
        }
        s
    }
    #[inline]
    pub fn contains(&self, id: u32) -> bool {
        self.bits.get(id as usize / 64).is_some_and(|w| w >> (id % 64) & 1 == 1)
    }
    pub fn len(&self) -> usize {
        self.count
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// What a search is restricted to.
#[derive(Clone, Debug)]
pub enum Scope {
    /// The declaration named so and everything below it (`Ns` and `Ns.…`),
    /// compared like all names here: ASCII case-insensitively.
    Namespace(String),
    /// Declarations of these modules.
    Modules(std::sync::Arc<ModuleSet>),
}

/// A scope as the reader chose it, before module names are resolved to ids.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ScopeSpec {
    pub kind: ScopeKind,
    /// Namespace, or dotted module-name prefix (a file or a directory).
    pub prefix: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ScopeKind {
    Namespace,
    Modules,
}

impl ScopeSpec {
    /// `name` is `prefix` itself or below it.
    pub fn covers(prefix: &str, name: &str) -> bool {
        name.len() >= prefix.len()
            && name.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
            && (name.len() == prefix.len() || name.as_bytes()[prefix.len()] == b'.')
    }

    /// Whether a declaration with this display name and module is in scope.
    pub fn accepts(&self, display: &str, module: Option<&str>) -> bool {
        match self.kind {
            ScopeKind::Namespace => Self::covers(&self.prefix, display),
            ScopeKind::Modules => module.is_some_and(|m| Self::covers(&self.prefix, m)),
        }
    }

    /// One level wider (`A.B.C` becomes `A.B`), or `None` at the top.
    pub fn widened(&self) -> Option<ScopeSpec> {
        let comps = crate::model::name_components(&self.prefix);
        if comps.len() <= 1 {
            return None;
        }
        Some(ScopeSpec { kind: self.kind, prefix: comps[..comps.len() - 1].join(".") })
    }

    pub fn label(&self) -> String {
        match self.kind {
            ScopeKind::Namespace => format!("namespace {}", self.prefix),
            ScopeKind::Modules => format!("files {}", self.prefix),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MatchTier {
    Exact,
    ExactLast,
    Prefix,
    LastPrefix,
    ComponentPrefix,
    Substring,
    AllTerms,
    Fuzzy,
    /// Not matched against a query: listed because it is in the scope.
    Listed,
}

impl MatchTier {
    fn base(self) -> i32 {
        match self {
            MatchTier::Exact => 100_000,
            MatchTier::ExactLast => 90_000,
            MatchTier::Prefix => 80_000,
            MatchTier::LastPrefix => 70_000,
            MatchTier::ComponentPrefix => 60_000,
            MatchTier::Substring => 50_000,
            MatchTier::AllTerms => 45_000,
            MatchTier::Fuzzy => 10_000,
            MatchTier::Listed => 0,
        }
    }
    /// Compact tag for dense result lists (legend in the help screen).
    pub fn tag(self) -> &'static str {
        match self {
            MatchTier::Exact => "exact",
            MatchTier::ExactLast => "name",
            MatchTier::Prefix | MatchTier::LastPrefix => "prefix",
            MatchTier::ComponentPrefix => "word",
            MatchTier::Substring => "substr",
            MatchTier::AllTerms => "words",
            MatchTier::Fuzzy => "fuzzy",
            MatchTier::Listed => "scope",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            MatchTier::Exact => "exact",
            MatchTier::ExactLast => "exact name",
            MatchTier::Prefix => "prefix",
            MatchTier::LastPrefix => "name prefix",
            MatchTier::ComponentPrefix => "word prefix",
            MatchTier::Substring => "substring",
            MatchTier::AllTerms => "all words",
            MatchTier::Fuzzy => "fuzzy",
            MatchTier::Listed => "in scope",
        }
    }
}

#[derive(Clone, Debug)]
pub struct SearchParams {
    pub query: String,
    pub local_only: bool,
    pub show_generated: bool,
    pub kinds: KindFilter,
    pub limit: usize,
    pub fuzzy_budget: Duration,
    /// Restrict the search; with an empty query the scope's first entries
    /// are listed in name order.
    pub scope: Option<Scope>,
}

impl Default for SearchParams {
    fn default() -> Self {
        SearchParams {
            query: String::new(),
            local_only: false,
            show_generated: false,
            kinds: KindFilter::All,
            limit: 200,
            fuzzy_budget: Duration::from_millis(30),
            scope: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub id: u32,
    pub score: i32,
    pub tier: MatchTier,
}

#[derive(Clone, Debug, Default)]
pub struct SearchOutcome {
    pub hits: Vec<Hit>,
    /// Matching entries after filters (a lower bound when `total_capped`).
    pub total: usize,
    pub total_capped: bool,
    pub filtered_out: usize,
    pub fuzzy_ran: bool,
    pub fuzzy_truncated: bool,
    pub cancelled: bool,
    pub elapsed: Duration,
}

/// Entry handed to [`NameIndex::build`].
pub struct Entry {
    pub id: u32,
    pub display: String,
    pub kind: DeclKind,
    pub origin: Origin,
    pub flags: u32,
    /// Module id in the index database (0: none).
    pub module: u32,
}

fn lower_ascii(s: &str) -> Vec<u8> {
    s.bytes().map(|b| b.to_ascii_lowercase()).collect()
}

fn last_start(name: &[u8]) -> usize {
    // last '.' outside «»; names are lower-cased display names
    let mut last = 0;
    let mut in_q = false;
    let mut i = 0;
    while i < name.len() {
        if name[i..].starts_with("«".as_bytes()) {
            in_q = true;
            i += 2;
            continue;
        }
        if name[i..].starts_with("»".as_bytes()) {
            in_q = false;
            i += 2;
            continue;
        }
        if name[i] == b'.' && !in_q {
            last = i + 1;
        }
        i += 1;
    }
    last
}

impl NameIndex {
    pub fn build(mut entries: Vec<Entry>, generation: &str) -> Result<NameIndex> {
        let mut keyed: Vec<(Vec<u8>, usize)> =
            entries.iter().enumerate().map(|(i, e)| (lower_ascii(&e.display), i)).collect();
        keyed.sort_unstable();
        let total: usize = keyed.iter().map(|k| k.0.len() + 1).sum();
        if total > u32::MAX as usize {
            bail!("name index too large ({total} bytes)");
        }
        let n = keyed.len();
        let mut buf = Vec::with_capacity(total);
        let mut starts = Vec::with_capacity(n + 1);
        let mut ids = Vec::with_capacity(n);
        let mut last = Vec::with_capacity(n);
        let mut meta = Vec::with_capacity(n);
        let mut module = Vec::with_capacity(n);
        for (k, i) in &keyed {
            starts.push(buf.len() as u32);
            buf.extend_from_slice(k);
            buf.push(b'\n');
            let e = &mut entries[*i];
            ids.push(e.id);
            last.push(last_start(k).min(u16::MAX as usize) as u16);
            meta.push(pack(e.kind, e.origin, e.flags));
            module.push(e.module);
            e.display = String::new();
        }
        starts.push(buf.len() as u32);
        let mut ni = NameIndex { generation: generation.to_string(), buf, starts, ids, last, meta, by_last: Vec::new(), module };
        let mut by_last: Vec<u32> = (0..n as u32).collect();
        by_last.sort_unstable_by(|&a, &b| ni.last_bytes(a as usize).cmp(ni.last_bytes(b as usize)).then(a.cmp(&b)));
        ni.by_last = by_last;
        Ok(ni)
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn memory_bytes(&self) -> usize {
        self.buf.len() + self.starts.len() * 4 + self.ids.len() * 4 + self.last.len() * 2 + self.meta.len() * 4 + self.by_last.len() * 4 + self.module.len() * 4
    }

    #[inline]
    fn entry(&self, i: usize) -> &[u8] {
        &self.buf[self.starts[i] as usize..self.starts[i + 1] as usize - 1]
    }

    #[inline]
    fn last_bytes(&self, i: usize) -> &[u8] {
        let e = self.entry(i);
        let l = self.last[i] as usize;
        &e[l.min(e.len())..]
    }

    fn accept(&self, i: usize, p: &SearchParams) -> bool {
        let m = self.meta[i];
        if !p.show_generated && m_generated(m) {
            return false;
        }
        if p.local_only && !m_origin(m).is_local() {
            return false;
        }
        p.kinds.accepts(m_kind(m))
    }

    fn score(&self, i: usize, tier: MatchTier) -> i32 {
        let m = self.meta[i];
        let mut s = tier.base();
        match m_origin(m) {
            Origin::Local | Origin::SourceCandidate => s += 600,
            Origin::Library => {}
            Origin::Unresolved => s -= 800,
        }
        if m_generated(m) {
            s -= 3000;
        }
        if matches!(m_kind(m), DeclKind::Theorem | DeclKind::Definition | DeclKind::Structure | DeclKind::Class | DeclKind::Inductive) {
            s += 20;
        }
        let len = self.entry(i).len() as i32;
        s - len.min(400)
    }

    /// Search. `cancel` is polled regularly; when it returns true the
    /// partial outcome is returned with `cancelled` set.
    pub fn search(&self, p: &SearchParams, cancel: &dyn Fn() -> bool) -> SearchOutcome {
        let t0 = Instant::now();
        let q = String::from_utf8(lower_ascii(p.query.trim())).unwrap_or_default();
        let mut out = SearchOutcome::default();
        if self.is_empty() {
            out.elapsed = t0.elapsed();
            return out;
        }
        // The scope as a range of entries (a namespace is contiguous in name
        // order) and a membership test for entries inside that range.
        let ns: Option<Vec<u8>> = match &p.scope {
            Some(Scope::Namespace(n)) => Some(lower_ascii(n.trim().trim_end_matches('.'))),
            _ => None,
        };
        let (lo_e, hi_e) = match &ns {
            Some(n) => {
                let lo = self.lower_bound(n, |s, i| s.entry(i));
                let (mut a, mut b) = (lo, self.len());
                while a < b {
                    let mid = (a + b) / 2;
                    if self.entry(mid).starts_with(n) {
                        a = mid + 1;
                    } else {
                        b = mid;
                    }
                }
                (lo, a)
            }
            None => (0, self.len()),
        };
        let in_scope = |i: usize| -> bool {
            match (&p.scope, &ns) {
                (None, _) => true,
                (Some(Scope::Modules(set)), _) => set.contains(self.module[i]),
                (Some(Scope::Namespace(_)), Some(n)) => {
                    let e = self.entry(i);
                    i >= lo_e && i < hi_e && (e.len() == n.len() || e.get(n.len()) == Some(&b'.'))
                }
                (Some(Scope::Namespace(_)), None) => false,
            }
        };
        const SCAN_CAP: usize = 300_000;
        let limit = p.limit.max(1);
        if q.is_empty() {
            // No query: with a scope, list what is in it, in name order.
            if p.scope.is_some() {
                let mut filtered = 0usize;
                for i in lo_e..hi_e {
                    if (i - lo_e) % 16384 == 16383 && cancel() {
                        out.cancelled = true;
                        break;
                    }
                    if !in_scope(i) {
                        continue;
                    }
                    if !self.accept(i, p) {
                        filtered += 1;
                        continue;
                    }
                    out.total += 1;
                    if out.hits.len() < limit {
                        out.hits.push(Hit { id: self.ids[i], score: -(out.hits.len() as i32), tier: MatchTier::Listed });
                    } else if out.total >= limit + SCAN_CAP || matches!(p.scope, Some(Scope::Modules(_))) {
                        // Counting a module set means reading the whole index: stop.
                        out.total_capped = true;
                        break;
                    }
                }
                out.filtered_out = filtered;
            }
            out.elapsed = t0.elapsed();
            return out;
        }
        let terms: Vec<&str> = q.split_whitespace().collect();
        let mut best: std::collections::HashMap<u32, MatchTier> = std::collections::HashMap::new();
        let mut filtered: std::collections::HashSet<u32> = std::collections::HashSet::new();
        let mut capped = false;
        let consider = |i: usize, tier: MatchTier, best: &mut std::collections::HashMap<u32, MatchTier>, filtered: &mut std::collections::HashSet<u32>| -> bool {
            if !in_scope(i) {
                return false;
            }
            if !self.accept(i, p) {
                filtered.insert(i as u32);
                return true;
            }
            let e = best.entry(i as u32).or_insert(tier);
            if tier < *e {
                *e = tier;
            }
            true
        };
        // The part of the buffer that holds the scope's entries.
        let (b_lo, b_hi) = (self.starts[lo_e] as usize, self.starts[hi_e] as usize);
        let scoped = p.scope.is_some();
        if terms.len() == 1 {
            let qb = q.as_bytes();
            // Full-name prefix range.
            let lo = self.lower_bound(qb, |s, i| s.entry(i));
            let mut i = lo;
            let mut scanned = 0;
            while i < self.len() && self.entry(i).starts_with(qb) {
                let tier = if self.entry(i).len() == qb.len() { MatchTier::Exact } else { MatchTier::Prefix };
                consider(i, tier, &mut best, &mut filtered);
                i += 1;
                scanned += 1;
                if scanned >= SCAN_CAP {
                    capped = true;
                    break;
                }
                if scanned % 16384 == 0 && cancel() {
                    out.cancelled = true;
                    break;
                }
            }
            // Inside a namespace the query is also read relative to it:
            // `reach` in `Demo.Graph` is a prefix of `Demo.Graph.reach_trans`.
            if let Some(n) = &ns {
                let mut rel = n.clone();
                rel.push(b'.');
                rel.extend_from_slice(qb);
                let mut i = self.lower_bound(&rel, |s, i| s.entry(i));
                let mut scanned = 0;
                while i < hi_e && self.entry(i).starts_with(&rel) && scanned < SCAN_CAP {
                    let tier = if self.entry(i).len() == rel.len() { MatchTier::Exact } else { MatchTier::Prefix };
                    consider(i, tier, &mut best, &mut filtered);
                    i += 1;
                    scanned += 1;
                }
            }
            // Last-component prefix range. (Inside a namespace the substring
            // pass below finds the same entries without leaving the range.)
            let lo = self.lower_bound_last(qb);
            let mut j = if ns.is_some() && qb.len() >= 2 { self.len() } else { lo };
            let mut scanned = 0;
            while j < self.len() && !out.cancelled {
                let idx = self.by_last[j] as usize;
                let lb = self.last_bytes(idx);
                if !lb.starts_with(qb) {
                    break;
                }
                let tier = if lb.len() == qb.len() { MatchTier::ExactLast } else { MatchTier::LastPrefix };
                consider(idx, tier, &mut best, &mut filtered);
                j += 1;
                scanned += 1;
                if scanned >= SCAN_CAP {
                    capped = true;
                    break;
                }
            }
            // Substring tier.
            if qb.len() >= 2 && !out.cancelled {
                let finder = memchr::memmem::Finder::new(qb);
                let mut entry = lo_e;
                let mut hits = 0usize;
                for (seen, pos) in finder.find_iter(&self.buf[b_lo..b_hi]).enumerate() {
                    // advance entry pointer
                    let pos = pos + b_lo;
                    let pos32 = pos as u32;
                    if self.starts[entry + 1] <= pos32 {
                        entry = match self.starts.binary_search(&pos32) {
                            Ok(k) => k,
                            Err(k) => k - 1,
                        };
                    }
                    let off = pos - self.starts[entry] as usize;
                    let e = self.entry(entry);
                    let tier = if off == 0 {
                        continue; // prefix tiers handled above
                    } else if off == self.last[entry] as usize {
                        if e.len() - off == qb.len() { MatchTier::ExactLast } else { MatchTier::LastPrefix }
                    } else if matches!(e[off - 1], b'.' | b'_' | b'\'' ) {
                        MatchTier::ComponentPrefix
                    } else {
                        MatchTier::Substring
                    };
                    // Only what is in scope counts towards the cap.
                    if consider(entry, tier, &mut best, &mut filtered) {
                        hits += 1;
                    }
                    if hits >= SCAN_CAP {
                        capped = true;
                        break;
                    }
                    if seen % 8192 == 8191 && cancel() {
                        out.cancelled = true;
                        break;
                    }
                }
            }
            // `feit_thompson` should also find `FeitThompson`: retry the
            // substring tier with separators removed when results are thin.
            let squashed: Vec<u8> = qb.iter().copied().filter(|&c| c != b'_').collect();
            if squashed.len() >= 3 && squashed.len() != qb.len() && best.len() < limit && !out.cancelled {
                let finder = memchr::memmem::Finder::new(&squashed);
                let mut entry = lo_e;
                for (k, pos) in finder.find_iter(&self.buf[b_lo..b_hi]).enumerate() {
                    let pos = pos + b_lo;
                    let pos32 = pos as u32;
                    if self.starts[entry + 1] <= pos32 {
                        entry = match self.starts.binary_search(&pos32) {
                            Ok(k) => k,
                            Err(k) => k - 1,
                        };
                    }
                    let off = pos - self.starts[entry] as usize;
                    let e = self.entry(entry);
                    let tier = if off == 0 || off == self.last[entry] as usize || matches!(e[off - 1], b'.' | b'_') {
                        MatchTier::ComponentPrefix
                    } else {
                        MatchTier::Substring
                    };
                    consider(entry, tier, &mut best, &mut filtered);
                    if k >= SCAN_CAP {
                        capped = true;
                        break;
                    }
                    if k % 8192 == 8191 && cancel() {
                        out.cancelled = true;
                        break;
                    }
                }
            }
            // Fuzzy tier: only when the precise tiers did not fill a page and
            // the user did not type an existing name exactly.
            let has_exact = best.values().any(|t| matches!(t, MatchTier::Exact | MatchTier::ExactLast));
            if qb.len() >= 3 && best.len() < limit.min(40) && !has_exact && !out.cancelled {
                out.fuzzy_ran = true;
                let deadline = t0 + p.fuzzy_budget.max(Duration::from_millis(1));
                for i in lo_e..hi_e {
                    if i % 4096 == 0 && (Instant::now() > deadline || cancel()) {
                        out.fuzzy_truncated = true;
                        break;
                    }
                    // The scope test is a bit test: before the hash lookup.
                    if (scoped && !in_scope(i)) || best.contains_key(&(i as u32)) {
                        continue;
                    }
                    if fuzzy_score(qb, self.entry(i)).is_some() {
                        if self.accept(i, p) {
                            best.insert(i as u32, MatchTier::Fuzzy);
                        } else {
                            filtered.insert(i as u32);
                        }
                    }
                }
            }
        } else {
            // Multi-word: every word must occur somewhere in the name.
            let mut sorted = terms.clone();
            sorted.sort_by_key(|t| Reverse(t.len()));
            let first = sorted[0].as_bytes();
            let finder = memchr::memmem::Finder::new(first);
            let mut entry = lo_e;
            let mut last_entry = usize::MAX;
            let mut hits = 0usize;
            for pos in finder.find_iter(&self.buf[b_lo..b_hi]) {
                let pos = pos + b_lo;
                let pos32 = pos as u32;
                if self.starts[entry + 1] <= pos32 {
                    entry = match self.starts.binary_search(&pos32) {
                        Ok(k) => k,
                        Err(k) => k - 1,
                    };
                }
                if entry == last_entry {
                    continue;
                }
                last_entry = entry;
                let e = self.entry(entry);
                if sorted[1..].iter().all(|t| memchr::memmem::find(e, t.as_bytes()).is_some()) {
                    consider(entry, MatchTier::AllTerms, &mut best, &mut filtered);
                }
                hits += 1;
                if hits % 8192 == 0 && cancel() {
                    out.cancelled = true;
                    break;
                }
                if hits >= SCAN_CAP {
                    capped = true;
                    break;
                }
            }
        }
        // Top-k selection.
        let mut heap: BinaryHeap<Reverse<(i32, Reverse<u32>)>> = BinaryHeap::with_capacity(limit + 1);
        for (&i, &tier) in &best {
            let mut s = self.score(i as usize, tier);
            if tier == MatchTier::Fuzzy {
                s += fuzzy_score(q.as_bytes(), self.entry(i as usize)).unwrap_or(0).min(9_000);
            }
            heap.push(Reverse((s, Reverse(i))));
            if heap.len() > limit {
                heap.pop();
            }
        }
        let mut top: Vec<(i32, u32)> = heap.into_iter().map(|Reverse((s, Reverse(i)))| (s, i)).collect();
        top.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        out.hits = top
            .into_iter()
            .map(|(s, i)| Hit { id: self.ids[i as usize], score: s, tier: best[&i] })
            .collect();
        out.total = best.len();
        out.total_capped = capped;
        out.filtered_out = filtered.len();
        out.elapsed = t0.elapsed();
        out
    }

    /// Exact lookup of a display name (case-insensitive). Returns decl ids.
    pub fn exact(&self, name: &str) -> Vec<u32> {
        let q = lower_ascii(name);
        let mut i = self.lower_bound(&q, |s, i| s.entry(i));
        let mut v = Vec::new();
        while i < self.len() && self.entry(i) == q.as_slice() {
            v.push(self.ids[i]);
            i += 1;
        }
        v
    }

    fn lower_bound(&self, q: &[u8], key: impl Fn(&Self, usize) -> &[u8]) -> usize {
        let (mut lo, mut hi) = (0usize, self.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            if key(self, mid) < q {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }

    fn lower_bound_last(&self, q: &[u8]) -> usize {
        let (mut lo, mut hi) = (0usize, self.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.last_bytes(self.by_last[mid] as usize) < q {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }

    // ---- persistence ----

    pub fn save(&self, path: &Path) -> Result<()> {
        let tmp = path.with_extension("names.tmp");
        {
            let f = std::fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
            let mut w = BufWriter::with_capacity(1 << 20, f);
            w.write_all(MAGIC)?;
            w.write_all(&VERSION.to_le_bytes())?;
            w.write_all(&0u32.to_le_bytes())?;
            let mut g = [b'0'; 16];
            for (d, s) in g.iter_mut().zip(self.generation.bytes()) {
                *d = s;
            }
            w.write_all(&g)?;
            w.write_all(&(self.len() as u64).to_le_bytes())?;
            w.write_all(&(self.buf.len() as u64).to_le_bytes())?;
            for v in &self.starts {
                w.write_all(&v.to_le_bytes())?;
            }
            for v in &self.ids {
                w.write_all(&v.to_le_bytes())?;
            }
            for v in &self.last {
                w.write_all(&v.to_le_bytes())?;
            }
            for v in &self.meta {
                w.write_all(&v.to_le_bytes())?;
            }
            for v in &self.by_last {
                w.write_all(&v.to_le_bytes())?;
            }
            for v in &self.module {
                w.write_all(&v.to_le_bytes())?;
            }
            w.write_all(&self.buf)?;
            w.flush()?;
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Loads and structurally validates a saved index. Fails if the file is
    /// malformed or belongs to a different generation.
    pub fn load(path: &Path, expect_generation: &str) -> Result<NameIndex> {
        let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        if data.len() < 48 || &data[0..8] != MAGIC {
            bail!("not a slim name index");
        }
        let rd_u32 = |o: usize| u32::from_le_bytes(data[o..o + 4].try_into().unwrap());
        let rd_u64 = |o: usize| u64::from_le_bytes(data[o..o + 8].try_into().unwrap());
        if rd_u32(8) != VERSION {
            bail!("name index version mismatch");
        }
        let generation = String::from_utf8_lossy(&data[16..32]).to_string();
        if generation != expect_generation {
            bail!("name index generation {generation} does not match index {expect_generation}");
        }
        let n = rd_u64(32) as usize;
        let buf_len = rd_u64(40) as usize;
        let need = 48usize
            .checked_add((n + 1) * 4)
            .and_then(|x| x.checked_add(n * (4 + 2 + 4 + 4 + 4)))
            .and_then(|x| x.checked_add(buf_len))
            .context("overflow")?;
        if data.len() != need {
            bail!("name index has wrong size");
        }
        let mut o = 48;
        let take_u32 = |count: usize, o: &mut usize| -> Vec<u32> {
            let v = data[*o..*o + count * 4].chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
            *o += count * 4;
            v
        };
        let starts = take_u32(n + 1, &mut o);
        let ids = take_u32(n, &mut o);
        let last: Vec<u16> = data[o..o + n * 2].chunks_exact(2).map(|c| u16::from_le_bytes(c.try_into().unwrap())).collect();
        o += n * 2;
        let meta = take_u32(n, &mut o);
        let by_last = take_u32(n, &mut o);
        let module = take_u32(n, &mut o);
        let buf = data[o..o + buf_len].to_vec();
        // Structural validation (cheap, O(n)).
        if starts.first() != Some(&0) || *starts.last().unwrap() as usize != buf_len {
            bail!("name index offsets corrupt");
        }
        if starts.windows(2).any(|w| w[1] <= w[0]) {
            bail!("name index offsets not increasing");
        }
        if by_last.iter().any(|&x| x as usize >= n) {
            bail!("name index permutation corrupt");
        }
        for i in 0..n {
            if buf[starts[i + 1] as usize - 1] != b'\n' {
                bail!("name index separators corrupt");
            }
        }
        Ok(NameIndex { generation, buf, starts, ids, last, meta, by_last, module })
    }
}

/// Subsequence match score (higher is better), `None` when `q` is not a
/// subsequence of `s`. Rewards matches at word starts and runs.
pub fn fuzzy_score(q: &[u8], s: &[u8]) -> Option<i32> {
    if q.is_empty() || q.len() > s.len() {
        return None;
    }
    // Try each word start where the first query character occurs; keep the
    // best compact match (span at most 3x the query length + 4).
    let max_span = q.len() * 3 + 4;
    let mut best: Option<i32> = None;
    let mut from = 0usize;
    while let Some(off) = memchr::memchr(q[0], &s[from..]) {
        let start = from + off;
        from = start + 1;
        if !(start == 0 || matches!(s[start - 1], b'.' | b'_' | b' ')) {
            continue;
        }
        let mut score = 80i32;
        let mut si = start + 1;
        let mut prev = start;
        let mut ok = true;
        let mut want_boundary = false;
        for &qc in &q[1..] {
            // Separators in the query (`_`, `.`, space) match any word
            // boundary: `reach_trans` finds `Reach.trans`.
            if matches!(qc, b'_' | b'.' | b' ') {
                want_boundary = true;
                continue;
            }
            let window_end = (start + max_span).min(s.len());
            if si >= window_end {
                ok = false;
                break;
            }
            let found = if want_boundary {
                // first occurrence of qc that starts a word
                let mut k = si;
                let mut hit = None;
                while let Some(o) = memchr::memchr(qc, &s[k..window_end]) {
                    let pos = k + o;
                    if pos > 0 && matches!(s[pos - 1], b'.' | b'_' | b' ') {
                        hit = Some(pos - si);
                        break;
                    }
                    k = pos + 1;
                }
                hit
            } else {
                memchr::memchr(qc, &s[si..window_end])
            };
            want_boundary = false;
            match found {
                None => {
                    ok = false;
                    break;
                }
                Some(o) => {
                    let pos = si + o;
                    if matches!(s[pos - 1], b'.' | b'_' | b' ') {
                        score += 50;
                    }
                    if pos == prev + 1 {
                        score += 40;
                    } else {
                        score -= (o as i32).min(12);
                    }
                    prev = pos;
                    si = pos + 1;
                }
            }
        }
        if ok {
            best = Some(best.map_or(score, |b: i32| b.max(score)));
        }
        if from >= s.len() {
            break;
        }
    }
    best.map(|b| b.max(0) * 8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idx(names: &[(&str, DeclKind, Origin, u32)]) -> NameIndex {
        let entries = names
            .iter()
            .enumerate()
            .map(|(i, (n, k, o, f))| Entry { id: i as u32 + 1, display: n.to_string(), kind: *k, origin: *o, flags: *f, module: if n.starts_with("Demo.Graph") { 2 } else { 1 } })
            .collect();
        NameIndex::build(entries, "0123456789abcdef").unwrap()
    }

    fn sample() -> NameIndex {
        use DeclKind::*;
        use Origin::*;
        idx(&[
            ("Demo.Rel.ReflTransGen.trans", Theorem, Local, 0),
            ("Demo.Rel.ReflTransGen", Inductive, Local, 0),
            ("Demo.Rel.ReflTransGen.rec", Recursor, Local, flags::GENERATED),
            ("Demo.Graph.reach_trans", Theorem, Local, 0),
            ("Trans", Class, Library, 0),
            ("trans", Definition, Library, 0),
            ("Nat.le_trans", Theorem, Library, 0),
            ("Demo.Graph.search_sound", Theorem, Local, 0),
            ("Demo.Graph.Reach", Definition, Local, 0),
        ])
    }

    fn names_of(ni: &NameIndex, out: &SearchOutcome) -> Vec<String> {
        out.hits
            .iter()
            .map(|h| {
                let i = ni.ids.iter().position(|&x| x == h.id).unwrap();
                String::from_utf8(ni.entry(i).to_vec()).unwrap()
            })
            .collect()
    }

    #[test]
    fn exact_qualified_match_ranks_first() {
        let ni = sample();
        let p = SearchParams { query: "demo.rel.refltransgen".into(), ..Default::default() };
        let out = ni.search(&p, &|| false);
        let n = names_of(&ni, &out);
        assert_eq!(n[0], "demo.rel.refltransgen");
        assert_eq!(out.hits[0].tier, MatchTier::Exact);
        assert_eq!(n[1], "demo.rel.refltransgen.trans");
        // Generated recursor hidden by default...
        assert!(!n.iter().any(|x| x.ends_with(".rec")));
        assert_eq!(out.filtered_out, 1);
        // ...and visible on request.
        let p2 = SearchParams { show_generated: true, ..p };
        assert!(names_of(&ni, &ni.search(&p2, &|| false)).iter().any(|x| x.ends_with(".rec")));
    }

    #[test]
    fn tiers_order_is_stable() {
        let ni = sample();
        let p = SearchParams { query: "trans".into(), ..Default::default() };
        let out = ni.search(&p, &|| false);
        let n = names_of(&ni, &out);
        // exact top-level names first, then exact last components, then others.
        assert_eq!(&n[..2], &["trans", "trans"]);
        assert_eq!(n[2], "demo.rel.refltransgen.trans");
        assert!(n.contains(&"nat.le_trans".to_string()));
        assert!(n.contains(&"demo.graph.reach_trans".to_string()));
        // local before library within the same tier
        let a = n.iter().position(|x| x == "demo.graph.reach_trans").unwrap();
        let b = n.iter().position(|x| x == "nat.le_trans").unwrap();
        assert!(a < b);
        // repeated searches give identical order
        assert_eq!(ni.search(&p, &|| false).hits, out.hits);
    }

    #[test]
    fn filters_and_fuzzy() {
        let ni = sample();
        let p = SearchParams { query: "trans".into(), local_only: true, ..Default::default() };
        let n = names_of(&ni, &ni.search(&p, &|| false));
        assert!(n.iter().all(|x| x.starts_with("demo")));
        let p = SearchParams { query: "srchsnd".into(), ..Default::default() };
        let out = ni.search(&p, &|| false);
        assert!(out.fuzzy_ran);
        assert_eq!(names_of(&ni, &out), vec!["demo.graph.search_sound"]);
        let p = SearchParams { query: "reach trans".into(), ..Default::default() };
        assert_eq!(names_of(&ni, &ni.search(&p, &|| false)), vec!["demo.graph.reach_trans"]);
        let p = SearchParams { query: "x".into(), kinds: KindFilter::Theorems, ..Default::default() };
        assert!(ni.search(&p, &|| false).hits.is_empty());
    }

    #[test]
    fn cancellation_is_respected() {
        let entries: Vec<Entry> = (0..100_000)
            .map(|i| Entry { id: i + 1, display: format!("Big.n{i}.lemma_{i}"), kind: DeclKind::Theorem, origin: Origin::Local, flags: 0, module: i % 100 })
            .collect();
        let ni = NameIndex::build(entries, "gen").unwrap();
        let p = SearchParams { query: "lemma".into(), ..Default::default() };
        let out = ni.search(&p, &|| true);
        assert!(out.cancelled);
        let full = ni.search(&p, &|| false);
        assert!(!full.cancelled);
        assert_eq!(full.total, 100_000);
        assert_eq!(full.hits.len(), 200);
    }

    #[test]
    fn save_load_round_trip_and_validation() {
        let ni = sample();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.names");
        ni.save(&p).unwrap();
        let l = NameIndex::load(&p, "0123456789abcdef").unwrap();
        let q = SearchParams { query: "trans".into(), ..Default::default() };
        assert_eq!(l.search(&q, &|| false).hits, ni.search(&q, &|| false).hits);
        assert!(NameIndex::load(&p, "ffffffffffffffff").is_err());
        let mut data = std::fs::read(&p).unwrap();
        let len = data.len();
        data[len - 3] ^= 0xff; // corrupt a separator region
        data.truncate(len - 1);
        std::fs::write(&p, &data).unwrap();
        assert!(NameIndex::load(&p, "0123456789abcdef").is_err());
    }

    #[test]
    fn fuzzy_scoring_prefers_word_starts() {
        let a = fuzzy_score(b"rtg", b"demo.refl_trans_gen").unwrap();
        assert!(fuzzy_score(b"rtg", b"demo.xrxtxg").is_none(), "first char must start a word");
        let b = fuzzy_score(b"rtg", b"demo.rxtxg").unwrap();
        assert!(a > b);
        assert!(fuzzy_score(b"abc", b"a_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxb_c").is_none(), "span too wide");
        assert!(fuzzy_score(b"reach_trans", b"demo.graph.reach.trans").is_some(), "separators are interchangeable");
        assert!(fuzzy_score(b"reach_trans", b"demo.graph.reachxtrans").is_none(), "separator needs a word boundary");
        assert!(fuzzy_score(b"zz", b"abc").is_none());
    }

    #[test]
    fn a_namespace_scope_is_a_range_and_reads_the_query_relative_to_it() {
        let ni = sample();
        let scope = |n: &str| Some(Scope::Namespace(n.into()));
        // `trans` matches five names; two of them are below Demo.Graph or Demo.Rel.ReflTransGen.
        let p = SearchParams { query: "trans".into(), scope: scope("Demo.Graph"), ..Default::default() };
        let out = ni.search(&p, &|| false);
        assert_eq!(names_of(&ni, &out), vec!["demo.graph.reach_trans"]);
        assert_eq!(out.filtered_out, 0, "the generated recursor outside the scope is not reported as filtered");
        let p = SearchParams { query: "trans".into(), scope: scope("demo.rel.refltransgen"), ..Default::default() };
        let out = ni.search(&p, &|| false);
        // The namespace's own declaration is in scope too (`trans` is a substring of it).
        assert_eq!(names_of(&ni, &out), vec!["demo.rel.refltransgen.trans", "demo.rel.refltransgen"]);
        assert_eq!(out.hits[0].tier, MatchTier::Exact, "the query is the name relative to the scope");
        assert_eq!(out.filtered_out, 1, "the generated recursor inside the scope is");
        // A prefix relative to the scope, and the full name, both work.
        let p = SearchParams { query: "rea".into(), scope: scope("Demo.Graph"), ..Default::default() };
        let out = ni.search(&p, &|| false);
        assert_eq!(names_of(&ni, &out), vec!["demo.graph.reach", "demo.graph.reach_trans"]);
        assert!(out.hits.iter().all(|h| h.tier == MatchTier::Prefix));
        let p = SearchParams { query: "demo.graph.reach".into(), scope: scope("Demo.Graph"), ..Default::default() };
        assert_eq!(ni.search(&p, &|| false).hits[0].tier, MatchTier::Exact);
        // `Demo.Gra` is not a namespace: `Demo.Graph.*` is not below it.
        let p = SearchParams { query: "reach".into(), scope: scope("Demo.Gra"), ..Default::default() };
        assert!(ni.search(&p, &|| false).hits.is_empty());
        // Fuzzy matching stays inside the scope.
        let p = SearchParams { query: "srchsnd".into(), scope: scope("Demo.Rel"), ..Default::default() };
        assert!(ni.search(&p, &|| false).hits.is_empty());
        let p = SearchParams { query: "srchsnd".into(), scope: scope("Demo"), ..Default::default() };
        assert_eq!(names_of(&ni, &ni.search(&p, &|| false)), vec!["demo.graph.search_sound"]);
        // No query: the scope is listed in name order, filters applied.
        let p = SearchParams { query: String::new(), scope: scope("Demo.Rel"), ..Default::default() };
        let out = ni.search(&p, &|| false);
        assert_eq!(names_of(&ni, &out), vec!["demo.rel.refltransgen", "demo.rel.refltransgen.trans"]);
        assert!(out.hits.iter().all(|h| h.tier == MatchTier::Listed));
        assert_eq!((out.total, out.filtered_out), (2, 1));
        // Without a scope an empty query still returns nothing.
        assert!(ni.search(&SearchParams::default(), &|| false).hits.is_empty());
    }

    #[test]
    fn a_module_scope_filters_by_the_module_of_each_entry() {
        let ni = sample();
        // Module 2 holds the Demo.Graph declarations in the sample.
        let set = |ids: &[u32]| Some(Scope::Modules(std::sync::Arc::new(ModuleSet::from_ids(ids.iter().copied()))));
        let p = SearchParams { query: "trans".into(), scope: set(&[2]), ..Default::default() };
        assert_eq!(names_of(&ni, &ni.search(&p, &|| false)), vec!["demo.graph.reach_trans"]);
        let p = SearchParams { query: "trans".into(), scope: set(&[1]), ..Default::default() };
        let n = names_of(&ni, &ni.search(&p, &|| false));
        assert_eq!(n.len(), 5);
        assert!(!n.contains(&"demo.graph.reach_trans".to_string()));
        let p = SearchParams { query: "reach trans".into(), scope: set(&[1]), ..Default::default() };
        assert!(ni.search(&p, &|| false).hits.is_empty(), "multi-word search is scoped too");
        let p = SearchParams { query: String::new(), scope: set(&[2]), ..Default::default() };
        assert_eq!(names_of(&ni, &ni.search(&p, &|| false)), vec!["demo.graph.reach", "demo.graph.reach_trans", "demo.graph.search_sound"]);
        assert!(ni.search(&SearchParams { query: "trans".into(), scope: set(&[]), ..Default::default() }, &|| false).hits.is_empty());
        // Round trip keeps the module ids.
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.names");
        ni.save(&f).unwrap();
        let l = NameIndex::load(&f, "0123456789abcdef").unwrap();
        let p = SearchParams { query: "trans".into(), scope: set(&[2]), ..Default::default() };
        assert_eq!(names_of(&l, &l.search(&p, &|| false)), vec!["demo.graph.reach_trans"]);
    }

    #[test]
    fn scope_specs_cover_whole_components_only() {
        assert!(ScopeSpec::covers("A.B", "A.B"));
        assert!(ScopeSpec::covers("A.B", "a.b.c"));
        assert!(!ScopeSpec::covers("A.B", "A.Bc"));
        assert!(!ScopeSpec::covers("A.B", "A.B'"));
        let s = ScopeSpec { kind: ScopeKind::Modules, prefix: "P.Q.R".into() };
        assert!(s.accepts("anything", Some("P.Q.R.S")) && !s.accepts("P.Q.R.x", None));
        assert_eq!(s.widened().unwrap().prefix, "P.Q");
        assert!(ScopeSpec { kind: ScopeKind::Namespace, prefix: "P".into() }.widened().is_none());
    }
}
