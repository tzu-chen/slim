//! End-to-end tests of the interactive browser in a pseudo-terminal:
//! idle CPU, clean exit, terminal restoration, signals, and killing/reaping
//! owned child processes when quitting while work is active.

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_slim");

struct Pty {
    child: Child,
    master: std::fs::File,
    out: Arc<Mutex<Vec<u8>>>,
}

fn spawn_in_pty(args: &[&str], env: &[(&str, &Path)], extra_env: &[(&str, &str)]) -> Pty {
    spawn_in_pty_sized(args, env, extra_env, 100, 30)
}

fn spawn_in_pty_sized(args: &[&str], env: &[(&str, &Path)], extra_env: &[(&str, &str)], cols: u16, rows: u16) -> Pty {
    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    let ws = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
    let r = unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), &ws) };
    assert_eq!(r, 0, "openpty failed");
    let slave_fd = unsafe { OwnedFd::from_raw_fd(slave) };
    let mut cmd = Command::new(BIN);
    cmd.args(args);
    cmd.env_clear();
    cmd.env("PATH", "/usr/bin:/bin");
    cmd.env("TERM", "xterm-256color");
    cmd.env("LANG", "C.UTF-8");
    cmd.env("HOME", std::env::var("HOME").unwrap_or_default());
    if let Ok(x) = std::env::var("XDG_RUNTIME_DIR") {
        cmd.env("XDG_RUNTIME_DIR", x);
    }
    if let Ok(x) = std::env::var("DBUS_SESSION_BUS_ADDRESS") {
        cmd.env("DBUS_SESSION_BUS_ADDRESS", x);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let s = slave_fd.as_raw_fd();
    unsafe {
        cmd.stdin(Stdio::from_raw_fd(libc::dup(s)));
        cmd.stdout(Stdio::from_raw_fd(libc::dup(s)));
        cmd.stderr(Stdio::from_raw_fd(libc::dup(s)));
        cmd.pre_exec(move || {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = cmd.spawn().expect("spawn slim in pty");
    drop(slave_fd);
    let master = unsafe { std::fs::File::from_raw_fd(master) };
    let out = Arc::new(Mutex::new(Vec::new()));
    let o2 = Arc::clone(&out);
    let mut reader = master.try_clone().unwrap();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => o2.lock().unwrap().extend_from_slice(&buf[..n]),
            }
        }
    });
    Pty { child, master, out }
}

impl Pty {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.out.lock().unwrap()).to_string()
    }
    fn wait_for(&self, needle: &str, timeout: Duration) -> bool {
        let t = Instant::now();
        while t.elapsed() < timeout {
            if self.text().contains(needle) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }
    /// The screen as a terminal would show it: a minimal emulator for what
    /// the UI emits (cursor positioning, clearing, text; styles are ignored).
    /// The raw stream is not searchable for phrases because only changed
    /// cells are redrawn.
    fn screen(&self, cols: usize, rows: usize) -> String {
        let bytes = self.out.lock().unwrap().clone();
        let text = String::from_utf8_lossy(&bytes);
        let mut grid = vec![vec![' '; cols]; rows];
        let (mut r, mut c) = (0usize, 0usize);
        let mut it = text.chars().peekable();
        while let Some(ch) = it.next() {
            match ch {
                '\x1b' => match it.next() {
                    Some('[') => {
                        let mut params = String::new();
                        let mut fin = ' ';
                        for x in it.by_ref() {
                            if ('\x40'..='\x7e').contains(&x) {
                                fin = x;
                                break;
                            }
                            params.push(x);
                        }
                        match fin {
                            'H' | 'f' => {
                                let mut n = params.split(';').map(|v| v.parse::<usize>().unwrap_or(1));
                                r = n.next().unwrap_or(1).saturating_sub(1).min(rows - 1);
                                c = n.next().unwrap_or(1).saturating_sub(1).min(cols - 1);
                            }
                            'J' => grid.iter_mut().for_each(|l| l.fill(' ')),
                            'h' | 'l' if params == "?1049" => grid.iter_mut().for_each(|l| l.fill(' ')),
                            _ => {}
                        }
                    }
                    Some(']') => {
                        for x in it.by_ref() {
                            if x == '\x07' {
                                break;
                            }
                        }
                    }
                    _ => {}
                },
                '\r' => c = 0,
                '\n' => r = (r + 1).min(rows - 1),
                ch if ch < ' ' => {}
                ch => {
                    if c < cols {
                        grid[r][c] = ch;
                    }
                    c += 1;
                }
            }
        }
        grid.into_iter().map(|l| l.into_iter().collect::<String>()).collect::<Vec<_>>().join("\n")
    }
    /// Waits until the emulated screen shows `needle` on one line.
    fn wait_screen(&self, needle: &str, size: (usize, usize)) -> bool {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(8) {
            if self.screen(size.0, size.1).contains(needle) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        eprintln!("screen without {needle:?}:\n{}", self.screen(size.0, size.1));
        false
    }
    fn send(&mut self, s: &str) {
        self.master.write_all(s.as_bytes()).unwrap();
        self.master.flush().unwrap();
        std::thread::sleep(Duration::from_millis(150));
    }
    fn wait_exit(&mut self, timeout: Duration) -> Option<std::process::ExitStatus> {
        let t = Instant::now();
        while t.elapsed() < timeout {
            if let Some(st) = self.child.try_wait().unwrap() {
                return Some(st);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// utime + stime in clock ticks.
fn cpu_ticks(pid: u32) -> u64 {
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let rest = &s[s.rfind(')').unwrap() + 2..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    f[11].parse::<u64>().unwrap() + f[12].parse::<u64>().unwrap()
}

fn alive(pid: i32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => !s[s.rfind(')').unwrap() + 1..].trim_start().starts_with('Z'),
        Err(_) => false,
    }
}

fn dirs() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let cache = d.path().join("cache");
    let data = d.path().join("data");
    (d, cache, data)
}

#[test]
fn demo_is_idle_when_idle_and_restores_the_terminal() {
    let (_d, cache, data) = dirs();
    let timing = cache.parent().unwrap().join("timing.jsonl");
    let mut p = spawn_in_pty(&["demo"], &[("SLIM_CACHE_DIR", &cache), ("SLIM_DATA_DIR", &data), ("SLIM_TIMING_LOG", &timing)], &[]);
    assert!(p.wait_for("SEMANTIC", Duration::from_secs(20)), "UI did not start:\n{}", p.text());
    assert!(p.wait_for("sources match index", Duration::from_secs(10)), "freshness check did not finish");
    std::thread::sleep(Duration::from_millis(500));
    // Idle: no busy loops or redraw timers.
    let pid = p.child.id();
    let before = cpu_ticks(pid);
    std::thread::sleep(Duration::from_secs(3));
    let used = cpu_ticks(pid) - before;
    assert!(used <= 2, "idle process used {used} clock ticks in 3 s");
    // A short reading session.
    // The demo ships a project map, so it opens on the map; `/` searches.
    assert!(p.wait_screen("Main results", (100, 30)), "the demo opens on its architecture map");
    p.send("/");
    p.send("searchWithin_correct");
    p.send("\r");
    assert!(p.wait_for("placeholder", Duration::from_secs(5)), "detail not shown");
    p.send("3");
    p.send("m");
    p.send("q");
    let st = p.wait_exit(Duration::from_secs(5)).expect("did not exit after q");
    assert!(st.success(), "exit status {st:?}");
    let out = p.text();
    let enter = out.rfind("\x1b[?1049h").expect("entered alternate screen");
    let leave = out.rfind("\x1b[?1049l").expect("left alternate screen");
    assert!(leave > enter);
    assert!(out[leave..].contains("\x1b[?25h") || out.rfind("\x1b[?25h").unwrap() > enter, "cursor shown again");
    // Bookmark persisted outside any project.
    let conn = rusqlite::Connection::open(data.join("userdata.sqlite")).unwrap();
    let n: i64 = conn.query_row("SELECT count(*) FROM bookmarks WHERE name = 'Demo.Fixtures.searchWithin_correct'", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
    let t = std::fs::read_to_string(&timing).unwrap();
    assert!(t.contains("first_frame_ms"));
}

#[test]
fn sigterm_restores_terminal() {
    let (_d, cache, data) = dirs();
    let mut p = spawn_in_pty(&["demo"], &[("SLIM_CACHE_DIR", &cache), ("SLIM_DATA_DIR", &data)], &[]);
    assert!(p.wait_for("SEMANTIC", Duration::from_secs(20)));
    unsafe {
        libc::kill(p.child.id() as i32, libc::SIGTERM);
    }
    let st = p.wait_exit(Duration::from_secs(5)).expect("did not exit on SIGTERM");
    assert!(st.success());
    assert!(p.text().contains("\x1b[?1049l"));
}

#[test]
fn quitting_during_a_large_scan_is_prompt_and_leaves_no_partial_index() {
    let (d, cache, data) = dirs();
    let root = d.path().join("big");
    for i in 0..4000 {
        let p = root.join(format!("M{}/F{i}.lean", i % 40));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let body: String = (0..60).map(|k| format!("theorem t{i}_{k} : True := trivial\n")).collect();
        std::fs::write(p, body).unwrap();
    }
    let mut p = spawn_in_pty(&[root.to_str().unwrap(), "--threads", "1"], &[("SLIM_CACHE_DIR", &cache), ("SLIM_DATA_DIR", &data)], &[]);
    assert!(p.wait_for("Building the source-only index", Duration::from_secs(10)), "{}", p.text());
    let t = Instant::now();
    p.send("q");
    let st = p.wait_exit(Duration::from_secs(5)).expect("did not exit while scanning");
    assert!(st.success());
    assert!(t.elapsed() < Duration::from_secs(4), "exit took {:?}", t.elapsed());
    assert!(p.text().contains("\x1b[?1049l"));
    // No staging file left behind.
    let leftovers: Vec<PathBuf> = walk(&cache).into_iter().filter(|p| p.to_string_lossy().ends_with(".building")).collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

fn walk(d: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(d) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                v.extend(walk(&p));
            } else {
                v.push(p);
            }
        }
    }
    v
}

#[test]
fn quitting_during_semantic_export_kills_and_reaps_the_process_tree() {
    let (d, cache, data) = dirs();
    // A project with (fake) build artifacts.
    let root = d.path().join("proj");
    std::fs::create_dir_all(root.join(".lake/build/lib/lean")).unwrap();
    std::fs::write(root.join("lean-toolchain"), "leanprover/lean4:v4.33.1\n").unwrap();
    std::fs::write(root.join("lakefile.toml"), "name = \"fakeproj\"\n").unwrap();
    std::fs::write(root.join("Fake.lean"), "theorem fake : True := trivial\n").unwrap();
    std::fs::write(root.join(".lake/build/lib/lean/Fake.olean"), "not really an olean").unwrap();
    // A fake toolchain whose `lake build` starts a grandchild and hangs.
    let elan = d.path().join("elan");
    let bin = elan.join("toolchains/leanprover--lean4---v4.33.1/bin");
    std::fs::create_dir_all(&bin).unwrap();
    let pids = d.path().join("pids");
    std::fs::create_dir_all(&pids).unwrap();
    let lake = format!(
        "#!/bin/sh\necho $$ > {p}/lake.pid\nsleep 120 &\necho $! > {p}/grandchild.pid\nwait\n",
        p = pids.display()
    );
    std::fs::write(bin.join("lake"), lake).unwrap();
    std::fs::write(bin.join("lean"), "#!/bin/sh\nexit 0\n").unwrap();
    for f in ["lake", "lean"] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(bin.join(f), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut p = spawn_in_pty(
        &[root.to_str().unwrap()],
        &[("SLIM_CACHE_DIR", &cache), ("SLIM_DATA_DIR", &data), ("ELAN_HOME", &elan)],
        &[],
    );
    assert!(p.wait_for("SOURCE-ONLY", Duration::from_secs(20)), "{}", p.text());
    assert!(p.wait_for("Outline", Duration::from_secs(20)));
    std::thread::sleep(Duration::from_millis(800));
    p.send("\x1b");
    p.send(":");
    p.send("semantic export");
    p.send("\r");
    if !p.wait_for("explicit trust decision", Duration::from_secs(10)) {
        let t = p.text();
        if t.contains("no usable sandbox") {
            eprintln!("skipping: bubblewrap with user namespaces is not available here");
            return;
        }
        panic!("no confirmation dialog:\n{}", &t[t.len().saturating_sub(4000)..]);
    }
    p.send("yes");
    p.send("\r");
    let t = Instant::now();
    while !(pids.join("lake.pid").exists() && pids.join("grandchild.pid").exists()) {
        assert!(t.elapsed() < Duration::from_secs(10), "fake lake never started:\n{}", p.text());
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(200));
    let lake_pid: i32 = std::fs::read_to_string(pids.join("lake.pid")).unwrap().trim().parse().unwrap();
    let gc_pid: i32 = std::fs::read_to_string(pids.join("grandchild.pid")).unwrap().trim().parse().unwrap();
    assert!(alive(lake_pid) && alive(gc_pid));
    p.send("q");
    let st = p.wait_exit(Duration::from_secs(8)).expect("did not exit while the export ran");
    assert!(st.success(), "{st:?}");
    std::thread::sleep(Duration::from_millis(300));
    assert!(!alive(lake_pid), "owned child survived quit");
    assert!(!alive(gc_pid), "grandchild survived quit");
    assert!(p.text().contains("\x1b[?1049l"));
}

/// A keyboard-only reading session over the bundled map: find the stated
/// objective, pin it as the goal, follow the curated argument, save a stop,
/// quit — and find goal, trail and view again after a restart.
#[test]
fn map_goal_and_trail_session_survives_a_restart() {
    let (_d, cache, data) = dirs();
    let env: [(&str, &Path); 2] = [("SLIM_CACHE_DIR", &cache), ("SLIM_DATA_DIR", &data)];
    let sz = (100, 30);
    let mut p = spawn_in_pty(&["demo", "--ascii"], &env, &[]);
    assert!(p.wait_screen("v Main results", sz), "the demo opens on its architecture map");
    assert!(p.wait_screen("v Unresolved references", sz));
    assert!(p.wait_screen("goal: none", sz));
    p.send("j"); // the first objective
    assert!(p.wait_screen("Soundness of the fuel-bounded", sz));
    assert!(p.wait_screen("Statement (elaborated)", sz), "the detail pane shows the formal statement");
    assert!(p.wait_screen("theorem Demo.Graph.searchWithin_sound", sz));
    assert!(p.wait_screen("annotation: authored", sz));
    p.send("*");
    assert!(p.wait_screen("goal pinned", sz));
    p.send("A");
    assert!(p.wait_screen("relation: Curated argument", sz));
    assert!(p.wait_screen("[cur 2] thm Graph.anyOf_eq_true", sz), "curated steps carry their label");
    p.send("r");
    p.send("r");
    assert!(p.wait_screen("relation: Lean dependencies", sz));
    assert!(p.wait_screen("generated (H)", sz), "hidden generated dependencies are counted");
    assert!(p.wait_screen("direction: uses", sz));
    assert!(p.wait_screen("[T+V] def Graph.Reach", sz), "Lean edges say statement and/or proof");
    p.send("r");
    p.send("r"); // back to the curated argument
    assert!(p.wait_screen("relation: Curated argument", sz));
    assert!(p.wait_screen("[cur 2] thm Graph.anyOf_eq_true", sz));
    p.send("j");
    p.send("j"); // anyOf_eq_true
    assert!(p.wait_screen("Path from the goal", sz), "the detail keeps the path the reader came by");
    assert!(p.wait_screen("Why this edge is shown", sz));
    p.send("a");
    assert!(p.wait_screen("why is it on the route", sz));
    p.send("the witness step");
    p.send("\r");
    assert!(p.wait_screen("added Demo.Graph.anyOf_eq_true to the trail", sz));
    // Open it in the declaration browser and come back: the goal stays.
    p.send("\r");
    assert!(p.wait_screen("Declaration", sz));
    assert!(p.wait_screen("(goal) goal: Graph.searchWithin_sound", sz), "the goal is pinned while browsing");
    p.send("\x7f");
    assert!(p.wait_screen("relation: Curated argument", sz), "Backspace returns to the goal explorer");
    p.send("T");
    assert!(p.wait_screen("Your trails (1)", sz));
    assert!(p.wait_screen("Shared trails in the project map (1)", sz));
    p.send("?");
    assert!(p.wait_screen("MOVING", sz) && p.wait_screen("VIEWS", sz), "help lists the views");
    p.send("q"); // closes the help
    p.send("q");
    let st = p.wait_exit(Duration::from_secs(5)).expect("did not exit after q");
    assert!(st.success());
    drop(p);
    // Restart: the trail view, the goal and the stop with its note are back.
    let mut p = spawn_in_pty(&["demo", "--ascii"], &env, &[]);
    assert!(p.wait_screen("Your trails (1)", sz), "resumes in the trail view");
    assert!(p.wait_screen("goal: Demo.Graph.searchWithin_sound", sz), "the pinned goal was restored");
    p.send("\r"); // expand the trail
    p.send("j"); // its stop: the note is in the detail pane
    assert!(p.wait_screen("why it is on the route:", sz));
    assert!(p.wait_screen("the witness step", sz));
    p.send("q");
    assert!(p.wait_exit(Duration::from_secs(5)).expect("did not exit").success());
    // And from the command line, without the UI.
    let out = Command::new(BIN).args(["trail", "export", "--project", "slim_demo"]).env("SLIM_DATA_DIR", &data).env("SLIM_CACHE_DIR", &cache).output().unwrap();
    let md = String::from_utf8_lossy(&out.stdout);
    assert!(md.contains("**`Demo.Graph.anyOf_eq_true`**") && md.contains("> the witness step"), "{md}");
}

/// Narrow terminals show one pane at a time; a malformed map never blocks
/// the browser and is never rewritten.
#[test]
fn narrow_terminal_and_malformed_map_stay_usable() {
    let (d, cache, data) = dirs();
    let proj = d.path().join("proj");
    std::fs::create_dir_all(proj.join("P")).unwrap();
    std::fs::write(proj.join("lakefile.toml"), "name = \"p\"\n").unwrap();
    std::fs::write(proj.join("P/A.lean"), "namespace P\n/-- See Lemma 3.9. -/\ntheorem main : True := trivial\nend P\n").unwrap();
    let broken = "schema = 1\n[[group]]\nid = \"a\"\npurpos = \"typo\"\n";
    std::fs::write(proj.join("lean-view.toml"), broken).unwrap();
    let env: [(&str, &Path); 2] = [("SLIM_CACHE_DIR", &cache), ("SLIM_DATA_DIR", &data)];
    let sz = (60, 24);
    let mut p = spawn_in_pty_sized(&[proj.to_str().unwrap(), "--ascii"], &env, &[], 60, 24);
    assert!(p.wait_screen("SOURCE-ONLY", sz));
    assert!(p.wait_screen("Outline [", sz), "the outline is the first thing on the left");
    // The source browser works although the map file is broken.
    p.send("/");
    assert!(p.wait_screen("names ready", sz));
    p.send("main");
    assert!(p.wait_screen("thm   P.main", sz));
    p.send("\r");
    assert!(p.wait_screen("theorem P.main", sz));
    // The map view lists the field error; the file is untouched.
    p.send("M");
    assert!(p.wait_screen("Map file could not be used", sz));
    assert!(p.wait_screen("purpos", sz));
    assert_eq!(std::fs::read_to_string(proj.join("lean-view.toml")).unwrap(), broken);
    // Fix the file by hand and reload: single-pane map, Tab shows the detail.
    let fixed = "schema = 1\n[[objective]]\ndecl = \"P.main\"\ntitle = \"The main fact\"\n[[group]]\nid = \"a\"\ntitle = \"Alpha\"\nlandmarks = [\"P.main\"]\n";
    std::fs::write(proj.join("lean-view.toml"), fixed).unwrap();
    p.send("r");
    assert!(p.wait_screen("The main fact", sz));
    assert!(p.wait_screen("Architecture map", sz));
    assert!(!p.screen(60, 24).contains(" Detail "), "one pane at a time at 60 columns");
    p.send("j");
    p.send("\t");
    assert!(p.wait_screen(" Detail ", sz), "Tab shows the detail pane");
    assert!(p.wait_screen("Statement (source text, not elaborated)", sz));
    assert!(p.wait_screen("not annotated", sz));
    p.send(" "); // page down in the detail
    assert!(p.wait_screen("\"Lemma 3.9\"", sz), "candidate paper reference shown literally");
    p.send("\t");
    // A Lean-only relation kind is explained, not skipped.
    p.send("*");
    p.send("A");
    assert!(p.wait_screen("relation: Curated argument", sz));
    p.send("r");
    p.send("r");
    assert!(p.wait_screen("relation: Lean dependencies", sz));
    assert!(p.wait_screen("Lean dependencies are unavailable", sz));
    p.send("q");
    assert!(p.wait_exit(Duration::from_secs(5)).expect("did not exit").success());
    assert_eq!(std::fs::read_to_string(proj.join("lean-view.toml")).unwrap(), fixed, "slim never rewrites the map");
}

fn copy_demo_sources(to: &Path) {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap().flatten() {
            let name = e.file_name();
            let s = name.to_string_lossy().to_string();
            if s.starts_with('.') || s == "snapshot" || s == "axiom-report" || s == "lean-view.toml" {
                continue;
            }
            if e.path().is_dir() {
                copy(&e.path(), &to.join(&name));
            } else {
                std::fs::copy(e.path(), to.join(&name)).unwrap();
            }
        }
    }
    copy(&Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/demo"), to);
}

/// The overview features in one keyboard session on a source-only project:
/// outline columns, skim, search within a directory, import layers with an
/// import path, and the Mentions highlight. Then the same project after a
/// restart.
#[test]
fn overview_session_on_a_source_only_project() {
    let (d, cache, data) = dirs();
    let proj = d.path().join("proj");
    copy_demo_sources(&proj);
    let env: [(&str, &Path); 2] = [("SLIM_CACHE_DIR", &cache), ("SLIM_DATA_DIR", &data)];
    let sz = (120, 34);
    let mut p = spawn_in_pty_sized(&[proj.to_str().unwrap(), "--ascii"], &env, &[], 120, 34);
    assert!(p.wait_screen("SOURCE-ONLY", sz));
    // It opens on the outline; the right column is the quantity it is ordered by.
    assert!(p.wait_screen("Outline [declarations]", sz));
    assert!(p.wait_screen("> SlimDemo", sz));
    p.send("o");
    p.send("o");
    p.send("o");
    assert!(p.wait_screen("Outline [proof lines]", sz));
    assert!(p.wait_screen("outline: most proof lines first", sz));
    p.send("o");
    assert!(p.wait_screen("Outline [sorry tokens]", sz));
    p.send("o");
    p.send("o");
    assert!(p.wait_screen("Outline [declarations]", sz));
    // Skim the whole library from its directory row.
    p.send("gg"); // no such binding: must not disturb the selection
    p.send("\x1b");
    let top = p.screen(120, 34);
    let first_is_lib = top.lines().nth(3).is_some_and(|l| l.contains("SlimDemo"));
    if !first_is_lib {
        p.send("j");
    }
    p.send("S");
    assert!(p.wait_screen("73 statements | source text", sz), "skim of the selected directory");
    assert!(p.wait_screen("8 files | 38 thm", sz));
    assert!(p.wait_screen("> theorem", sz) || p.wait_screen("> def", sz), "a statement is selected");
    // Open the selected statement and come back to the skim.
    p.send("\r");
    assert!(p.wait_screen(" Declaration ", sz));
    assert!(p.wait_screen("Statement (source text", sz));
    p.send("\x7f");
    assert!(p.wait_screen("73 statements | source text", sz), "Backspace returns to the skim");
    p.send("\x1b");
    assert!(p.wait_screen(" Declaration ", sz), "Esc closes the skim");
    // Search within the directory selected in the outline.
    p.send("O");
    p.send("f");
    assert!(p.wait_screen("in files SlimDemo", sz));
    assert!(p.wait_screen("in scope", sz), "an empty query lists the scope");
    p.send("searchWithin_sound");
    assert!(p.wait_screen("1 name match", sz));
    p.send("\r");
    assert!(p.wait_screen("theorem searchWithin_sound", sz));
    // Mentions: moving in the list says where the identifier is written.
    p.send("1");
    assert!(p.wait_screen("Mentions (lexical", sz));
    p.send("j");
    p.send("j");
    // (That the cells of the identifier are painted is a render test.)
    assert!(p.wait_screen("lexical match with", sz), "the selected mention says where it is written");
    // Import layers, from the open declaration: its file is selected.
    p.send("I");
    assert!(p.wait_screen("Layers by import depth", sz));
    assert!(p.wait_screen("9 files, depth 0-6", sz));
    assert!(p.wait_screen("file SlimDemo.Graph.Search", sz), "I shows where the current file sits");
    assert!(p.wait_screen("Depth 3", sz));
    assert!(p.wait_screen("Endpoints", sz) && p.wait_screen("Hubs", sz));
    assert!(p.wait_screen("build order, not mathematics", sz));
    // Why does it import Relation.Basic?
    p.send("p");
    assert!(p.wait_screen("Why does SlimDemo.Graph.Search import", sz));
    p.send("Relation.Basic\r");
    assert!(p.wait_screen("v Import path", sz));
    assert!(p.wait_screen("3 steps", sz));
    assert!(p.wait_screen("4. SlimDemo.Relation.Basic", sz));
    p.send("?");
    assert!(p.wait_screen("MOVING", sz));
    // The help is long: page down to the section about these keys.
    let mut found = false;
    for _ in 0..8 {
        if p.screen(120, 34).contains("THE BIG PICTURE") {
            found = true;
            break;
        }
        p.send(" ");
    }
    assert!(found, "help explains the overview keys:\n{}", p.screen(120, 34));
    p.send("q");
    p.send("q");
    assert!(p.wait_exit(Duration::from_secs(5)).expect("did not exit after q").success());
    drop(p);
    // Restart: the import view and the search scope are back.
    let mut p = spawn_in_pty_sized(&[proj.to_str().unwrap(), "--ascii"], &env, &[], 120, 34);
    assert!(p.wait_screen("Layers by import depth", sz), "resumes in the import view, with the graph read again");
    assert!(p.wait_screen("9 files, depth 0-6", sz));
    p.send("/");
    assert!(p.wait_screen("in files SlimDemo", sz), "the search scope was restored");
    p.send("\x1b");
    p.send("F");
    assert!(p.wait_screen("search scope cleared", sz));
    p.send("q");
    assert!(p.wait_exit(Duration::from_secs(5)).expect("did not exit").success());
    // The same answers without the UI.
    let run = |args: &[&str]| -> String {
        let out = Command::new(BIN).args(args).env("SLIM_DATA_DIR", &data).env("SLIM_CACHE_DIR", &cache).output().unwrap();
        String::from_utf8_lossy(&out.stdout).to_string()
    };
    let dir = proj.to_str().unwrap();
    let out = run(&["imports", dir, "SlimDemo.Graph.Search", "Relation.Basic"]);
    assert!(out.contains("3 import lines from SlimDemo.Graph.Search to SlimDemo.Relation.Basic"), "{out}");
    let out = run(&["imports", dir]);
    assert!(out.contains("endpoints: 3 files") && out.contains("SlimDemo.Fixtures.Trust"), "{out}");
    let out = run(&["skim", dir, "SlimDemo/Graph/Search.lean"]);
    assert!(out.contains("theorem searchWithin_sound") && !out.contains("subst hst") && out.contains("lines folded"), "{out}");
    let out = run(&["search", dir, "trans", "--within", "SlimDemo.Graph"]);
    assert!(out.contains("within files SlimDemo.Graph") && out.contains("Reach.trans") && !out.contains("ReflTransGen.trans"), "{out}");
}

/// An index written with another index format is never read as if it were
/// current: a source index is rebuilt on opening; a semantic one is
/// imported again from its cached snapshot, or the reader is told.
#[test]
fn indexes_of_another_format_are_rebuilt_or_explained() {
    let (d, cache, data) = dirs();
    let proj = d.path().join("proj");
    copy_demo_sources(&proj);
    let env: [(&str, &Path); 2] = [("SLIM_CACHE_DIR", &cache), ("SLIM_DATA_DIR", &data)];
    let sz = (120, 30);
    let cli = |args: &[&str]| -> String {
        let out = Command::new(BIN).args(args).env("SLIM_DATA_DIR", &data).env("SLIM_CACHE_DIR", &cache).output().unwrap();
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
    };
    let dir = proj.to_str().unwrap();
    let degrade = |idx: &Path| {
        let c = rusqlite::Connection::open(idx).unwrap();
        c.execute("UPDATE meta SET value = '1' WHERE key = 'schema_version'", []).unwrap();
    };
    let index_path = |out: &str| -> PathBuf { PathBuf::from(out.lines().find_map(|l| l.strip_prefix("index").map(|p| p.trim().to_string())).unwrap_or_else(|| panic!("no index path in:\n{out}"))) };
    // Source-only: opening rebuilds it.
    let src = index_path(&cli(&["scan", dir]));
    assert!(src.ends_with("source.sqlite"));
    degrade(&src);
    assert!(cli(&["info", dir]).contains("index format 1"), "the old index is refused, not misread");
    let mut p = spawn_in_pty_sized(&[dir, "--ascii"], &env, &[], 120, 30);
    assert!(p.wait_screen("SOURCE-ONLY", sz));
    assert!(p.wait_screen("> SlimDemo", sz), "the source index was rebuilt on opening");
    p.send("\x1bq");
    assert!(p.wait_exit(Duration::from_secs(5)).expect("did not exit").success());
    drop(p);
    assert!(cli(&["info", dir]).contains("mode: source"));
    // Semantic, with the snapshot it came from still in the cache: imported again.
    let snap = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/demo/snapshot/slim_demo.lean-v4.33.1.jsonl");
    let sem = index_path(&cli(&["import", snap.to_str().unwrap(), "--source-root", dir]));
    assert!(sem.ends_with("semantic.sqlite"));
    std::fs::copy(&snap, sem.with_file_name("snapshot.jsonl")).unwrap();
    degrade(&sem);
    let mut p = spawn_in_pty_sized(&[dir, "--ascii"], &env, &[], 120, 30);
    assert!(p.wait_screen("SEMANTIC snapshot", sz), "the semantic index was imported again");
    assert!(p.wait_screen("rebuilt from its cached snapshot", sz));
    p.send("\x1bq");
    assert!(p.wait_exit(Duration::from_secs(5)).expect("did not exit").success());
    drop(p);
    assert!(cli(&["info", dir]).contains("mode: semantic"));
    // Without the snapshot: the source-only index is shown and the reader is told why.
    std::fs::remove_file(sem.with_file_name("snapshot.jsonl")).unwrap();
    degrade(&sem);
    let mut p = spawn_in_pty_sized(&[dir, "--ascii"], &env, &[], 120, 30);
    assert!(p.wait_screen("SOURCE-ONLY", sz));
    assert!(p.wait_screen("the semantic index cannot be used", sz));
    p.send("\x1bq");
    assert!(p.wait_exit(Duration::from_secs(5)).expect("did not exit").success());
}

/// Processes below `pid` (not `pid` itself).
fn children_of(pid: u32) -> Vec<i32> {
    slim::process::descendants(pid as i32).into_iter().filter(|p| *p != pid as i32).collect()
}

/// Syntax colours come from the text alone: opening a source file starts
/// no process, colours it with the VS Code grammar and theme, stays idle
/// afterwards, and a theme change needs no new tokens.
#[test]
fn source_view_is_coloured_without_starting_any_process() {
    let (_d, cache, data) = dirs();
    let mut p = spawn_in_pty_sized(
        &["demo"],
        &[("SLIM_CACHE_DIR", &cache), ("SLIM_DATA_DIR", &data)],
        &[("COLORTERM", "truecolor")],
        120,
        34,
    );
    assert!(p.wait_for("sources match index", Duration::from_secs(20)), "UI did not start:\n{}", p.text());
    p.send("/");
    p.send("searchWithin_correct");
    p.send("\r");
    assert!(p.wait_for("placeholder", Duration::from_secs(5)), "detail not shown");
    p.send("s");
    assert!(p.wait_screen("Fixtures/Trust.lean │ syntax ", (120, 34)), "the title says what the colours are");
    // Comment green, and the `sorry` of the fixture in the theme's error red.
    assert!(p.wait_for("38;2;106;153;85", Duration::from_secs(5)));
    assert!(p.text().contains("38;2;244;71;71"), "invalid.illegal colour for sorry");
    let pid = p.child.id();
    assert!(children_of(pid).is_empty(), "browsing started a process: {:?}", children_of(pid));
    // Idle after colouring: no polling loop.
    std::thread::sleep(Duration::from_millis(600));
    let before = cpu_ticks(pid);
    std::thread::sleep(Duration::from_secs(3));
    let used = cpu_ticks(pid) - before;
    assert!(used <= 2, "idle highlighted view used {used} clock ticks in 3 s");
    // Theme change: F5 twice reaches Light+ (comment green #008000).
    p.send("\x1b[15~");
    p.send("\x1b[15~");
    assert!(p.wait_for("38;2;0;128;0", Duration::from_secs(5)), "Light+ colours after F5");
    assert!(children_of(pid).is_empty());
    // F6: the facts behind the colours.
    p.send("\x1b[17~");
    assert!(p.wait_screen("Semantic tokens: none cached for this file", (120, 34)));
    assert!(p.wait_screen("Browsing never starts Lean", (120, 34)));
    p.send("\x1b");
    // F4 twice: syntax only, then off.
    p.send("\x1b[14~");
    p.send("\x1b[14~");
    assert!(p.wait_screen("│ plain", (120, 34)), "highlighting off is labelled");
    p.send("q");
    let st = p.wait_exit(Duration::from_secs(5)).expect("did not exit after q");
    assert!(st.success());
}

/// The explicit job from the browser: plan, typed confirmation, progress,
/// cancellation that leaves no process behind, finished files coloured
/// from the cache, and a clean quit while it runs.
#[test]
fn semantic_indexing_from_the_browser_cancels_and_quits_cleanly() {
    if !Path::new("/usr/bin/python3").exists() {
        eprintln!("skipping: /usr/bin/python3 not available");
        return;
    }
    let (d, cache, data) = dirs();
    let root = d.path().join("proj");
    std::fs::create_dir_all(root.join(".lake/build/lib/lean")).unwrap();
    std::fs::write(root.join("lean-toolchain"), "leanprover/lean4:v4.33.1\n").unwrap();
    std::fs::write(root.join("lakefile.toml"), "name = \"fakeproj\"\n").unwrap();
    for i in 0..6 {
        std::fs::write(root.join(format!("F{i}.lean")), format!("theorem thm{i} (x : Nat) : x = x := by\n  rfl\n")).unwrap();
        std::fs::write(root.join(format!(".lake/build/lib/lean/F{i}.olean")), "not really an olean").unwrap();
    }
    let elan = d.path().join("elan");
    let bin = elan.join("toolchains/leanprover--lean4---v4.33.1/bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/fake_lean_server.py"), bin.join("fake_lean_server.py")).unwrap();
    std::fs::write(bin.join("fake.json"), r#"{"delay_ms": 700, "spawn_helper": true}"#).unwrap();
    std::fs::write(bin.join("lake"), format!("#!/bin/sh\nexec /usr/bin/python3 {}/fake_lean_server.py \"$@\"\n", bin.display())).unwrap();
    std::fs::write(bin.join("lean"), "#!/bin/sh\necho 'Lean (version 4.33.1, fake)'\n").unwrap();
    for f in ["lake", "lean"] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(bin.join(f), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let size = (130, 36);
    let start = |keys: &dyn Fn(&mut Pty)| -> Option<Pty> {
        let mut p = spawn_in_pty_sized(
            &[root.to_str().unwrap()],
            &[("SLIM_CACHE_DIR", &cache), ("SLIM_DATA_DIR", &data), ("ELAN_HOME", &elan)],
            &[("COLORTERM", "truecolor")],
            size.0 as u16,
            size.1 as u16,
        );
        assert!(p.wait_for("Outline", Duration::from_secs(20)), "{}", p.text());
        std::thread::sleep(Duration::from_millis(600));
        assert!(children_of(p.child.id()).is_empty(), "opening the project started a process");
        p.send("\x1b");
        p.send(":");
        p.send("highlighting: build");
        p.send("\r");
        if !p.wait_for("Semantic highlighting: explicit trust decision", Duration::from_secs(10)) {
            let t = p.text();
            if t.contains("no usable sandbox") {
                eprintln!("skipping: bubblewrap with user namespaces is not available here");
                return None;
            }
            panic!("no confirmation dialog:\n{}", &t[t.len().saturating_sub(3000)..]);
        }
        // Nothing runs before the answer.
        assert!(children_of(p.child.id()).is_empty(), "something ran before the confirmation");
        keys(&mut p);
        Some(p)
    };
    // Declined: nothing runs.
    let Some(mut p) = start(&|p| {
        p.send("no");
        p.send("\r");
    }) else {
        return;
    };
    assert!(p.wait_screen("not started", size));
    std::thread::sleep(Duration::from_millis(300));
    assert!(children_of(p.child.id()).is_empty());
    p.send("q");
    assert!(p.wait_exit(Duration::from_secs(5)).is_some());

    // Confirmed, then cancelled from the palette after the first files.
    let mut p = start(&|p| {
        p.send("yes");
        p.send("\r");
    })
    .unwrap();
    let pid = p.child.id();
    assert!(p.wait_screen("semantic index 2/6", size), "progress is shown in the title bar");
    let tree = children_of(pid);
    assert!(tree.len() >= 2, "server processes are running: {tree:?}");
    // Browsing goes on meanwhile.
    p.send("/");
    p.send("thm0");
    p.send("\r");
    assert!(p.wait_screen("theorem thm0", size));
    p.send("\x1b");
    // Pause: the title says so and no further file is opened; resume.
    p.send(":");
    p.send("pause / resume indexing");
    p.send("\r");
    assert!(p.wait_screen("PAUSED", size), "pause is shown");
    p.send("\x1b[17~");
    assert!(p.wait_screen("Files: ", size) && p.wait_screen("PAUSED", size), "the details show the paused job");
    p.send("\x1b");
    p.send(":");
    p.send("pause / resume indexing");
    p.send("\r");
    assert!(p.wait_screen("semantic indexing resumed", size));
    p.send(":");
    p.send("cancel indexing");
    p.send("\r");
    let t = Instant::now();
    assert!(p.wait_screen("semantic indexing cancelled", size), "cancellation is reported");
    assert!(t.elapsed() < Duration::from_secs(4), "cancellation took {:?}", t.elapsed());
    std::thread::sleep(Duration::from_millis(300));
    for q in &tree {
        assert!(!alive(*q), "server process {q} survived the cancellation");
    }
    assert!(children_of(pid).is_empty());
    // A finished file is now shown with its cached semantic tokens: the
    // stand-in server marks `x` a variable (Dark+ #9CDCFE).
    p.send("s");
    assert!(p.wait_screen("│ syntax + semantic ", size));
    assert!(p.wait_for("38;2;156;220;254", Duration::from_secs(5)), "semantic variable colour");
    assert!(children_of(pid).is_empty(), "showing cached tokens starts nothing");
    p.send("\x1b[17~");
    assert!(p.wait_screen("Source alignment: cached token positions were computed for exactly these bytes", size));
    assert!(p.wait_screen("Extraction coverage: complete", size));
    p.send("\x1b");
    // Refresh only the file on screen: the plan names one file.
    p.send(":");
    p.send("refresh this file");
    p.send("\r");
    assert!(p.wait_screen("1 Lean files", size), "the plan is for the file on screen only");
    p.send("yes");
    p.send("\r");
    assert!(p.wait_screen("semantic indexing finished: 1 complete", size), "one file was redone");
    std::thread::sleep(Duration::from_millis(300));
    assert!(children_of(pid).is_empty(), "the finished job left a process");
    assert!(p.wait_screen("│ syntax + semantic ", size), "the view keeps its colours after the refresh");
    p.send("q");
    assert!(p.wait_exit(Duration::from_secs(5)).is_some());

    // Started again (it resumes), and the browser quits while it runs.
    let mut p = start(&|p| {
        p.send("yes");
        p.send("\r");
    })
    .unwrap();
    let pid = p.child.id();
    assert!(p.wait_screen("semantic index", size));
    let t = Instant::now();
    while children_of(pid).len() < 2 {
        assert!(t.elapsed() < Duration::from_secs(10), "server never started");
        std::thread::sleep(Duration::from_millis(50));
    }
    let tree = children_of(pid);
    p.send("q");
    let st = p.wait_exit(Duration::from_secs(8)).expect("did not exit while indexing ran");
    assert!(st.success(), "{st:?}");
    std::thread::sleep(Duration::from_millis(400));
    for q in &tree {
        assert!(!alive(*q), "server process {q} survived quitting");
    }
    assert!(p.text().contains("\x1b[?1049l"));

    // The browser itself is killed outright (a crash with no chance to
    // clean up): the sandboxed server tree goes with it.
    std::fs::write(root.join("F0.lean"), "theorem changed (x : Nat) : x = x := by\n  rfl\n").unwrap();
    let p = start(&|p| {
        p.send("yes");
        p.send("\r");
    })
    .unwrap();
    let pid = p.child.id();
    let t = Instant::now();
    while children_of(pid).len() < 2 {
        assert!(t.elapsed() < Duration::from_secs(10), "server never started");
        std::thread::sleep(Duration::from_millis(50));
    }
    let tree = children_of(pid);
    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    let t = Instant::now();
    while tree.iter().any(|q| alive(*q)) && t.elapsed() < Duration::from_secs(4) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let left: Vec<_> = tree.iter().filter(|q| alive(**q)).collect();
    assert!(left.is_empty(), "server processes outlived a killed browser: {left:?}");
}
