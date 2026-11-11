//! Staleness, moved checkouts, dependency changes and persistence.

use slim::db::{self, query};
use slim::paths::Project;
use slim::source::index::{self as sindex, ScanConfig};
use slim::source::lexer::LexProfile;
use slim::userdata::{Mark, UserStore};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
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

fn import_demo_at(src_root: &Path, idx: &Path) {
    let snap = root().join("fixtures/demo/snapshot/slim_demo.lean-v4.33.1.jsonl");
    let opts = db::import::ImportOptions { source_root: Some(src_root.to_path_buf()), ..Default::default() };
    db::import::import_snapshot(&snap, idx, &opts).unwrap();
}

#[test]
fn semantic_snapshot_goes_stale_when_sources_or_dependencies_change() {
    let d = tempfile::tempdir().unwrap();
    let proj = d.path().join("demo");
    copy_tree(&root().join("fixtures/demo"), &proj);
    let idx = d.path().join("sem.sqlite");
    import_demo_at(&proj, &idx);
    let cancel = AtomicBool::new(false);
    let conn = db::open_read(&idx).unwrap();
    let fr = sindex::check_freshness(&conn, &proj, false, true, &cancel).unwrap();
    assert!(fr.is_fresh(), "{fr:?}");
    // Edit one module: that module is reported changed.
    let f = proj.join("SlimDemo/Graph/Search.lean");
    let mut text = std::fs::read_to_string(&f).unwrap();
    text.push_str("\n-- edited\n");
    std::fs::write(&f, text).unwrap();
    let fr = sindex::check_freshness(&conn, &proj, false, false, &cancel).unwrap();
    assert_eq!(fr.changed, vec!["SlimDemo/Graph/Search.lean".to_string()]);
    // The recorded fingerprint no longer matches the file.
    let id = query::decl_by_name(&conn, "Demo.Graph.searchWithin_sound").unwrap().unwrap();
    let det = query::detail(&conn, id).unwrap().unwrap();
    let now = slim::paths::sha256_hex(&std::fs::read(&f).unwrap());
    assert_ne!(det.module_sha256.as_deref(), Some(now.as_str()));
    // Dependency/toolchain state changes are reported separately.
    std::fs::write(proj.join("lean-toolchain"), "leanprover/lean4:v4.35.0\n").unwrap();
    std::fs::write(proj.join("lake-manifest.json"), "{\"version\": \"1.2.0\", \"packages\": []}\n").unwrap();
    let fr = sindex::check_freshness(&conn, &proj, false, false, &cancel).unwrap();
    assert_eq!(fr.env_changed.len(), 2, "{:?}", fr.env_changed);
    // Deleting a module is reported as deleted.
    std::fs::remove_file(proj.join("SlimDemo/Relation/Equivalence.lean")).unwrap();
    let fr = sindex::check_freshness(&conn, &proj, false, false, &cancel).unwrap();
    assert_eq!(fr.deleted, vec!["SlimDemo/Relation/Equivalence.lean".to_string()]);
}

#[test]
fn moved_checkout_keeps_relative_paths_and_reading_data() {
    let d = tempfile::tempdir().unwrap();
    let a = d.path().join("first/demo");
    copy_tree(&root().join("fixtures/demo"), &a);
    let idx = d.path().join("sem.sqlite");
    import_demo_at(&a, &idx);
    // Move the checkout; the snapshot's paths are relative, so pointing the
    // index at the new root (an explicit, approved root) keeps excerpts working.
    let b = d.path().join("second/elsewhere");
    std::fs::create_dir_all(b.parent().unwrap()).unwrap();
    std::fs::rename(&a, &b).unwrap();
    let conn = db::open_read(&idx).unwrap();
    let cancel = AtomicBool::new(false);
    let fr = sindex::check_freshness(&conn, &b, false, true, &cancel).unwrap();
    assert!(fr.is_fresh(), "content verified at the new location: {fr:?}");
    let fr_old = sindex::check_freshness(&conn, &b.join("does-not-exist"), false, false, &cancel);
    assert!(fr_old.is_err() || !fr_old.unwrap().is_fresh());
    // Cache keys follow the location; reading data follows the package name.
    let pa = Project::open(&b).unwrap();
    std::fs::create_dir_all(&a).unwrap();
    std::fs::write(a.join("lakefile.toml"), "name = \"slim_demo\"\n").unwrap();
    let pb = Project::open(&a).unwrap();
    assert_ne!(pa.key, pb.key);
    assert_eq!(pa.user_key(), pb.user_key());
    let store = UserStore::open_at(&d.path().join("u.sqlite")).unwrap();
    store
        .toggle_bookmark(&pa.user_key(), &Mark { name: "Demo.Graph.searchWithin_sound".into(), display: "x".into(), kind: "theorem".into(), ..Default::default() })
        .unwrap();
    assert!(store.is_bookmarked(&pb.user_key(), "Demo.Graph.searchWithin_sound").unwrap());
}

#[test]
fn renamed_declaration_is_reported_missing_not_silently_redirected() {
    let d = tempfile::tempdir().unwrap();
    let proj = d.path().join("p");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("lakefile.toml"), "name = \"p\"\n").unwrap();
    std::fs::write(proj.join("A.lean"), "theorem old_name : True := trivial\n").unwrap();
    let project = Project::open(&proj).unwrap();
    let idx = d.path().join("src.sqlite");
    let cfg = ScanConfig { threads: 1, max_file_bytes: 1 << 20, profile: LexProfile::NEWEST };
    let cancel = AtomicBool::new(false);
    sindex::build_source_index(&project, &idx, &cfg, &cancel, None).unwrap();
    std::fs::write(proj.join("A.lean"), "theorem new_name : True := trivial\n").unwrap();
    let conn = db::open_read(&idx).unwrap();
    let fr = sindex::check_freshness(&conn, &proj, true, false, &cancel).unwrap();
    drop(conn);
    sindex::apply_incremental(&idx, &project, &fr, &cfg, &cancel).unwrap();
    let conn = db::open_read(&idx).unwrap();
    assert!(query::decl_by_name(&conn, "old_name").unwrap().is_none());
    assert!(query::decl_by_name(&conn, "new_name").unwrap().is_some());
}

#[test]
fn source_index_records_toolchain_and_reports_dependency_changes() {
    let d = tempfile::tempdir().unwrap();
    let proj = d.path().join("p");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("lean-toolchain"), "leanprover/lean4:v4.21.0\n").unwrap();
    std::fs::write(proj.join("lake-manifest.json"), "{}").unwrap();
    std::fs::write(proj.join("A.lean"), "def café := 1\n").unwrap();
    let project = Project::open(&proj).unwrap();
    let idx = d.path().join("src.sqlite");
    let cfg = ScanConfig::for_project(&project);
    // The lexer follows the project's Lean version: `é` is not an identifier
    // character before Lean 4.28, so the candidate is `caf`.
    assert!(!cfg.profile.latin_letters);
    let cancel = AtomicBool::new(false);
    sindex::build_source_index(&project, &idx, &cfg, &cancel, None).unwrap();
    let conn = db::open_read(&idx).unwrap();
    assert!(query::decl_by_name(&conn, "caf").unwrap().is_some());
    std::fs::write(proj.join("lean-toolchain"), "leanprover/lean4:v4.35.0\n").unwrap();
    let fr = sindex::check_freshness(&conn, &proj, true, false, &cancel).unwrap();
    assert_eq!(fr.env_changed.len(), 1);
    assert!(fr.env_changed[0].contains("v4.35.0"));
}

#[test]
fn hostile_snapshot_paths_never_leave_the_root() {
    let d = tempfile::tempdir().unwrap();
    let snap = root().join("fixtures/demo/snapshot/slim_demo.lean-v4.33.1.jsonl");
    let text = std::fs::read_to_string(&snap).unwrap();
    let evil = text.replacen("\"path\":\"SlimDemo/Graph/Search.lean\"", "\"path\":\"../../../../etc/passwd\"", 1);
    assert_ne!(evil, text, "fixture contains the expected path");
    let p = d.path().join("evil.jsonl");
    std::fs::write(&p, evil).unwrap();
    let idx = d.path().join("i.sqlite");
    let err = db::import::import_snapshot(&p, &idx, &db::import::ImportOptions::default()).unwrap_err();
    assert!(format!("{err:#}").contains("rejected source path"), "{err:#}");
    // Terminal escapes in text fields are stored raw but sanitised for display.
    let esc = text.replacen("TEST FIXTURE:", "TEST FIXTURE:\\u001b]52;c;ZXZpbA==\\u0007\\u001b[2J", 1);
    let p2 = d.path().join("esc.jsonl");
    std::fs::write(&p2, esc).unwrap();
    db::import::import_snapshot(&p2, &idx, &db::import::ImportOptions::default()).unwrap();
    let conn = db::open_read(&idx).unwrap();
    let doc: String = conn.query_row("SELECT doc FROM decls WHERE doc LIKE '%' || char(27) || '%' LIMIT 1", [], |r| r.get(0)).unwrap();
    let shown = slim::text::sanitize(&doc, false);
    assert!(!shown.contains('\x1b') && !shown.contains('\x07'));
}
