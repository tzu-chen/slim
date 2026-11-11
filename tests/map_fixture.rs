//! The project map, the goal explorer and reading trails against real data:
//! the bundled demo snapshot (produced by Lean 4.33.1, see
//! fixtures/demo/README.md), the demo sources scanned source-only, and a
//! labelled synthetic index for scale.

use slim::db::query::Direction;
use slim::db::{self, IndexMode};
use slim::map::{self, MapLoad, ProjectMap, RefStatus};
use slim::paths::Project;
use slim::relations::{self as rel, ChildQuery, ConnOutcome, ConnQuery, EdgeLabel, Env, Limits, RelMode, TypeBody};
use slim::source::index::{self as sindex, ScanConfig};
use slim::sourceview::SourceCache;
use slim::userdata::{self, ReadState, RefKind, Stop, StopStatus, UserStore};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn demo_dir() -> PathBuf {
    root().join("fixtures/demo")
}

fn demo_index() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let idx = d.path().join("demo.sqlite");
    let snap = demo_dir().join("snapshot/slim_demo.lean-v4.33.1.jsonl");
    let opts = db::import::ImportOptions { source_root: Some(demo_dir()), ..Default::default() };
    db::import::import_snapshot(&snap, &idx, &opts).unwrap();
    (d, idx)
}

fn demo_map() -> Arc<ProjectMap> {
    match map::load_for_root(&demo_dir()) {
        MapLoad::Loaded(m) => m,
        other => panic!("the sample map must load: {other:?}"),
    }
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let name = e.file_name();
        if name == ".lake" {
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

fn q(name: &str, mode: RelMode, dir: Direction) -> ChildQuery {
    ChildQuery { name: name.into(), mode, dir, local_only: true, show_generated: false, tb: TypeBody::All, offset: 0, limit: 50 }
}

fn names(p: &rel::ChildPage) -> Vec<&str> {
    p.children.iter().map(|c| c.name.as_str()).collect()
}

const SOUND: &str = "Demo.Graph.searchWithin_sound";
const HEAD: &str = "Demo.Rel.ReflTransGen.head";

#[test]
fn sample_map_is_valid_and_resolves_against_the_real_snapshot() {
    let m = demo_map();
    assert!(m.issues.is_empty(), "{:?}", m.issues);
    assert_eq!(m.project_name.as_deref(), Some("slim_demo"));
    assert_eq!(m.objectives.len(), 2);
    // Nested groups, and one declaration placed twice with one annotation.
    let closure = m.group_by_id("closure").unwrap();
    assert_eq!(m.groups[closure].parent, m.group_by_id("relations"));
    let placed: Vec<&str> = m.placements(HEAD).into_iter().map(|k| m.groups[k].id.as_str()).collect();
    assert_eq!(placed, ["closure", "search"]);
    assert_eq!(m.landmarks.iter().filter(|l| l.decl == HEAD).count(), 1);

    let (_d, idx) = demo_index();
    let conn = db::open_read(&idx).unwrap();
    let res = map::resolve(&m, &conn).unwrap();
    // Exactly the one deliberately wrong name is unresolved, with a hint
    // that is offered and never applied.
    let un = res.unresolved(&m);
    assert_eq!(un.len(), 1, "{un:?}");
    assert_eq!(un[0].name, "Demo.Graph.searchWithin_complete");
    assert_eq!(un[0].suggestions, ["Demo.Fixtures.searchWithin_complete"]);
    assert_eq!(un[0].used_in, ["group fixtures"]);
    assert!(res.row("Demo.Graph.searchWithin_complete").is_none());
    // Library declarations of the foundation group resolve as external.
    let core = m.group_by_id("core").unwrap();
    for p in &m.groups[core].landmarks {
        assert!(res.is_external(&p.decl), "{} should be a library declaration", p.decl);
    }
    assert!(!res.is_external(SOUND));
    // Unicode and quoted names resolve by their exact name.
    assert!(map::resolve_name(&conn, "Demo.Fixtures.«reach is transitive»").unwrap().is_some());
    assert!(map::resolve_name(&conn, "Demo.Fixtures.ℛ").unwrap().is_some());
    // Paper references keep their provenance; nothing is invented.
    let lm = m.landmark(SOUND).unwrap();
    assert_eq!(lm.paper[0].source.as_deref(), Some("readme"));
    assert!(m.sources.iter().any(|s| s.id == "readme"));
}

#[test]
fn source_only_index_explains_unresolvable_library_names() {
    let d = tempfile::tempdir().unwrap();
    let proj = d.path().join("demo");
    copy_tree(&demo_dir(), &proj);
    let p = Project::open(&proj).unwrap();
    let idx = d.path().join("src.sqlite");
    sindex::build_source_index(&p, &idx, &ScanConfig::for_project(&p), &AtomicBool::new(false), None).unwrap();
    let conn = db::open_read(&idx).unwrap();
    let m = demo_map();
    let res = map::resolve(&m, &conn).unwrap();
    let un = res.unresolved(&m);
    let prop = un.iter().find(|u| u.name == "propext").expect("library names cannot resolve in a source-only index");
    assert!(prop.why.contains("source-only index"), "{}", prop.why);
    assert!(un.iter().find(|u| u.name == "Demo.Graph.searchWithin_complete").unwrap().why.contains("renamed"));
    // Every local landmark still resolves: the map is usable without Lean.
    assert!(res.row(SOUND).is_some() && res.row(HEAD).is_some());

    // The goal explorer stays useful: curated and lexical work, Lean says why not.
    let mut cache = SourceCache::default();
    let mut env = Env { conn: &conn, mode: IndexMode::Source, map: Some(&m), source_root: Some(&proj), cache: &mut cache };
    let cur = rel::children(&mut env, &q(SOUND, RelMode::Curated, Direction::Uses)).unwrap();
    assert_eq!(names(&cur)[..4], ["Demo.Graph.searchWithin", "Demo.Graph.anyOf_eq_true", "Demo.Graph.mem_succsOf", HEAD]);
    let lex = rel::children(&mut env, &q(SOUND, RelMode::Mentions, Direction::Uses)).unwrap();
    assert!(names(&lex).contains(&"Demo.Graph.anyOf_eq_true") && names(&lex).contains(&HEAD));
    assert!(lex.children.iter().all(|c| c.labels.iter().all(|l| matches!(l, EdgeLabel::Lexical { .. }))));
    assert!(RelMode::Mentions.explain().contains("not resolved by Lean"));
    let lean = rel::children(&mut env, &q(SOUND, RelMode::Lean, Direction::Uses)).unwrap();
    assert!(lean.children.is_empty() && lean.lean_counts.is_none());
    assert!(lean.notes.iter().any(|(t, _)| t.contains("unavailable") && t.contains("source-only")));
    let lim = Limits::default();
    let c = rel::connection(&mut env, &ConnQuery { from: SOUND.into(), to: HEAD.into(), mode: RelMode::Lean, dir: Direction::Uses, tb: TypeBody::All }, &lim, &|| false);
    assert!(matches!(&c.outcome, ConnOutcome::Unavailable(w) if w.contains("source-only index")), "{:?}", c.outcome);
    // A lexical connection is found and labelled lexical end to end.
    let c = rel::connection(&mut env, &ConnQuery { from: SOUND.into(), to: "Demo.Rel.ReflTransGen.trans".into(), mode: RelMode::Mentions, dir: Direction::Uses, tb: TypeBody::All }, &lim, &|| false);
    match &c.outcome {
        ConnOutcome::Path(p) => {
            assert_eq!(p.first().unwrap().name, SOUND);
            assert_eq!(p.last().unwrap().name, "Demo.Rel.ReflTransGen.trans");
            assert!(p[..p.len() - 1].iter().all(|s| matches!(s.next, Some(EdgeLabel::Lexical { .. }))));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn lean_dependencies_match_the_fixture_and_keep_type_and_body_apart() {
    let (_d, idx) = demo_index();
    let conn = db::open_read(&idx).unwrap();
    let m = demo_map();
    let mut cache = SourceCache::default();
    let mut env = Env { conn: &conn, mode: IndexMode::Semantic, map: Some(&m), source_root: Some(&demo_dir()), cache: &mut cache };
    // `head` is `trans (single hab) hbc`, stated over `Rel` and `ReflTransGen`:
    // the expected local edges of the fixture source, each with its evidence.
    let p = rel::children(&mut env, &q(HEAD, RelMode::Lean, Direction::Uses)).unwrap();
    let got: BTreeSet<(String, String)> = p.children.iter().map(|c| (c.name.clone(), c.labels[0].tag())).collect();
    let want: BTreeSet<(String, String)> = [
        ("Demo.Rel", "T+V"),
        ("Demo.Rel.ReflTransGen", "T+V"),
        ("Demo.Rel.ReflTransGen.single", "V"),
        ("Demo.Rel.ReflTransGen.trans", "V"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    assert_eq!(got, want);
    assert!(p.children.iter().all(|c| c.labels.len() == 1 && matches!(c.labels[0], EdgeLabel::Lean { .. })));
    // Statement-only: the proof's lemmas disappear and are counted, not lost.
    let ty = rel::children(&mut env, &ChildQuery { tb: TypeBody::Type, ..q(HEAD, RelMode::Lean, Direction::Uses) }).unwrap();
    assert_eq!(names(&ty).into_iter().collect::<BTreeSet<_>>(), ["Demo.Rel", "Demo.Rel.ReflTransGen"].into_iter().collect());
    let counts = p.lean_counts.unwrap();
    assert_eq!(ty.hidden_kind, counts[1] + counts[2]);
    assert!(ty.hidden_kind >= 4);
    // The soundness theorem is compiled by structural recursion: its direct
    // proof-term edge goes to a Lean-generated auxiliary. Hidden by default,
    // counted, and shown on request.
    let s = rel::children(&mut env, &q(SOUND, RelMode::Lean, Direction::Uses)).unwrap();
    assert!(names(&s).contains(&"Demo.Graph.Reach"));
    assert!(!names(&s).iter().any(|n| n.contains("._f")));
    assert!(s.hidden_generated >= 1, "{s:?}");
    let all = rel::children(&mut env, &ChildQuery { local_only: false, show_generated: true, ..q(SOUND, RelMode::Lean, Direction::Uses) }).unwrap();
    let aux = all.children.iter().find(|c| c.name.ends_with("searchWithin_sound._f")).expect("generated auxiliary shown");
    assert_eq!(aux.labels[0].tag(), "V");
    assert_eq!(all.hidden_generated + all.hidden_external, 0);
    assert!(all.children.len() > s.children.len());
    // External declarations: hidden with a count when local-only.
    let ext = rel::children(&mut env, &q("Demo.Graph.mem_cons_iff", RelMode::Lean, Direction::Uses)).unwrap();
    assert!(ext.hidden_external >= 1);
    // Direction: `head` is used by what the curated argument says rests on it.
    let by = rel::children(&mut env, &ChildQuery { show_generated: true, ..q(HEAD, RelMode::Lean, Direction::UsedBy) }).unwrap();
    assert!(names(&by).contains(&"Demo.Rel.ReflTransGen.symmetric"));
    assert!(names(&by).iter().any(|n| n.ends_with("searchWithin_sound._f")));
    assert!(!names(&by).contains(&"Demo.Rel.ReflTransGen.single"), "used-by is not uses");
}

#[test]
fn curated_lexical_and_lean_edges_keep_their_own_labels_in_the_mixed_view() {
    let (_d, idx) = demo_index();
    let conn = db::open_read(&idx).unwrap();
    let m = demo_map();
    let mut cache = SourceCache::default();
    let mut env = Env { conn: &conn, mode: IndexMode::Semantic, map: Some(&m), source_root: Some(&demo_dir()), cache: &mut cache };
    // The curated argument, in its written order, labelled as curation.
    let cur = rel::children(&mut env, &q(SOUND, RelMode::Curated, Direction::Uses)).unwrap();
    assert_eq!(names(&cur), ["Demo.Graph.searchWithin", "Demo.Graph.anyOf_eq_true", "Demo.Graph.mem_succsOf", HEAD, "Demo.Graph.Reach"]);
    assert!(cur.children.iter().all(|c| matches!(c.labels[..], [EdgeLabel::Curated { .. }])));
    assert_eq!(cur.children[1].labels[0].tag(), "cur 2");
    assert!(cur.children[1].note.as_deref().unwrap().contains("concrete successor"));
    assert!(cur.lean_counts.is_none(), "a curated view contains no Lean-derived edge");
    // Sub-steps hang under the step they support.
    let sub = rel::children(&mut env, &q(HEAD, RelMode::Curated, Direction::Uses)).unwrap();
    assert_eq!(names(&sub), ["Demo.Rel.ReflTransGen.single", "Demo.Rel.ReflTransGen.trans"]);
    let rev = rel::children(&mut env, &q(HEAD, RelMode::Curated, Direction::UsedBy)).unwrap();
    assert_eq!(names(&rev), [SOUND]);
    // Mixed: one row per declaration, every kind of evidence still labelled.
    let mix = rel::children(&mut env, &q(SOUND, RelMode::Mixed, Direction::Uses)).unwrap();
    let reach = mix.children.iter().find(|c| c.name == "Demo.Graph.Reach").unwrap();
    let tags: Vec<String> = reach.labels.iter().map(|l| l.tag()).collect();
    // (The source writes `g.Reach`, which a lexical scan does not see: no [lex].)
    assert_eq!(tags, ["cur", "T+V"]);
    // `anyOf_eq_true` is used inside the generated auxiliary, so on the
    // theorem itself it is curated and lexical but NOT a direct Lean edge.
    let any = mix.children.iter().find(|c| c.name == "Demo.Graph.anyOf_eq_true").unwrap();
    assert!(any.labels.iter().any(|l| matches!(l, EdgeLabel::Curated { .. })));
    assert!(any.labels.iter().any(|l| matches!(l, EdgeLabel::Lexical { .. })));
    assert!(!any.labels.iter().any(|l| matches!(l, EdgeLabel::Lean { .. })));
    // Lexical mentions work on a snapshot too (line ranges), still labelled.
    let lex = rel::children(&mut env, &q(SOUND, RelMode::Mentions, Direction::Uses)).unwrap();
    assert!(names(&lex).contains(&"Demo.Graph.mem_succsOf"));
    let back = rel::children(&mut env, &q(SOUND, RelMode::Mentions, Direction::UsedBy)).unwrap();
    assert!(back.children.is_empty() && back.notes.iter().any(|(t, _)| t.contains("outward only")));
    // A name the index does not have is a child without a row, not a crash.
    let none = rel::children(&mut env, &q("Demo.Nowhere", RelMode::Mixed, Direction::Uses)).unwrap();
    assert!(none.children.is_empty() && none.notes.iter().any(|(t, _)| t.contains("not in this index")));
}

#[test]
fn connection_queries_are_bounded_and_say_what_they_found() {
    let (_d, idx) = demo_index();
    let conn = db::open_read(&idx).unwrap();
    let m = demo_map();
    let mut cache = SourceCache::default();
    let mut env = Env { conn: &conn, mode: IndexMode::Semantic, map: Some(&m), source_root: Some(&demo_dir()), cache: &mut cache };
    let lim = Limits::default();
    let cq = |to: &str, mode, dir, tb| ConnQuery { from: SOUND.into(), to: to.into(), mode, dir, tb };
    let never = || false;
    // Lean dependencies: the path runs through the generated auxiliary.
    let r = rel::connection(&mut env, &cq("Demo.Rel.ReflTransGen.trans", RelMode::Lean, Direction::Uses, TypeBody::All), &lim, &never);
    let ConnOutcome::Path(p) = &r.outcome else { panic!("{:?}", r.outcome) };
    let path: Vec<&str> = p.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(path, [SOUND, "Demo.Graph.searchWithin_sound._f", HEAD, "Demo.Rel.ReflTransGen.trans"]);
    assert!(p[..3].iter().all(|s| matches!(s.next, Some(EdgeLabel::Lean { body: true, .. }))) && p[3].next.is_none());
    // Each step is a recorded edge.
    for w in p.windows(2) {
        let n: i64 = conn
            .query_row("SELECT count(*) FROM edges WHERE src = ?1 AND dst = ?2", [w[0].row.as_ref().unwrap().id, w[1].row.as_ref().unwrap().id], |r| r.get(0))
            .unwrap();
        assert!(n >= 1);
    }
    // Statement-only edges do not reach a proof lemma: an exhaustive "no path".
    let r = rel::connection(&mut env, &cq("Demo.Rel.ReflTransGen.trans", RelMode::Lean, Direction::Uses, TypeBody::Type), &lim, &never);
    assert_eq!(r.outcome, ConnOutcome::NoPath);
    assert!(r.explored > 1);
    // The direction is part of the question.
    let r = rel::connection(&mut env, &cq("Demo.Rel.ReflTransGen.trans", RelMode::Lean, Direction::UsedBy, TypeBody::All), &lim, &never);
    assert_eq!(r.outcome, ConnOutcome::NoPath);
    let r = rel::connection(&mut env, &ConnQuery { from: "Demo.Rel.ReflTransGen.trans".into(), to: SOUND.into(), mode: RelMode::Lean, dir: Direction::UsedBy, tb: TypeBody::All }, &lim, &never);
    assert!(matches!(r.outcome, ConnOutcome::Path(ref p) if p.len() == 4));
    // A work limit gives "truncated", never "no path".
    let r = rel::connection(&mut env, &cq("Demo.Rel.ReflTransGen.trans", RelMode::Lean, Direction::Uses, TypeBody::All), &Limits { max_nodes: 3, max_ms: 5000 }, &never);
    assert_eq!(r.outcome, ConnOutcome::Truncated { by_time: false });
    let r = rel::connection(&mut env, &cq("Demo.Rel.ReflTransGen.trans", RelMode::Lean, Direction::Uses, TypeBody::All), &lim, &|| true);
    assert_eq!(r.outcome, ConnOutcome::Cancelled);
    // Curated relations: the author's route, shorter and labelled as such.
    let r = rel::connection(&mut env, &cq("Demo.Rel.ReflTransGen.trans", RelMode::Curated, Direction::Uses, TypeBody::All), &lim, &never);
    let ConnOutcome::Path(p) = &r.outcome else { panic!("{:?}", r.outcome) };
    assert_eq!(p.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), [SOUND, HEAD, "Demo.Rel.ReflTransGen.trans"]);
    assert!(matches!(p[0].next, Some(EdgeLabel::Curated { step: Some(4), .. })));
    let r = rel::connection(&mut env, &cq("Demo.Rel.ext", RelMode::Curated, Direction::Uses, TypeBody::All), &lim, &never);
    assert_eq!(r.outcome, ConnOutcome::NoPath);
    // One kind at a time; unknown endpoints are named.
    let r = rel::connection(&mut env, &cq(HEAD, RelMode::Mixed, Direction::Uses, TypeBody::All), &lim, &never);
    assert!(matches!(r.outcome, ConnOutcome::Unavailable(_)));
    let r = rel::connection(&mut env, &cq("Demo.Nowhere", RelMode::Lean, Direction::Uses, TypeBody::All), &lim, &never);
    assert!(matches!(&r.outcome, ConnOutcome::Unavailable(w) if w.contains("Demo.Nowhere")));
    assert!(r.query.describe().contains("Lean dependencies"));
}

#[test]
fn trails_survive_reindexing_and_show_stale_and_renamed_stops() {
    let d = tempfile::tempdir().unwrap();
    let proj = d.path().join("demo");
    copy_tree(&demo_dir(), &proj);
    let p = Project::open(&proj).unwrap();
    let idx = d.path().join("src.sqlite");
    let cfg = ScanConfig::for_project(&p);
    let cancel = AtomicBool::new(false);
    sindex::build_source_index(&p, &idx, &cfg, &cancel, None).unwrap();
    let store = UserStore::open_at(&d.path().join("user.sqlite")).unwrap();
    let key = p.user_key();
    let m = demo_map();
    // Start the shared trail: a personal copy with locators from the index.
    let shared = &m.trails[0];
    let (tid, status_before) = {
        let conn = db::open_read(&idx).unwrap();
        let stops: Vec<Stop> = shared
            .stops
            .iter()
            .map(|s| match &s.target {
                map::StopRef::Decl(n) => Stop { note: s.note.clone(), question: s.question.clone(), ..slim::app::worker::stop_for_name(&conn, n, "source-only index") },
                map::StopRef::Group(g) => Stop { ref_kind: RefKind::Group, name: g.clone(), display: g.clone(), kind: "group".into(), note: s.note.clone(), ..Default::default() },
            })
            .collect();
        let (tid, new) = store.start_shared_trail(&key, shared, &stops).unwrap();
        assert!(new);
        assert_eq!(store.start_shared_trail(&key, shared, &stops).unwrap(), (tid, false), "started once");
        let t = store.trail(&key, tid).unwrap().unwrap();
        assert_eq!(t.stops.len(), 8);
        assert!(t.stops.iter().filter(|s| s.ref_kind == RefKind::Decl).all(|s| s.sha256.is_some() && s.path.is_some() && s.line.is_some()));
        let st = userdata::stop_statuses(&conn, Some(&m), &t);
        (tid, st)
    };
    assert!(status_before.values().all(|s| *s == StopStatus::Current), "{status_before:?}");
    let t = store.trail(&key, tid).unwrap().unwrap();
    let sound = t.stops.iter().find(|s| s.name == SOUND).unwrap().id;
    let closure = t.stops.iter().find(|s| s.name == "Demo.Rel.ReflTransGen").unwrap().id;
    let any = t.stops.iter().find(|s| s.name == "Demo.Graph.anyOf_eq_true").unwrap().id;
    store.stop_set_state(&key, sound, ReadState::Understood).unwrap();
    store.stop_set_note(&key, sound, Some("my own summary")).unwrap();

    // The sources change under the reader: one file is edited without a
    // commit (a dirty worktree), and one declaration is renamed.
    let f = proj.join("SlimDemo/Graph/Search.lean");
    let text = std::fs::read_to_string(&f).unwrap().replace("anyOf_eq_true", "anyOf_true_iff");
    std::fs::write(&f, text).unwrap();
    {
        let conn = db::open_read(&idx).unwrap();
        let fr = sindex::check_freshness(&conn, &proj, true, true, &cancel).unwrap();
        assert_eq!(fr.changed, ["SlimDemo/Graph/Search.lean"]);
        drop(conn);
        sindex::apply_incremental(&idx, &p, &fr, &cfg, &cancel).unwrap();
    }
    let conn = db::open_read(&idx).unwrap();
    let t = store.trail(&key, tid).unwrap().unwrap();
    assert_eq!(t.stops.iter().find(|s| s.id == sound).unwrap().note.as_deref(), Some("my own summary"), "notes survive reindexing");
    let st = userdata::stop_statuses(&conn, Some(&m), &t);
    assert!(matches!(st[&sound], StopStatus::Stale(_)), "same name, changed file: {:?}", st[&sound]);
    assert!(matches!(st[&any], StopStatus::Unresolved(_)), "renamed away: {:?}", st[&any]);
    assert_eq!(st[&closure], StopStatus::Current, "an untouched file stays current");
    // The map sees the rename as an unresolved reference with a hint only.
    let res = map::resolve(&m, &conn).unwrap();
    assert!(matches!(&res.refs["Demo.Graph.anyOf_eq_true"].status, RefStatus::Missing { .. }));
    assert!(res.row("Demo.Graph.anyOf_true_iff").is_none(), "the annotation did not silently move to the new name");
    // An annotation that recorded a fingerprint goes stale with its file.
    let sha = res.refs[SOUND].module_sha256.clone().unwrap();
    let pinned = format!("schema = 1\n[[landmark]]\ndecl = \"{SOUND}\"\npurpose = \"p\"\nsource_sha256 = \"{}\"\n", &sha[..16]);
    let pm = map::parse(&pinned, Path::new("x")).unwrap();
    assert!(!map::resolve(&pm, &conn).unwrap().is_stale(&pm.landmarks[0]));
    let old = pinned.replace(&sha[..16], "0000000000000000");
    let om = map::parse(&old, Path::new("x")).unwrap();
    assert!(map::resolve(&om, &conn).unwrap().is_stale(&om.landmarks[0]));

    // Export: identity, revision (none here), states, notes, stale and unresolved.
    let info = userdata::ExportInfo { project: key.clone(), revision: slim::paths::git_revision(&proj), index_desc: "the project's source-only index".into(), statuses: st };
    let md = userdata::trail_export_markdown(&t, &info);
    assert!(md.contains("# The soundness of bounded search in ten minutes"));
    assert!(md.contains("- Project: `lake:slim_demo`"));
    assert!(md.contains("started from the shared trail `soundness`"));
    assert!(md.contains("**3 of 8 stops need attention**"), "two stale stops in the edited file, one renamed away");
    assert!(md.contains("**`Demo.Graph.searchWithin_sound`** (theorem) — `SlimDemo/Graph/Search.lean:"));
    assert!(md.contains("state: understood") && md.contains("> my own summary"));
    assert!(md.contains("**STALE**") && md.contains("**UNRESOLVED**"));
    assert!(md.contains("1. **`graphs`** (map group)"));
    assert!(md.contains("- Question: What would a proof of completeness need"));
    // The inspected project was never written to.
    assert!(!proj.join("lean-view.toml.bak").exists());
    assert_eq!(std::fs::read_to_string(proj.join("lean-view.toml")).unwrap(), std::fs::read_to_string(demo_dir().join("lean-view.toml")).unwrap());
}

/// SYNTHETIC data (deterministic generator, labelled as such in the index):
/// 100,000 declarations with sparse edges, plus one adversarial hub.
#[test]
fn synthetic_100k_with_a_hub_stays_paged_and_bounded() {
    let d = tempfile::tempdir().unwrap();
    let idx = d.path().join("syn.sqlite");
    let snap = d.path().join("syn.jsonl");
    slim::bench::generate(&snap, 100_000, 400_000, 7).unwrap();
    db::import::import_snapshot(&snap, &idx, &db::import::ImportOptions::default()).unwrap();
    // Turn declaration 1 into a hub: a value edge to 60,000 others.
    let (hub, far) = {
        let conn = db::open_write(&idx).unwrap();
        conn.execute_batch("BEGIN; INSERT OR IGNORE INTO edges(src, kind, dst) SELECT 1, 2, id FROM decls WHERE id BETWEEN 2 AND 60001; COMMIT;").unwrap();
        let name = |id: i64| conn.query_row("SELECT name FROM decls WHERE id = ?1", [id], |r| r.get::<_, String>(0)).unwrap();
        (name(1), name(99_999))
    };
    let conn = db::open_read(&idx).unwrap();
    assert_eq!(db::get_meta(&conn, "synthetic").unwrap().as_deref(), Some("1"));
    let mut cache = SourceCache::default();
    let mut env = Env { conn: &conn, mode: IndexMode::Semantic, map: None, source_root: None, cache: &mut cache };
    let mut worst = 0u128;
    let mut seen = BTreeSet::new();
    let mut offset = 0;
    for _ in 0..5 {
        let t = std::time::Instant::now();
        let p = rel::children(&mut env, &ChildQuery { name: hub.clone(), mode: RelMode::Lean, dir: Direction::Uses, local_only: false, show_generated: true, tb: TypeBody::All, offset, limit: 50 }).unwrap();
        worst = worst.max(t.elapsed().as_millis());
        assert!(p.children.len() <= 50, "one page at a time");
        seen.extend(p.children.iter().map(|c| c.name.clone()));
        if offset == 0 {
            assert!(p.lean_counts.unwrap()[1] >= 60_000);
        }
        assert!(p.remaining >= 9_000, "the rest is counted (capped), not loaded: {}", p.remaining);
        offset = p.more.expect("more pages");
    }
    eprintln!("SYNTHETIC 100k: worst of 5 pages of a 60,000-edge hub: {worst} ms");
    assert!(seen.len() > 200, "pages do not repeat each other");
    assert!(worst < 500, "a page of a 60,000-edge hub took {worst} ms");
    // A connection query from the hub is bounded by its limits.
    let t = std::time::Instant::now();
    let r = rel::connection(&mut env, &ConnQuery { from: hub.clone(), to: far, mode: RelMode::Lean, dir: Direction::Uses, tb: TypeBody::All }, &Limits { max_nodes: 20_000, max_ms: 1500 }, &|| false);
    eprintln!("SYNTHETIC 100k: connection query from the hub: {:?} after {} declarations in {} ms", std::mem::discriminant(&r.outcome), r.explored, r.ms);
    eprintln!("SYNTHETIC 100k: outcome {}", match &r.outcome { ConnOutcome::Path(p) => format!("path of {} steps", p.len() - 1), o => format!("{o:?}") });
    assert!(t.elapsed().as_millis() < 3000);
    assert!(matches!(r.outcome, ConnOutcome::Path(_) | ConnOutcome::Truncated { .. } | ConnOutcome::NoPath), "{:?}", r.outcome);
    assert!(r.explored <= 20_000 + 1);
}
