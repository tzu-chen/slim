//! The reference graph of a project: which declaration refers to which.
//!
//! One file beside the index (`<index>.graph`), built on request by
//! `slim graph build` and read whole into memory. It holds, for every
//! declaration of the index, the declarations of the index it refers to
//! and whether each reference stands in the statement, after it, or both.
//!
//! Where the edges come from is recorded per file and shown wherever they
//! are used:
//!
//! * **Lean references**: the `.ilean` files of a build (see
//!   [`crate::leanrefs`]): every constant written in a file, as the
//!   elaborator resolved it, notation and dot notation included. What Lean
//!   found without anything being written (instances, the default `simp`
//!   set) is not there: this is not the kernel's dependency relation.
//! * **lexical**: identifiers that equal an indexed name under Lean's
//!   lookup rules. Approximate, used for files without usable artifacts.
//! * **Lean dependencies**: the stored T/V/C edges of a semantic index.
//!
//! Everything derived from the graph (what an endpoint reaches, the chain
//! back to it, the dominator tree and its sizes) is computed in memory for
//! the roots asked about and inherits the graph's label.

use crate::db::IndexMode;
use crate::model::{flags as dflags, DeclKind};
use crate::source::lexer::{LexProfile, Lexer, TokKind};
use crate::sourceview::SourceFileText;
use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Instant;

const MAGIC: &[u8; 8] = b"SLIMGRF\x01";
/// Version 2 added the `sorry` flag of a node.
const VERSION: u32 = 2;

/// Edge bits (the top two bits of a stored target or source).
pub const STMT: u32 = 1 << 31;
pub const BODY: u32 = 1 << 30;
pub const NODE: u32 = (1 << 30) - 1;
pub const NONE: u32 = u32::MAX;

/// Node flags.
pub mod nf {
    /// A theorem whose whole proof is one name (`theorem a : T := b`).
    pub const RELAY: u8 = 1;
    /// A definition, structure or inductive type of sort `Prop`: a named
    /// statement.
    pub const PROP: u8 = 2;
    /// A theorem whose statement refers to no declaration of the project:
    /// it reads with the libraries' vocabulary alone.
    pub const LIB_ONLY: u8 = 4;
    /// The edges out of this declaration are Lean's references.
    pub const LEAN: u8 = 8;
    /// A theorem without binders whose type is a single name.
    pub const BARE: u8 = 16;
    pub const THEOREM: u8 = 32;
    /// Nested in another declaration (a field, a constructor).
    pub const NESTED: u8 = 64;
    /// A `sorry` token stands in its text (lexical).
    pub const SORRY: u8 = 128;
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Stats {
    /// `lean-refs`, `lexical`, `mixed` or `lean-deps`.
    pub source: String,
    pub files: u64,
    /// Files whose edges are Lean's references, by how they were tied to
    /// the text: Lake's source hash; the `.ilean` hash of a trace and the
    /// recorded places; the recorded places alone.
    pub files_hashed: u64,
    pub files_consistent: u64,
    pub files_lexical: u64,
    /// Files with no edges: changed since the index was written.
    #[serde(default)]
    pub files_left_out: u64,
    /// Why files fell back to the lexical reading, or were left out (reason, count).
    pub fallbacks: Vec<(String, u64)>,
    pub edges: u64,
    pub unresolved: u64,
    pub artifacts: Option<String>,
    pub built_at: i64,
    pub ms: u64,
}

impl Stats {
    /// One line for a header: where the edges are from.
    pub fn label(&self) -> &'static str {
        match self.source.as_str() {
            "lean-refs" => "Lean references",
            "mixed" => "Lean references, lexical where a file has none",
            "lean-deps" => "Lean dependencies",
            _ => "source mentions (lexical)",
        }
    }
    /// The short tag shown on rows.
    pub fn tag(&self) -> &'static str {
        match self.source.as_str() {
            "lean-refs" | "mixed" => "ref",
            "lean-deps" => "lean",
            _ => "lex",
        }
    }
    pub fn describe(&self) -> String {
        if self.source == "lean-deps" {
            return format!("{}: {} edges of the semantic index (statement, proof or body, constructors)", self.label(), self.edges);
        }
        let mut s = format!("{}: {} edges over {} files", self.label(), self.edges, self.files);
        if self.source != "lean-deps" {
            s += &format!(
                " ({} tied to the text by Lake's hash, {} by the recorded places, {} read lexically)",
                self.files_hashed, self.files_consistent, self.files_lexical
            );
            if self.files_left_out > 0 {
                s += &format!("; {} left out: changed since the index was written", self.files_left_out);
            }
        }
        s
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Header {
    generation: String,
    nodes: u64,
    edges: u64,
    stats: Stats,
}

pub struct Graph {
    pub generation: String,
    /// Declaration id of each node, ascending.
    pub ids: Vec<i64>,
    fwd_off: Vec<u32>,
    fwd: Vec<u32>,
    rev_off: Vec<u32>,
    rev: Vec<u32>,
    pub flags: Vec<u8>,
    pub stats: Stats,
}

pub fn graph_path(index: &Path) -> PathBuf {
    let mut s = index.as_os_str().to_owned();
    s.push(".graph");
    PathBuf::from(s)
}

fn put_u32s(w: &mut impl Write, v: &[u32]) -> std::io::Result<()> {
    let mut buf = Vec::with_capacity(1 << 16);
    for chunk in v.chunks(1 << 14) {
        buf.clear();
        for x in chunk {
            buf.extend_from_slice(&x.to_le_bytes());
        }
        w.write_all(&buf)?;
    }
    Ok(())
}

fn take<'a>(b: &mut &'a [u8], n: usize) -> Result<&'a [u8]> {
    if b.len() < n {
        bail!("the graph file is cut short");
    }
    let (a, rest) = b.split_at(n);
    *b = rest;
    Ok(a)
}

fn get_u32s(b: &mut &[u8], n: usize) -> Result<Vec<u32>> {
    Ok(take(b, n * 4)?.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().expect("four bytes"))).collect())
}

impl Graph {
    pub fn len(&self) -> usize {
        self.ids.len()
    }
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
    pub fn node(&self, id: i64) -> Option<u32> {
        self.ids.binary_search(&id).ok().map(|i| i as u32)
    }
    pub fn id(&self, v: u32) -> i64 {
        self.ids[v as usize]
    }
    /// What `v` refers to: node with [`STMT`]/[`BODY`] bits.
    pub fn uses(&self, v: u32) -> &[u32] {
        &self.fwd[self.fwd_off[v as usize] as usize..self.fwd_off[v as usize + 1] as usize]
    }
    /// What refers to `v`, with the same bits.
    pub fn users(&self, v: u32) -> &[u32] {
        &self.rev[self.rev_off[v as usize] as usize..self.rev_off[v as usize + 1] as usize]
    }
    pub fn edge_count(&self) -> usize {
        self.fwd.len()
    }
    pub fn has(&self, v: u32, f: u8) -> bool {
        self.flags[v as usize] & f != 0
    }

    /// The declaration a relay theorem hands over to: the one name its
    /// proof consists of.
    pub fn relay_target(&self, v: u32) -> Option<u32> {
        if !self.has(v, nf::RELAY) {
            return None;
        }
        let mut body = self.uses(v).iter().filter(|e| *e & BODY != 0).map(|e| e & NODE);
        let first = body.next()?;
        body.next().is_none().then_some(first)
    }

    /// The named statement a theorem proves: the single name its type is.
    pub fn proves(&self, v: u32) -> Option<u32> {
        if !self.has(v, nf::BARE) {
            return None;
        }
        let mut st = self.uses(v).iter().filter(|e| *e & STMT != 0).map(|e| e & NODE);
        let first = st.next()?;
        (st.next().is_none() && self.has(first, nf::PROP)).then_some(first)
    }

    /// The theorems that prove the named statement `v`.
    pub fn provers(&self, v: u32) -> Vec<u32> {
        if !self.has(v, nf::PROP) {
            return Vec::new();
        }
        self.users(v).iter().filter(|e| *e & STMT != 0).map(|e| e & NODE).filter(|&u| self.proves(u) == Some(v)).collect()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        // Its own name: two slims opening one project both build the graph.
        let tmp = path.with_extension(format!("graph.part{}", std::process::id()));
        {
            let f = std::fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
            let mut w = BufWriter::new(f);
            w.write_all(MAGIC)?;
            w.write_all(&VERSION.to_le_bytes())?;
            let h = serde_json::to_vec(&Header { generation: self.generation.clone(), nodes: self.ids.len() as u64, edges: self.fwd.len() as u64, stats: self.stats.clone() })?;
            w.write_all(&(h.len() as u32).to_le_bytes())?;
            w.write_all(&h)?;
            for id in &self.ids {
                w.write_all(&id.to_le_bytes())?;
            }
            put_u32s(&mut w, &self.fwd_off)?;
            put_u32s(&mut w, &self.fwd)?;
            put_u32s(&mut w, &self.rev_off)?;
            put_u32s(&mut w, &self.rev)?;
            w.write_all(&self.flags)?;
            w.flush()?;
        }
        std::fs::rename(&tmp, path).with_context(|| format!("installing {}", path.display()))?;
        Ok(())
    }

    /// Reads the graph of `index`, if there is one and it was built for
    /// the index as it is now (`generation`).
    pub fn load(index: &Path, generation: &str) -> Result<Option<Graph>> {
        let path = graph_path(index);
        let mut f = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(_) => return Ok(None),
        };
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes)?;
        let mut b = bytes.as_slice();
        if take(&mut b, 8)? != MAGIC {
            bail!("{} is not a slim graph", path.display());
        }
        if u32::from_le_bytes(take(&mut b, 4)?.try_into().expect("four")) != VERSION {
            return Ok(None);
        }
        let hl = u32::from_le_bytes(take(&mut b, 4)?.try_into().expect("four")) as usize;
        let h: Header = serde_json::from_slice(take(&mut b, hl)?).context("graph header")?;
        if h.generation != generation {
            return Ok(None);
        }
        let (n, e) = (h.nodes as usize, h.edges as usize);
        let ids: Vec<i64> = take(&mut b, n * 8)?.chunks_exact(8).map(|c| i64::from_le_bytes(c.try_into().expect("eight"))).collect();
        let fwd_off = get_u32s(&mut b, n + 1)?;
        let fwd = get_u32s(&mut b, e)?;
        let rev_off = get_u32s(&mut b, n + 1)?;
        let rev = get_u32s(&mut b, e)?;
        let flags = take(&mut b, n)?.to_vec();
        let ok = |off: &[u32], adj: &[u32]| off.first() == Some(&0) && off.last() == Some(&(e as u32)) && off.windows(2).all(|w| w[0] <= w[1]) && adj.iter().all(|x| ((x & NODE) as usize) < n);
        if !ok(&fwd_off, &fwd) || !ok(&rev_off, &rev) || !ids.windows(2).all(|w| w[0] < w[1]) {
            bail!("{} is damaged; build it again with `slim graph build`", path.display());
        }
        Ok(Some(Graph { generation: h.generation, ids, fwd_off, fwd, rev_off, rev, flags, stats: h.stats }))
    }

    /// From `(source, target | bits)` pairs; duplicates merge their bits.
    fn from_edges(generation: String, ids: Vec<i64>, mut edges: Vec<u64>, flags: Vec<u8>, stats: Stats) -> Graph {
        let n = ids.len();
        // Sort by (source, target), bits last.
        edges.sort_unstable_by_key(|e| ((e >> 32) << 32) | ((*e as u32 & NODE) as u64));
        let mut merged: Vec<u64> = Vec::with_capacity(edges.len());
        for e in edges {
            match merged.last_mut() {
                Some(l) if (*l >> 32) == (e >> 32) && (*l as u32 & NODE) == (e as u32 & NODE) => *l |= e & (STMT | BODY) as u64,
                _ => merged.push(e),
            }
        }
        let mut fwd_off = vec![0u32; n + 1];
        let mut rev_off = vec![0u32; n + 1];
        for e in &merged {
            fwd_off[(e >> 32) as usize + 1] += 1;
            rev_off[(*e as u32 & NODE) as usize + 1] += 1;
        }
        for i in 0..n {
            fwd_off[i + 1] += fwd_off[i];
            rev_off[i + 1] += rev_off[i];
        }
        let fwd: Vec<u32> = merged.iter().map(|e| *e as u32).collect();
        let mut rev = vec![0u32; merged.len()];
        let mut at = rev_off.clone();
        for e in &merged {
            let (s, t) = ((e >> 32) as u32, *e as u32);
            let slot = &mut at[(t & NODE) as usize];
            rev[*slot as usize] = s | (t & (STMT | BODY));
            *slot += 1;
        }
        let mut g = Graph { generation, ids, fwd_off, fwd, rev_off, rev, flags, stats };
        g.stats.edges = g.fwd.len() as u64;
        // A theorem whose statement names nothing of the project.
        for v in 0..n as u32 {
            if g.has(v, nf::THEOREM) && !g.uses(v).iter().any(|e| e & STMT != 0) {
                g.flags[v as usize] |= nf::LIB_ONLY;
            }
        }
        g
    }
}

// ---------------------------------------------------------------------------
// What an endpoint reaches.

/// Everything reachable from a set of roots, with the shortest chain back
/// and the dominator tree (the virtual root above all roots is `n`).
pub struct Cone {
    pub roots: Vec<u32>,
    /// Steps from the nearest root; `u16::MAX` outside.
    pub depth: Vec<u16>,
    /// The declaration one step nearer a root; `NONE` for roots and outside.
    pub parent: Vec<u32>,
    /// Immediate dominator; the virtual root is `n`; `NONE` outside.
    pub idom: Vec<u32>,
    /// Size of the dominator subtree: the declarations reached only
    /// through this one, itself included.
    pub excl: Vec<u32>,
    kid_off: Vec<u32>,
    kids: Vec<u32>,
    pub reached: usize,
    pub ms: f64,
}

/// Which references a walk follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Follow {
    All,
    /// The statement of a theorem and the whole of anything else: what a
    /// declaration states, down to the definitions it is made of.
    Says,
}

impl Graph {
    fn follows(&self, v: u32, e: u32, follow: Follow) -> bool {
        match follow {
            Follow::All => true,
            Follow::Says => !self.has(v, nf::THEOREM) || e & STMT != 0,
        }
    }

    /// The roots as given, sorted, without repeats or strangers; the
    /// steps of every declaration from the nearest of them (`u16::MAX`:
    /// not reached); the declaration one step nearer; how many are reached.
    pub fn reach(&self, roots: &[u32], follow: Follow) -> (Vec<u32>, Vec<u16>, Vec<u32>, usize) {
        let n = self.len();
        let mut depth = vec![u16::MAX; n];
        let mut parent = vec![NONE; n];
        let mut queue: std::collections::VecDeque<u32> = std::collections::VecDeque::new();
        let mut roots: Vec<u32> = roots.iter().copied().filter(|&r| (r as usize) < n).collect();
        roots.sort_unstable();
        roots.dedup();
        for &r in &roots {
            depth[r as usize] = 0;
            queue.push_back(r);
        }
        let mut reached = 0usize;
        while let Some(v) = queue.pop_front() {
            reached += 1;
            let d = depth[v as usize];
            for &e in self.uses(v) {
                let w = e & NODE;
                if depth[w as usize] == u16::MAX && self.follows(v, e, follow) {
                    depth[w as usize] = d.saturating_add(1).min(u16::MAX - 1);
                    parent[w as usize] = v;
                    queue.push_back(w);
                }
            }
        }
        (roots, depth, parent, reached)
    }

    pub fn cone(&self, roots: &[u32], follow: Follow) -> Cone {
        let t = Instant::now();
        let n = self.len();
        let (roots, depth, parent, reached) = self.reach(roots, follow);
        // Postorder of a depth-first walk from the virtual root.
        let mut post: Vec<u32> = Vec::with_capacity(reached + 1);
        let mut num = vec![NONE; n + 1];
        let mut seen = vec![false; n];
        let mut stack: Vec<(u32, u32)> = Vec::new();
        for &r in roots.iter().rev() {
            if seen[r as usize] {
                continue;
            }
            seen[r as usize] = true;
            stack.push((r, self.fwd_off[r as usize]));
            while let Some(&mut (v, ref mut at)) = stack.last_mut() {
                let end = self.fwd_off[v as usize + 1];
                let mut pushed = false;
                while *at < end {
                    let e = self.fwd[*at as usize];
                    *at += 1;
                    let w = e & NODE;
                    if !seen[w as usize] && self.follows(v, e, follow) {
                        seen[w as usize] = true;
                        stack.push((w, self.fwd_off[w as usize]));
                        pushed = true;
                        break;
                    }
                }
                if !pushed {
                    num[v as usize] = post.len() as u32;
                    post.push(v);
                    stack.pop();
                }
            }
        }
        let vroot = post.len() as u32;
        num[n] = vroot;
        // Cooper, Harvey and Kennedy's iteration over postorder numbers.
        let m = post.len() + 1;
        let mut dom = vec![NONE; m];
        dom[vroot as usize] = vroot;
        let is_root: HashSet<u32> = roots.iter().copied().collect();
        let mut changed = true;
        while changed {
            changed = false;
            for b in (0..post.len()).rev() {
                let v = post[b];
                let mut new = if is_root.contains(&v) { vroot } else { NONE };
                for &e in self.users(v) {
                    let u = e & NODE;
                    let p = num[u as usize];
                    if p == NONE || dom[p as usize] == NONE || !self.follows(u, e, follow) {
                        continue;
                    }
                    if new == NONE {
                        new = p;
                        continue;
                    }
                    let (mut a, mut c) = (p, new);
                    while a != c {
                        while a < c {
                            a = dom[a as usize];
                        }
                        while c < a {
                            c = dom[c as usize];
                        }
                    }
                    new = a;
                }
                if new != dom[b] {
                    dom[b] = new;
                    changed = true;
                }
            }
        }
        // Subtree sizes (a child has a smaller postorder number) and children.
        let mut size = vec![1u32; m];
        for b in 0..post.len() {
            let d = dom[b] as usize;
            size[d] += size[b];
        }
        let mut idom = vec![NONE; n];
        let mut excl = vec![0u32; n];
        let mut kid_off = vec![0u32; n + 2];
        let slot = |d: u32| if d == vroot { n } else { post[d as usize] as usize };
        for b in 0..post.len() {
            let v = post[b] as usize;
            idom[v] = if dom[b] == vroot { n as u32 } else { post[dom[b] as usize] };
            excl[v] = size[b];
            kid_off[slot(dom[b]) + 1] += 1;
        }
        for i in 0..=n {
            kid_off[i + 1] += kid_off[i];
        }
        let mut kids = vec![0u32; post.len()];
        let mut at = kid_off.clone();
        for b in 0..post.len() {
            let s = slot(dom[b]);
            kids[at[s] as usize] = post[b];
            at[s] += 1;
        }
        for i in 0..=n {
            kids[kid_off[i] as usize..kid_off[i + 1] as usize].sort_unstable_by_key(|&k| (std::cmp::Reverse(excl[k as usize]), k));
        }
        Cone { roots, depth, parent, idom, excl, kid_off, kids, reached, ms: t.elapsed().as_secs_f64() * 1000.0 }
    }
}

impl Cone {
    pub fn contains(&self, v: u32) -> bool {
        self.depth.get(v as usize).is_some_and(|&d| d != u16::MAX)
    }
    /// The declarations that `v` dominates directly, largest first.
    pub fn children(&self, v: u32) -> &[u32] {
        &self.kids[self.kid_off[v as usize] as usize..self.kid_off[v as usize + 1] as usize]
    }
    /// The children of the virtual root: the roots, and what several of
    /// them share.
    pub fn top(&self) -> &[u32] {
        let n = self.depth.len();
        &self.kids[self.kid_off[n] as usize..self.kid_off[n + 1] as usize]
    }
    /// A shortest chain from a root down to `v` (both included).
    pub fn chain(&self, v: u32) -> Vec<u32> {
        let mut out = Vec::new();
        if !self.contains(v) {
            return out;
        }
        let mut at = v;
        loop {
            out.push(at);
            match self.parent[at as usize] {
                NONE => break,
                p => at = p,
            }
            if out.len() > u16::MAX as usize {
                break;
            }
        }
        out.reverse();
        out
    }
    /// The chain of dominators from the top down to `v` (both included).
    pub fn dominators(&self, v: u32) -> Vec<u32> {
        let n = self.depth.len() as u32;
        let mut out = Vec::new();
        let mut at = v;
        while at != n && at != NONE && out.len() <= u16::MAX as usize {
            out.push(at);
            at = self.idom[at as usize];
        }
        out.reverse();
        out
    }
}

// ---------------------------------------------------------------------------
// Local names: what a declaration binds itself is not a mention of a
// declaration that happens to have the same name.

/// Words after which the next plain identifiers are bound. Words that
/// are followed by a term (`rcases h with`, `simpa using h`, `show P`)
/// are not here: what follows them may be a declaration.
fn binds(w: &str) -> bool {
    matches!(
        w,
        "have" | "let" | "set" | "obtain" | "intro" | "intros" | "rintro" | "fun" | "λ" | "∀" | "∃" | "∃!" | "Σ" | "∑" | "∏" | "⋃" | "⋂" | "∫" | "assume" | "with" | "ext" | "funext"
            | "by_cases" | "by_contra" | "choose" | "rename_i" | "suffices" | "⨆" | "⨅"
    )
}

/// The identifiers a declaration's text binds: its header's binder names
/// and the names introduced after `have`, `intro`, `fun`, `obtain ⟨…⟩` and
/// their kin. A dotted identifier is never a binder. Lexical and
/// generous: a real reference that shares its name with a local is lost,
/// which is the error to prefer in a list a reader follows.
pub fn local_binders(text: &str, header_len: usize) -> HashSet<&str> {
    let mut out: HashSet<&str> = HashSet::new();
    let mut lx = Lexer::new(text, LexProfile::NEWEST);
    let mut toks: Vec<(TokKind, usize, usize, u32)> = Vec::new();
    while let Some(t) = lx.next_token() {
        if !t.is_comment() {
            toks.push((t.kind, t.start as usize, t.end as usize, t.line));
        }
    }
    let word = |k: usize| &text[toks[k].1..toks[k].2];
    let plain = |k: usize| toks[k].0 == TokKind::Ident && !word(k).contains('.');
    // Header binders: `(a b : T)`, `{a : T}`, `[inst : C]`, `⦃a⦄`.
    let mut depth = 0i32;
    let mut k = 0;
    while k < toks.len() && toks[k].1 < header_len {
        match toks[k].0 {
            TokKind::Open => {
                depth += 1;
                if depth == 1 {
                    // Names up to the `:` of this group.
                    let mut j = k + 1;
                    let mut names = Vec::new();
                    while j < toks.len() && plain(j) {
                        names.push(word(j));
                        j += 1;
                    }
                    // (`[C α]` and `(f x)` are a class and an application,
                    // not binders: only names before a colon are.)
                    if j < toks.len() && word(j) == ":" {
                        out.extend(names);
                    }
                }
            }
            TokKind::Close => depth -= 1,
            _ => {}
        }
        k += 1;
    }
    // After a binding word: the plain identifiers that follow on its line,
    // through patterns, up to what ends the binder list.
    for k in 0..toks.len() {
        if !(toks[k].0 == TokKind::Ident || toks[k].0 == TokKind::Sym) || !binds(word(k)) {
            continue;
        }
        let line = toks[k].3;
        // After these a list of names or patterns follows; after the
        // others, one name.
        let anonymous_ok = matches!(word(k), "obtain" | "rintro" | "with" | "intro" | "fun" | "λ" | "choose" | "ext" | "funext" | "rename_i" | "intros" | "∀" | "∃" | "∃!" | "assume");
        let mut j = k + 1;
        let mut taken = 0;
        // A comma inside a pattern separates its parts; outside it ends
        // the binders (`∀ n, P n`: `P` is not bound).
        let mut depth = 0i32;
        while j < toks.len() && toks[j].3 == line && taken < 24 {
            let w = word(j);
            match toks[j].0 {
                TokKind::Ident if !w.contains('.') => {
                    if matches!(w, "by" | "from" | "using" | "at" | "with" | "in" | "then" | "else" | "do" | "fun" | "show" | "exact") {
                        break;
                    }
                    out.insert(w);
                    taken += 1;
                    if !anonymous_ok {
                        break;
                    }
                }
                TokKind::Open if anonymous_ok => depth += 1,
                TokKind::Close if anonymous_ok && depth > 0 => depth -= 1,
                // `(x : T)`: what follows the colon is the type, up to
                // the bracket that closes the binder.
                TokKind::Sym if anonymous_ok && depth > 0 && w == ":" => {
                    let mut d = 0i32;
                    while j + 1 < toks.len() {
                        match toks[j + 1].0 {
                            TokKind::Open => d += 1,
                            TokKind::Close if d == 0 => break,
                            TokKind::Close => d -= 1,
                            _ => {}
                        }
                        j += 1;
                    }
                }
                TokKind::Sym if anonymous_ok && depth > 0 && matches!(w, "," | "|" | "_" | "-" | "@") => {}
                TokKind::Sym if anonymous_ok && matches!(w, "|" | "_" | "-" | "@") => {}
                _ => break,
            }
            j += 1;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Building.

#[derive(Clone, Debug, Default)]
pub struct BuildOptions {
    pub threads: usize,
    /// Read every file lexically, even where artifacts are usable.
    pub lexical_only: bool,
    /// A directory laid out like a Lake build directory, for packages
    /// that have none of their own.
    pub artifacts: Option<PathBuf>,
}

struct D {
    name: String,
    kind: DeclKind,
    dflags: u32,
    module: u32,
    start: u32,
    kw: u32,
    hend: u32,
    end: u32,
    keyword: String,
}

struct M {
    rel: String,
    /// SHA-256 of the file as the index saw it.
    sha256: Option<String>,
    /// Lean references: build directory and module path below it.
    place: Option<(PathBuf, String)>,
}

/// What every worker reads.
struct Shared<'a> {
    root: &'a Path,
    decls: &'a [D],
    /// Nodes of each module, by start byte.
    by_mod: &'a [Vec<u32>],
    mods: &'a [M],
    first: &'a HashMap<&'a str, u32>,
    dups: &'a HashMap<&'a str, Vec<u32>>,
    namespaces: &'a HashSet<&'a str>,
    /// Local modules by the name Lean uses.
    lake: &'a HashMap<String, u32>,
    lexical_only: bool,
}

#[derive(Default)]
struct ModOut {
    edges: Vec<u64>,
    flags: Vec<(u32, u8)>,
    /// (source | bits, target module, target name)
    unresolved: Vec<(u32, u32, String)>,
    /// Names the `.ilean` gives declarations the index knows otherwise.
    aux: Vec<(u32, String, u32)>,
    /// 0 lexical, 1 hashed, 2 consistent, 3 left out.
    how: u8,
    why_lexical: Option<String>,
}

impl Shared<'_> {
    /// The node called `name`, declared in module `m` when several are.
    fn named(&self, name: &str, m: Option<u32>) -> Option<u32> {
        let v = *self.first.get(name)?;
        if let (Some(all), Some(m)) = (self.dups.get(name), m) {
            if let Some(x) = all.iter().copied().find(|&x| self.decls[x as usize].module == m) {
                return Some(x);
            }
        }
        Some(v)
    }

    /// The innermost declaration of module `m` whose text holds byte `at`.
    fn holder(&self, m: u32, at: u32) -> Option<u32> {
        let v = &self.by_mod[m as usize];
        let k = v.partition_point(|&x| self.decls[x as usize].start <= at);
        v[..k].iter().rev().take(4096).copied().find(|&x| at < self.decls[x as usize].end)
    }
}

fn is_keyword(w: &str) -> bool {
    matches!(
        w,
        "theorem" | "lemma" | "def" | "fun" | "by" | "have" | "show" | "from" | "intro" | "exact" | "apply" | "simp" | "rw"
            | "at" | "with" | "match" | "if" | "then" | "else" | "let" | "in" | "do" | "where" | "Type" | "Prop" | "Sort"
            | "sorry" | "rfl" | "cases" | "induction" | "obtain" | "rcases" | "refine" | "constructor" | "omega" | "decide"
            | "instance" | "structure" | "class" | "inductive" | "abbrev" | "calc" | "using" | "only" | "this"
    )
}

/// Flags read from a declaration's own text.
fn text_flags(d: &D, text: &str) -> u8 {
    let (kw, hend, end) = (d.kw as usize, (d.hend as usize).max(d.kw as usize), d.end as usize);
    if end > text.len() || kw > hend || hend > end || !text.is_char_boundary(kw) || !text.is_char_boundary(hend) || !text.is_char_boundary(end) {
        return 0;
    }
    let mut f = 0u8;
    if d.kind == DeclKind::Theorem {
        f |= nf::THEOREM;
    }
    if d.dflags & dflags::LEX_SORRY != 0 {
        f |= nf::SORRY;
    }
    let header = &text[kw..hend];
    let mut lx = Lexer::new(header, LexProfile::NEWEST);
    let mut toks: Vec<(TokKind, &str)> = Vec::new();
    while let Some(t) = lx.next_token() {
        if !t.is_comment() {
            toks.push((t.kind, t.text(header)));
        }
        if toks.len() > 4096 {
            break;
        }
    }
    let skip = d.keyword.split_whitespace().count().max(1) + (d.dflags & dflags::ANONYMOUS == 0) as usize;
    let rest = toks.get(skip..).unwrap_or(&[]);
    // `: Name`, or `: Name.{u, v}` with universe levels given.
    let bare = match rest {
        [(TokKind::Sym, ":"), (TokKind::Ident, _)] => true,
        [(TokKind::Sym, ":"), (TokKind::Ident, _), (TokKind::Sym, "."), (TokKind::Open, "{"), levels @ .., (TokKind::Close, "}")] => levels.iter().all(|t| t.0 == TokKind::Ident || *t == (TokKind::Sym, ",")),
        _ => false,
    };
    if d.kind == DeclKind::Theorem && bare {
        f |= nf::BARE;
    }
    if matches!(d.kind, DeclKind::Definition | DeclKind::Abbrev | DeclKind::Structure | DeclKind::Class | DeclKind::Inductive) && d.keyword != "field" {
        if let [.., (TokKind::Sym, ":"), (TokKind::Ident, "Prop")] = rest {
            f |= nf::PROP;
        }
    }
    if d.kind == DeclKind::Theorem {
        let body = &text[hend..end];
        let mut lx = Lexer::new(body, LexProfile::NEWEST);
        let mut b: Vec<(TokKind, &str)> = Vec::new();
        while let Some(t) = lx.next_token() {
            if !t.is_comment() {
                b.push((t.kind, t.text(body)));
            }
            if b.len() > 4 {
                break;
            }
        }
        if matches!(b.as_slice(), [(TokKind::Sym, ":="), (TokKind::Ident, _)] | [(TokKind::Sym, ":="), (TokKind::Sym, "@"), (TokKind::Ident, _)]) {
            f |= nf::RELAY;
        }
    }
    f
}

/// The lexical reading of one declaration: its identifiers looked up the
/// way Lean looks a name up, locals left out.
fn lexical_edges(sh: &Shared, v: u32, text: &str, opens: &[String], out: &mut Vec<u64>, buf: &mut String) {
    let d = &sh.decls[v as usize];
    let (kw, hend, end) = (d.kw as usize, d.hend as usize, (d.end as usize).min(text.len()));
    if kw >= end || !text.is_char_boundary(kw) || !text.is_char_boundary(end) {
        return;
    }
    let body = &text[kw..end];
    let locals = local_binders(body, hend.saturating_sub(kw));
    let ns = crate::model::namespace_of(&d.name);
    let comps = crate::model::name_components(ns);
    let inner: Vec<String> = if ns.is_empty() { Vec::new() } else { (1..=comps.len()).rev().map(|k| comps[..k].join(".") + ".").collect() };
    let mut outer: Vec<String> = vec![String::new()];
    for o in opens.iter().take(24) {
        for p in inner.iter().map(|s| s.as_str()).chain(std::iter::once("")) {
            buf.clear();
            buf.push_str(p);
            buf.push_str(o);
            if sh.namespaces.contains(buf.as_str()) {
                let full = format!("{buf}.");
                if !outer.contains(&full) && !inner.contains(&full) {
                    outer.push(full);
                }
            }
        }
    }
    let mut seen: HashMap<&str, u32> = HashMap::new();
    let mut lx = Lexer::new(body, LexProfile::NEWEST);
    while let Some(t) = lx.next_token() {
        if t.kind != TokKind::Ident {
            continue;
        }
        let w = t.text(body);
        if w.len() <= 1 || is_keyword(w) {
            continue;
        }
        let bit = if kw + (t.start as usize) < hend { STMT } else { BODY };
        *seen.entry(w).or_insert(0) |= bit;
        if seen.len() > 600 {
            break;
        }
    }
    for (w, bits) in seen {
        // `_root_.A.b` is `A.b` and nothing else.
        if let Some(rooted) = w.strip_prefix("_root_.") {
            if let Some(x) = sh.named(rooted, Some(d.module)).filter(|&x| x != v) {
                out.push(((v as u64) << 32) | (x | bits) as u64);
            }
            continue;
        }
        let head = w.split('.').next().unwrap_or(w);
        if locals.contains(head) {
            continue;
        }
        let mut hit = None;
        for p in inner.iter().chain(outer.iter()) {
            buf.clear();
            buf.push_str(p);
            buf.push_str(w);
            if buf.as_str() == d.name {
                continue;
            }
            if let Some(x) = sh.named(buf.as_str(), Some(d.module)) {
                if x != v {
                    hit = Some(x);
                    break;
                }
            }
        }
        if let Some(x) = hit {
            out.push(((v as u64) << 32) | (x | bits) as u64);
        }
    }
}

fn process_module(sh: &Shared, m: u32) -> ModOut {
    let mut out = ModOut::default();
    let md = &sh.mods[m as usize];
    let nodes = &sh.by_mod[m as usize];
    let Ok(bytes) = std::fs::read(sh.root.join(&md.rel)) else {
        out.why_lexical = Some("the file could not be read".into());
        return out;
    };
    // The extents of the declarations are those of the text the index
    // saw: a file changed since then is left out, and said to be.
    if md.sha256.as_deref().is_some_and(|h| h != crate::paths::sha256_hex(&bytes)) {
        out.why_lexical = Some("left out: the file changed since it was indexed (scan the project again)".into());
        out.how = 3;
        return out;
    }
    let (text, _) = crate::text::decode_lossy(&bytes);
    let text: &str = &text;
    // Nesting: a field or constructor is part of what its parent states.
    let mut open: Vec<u32> = Vec::new();
    for &v in nodes {
        let d = &sh.decls[v as usize];
        while open.last().is_some_and(|&p| sh.decls[p as usize].end <= d.start) {
            open.pop();
        }
        let mut f = text_flags(d, text);
        if let Some(&p) = open.last() {
            if sh.decls[p as usize].end >= d.end && p != v {
                out.edges.push(((p as u64) << 32) | (v | STMT) as u64);
                f |= nf::NESTED;
            }
        }
        out.flags.push((v, f));
        open.push(v);
    }
    // Lean's references, when the artifacts are of this text.
    let mut lean = None;
    if sh.lexical_only {
        out.why_lexical = Some("asked for".into());
    } else if let Some((lib, mrel)) = &md.place {
        let f = SourceFileText::from_text(sh.root.join(&md.rel), text);
        match crate::leanrefs::module_refs(lib, mrel, &f) {
            Ok((refs, status)) => lean = Some((refs, status, f)),
            Err(e) => out.why_lexical = Some(e.explain()),
        }
    } else {
        out.why_lexical = Some("none: the project has no build directory".into());
    }
    if let Some((refs, status, f)) = lean {
        out.how = if status == crate::leanrefs::RefStatus::Verified { 1 } else { 2 };
        let byte = |line: u32, col: u32| -> Option<u32> {
            let l = line as usize + 1;
            (l <= f.line_count()).then(|| f.byte_of(l, col as usize, crate::text::PosEncoding::Utf16) as u32)
        };
        // Names Lean gives declarations that the index knows by position
        // (an anonymous instance, a name an attribute made).
        for (name, r) in &refs.decls {
            let plain = crate::leanrefs::private_stripped(name);
            if sh.named(plain, Some(m)).is_some_and(|x| sh.decls[x as usize].module == m) {
                continue;
            }
            if let Some(x) = byte(r[4], r[5]).and_then(|b| sh.holder(m, b)) {
                out.aux.push((m, plain.to_string(), x));
            }
        }
        for (module, name, uses) in &refs.refs {
            let Some(&tm) = sh.lake.get(module.as_str()) else { continue };
            let plain = crate::leanrefs::private_stripped(name);
            let target = sh.named(plain, Some(tm)).filter(|&x| sh.decls[x as usize].module == tm);
            for u in uses {
                let Some(b) = byte(u[0], u[1]) else { continue };
                let Some(src) = sh.holder(m, b) else { continue };
                let bits = if b < sh.decls[src as usize].hend { STMT } else { BODY };
                match target {
                    Some(t) if t != src => out.edges.push(((src as u64) << 32) | (t | bits) as u64),
                    Some(_) => {}
                    None => out.unresolved.push((src | bits, tm, plain.to_string())),
                }
            }
        }
        for f in out.flags.iter_mut() {
            f.1 |= nf::LEAN;
        }
        return out;
    }
    // The lexical reading.
    let ls = crate::text::line_starts(text);
    let extents: Vec<(usize, usize)> = nodes.iter().map(|&v| (sh.decls[v as usize].start as usize, sh.decls[v as usize].end as usize)).collect();
    let fs = crate::source::scope::file_scope(text, &ls, &extents);
    let mut buf = String::new();
    for &v in nodes {
        let at = sh.decls[v as usize].start as usize;
        let mut opens: Vec<String> = Vec::new();
        for (e, kind) in fs.in_force(at) {
            if kind == crate::source::scope::CmdKind::Open {
                for o in crate::source::scope::opened(text.get(e.start..e.end).unwrap_or("")) {
                    if !opens.contains(&o) {
                        opens.push(o);
                    }
                }
            }
        }
        lexical_edges(sh, v, text, &opens, &mut out.edges, &mut buf);
    }
    out
}

pub type Progress<'a> = &'a (dyn Fn(&str, u64, u64) + Sync);

/// Whether the graph beside `index` was built for the index as it is now
/// (its header is read, not the graph).
pub fn is_current(index: &Path) -> bool {
    let Ok(conn) = crate::db::open_read(index) else { return false };
    let generation = crate::db::get_meta(&conn, "name_index_generation").ok().flatten().unwrap_or_default();
    let Ok(mut f) = std::fs::File::open(graph_path(index)) else { return false };
    let mut head = [0u8; 16];
    if f.read_exact(&mut head).is_err() || &head[..8] != MAGIC || u32::from_le_bytes(head[8..12].try_into().expect("four")) != VERSION {
        return false;
    }
    let hl = u32::from_le_bytes(head[12..16].try_into().expect("four")) as usize;
    if hl > 1 << 20 {
        return false;
    }
    let mut h = vec![0u8; hl];
    f.read_exact(&mut h).is_ok() && serde_json::from_slice::<Header>(&h).map(|h| h.generation == generation).unwrap_or(false)
}

/// The number of threads a build takes when nobody said.
pub fn default_threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(1, 4)
}

/// Builds the graph of `index` the way `slim graph build` does without
/// options: from Lean's reference files where they are usable (those of
/// an artifact directory recorded beside the index included), lexically
/// elsewhere. Files are read; nothing is run.
pub fn build_default(index: &Path, root: Option<&Path>, threads: Option<usize>, cancel: &AtomicBool, progress: Option<Progress>) -> Result<Graph> {
    let opts = BuildOptions { threads: threads.unwrap_or_else(default_threads), lexical_only: false, artifacts: crate::leanrefs::artifacts_for_index(index) };
    build(index, root, &opts, cancel, progress)
}

/// Builds the graph of the index at `index` and writes it beside it.
pub fn build(index: &Path, root: Option<&Path>, opts: &BuildOptions, cancel: &AtomicBool, progress: Option<Progress>) -> Result<Graph> {
    let t0 = Instant::now();
    let conn = crate::db::open_read(index)?;
    let generation = crate::db::get_meta(&conn, "name_index_generation")?.unwrap_or_default();
    let mode = crate::db::mode_of(&conn)?;
    let g = match mode {
        IndexMode::Semantic => build_semantic(&conn, generation)?,
        IndexMode::Source => {
            let root = root.context("a source index needs its source root to read the files")?;
            build_source(&conn, root, generation, opts, cancel, progress)?
        }
    };
    let mut g = g;
    g.stats.built_at = crate::db::now_unix();
    g.stats.ms = t0.elapsed().as_millis() as u64;
    g.save(&graph_path(index))?;
    Ok(g)
}

fn build_semantic(conn: &Connection, generation: String) -> Result<Graph> {
    let mut st = conn.prepare("SELECT id, kind FROM decls ORDER BY id")?;
    let rows: Vec<(i64, u8)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    let ids: Vec<i64> = rows.iter().map(|r| r.0).collect();
    let flags: Vec<u8> = rows.iter().map(|r| if DeclKind::from_u8(r.1) == DeclKind::Theorem { nf::THEOREM | nf::LEAN } else { nf::LEAN }).collect();
    let node = |id: i64| ids.binary_search(&id).ok().map(|i| i as u64);
    let mut edges = Vec::new();
    let mut st = conn.prepare("SELECT src, kind, dst FROM edges")?;
    for r in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, u8>(1)?, r.get::<_, i64>(2)?)))? {
        let (s, k, d) = r?;
        if let (Some(s), Some(d)) = (node(s), node(d)) {
            if s != d {
                // T and C are what a declaration states; V is its proof or body.
                let bit = if k == crate::model::EdgeKind::Value as u8 { BODY } else { STMT };
                edges.push((s << 32) | (d as u32 | bit) as u64);
            }
        }
    }
    let stats = Stats { source: "lean-deps".into(), ..Default::default() };
    Ok(Graph::from_edges(generation, ids, edges, flags, stats))
}

fn build_source(conn: &Connection, root: &Path, generation: String, opts: &BuildOptions, cancel: &AtomicBool, progress: Option<Progress>) -> Result<Graph> {
    // Modules and where Lake keeps each one's artifacts.
    let mut cache = crate::sourceview::SourceCache::default();
    cache.lean.artifacts = opts.artifacts.clone();
    let mut st = conn.prepare("SELECT id, name, path, sha256 FROM modules WHERE is_local = 1 AND path IS NOT NULL ORDER BY id")?;
    let mrows: Vec<(i64, String, String, Option<String>)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<rusqlite::Result<_>>()?;
    let mut layout = crate::lake::Layout::new(root);
    let mut mods: Vec<M> = Vec::with_capacity(mrows.len());
    let mut mod_ix: HashMap<i64, u32> = HashMap::new();
    let mut lake: HashMap<String, u32> = HashMap::new();
    for (id, name, rel, sha256) in mrows {
        let k = mods.len() as u32;
        mod_ix.insert(id, k);
        let ln = layout.lake_name(&rel);
        // Two packages may both have a module of one name: the first stays.
        lake.entry(ln).or_insert(k);
        lake.entry(name).or_insert(k);
        let place = if opts.lexical_only { None } else { crate::leanrefs::artifact_place(&mut cache, root, &rel) };
        mods.push(M { rel, sha256, place });
    }
    // Declarations with their extents.
    let mut st = conn.prepare(
        "SELECT id, name, kind, flags, module_id, start_byte, kw_byte, header_end_byte, end_byte, keyword FROM decls
          WHERE module_id IS NOT NULL AND start_byte IS NOT NULL AND end_byte IS NOT NULL ORDER BY id",
    )?;
    let mut ids: Vec<i64> = Vec::new();
    let mut decls: Vec<D> = Vec::new();
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, u8>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, i64>(4)?,
            r.get::<_, i64>(5)?,
            r.get::<_, Option<i64>>(6)?,
            r.get::<_, Option<i64>>(7)?,
            r.get::<_, i64>(8)?,
            r.get::<_, Option<String>>(9)?,
        ))
    })?;
    for r in rows {
        let (id, name, kind, fl, mid, start, kw, hend, end, keyword) = r?;
        let Some(&module) = mod_ix.get(&mid) else { continue };
        let kw = kw.unwrap_or(start);
        ids.push(id);
        decls.push(D {
            name,
            kind: DeclKind::from_u8(kind),
            dflags: fl as u32,
            module,
            start: start as u32,
            kw: kw as u32,
            hend: hend.unwrap_or(end).clamp(kw.min(end), end) as u32,
            end: end as u32,
            keyword: keyword.unwrap_or_default(),
        });
    }
    if decls.len() >= NODE as usize {
        bail!("too many declarations for one graph");
    }
    let n = decls.len();
    let mut by_mod: Vec<Vec<u32>> = vec![Vec::new(); mods.len()];
    for (v, d) in decls.iter().enumerate() {
        by_mod[d.module as usize].push(v as u32);
    }
    for v in by_mod.iter_mut() {
        v.sort_by_key(|&x| (decls[x as usize].start, std::cmp::Reverse(decls[x as usize].end)));
    }
    let mut first: HashMap<&str, u32> = HashMap::with_capacity(n);
    let mut dups: HashMap<&str, Vec<u32>> = HashMap::new();
    let mut namespaces: HashSet<&str> = HashSet::new();
    for (v, d) in decls.iter().enumerate() {
        match first.get(d.name.as_str()).copied() {
            Some(f) => {
                dups.entry(d.name.as_str()).or_insert_with(|| vec![f]).push(v as u32);
                // A name means the declaration that is proved, not a
                // restatement with `sorry` in another file.
                if decls[f as usize].dflags & dflags::LEX_SORRY != 0 && d.dflags & dflags::LEX_SORRY == 0 {
                    first.insert(d.name.as_str(), v as u32);
                }
            }
            None => {
                first.insert(d.name.as_str(), v as u32);
            }
        }
        let mut name = d.name.as_str();
        loop {
            let ns = crate::model::namespace_of(name);
            if ns.is_empty() || !namespaces.insert(ns) {
                break;
            }
            name = ns;
        }
    }
    let sh = Shared { root, decls: &decls, by_mod: &by_mod, mods: &mods, first: &first, dups: &dups, namespaces: &namespaces, lake: &lake, lexical_only: opts.lexical_only };
    let threads = opts.threads.clamp(1, 16);
    let next = AtomicUsize::new(0);
    let (tx, rx) = mpsc::sync_channel::<ModOut>(256);
    let mut edges: Vec<u64> = Vec::new();
    let mut flags = vec![0u8; n];
    let mut unresolved: Vec<(u32, u32, String)> = Vec::new();
    let mut aux: HashMap<(u32, String), u32> = HashMap::new();
    let mut stats = Stats { files: mods.len() as u64, artifacts: opts.artifacts.as_ref().map(|p| p.display().to_string()), ..Default::default() };
    let mut why: HashMap<String, u64> = HashMap::new();
    std::thread::scope(|s| {
        for _ in 0..threads {
            let tx = tx.clone();
            let (sh, next) = (&sh, &next);
            s.spawn(move || loop {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= sh.mods.len() {
                    break;
                }
                if tx.send(process_module(sh, i as u32)).is_err() {
                    break;
                }
            });
        }
        drop(tx);
        let mut done = 0u64;
        for o in rx.iter() {
            edges.extend(o.edges);
            for (v, f) in o.flags {
                flags[v as usize] |= f;
            }
            unresolved.extend(o.unresolved);
            for (m, name, v) in o.aux {
                aux.insert((m, name), v);
            }
            match o.how {
                1 => stats.files_hashed += 1,
                2 => stats.files_consistent += 1,
                3 => {
                    stats.files_left_out += 1;
                    *why.entry(o.why_lexical.unwrap_or_default()).or_default() += 1;
                }
                _ => {
                    stats.files_lexical += 1;
                    *why.entry(o.why_lexical.unwrap_or_else(|| "unknown".into())).or_default() += 1;
                }
            }
            done += 1;
            if done % 2000 == 0 {
                if let Some(p) = progress {
                    p("reading references", done, sh.mods.len() as u64);
                }
            }
        }
    });
    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled");
    }
    // A name the index does not have under that spelling: the declaration
    // the `.ilean` places it at; else the declaration it belongs to
    // (`Foo.mk`, `Foo.rec`, `foo._proof_1` are parts of `Foo`, `foo`).
    for (src, tm, name) in unresolved {
        let (bits, s) = (src & (STMT | BODY), src & NODE);
        let mut found = aux.get(&(tm, name.clone())).copied();
        let mut at = name.as_str();
        let mut pops = 0;
        while found.is_none() && pops < 3 {
            let ns = crate::model::namespace_of(at);
            if ns.is_empty() {
                break;
            }
            at = ns;
            pops += 1;
            found = sh.named(at, Some(tm)).filter(|&x| decls[x as usize].module == tm);
        }
        match found {
            Some(t) if t != s => edges.push(((s as u64) << 32) | (t | bits) as u64),
            Some(_) => {}
            None => stats.unresolved += 1,
        }
    }
    stats.source = match (stats.files_hashed + stats.files_consistent, stats.files_lexical) {
        (0, _) => "lexical",
        (_, 0) => "lean-refs",
        _ => "mixed",
    }
    .into();
    let mut fb: Vec<(String, u64)> = why.into_iter().collect();
    fb.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    stats.fallbacks = fb;
    if let Some(p) = progress {
        p("sorting", 0, 1);
    }
    Ok(Graph::from_edges(generation, ids, edges, flags, stats))
}

// ---------------------------------------------------------------------------
// Roots.

/// The declarations a project ends in, when nobody chose any: the theorems
/// of the files with declarations that no declaring file imports, directly
/// or through files that only import. `None` when there are too many to be
/// meant (a library has hundreds of last files).
pub fn import_endpoints(conn: &Connection, max: usize) -> Result<Option<Vec<i64>>> {
    let mut st = conn.prepare("SELECT id, decl_count FROM modules WHERE is_local = 1")?;
    let local: HashMap<i64, i64> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    let mut imports: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut st = conn.prepare("SELECT src, dst FROM module_imports")?;
    for r in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
        let (a, b) = r?;
        if local.contains_key(&a) && local.contains_key(&b) && a != b {
            imports.entry(a).or_default().push(b);
        }
    }
    // Imported by a declaring file, or by an import-only file that is.
    let mut marked: HashSet<i64> = HashSet::new();
    let mut work: Vec<i64> = Vec::new();
    for (&m, &decls) in &local {
        if decls > 0 {
            for &d in imports.get(&m).map(|v| v.as_slice()).unwrap_or(&[]) {
                if marked.insert(d) {
                    work.push(d);
                }
            }
        }
    }
    while let Some(m) = work.pop() {
        if local.get(&m).copied().unwrap_or(0) == 0 {
            for &d in imports.get(&m).map(|v| v.as_slice()).unwrap_or(&[]) {
                if marked.insert(d) {
                    work.push(d);
                }
            }
        }
    }
    let mut files: Vec<i64> = local.iter().filter(|(m, &d)| d > 0 && !marked.contains(m)).map(|(&m, _)| m).collect();
    if files.is_empty() || files.len() > max {
        return Ok(None);
    }
    // In the order of their names: a file's id is the order in which a
    // scan happened to reach it.
    let mut named: Vec<(String, i64)> = Vec::with_capacity(files.len());
    for &m in &files {
        named.push((conn.query_row("SELECT name FROM modules WHERE id = ?1", [m], |r| r.get(0))?, m));
    }
    named.sort();
    files = named.into_iter().map(|x| x.1).collect();
    let mut out = Vec::new();
    // A theorem restated with `sorry` where another file proves it under
    // the same name (a Comparator challenge file) is not what the project
    // ends in: the proof is.
    let mut st = conn.prepare(&format!(
        "SELECT d.id, d.kind FROM decls d WHERE d.module_id = ?1
            AND NOT ((d.flags & {s}) != 0 AND EXISTS (SELECT 1 FROM decls o WHERE o.name = d.name AND o.id != d.id AND (o.flags & {s}) = 0))
          ORDER BY d.start_line, d.id",
        s = dflags::LEX_SORRY
    ))?;
    for m in files {
        let rows: Vec<(i64, u8)> = st.query_map([m], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        let thms: Vec<i64> = rows.iter().filter(|r| DeclKind::from_u8(r.1) == DeclKind::Theorem).map(|r| r.0).collect();
        out.extend(if thms.is_empty() { rows.iter().map(|r| r.0).collect() } else { thms });
        if out.len() > max {
            return Ok(None);
        }
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(n: usize, edges: &[(u32, u32, u32)]) -> Graph {
        let e = edges.iter().map(|&(a, b, bits)| ((a as u64) << 32) | (b | bits) as u64).collect();
        Graph::from_edges("g".into(), (1..=n as i64).collect(), e, vec![nf::THEOREM; n], Stats::default())
    }

    #[test]
    fn cone_gives_depth_chain_dominators_and_exclusive_sizes() {
        // 0 → 1 → 3, 0 → 2 → 3, 3 → 4, 2 → 5; 6 is not reached.
        let g = g(7, &[(0, 1, BODY), (0, 2, BODY), (1, 3, BODY), (2, 3, BODY), (3, 4, STMT), (2, 5, BODY), (6, 0, BODY)]);
        let c = g.cone(&[0], Follow::All);
        assert_eq!(c.reached, 6);
        assert!(!c.contains(6));
        assert_eq!(c.depth[..6], [0, 1, 1, 2, 3, 2]);
        assert_eq!(c.chain(4).len(), 4);
        // 3 is reached two ways: 0 dominates it, not 1 or 2.
        assert_eq!(c.idom[3], 0);
        assert_eq!(c.idom[4], 3);
        assert_eq!(c.idom[5], 2);
        assert_eq!(c.excl[..6], [6, 1, 2, 2, 1, 1]);
        assert_eq!(c.top(), &[0]);
        assert_eq!(c.children(0), &[2, 3, 1]);
        assert_eq!(c.dominators(4), vec![0, 3, 4]);
        // What a theorem states stops at its statement.
        let s = g.cone(&[3], Follow::Says);
        assert_eq!(s.reached, 2);
        assert_eq!(g.users(3).iter().map(|e| e & NODE).collect::<Vec<_>>(), vec![1, 2]);
    }

    #[test]
    fn two_roots_share_what_both_use() {
        let g = g(4, &[(0, 2, BODY), (1, 2, BODY), (2, 3, BODY)]);
        let c = g.cone(&[0, 1], Follow::All);
        assert_eq!(c.idom[2], 4, "the virtual root");
        assert_eq!(c.top(), &[2, 0, 1]);
        assert_eq!(c.excl[2], 2);
    }

    #[test]
    fn a_cycle_does_not_hang_the_walks() {
        let g = g(3, &[(0, 1, BODY), (1, 2, BODY), (2, 1, BODY)]);
        let c = g.cone(&[0], Follow::All);
        assert_eq!(c.reached, 3);
        assert_eq!(c.excl[0], 3);
        assert_eq!(c.idom[2], 1);
    }

    #[test]
    fn binders_are_not_mentions() {
        let t = "theorem foo (key : Nat) {x y : Nat} [Cls T] (hp : Pred (app arg)) : P := by\n  have main := bar key\n  obtain ⟨a, b⟩ := main\n  intro h\n  rcases lem with ⟨c, d⟩ | e\n  exact fun z => baz z h a (∀ n, Q n) (∃ m : Nat, R m) (fun (u : Ty v) (w : Ty2) => S) (∀ (q : Dom), Cod q)";
        let l = local_binders(t, t.find(":=").unwrap());
        for w in ["key", "x", "y", "hp", "main", "a", "b", "c", "d", "e", "h", "z", "n", "m", "u", "w", "q"] {
            assert!(l.contains(w), "{w}");
        }
        // What follows a binder list, or a word that takes a term, is not
        // bound; neither is a class, an application, or a binder's type.
        for w in ["bar", "baz", "P", "foo", "Q", "R", "lem", "Nat", "Cls", "T", "Pred", "app", "arg", "Ty", "v", "Ty2", "S", "Dom", "Cod"] {
            assert!(!l.contains(w), "{w}");
        }
    }

    #[test]
    fn save_and_load_round_trip_and_refuse_another_generation() {
        let d = tempfile::tempdir().unwrap();
        let idx = d.path().join("i.sqlite");
        let a = g(3, &[(0, 1, BODY), (1, 2, STMT | BODY)]);
        a.save(&graph_path(&idx)).unwrap();
        let b = Graph::load(&idx, "g").unwrap().unwrap();
        assert_eq!((b.ids.clone(), b.uses(1).to_vec(), b.users(2).to_vec()), (a.ids.clone(), a.uses(1).to_vec(), a.users(2).to_vec()));
        assert!(Graph::load(&idx, "other").unwrap().is_none());
    }
}
