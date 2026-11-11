//! Owned child processes: resource limits, process groups, timeouts,
//! bounded output capture, cancellation and guaranteed reaping.
//!
//! Every child slim starts runs in its own session/process group with
//! `PR_SET_PDEATHSIG(SIGKILL)` so it cannot outlive slim, under
//! `setrlimit` limits (address space, CPU time, output file size, no core
//! dumps). The parent enforces a wall-clock timeout and cancellation by
//! signalling the whole group (`SIGTERM`, then `SIGKILL`) and always waits
//! for the child so no zombie remains. A process-wide registry lets the
//! panic hook and signal handler kill outstanding groups. These are limits,
//! not a sandbox; isolation comes from [`crate::export::Sandbox`].

use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct Limits {
    /// Coarse backstop only: Lean reserves large virtual ranges for thread
    /// stacks, so this must stay well above the real memory budget.
    pub address_space_mib: Option<u64>,
    /// Real memory budget, enforced by a systemd user scope (cgroup v2
    /// `MemoryMax`, swap disabled) when available.
    pub memory_mib: Option<u64>,
    pub tasks_max: Option<u64>,
    pub cpu_seconds: Option<u64>,
    pub file_size_mib: Option<u64>,
    pub wall: Duration,
    pub max_stderr_lines: usize,
    /// Also capture stdout into the bounded output tail.
    pub merge_stdout: bool,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            address_space_mib: Some(65536),
            memory_mib: Some(8192),
            tasks_max: Some(512),
            cpu_seconds: Some(3600),
            file_size_mib: Some(4096),
            wall: Duration::from_secs(1800),
            max_stderr_lines: 200,
            merge_stdout: false,
        }
    }
}

/// Whether `systemd-run --user --scope` can apply cgroup limits here.
pub fn cgroup_scope_available() -> bool {
    static AVAIL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAIL.get_or_init(|| {
        Command::new("systemd-run")
            .args(["--user", "--scope", "--quiet", "--collect", "-p", "MemoryMax=64M", "-p", "MemorySwapMax=0", "--", "/bin/true"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// Wraps `program args` in a cgroup scope when available and requested.
pub fn with_cgroup(limits: &Limits, program: &std::path::Path, args: &[std::ffi::OsString]) -> Command {
    match limits.memory_mib {
        Some(m) if cgroup_scope_available() => {
            let mut c = Command::new("systemd-run");
            c.args(["--user", "--scope", "--quiet", "--collect"]);
            c.arg("-p").arg(format!("MemoryMax={m}M"));
            c.args(["-p", "MemorySwapMax=0"]);
            if let Some(t) = limits.tasks_max {
                c.arg("-p").arg(format!("TasksMax={t}"));
            }
            c.arg("--").arg(program).args(args);
            c
        }
        _ => {
            let mut c = Command::new(program);
            c.args(args);
            c
        }
    }
}

impl Limits {
    pub fn describe(&self) -> String {
        let mut v = Vec::new();
        match self.memory_mib {
            Some(m) if cgroup_scope_available() => v.push(format!("memory ≤ {m} MiB, no swap (cgroup via systemd user scope)")),
            Some(m) => v.push(format!("memory budget {m} MiB NOT enforceable here (no systemd user scope); only the address-space backstop applies")),
            None => {}
        }
        if let Some(t) = self.tasks_max {
            if cgroup_scope_available() && self.memory_mib.is_some() {
                v.push(format!("≤ {t} tasks"));
            }
        }
        if let Some(a) = self.address_space_mib {
            v.push(format!("address space ≤ {a} MiB (RLIMIT_AS backstop)"));
        }
        if let Some(c) = self.cpu_seconds {
            v.push(format!("CPU time ≤ {c} s (RLIMIT_CPU)"));
        }
        if let Some(f) = self.file_size_mib {
            v.push(format!("files written ≤ {f} MiB (RLIMIT_FSIZE)"));
        }
        v.push(format!("wall clock ≤ {} s", self.wall.as_secs()));
        v.push("no core dumps".into());
        v.push("killed if slim exits (PR_SET_PDEATHSIG)".into());
        v.join(", ")
    }
}

static REGISTRY: Mutex<Vec<i32>> = Mutex::new(Vec::new());

fn register(pgid: i32) {
    if let Ok(mut r) = REGISTRY.lock() {
        r.push(pgid);
    }
}

fn unregister(pgid: i32) {
    if let Ok(mut r) = REGISTRY.lock() {
        r.retain(|&p| p != pgid);
    }
}

/// Process groups currently owned by this process.
pub fn registered_groups() -> Vec<i32> {
    REGISTRY.lock().map(|r| r.clone()).unwrap_or_default()
}

/// Emergency cleanup (panic hook, fatal signal): SIGKILL every owned group.
/// Only groups this process created are signalled.
pub fn kill_all_registered() {
    let groups = match REGISTRY.try_lock() {
        Ok(r) => r.clone(),
        Err(_) => return,
    };
    for pgid in groups {
        if pgid > 1 {
            unsafe {
                libc::kill(-pgid, libc::SIGKILL);
            }
        }
    }
}

#[derive(Debug)]
pub enum Outcome {
    Exited(ExitStatus),
    TimedOut,
    Cancelled,
}

pub struct OwnedChild {
    child: Option<Child>,
    pgid: i32,
    started: Instant,
    limits: Limits,
    stderr_tail: Arc<Mutex<Vec<String>>>,
    stderr_thread: Option<std::thread::JoinHandle<()>>,
}

/// Lowers a limit to `v`; a limit that is already lower (for example one
/// inherited from an outer resource guard) is kept.
fn set_rlimit(res: libc::__rlimit_resource_t, v: u64) -> std::io::Result<()> {
    let mut cur = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    let mut v = v as libc::rlim_t;
    if unsafe { libc::getrlimit(res, &mut cur) } == 0 && cur.rlim_max != libc::RLIM_INFINITY {
        v = v.min(cur.rlim_max);
    }
    let lim = libc::rlimit { rlim_cur: v, rlim_max: v };
    if unsafe { libc::setrlimit(res, &lim) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Spawns `cmd` with limits in a new session. stdin is closed; stderr is
/// captured (bounded tail, optional line callback via a channel); stdout is
/// whatever the caller configured (default: discarded).
pub fn spawn(cmd: &mut Command, limits: &Limits, line_tx: Option<std::sync::mpsc::Sender<String>>) -> Result<OwnedChild> {
    spawn_inner(cmd, limits, line_tx, false)
}

/// Like [`spawn`], with stdin and stdout connected to pipes for a protocol
/// conversation (take them with [`OwnedChild::take_stdin`] and
/// [`OwnedChild::take_stdout`]). stderr is still captured as a bounded tail.
pub fn spawn_piped(cmd: &mut Command, limits: &Limits) -> Result<OwnedChild> {
    spawn_inner(cmd, limits, None, true)
}

fn spawn_inner(cmd: &mut Command, limits: &Limits, line_tx: Option<std::sync::mpsc::Sender<String>>, piped: bool) -> Result<OwnedChild> {
    let parent = unsafe { libc::getpid() };
    let lim = limits.clone();
    if piped {
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
    cmd.stderr(Stdio::piped());
    unsafe {
        cmd.pre_exec(move || {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong, 0, 0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // The parent may have died between fork and prctl.
            if libc::getppid() != parent {
                libc::_exit(1);
            }
            set_rlimit(libc::RLIMIT_CORE, 0)?;
            if let Some(a) = lim.address_space_mib {
                set_rlimit(libc::RLIMIT_AS, a << 20)?;
            }
            if let Some(c) = lim.cpu_seconds {
                set_rlimit(libc::RLIMIT_CPU, c)?;
            }
            if let Some(f) = lim.file_size_mib {
                set_rlimit(libc::RLIMIT_FSIZE, f << 20)?;
            }
            Ok(())
        });
    }
    if limits.merge_stdout && !piped {
        cmd.stdout(Stdio::piped());
    }
    let mut child = cmd.spawn().with_context(|| format!("starting {:?}", cmd.get_program()))?;
    let pgid = child.id() as i32;
    register(pgid);
    let tail = Arc::new(Mutex::new(Vec::new()));
    if let Some(out) = (limits.merge_stdout && !piped).then(|| child.stdout.take()).flatten() {
        let t3 = Arc::clone(&tail);
        let max_lines = limits.max_stderr_lines;
        std::thread::spawn(move || {
            let r = BufReader::new(out.take(256 << 20));
            for line in r.lines().map_while(|l| l.ok()) {
                if let Ok(mut t) = t3.lock() {
                    t.push(line);
                    if t.len() > max_lines {
                        let excess = t.len() - max_lines;
                        t.drain(..excess);
                    }
                }
            }
        });
    }
    let stderr = child.stderr.take();
    let t2 = Arc::clone(&tail);
    let max_lines = limits.max_stderr_lines;
    let handle = stderr.map(|e| {
        std::thread::spawn(move || {
            // Bounded line reads: lines longer than 64 KiB are cut.
            let mut r = BufReader::new(e.take(256 << 20));
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match r.by_ref().take(64 << 10).read_until(b'\n', &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let line = String::from_utf8_lossy(&buf).trim_end().to_string();
                        if let Some(tx) = &line_tx {
                            let _ = tx.send(line.clone());
                        }
                        if let Ok(mut t) = t2.lock() {
                            t.push(line);
                            if t.len() > max_lines {
                                let excess = t.len() - max_lines;
                                t.drain(..excess);
                            }
                        }
                    }
                }
            }
        })
    });
    Ok(OwnedChild { child: Some(child), pgid, started: Instant::now(), limits: limits.clone(), stderr_tail: tail, stderr_thread: handle })
}

impl OwnedChild {
    pub fn pid(&self) -> i32 {
        self.pgid
    }

    pub fn take_stdin(&mut self) -> Option<std::process::ChildStdin> {
        self.child.as_mut().and_then(|c| c.stdin.take())
    }

    pub fn take_stdout(&mut self) -> Option<std::process::ChildStdout> {
        self.child.as_mut().and_then(|c| c.stdout.take())
    }

    /// Whether the child has exited (non-blocking; reaps it if so).
    pub fn exited(&mut self) -> Option<ExitStatus> {
        let st = self.child.as_mut()?.try_wait().ok().flatten()?;
        self.finish();
        Some(st)
    }

    pub fn stderr_tail(&self) -> Vec<String> {
        self.stderr_tail.lock().map(|t| t.clone()).unwrap_or_default()
    }

    /// Waits for exit, timeout or cancellation (polling every 50 ms; this
    /// runs on a worker thread, never on the UI thread).
    pub fn wait(&mut self, cancel: &AtomicBool) -> Result<Outcome> {
        loop {
            let child = self.child.as_mut().context("child already reaped")?;
            if let Some(st) = child.try_wait()? {
                self.finish();
                return Ok(Outcome::Exited(st));
            }
            if cancel.load(Ordering::Relaxed) {
                self.terminate();
                return Ok(Outcome::Cancelled);
            }
            if self.started.elapsed() > self.limits.wall {
                self.terminate();
                return Ok(Outcome::TimedOut);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn finish(&mut self) {
        // The leader exited; make sure no member of its group survives.
        unsafe {
            libc::kill(-self.pgid, libc::SIGKILL);
        }
        if let Some(h) = self.stderr_thread.take() {
            let _ = h.join();
        }
        unregister(self.pgid);
        self.child = None;
    }

    /// SIGTERM to the group, a grace period, then SIGKILL; always reaps.
    pub fn terminate(&mut self) {
        let Some(mut child) = self.child.take() else { return };
        unsafe {
            libc::kill(-self.pgid, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_millis(1500);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                _ => {
                    unsafe {
                        libc::kill(-self.pgid, libc::SIGKILL);
                    }
                    let _ = child.wait();
                    break;
                }
            }
        }
        unsafe {
            libc::kill(-self.pgid, libc::SIGKILL);
        }
        if let Some(h) = self.stderr_thread.take() {
            let _ = h.join();
        }
        unregister(self.pgid);
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.child.is_some() {
            self.terminate();
        }
    }
}

/// Runs a command to completion with limits, returning (status, stderr tail).
pub fn run_bounded(cmd: &mut Command, limits: &Limits, cancel: &AtomicBool) -> Result<(ExitStatus, Vec<String>)> {
    if !limits.merge_stdout {
        cmd.stdout(Stdio::null());
    }
    let mut ch = spawn(cmd, limits, None)?;
    match ch.wait(cancel)? {
        Outcome::Exited(st) => Ok((st, ch.stderr_tail())),
        Outcome::TimedOut => bail!("timed out after {} s", limits.wall.as_secs()),
        Outcome::Cancelled => bail!("cancelled"),
    }
}

/// `pid` and every live descendant of it (by parent links in `/proc`).
/// Processes in a sandbox's PID namespace are visible here by host pid.
pub fn descendants(pid: i32) -> Vec<i32> {
    let mut children: std::collections::HashMap<i32, Vec<i32>> = Default::default();
    if let Ok(rd) = std::fs::read_dir("/proc") {
        for e in rd.flatten() {
            let Some(p) = e.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) else { continue };
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{p}/stat")) else { continue };
            // The command name may contain spaces and parentheses: fields
            // start after the last ')'.
            let Some(rest) = stat.rfind(')').map(|i| &stat[i + 1..]) else { continue };
            let mut f = rest.split_whitespace();
            if f.next() == Some("Z") {
                continue;
            }
            if let Some(ppid) = f.next().and_then(|s| s.parse::<i32>().ok()) {
                children.entry(ppid).or_default().push(p);
            }
        }
    }
    let mut out = Vec::new();
    let mut stack = vec![pid];
    while let Some(p) = stack.pop() {
        if p != pid || process_alive(p) {
            out.push(p);
        }
        if let Some(c) = children.get(&p) {
            stack.extend(c);
        }
    }
    out
}

/// Resident memory (KiB) summed over `pid` and its descendants, and how
/// many processes that is. Shared pages are counted once per process.
pub fn tree_rss_kib(pid: i32) -> (u64, usize) {
    let page_kib = (unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64) / 1024;
    let procs = descendants(pid);
    let mut total = 0;
    for p in &procs {
        if let Ok(s) = std::fs::read_to_string(format!("/proc/{p}/statm")) {
            total += s.split_whitespace().nth(1).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) * page_kib;
        }
    }
    (total, procs.len())
}

/// Memory of the cgroup a process is in, as the kernel accounts it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CgroupMemory {
    /// Anonymous memory (heaps and stacks; not file cache), bytes.
    pub anon: u64,
    /// Everything charged, including file cache, bytes.
    pub current: u64,
    /// The cgroup's own limit (`memory.max`), if it has one.
    pub max: Option<u64>,
}

/// Reads the cgroup-v2 memory accounting of the cgroup `pid` is in. Only
/// that cgroup's own limit is reported: a limit further up (a whole user
/// session, say) also counts processes that are none of our business.
pub fn cgroup_memory(pid: i32) -> Option<CgroupMemory> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    let rel = text.lines().find_map(|l| l.strip_prefix("0::"))?.trim();
    let own = std::path::Path::new("/sys/fs/cgroup").join(rel.trim_start_matches('/'));
    let num = |name: &str| std::fs::read_to_string(own.join(name)).ok().and_then(|s| s.trim().parse::<u64>().ok());
    let current = num("memory.current")?;
    let anon = std::fs::read_to_string(own.join("memory.stat"))
        .ok()
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix("anon ").and_then(|v| v.trim().parse::<u64>().ok())))
        .unwrap_or(current);
    // `memory.max` reads "max" when there is no limit.
    Some(CgroupMemory { anon, current, max: num("memory.max") })
}

/// True if a process with this pid exists (and is not a zombie).
pub fn process_alive(pid: i32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => {
            // state is the field after the parenthesised command name
            match s.rfind(')') {
                Some(i) => !s[i + 1..].trim_start().starts_with('Z'),
                None => true,
            }
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_kills_whole_group_and_reaps() {
        let d = tempfile::tempdir().unwrap();
        let pidfile = d.path().join("grandchild.pid");
        let script = format!("sleep 30 & echo $! > {}; wait", pidfile.display());
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg(script);
        let limits = Limits { wall: Duration::from_millis(400), ..Default::default() };
        let mut ch = spawn(&mut cmd, &limits, None).unwrap();
        let pid = ch.pid();
        let cancel = AtomicBool::new(false);
        let out = ch.wait(&cancel).unwrap();
        assert!(matches!(out, Outcome::TimedOut));
        std::thread::sleep(Duration::from_millis(100));
        let gc: i32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
        assert!(!process_alive(pid), "child reaped");
        assert!(!process_alive(gc), "grandchild killed");
        assert!(!registered_groups().contains(&pid));
    }

    #[test]
    fn cancellation_and_limits() {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg("sleep 30");
        let mut ch = spawn(&mut cmd, &Limits::default(), None).unwrap();
        let pid = ch.pid();
        let cancel = AtomicBool::new(true);
        assert!(matches!(ch.wait(&cancel).unwrap(), Outcome::Cancelled));
        assert!(!process_alive(pid));
        // RLIMIT_FSIZE is applied: writing more than 1 MiB fails.
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("big");
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg(format!("head -c 3000000 /dev/zero > {}", f.display()));
        let lim = Limits { file_size_mib: Some(1), ..Default::default() };
        let (st, _) = run_bounded(&mut cmd, &lim, &AtomicBool::new(false)).unwrap();
        assert!(!st.success());
        assert!(std::fs::metadata(&f).unwrap().len() <= 1 << 20);
    }

    #[test]
    fn drop_kills_child() {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg("sleep 30");
        let ch = spawn(&mut cmd, &Limits::default(), None).unwrap();
        let pid = ch.pid();
        drop(ch);
        assert!(!process_alive(pid));
    }

    #[test]
    fn stderr_is_captured_and_bounded() {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg("for i in $(seq 1 500); do echo line$i >&2; done");
        let lim = Limits { max_stderr_lines: 10, ..Default::default() };
        let (st, tail) = run_bounded(&mut cmd, &lim, &AtomicBool::new(false)).unwrap();
        assert!(st.success());
        assert_eq!(tail.len(), 10);
        assert_eq!(tail.last().unwrap(), "line500");
    }
}
