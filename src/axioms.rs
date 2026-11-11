//! Transitive axiom analysis with witness paths.
//!
//! The relation followed is exactly the one Lean's `collectAxioms` follows
//! (checked in `Lean/Util/CollectAxioms.lean` for the supported toolchains):
//! constants in types, in values of definitions/theorems/opaques, and from an
//! inductive type to its constructors. Cycles (an inductive and its
//! constructors, unsafe recursion) are handled by strongly connected
//! components, so every member of a cycle gets the full set.
//!
//! A pseudo-axiom (index 0, [`INCOMPLETE`]) marks declarations from which a
//! node with unavailable term data is reachable: for those the reported set
//! is a lower bound. For every (declaration, axiom) pair a `via` successor is
//! recorded so that a concrete path from the declaration to the axiom (or to
//! the coverage gap) can be shown.

/// Index of the coverage-gap pseudo-axiom.
pub const INCOMPLETE: u32 = 0;

/// Compressed sparse adjacency over dense node indices `0..n`.
pub struct Graph {
    pub offsets: Vec<u32>,
    pub targets: Vec<u32>,
    pub kinds: Vec<u8>,
}

impl Graph {
    /// Builds a graph from `(src, kind, dst)` triples (any order).
    pub fn from_edges(n: usize, mut edges: Vec<(u32, u8, u32)>) -> Graph {
        edges.sort_unstable();
        edges.dedup();
        let mut offsets = vec![0u32; n + 1];
        for &(s, _, _) in &edges {
            offsets[s as usize + 1] += 1;
        }
        for i in 0..n {
            offsets[i + 1] += offsets[i];
        }
        let targets = edges.iter().map(|e| e.2).collect();
        let kinds = edges.iter().map(|e| e.1).collect();
        Graph { offsets, targets, kinds }
    }

    pub fn n(&self) -> usize {
        self.offsets.len() - 1
    }

    pub fn succ(&self, v: usize) -> impl Iterator<Item = (u32, u8)> + '_ {
        let (a, b) = (self.offsets[v] as usize, self.offsets[v + 1] as usize);
        self.targets[a..b].iter().copied().zip(self.kinds[a..b].iter().copied())
    }
}

/// Strongly connected components (iterative Tarjan). Returns the component
/// id of every node; component ids are assigned in reverse topological
/// order, i.e. every edge goes from a higher-or-equal id to a lower-or-equal
/// id, and components reachable from `c` have ids `<= c`.
pub fn scc(g: &Graph) -> (Vec<u32>, u32) {
    let n = g.n();
    const UNVISITED: u32 = u32::MAX;
    let mut index = vec![UNVISITED; n];
    let mut low = vec![0u32; n];
    let mut on_stack = vec![false; n];
    let mut comp = vec![UNVISITED; n];
    let mut stack: Vec<u32> = Vec::new();
    let mut next_index = 0u32;
    let mut next_comp = 0u32;
    // (node, next edge offset)
    let mut call: Vec<(u32, u32)> = Vec::new();
    for root in 0..n {
        if index[root] != UNVISITED {
            continue;
        }
        call.push((root as u32, g.offsets[root]));
        index[root] = next_index;
        low[root] = next_index;
        next_index += 1;
        stack.push(root as u32);
        on_stack[root] = true;
        while let Some(&mut (v, ref mut ei)) = call.last_mut() {
            let v = v as usize;
            let end = g.offsets[v + 1];
            if *ei < end {
                let w = g.targets[*ei as usize] as usize;
                *ei += 1;
                if index[w] == UNVISITED {
                    index[w] = next_index;
                    low[w] = next_index;
                    next_index += 1;
                    stack.push(w as u32);
                    on_stack[w] = true;
                    call.push((w as u32, g.offsets[w]));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
            } else {
                call.pop();
                if let Some(&(parent, _)) = call.last() {
                    let p = parent as usize;
                    low[p] = low[p].min(low[v]);
                }
                if low[v] == index[v] {
                    loop {
                        let w = stack.pop().expect("tarjan stack") as usize;
                        on_stack[w] = false;
                        comp[w] = next_comp;
                        if w == v {
                            break;
                        }
                    }
                    next_comp += 1;
                }
            }
        }
    }
    (comp, next_comp)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Witness {
    pub node: u32,
    /// Axiom index (0 = coverage gap; `k >= 1` is `axioms[k - 1]`).
    pub axiom: u32,
    /// Next hop towards the axiom and the edge kind used, or `None` when the
    /// node itself is the axiom (or itself has unavailable data).
    pub via: Option<(u32, u8)>,
}

pub struct Analysis {
    /// Node indices of the axioms, in index order (index `k` is `axioms[k-1]`).
    pub axioms: Vec<u32>,
    pub witnesses: Vec<Witness>,
}

/// Runs the analysis. `is_axiom[v]` marks axiom nodes; `incomplete[v]` marks
/// nodes whose own type/value references are not fully known.
pub fn analyze(g: &Graph, is_axiom: &[bool], incomplete: &[bool]) -> Analysis {
    let n = g.n();
    let axioms: Vec<u32> = (0..n as u32).filter(|&v| is_axiom[v as usize]).collect();
    let mut ax_index = vec![0u32; n];
    for (k, &v) in axioms.iter().enumerate() {
        ax_index[v as usize] = k as u32 + 1;
    }
    let bits = axioms.len() + 1;
    let words = bits.div_ceil(64);
    let (comp, ncomp) = scc(g);
    // Members per component.
    let mut comp_start = vec![0u32; ncomp as usize + 1];
    for &c in &comp {
        comp_start[c as usize + 1] += 1;
    }
    for i in 0..ncomp as usize {
        comp_start[i + 1] += comp_start[i];
    }
    let mut fill = comp_start.clone();
    let mut members = vec![0u32; n];
    for v in 0..n {
        let c = comp[v] as usize;
        members[fill[c] as usize] = v as u32;
        fill[c] += 1;
    }
    let mut comp_set = vec![0u64; ncomp as usize * words];
    let own_bit = |v: usize| -> Option<usize> {
        if is_axiom[v] {
            Some(ax_index[v] as usize)
        } else {
            None
        }
    };
    let mut witnesses = Vec::new();
    let mut acc = vec![0u64; words];
    for c in 0..ncomp as usize {
        let ms = &members[comp_start[c] as usize..comp_start[c + 1] as usize];
        acc.iter_mut().for_each(|w| *w = 0);
        for &v in ms {
            let v = v as usize;
            if let Some(b) = own_bit(v) {
                acc[b / 64] |= 1 << (b % 64);
            }
            if incomplete[v] {
                acc[0] |= 1;
            }
            for (w, _) in g.succ(v) {
                let cw = comp[w as usize] as usize;
                if cw != c {
                    // components reachable from c were processed already
                    let src = &comp_set[cw * words..cw * words + words];
                    for (a, s) in acc.iter_mut().zip(src) {
                        *a |= *s;
                    }
                }
            }
        }
        comp_set[c * words..c * words + words].copy_from_slice(&acc);
        // Witnesses.
        let set_bits: Vec<usize> =
            (0..bits).filter(|&b| acc[b / 64] >> (b % 64) & 1 == 1).collect();
        if set_bits.is_empty() {
            continue;
        }
        let has = |cs: &[u64], cw: usize, b: usize| cs[cw * words + b / 64] >> (b % 64) & 1 == 1;
        for &b in &set_bits {
            let mut assigned: Vec<Option<Option<(u32, u8)>>> = vec![None; ms.len()];
            let mut any_unassigned = false;
            for (i, &v) in ms.iter().enumerate() {
                let vu = v as usize;
                let own = if b == 0 { incomplete[vu] } else { own_bit(vu) == Some(b) };
                if own {
                    assigned[i] = Some(None);
                    continue;
                }
                let mut found = None;
                for (w, k) in g.succ(vu) {
                    let cw = comp[w as usize] as usize;
                    if cw != c && has(&comp_set, cw, b) {
                        found = Some((w, k));
                        break;
                    }
                }
                match found {
                    Some(f) => assigned[i] = Some(Some(f)),
                    None => any_unassigned = true,
                }
            }
            if any_unassigned {
                // BFS backwards inside the component from assigned members.
                let pos: std::collections::HashMap<u32, usize> =
                    ms.iter().enumerate().map(|(i, &v)| (v, i)).collect();
                let mut rev: Vec<Vec<(usize, u8)>> = vec![Vec::new(); ms.len()];
                for (i, &v) in ms.iter().enumerate() {
                    for (w, k) in g.succ(v as usize) {
                        if let Some(&j) = pos.get(&w) {
                            rev[j].push((i, k));
                        }
                    }
                }
                let mut queue: std::collections::VecDeque<usize> =
                    (0..ms.len()).filter(|&i| assigned[i].is_some()).collect();
                while let Some(x) = queue.pop_front() {
                    for &(u, k) in &rev[x] {
                        if assigned[u].is_none() {
                            assigned[u] = Some(Some((ms[x], k)));
                            queue.push_back(u);
                        }
                    }
                }
            }
            for (i, &v) in ms.iter().enumerate() {
                if let Some(via) = assigned[i] {
                    witnesses.push(Witness { node: v, axiom: b as u32, via });
                }
            }
        }
    }
    Analysis { axioms, witnesses }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(an: &Analysis, start: u32, axiom: u32) -> Vec<u32> {
        let mut p = vec![start];
        let mut cur = start;
        for _ in 0..100 {
            let w = an.witnesses.iter().find(|w| w.node == cur && w.axiom == axiom).expect("witness");
            match w.via {
                None => return p,
                Some((nx, _)) => {
                    p.push(nx);
                    cur = nx;
                }
            }
        }
        panic!("path too long");
    }

    #[test]
    fn chains_and_cycles() {
        // 0 -> 1 -> 2(axiom A); 3 <-> 4 (cycle), 4 -> 5(axiom B); 6 incomplete; 0 -> 6
        let edges = vec![(0, 2, 1), (1, 1, 2), (3, 1, 4), (4, 3, 3), (4, 2, 5), (0, 2, 6), (7, 2, 3)];
        let g = Graph::from_edges(8, edges);
        let is_axiom = [false, false, true, false, false, true, false, false];
        let inc = [false, false, false, false, false, false, true, false];
        let an = analyze(&g, &is_axiom, &inc);
        assert_eq!(an.axioms, vec![2, 5]);
        assert_eq!(path(&an, 0, 1), vec![0, 1, 2]);
        assert_eq!(path(&an, 0, 0), vec![0, 6]);
        assert_eq!(path(&an, 3, 2), vec![3, 4, 5]);
        assert_eq!(path(&an, 7, 2), vec![7, 3, 4, 5]);
        // Node 3 does not reach axiom A.
        assert!(!an.witnesses.iter().any(|w| w.node == 3 && w.axiom == 1));
        // Axiom nodes have themselves as witness.
        assert!(an.witnesses.iter().any(|w| w.node == 2 && w.axiom == 1 && w.via.is_none()));
    }

    #[test]
    fn scc_order_is_reverse_topological() {
        let g = Graph::from_edges(4, vec![(0, 1, 1), (1, 1, 2), (2, 1, 1), (2, 1, 3)]);
        let (comp, n) = scc(&g);
        assert_eq!(n, 3);
        assert_eq!(comp[1], comp[2]);
        assert!(comp[3] < comp[1] && comp[1] < comp[0]);
    }

    #[test]
    fn long_chain_no_stack_overflow() {
        let n = 200_000;
        let edges: Vec<(u32, u8, u32)> = (0..n as u32 - 1).map(|i| (i, 2, i + 1)).collect();
        let g = Graph::from_edges(n, edges);
        let mut is_axiom = vec![false; n];
        is_axiom[n - 1] = true;
        let an = analyze(&g, &is_axiom, &vec![false; n]);
        assert_eq!(an.witnesses.len(), n);
    }
}
