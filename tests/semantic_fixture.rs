//! Tests against the bundled demo snapshot, which was produced from real
//! Lean 4.33.1 compiler data by `slim export` (see fixtures/demo/README.md),
//! and against Lean's own `#print axioms` output for the same fixture.

use slim::db::{self, query};
use slim::model::{flags, AxiomCategory, Coverage, DeclKind, EdgeKind, KindFilter, Origin};
use slim::nameindex::{MatchTier, SearchParams};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn demo_index() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let idx = d.path().join("demo.sqlite");
    let snap = root().join("fixtures/demo/snapshot/slim_demo.lean-v4.33.1.jsonl");
    let opts = db::import::ImportOptions { source_root: Some(root().join("fixtures/demo")), ..Default::default() };
    db::import::import_snapshot(&snap, &idx, &opts).unwrap();
    (d, idx)
}

fn id(conn: &rusqlite::Connection, name: &str) -> i64 {
    query::decl_by_name(conn, name).unwrap().unwrap_or_else(|| panic!("{name} not in index"))
}

fn uses(conn: &rusqlite::Connection, name: &str, kind: EdgeKind) -> BTreeSet<String> {
    let f = query::NeighborFilter { local_only: false, show_generated: true, kinds: KindFilter::All };
    let groups = query::neighbors(conn, id(conn, name), query::Direction::Uses, &f, 10_000, 0).unwrap();
    groups.into_iter().filter(|g| g.kind == kind).flat_map(|g| g.rows.into_iter().map(|r| r.name)).collect()
}

#[test]
fn provenance_is_real_compiler_data() {
    let (_d, idx) = demo_index();
    let conn = db::open_read(&idx).unwrap();
    assert_eq!(db::get_meta(&conn, "lean_version").unwrap().as_deref(), Some("4.33.1"));
    assert_eq!(db::get_meta(&conn, "synthetic").unwrap().as_deref(), Some("0"));
    let exporter = db::get_meta(&conn, "exporter").unwrap().unwrap();
    assert!(exporter.contains("slim-export"));
    let iso = db::get_meta(&conn, "isolation").unwrap().unwrap();
    assert!(iso.contains("bubblewrap"));
}

#[test]
fn edges_have_precise_meanings() {
    let (_d, idx) = demo_index();
    let conn = db::open_read(&idx).unwrap();
    // proof term reference (no `sorry` token in the source, yet uses the sorry'd lemma)
    assert!(uses(&conn, "Demo.Fixtures.searchWithin_correct", EdgeKind::Value).contains("Demo.Fixtures.searchWithin_complete"));
    assert!(!uses(&conn, "Demo.Fixtures.searchWithin_correct", EdgeKind::Type).contains("Demo.Fixtures.searchWithin_complete"));
    // statement reference
    assert!(uses(&conn, "Demo.Graph.searchWithin_sound", EdgeKind::Type).contains("Demo.Graph.Reach"));
    // inductive → constructors
    let ctors = uses(&conn, "Demo.Rel.ReflTransGen", EdgeKind::Ctor);
    assert_eq!(ctors, ["Demo.Rel.ReflTransGen.refl", "Demo.Rel.ReflTransGen.tail"].iter().map(|s| s.to_string()).collect());
    // the custom axiom is used in the proof of the fixture theorem
    assert!(uses(&conn, "Demo.Fixtures.reach_iff_bounded_search", EdgeKind::Value).contains("Demo.Fixtures.oracle_axiom"));
    // reverse edges agree with forward edges
    let f = query::NeighborFilter { local_only: true, show_generated: true, kinds: KindFilter::All };
    let g = query::neighbors(&conn, id(&conn, "Demo.Fixtures.searchWithin_complete"), query::Direction::UsedBy, &f, 100, 0).unwrap();
    let users: BTreeSet<String> = g.iter().filter(|g| g.kind == EdgeKind::Value).flat_map(|g| g.rows.iter().map(|r| r.name.clone())).collect();
    assert!(users.contains("Demo.Fixtures.searchWithin_correct"));
    // coverage: every local declaration with a value has complete value coverage
    let bad: i64 = conn
        .query_row(
            &format!(
                "SELECT count(*) FROM decls WHERE origin = 0 AND kind IN ({},{},{},{},{}) AND value_cov != {}",
                DeclKind::Theorem as u8,
                DeclKind::Definition as u8,
                DeclKind::Abbrev as u8,
                DeclKind::Instance as u8,
                DeclKind::Opaque as u8,
                Coverage::Complete as u8
            ),
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(bad, 0);
}

fn axiom_set(conn: &rusqlite::Connection, name: &str) -> BTreeMap<String, AxiomCategory> {
    query::detail(conn, id(conn, name)).unwrap().unwrap().axioms.into_iter().map(|a| (a.name, a.category)).collect()
}

#[test]
fn axiom_evidence_paths_by_category() {
    let (_d, idx) = demo_index();
    let conn = db::open_read(&idx).unwrap();
    // indirect placeholder: path search_trans → searchWithin_correct → searchWithin_complete → sorryAx
    let ev = query::axiom_evidence(&conn, id(&conn, "Demo.Fixtures.search_trans")).unwrap();
    let sorry = ev.iter().find(|e| e.axiom.name == "sorryAx").expect("sorryAx reported");
    let path: Vec<&str> = sorry.path.iter().map(|s| s.row.name.as_str()).collect();
    assert_eq!(
        path,
        vec!["Demo.Fixtures.search_trans", "Demo.Fixtures.searchWithin_correct", "Demo.Fixtures.searchWithin_complete", "sorryAx"]
    );
    assert!(sorry.path.iter().take(3).all(|s| s.next_kind == Some(EdgeKind::Value)));
    // hidden sorry through a tactic macro
    assert_eq!(axiom_set(&conn, "Demo.Fixtures.searchWithin_succ").get("sorryAx"), Some(&AxiomCategory::Sorry));
    // custom axiom
    assert_eq!(axiom_set(&conn, "Demo.Fixtures.reach_iff_bounded_search").get("Demo.Fixtures.oracle_axiom"), Some(&AxiomCategory::Custom));
    // compiler trust (per-use native_decide axiom in Lean 4.33)
    assert!(axiom_set(&conn, "Demo.Fixtures.sample_search_native").values().all(|c| *c == AxiomCategory::CompilerTrust));
    assert!(!axiom_set(&conn, "Demo.Fixtures.sample_search_native").is_empty());
    // standard axioms by their names
    let ext = axiom_set(&conn, "Demo.Rel.ext");
    assert_eq!(ext.keys().cloned().collect::<Vec<_>>(), vec!["Quot.sound".to_string(), "propext".to_string()]);
    assert!(ext.values().all(|c| *c == AxiomCategory::Standard));
    // fully proved soundness theorem: nothing
    assert!(axiom_set(&conn, "Demo.Graph.searchWithin_sound").is_empty());
    // the kernel-checked counterpart of the native_decide fixture: nothing
    assert!(axiom_set(&conn, "Demo.Fixtures.sample_reach").is_empty());
}

/// Parses Lean's `#print axioms` output (fixtures/demo/axiom-report/*.txt).
fn parse_report(path: &Path) -> BTreeMap<String, BTreeSet<String>> {
    let mut m = BTreeMap::new();
    for line in std::fs::read_to_string(path).unwrap().lines() {
        let Some(rest) = line.strip_prefix('\'') else { continue };
        let Some((name, tail)) = rest.split_once('\'') else { continue };
        let set: BTreeSet<String> = if let Some(list) = tail.trim().strip_prefix("depends on axioms: [") {
            list.trim_end_matches(']').split(", ").map(String::from).collect()
        } else {
            BTreeSet::new()
        };
        m.insert(name.to_string(), set);
    }
    m
}

#[test]
fn matches_lean_print_axioms_for_every_reported_declaration() {
    let (_d, idx) = demo_index();
    let conn = db::open_read(&idx).unwrap();
    let report = parse_report(&root().join("fixtures/demo/axiom-report/v4.33.1.txt"));
    assert!(report.len() >= 70, "report has {} entries", report.len());
    let mut checked = 0;
    for (name, expected) in &report {
        let ours: BTreeSet<String> =
            axiom_set(&conn, name).into_iter().filter(|(_, c)| *c != AxiomCategory::Incomplete).map(|(n, _)| n).collect();
        assert_eq!(&ours, expected, "axioms of {name}");
        checked += 1;
    }
    assert_eq!(checked, report.len());
    // and the exporter's own collectAxioms column agrees everywhere
    let r = slim::axioms_check::compare_with_lean(&conn).unwrap();
    assert!(r.checked >= 150);
    assert!(r.mismatches.is_empty(), "{:?}", r.mismatches);
}

#[test]
fn unicode_private_and_generated_declarations() {
    let (_d, idx) = demo_index();
    let conn = db::open_read(&idx).unwrap();
    for n in ["Demo.Fixtures.ℛ", "Demo.Fixtures.step₁", "Demo.Fixtures.reach_αβ", "Demo.Fixtures.«reach is transitive»"] {
        let d = query::detail(&conn, id(&conn, n)).unwrap().unwrap();
        assert_eq!(d.row.origin, Origin::Local, "{n}");
        assert!(d.row.flags & flags::GENERATED == 0, "{n} is user-written");
    }
    let private = "_private.SlimDemo.Fixtures.Special.0.Demo.Fixtures.chain_reach_aux";
    let d = query::detail(&conn, id(&conn, private)).unwrap().unwrap();
    assert_eq!(d.row.display, "Demo.Fixtures.chain_reach_aux");
    assert!(d.row.flags & flags::PRIVATE != 0);
    let rec = query::detail(&conn, id(&conn, "Demo.Rel.ReflTransGen.rec")).unwrap().unwrap();
    assert_eq!(rec.row.kind, DeclKind::Recursor);
    assert!(rec.row.flags & flags::GENERATED != 0);
    let inst = query::detail(&conn, id(&conn, "Demo.Fixtures.instInhabitedGraph")).unwrap().unwrap();
    assert_eq!(inst.row.kind, DeclKind::Instance);
    // Positions are code-point columns: the selection of reach_αβ points at its name.
    let d = query::detail(&conn, id(&conn, "Demo.Fixtures.reach_αβ")).unwrap().unwrap();
    let src = std::fs::read_to_string(root().join("fixtures/demo").join(d.module_path.unwrap())).unwrap();
    let line = src.lines().nth(d.pos.sel_line.unwrap() as usize - 1).unwrap();
    let b = slim::text::col_to_byte(line, d.pos.sel_col.unwrap() as usize, slim::text::PosEncoding::CodePoint);
    assert!(line[b..].starts_with("reach_αβ"), "{line:?} @ {b}");
}

#[test]
fn search_ranks_exact_qualified_matches_first_and_finds_docs() {
    let (_d, idx) = demo_index();
    let conn = db::open_read(&idx).unwrap();
    let gen = db::get_meta(&conn, "name_index_generation").unwrap().unwrap();
    let ni = slim::nameindex::NameIndex::load(&db::import::names_path(&idx), &gen).unwrap();
    let top = |q: &str| {
        let o = ni.search(&SearchParams { query: q.into(), ..Default::default() }, &|| false);
        let rows = query::rows_by_ids(&conn, &o.hits.iter().map(|h| h.id as i64).collect::<Vec<_>>()).unwrap();
        (o, rows)
    };
    let (o, rows) = top("Demo.Graph.Reach");
    assert_eq!(rows[0].name, "Demo.Graph.Reach");
    assert_eq!(o.hits[0].tier, MatchTier::Exact);
    let (o, rows) = top("searchWithin_sound");
    assert_eq!(rows[0].name, "Demo.Graph.searchWithin_sound");
    assert_eq!(o.hits[0].tier, MatchTier::ExactLast);
    let (_o, rows) = top("reach trans");
    assert!(rows.iter().any(|r| r.name == "Demo.Graph.Reach.trans"));
    // documentation words
    let ids = query::text_search(&conn, "decision procedure", 20).unwrap();
    let names: Vec<String> = query::rows_by_ids(&conn, &ids).unwrap().into_iter().map(|r| r.name).collect();
    assert!(names.contains(&"Demo.Fixtures.searchWithin_correct".to_string()), "{names:?}");
}

#[test]
fn source_scanner_agrees_with_lean_on_the_fixture() {
    let (_d, idx) = demo_index();
    let conn = db::open_read(&idx).unwrap();
    let mut st = conn
        .prepare(&format!("SELECT display FROM decls WHERE origin = 0 AND (flags & {}) = 0 AND kind NOT IN ({}, {})", flags::GENERATED, DeclKind::Recursor as u8, DeclKind::Constructor as u8))
        .unwrap();
    let semantic: BTreeSet<String> = st.query_map([], |r| r.get(0)).unwrap().map(|x| x.unwrap()).collect();
    let mut lexical: BTreeSet<String> = BTreeSet::new();
    let profile = slim::source::lexer::LexProfile::for_toolchain("leanprover/lean4:v4.33.1");
    let mut fake = Vec::new();
    for entry in walk(&root().join("fixtures/demo/SlimDemo")) {
        let text = std::fs::read_to_string(&entry).unwrap();
        let fs = slim::source::scan::scan_source(&text, profile);
        assert!(fs.lex_ok, "{entry:?}: {:?}", fs.issues);
        for d in fs.decls {
            if d.name.contains("fake_") {
                fake.push(d.name.clone());
            }
            lexical.insert(d.name);
        }
    }
    assert!(fake.is_empty(), "declarations inside comments were reported: {fake:?}");
    let found = semantic.iter().filter(|n| lexical.contains(*n)).count();
    let recall = found as f64 / semantic.len() as f64;
    let missing: Vec<&String> = semantic.iter().filter(|n| !lexical.contains(*n)).collect();
    assert!(recall >= 0.85, "lexical recall {recall:.2}; missing {missing:?}");
}

fn walk(d: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for e in std::fs::read_dir(d).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            v.extend(walk(&p));
        } else if p.extension().is_some_and(|x| x == "lean") {
            v.push(p);
        }
    }
    v.sort();
    v
}

#[test]
fn source_excerpts_are_contained_in_the_approved_root() {
    let (_d, idx) = demo_index();
    let conn = db::open_read(&idx).unwrap();
    let mut st = conn.prepare("SELECT path FROM modules WHERE is_local = 1 AND path IS NOT NULL").unwrap();
    let paths: Vec<String> = st.query_map([], |r| r.get(0)).unwrap().map(|x| x.unwrap()).collect();
    assert_eq!(paths.len(), 8);
    for p in paths {
        let r = slim::paths::resolve_contained(&root().join("fixtures/demo"), &p).unwrap();
        assert!(r.starts_with(root().join("fixtures/demo").canonicalize().unwrap()));
    }
}
