//! The semantic-highlight pipeline end to end, without Lean: the real
//! client, process handling, cache and CLI against a stand-in language
//! server (`tests/support/fake_lean_server.py`), and, where the fixture's
//! toolchain and build are present, against the real one.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_slim");
const TC: &str = "leanprover/lean4:v4.33.1";

fn have_python() -> bool {
    Path::new("/usr/bin/python3").exists()
}

fn alive(pid: i32) -> bool {
    slim::process::process_alive(pid)
}

/// A project with (fake) build artifacts and a fake toolchain whose
/// `lake serve` is the stand-in server configured by `cfg`.
struct Fixture {
    dir: tempfile::TempDir,
    root: PathBuf,
    elan: PathBuf,
    cache: PathBuf,
}

fn fixture(files: &[(&str, &str)], cfg: serde_json::Value) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    std::fs::create_dir_all(root.join(".lake/build/lib/lean")).unwrap();
    std::fs::write(root.join("lean-toolchain"), format!("{TC}\n")).unwrap();
    std::fs::write(root.join("lakefile.toml"), "name = \"fakeproj\"\n").unwrap();
    for (rel, text) in files {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        let o = root.join(".lake/build/lib/lean").join(rel.replace(".lean", ".olean"));
        std::fs::create_dir_all(o.parent().unwrap()).unwrap();
        std::fs::write(o, "not really an olean").unwrap();
    }
    let elan = dir.path().join("elan");
    let bin = elan.join("toolchains/leanprover--lean4---v4.33.1/bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/fake_lean_server.py"), bin.join("fake_lean_server.py")).unwrap();
    std::fs::write(bin.join("fake.json"), cfg.to_string()).unwrap();
    std::fs::write(bin.join("lake"), format!("#!/bin/sh\nexec /usr/bin/python3 {}/fake_lean_server.py \"$@\"\n", bin.display())).unwrap();
    std::fs::write(bin.join("lean"), "#!/bin/sh\necho 'Lean (version 4.33.1, fake)'\n").unwrap();
    for f in ["lake", "lean"] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(bin.join(f), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let cache = dir.path().join("cache");
    Fixture { dir, root, elan, cache }
}

impl Fixture {
    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(BIN);
        c.args(args);
        c.env("ELAN_HOME", &self.elan).env("SLIM_CACHE_DIR", &self.cache).env("SLIM_DATA_DIR", self.dir.path().join("data"));
        c.stdin(Stdio::null());
        c
    }
    fn build(&self, extra: &[&str]) -> std::process::Output {
        let mut args = vec!["highlight", "build", self.root.to_str().unwrap(), "--yes", "--json", "--sandbox", "none", "--trust-project", "--cgroup", "none"];
        args.extend_from_slice(extra);
        self.cmd(&args).output().unwrap()
    }
    fn store(&self) -> slim::highlight::store::Store {
        let key = slim::paths::project_key("fakeproj", &self.root.canonicalize().unwrap());
        slim::highlight::store::Store::open_existing(&self.cache.join("projects").join(key).join("highlight.sqlite")).unwrap().expect("cache exists")
    }
}

fn summary(out: &std::process::Output) -> serde_json::Value {
    let s = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(s.lines().last().unwrap_or("")).unwrap_or_else(|_| panic!("no JSON summary:\n{s}\n{}", String::from_utf8_lossy(&out.stderr)))
}

#[test]
fn indexing_through_the_real_client_stores_exact_unicode_ranges() {
    if !have_python() {
        eprintln!("skipping: /usr/bin/python3 not available");
        return;
    }
    // Non-ASCII before the tokens: 𝔽 is 2 UTF-16 units and 4 bytes, α 1 and
    // 2, a tab 1 and 1, e + combining acute 2 and 3.
    let text = "def 𝔽α := 1\n\tdef e\u{301}x := 𝔽α.succ\r\n-- 中文 by x\n";
    for enc in [None, Some("utf-8"), Some("utf-32")] {
        let f = fixture(&[("A.lean", text), ("Sub/B.lean", "import A\ndef b := 2\n"), ("Empty.lean", "")], serde_json::json!({"encoding": enc, "spawn_helper": true}));
        let out = f.build(&[]);
        let s = summary(&out);
        assert_eq!(s["outcome"], "finished", "{s}");
        assert_eq!((s["complete"].as_u64(), s["failed"].as_u64()), (Some(3), Some(0)), "{s}\n{}", String::from_utf8_lossy(&out.stderr));
        // An empty file is complete with no tokens.
        let e = f.store().get_semantic_meta("Empty.lean").unwrap().unwrap();
        assert_eq!((e.status, e.n_tokens, e.n_lines), (slim::highlight::Coverage::Complete, 0, 1));
        assert_eq!(s["lingering_children"], 0);
        assert_eq!(s["shutdown_clean"], true);
        assert!(s["lean_version"].as_str().unwrap().contains("4.33.1"));
        let rec = f.store().get_semantic("A.lean").unwrap().unwrap();
        assert_eq!(rec.meta.position_encoding, enc.unwrap_or("utf-16"));
        assert_eq!(rec.meta.line_endings, "mixed");
        assert_eq!(rec.meta.legend.types[2], "property");
        let blob = slim::highlight::codec::Blob::<slim::highlight::SemTok>::parse(rec.blob.unwrap()).unwrap();
        let c = blob.chunk(0).unwrap();
        let lines: Vec<&str> = text.split('\n').collect();
        let words = |n: usize| -> Vec<(&str, u16)> { c.line(n).iter().map(|t| (&lines[n][t.start as usize..t.end as usize], t.ty)).collect() };
        // Whatever the negotiated units, the stored byte ranges cut out
        // exactly the words.
        assert_eq!(words(0), vec![("def", 0), ("𝔽α", 1)], "{enc:?}");
        // (The stand-in server ends a word at the combining mark.)
        assert_eq!(words(1), vec![("def", 0), ("e", 1), ("x", 1), ("𝔽α", 1), ("succ", 2)], "{enc:?}");
        assert_eq!(words(2), vec![("中文", 1), ("by", 0), ("x", 1)], "{enc:?}");
        // B imports A: one local import recorded.
        assert_eq!(f.store().get_semantic_meta("Sub/B.lean").unwrap().unwrap().deps_count, 1);
    }
}

#[test]
fn stale_imports_time_limits_and_crashes_are_recorded_for_what_they_are() {
    if !have_python() {
        eprintln!("skipping: /usr/bin/python3 not available");
        return;
    }
    let body = "def a := 1\ndef b := 2\ndef c := 3\ndef d := 4\n";
    let f = fixture(
        &[("A.lean", body), ("Crash.lean", body), ("Hang.lean", body), ("Stale.lean", body), ("Z.lean", body)],
        serde_json::json!({"hang_on": "Hang.lean", "crash_on": "Crash.lean", "stale_on": "Stale.lean"}),
    );
    let out = f.build(&["--file-timeout", "1"]);
    let s = summary(&out);
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(s["outcome"], "finished", "{s}\n{err}");
    assert_eq!((s["complete"].as_u64(), s["partial"].as_u64(), s["failed"].as_u64()), (Some(2), Some(1), Some(2)), "{s}\n{err}");
    assert_eq!(s["restarts"], 1, "the crashed server was replaced once");
    assert_eq!(s["lingering_children"], 0);
    let st = f.store();
    let m = |p: &str| st.get_semantic_meta(p).unwrap().unwrap();
    assert!(m("Crash.lean").detail.contains("language server stopped"), "{}", m("Crash.lean").detail);
    assert!(m("Stale.lean").detail.contains("out of date"));
    let hang = m("Hang.lean");
    assert_eq!(hang.status, slim::highlight::Coverage::Partial);
    assert!(hang.detail.contains("time limit of 1 s"), "{}", hang.detail);
    // The tokens of the processed half are there, the rest is not.
    assert!(hang.n_tokens > 0 && hang.n_tokens < m("Z.lean").n_tokens, "{} vs {}", hang.n_tokens, m("Z.lean").n_tokens);
    // `status` reads it back without running anything.
    let out = f.cmd(&["highlight", "status", f.root.to_str().unwrap(), "--failures"]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("2 complete, 1 partial, 2 failed"), "{text}");
    assert!(text.contains("partial  Hang.lean") && text.contains("failed   Stale.lean"), "{text}");
    // A second run does not repeat the known failures; --retry does.
    let s2 = summary(&f.build(&["--file-timeout", "1"]));
    assert_eq!((s2["attempted"].as_u64(), s2["fresh"].as_u64()), (Some(0), Some(5)), "{s2}");
}

#[test]
fn interrupting_the_cli_stops_the_server_tree_and_a_rerun_resumes() {
    if !have_python() {
        eprintln!("skipping: /usr/bin/python3 not available");
        return;
    }
    let files: Vec<(String, String)> = (0..8).map(|i| (format!("F{i}.lean"), format!("def f{i} := {i}\n"))).collect();
    let refs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let f = fixture(&refs, serde_json::json!({"delay_ms": 400, "spawn_helper": true}));
    let mut c = f.cmd(&["highlight", "build", f.root.to_str().unwrap(), "--yes", "--json", "--sandbox", "none", "--trust-project", "--cgroup", "none"]);
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = c.spawn().unwrap();
    let pid = child.id() as i32;
    // Let two or three files finish, then Ctrl-C.
    std::thread::sleep(Duration::from_millis(1500));
    let tree = slim::process::descendants(pid);
    assert!(tree.len() >= 3, "the server and its helper are running: {tree:?}");
    let t = Instant::now();
    unsafe { libc::kill(pid, libc::SIGINT) };
    let out = child.wait_with_output().unwrap();
    let took = t.elapsed();
    assert_eq!(out.status.code(), Some(130), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(took < Duration::from_millis(2500), "cancellation took {took:?}");
    std::thread::sleep(Duration::from_millis(200));
    for p in &tree {
        assert!(!alive(*p), "process {p} of the server tree survived the interrupt");
    }
    let s = summary(&out);
    assert_eq!(s["outcome"], "cancelled");
    assert_eq!(s["lingering_children"], 0);
    let done = s["complete"].as_u64().unwrap();
    assert!((1..8).contains(&done), "{s}");
    // Finished files are kept and not redone.
    let s2 = summary(&f.build(&[]));
    assert_eq!(s2["outcome"], "finished");
    assert_eq!(s2["fresh"].as_u64(), Some(done), "{s2}");
    assert_eq!(s2["complete"].as_u64(), Some(8 - done), "{s2}");
}

#[test]
fn killing_the_indexer_outright_takes_the_server_with_it() {
    if !have_python() {
        eprintln!("skipping: /usr/bin/python3 not available");
        return;
    }
    let f = fixture(&[("A.lean", "def a := 1\n"), ("Hang.lean", "def h := 1\n")], serde_json::json!({"hang_on": "Hang.lean", "spawn_helper": true}));
    let mut c = f.cmd(&["highlight", "build", f.root.to_str().unwrap(), "--yes", "--sandbox", "none", "--trust-project", "--cgroup", "none"]);
    c.stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = c.spawn().unwrap();
    let pid = child.id() as i32;
    std::thread::sleep(Duration::from_millis(1200));
    let tree: Vec<i32> = slim::process::descendants(pid).into_iter().filter(|p| *p != pid).collect();
    assert!(!tree.is_empty(), "server running");
    // A crash of slim itself: no chance to clean up.
    unsafe { libc::kill(pid, libc::SIGKILL) };
    let _ = child.wait();
    let t = Instant::now();
    // The server process is killed by the kernel (PR_SET_PDEATHSIG); its
    // own helper is its responsibility (a real Lean worker exits when the
    // watchdog's pipes close; inside the sandbox the PID namespace ends).
    while alive(tree[0]) && t.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!alive(tree[0]), "the server outlived slim");
    for p in &tree {
        unsafe { libc::kill(*p, libc::SIGKILL) };
    }
    // The interrupted run is visible as such; the finished file is kept.
    let st = f.store();
    assert!(st.last_run().unwrap().unwrap().1.is_none(), "run has no end time");
    assert!(st.get_semantic_meta("A.lean").unwrap().is_some());
}

#[test]
fn sandboxed_server_is_cut_off_and_leaves_nothing_behind() {
    if !have_python() {
        eprintln!("skipping: /usr/bin/python3 not available");
        return;
    }
    if matches!(slim::export::detect_sandbox(), slim::export::Sandbox::None) {
        eprintln!("skipping: bubblewrap with user namespaces is not available here");
        return;
    }
    let f = fixture(&[("A.lean", "def a := 1\n")], serde_json::json!({"spawn_helper": true}));
    let before = std::fs::read_dir(&f.root).unwrap().count();
    let out = f.cmd(&["highlight", "build", f.root.to_str().unwrap(), "--yes", "--json", "--cgroup", "none"]).output().unwrap();
    let s = summary(&out);
    assert_eq!((s["outcome"].as_str(), s["complete"].as_u64(), s["lingering_children"].as_u64()), (Some("finished"), Some(1), Some(0)), "{s}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stderr).contains("bubblewrap"));
    assert_eq!(std::fs::read_dir(&f.root).unwrap().count(), before, "nothing was written into the project");
    // Without the confirmation nothing runs at all.
    let f2 = fixture(&[("A.lean", "def a := 1\n")], serde_json::json!({}));
    let out = f2.cmd(&["highlight", "build", f2.root.to_str().unwrap(), "--cgroup", "none"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("not confirmed"));
    assert!(!f2.cache.join("projects").exists() || std::fs::read_dir(f2.cache.join("projects")).unwrap().flatten().all(|e| !e.path().join("highlight.sqlite").exists()));
}

#[test]
fn clean_removes_only_the_cache() {
    if !have_python() {
        eprintln!("skipping: /usr/bin/python3 not available");
        return;
    }
    let f = fixture(&[("A.lean", "def a := 1\n")], serde_json::json!({}));
    assert_eq!(summary(&f.build(&[]))["complete"], 1);
    let root = f.root.to_str().unwrap();
    let out = f.cmd(&["highlight", "clean", root]).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "needs confirmation");
    assert!(f.store().get_semantic_meta("A.lean").unwrap().is_some());
    let out = f.cmd(&["highlight", "clean", root, "--semantic", "--yes"]).output().unwrap();
    assert!(out.status.success());
    assert!(f.store().get_semantic_meta("A.lean").unwrap().is_none());
    assert!(f.cmd(&["highlight", "clean", root, "--yes"]).output().unwrap().status.success());
    assert!(f.root.join("A.lean").exists());
    let out = f.cmd(&["highlight", "status", root]).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("no highlight cache"));
}

/// The real server, when this checkout has the fixture built for the
/// installed toolchain (see fixtures/demo/README.md); skipped otherwise.
#[test]
fn real_lean_server_on_the_built_fixture() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/demo");
    let tc = slim::export::toolchain_dir(TC);
    if !tc.join("bin/lake").exists() || !root.join(".lake/build/lib/lean/SlimDemo.olean").exists() {
        eprintln!("skipping: needs Lean {TC} installed and fixtures/demo built with it");
        return;
    }
    if matches!(slim::export::detect_sandbox(), slim::export::Sandbox::None) {
        eprintln!("skipping: no sandbox");
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let cache = d.path().join("cache");
    let out = Command::new(BIN)
        .args(["highlight", "build", root.to_str().unwrap(), "--yes", "--json", "--files", "SlimDemo/Graph/Basic.lean", "--files", "SlimDemo/Fixtures/Trust.lean"])
        .env("SLIM_CACHE_DIR", &cache)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let s = summary(&out);
    assert_eq!((s["outcome"].as_str(), s["complete"].as_u64(), s["lingering_children"].as_u64()), (Some("finished"), Some(2), Some(0)), "{s}\n{}", String::from_utf8_lossy(&out.stderr));
    let project = slim::paths::Project::open(&root).unwrap();
    let store = slim::highlight::store::Store::open_existing(&cache.join("projects").join(&project.key).join("highlight.sqlite")).unwrap().unwrap();
    let rec = store.get_semantic("SlimDemo/Graph/Basic.lean").unwrap().unwrap();
    assert_eq!(rec.meta.status, slim::highlight::Coverage::Complete);
    assert_eq!(rec.meta.diag_errors, 0);
    let legend = rec.meta.legend.clone();
    assert!(legend.types.contains(&"keyword".to_string()) && legend.types.contains(&"variable".to_string()));
    // Lean's own classification of a known line: `structure Graph where`.
    let text = std::fs::read_to_string(root.join("SlimDemo/Graph/Basic.lean")).unwrap();
    let lines: Vec<&str> = text.split('\n').collect();
    let n = lines.iter().position(|l| l.starts_with("structure Graph")).unwrap();
    let blob = slim::highlight::codec::Blob::<slim::highlight::SemTok>::parse(rec.blob.unwrap()).unwrap();
    let c = blob.chunk(0).unwrap();
    let toks: Vec<(&str, &str)> = c.line(n).iter().map(|t| (&lines[n][t.start as usize..t.end as usize], legend.types[t.ty as usize].as_str())).collect();
    assert!(toks.contains(&("structure", "keyword")) && toks.contains(&("where", "keyword")), "{toks:?}");
    // Somewhere a bound variable and a projection are marked.
    let all: Vec<&str> = (0..lines.len()).flat_map(|i| c.line(i).iter().map(|t| legend.types[t.ty as usize].as_str()).collect::<Vec<_>>()).collect();
    assert!(all.contains(&"variable") && all.contains(&"property"), "token types seen: {:?}", all.iter().collect::<std::collections::BTreeSet<_>>());
    let env = store.get_env(&rec.meta.env_fingerprint).unwrap().unwrap();
    assert!(env.lean_version.unwrap().contains("4.33.1"));
    // A keyword the project defines itself: the tactic macro
    // `fixture_trust_me`. The grammar cannot know it (plain scope); Lean's
    // semantic tokens mark its use as a keyword.
    let rec = store.get_semantic("SlimDemo/Fixtures/Trust.lean").unwrap().unwrap();
    assert_eq!(rec.meta.status, slim::highlight::Coverage::Complete);
    assert!(rec.meta.detail.contains("processed completely"), "{}", rec.meta.detail);
    let legend = rec.meta.legend.clone();
    let text = std::fs::read_to_string(root.join("SlimDemo/Fixtures/Trust.lean")).unwrap();
    let lines: Vec<&str> = text.split('\n').collect();
    let n = lines.iter().position(|l| l.trim_start().starts_with("fixture_trust_me")).expect("the macro is used");
    let blob = slim::highlight::codec::Blob::<slim::highlight::SemTok>::parse(rec.blob.unwrap()).unwrap();
    let c = blob.chunk(n / slim::highlight::CHUNK_LINES).unwrap();
    let toks: Vec<(&str, &str)> = c.line(n % slim::highlight::CHUNK_LINES).iter().map(|t| (&lines[n][t.start as usize..t.end as usize], legend.types[t.ty as usize].as_str())).collect();
    assert!(toks.contains(&("fixture_trust_me", "keyword")), "{toks:?}");
    let mut b = slim::highlight::baseline::BaselineBuilder::new();
    let mut base = Vec::new();
    for (i, l) in slim::highlight::baseline::lines(&text).enumerate() {
        b.feed(l, &mut base);
        if i == n {
            break;
        }
    }
    let col = lines[n].find("fixture_trust_me").unwrap() as u32;
    let scope = base.iter().rev().find(|t| t.0 <= col).unwrap().1;
    assert_eq!(slim::highlight::textmate::scopes().display(scope), "source.lean4", "the grammar alone leaves it plain");
}

/// slim's baseline provider against VS Code's own tokenizer
/// (vscode-textmate 9.3.2 + vscode-oniguruma 2.0.1, the Lean extension's
/// grammars, VS Code's bundled themes) on the original fixtures: the same
/// scope stack for every byte, and the same resolved colour and font style.
#[test]
fn baseline_matches_the_vscode_reference_on_the_fixtures() {
    use slim::highlight::baseline::{lines, BaselineBuilder};
    use slim::highlight::textmate::scopes;
    use slim::highlight::theme::SyntaxTheme;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    // (start, value) lists with adjacent equal values merged.
    fn merged<T: PartialEq + Clone>(v: impl IntoIterator<Item = (u64, T)>) -> Vec<(u64, T)> {
        let mut out: Vec<(u64, T)> = Vec::new();
        for (s, x) in v {
            if out.last().is_some_and(|l| l.1 == x) {
                continue;
            }
            if out.last().is_some_and(|l| l.0 == s) {
                out.pop();
                if out.last().is_some_and(|l| l.1 == x) {
                    continue;
                }
            }
            out.push((s, x));
        }
        out
    }
    let mut checked_lines = 0;
    for (reference, theme_name, check_scopes) in [("dark_plus", "dark-plus", true), ("light_plus", "light-plus", false), ("hc_black", "hc-black", false)] {
        let theme = SyntaxTheme::builtin(theme_name).unwrap();
        let default_fg = match theme.editor_fg {
            Some(c) => format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]),
            None => "#000000".to_string(),
        };
        let text = std::fs::read_to_string(root.join(format!("fixtures/highlight/reference/{reference}.jsonl"))).unwrap();
        let mut b = BaselineBuilder::new();
        let mut files = 0;
        for l in text.lines() {
            let r: serde_json::Value = serde_json::from_str(l).unwrap();
            let rel = r["path"].as_str().unwrap();
            let raw = std::fs::read(root.join(rel)).unwrap();
            let src = String::from_utf8(raw).unwrap();
            // The reference (like VS Code) does not count a byte-order mark.
            let bom = if src.starts_with('\u{FEFF}') { 3u64 } else { 0 };
            b.reset();
            let ref_lines = r["lines"].as_array().unwrap();
            let ref_styles = r["styles"].as_array().unwrap();
            assert_eq!(lines(&src).count(), ref_lines.len(), "{rel}: line count");
            let mut toks = Vec::new();
            for (n, line) in lines(&src).enumerate() {
                b.feed(line, &mut toks);
                let shift = |s: u32| (s as u64).saturating_sub(if n == 0 { bom } else { 0 });
                if check_scopes {
                    let ours = merged(toks.iter().map(|&(s, id)| (shift(s), scopes().display(id))));
                    let theirs = merged(ref_lines[n].as_array().unwrap().iter().map(|t| (t[0].as_u64().unwrap(), t[1].as_str().unwrap().to_string())));
                    assert_eq!(ours, theirs, "{rel}:{} scopes\n  line: {line:?}", n + 1);
                }
                let ours = merged(toks.iter().map(|&(s, id)| {
                    let st = theme.scope_style(&scopes().path(id));
                    (shift(s), (st.fg.map(|c| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])).unwrap_or_else(|| default_fg.clone()), st.font as u64))
                }));
                let theirs = merged(ref_styles[n].as_array().unwrap().iter().map(|t| (t[0].as_u64().unwrap(), (t[1].as_str().unwrap().to_string(), t[2].as_u64().unwrap()))));
                assert_eq!(ours, theirs, "{rel}:{} styles in {theme_name}\n  line: {line:?}", n + 1);
                checked_lines += 1;
            }
            files += 1;
        }
        assert_eq!(files, 9, "every fixture has a reference");
    }
    assert!(checked_lines > 800);
}
