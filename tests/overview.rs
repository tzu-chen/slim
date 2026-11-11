//! The overview features against real data: the demo library scanned
//! source-only and its bundled Lean 4.33.1 snapshot. Per-file counts, the
//! import graph, skimming, scoped search and the positions of mentions are
//! each checked against something computed independently here.

use slim::db::{self, query, IndexMode};
use slim::imports::{ImportGraph, PathAnswer};
use slim::map::{self, MapLoad};
use slim::model::flags;
use slim::nameindex::{ModuleSet, Scope, ScopeKind, ScopeSpec, SearchParams};
use slim::paths::Project;
use slim::skim::{self, SkimItem, SkimScope};
use slim::source::index::{self as sindex, ScanConfig};
use slim::sourceview::SourceCache;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

fn demo_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/demo")
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let name = e.file_name();
        if name.to_string_lossy().starts_with('.') || name == "snapshot" || name == "axiom-report" {
            continue;
        }
        let p = e.path();
        if p.is_dir() {
            copy_tree(&p, &to.join(&name));
        } else {
            std::fs::copy(&p, to.join(&name)).unwrap();
        }
    }
}

/// A private copy of the demo sources, scanned source-only.
fn source_project() -> (tempfile::TempDir, Project, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("proj");
    copy_tree(&demo_dir(), &root);
    let project = Project::open(&root).unwrap();
    let idx = d.path().join("source.sqlite");
    let cfg = ScanConfig { threads: 2, ..ScanConfig::for_project(&project) };
    sindex::build_source_index(&project, &idx, &cfg, &AtomicBool::new(false), None).unwrap();
    (d, project, idx)
}

fn semantic_index() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let idx = d.path().join("demo.sqlite");
    let snap = demo_dir().join("snapshot/slim_demo.lean-v4.33.1.jsonl");
    let opts = db::import::ImportOptions { source_root: Some(demo_dir()), ..Default::default() };
    db::import::import_snapshot(&snap, &idx, &opts).unwrap();
    (d, idx)
}

fn stats_of(conn: &rusqlite::Connection, module: &str) -> query::ModStats {
    let id: i64 = conn.query_row("SELECT id FROM modules WHERE name = ?1", [module], |r| r.get(0)).unwrap();
    query::module_stats(conn, id).unwrap().unwrap()
}

#[test]
fn source_counts_equal_what_the_declarations_say() {
    let (_d, project, idx) = source_project();
    let conn = db::open_read(&idx).unwrap();
    let mods = query::modules_with_stats(&conn, true).unwrap();
    assert_eq!(mods.len(), 9);
    for (m, st) in &mods {
        // Recount from the declaration rows.
        let kind = |list: &str| -> u32 { conn.query_row(&format!("SELECT count(*) FROM decls WHERE module_id = ?1 AND kind IN ({list})"), [m.id], |r| r.get(0)).unwrap() };
        assert_eq!((st.thm, st.def, st.typ, st.inst, st.axiom), (kind("0"), kind("1, 2, 10"), kind("4, 5, 6"), kind("3"), kind("9")), "{}", m.name);
        let doc: u32 = conn
            .query_row(&format!("SELECT count(*) FROM decls WHERE module_id = ?1 AND kind IN (0, 1, 2, 3, 4, 5, 6, 9, 10) AND doc IS NOT NULL AND (flags & {}) != 0", flags::HAS_DOC), [m.id], |r| r.get(0))
            .unwrap();
        assert_eq!(st.doc, doc, "{}", m.name);
        let sorry: u32 = conn.query_row(&format!("SELECT count(*) FROM decls WHERE module_id = ?1 AND (flags & {}) != 0", flags::LEX_SORRY), [m.id], |r| r.get(0)).unwrap();
        assert_eq!(st.sorry, sorry, "{}", m.name);
        // Proof lines, recounted from the file: the lines from the end of
        // each theorem's header to the end of the declaration.
        let text = std::fs::read_to_string(project.root.join(m.path.as_ref().unwrap())).unwrap();
        assert_eq!(st.lines as usize, text.lines().count(), "{}", m.name);
        let mut st2 = conn.prepare("SELECT header_end_byte, end_byte FROM decls WHERE module_id = ?1 AND kind = 0").unwrap();
        let spans: Vec<(usize, usize)> = st2.query_map([m.id], |r| Ok((r.get::<_, i64>(0)? as usize, r.get::<_, i64>(1)? as usize))).unwrap().map(|x| x.unwrap()).collect();
        let proof: usize = spans.iter().filter(|(a, z)| z > a).map(|&(a, z)| text[a..z].trim_end_matches('\n').matches('\n').count() + 1).sum();
        assert_eq!(st.proof_lines as usize, proof, "{}", m.name);
        assert_eq!((st.files, st.partial), (1, 0));
        assert!(st.mtime_ns > 0);
    }
    // What a reader can check by opening the file.
    let s = stats_of(&conn, "SlimDemo.Graph.Search");
    assert_eq!((s.thm, s.def, s.typ, s.doc), (4, 2, 0, 6));
    let t = stats_of(&conn, "SlimDemo.Fixtures.Trust");
    assert_eq!(t.sorry, 1, "one declaration of the fixture has a `sorry` token in its text");
    assert_eq!(stats_of(&conn, "SlimDemo").named(), 0, "the umbrella file declares nothing");
    // The outline sums files into directories.
    let tree = slim::app::worker::build_outline_tree(mods.clone());
    let root = &tree.nodes[tree.by_name["SlimDemo"]];
    let below: Vec<&query::ModStats> = mods.iter().filter(|(m, _)| m.name.starts_with("SlimDemo")).map(|(_, s)| s).collect();
    assert_eq!(root.roll.files as usize, below.len());
    assert_eq!(root.roll.thm, below.iter().map(|s| s.thm).sum::<u32>());
    assert_eq!(root.roll.proof_lines, below.iter().map(|s| s.proof_lines).sum::<u32>());
    assert_eq!(root.roll.mtime_ns, below.iter().map(|s| s.mtime_ns).max().unwrap());
    let fixtures = &tree.nodes[tree.by_name["SlimDemo.Fixtures"]];
    assert_eq!((fixtures.roll.files, fixtures.roll.sorry), (2, 1));
}

#[test]
fn counts_follow_an_incremental_rescan() {
    let (_d, project, idx) = source_project();
    let before = stats_of(&db::open_read(&idx).unwrap(), "SlimDemo.Graph.Basic");
    let other = stats_of(&db::open_read(&idx).unwrap(), "SlimDemo.Graph.Search");
    let p = project.root.join("SlimDemo/Graph/Basic.lean");
    let mut text = std::fs::read_to_string(&p).unwrap();
    text.push_str("\n/-- Added. -/\ntheorem Demo.Graph.added_later : True := by\n  sorry\n\ndef Demo.Graph.addedDef : Nat := 0\n");
    std::fs::write(&p, text).unwrap();
    let cancel = AtomicBool::new(false);
    let conn = db::open_read(&idx).unwrap();
    let fr = sindex::check_freshness(&conn, &project.root, true, false, &cancel).unwrap();
    drop(conn);
    assert_eq!(fr.changed, vec!["SlimDemo/Graph/Basic.lean"]);
    sindex::apply_incremental(&idx, &project, &fr, &ScanConfig::for_project(&project), &cancel).unwrap();
    let conn = db::open_read(&idx).unwrap();
    let after = stats_of(&conn, "SlimDemo.Graph.Basic");
    assert_eq!((after.thm, after.def, after.sorry, after.doc), (before.thm + 1, before.def + 1, before.sorry + 1, before.doc + 1));
    assert_eq!(after.proof_lines, before.proof_lines + 2);
    assert_eq!(after.lines, before.lines + 6);
    assert_eq!(stats_of(&conn, "SlimDemo.Graph.Search"), other, "other files keep their counts");
    // A deleted file leaves no counts behind.
    std::fs::remove_file(&p).unwrap();
    let fr = sindex::check_freshness(&conn, &project.root, true, false, &cancel).unwrap();
    drop(conn);
    sindex::apply_incremental(&idx, &project, &fr, &ScanConfig::for_project(&project), &cancel).unwrap();
    let conn = db::open_read(&idx).unwrap();
    assert!(query::modules_with_stats(&conn, true).unwrap().iter().all(|(m, _)| m.name != "SlimDemo.Graph.Basic"));
}

#[test]
fn semantic_counts_come_from_the_snapshot() {
    let (_d, idx) = semantic_index();
    let conn = db::open_read(&idx).unwrap();
    let mods = query::modules_with_stats(&conn, true).unwrap();
    assert_eq!(mods.len(), 8);
    let g = flags::GENERATED;
    for (m, st) in &mods {
        let thm: u32 = conn.query_row(&format!("SELECT count(*) FROM decls WHERE module_id = ?1 AND kind = 0 AND (flags & {g}) = 0"), [m.id], |r| r.get(0)).unwrap();
        assert_eq!(st.thm, thm, "{}", m.name);
        // Dependents of sorryAx, recounted from the axiom analysis.
        let sorry: u32 = conn
            .query_row(
                &format!(
                    "SELECT count(*) FROM decls d WHERE d.module_id = ?1 AND (d.flags & {g}) = 0 AND EXISTS
                       (SELECT 1 FROM decl_axioms da JOIN axioms a ON a.id = da.axiom_id WHERE da.decl_id = d.id AND a.name = 'sorryAx')"
                ),
                [m.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(st.sorry, sorry, "{}", m.name);
        assert_eq!(st.proof_lines, 0, "proof lines are recorded by the source scan only");
    }
    // The fixture's sorry is reached by the theorem that uses the sorry'd lemma too.
    assert!(stats_of(&conn, "SlimDemo.Fixtures.Trust").sorry >= 2);
    assert_eq!(stats_of(&conn, "SlimDemo.Relation.Basic").sorry, 0);
    // The same user-written theorems as the lexical scan finds in the file.
    let (_d2, _project, sidx) = source_project();
    let sconn = db::open_read(&sidx).unwrap();
    for name in ["SlimDemo.Graph.Search", "SlimDemo.Relation.Closure", "SlimDemo.Relation.Basic"] {
        assert_eq!(stats_of(&conn, name).thm, stats_of(&sconn, name).thm, "{name}");
    }
}

#[test]
fn import_graph_is_the_same_from_headers_and_from_the_snapshot() {
    let (_d, _project, sidx) = source_project();
    let (_d2, idx) = semantic_index();
    let src = ImportGraph::load(&db::open_read(&sidx).unwrap()).unwrap();
    let sem = ImportGraph::load(&db::open_read(&idx).unwrap()).unwrap();
    // The snapshot has no AxiomReport.lean (it is not part of the library).
    assert_eq!((src.local_count, sem.local_count), (9, 8));
    for v in 0..sem.nodes.len() as u32 {
        let n = &sem.nodes[v as usize];
        if !n.local {
            continue;
        }
        let w = src.node(&n.name).unwrap_or_else(|| panic!("{} not scanned", n.name));
        assert_eq!(src.depth[w as usize], sem.depth[v as usize], "depth of {}", n.name);
        let names = |g: &ImportGraph, ids: &[u32]| -> BTreeSet<String> { ids.iter().filter(|&&i| g.nodes[i as usize].local).map(|&i| g.nodes[i as usize].name.clone()).collect() };
        assert_eq!(names(&src, src.imports_of(w)), names(&sem, sem.imports_of(v)), "local imports of {}", n.name);
    }
    let endpoints = |g: &ImportGraph| -> BTreeSet<String> { g.endpoints.iter().map(|&i| g.nodes[i as usize].name.clone()).collect() };
    assert_eq!(endpoints(&src), endpoints(&sem));
    assert_eq!(endpoints(&sem), ["SlimDemo.Fixtures.Special", "SlimDemo.Fixtures.Trust", "SlimDemo.Relation.Equivalence"].iter().map(|s| s.to_string()).collect());
    // The directory order is the one a reader would expect of this library.
    let root = sem.dir("SlimDemo").unwrap();
    let order: Vec<&str> = sem.dirs[root as usize].children.iter().map(|&c| sem.dirs[c as usize].label.as_str()).collect();
    assert_eq!(order, ["Relation", "Graph", "Fixtures"]);
    // Every step of an import path is an import line of the file before it.
    match sem.explain("SlimDemo.Fixtures.Trust", "Relation.Basic") {
        PathAnswer::Path(p) => {
            assert_eq!((p.first().unwrap().as_str(), p.last().unwrap().as_str()), ("SlimDemo.Fixtures.Trust", "SlimDemo.Relation.Basic"));
            for w in p.windows(2) {
                let rel = format!("{}.lean", w[0].replace('.', "/"));
                let text = std::fs::read_to_string(demo_dir().join(rel)).unwrap();
                assert!(text.lines().any(|l| l.trim() == format!("import {}", w[1])), "{} does not import {}", w[0], w[1]);
            }
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(sem.explain("SlimDemo.Relation.Basic", "SlimDemo.Graph.Search"), PathAnswer::NoPath { .. }));
    // A semantic snapshot records the library's own imports: a path can leave the project.
    assert!(matches!(sem.explain("SlimDemo.Relation.Basic", "Init"), PathAnswer::Path(_)));
    // The summary of a directory counts the same import lines.
    let s = sem.dir_summary(sem.dir("SlimDemo.Graph").unwrap(), 10);
    assert_eq!(s.imports_from, vec![("SlimDemo.Relation".to_string(), 1)]);
    assert!(s.imported_by.iter().any(|(n, k)| n == "SlimDemo.Fixtures" && *k == 2));
}

#[test]
fn skim_shows_statements_from_source_and_from_the_snapshot() {
    let (_d, project, sidx) = source_project();
    let sconn = db::open_read(&sidx).unwrap();
    let mut cache = SourceCache::default();
    let mut env = skim::Env { conn: &sconn, mode: IndexMode::Source, source_root: Some(&project.root), cache: &mut cache, map: None, show_generated: false };
    let s = skim::skim(&mut env, &SkimScope::Module("SlimDemo.Graph.Search".into())).unwrap();
    let file = std::fs::read_to_string(project.root.join("SlimDemo/Graph/Search.lean")).unwrap();
    let mut n = 0;
    for item in &s.items {
        if let SkimItem::Decl(d) = item {
            n += 1;
            // The statement is text of the file, and stops before the body.
            assert!(file.contains(&d.sig), "{:?} is not in the file", d.sig);
            assert!(!d.sig.contains(":="), "{}", d.sig);
            assert!(d.doc.is_some(), "every declaration of this file is documented");
            assert!(!d.sig_cut);
        }
    }
    assert_eq!((n, s.decls, s.files), (6, 6, 1));
    // The same file from the snapshot: elaborated signatures with full names.
    let (_d2, idx) = semantic_index();
    let conn = db::open_read(&idx).unwrap();
    let root = demo_dir();
    let m = match map::load_for_root(&root) {
        MapLoad::Loaded(m) => m,
        other => panic!("{other:?}"),
    };
    let mut env = skim::Env { conn: &conn, mode: IndexMode::Semantic, source_root: Some(&root), cache: &mut cache, map: Some(&m), show_generated: false };
    let s = skim::skim(&mut env, &SkimScope::Module("SlimDemo.Graph.Search".into())).unwrap();
    let sound = s.items.iter().find_map(|i| if let SkimItem::Decl(d) = i { (d.row.name == "Demo.Graph.searchWithin_sound").then_some(d) } else { None }).unwrap();
    assert!(sound.sig.starts_with("theorem Demo.Graph.searchWithin_sound {g : Demo.Graph}"), "{}", sound.sig);
    assert!(sound.sig.ends_with("g.Reach s t"), "the conclusion as Lean prints it: {}", sound.sig);
    assert!(sound.doc.as_deref().unwrap().starts_with("Soundness of the search"));
    assert!(s.items.iter().all(|i| !matches!(i, SkimItem::Decl(d) if d.row.flags & flags::GENERATED != 0)), "generated declarations are left out");
    // A map group: its landmarks in the map's order, subgroups after it.
    let s = skim::skim(&mut env, &SkimScope::Group("search".into())).unwrap();
    let g = &m.groups[m.group_by_id("search").unwrap()];
    let names: Vec<&str> = s.items.iter().filter_map(|i| if let SkimItem::Decl(d) = i { Some(d.row.name.as_str()) } else { None }).collect();
    assert_eq!(names, g.landmarks.iter().map(|p| p.decl.as_str()).collect::<Vec<_>>());
    assert!(matches!(&s.items[0], SkimItem::Header { title, .. } if *title == g.title));
    assert!(matches!(&skim::skim(&mut env, &SkimScope::Group("nope".into())).unwrap().items[0], SkimItem::Note { warn: true, .. }));
    // A namespace spans files.
    let s = skim::skim(&mut env, &SkimScope::Namespace("Demo.Graph".into())).unwrap();
    let files: Vec<&str> = s.items.iter().filter_map(|i| if let SkimItem::Header { title, .. } = i { Some(title.as_str()) } else { None }).collect();
    assert!(files.len() >= 2 && files.contains(&"SlimDemo.Graph.Basic") && files.contains(&"SlimDemo.Graph.Search"), "{files:?}");
    assert!(s.items.iter().all(|i| !matches!(i, SkimItem::Decl(d) if !d.row.name.starts_with("Demo.Graph"))));
}

#[test]
fn scoped_search_only_returns_what_is_in_scope() {
    let (_d, _project, idx) = source_project();
    let conn = db::open_read(&idx).unwrap();
    let ni = db::import::build_name_index(&conn, "g").unwrap();
    let rows = |p: &SearchParams| -> Vec<query::DeclRow> {
        let o = ni.search(p, &|| false);
        query::rows_by_ids(&conn, &o.hits.iter().map(|h| h.id as i64).collect::<Vec<_>>()).unwrap()
    };
    let everywhere = rows(&SearchParams { query: "trans".into(), limit: 500, ..Default::default() });
    assert!(everywhere.iter().any(|r| r.module.as_deref() == Some("SlimDemo.Relation.Closure")) && everywhere.iter().any(|r| r.module.as_deref() == Some("SlimDemo.Graph.Basic")));
    // Files: every hit is in a file below the prefix, and none there is lost.
    let spec = ScopeSpec { kind: ScopeKind::Modules, prefix: "SlimDemo.Graph".into() };
    let ids: Vec<i64> = conn
        .prepare("SELECT id FROM modules WHERE name = ?1 OR (name >= ?1 || '.' AND name < ?1 || '/')")
        .unwrap()
        .query_map([&spec.prefix], |r| r.get(0))
        .unwrap()
        .map(|x| x.unwrap())
        .collect();
    assert_eq!(ids.len(), 2);
    let set = Arc::new(ModuleSet::from_ids(ids.into_iter().map(|i| i as u32)));
    let scoped = rows(&SearchParams { query: "trans".into(), limit: 500, scope: Some(Scope::Modules(set.clone())), ..Default::default() });
    assert!(!scoped.is_empty());
    assert!(scoped.iter().all(|r| spec.accepts(&r.display, r.module.as_deref())), "{scoped:?}");
    let want: BTreeSet<i64> = everywhere.iter().filter(|r| spec.accepts(&r.display, r.module.as_deref())).map(|r| r.id).collect();
    assert_eq!(scoped.iter().map(|r| r.id).collect::<BTreeSet<_>>(), want);
    // No query: the scope's declarations, which is what the files hold.
    let listed = rows(&SearchParams { query: String::new(), limit: 500, scope: Some(Scope::Modules(set)), ..Default::default() });
    let in_files: i64 = conn.query_row("SELECT sum(decl_count) FROM modules WHERE name LIKE 'SlimDemo.Graph.%'", [], |r| r.get(0)).unwrap();
    assert_eq!(listed.len() as i64, in_files);
    // A namespace: names at or below it, whatever file they are in.
    let ns = ScopeSpec { kind: ScopeKind::Namespace, prefix: "Demo.Rel".into() };
    let scoped = rows(&SearchParams { query: "trans".into(), limit: 500, scope: Some(Scope::Namespace(ns.prefix.clone())), ..Default::default() });
    assert!(!scoped.is_empty() && scoped.iter().all(|r| r.display.starts_with("Demo.Rel.")), "{scoped:?}");
    let want: BTreeSet<i64> = everywhere.iter().filter(|r| ns.accepts(&r.display, r.module.as_deref())).map(|r| r.id).collect();
    assert_eq!(scoped.iter().map(|r| r.id).collect::<BTreeSet<_>>(), want);
}

#[test]
fn every_recorded_occurrence_of_a_mention_is_that_identifier_in_the_file() {
    let (_d, project, idx) = source_project();
    let conn = db::open_read(&idx).unwrap();
    let mut cache = SourceCache::default();
    let mut checked = 0;
    let ids: Vec<(i64, String, i64, i64)> = conn
        .prepare("SELECT d.id, m.path, d.start_line, d.end_line FROM decls d JOIN modules m ON m.id = d.module_id WHERE d.kind IN (0, 1)")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(|x| x.unwrap())
        .collect();
    for (id, path, first, last) in ids {
        let text = std::fs::read_to_string(project.root.join(&path)).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        for m in slim::relations::lexical_mentions(&conn, Some(&project.root), &mut cache, id).unwrap() {
            assert!(!m.occurrences.is_empty(), "{} has no recorded place", m.ident);
            for o in &m.occurrences {
                let line = lines[o.line as usize - 1];
                assert_eq!(&line[o.start as usize..o.end as usize], m.ident, "line {} of {path}", o.line);
                assert!((o.line as i64) >= first && (o.line as i64) <= last, "{} at line {} is outside the declaration ({first}-{last})", m.ident, o.line);
                checked += 1;
            }
            assert!(m.occurrences.windows(2).all(|w| (w[0].line, w[0].start) < (w[1].line, w[1].start)), "in reading order");
        }
    }
    assert!(checked > 100, "only {checked} occurrences checked");
    // One a reader can verify: the soundness proof names the witness lemma once, on line 88.
    let id = query::decl_by_name(&conn, "Demo.Graph.searchWithin_sound").unwrap().unwrap();
    let ms = slim::relations::lexical_mentions(&conn, Some(&project.root), &mut cache, id).unwrap();
    let m = ms.iter().find(|m| m.ident == "anyOf_eq_true").unwrap();
    assert_eq!(m.occurrences.iter().map(|o| o.line).collect::<Vec<_>>(), vec![88]);
    // And the reflexivity constructor twice, once in each branch.
    let refl = ms.iter().find(|m| m.ident == "Rel.ReflTransGen.refl").unwrap();
    assert_eq!(refl.occurrences.iter().map(|o| o.line).collect::<Vec<_>>(), vec![76, 84]);
}

#[test]
fn an_index_of_another_format_is_refused_with_a_way_out() {
    let (_d, _project, idx) = source_project();
    {
        let c = rusqlite::Connection::open(&idx).unwrap();
        c.execute("UPDATE meta SET value = '1' WHERE key = 'schema_version'", []).unwrap();
    }
    let err = db::open_read(&idx).unwrap_err().to_string();
    assert!(err.contains("index format 1") && err.contains("slim scan") && err.contains("derived data"), "{err}");
}
