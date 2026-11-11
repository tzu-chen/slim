#!/usr/bin/env python3
"""Tables for docs/bench/highlight.md from the raw session reports.

  scripts/summarize-highlight-bench.py SESSIONS.md...      (pseudo-terminal sessions)
  scripts/summarize-highlight-bench.py --bench REPORT.json (slim highlight bench)

Sessions: one row per variant; each cell is the value of the three runs
(median, with the range when the runs differ).
"""
import collections, json, re, sys

KEYS = [
    ("input event to frame written", "event → frame"),
    ("frame (render + write)", "frame"),
    ("ui event handling", "UI per event"),
    ("highlight: file asked to spans on the UI thread", "file asked → spans"),
    ("highlight: lines further down asked to spans on the UI thread", "jump further down in a file not yet tokenized that far → spans"),
    ("source: file asked to text on the UI thread", "file asked → text"),
]

def med(v):
    v = sorted(v)
    return v[len(v) // 2]

def cell(vals, fmt="{:.2f}"):
    if not vals:
        return "–"
    lo, hi, m = min(vals), max(vals), med(vals)
    return fmt.format(m) if fmt.format(lo) == fmt.format(hi) else f"{fmt.format(m)} ({fmt.format(lo)}–{fmt.format(hi)})"

def sessions(paths):
    agg = collections.OrderedDict()
    for p in paths:
        for r in re.split(r"^## ", open(p).read(), flags=re.M)[1:]:
            lines = r.splitlines()
            label = lines[0].split(" — run")[0]
            d = agg.setdefault(label, collections.defaultdict(list))
            m = re.search(r"first frame (\d+) ms; search ready after (\d+) ms; peak RSS (\d+) MiB", r)
            if m:
                d["first"].append(int(m.group(1))); d["ready"].append(int(m.group(2))); d["rss"].append(int(m.group(3)))
            m = re.search(r"CPU ([\d.]+) s; read ([\d.]+) MiB \(from storage ([\d.]+) MiB\); written ([\d.]+) MiB \(to storage ([\d.]+) MiB\)", r)
            if m:
                for k, g in zip(["cpu", "read", "read_dev", "written", "written_dev"], m.groups()):
                    d[k].append(float(g))
            m = re.search(r"idle for (\d+) s: (\d+) ms of CPU time", r)
            if m:
                d["idle"].append(int(m.group(2)))
            for l in lines:
                if l.startswith("| ") and not l.startswith("| measurement") and not l.startswith("|---"):
                    c = [x.strip() for x in l.strip("|").split("|")]
                    d[c[0]].append(c[1:])
    print("| session | runs | first frame ms | search ready ms | CPU s | peak RSS MiB | read MiB | written MiB |")
    print("|---|---|---|---|---|---|---|---|")
    for label, d in agg.items():
        print(f"| {label} | {len(d['first'])} | {cell(d['first'], '{:.0f}')} | {cell(d['ready'], '{:.0f}')} | {cell(d['cpu'])} | {cell(d['rss'], '{:.0f}')} | {cell(d['read'], '{:.1f}')} | {cell(d['written'])} |")
    for key, title in KEYS:
        print(f"\n**{title}** (ms; `{key}`)\n")
        print("| session | n per run | p50 | p95 | p99 | max |")
        print("|---|---|---|---|---|---|")
        for label, d in agg.items():
            rows = d.get(key)
            if not rows:
                why = "did not occur" if key.startswith("highlight: lines further down") else "not recorded by this build" if "before highlighting" in label else "does not apply"
                print(f"| {label} | – | {why} | | | |")
                continue
            col = lambda i: [float(r[i]) for r in rows if r[i] != "-"]
            print(f"| {label} | {rows[0][0]} | {cell(col(1))} | {cell(col(2))} | {cell(col(3))} | {cell(col(4))} |")
    idle = [(l, d["idle"]) for l, d in agg.items() if d.get("idle")]
    if idle:
        print("\n**Idle minute**\n")
        for l, v in idle:
            print(f"* {l}: {v[0]} ms of CPU time in 60 s")

def bench(path):
    r = json.load(open(path))
    t = r["tokenize"]
    f = lambda x: f"{x:.2f}"
    d = lambda x: f"{f(x['p50'])} / {f(x['p95'])} / {f(x['p99'])} / {f(x['max'])}" if x.get("n") else "–"
    print(f"Project `{r['project']}`: {r['local_files']} local Lean files, {r['local_mib']:.1f} MiB; measured {r['measured_files']} files"
          + (" (a deterministic sample: every k-th file by path plus the 20 largest)" if r["sampled"] else " (all)") + ".")
    print(f"\n* Tokenizing: {t['bytes'] / 1048576:.1f} MiB, {t['lines']} lines, {t['tokens']} tokens in {t['seconds']:.2f} s: "
          f"**{t['mib_per_s']:.1f} MiB/s, {t['lines_per_s']:.0f} lines/s** on one thread.")
    print(f"* Per file (ms, p50 / p95 / p99 / max): whole file {d(t['per_file_ms'])}; first 240 lines (a viewport and its margin) {d(t['first_240_lines_ms'])}.")
    print("* Slowest files: " + "; ".join(f"`{s['file']}` ({s['bytes'] / 1024:.0f} KiB) {s['ms']:.0f} ms" for s in t["slowest"]) + ".")
    print("\n| opening a file through the worker's code path | n | first answer ms: p50 / p95 / p99 / max |")
    print("|---|---|---|")
    for k, v in r["open_ms"].items():
        print(f"| {k} | {v['first_answer_ms'].get('n', 0)} | {d(v['first_answer_ms'])} |")
    n = r["navigation"]
    print(f"\n* Long navigation: {n['files_opened_and_fully_tokenized']} files opened one after another and tokenized completely in {n['seconds']:.1f} s; "
          f"worker cache peak {n['peak_worker_cache_mib']:.1f} MiB with a {n['budget_mib']} MiB budget ({n['files_in_cache_at_end']} files still cached at the end); "
          f"process RSS {n['rss_before_mib']:.0f} → {n['rss_after_mib']:.0f} MiB.")
    print("\n| headless frame of the source view | off | syntax | syntax + semantic |")
    print("|---|---|---|---|")
    for size, v in r["render_frame_ms"].items():
        print(f"| {size}, {v['off']['n']} frames: p50 / p95 / p99 / max ms | {d(v['off'])} | {d(v['syntax'])} | {d(v['syntax + semantic'])} |")
    s = r["semantic_cache"]
    print(f"\n* Semantic cache: {s['note']}." + (f" Reading and parsing a record: {d(s['read_and_parse_ms'])} ms; decoding all its chunks: {d(s['decode_all_chunks_ms'])} ms; "
          f"token blob size p50 / p95 / max: {s['blob_bytes']['p50']:.0f} / {s['blob_bytes']['p95']:.0f} / {s['blob_bytes']['max']:.0f} bytes." if s.get("records_with_tokens") else ""))

if sys.argv[1] == "--bench":
    bench(sys.argv[2])
else:
    sessions(sys.argv[1:])
