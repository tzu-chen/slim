#!/usr/bin/env python3
"""Independent cross-check of slim's cached semantic tokens.

  scripts/check-semantic-tokens.py PROJECT_DIR HIGHLIGHT.sqlite REL_FILE...

For each file this script talks to the project's pinned Lean language server
itself (a second, independent LSP client: `lake serve` in bubblewrap with no
network and the project read-only, documents opened with
dependencyBuildMode "never"), decodes the relative UTF-16 token data on its
own, decodes slim's stored blob on its own, and compares the two token
lists: line, start byte, end byte, token type name. It shares no code with
slim. Run it through the machine's resource guard for real projects.
"""
import json, os, sqlite3, subprocess, sys, threading

def serve(root, tcdir):
    lake = os.path.join(tcdir, "bin", "lake")
    cmd = ["bwrap", "--unshare-all", "--die-with-parent", "--new-session", "--cap-drop", "ALL", "--ro-bind", "/usr", "/usr",
           "--symlink", "usr/lib", "/lib", "--symlink", "usr/lib64", "/lib64", "--symlink", "usr/bin", "/bin", "--symlink", "usr/sbin", "/sbin",
           "--ro-bind-try", "/etc/ld.so.cache", "/etc/ld.so.cache", "--ro-bind-try", "/etc/localtime", "/etc/localtime",
           "--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp", "--ro-bind", tcdir, tcdir, "--ro-bind", root, root,
           "--clearenv", "--setenv", "PATH", "/usr/bin", "--setenv", "HOME", "/tmp", "--setenv", "LANG", "C.UTF-8", "--chdir", root, "--", lake, "serve", "--"]
    p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    return p

def send(p, m):
    b = json.dumps(m).encode()
    p.stdin.write(b"Content-Length: %d\r\n\r\n" % len(b) + b)
    p.stdin.flush()

def read(p):
    n = None
    while True:
        l = p.stdout.readline()
        if not l:
            return None
        l = l.strip()
        if not l:
            break
        k, _, v = l.partition(b":")
        if k.lower() == b"content-length":
            n = int(v)
    return json.loads(p.stdout.read(n))

def server_tokens(p, root, rel, version, legend):
    path = os.path.join(root, rel)
    text = open(path, "rb").read().decode("utf-8")
    uri = "file://" + path
    send(p, {"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {"textDocument": {"uri": uri, "languageId": "lean4", "version": version, "text": text}, "dependencyBuildMode": "never"}})
    wait_id, tok_id = 1000 + version * 2, 1001 + version * 2
    send(p, {"jsonrpc": "2.0", "id": wait_id, "method": "textDocument/waitForDiagnostics", "params": {"uri": uri, "version": version}})
    waited = empty = False
    data = None
    while data is None:
        m = read(p)
        if m is None:
            raise SystemExit("server exited")
        if "method" in m and "id" in m:
            send(p, {"jsonrpc": "2.0", "id": m["id"], "result": None})
        elif m.get("method") == "$/lean/fileProgress" and m["params"]["textDocument"]["uri"] == uri:
            empty = not m["params"]["processing"]
        elif "method" not in m and m.get("id") == wait_id:
            waited = True
        elif "method" not in m and m.get("id") == tok_id:
            data = m["result"]["data"]
        if waited and empty and tok_id is not None and data is None and not getattr(server_tokens, "asked", {}).get(uri):
            server_tokens.asked = {uri: True}
            send(p, {"jsonrpc": "2.0", "id": tok_id, "method": "textDocument/semanticTokens/full", "params": {"textDocument": {"uri": uri}}})
    send(p, {"jsonrpc": "2.0", "method": "textDocument/didClose", "params": {"textDocument": {"uri": uri}}})
    # Independent decoding: relative UTF-16 positions -> bytes within the line.
    lines = text.split("\n")
    out, line, col = [], 0, 0
    for i in range(0, len(data), 5):
        dl, ds, length, ty, _mods = data[i:i + 5]
        line += dl
        col = ds if dl else col + ds
        u16 = lines[line].encode("utf-16-le")
        start = len(u16[: col * 2].decode("utf-16-le").encode("utf-8"))
        end = len(u16[: (col + length) * 2].decode("utf-16-le").encode("utf-8"))
        out.append((line, start, end, legend[ty]))
    return out, text

def varint(b, i):
    v = shift = 0
    while True:
        x = b[i]; i += 1
        v |= (x & 0x7F) << shift
        if not x & 0x80:
            return v, i
        shift += 7

def stored_tokens(db, rel):
    row = db.execute("select blob, legend, source_sha256, status from semantic where path = ?", (rel,)).fetchone()
    if row is None or row[0] is None:
        return None, None, None
    blob, legend = row[0], json.loads(row[1])["types"]
    assert blob[:4] == b"LNS1" and blob[4] == 1
    n_lines, i = varint(blob, 5)
    n_chunks, i = varint(blob, i)
    i += 4 * n_chunks
    out = []
    for line in range(n_lines):
        n, i = varint(blob, i)
        pos = 0
        for _ in range(n):
            d, i = varint(blob, i); length, i = varint(blob, i); ty, i = varint(blob, i); _m, i = varint(blob, i)
            start = pos + d
            out.append((line, start, start + length, legend[ty]))
            pos = start + length
    assert i == len(blob)
    return out, row[2], row[3]

def main():
    root, dbpath, files = os.path.realpath(sys.argv[1]), sys.argv[2], sys.argv[3:]
    tc = open(os.path.join(root, "lean-toolchain")).read().strip()
    tcdir = os.path.expanduser("~/.elan/toolchains/" + tc.replace("/", "--").replace(":", "---"))
    db = sqlite3.connect(f"file:{dbpath}?mode=ro", uri=True)
    p = serve(root, tcdir)
    send(p, {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"processId": os.getpid(), "rootUri": "file://" + root, "capabilities": {
        "textDocument": {"semanticTokens": {"requests": {"full": True}, "tokenTypes": [], "tokenModifiers": [], "formats": ["relative"]}}}}})
    while True:
        m = read(p)
        if "method" in m and "id" in m:
            send(p, {"jsonrpc": "2.0", "id": m["id"], "result": None})
        elif m.get("id") == 1:
            legend = m["result"]["capabilities"]["semanticTokensProvider"]["legend"]["tokenTypes"]
            break
    send(p, {"jsonrpc": "2.0", "method": "initialized", "params": {}})
    bad = 0
    import hashlib
    for v, rel in enumerate(files, 1):
        server_tokens.asked = {}
        ours, sha, status = stored_tokens(db, rel)
        if ours is None:
            # Nothing to compare (a failed or skipped record): do not ask the server.
            print(f"{rel}: no stored tokens (not compared)", flush=True); continue
        theirs, text = server_tokens(p, root, rel, v, legend)
        if sha != hashlib.sha256(text.encode()).hexdigest():
            print(f"{rel}: the file changed since slim indexed it; not comparable"); continue
        same = ours == theirs
        kinds = {}
        for t in theirs:
            kinds[t[3]] = kinds.get(t[3], 0) + 1
        print(f"{rel}: {'identical' if same else 'DIFFERENT'} — {len(theirs)} tokens from the server, {len(ours)} stored ({status}); {kinds}", flush=True)
        if not same:
            bad += 1
            for a, b in zip(ours, theirs):
                if a != b:
                    print("  first difference: stored", a, "server", b); break
    send(p, {"jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": None})
    send(p, {"jsonrpc": "2.0", "method": "exit"})
    try:
        p.wait(timeout=10)
    except subprocess.TimeoutExpired:
        p.kill()
    sys.exit(1 if bad else 0)

main()
