//! The module import graph, held in memory.
//!
//! It is read from the `modules` and `module_imports` tables (the `import`
//! lines of file headers in a source-only index, `.olean` headers in a
//! semantic one) on the query worker, the first time the import view is
//! opened, and kept for the session. Everything here is about files
//! importing files: build dependencies, not mathematical ones.
//!
//! Vocabulary, as shown to the reader:
//!
//! * **depth** of a local file: the longest chain of local imports below it
//!   (0: it imports no file of the project);
//! * **endpoint**: a local file with declarations that no local file with
//!   declarations imports (import-only umbrella files do not count as
//!   importers, otherwise one `import` of everything would hide every
//!   endpoint);
//! * **hub**: a local file many local files import directly.

use anyhow::Result;
use rusqlite::Connection;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Node {
    pub id: i64,
    pub name: String,
    pub local: bool,
    pub decls: u32,
}

/// Adjacency in compressed rows: `adj[start[v]..start[v + 1]]`.
#[derive(Clone, Debug, Default)]
struct Csr {
    start: Vec<u32>,
    adj: Vec<u32>,
}

impl Csr {
    fn build(n: usize, edges: impl Iterator<Item = (u32, u32)> + Clone) -> Csr {
        let mut start = vec![0u32; n + 1];
        for (a, _) in edges.clone() {
            start[a as usize + 1] += 1;
        }
        for i in 0..n {
            start[i + 1] += start[i];
        }
        let mut fill = start.clone();
        let mut adj = vec![0u32; start[n] as usize];
        for (a, b) in edges {
            adj[fill[a as usize] as usize] = b;
            fill[a as usize] += 1;
        }
        Csr { start, adj }
    }
    fn of(&self, v: u32) -> &[u32] {
        &self.adj[self.start[v as usize] as usize..self.start[v as usize + 1] as usize]
    }
}

/// A node of the directory tree of local files (dotted module names).
#[derive(Clone, Debug)]
pub struct DirNode {
    pub label: String,
    pub full: String,
    pub parent: Option<u32>,
    /// Ordered from the bottom of the import order to the top: by the
    /// deepest file, then the shallowest, then by name.
    pub children: Vec<u32>,
    /// The file with exactly this name, if there is one.
    pub module: Option<u32>,
    pub level: u16,
    pub files: u32,
    pub decls: u32,
    /// Depth range of the files below (files with declarations, when any).
    pub dmin: u32,
    pub dmax: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DirSummary {
    pub full: String,
    /// Other parts of the project the files below import, most imports
    /// first: `(directory or file, number of import lines)`.
    pub imports_from: Vec<(String, u32)>,
    pub imported_by: Vec<(String, u32)>,
    /// Imports of modules outside the project, by first name component.
    pub external: Vec<(String, u32)>,
    /// Import lines between two files below this node.
    pub inside: u32,
    pub out_total: u32,
    pub in_total: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Found {
    Exact(u32),
    /// Not a full module name, but exactly one module ends with it.
    Unique(u32),
    Ambiguous { count: usize, examples: Vec<String> },
    Missing,
}

/// The answer to "why does A import B".
#[derive(Clone, Debug, PartialEq)]
pub enum PathAnswer {
    /// A chain of import lines from the first module to the last.
    Path(Vec<String>),
    /// Every module reachable through imports was visited: there is none.
    NoPath { from: String, to: String, reached: usize },
    Unknown { text: String, examples: Vec<String>, count: usize },
}

pub struct ImportGraph {
    /// Every module of the index, in name order.
    pub nodes: Vec<Node>,
    imports: Csr,
    importers: Csr,
    /// Longest chain of local imports below each local module.
    pub depth: Vec<u32>,
    /// Local modules on or above an import cycle (their depth is a lower bound).
    pub cyclic: Vec<bool>,
    pub cyclic_count: u32,
    pub local_importers: Vec<u32>,
    /// Local importers that declare something themselves.
    pub declaring_importers: Vec<u32>,
    pub dirs: Vec<DirNode>,
    pub roots: Vec<u32>,
    /// Directory-tree node of each local module.
    dir_of: Vec<u32>,
    dir_by_name: HashMap<String, u32>,
    pub endpoints: Vec<u32>,
    /// Most imported local modules first (at least two importers).
    pub hubs: Vec<u32>,
    pub import_only: Vec<u32>,
    pub local_count: u32,
    pub local_edges: u32,
    pub external_edges: u32,
    pub max_depth: u32,
    pub load_ms: f64,
}

const NO_DIR: u32 = u32::MAX;
pub const HUBS_SHOWN: usize = 40;

impl ImportGraph {
    pub fn load(conn: &Connection) -> Result<ImportGraph> {
        let t = std::time::Instant::now();
        let mut st = conn.prepare("SELECT id, name, is_local, decl_count FROM modules ORDER BY name")?;
        let nodes: Vec<Node> = st
            .query_map([], |r| {
                Ok(Node { id: r.get(0)?, name: r.get(1)?, local: r.get::<_, i64>(2)? != 0, decls: r.get::<_, i64>(3)?.clamp(0, u32::MAX as i64) as u32 })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let max_id = nodes.iter().map(|n| n.id).max().unwrap_or(0).max(0) as usize;
        let mut index_of = vec![u32::MAX; max_id + 1];
        for (i, n) in nodes.iter().enumerate() {
            if n.id >= 0 {
                index_of[n.id as usize] = i as u32;
            }
        }
        let mut st = conn.prepare("SELECT src, dst FROM module_imports")?;
        let mut edges: Vec<(u32, u32)> = Vec::new();
        for row in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
            let (a, b) = row?;
            let ix = |x: i64| index_of.get(x as usize).copied().filter(|&i| i != u32::MAX);
            if let (Some(a), Some(b)) = (ix(a), ix(b)) {
                if a != b {
                    edges.push((a, b));
                }
            }
        }
        let mut g = ImportGraph::build(nodes, edges);
        g.load_ms = t.elapsed().as_secs_f64() * 1000.0;
        Ok(g)
    }

    /// `nodes` in name order; `edges` are `(importer, imported)` indexes.
    pub fn build(nodes: Vec<Node>, edges: Vec<(u32, u32)>) -> ImportGraph {
        let n = nodes.len();
        let imports = Csr::build(n, edges.iter().copied());
        let importers = Csr::build(n, edges.iter().map(|&(a, b)| (b, a)));
        let mut g = ImportGraph {
            nodes,
            imports,
            importers,
            depth: vec![0; n],
            cyclic: vec![false; n],
            cyclic_count: 0,
            local_importers: vec![0; n],
            declaring_importers: vec![0; n],
            dirs: Vec::new(),
            roots: Vec::new(),
            dir_of: vec![NO_DIR; n],
            dir_by_name: HashMap::new(),
            endpoints: Vec::new(),
            hubs: Vec::new(),
            import_only: Vec::new(),
            local_count: 0,
            local_edges: 0,
            external_edges: 0,
            max_depth: 0,
            load_ms: 0.0,
        };
        g.analyse();
        g.build_dirs();
        g
    }

    fn analyse(&mut self) {
        let n = self.nodes.len();
        // Depth: process a file once all its local imports are done.
        let mut waiting = vec![0u32; n];
        let mut ready: Vec<u32> = Vec::new();
        for v in 0..n {
            if !self.nodes[v].local {
                continue;
            }
            self.local_count += 1;
            for &w in self.imports.of(v as u32) {
                if self.nodes[w as usize].local {
                    waiting[v] += 1;
                    self.local_edges += 1;
                    self.local_importers[w as usize] += 1;
                    if self.nodes[v].decls > 0 {
                        self.declaring_importers[w as usize] += 1;
                    }
                } else {
                    self.external_edges += 1;
                }
            }
            if waiting[v] == 0 {
                ready.push(v as u32);
            }
        }
        let mut done = 0u32;
        while let Some(v) = ready.pop() {
            done += 1;
            let d = self.depth[v as usize];
            self.max_depth = self.max_depth.max(d);
            for &p in self.importers.of(v) {
                let p = p as usize;
                if !self.nodes[p].local {
                    continue;
                }
                self.depth[p] = self.depth[p].max(d + 1);
                waiting[p] -= 1;
                if waiting[p] == 0 {
                    ready.push(p as u32);
                }
            }
        }
        if done < self.local_count {
            for v in 0..n {
                if self.nodes[v].local && waiting[v] > 0 {
                    self.cyclic[v] = true;
                    self.cyclic_count += 1;
                    self.max_depth = self.max_depth.max(self.depth[v]);
                }
            }
        }
        for v in 0..n as u32 {
            let node = &self.nodes[v as usize];
            if !node.local {
                continue;
            }
            if node.decls > 0 && self.declaring_importers[v as usize] == 0 {
                self.endpoints.push(v);
            }
            if node.decls == 0 && !self.imports.of(v).is_empty() {
                self.import_only.push(v);
            }
            if self.local_importers[v as usize] >= 2 {
                self.hubs.push(v);
            }
        }
        // Endpoints from the top of the import order down.
        let depth = &self.depth;
        self.endpoints.sort_by_key(|&v| (std::cmp::Reverse(depth[v as usize]), v));
        let li = &self.local_importers;
        self.hubs.sort_by_key(|&v| (std::cmp::Reverse(li[v as usize]), v));
        self.hubs.truncate(HUBS_SHOWN);
    }

    fn build_dirs(&mut self) {
        // Declaring files decide a directory's range; import-only files
        // only when it has nothing else.
        let mut all: Vec<(u32, u32)> = Vec::new();
        for v in 0..self.nodes.len() {
            if !self.nodes[v].local {
                continue;
            }
            let name = self.nodes[v].name.clone();
            let comps = crate::model::name_components(&name);
            let mut parent: Option<u32> = None;
            let mut full = String::new();
            for (k, c) in comps.iter().enumerate() {
                if k > 0 {
                    full.push('.');
                }
                full.push_str(c);
                let idx = match self.dir_by_name.get(&full) {
                    Some(&i) => i,
                    None => {
                        let i = self.dirs.len() as u32;
                        self.dirs.push(DirNode {
                            label: c.to_string(),
                            full: full.clone(),
                            parent,
                            children: Vec::new(),
                            module: None,
                            level: k as u16,
                            files: 0,
                            decls: 0,
                            dmin: u32::MAX,
                            dmax: 0,
                        });
                        all.push((u32::MAX, 0));
                        self.dir_by_name.insert(full.clone(), i);
                        match parent {
                            Some(p) => self.dirs[p as usize].children.push(i),
                            None => self.roots.push(i),
                        }
                        i
                    }
                };
                let (d, decls) = (self.depth[v], self.nodes[v].decls);
                let dn = &mut self.dirs[idx as usize];
                dn.files += 1;
                dn.decls += decls;
                if decls > 0 {
                    dn.dmin = dn.dmin.min(d);
                    dn.dmax = dn.dmax.max(d);
                }
                let a = &mut all[idx as usize];
                *a = (a.0.min(d), a.1.max(d));
                parent = Some(idx);
            }
            if let Some(p) = parent {
                self.dirs[p as usize].module = Some(v as u32);
                self.dir_of[v] = p;
            }
        }
        for (i, d) in self.dirs.iter_mut().enumerate() {
            if d.dmin == u32::MAX {
                (d.dmin, d.dmax) = if all[i].0 == u32::MAX { (0, 0) } else { all[i] };
            }
        }
        let key = |dirs: &Vec<DirNode>, i: u32| (dirs[i as usize].dmax, dirs[i as usize].dmin);
        for i in 0..self.dirs.len() {
            let mut ch = std::mem::take(&mut self.dirs[i].children);
            ch.sort_by(|&a, &b| key(&self.dirs, a).cmp(&key(&self.dirs, b)).then_with(|| self.dirs[a as usize].label.cmp(&self.dirs[b as usize].label)));
            self.dirs[i].children = ch;
        }
        let mut roots = std::mem::take(&mut self.roots);
        roots.sort_by(|&a, &b| key(&self.dirs, a).cmp(&key(&self.dirs, b)).then_with(|| self.dirs[a as usize].label.cmp(&self.dirs[b as usize].label)));
        self.roots = roots;
    }

    pub fn imports_of(&self, v: u32) -> &[u32] {
        self.imports.of(v)
    }

    pub fn importers_of(&self, v: u32) -> &[u32] {
        self.importers.of(v)
    }

    pub fn dir(&self, full: &str) -> Option<u32> {
        self.dir_by_name.get(full).copied()
    }

    pub fn node(&self, name: &str) -> Option<u32> {
        self.nodes.binary_search_by(|n| n.name.as_str().cmp(name)).ok().map(|i| i as u32)
    }

    /// A module by its name, a `Dir/File.lean` path, or an unambiguous end
    /// of its name.
    pub fn find(&self, text: &str) -> Found {
        let t = text.trim();
        let t = match t.strip_suffix(".lean") {
            Some(p) => p.replace('/', "."),
            None => t.to_string(),
        };
        if t.is_empty() {
            return Found::Missing;
        }
        if let Some(i) = self.node(&t) {
            return Found::Exact(i);
        }
        let tail = format!(".{t}");
        let hits: Vec<u32> = (0..self.nodes.len() as u32).filter(|&i| self.nodes[i as usize].name.ends_with(&tail)).collect();
        match hits.len() {
            0 => Found::Missing,
            1 => Found::Unique(hits[0]),
            n => Found::Ambiguous { count: n, examples: hits.iter().take(5).map(|&i| self.nodes[i as usize].name.clone()).collect() },
        }
    }

    /// A shortest chain of imports from `from` to `to`, if `from` imports
    /// `to` directly or through other modules. The whole reachable set is
    /// searched (the graph is in memory), so `None` is an answer.
    pub fn path(&self, from: u32, to: u32) -> (Option<Vec<u32>>, usize) {
        let n = self.nodes.len();
        let mut prev = vec![u32::MAX; n];
        let mut queue = std::collections::VecDeque::new();
        prev[from as usize] = from;
        queue.push_back(from);
        let mut reached = 0usize;
        while let Some(v) = queue.pop_front() {
            if v == to {
                let mut p = vec![v];
                let mut x = v;
                while x != from {
                    x = prev[x as usize];
                    p.push(x);
                }
                p.reverse();
                return (Some(p), reached);
            }
            reached += 1;
            for &w in self.imports.of(v) {
                if prev[w as usize] == u32::MAX {
                    prev[w as usize] = v;
                    queue.push_back(w);
                }
            }
        }
        (None, reached.saturating_sub(1))
    }

    /// "Why does `from` import `to`": both given as text.
    pub fn explain(&self, from: &str, to: &str) -> PathAnswer {
        let pick = |text: &str| -> std::result::Result<u32, PathAnswer> {
            match self.find(text) {
                Found::Exact(i) | Found::Unique(i) => Ok(i),
                Found::Ambiguous { count, examples } => Err(PathAnswer::Unknown { text: text.trim().to_string(), examples, count }),
                Found::Missing => Err(PathAnswer::Unknown { text: text.trim().to_string(), examples: Vec::new(), count: 0 }),
            }
        };
        let (a, b) = match (pick(from), pick(to)) {
            (Ok(a), Ok(b)) => (a, b),
            (Err(e), _) | (_, Err(e)) => return e,
        };
        match self.path(a, b) {
            (Some(p), _) => PathAnswer::Path(p.into_iter().map(|i| self.nodes[i as usize].name.clone()).collect()),
            (None, reached) => PathAnswer::NoPath { from: self.nodes[a as usize].name.clone(), to: self.nodes[b as usize].name.clone(), reached },
        }
    }

    /// One longest chain of local imports below `v` (a witness of its depth).
    pub fn chain_below(&self, v: u32) -> Vec<u32> {
        let mut out = vec![v];
        let mut cur = v;
        for _ in 0..=self.depth[v as usize] {
            let next = self
                .imports
                .of(cur)
                .iter()
                .copied()
                .filter(|&w| self.nodes[w as usize].local && self.depth[w as usize] < self.depth[cur as usize])
                .max_by_key(|&w| (self.depth[w as usize], std::cmp::Reverse(w)));
            match next {
                Some(w) => {
                    out.push(w);
                    cur = w;
                }
                None => break,
            }
        }
        out
    }

    fn ancestor_at(&self, mut d: u32, level: u16) -> u32 {
        while self.dirs[d as usize].level > level {
            match self.dirs[d as usize].parent {
                Some(p) => d = p,
                None => break,
            }
        }
        d
    }

    /// Local modules below a directory-tree node (itself included).
    pub fn modules_below(&self, dir: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut stack = vec![dir];
        while let Some(d) = stack.pop() {
            let n = &self.dirs[d as usize];
            if let Some(m) = n.module {
                out.push(m);
            }
            stack.extend(n.children.iter().copied());
        }
        out
    }

    /// What the files below `dir` import from the rest of the project and
    /// what imports them, grouped by directory at the same level.
    pub fn dir_summary(&self, dir: u32, top: usize) -> DirSummary {
        let level = self.dirs[dir as usize].level;
        let mut s = DirSummary { full: self.dirs[dir as usize].full.clone(), ..Default::default() };
        let (mut out, mut inn): (HashMap<u32, u32>, HashMap<u32, u32>) = (HashMap::new(), HashMap::new());
        let mut ext: HashMap<&str, u32> = HashMap::new();
        let below = |v: u32| -> Option<u32> {
            let d = self.dir_of[v as usize];
            (d != NO_DIR).then(|| self.ancestor_at(d, level))
        };
        for v in self.modules_below(dir) {
            for &w in self.imports.of(v) {
                match below(w) {
                    Some(g) if g == dir => s.inside += 1,
                    Some(g) => {
                        *out.entry(g).or_default() += 1;
                        s.out_total += 1;
                    }
                    None => {
                        let name = self.nodes[w as usize].name.as_str();
                        *ext.entry(crate::model::name_components(name)[0]).or_default() += 1;
                    }
                }
            }
            for &p in self.importers.of(v) {
                if let Some(g) = below(p).filter(|&g| g != dir) {
                    *inn.entry(g).or_default() += 1;
                    s.in_total += 1;
                }
            }
        }
        let rank = |m: HashMap<u32, u32>| -> Vec<(String, u32)> {
            let mut v: Vec<(u32, u32)> = m.into_iter().collect();
            v.sort_by_key(|&(g, n)| (std::cmp::Reverse(n), g));
            v.into_iter().take(top).map(|(g, n)| (self.dirs[g as usize].full.clone(), n)).collect()
        };
        s.imports_from = rank(out);
        s.imported_by = rank(inn);
        let mut e: Vec<(String, u32)> = ext.into_iter().map(|(k, n)| (k.to_string(), n)).collect();
        e.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        e.truncate(top);
        s.external = e;
        s
    }

    /// Memory held, roughly (for the index information tab).
    pub fn memory_bytes(&self) -> usize {
        self.nodes.iter().map(|n| n.name.len() + 40).sum::<usize>()
            + (self.imports.adj.len() + self.importers.adj.len()) * 4
            + self.nodes.len() * 28
            + self.dirs.iter().map(|d| d.full.len() * 2 + d.label.len() + d.children.len() * 4 + 96).sum::<usize>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(name, local, decls, imports…)`.
    fn graph(spec: &[(&str, bool, u32, &[&str])]) -> ImportGraph {
        let mut names: Vec<&str> = spec.iter().map(|s| s.0).collect();
        names.sort();
        let ix = |n: &str| names.binary_search(&n).unwrap() as u32;
        let mut nodes: Vec<Node> = spec.iter().map(|s| Node { id: ix(s.0) as i64 + 1, name: s.0.into(), local: s.1, decls: s.2 }).collect();
        nodes.sort_by(|a, b| a.name.cmp(&b.name));
        let edges = spec.iter().flat_map(|s| s.3.iter().map(|t| (ix(s.0), ix(t)))).collect();
        ImportGraph::build(nodes, edges)
    }

    fn sample() -> ImportGraph {
        graph(&[
            ("P", true, 0, &["P.Found.Basic", "P.Found.Order", "P.Alg.Group", "P.Main.Result", "P.Main.Other"]),
            ("P.Found.Basic", true, 5, &["Lib.Core"]),
            ("P.Found.Order", true, 3, &["P.Found.Basic", "Lib.Core", "Lib.Data.List"]),
            ("P.Alg.Group", true, 9, &["P.Found.Order", "P.Found.Basic"]),
            ("P.Main.Result", true, 2, &["P.Alg.Group"]),
            ("P.Main.Other", true, 1, &["P.Found.Basic"]),
            ("Lib.Core", false, 0, &[]),
            ("Lib.Data.List", false, 0, &["Lib.Core"]),
        ])
    }

    fn names(g: &ImportGraph, v: &[u32]) -> Vec<String> {
        v.iter().map(|&i| g.nodes[i as usize].name.clone()).collect()
    }

    #[test]
    fn depth_is_the_longest_local_chain_and_endpoints_ignore_umbrella_files() {
        let g = sample();
        let d = |n: &str| g.depth[g.node(n).unwrap() as usize];
        assert_eq!((d("P.Found.Basic"), d("P.Found.Order"), d("P.Alg.Group"), d("P.Main.Result"), d("P.Main.Other")), (0, 1, 2, 3, 1));
        assert_eq!(d("P"), 4, "the umbrella file sits above everything it imports");
        assert_eq!(g.max_depth, 4);
        assert_eq!((g.local_count, g.local_edges, g.external_edges), (6, 10, 3));
        // `P` imports everything but declares nothing, so it hides no endpoint.
        assert_eq!(names(&g, &g.endpoints), vec!["P.Main.Result", "P.Main.Other"]);
        assert_eq!(names(&g, &g.import_only), vec!["P"]);
        assert_eq!(names(&g, &g.hubs)[0], "P.Found.Basic");
        assert_eq!(g.local_importers[g.node("P.Found.Basic").unwrap() as usize], 4);
        assert_eq!(g.cyclic_count, 0);
        assert_eq!(names(&g, &g.chain_below(g.node("P.Main.Result").unwrap())), vec!["P.Main.Result", "P.Alg.Group", "P.Found.Order", "P.Found.Basic"]);
    }

    #[test]
    fn directories_are_ordered_bottom_up_and_summarised_by_sibling() {
        let g = sample();
        assert_eq!(g.roots.len(), 1);
        let p = &g.dirs[g.roots[0] as usize];
        assert_eq!((p.files, p.decls, p.dmin, p.dmax), (6, 20, 0, 3), "the range ignores the import-only file");
        let kids: Vec<(&str, u32, u32)> = p.children.iter().map(|&c| (g.dirs[c as usize].label.as_str(), g.dirs[c as usize].dmin, g.dirs[c as usize].dmax)).collect();
        assert_eq!(kids, vec![("Found", 0, 1), ("Alg", 2, 2), ("Main", 1, 3)]);
        let s = g.dir_summary(g.dir("P.Main").unwrap(), 10);
        assert_eq!(s.imports_from, vec![("P.Alg".to_string(), 1), ("P.Found".to_string(), 1)]);
        assert_eq!(s.imported_by, vec![("P".to_string(), 2)], "the umbrella file P.lean is outside P/Main");
        assert_eq!((s.inside, s.out_total, s.in_total), (0, 2, 2));
        let s = g.dir_summary(g.dir("P.Found").unwrap(), 10);
        assert_eq!(s.imported_by, vec![("P".to_string(), 2), ("P.Alg".to_string(), 2), ("P.Main".to_string(), 1)]);
        assert_eq!(s.external, vec![("Lib".to_string(), 3)]);
        assert_eq!(s.inside, 1);
        assert_eq!(names(&g, &g.modules_below(g.dir("P.Found").unwrap())).len(), 2);
    }

    #[test]
    fn import_paths_are_exhaustive_and_names_are_found_leniently() {
        let g = sample();
        assert_eq!(
            g.explain("P.Main.Result", "Lib.Data.List"),
            PathAnswer::Path(vec!["P.Main.Result".into(), "P.Alg.Group".into(), "P.Found.Order".into(), "Lib.Data.List".into()])
        );
        // A file path and an unambiguous tail are accepted.
        assert_eq!(g.find("P/Alg/Group.lean"), Found::Exact(g.node("P.Alg.Group").unwrap()));
        assert_eq!(g.find("Group"), Found::Unique(g.node("P.Alg.Group").unwrap()));
        assert!(matches!(g.find("Basic"), Found::Unique(_)));
        assert_eq!(g.find("Nope"), Found::Missing);
        match g.explain("P.Found.Basic", "P.Main.Result") {
            PathAnswer::NoPath { reached, .. } => assert_eq!(reached, 1, "only Lib.Core is below it"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(g.explain("P.Found.Basic", "Missing.Module"), PathAnswer::Unknown { count: 0, .. }));
        let amb = graph(&[("A.X", true, 1, &[]), ("B.X", true, 1, &[])]);
        assert!(matches!(amb.find("X"), Found::Ambiguous { count: 2, .. }));
    }

    #[test]
    fn cycles_terminate_and_are_reported() {
        let g = graph(&[("A", true, 1, &["B"]), ("B", true, 1, &["C"]), ("C", true, 1, &["B"]), ("D", true, 1, &[])]);
        assert_eq!(g.cyclic_count, 3, "B and C are on the cycle, A is above it");
        assert!(!g.cyclic[g.node("D").unwrap() as usize]);
        assert!(g.chain_below(g.node("A").unwrap()).len() <= 4);
        assert!(g.path(g.node("A").unwrap(), g.node("D").unwrap()).0.is_none());
    }

    #[test]
    fn loads_from_an_index() {
        use crate::paths::Project;
        use crate::source::index::{build_source_index, ScanConfig};
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("proj");
        for (rel, text) in [
            ("lakefile.toml", "name = \"p\"\n"),
            ("P.lean", "import P.A\nimport P.B\n"),
            ("P/A.lean", "import Mathlib.Data.Nat.Basic\ntheorem a : True := trivial\n"),
            ("P/B.lean", "import P.A\ntheorem b : True := trivial\n"),
        ] {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        let project = Project::open(&root).unwrap();
        let idx = d.path().join("i.sqlite");
        let cfg = ScanConfig { threads: 1, max_file_bytes: 1 << 20, profile: crate::source::lexer::LexProfile::NEWEST };
        build_source_index(&project, &idx, &cfg, &std::sync::atomic::AtomicBool::new(false), None).unwrap();
        let g = ImportGraph::load(&crate::db::open_read(&idx).unwrap()).unwrap();
        assert_eq!((g.local_count, g.local_edges, g.external_edges), (3, 3, 1));
        assert_eq!(names(&g, &g.endpoints), vec!["P.B"]);
        assert_eq!(g.depth[g.node("P.B").unwrap() as usize], 1);
        assert!(!g.nodes[g.node("Mathlib.Data.Nat.Basic").unwrap() as usize].local);
    }
}
