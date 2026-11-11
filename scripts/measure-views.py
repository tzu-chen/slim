#!/usr/bin/env python3
"""Drives slim in a pseudo-terminal with a fixed key script and prints the
timing facts it records (SLIM_TIMING_LOG), plus peak resident memory.

Usage: measure-views.py LABEL BIN KEYS [--runs N] [--settle S] [--idle S] -- ARGS...

KEYS is a space-separated script: single characters, or names such as Enter,
Esc, Tab, Space, Backspace, Down, Up, PgDn, F4; `x*40` repeats a key; `~1.5`
sleeps; `Resize:200x60` changes the terminal size. Keys are sent 25 ms apart
(40 keys/s). `--idle S` then leaves the program alone for S seconds and
reports the CPU time it used. Nothing but slim runs; it is a direct child
and is killed if it does not quit by itself.
"""
import fcntl, json, os, pty, select, signal, struct, sys, tempfile, termios, time

NAMES = {"Enter": "\r", "Esc": "\x1b", "Tab": "\t", "Space": " ", "Backspace": "\x7f",
         "Down": "\x1b[B", "Up": "\x1b[A", "PgDn": "\x1b[6~", "PgUp": "\x1b[5~", "End": "\x1b[F", "Home": "\x1b[H", "Slash": "/", "Star": "*",
         "CtrlU": "\x15", "F4": "\x1b[14~", "F5": "\x1b[15~", "F6": "\x1b[17~", "Colon": ":", "LBracket": "[", "RBracket": "]"}


def cpu_seconds(pid):
    f = open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()
    return (int(f[11]) + int(f[12])) / os.sysconf("SC_CLK_TCK")


def drain(fd, seconds):
    end = time.time() + seconds
    while True:
        left = end - time.time()
        if left <= 0:
            return
        r, _, _ = select.select([fd], [], [], left)
        if r:
            try:
                if not os.read(fd, 65536):
                    return
            except OSError:
                return


def run(binary, args, keys, settle, env, idle=0.0):
    log = tempfile.NamedTemporaryFile(prefix="slim-timing-", suffix=".jsonl", delete=False)
    log.close()
    env = dict(env, SLIM_TIMING_LOG=log.name, TERM="xterm-256color", LANG="C.UTF-8", COLORTERM="truecolor")
    pid, fd = pty.fork()
    if pid == 0:
        os.execve(binary, [binary] + args, env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 34, 120, 0, 0))
    os.kill(pid, signal.SIGWINCH)
    drain(fd, settle)
    for k in keys:
        if k.startswith("~"):
            drain(fd, float(k[1:]))
            continue
        if k.startswith("Resize:"):
            cols, rows = k[7:].split("x")
            fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", int(rows), int(cols), 0, 0))
            os.kill(pid, signal.SIGWINCH)
            drain(fd, 0.15)
            continue
        key, _, n = k.partition("*") if len(k) > 1 and "*" in k[1:] else (k, "", "")
        for _ in range(int(n or 1)):
            os.write(fd, NAMES.get(key, key).encode())
            drain(fd, 0.025)
    drain(fd, 1.0)
    idle_cpu = None
    if idle > 0:
        c0 = cpu_seconds(pid)
        drain(fd, idle)
        idle_cpu = cpu_seconds(pid) - c0
    io = {}
    try:
        for line in open(f"/proc/{pid}/io"):
            k, v = line.split(":")
            io[k] = int(v)
    except OSError:
        pass
    cpu_total = cpu_seconds(pid)
    hwm = None
    try:
        for line in open(f"/proc/{pid}/status"):
            if line.startswith("VmHWM:"):
                hwm = int(line.split()[1]) / 1024
    except OSError:
        pass
    os.write(fd, b"\x1b")
    drain(fd, 0.1)
    os.write(fd, b"q")
    t = time.time()
    while time.time() - t < 5:
        drain(fd, 0.1)
        p, _ = os.waitpid(pid, os.WNOHANG)
        if p:
            break
    else:
        os.kill(pid, signal.SIGKILL)
        os.waitpid(pid, 0)
    try:
        rec = json.loads(open(log.name).read().strip().splitlines()[-1])
    except (IndexError, ValueError):
        rec = {}
    os.unlink(log.name)
    rec["peak_rss_mib"] = hwm
    rec["idle_cpu_s"] = idle_cpu
    rec["io"] = io
    rec["cpu_s"] = cpu_total
    return rec


def main():
    a = sys.argv[1:]
    cut = a.index("--")
    head, args = a[:cut], a[cut + 1:]
    label, binary, keys = head[0], os.path.abspath(head[1]), head[2].split()
    runs = int(head[head.index("--runs") + 1]) if "--runs" in head else 3
    settle = float(head[head.index("--settle") + 1]) if "--settle" in head else 2.5
    idle = float(head[head.index("--idle") + 1]) if "--idle" in head else 0.0
    env = {k: v for k, v in os.environ.items() if k.startswith("SLIM_") or k in ("HOME", "PATH")}
    for i in range(runs):
        data = tempfile.mkdtemp(prefix="slim-measure-data-")
        rec = run(binary, args, keys, settle, dict(env, SLIM_DATA_DIR=data), idle)
        stats = rec.get("stats_ms", {})
        print(f"## {label} — run {i + 1}")
        print(f"first frame {rec.get('first_frame_ms')} ms; search ready after {rec.get('search_ready_after_start_ms')} ms; peak RSS {rec.get('peak_rss_mib'):.0f} MiB; mode {rec.get('mode')}")
        io = rec.get("io") or {}
        if io:
            print(f"CPU {rec.get('cpu_s'):.2f} s; read {io.get('rchar', 0) / 1048576:.1f} MiB (from storage {io.get('read_bytes', 0) / 1048576:.1f} MiB); written {io.get('wchar', 0) / 1048576:.2f} MiB (to storage {io.get('write_bytes', 0) / 1048576:.2f} MiB)")
        if rec.get("idle_cpu_s") is not None:
            print(f"idle for {idle:.0f} s: {rec['idle_cpu_s'] * 1000:.0f} ms of CPU time")
        print("| measurement | n | p50 ms | p95 ms | p99 ms | max ms |\n|---|---|---|---|---|---|")
        for k in sorted(stats):
            v = stats[k]
            p99 = f"{v['p99']:.2f}" if "p99" in v else "-"
            print(f"| {k} | {v['n']} | {v['p50']:.2f} | {v['p95']:.2f} | {p99} | {v['max']:.2f} |")
        print()


if __name__ == "__main__":
    main()
