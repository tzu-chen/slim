//! Validation of slim's axiom analysis against Lean's own report.
//!
//! The exporter can record, for local declarations, the result of Lean's
//! `Lean.collectAxioms` (the function behind `#print axioms`). This module
//! compares those lists with the sets slim derives from the exported
//! edges, ignoring slim's coverage-gap pseudo-axiom.
//!
//! Lean changed `collectAxioms` in 4.28: since then it also follows the
//! *types* of axioms (so `Lean.ofReduceBool`, whose type mentions an opaque
//! constant defined using `Lean.trustCompiler`, also reports the latter).
//! slim always follows axiom types (the more complete reading). For
//! snapshots from older toolchains the check therefore recomputes the
//! expected sets under the older rule and reports the difference as a
//! documented semantic change rather than as a mismatch.

use crate::axioms::{analyze, Graph};
use crate::model::{Coverage, DeclKind, EdgeKind, Origin};
use crate::source::lexer::parse_lean_version;
use anyhow::Result;
use rusqlite::Connection;
use std::collections::{BTreeSet, HashMap};

pub struct CheckReport {
    pub checked: usize,
    pub lean_version: String,
    /// True when Lean's report follows the pre-4.28 rule (axiom types not traversed).
    pub pre_428_semantics: bool,
    /// (declaration, slim set under Lean's rule, Lean set)
    pub mismatches: Vec<(String, Vec<String>, Vec<String>)>,
    /// Declarations whose slim set (axiom types traversed) differs from
    /// Lean's pre-4.28 report only because of that rule change.
    pub explained_by_rule_change: Vec<(String, Vec<String>)>,
}

/// Axiom sets for every declaration under the chosen traversal rule.
fn sets(conn: &Connection, follow_axiom_types: bool) -> Result<(Vec<BTreeSet<String>>, HashMap<i64, usize>)> {
    let mut ids: Vec<i64> = Vec::new();
    let mut kinds = Vec::new();
    let mut names = Vec::new();
    let mut incomplete = Vec::new();
    let mut st = conn.prepare("SELECT id, name, kind, origin, type_cov, value_cov FROM decls ORDER BY id")?;
    let rows = st.query_map([], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, u8>(2)?, r.get::<_, u8>(3)?, r.get::<_, u8>(4)?, r.get::<_, u8>(5)?))
    })?;
    for row in rows {
        let (id, name, k, o, tc, vc) = row?;
        let k = DeclKind::from_u8(k);
        ids.push(id);
        kinds.push((k, Origin::from_u8(o)));
        names.push(name);
        let tc = Coverage::from_u8(tc);
        let vc = Coverage::from_u8(vc);
        incomplete.push(tc.is_incomplete() || (k.has_value() && vc.is_incomplete()) || Origin::from_u8(o) == Origin::Unresolved);
    }
    let index: HashMap<i64, usize> = ids.iter().enumerate().map(|(i, &id)| (id, i)).collect();
    let mut edges = Vec::new();
    let mut st = conn.prepare("SELECT src, kind, dst FROM edges")?;
    for row in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, u8>(1)?, r.get::<_, i64>(2)?)))? {
        let (s, k, d) = row?;
        let (si, di) = (index[&s], index[&d]);
        if !follow_axiom_types && kinds[si].0 == DeclKind::Axiom && k == EdgeKind::Type as u8 {
            continue;
        }
        edges.push((si as u32, k, di as u32));
    }
    let g = Graph::from_edges(ids.len(), edges);
    let is_axiom: Vec<bool> = kinds.iter().map(|(k, o)| *k == DeclKind::Axiom && *o != Origin::Unresolved).collect();
    let an = analyze(&g, &is_axiom, &incomplete);
    let mut out = vec![BTreeSet::new(); ids.len()];
    for w in &an.witnesses {
        if w.axiom > 0 {
            let ax = an.axioms[w.axiom as usize - 1] as usize;
            out[w.node as usize].insert(names[ax].clone());
        }
    }
    Ok((out, index))
}

pub fn compare_with_lean(conn: &Connection) -> Result<CheckReport> {
    let lean_version: String = conn
        .query_row("SELECT value FROM meta WHERE key = 'lean_version'", [], |r| r.get(0))
        .unwrap_or_default();
    let pre = matches!(parse_lean_version(&lean_version), Some((4, m, _)) if m < 28);
    let has_report: bool = conn
        .query_row("SELECT value = '1' FROM meta WHERE key = 'lean_axioms_present'", [], |r| r.get(0))
        .unwrap_or(false);
    let ids: Vec<i64> = if has_report {
        let mut st = conn.prepare("SELECT id FROM decls WHERE origin = 0 ORDER BY id")?;
        let v = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        v
    } else {
        let mut st = conn.prepare("SELECT DISTINCT decl_id FROM lean_axioms ORDER BY decl_id")?;
        let v = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        v
    };
    let (full, index) = sets(conn, true)?;
    let lean_rule = if pre { Some(sets(conn, false)?.0) } else { None };
    let mut lean_st = conn.prepare("SELECT axiom_name FROM lean_axioms WHERE decl_id = ?1")?;
    let mut name_st = conn.prepare("SELECT name FROM decls WHERE id = ?1")?;
    let mut rep = CheckReport {
        checked: 0,
        lean_version,
        pre_428_semantics: pre,
        mismatches: Vec::new(),
        explained_by_rule_change: Vec::new(),
    };
    for id in ids {
        let lean: BTreeSet<String> = lean_st.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        let i = index[&id];
        let ours_full = &full[i];
        let expected = lean_rule.as_ref().map(|s| &s[i]).unwrap_or(ours_full);
        rep.checked += 1;
        let name: String = name_st.query_row([id], |r| r.get(0))?;
        if *expected != lean {
            rep.mismatches.push((name, expected.iter().cloned().collect(), lean.into_iter().collect()));
        } else if ours_full != expected {
            rep.explained_by_rule_change.push((name, ours_full.difference(expected).cloned().collect()));
        }
    }
    // The stored analysis (used by the UI) must agree with the in-memory one.
    let mut st = conn.prepare(
        "SELECT da.decl_id, a.name FROM decl_axioms da JOIN axioms a ON a.id = da.axiom_id WHERE a.id != 0",
    )?;
    let mut stored: Vec<BTreeSet<String>> = vec![BTreeSet::new(); full.len()];
    for row in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
        let (id, n) = row?;
        stored[index[&id]].insert(n);
    }
    if stored != full {
        anyhow::bail!("internal inconsistency: stored axiom analysis differs from a fresh computation");
    }
    Ok(rep)
}
