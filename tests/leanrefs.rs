//! Lean's references read from build artifacts: a project with an `.ilean`
//! and a `.trace` written the way Lake writes them.

use slim::db::{self, query};
use slim::leanrefs::{self, RefStatus};
use slim::model::Origin;
use slim::paths::Project;
use slim::relations::{self, Part};
use slim::source::index::{self as sindex, ScanConfig};
use slim::sourceview::SourceCache;
use std::sync::atomic::AtomicBool;

const SRC: &str = "namespace P\ndef foo : Nat := 1\ntheorem bar : foo = 1 := rfl\ntheorem baz : foo = 1 := bar\nend P\n";

/// What Lake 4.28 and later write (version 5): ranges are 0-based lines and
/// UTF-16 columns; a use names the declaration it stands in.
const ILEAN: &str = r#"{"version":5,"module":"P.A","directImports":[],"decls":{"P.foo":[1,0,1,18,1,4,1,7]},"references":{
"{\"c\":{\"m\":\"P.A\",\"n\":\"P.foo\"}}":{"definition":[1,4,1,7],"usages":[[2,14,2,17,"P.bar"],[3,14,3,17,"P.baz"]]},
"{\"c\":{\"m\":\"P.A\",\"n\":\"P.bar\"}}":{"definition":[2,8,2,11],"usages":[[3,25,3,28,"P.baz"]]},
"{\"c\":{\"m\":\"Init.Prelude\",\"n\":\"Nat\"}}":{"usages":[[1,10,1,13,"P.foo"]]},
"{\"c\":{\"m\":\"Init.Core\",\"n\":\"«term_=_»\"}}":{"usages":[[2,14,2,21,"P.bar"],[3,14,3,21,"P.baz"]]}}}"#;

#[test]
fn references_come_from_artifacts_built_from_the_text_shown() {
    // The package is the opened directory, or lies below it (a repository
    // that keeps its Lake package in `lean/`).
    for sub in ["", "lean/"] {
        check(sub);
    }
}

fn check(sub: &str) {
    let d = tempfile::tempdir().unwrap();
    let opened = d.path().join("p");
    let root = opened.join(sub);
    let lib = root.join(".lake/build/lib/lean/P");
    std::fs::create_dir_all(root.join("P")).unwrap();
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(root.join("lakefile.toml"), "name = \"p\"\n").unwrap();
    std::fs::write(root.join("P/A.lean"), SRC).unwrap();
    std::fs::write(lib.join("A.ilean"), ILEAN).unwrap();
    let trace = format!(
        r#"{{"schemaVersion":"2025-09-10","inputs":[["deps",[]],["{}","{:016x}"]],"outputs":{{"i":"{:016x}.ilean"}}}}"#,
        root.join("P/A.lean").display(),
        leanrefs::lake_text_hash(SRC),
        leanrefs::lake_hash(ILEAN.as_bytes())
    );
    std::fs::write(lib.join("A.trace"), trace).unwrap();
    let pkg = root;
    let root = opened;
    let project = Project::open(&root).unwrap();
    let idx = d.path().join("source.sqlite");
    sindex::build_source_index(&project, &idx, &ScanConfig { threads: 1, ..ScanConfig::for_project(&project) }, &AtomicBool::new(false), None).unwrap();
    let conn = db::open_read(&idx).unwrap();
    let mut cache = SourceCache::default();
    let id = |n: &str| query::decl_by_name(&conn, n).unwrap().unwrap();

    // What `baz` writes, as Lean resolved it: `foo` in the statement, `bar`
    // in the proof. The `=` notation is resolved too, and is not a name.
    let m = relations::mentions(&conn, Some(&root), &mut cache, id("P.baz")).unwrap();
    assert!(m.lean && m.status == RefStatus::Verified, "{:?}", m.status);
    let got: Vec<(&str, Part, i64, u32)> = m.items.iter().map(|x| (x.candidates[0].name.as_str(), x.part, x.candidates[0].id, x.occurrences[0].start)).collect();
    assert_eq!(got, vec![("P.foo", Part::Statement, id("P.foo"), 14), ("P.bar", Part::Body, id("P.bar"), 25)]);
    // A library name has no row in the index: it is known by name and module.
    let m = relations::mentions(&conn, Some(&root), &mut cache, id("P.foo")).unwrap();
    let nat = &m.items[0].candidates[0];
    assert!(nat.id < 0 && nat.origin == Origin::Library && nat.module.as_deref() == Some("Init.Prelude") && nat.name == "Nat");

    // Who uses `foo`: each use goes to the declaration whose lines hold it.
    let u = leanrefs::used_by(&conn, &root, &mut cache, "P.foo", Some("P.A"), id("P.foo"), 50).unwrap();
    assert_eq!(u.users.iter().map(|x| (x.row.name.as_str(), x.uses, x.line)).collect::<Vec<_>>(), vec![("P.bar", 1, 3), ("P.baz", 1, 4)]);
    assert_eq!((u.read_files, u.local_files), (1, 1));

    // The file changes: its artifacts are of another text, and are not used.
    std::fs::write(pkg.join("P/A.lean"), format!("{SRC}-- edited\n")).unwrap();
    let m = relations::mentions(&conn, Some(&root), &mut SourceCache::default(), id("P.baz")).unwrap();
    assert!(!m.lean && m.status == RefStatus::SourceDiffers, "{:?}", m.status);
    let u = leanrefs::used_by(&conn, &root, &mut SourceCache::default(), "P.foo", Some("P.A"), id("P.foo"), 50).unwrap();
    assert!(u.users.is_empty() && u.read_files == 0);
}
