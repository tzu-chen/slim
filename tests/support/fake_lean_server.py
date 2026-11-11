#!/usr/bin/env python3
"""A stand-in for `lake serve` in tests: speaks just enough of the Lean
language server protocol for slim's semantic-highlight indexer.

Behaviour comes from `fake.json` beside this script:
  delay_ms            time between didOpen and the end of "elaboration"
  hang_on / crash_on  file name suffixes that never finish / kill the server
  stale_on            file name suffix that reports out-of-date imports
  encoding            positionEncoding to announce (default: none = UTF-16)
  spawn_helper        start a `sleep` child, like a file worker process

Tokens: every `def`/`theorem`/`by`/`import` is a keyword, every other
identifier-like word a variable; a word after a dot is a property.
"""
import json, os, re, subprocess, sys, threading, time

here = os.path.dirname(os.path.abspath(__file__))
try:
    cfg = json.load(open(os.path.join(here, "fake.json")))
except Exception:
    cfg = {}
out_lock = threading.Lock()

def send(msg):
    body = json.dumps(msg).encode()
    with out_lock:
        sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
        sys.stdout.buffer.flush()

def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        k, _, v = line.partition(b":")
        if k.lower() == b"content-length":
            length = int(v)
    return json.loads(sys.stdin.buffer.read(length))

def units(s):
    enc = cfg.get("encoding")
    if enc == "utf-8":
        return len(s.encode("utf-8"))
    if enc == "utf-32":
        return len(s)
    return len(s.encode("utf-16-le")) // 2

KEYWORDS = {"def", "theorem", "by", "import", "example", "where"}

def tokens(text):
    data, last_line, last_col = [], 0, 0
    for n, line in enumerate(text.split("\n")):
        for m in re.finditer(r"[^\W\d]\w*", line):
            w = m.group(0)
            col = units(line[: m.start()])
            ty = 0 if w in KEYWORDS else (2 if m.start() > 0 and line[m.start() - 1] == "." else 1)
            data += [n - last_line, col - (last_col if n == last_line else 0), units(w), ty, 0]
            last_line, last_col = n, col
    return data

docs, done = {}, {}
waiting = {}  # uri -> [request ids]

def finish(uri, version, text):
    name = uri.rsplit("/", 1)[-1]
    nlines = text.count("\n") + 1
    def progress(from_line):
        proc = [] if from_line is None else [{"range": {"start": {"line": from_line, "character": 0}, "end": {"line": nlines, "character": 0}}, "kind": 1}]
        send({"jsonrpc": "2.0", "method": "$/lean/fileProgress", "params": {"textDocument": {"uri": uri, "version": version}, "processing": proc}})
    progress(0)
    if cfg.get("crash_on") and name.endswith(cfg["crash_on"]):
        os._exit(7)
    if cfg.get("hang_on") and name.endswith(cfg["hang_on"]):
        progress(max(0, nlines // 2))
        return
    time.sleep(cfg.get("delay_ms", 0) / 1000.0)
    diags = []
    if cfg.get("stale_on") and name.endswith(cfg["stale_on"]):
        diags = [{"severity": 1, "message": "Imports are out of date and must be rebuilt; use the \"Restart File\" command in your editor.",
                  "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}}]
    send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {"uri": uri, "version": version, "diagnostics": diags}})
    send({"jsonrpc": "2.0", "id": "refresh-%s" % version, "method": "workspace/semanticTokens/refresh"})
    progress(None)
    done[uri] = True
    for rid in waiting.pop(uri, []):
        send({"jsonrpc": "2.0", "id": rid, "result": {}})

helper = None
if cfg.get("spawn_helper"):
    helper = subprocess.Popen(["sleep", "300"])

while True:
    m = read()
    if m is None:
        break
    method = m.get("method")
    if method == "initialize":
        caps = {"semanticTokensProvider": {"full": True, "range": True, "legend": {
            "tokenTypes": ["keyword", "variable", "property", "function"], "tokenModifiers": ["declaration"]}}}
        if cfg.get("encoding"):
            caps["positionEncoding"] = cfg["encoding"]
        send({"jsonrpc": "2.0", "id": m["id"], "result": {"capabilities": caps, "serverInfo": {"name": "fake-lean", "version": "0"}}})
        send({"jsonrpc": "2.0", "id": "register", "method": "client/registerCapability", "params": {"registrations": []}})
    elif method == "textDocument/didOpen":
        td = m["params"]["textDocument"]
        if m["params"].get("dependencyBuildMode") != "never":
            # The indexer must never let the server build anything.
            os._exit(9)
        docs[td["uri"]] = td["text"]
        threading.Thread(target=finish, args=(td["uri"], td["version"], td["text"]), daemon=True).start()
    elif method == "textDocument/waitForDiagnostics":
        uri = m["params"]["uri"]
        if done.get(uri):
            send({"jsonrpc": "2.0", "id": m["id"], "result": {}})
        else:
            waiting.setdefault(uri, []).append(m["id"])
    elif method == "textDocument/semanticTokens/full":
        uri = m["params"]["textDocument"]["uri"]
        text = docs.get(uri, "")
        if cfg.get("hang_on") and uri.endswith(cfg["hang_on"]):
            # Only the processed prefix.
            text = "\n".join(text.split("\n")[: max(0, (text.count("\n") + 1) // 2)])
        send({"jsonrpc": "2.0", "id": m["id"], "result": {"data": tokens(text)}})
    elif method == "textDocument/didClose":
        uri = m["params"]["textDocument"]["uri"]
        docs.pop(uri, None)
        done.pop(uri, None)
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": m["id"], "result": None})
    elif method == "exit":
        break
if helper:
    helper.kill()
