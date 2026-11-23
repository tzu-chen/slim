//! The reading aids on a small project laid out like a large published
//! proof: a Lake package below the opened root with a source directory,
//! named statements, relay theorems, a file nothing imports. Checked
//! through the command line, as a reader would use them.

use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_slim");

struct World {
    _d: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
    data: PathBuf,
}

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

const DEFS: &str = "namespace P\n\ndef Good (n : Nat) : Prop := n = n\n\ndef Big : Prop := ∀ n, Good n\n\nend P\n";
const LEMMAS: &str = "import Defs\nnamespace P\ntheorem good_all (n : Nat) : Good n := rfl\ntheorem big_holds : Big := fun n => good_all n\ntheorem unused (key : Nat) : key = key := by\n  have good_all := key\n  rfl\nend P\n";
const MAIN: &str = "import Lemmas\ntheorem main_thm : P.Big := P.big_holds\ntheorem relay : P.Big := main_thm\n";

fn world() -> World {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("repo");
    write(&root, "lean/lakefile.toml", "name = \"tree\"\nsrcDir = \"src\"\n");
    write(&root, "lean/src/Defs.lean", DEFS);
    write(&root, "lean/src/Lemmas.lean", LEMMAS);
    write(&root, "lean/src/Main.lean", MAIN);
    let w = World { root, cache: d.path().join("cache"), data: d.path().join("data"), _d: d };
    let out = w.run(&["scan", w.dir()]);
    assert!(out.contains("3 files"), "{out}");
    w
}

impl World {
    fn dir(&self) -> &str {
        self.root.to_str().unwrap()
    }
    fn run(&self, args: &[&str]) -> String {
        self.run_code(args).0
    }
    fn run_code(&self, args: &[&str]) -> (String, i32) {
        let out = Command::new(BIN).args(args).env("SLIM_CACHE_DIR", &self.cache).env("SLIM_DATA_DIR", &self.data).output().unwrap();
        (format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)), out.status.code().unwrap_or(-1))
    }
}

#[test]
fn imports_resolve_in_a_package_below_the_root_and_point_at_the_last_file() {
    let w = world();
    let out = w.run(&["imports", w.dir()]);
    assert!(out.contains("2 import lines between them"), "{out}");
    assert!(out.contains("deepest chain 2"), "{out}");
    // The one file with declarations that nothing imports.
    let endpoints = out.split("endpoints:").nth(1).unwrap_or("");
    assert!(endpoints.contains("lean.src.Main"), "{out}");
}

#[test]
fn the_lexical_graph_reaches_what_the_last_file_uses_and_explains_why() {
    let w = world();
    let out = w.run(&["graph", "build", w.dir(), "--lexical"]);
    assert!(out.contains("source mentions (lexical)"), "{out}");
    assert!(out.contains("endpoints (the theorems of the files that nothing imports): main_thm, relay"), "{out}");
    // main_thm, relay, P.big_holds, P.Big, P.good_all, P.Good; not P.unused.
    assert!(out.contains("reached from the endpoints: 6 of 7 declarations"), "{out}");

    let (why, code) = w.run_code(&["why", w.dir(), "P.good_all"]);
    assert_eq!(code, 0, "{why}");
    let chain: Vec<&str> = why.lines().filter(|l| l.starts_with("  ") && l.trim_start().starts_with(|c: char| c.is_ascii_digit())).collect();
    assert_eq!(chain.len(), 3, "{why}");
    assert!(chain[0].contains("main_thm") && chain[0].contains("endpoint"), "{why}");
    assert!(chain[1].contains("P.big_holds") && chain[1].contains("in the proof or body"), "{why}");
    assert!(chain[2].contains("P.good_all") && chain[2].contains("lean/src/Lemmas.lean:3"), "{why}");
    assert!(why.contains("every chain to it passes through: main_thm ▸ P.big_holds"), "{why}");

    // Not reached: said, with what does refer to it; and a name it binds
    // itself (`have good_all`) is not a reference to the theorem.
    let (not, code) = w.run_code(&["why", w.dir(), "P.unused"]);
    assert_eq!(code, 1);
    assert!(not.contains("not reached from the endpoints"), "{not}");
    let show = w.run(&["graph", "show", w.dir(), "P.unused"]);
    assert!(show.contains("refers to (0"), "{show}");

    // The contents: a relay and its target on one row; sizes are subtrees.
    let c = w.run(&["contents", w.dir()]);
    // (P.Big and P.Good are named by both endpoints: neither owns them.)
    assert!(c.lines().any(|l| l.trim_start().starts_with("3") && l.contains("thm main_thm = P.big_holds")), "{c}");
    assert!(c.lines().any(|l| l.contains("relay") && l.contains("= main_thm")), "{c}");

    // The ledger pairs a named statement with the theorem of that type.
    let l = w.run(&["ledger", w.dir()]);
    assert!(l.contains("2 named statements"), "{l}");
    assert!(l.lines().any(|x| x.contains("P.Big") && x.contains("⊢ P.big_holds")), "{l}");
    assert!(l.lines().any(|x| x.contains("P.Good") && x.contains("—")), "{l}");
}

#[test]
fn a_statement_unfolds_into_the_definitions_it_names() {
    let w = world();
    let one = w.run(&["contract", w.dir(), "P.big_holds"]);
    assert!(one.contains("then  Big"), "{one}");
    assert!(one.contains("│ def Big : Prop := ∀ n, Good n"), "{one}");
    assert!(one.contains("⋯ P.Good (folded)"), "{one}");
    assert!(one.contains("1 definition(s) unfolded to depth 1"), "{one}");
    let two = w.run(&["contract", w.dir(), "P.big_holds", "--depth", "2"]);
    assert!(two.contains("│ def Good (n : Nat) : Prop := n = n"), "{two}");
    assert!(two.contains("nothing of the project is left folded"), "{two}");
    // Binders are given, the conclusion is then.
    let g = w.run(&["contract", w.dir(), "P.good_all", "--depth", "0"]);
    assert!(g.contains("given (n : Nat)") && g.contains("then  Good n"), "{g}");
}

#[test]
fn lean_references_from_an_artifact_directory_replace_the_lexical_reading() {
    let w = world();
    // What Lean would have written for Lemmas.lean (0-based lines, UTF-16 columns).
    let art = w.root.parent().unwrap().join("artifacts");
    let ilean = serde_json::json!({
        "version": 4,
        "module": "Lemmas",
        "directImports": [["Defs", false, false, false]],
        "decls": {
            "P.good_all": [2, 0, 2, 43, 2, 8, 2, 16],
            "P.big_holds": [3, 0, 3, 46, 3, 8, 3, 17],
            "P.unused": [4, 0, 6, 5, 4, 8, 4, 14]
        },
        "references": {
            "{\"c\":{\"m\":\"Defs\",\"n\":\"P.Good\"}}": {"definition": null, "usages": [[2, 29, 2, 33, "P.good_all"]]},
            "{\"c\":{\"m\":\"Defs\",\"n\":\"P.Big\"}}": {"definition": null, "usages": [[3, 20, 3, 23, "P.big_holds"]]},
            "{\"c\":{\"m\":\"Lemmas\",\"n\":\"P.good_all\"}}": {"definition": [2, 8, 2, 16], "usages": [[3, 36, 3, 44, "P.big_holds"]]},
            "{\"c\":{\"m\":\"Lemmas\",\"n\":\"P.big_holds\"}}": {"definition": [3, 8, 3, 17], "usages": []},
            "{\"c\":{\"m\":\"Lemmas\",\"n\":\"P.unused\"}}": {"definition": [4, 8, 4, 14], "usages": []},
            "{\"c\":{\"m\":\"Init.Prelude\",\"n\":\"Nat\"}}": {"definition": null, "usages": [[2, 22, 2, 25, "P.good_all"]]}
        }
    });
    write(&art, "lib/lean/Lemmas.ilean", &ilean.to_string());
    let out = w.run(&["graph", "build", w.dir(), "--artifacts", art.to_str().unwrap()]);
    assert!(out.contains("Lean references, lexical where a file has none"), "{out}");
    assert!(out.contains("1 by the recorded places, 2 read lexically"), "{out}");
    let show = w.run(&["graph", "show", w.dir(), "P.big_holds"]);
    let refers = show.split("referred to by").next().unwrap();
    assert!(refers.contains("S   def   P.Big") && refers.contains("  B thm   P.good_all"), "{show}");
    // The directory is remembered beside the index: status needs no flag.
    let st = w.run(&["graph", "status", w.dir()]);
    assert!(st.contains("artifacts read from"), "{st}");
    // A graph is for the index it was built from.
    write(&w.root, "lean/src/Extra.lean", "def extra := 1\n");
    w.run(&["scan", w.dir()]);
    let (stale, code) = w.run_code(&["graph", "status", w.dir()]);
    assert_eq!(code, 1);
    assert!(stale.contains("built for an earlier state of the index"), "{stale}");
}

#[test]
fn search_ranks_what_the_endpoints_reach_and_says_when_nothing_matches() {
    let w = world();
    w.run(&["graph", "build", w.dir(), "--lexical"]);
    let out = w.run(&["search", w.dir(), "good"]);
    // P.unused does not match; of the matches, the reached ones are not marked.
    assert!(out.lines().any(|l| l.contains("P.good_all") && !l.trim_start().starts_with('·')), "{out}");
    let un = w.run(&["search", w.dir(), "unused"]);
    assert!(un.contains("not reached from the endpoints"), "{un}");
    assert!(un.lines().any(|l| l.trim_start().starts_with('·') && l.contains("P.unused")), "{un}");
    let only = w.run(&["search", w.dir(), "unused", "--reached"]);
    assert!(only.contains("0 name matches"), "{only}");
    // A statement is found by what it says: `key = key` is P.unused's.
    let st = w.run(&["search", w.dir(), "key"]);
    assert!(st.split("documentation/statement matches").nth(1).is_some_and(|t| t.contains("P.unused")), "{st}");
    let none = w.run(&["search", w.dir(), "Stepanov"]);
    assert!(none.contains("0 name matches") || none.contains("no name contains"), "{none}");
}

#[test]
fn a_dossier_is_a_page_about_one_declaration() {
    let w = world();
    w.run(&["graph", "build", w.dir(), "--lexical"]);
    let md = w.run(&["dossier", w.dir(), "P.big_holds", "--from", "https://example.org/blob/abc"]);
    for part in ["# P.big_holds", "## What it states", "def Good (n : Nat) : Prop := n = n", "## Where it stands", "1. `main_thm`", "## What its proof refers to", "`P.good_all`", "## What refers to it", "[lean/src/Lemmas.lean:4](https://example.org/blob/abc/src/Lemmas.lean#L4)"] {
        assert!(md.contains(part), "missing {part:?} in:\n{md}");
    }
}

#[test]
fn two_editions_are_compared_by_name_statement_and_proof() {
    let w = world();
    let other = w.root.parent().unwrap().join("repo2");
    write(&other, "lean/lakefile.toml", "name = \"tree\"\nsrcDir = \"src\"\n");
    write(&other, "lean/src/Defs.lean", &DEFS.replace("n = n", "n + 0 = n"));
    write(&other, "lean/src/Lemmas.lean", &LEMMAS.replace("fun n => good_all n", "fun n =>   good_all   n").replace("theorem unused (key : Nat) : key = key := by\n  have good_all := key\n  rfl\n", "theorem good_all' (n : Nat) : Good n := by simp [Good]\n").replace("theorem good_all (n : Nat) : Good n := rfl", "theorem good_all (n : Nat) : Good n := by simp [Good]"));
    write(&other, "lean/src/Main.lean", MAIN);
    w.run(&["scan", other.to_str().unwrap()]);
    let out = w.run(&["diff", w.dir(), other.to_str().unwrap()]);
    let count = |label: &str| -> String { out.lines().find(|l| l.trim_end().ends_with(label)).map(|l| l.split_whitespace().next().unwrap_or("").to_string()).unwrap_or_default() };
    assert_eq!(count("unchanged"), "4", "{out}"); // P.Big, P.big_holds (reformatted), main_thm, relay
    assert_eq!(count("statement or definition changed"), "1", "{out}"); // P.Good
    assert_eq!(count("proof or body changed, statement the same"), "1", "{out}"); // P.good_all
    assert_eq!(count("removed"), "1", "{out}"); // P.unused
    assert_eq!(count("added"), "1", "{out}"); // P.good_all'
    assert!(out.contains("statement or definition changed (1):") && out.contains("P.Good"), "{out}");
}

#[test]
fn a_reading_model_becomes_a_map_and_glosses_travel_as_text() {
    let w = world();
    let model = serde_json::json!({
        "schema": "riemann-rev-model/1",
        "title": "A reading of the small project",
        "pins": {"mirror": "https://example.org/mirror", "mirror_commit": "abc123"},
        "status": {"headline": "Somebody checked something.", "as_of": "2026-01-01T00:00:00Z"},
        "strata": [{"id": "dest", "title": "The destination", "blurb": "Where it ends."}],
        "tiers": {"checked": {"label": "Kernel-checked"}},
        "components": [{"id": "ends", "stratum": "dest", "n": "1", "title": "The endpoint", "gist": "What is proved.", "tier": "checked", "status": "Replayed once.", "explanation": ["One paragraph."]}],
        "nodes": [
            {"id": "main", "component": "ends", "kind": "export", "title": "The theorem", "contract": "Everything is big.", "idea": "By the lemma.", "declaration": "main_thm"},
            {"id": "big", "component": "ends", "kind": "lemma", "title": "Bigness", "contract": "Big holds.", "declaration": "big_holds"},
            {"id": "gone", "component": "ends", "kind": "lemma", "title": "Not there", "declaration": "no_such_declaration"},
            {"id": "idea", "component": "ends", "kind": "idea", "title": "An idea"}
        ],
        "edges": [{"from": "main", "to": "big", "relation": "calls", "token": "P.big_holds"}],
        "itineraries": [{"title": "Read the end first", "question": "Why is it big?", "nodes": ["main", "big"]}]
    });
    let file = w.root.parent().unwrap().join("model.json");
    std::fs::write(&file, model.to_string()).unwrap();
    let map = w.root.parent().unwrap().join("imported.toml");
    let out = w.run(&["map", "import", w.dir(), "--from", file.to_str().unwrap(), "-o", map.to_str().unwrap()]);
    assert!(out.contains("4 nodes in the model; 2 name a declaration") && out.contains("(1 by the end of the name only)"), "{out}");
    assert!(out.contains("big_holds read as P.big_holds") && out.contains("not in the index, left out: no_such_declaration"), "{out}");
    let text = std::fs::read_to_string(&map).unwrap();
    assert!(text.contains("origin = \"imported\"") && text.contains("Recorded by the reading project (2026-01-01T00:00:00Z): Kernel-checked. Replayed once."), "{text}");
    // It is never overwritten, and it is a map slim accepts.
    let again = w.run(&["map", "import", w.dir(), "--from", file.to_str().unwrap(), "-o", map.to_str().unwrap()]);
    assert!(again.contains("nothing was overwritten"), "{again}");
    let check = w.run(&["map", "check", w.dir(), "--map", map.to_str().unwrap()]);
    // (The steps of the argument count as curated relations too.)
    assert!(check.contains("1 chosen objectives, 2 groups, 2 curated landmarks, 2 curated relations, 1 arguments, 1 shared trails"), "{check}");
    assert!(check.contains("0 error(s)") && check.contains("0 unresolved"), "{check}");
    // With the map, its objective is the endpoint.
    w.run(&["graph", "build", w.dir(), "--lexical"]);
    let st = w.run(&["graph", "status", w.dir(), "--map", map.to_str().unwrap()]);
    assert!(st.contains("endpoints (the objectives of the project map): main_thm"), "{st}");

    // Glosses: taken in from map entries, kept when the reader has one, written out again.
    let theirs = w.root.parent().unwrap().join("theirs.toml");
    std::fs::write(&theirs, "schema = 1\n[project]\nname = \"tree\"\n[[landmark]]\ndecl = \"P.Good\"\npurpose = \"a number equal to itself\"\n[[landmark]]\ndecl = \"P\"\npurpose = \"the P namespace\"\n").unwrap();
    let imp = w.run(&["gloss", "import", w.dir(), theirs.to_str().unwrap()]);
    assert!(imp.contains("2 glosses added, 0 replaced"), "{imp}");
    std::fs::write(&theirs, "schema = 1\n[project]\nname = \"tree\"\n[[landmark]]\ndecl = \"P.Good\"\npurpose = \"another reading\"\n").unwrap();
    let imp = w.run(&["gloss", "import", w.dir(), theirs.to_str().unwrap()]);
    assert!(imp.contains("0 glosses added, 0 replaced, 1 of your own kept"), "{imp}");
    let exp = w.run(&["gloss", "export", w.dir()]);
    assert!(exp.contains("decl = \"P.Good\"\npurpose = \"a number equal to itself\""), "{exp}");
    // Landmark candidates nobody chose, from the graph.
    let init = w.run(&["map", "init", w.dir(), "--landmarks", "--goal", "main_thm"]);
    assert!(init.contains("generated-library-statements") && init.contains("origin = \"generated\""), "{init}");
}

/// A project checked with Comparator restates its theorems in a challenge
/// file, `sorry` for proofs, under the names the solution file proves.
#[test]
fn a_challenge_file_with_sorry_does_not_stand_in_for_the_proofs() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("proj");
    write(&root, "lakefile.toml", "name = \"proj\"\n");
    write(&root, "Statement.lean", "def Main : Prop := ∀ n : Nat, n = n\n");
    write(&root, "Paper.lean", "import Statement\ntheorem paper_main : Main := fun _ => rfl\n");
    write(&root, "Challenge.lean", "import Statement\nnamespace Solution\ntheorem mainTheorem : Main := by\n  sorry\nend Solution\n");
    write(&root, "Solution.lean", "import Statement\nimport Paper\nnamespace Solution\ntheorem mainTheorem : Main := paper_main\nend Solution\n");
    let w = World { root, cache: d.path().join("cache"), data: d.path().join("data"), _d: d };
    w.run(&["scan", w.dir()]);
    let out = w.run(&["graph", "build", w.dir(), "--lexical"]);
    // One endpoint, not two of one name; and it is the proof.
    assert!(out.contains("endpoints (the theorems of the files that nothing imports): Solution.mainTheorem\n"), "{out}");
    assert!(out.contains("reached from the endpoints: 3 of 4 declarations"), "{out}");
    let why = w.run(&["why", w.dir(), "paper_main"]);
    assert!(why.contains("Solution.mainTheorem  [Solution.lean:4]  endpoint"), "{why}");
    // A name given as the goal means the proof too.
    // (Also by the end of the name: one name declared twice is one name.)
    let by_end = w.run(&["why", w.dir(), "paper_main", "--goal", "mainTheorem"]);
    assert!(by_end.contains("endpoints (the goal): Solution.mainTheorem") && by_end.contains("[Solution.lean:4]  endpoint"), "{by_end}");
    let goal = w.run(&["why", w.dir(), "paper_main", "--goal", "Solution.mainTheorem"]);
    assert!(goal.contains("endpoints (the goal): Solution.mainTheorem") && goal.contains("[Solution.lean:4]  endpoint") && goal.contains("paper_main"), "{goal}");
    // The ledger names a theorem that proves the statement, not the restatement.
    let l = w.run(&["ledger", w.dir()]);
    assert!(l.lines().any(|x| x.contains("Main") && x.contains("⊢ paper_main")), "{l}");
    let c = w.run(&["contract", w.dir(), "Solution.mainTheorem"]);
    assert!(c.contains("Solution.lean:4"), "{c}");
}

/// A directory of several Lake packages has no endpoints of its own, and says which packages to open.
#[test]
fn a_directory_of_several_packages_says_to_open_one() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("many");
    // More last files than anyone would mean as endpoints.
    for p in ["a", "b"] {
        write(&root, &format!("{p}/lakefile.toml"), &format!("name = \"{p}\"\n"));
        for k in 0..20 {
            write(&root, &format!("{p}/T{k}.lean"), &format!("theorem t{k}_{p} : True := trivial\n"));
        }
    }
    let w = World { root, cache: d.path().join("cache"), data: d.path().join("data"), _d: d };
    w.run(&["scan", w.dir()]);
    let out = w.run(&["graph", "build", w.dir(), "--lexical"]);
    assert!(out.contains("endpoints: none chosen: this directory holds 2 Lake packages (a, b)"), "{out}");
    // One of them, opened by itself, has its own.
    let one = w.root.join("a");
    w.run(&["scan", one.to_str().unwrap()]);
    let out = w.run(&["graph", "build", one.to_str().unwrap(), "--lexical"]);
    assert!(out.contains("endpoints (the theorems of the files that nothing imports): t0_a"), "{out}");
}

/// A file that changed since it was indexed: no command reads its old
/// extents against its new text, and none of them stops on it.
#[test]
fn a_file_changed_since_the_scan_is_left_out_not_misread() {
    let w = world();
    // Shorter than the offsets the index holds for it.
    write(&w.root, "lean/src/Lemmas.lean", "import Defs\n");
    let out = w.run(&["graph", "build", w.dir(), "--lexical"]);
    assert!(out.contains("1 left out: changed since the index was written"), "{out}");
    assert!(out.contains("1 files left out: the file changed since it was indexed"), "{out}");
    let (c, code) = w.run_code(&["contract", w.dir(), "P.big_holds"]);
    assert!(code == 0 || c.contains("error"), "{c}");
    assert!(!c.contains("panicked"), "{c}");
    let other = w.root.parent().unwrap().join("copy");
    write(&other, "lean/lakefile.toml", "name = \"tree\"\nsrcDir = \"src\"\n");
    write(&other, "lean/src/Defs.lean", DEFS);
    w.run(&["scan", other.to_str().unwrap()]);
    let (d, code) = w.run_code(&["diff", w.dir(), other.to_str().unwrap()]);
    assert_eq!(code, 0, "{d}");
    assert!(!d.contains("panicked") && d.contains("removed"), "{d}");
}

#[test]
fn names_are_found_by_their_end_and_reach_follows_the_goal_given() {
    let w = world();
    w.run(&["graph", "build", w.dir(), "--lexical"]);
    // The end of exactly one name.
    let why = w.run(&["why", w.dir(), "good_all"]);
    assert!(why.contains("P.good_all") && why.contains("a shortest chain"), "{why}");
    // Search marks what the roots in force do not reach: with the goal
    // given, that one is reached and the old endpoint is not.
    let un = w.run(&["search", w.dir(), "unused", "--goal", "P.unused"]);
    assert!(un.lines().any(|l| l.contains("P.unused") && !l.trim_start().starts_with('·')), "{un}");
    let main = w.run(&["search", w.dir(), "main_thm", "--goal", "P.unused"]);
    assert!(main.lines().any(|l| l.trim_start().starts_with('·') && l.contains("main_thm")), "{main}");
    // A gloss on a namespace is a declaration of neither edition.
    let theirs = w.root.parent().unwrap().join("ns.toml");
    std::fs::write(&theirs, "schema = 1\n[project]\nname = \"tree\"\n[[landmark]]\ndecl = \"P\"\npurpose = \"the P namespace\"\n[[landmark]]\ndecl = \"P.Good\"\npurpose = \"good\"\n").unwrap();
    w.run(&["gloss", "import", w.dir(), theirs.to_str().unwrap()]);
    let d = w.run(&["diff", w.dir(), w.dir()]);
    assert!(d.contains("1 stand: unchanged"), "{d}");
    assert!(d.contains("1 name no declaration of either edition"), "{d}");
    assert!(!d.contains("to read again"), "{d}");
}
