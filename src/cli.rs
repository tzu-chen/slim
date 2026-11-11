//! Minimal argument parsing (no external dependency).

use anyhow::{bail, Result};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Default)]
pub struct Args {
    pub command: Option<String>,
    pub positional: Vec<String>,
    flags: HashMap<String, Vec<String>>,
    switches: std::collections::HashSet<String>,
}

/// Options that take a value.
const VALUED: &[&str] = &[
    "threads", "index", "source-root", "cache-dir", "theme", "out", "o", "project", "toolchain", "library",
    "library-types", "timeout", "memory-mib", "sandbox", "decls", "edges", "seed", "repeat", "max-seconds", "lean",
    "exporter-dir", "limit", "editor", "query", "max-library-decls", "module", "report", "pp", "map", "trail",
    "syntax-theme", "highlight", "list", "files", "max-files", "file-timeout", "budget", "cgroup", "highlight-memory-mib",
    "open-docs", "max-file-mib", "trace", "repeat-trace", "namespace", "group", "within",
];

impl Args {
    pub fn parse(argv: impl IntoIterator<Item = OsString>) -> Result<Args> {
        let mut a = Args::default();
        let mut it = argv.into_iter().skip(1).map(|s| s.to_string_lossy().to_string()).peekable();
        let mut only_positional = false;
        while let Some(arg) = it.next() {
            if only_positional || !arg.starts_with('-') || arg == "-" {
                if a.command.is_none() && a.positional.is_empty() && is_command(&arg) && !only_positional {
                    a.command = Some(arg);
                } else {
                    a.positional.push(arg);
                }
                continue;
            }
            if arg == "--" {
                only_positional = true;
                continue;
            }
            let body = arg.trim_start_matches('-');
            let (key, inline) = match body.split_once('=') {
                Some((k, v)) => (k.to_string(), Some(v.to_string())),
                None => (body.to_string(), None),
            };
            if VALUED.contains(&key.as_str()) {
                let v = match inline {
                    Some(v) => v,
                    None => match it.next() {
                        Some(v) => v,
                        None => bail!("option --{key} needs a value"),
                    },
                };
                a.flags.entry(key).or_default().push(v);
            } else {
                if inline.is_some() {
                    bail!("option --{key} does not take a value");
                }
                a.switches.insert(key);
            }
        }
        Ok(a)
    }

    pub fn has(&self, k: &str) -> bool {
        self.switches.contains(k)
    }
    pub fn get(&self, k: &str) -> Option<&str> {
        self.flags.get(k).and_then(|v| v.last()).map(|s| s.as_str())
    }
    pub fn get_all(&self, k: &str) -> Vec<String> {
        self.flags.get(k).cloned().unwrap_or_default()
    }
    pub fn path(&self, k: &str) -> Option<PathBuf> {
        self.get(k).map(PathBuf::from)
    }
    pub fn num<T: std::str::FromStr>(&self, k: &str) -> Result<Option<T>> {
        match self.get(k) {
            None => Ok(None),
            Some(v) => v.parse::<T>().map(Some).map_err(|_| anyhow::anyhow!("--{k}: invalid number {v:?}")),
        }
    }
    pub fn unknown_switches(&self, allowed: &[&str]) -> Vec<String> {
        self.switches.iter().filter(|s| !allowed.contains(&s.as_str())).cloned().collect()
    }
}

fn is_command(s: &str) -> bool {
    matches!(
        s,
        "demo" | "open" | "scan" | "import" | "export" | "info" | "search" | "trail" | "bench" | "help" | "version" | "check-axioms"
            | "bookmarks" | "fixture-demo-snapshot" | "map" | "highlight" | "imports" | "skim"
    )
}

pub const HELP: &str = r#"slim — a fast terminal navigator for reading large Lean 4 projects

USAGE
  slim demo                             Open the bundled demo (no Lean, network or build needed)
  slim [open] <PROJECT_DIR>             Browse a project: its imported semantic snapshot if one
                                        exists, otherwise a source-only index (built on first open)
  slim open --index <FILE.sqlite>       Open an existing index file directly
  slim scan <PROJECT_DIR>               Build/refresh the source-only index without the UI
  slim import <SNAPSHOT.jsonl> [--source-root DIR] [--allow-partial]
                                        Import a semantic snapshot (validated, transactional,
                                        resumable); --source-root is the approved directory used
                                        to show source excerpts
  slim export <PROJECT_DIR> [--import] [--out FILE] [--sandbox auto|bwrap|none] [--with-highlight]
                                        EXPLICIT semantic extraction: builds the version-pinned
                                        exporter for the project's toolchain and runs it on the
                                        project's existing build artifacts. Prints exactly what will
                                        run and asks for confirmation. See README "Trust".
  slim info [<PROJECT_DIR> | --index FILE]       Provenance, coverage and freshness
  slim search <PROJECT_DIR|--index FILE> <QUERY...> [--within FILES | --namespace NS]
                                        Non-interactive search, optionally restricted to the
                                        files below a dotted module prefix or to a namespace
  slim skim <PROJECT_DIR|--index FILE> [MODULE_OR_DIR] [--namespace NS] [--group ID]
                                        Statements of a file, a directory, a namespace or a map
                                        group, with the first line of their documentation and no
                                        proofs (bounded; says when it stopped early)
  slim imports <PROJECT_DIR|--index FILE> [FROM TO] [--limit N]
                                        The import order: directories by import depth, endpoints
                                        (files nothing that declares imports) and hubs; with two
                                        modules, a chain of import lines from the first to the second
  slim check-axioms --index FILE        Compare slim's axiom analysis with Lean's own report
  slim map init <PROJECT_DIR> [-o FILE]        Print (or write, never overwriting) a starting
                                        lean-view.toml derived from directories, labelled generated
  slim map check <PROJECT_DIR> [--map FILE] [--fingerprints]
                                        Validate the project map against the index: field errors,
                                        unresolved or ambiguous names, stale annotations
  slim trail list|export [<PROJECT_DIR> | --project NAME] [--trail ID|TITLE] [-o FILE.md]
                                        List reading trails, or export one as Markdown (with the
                                        project directory, stops are checked against its index)
  slim bookmarks [--project NAME]       List bookmarks
  slim bench [--smoke | --full] [--decls N --edges M --seed S --max-seconds T]
                                        Deterministic synthetic benchmark of the index/browser
  slim highlight build <PROJECT_DIR> [--yes] [--files PATH]... [--budget S] [--file-timeout S]
                                        EXPLICIT semantic-highlight indexing: runs the project's own
                                        pinned Lean language server (sandboxed, never building or
                                        fetching) and caches Lean's semantic tokens per file.
                                        Resumable. `slim highlight` lists status, clean, dump.

COMMON OPTIONS
  --theme NAME                  lean (default: lean-lang.org palette, dark or light from
                                COLORFGBG), lean-dark, lean-light, dark, light, contrast,
                                mono. Also SLIM_THEME. NO_COLOR selects mono.
  --highlight MODE              semantic (default: syntax colours plus cached semantic tokens
                                when current), syntax, off. Also SLIM_HIGHLIGHT; F4 cycles.
  --syntax-theme NAME|FILE      dark-plus, light-plus, hc-black, hc-light or a VS Code colour
                                theme .json (default: follows --theme). Also
                                SLIM_SYNTAX_THEME; F5 cycles.
  --highlight-memory-mib N      Memory budget of the highlight worker's file cache (default 128)
  --ascii                       ASCII-only UI chrome (mathematical text is never altered)
  --threads N                   Scanner worker threads (default: min(4, available CPUs))
  --cache-dir DIR               Override the cache directory (default ~/.cache/slim)
  --editor CMD                  Executable for "open in editor" (arguments: +LINE FILE)
  --no-mouse                    Leave the mouse to the terminal (default: click and wheel)
  --map FILE                    Use this project map instead of <PROJECT_DIR>/lean-view.toml

Browsing never runs Lean, Lake, project code or the network. Only `export` and
`highlight build` (or their palette commands) run anything, and only after an explicit
confirmation.
"#;
