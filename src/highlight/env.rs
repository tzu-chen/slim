//! The inputs a semantic record depends on besides its own source text:
//! the toolchain and project configuration ("environment"), the sources of
//! the modules it imports (transitively, captured from module headers), and
//! the compiled artifacts of those imports.
//!
//! Everything here is read as data. Nothing runs Lean or Lake, and nothing
//! is taken from the browser's lexical mention graph: imports come from the
//! fixed `import` header syntax only.

use crate::paths::sha256_hex;
use crate::source::lexer::{LexProfile, Lexer, TokKind};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

fn mtime_ns(md: &std::fs::Metadata) -> i64 {
    md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos() as i64).unwrap_or(0)
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileId {
    pub path: String,
    pub size: u64,
    pub mtime_ns: i64,
}

/// Toolchain and configuration inputs, as far as they can be known without
/// executing anything.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvInfo {
    /// Contents of `lean-toolchain`.
    pub toolchain_spec: Option<String>,
    /// The `lean` executable of that toolchain as installed (identity by
    /// path, size and modification time).
    pub lean_exe: Option<FileId>,
    pub lake_manifest_sha256: Option<String>,
    /// (`lakefile.toml` | `lakefile.lean`, SHA-256).
    pub lakefile: Option<(String, String)>,
    /// (package, revision) from `lake-manifest.json`; path dependencies
    /// appear with their directory.
    pub packages: Vec<(String, String)>,
    pub extractor: String,
}

impl EnvInfo {
    pub fn current(root: &Path) -> EnvInfo {
        let toolchain_spec = std::fs::read_to_string(root.join("lean-toolchain")).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && s.len() < 200);
        let lean_exe = toolchain_spec.as_ref().and_then(|tc| {
            let p = crate::export::toolchain_dir(tc).join("bin/lean");
            let md = std::fs::metadata(&p).ok()?;
            Some(FileId { path: p.display().to_string(), size: md.len(), mtime_ns: mtime_ns(&md) })
        });
        let manifest = std::fs::read(root.join("lake-manifest.json")).ok();
        let mut packages = Vec::new();
        if let Some(j) = manifest.as_ref().and_then(|b| serde_json::from_slice::<serde_json::Value>(b).ok()) {
            for p in j.get("packages").and_then(|p| p.as_array()).into_iter().flatten() {
                let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("?").to_string();
                let rev = p.get("rev").or(p.get("dir")).and_then(|v| v.as_str()).unwrap_or("?").to_string();
                packages.push((name, rev));
            }
        }
        packages.sort();
        let lakefile = ["lakefile.toml", "lakefile.lean"]
            .iter()
            .find_map(|n| std::fs::read(root.join(n)).ok().map(|b| (n.to_string(), sha256_hex(&b))));
        EnvInfo {
            toolchain_spec,
            lean_exe,
            lake_manifest_sha256: manifest.map(|b| sha256_hex(&b)),
            lakefile,
            packages,
            extractor: super::EXTRACTOR_VERSION.to_string(),
        }
    }

    pub fn fingerprint(&self) -> String {
        sha256_hex(serde_json::to_string(self).unwrap_or_default().as_bytes())
    }

    /// What differs from `then` (the environment a record was made in), in
    /// words; empty when nothing known differs.
    pub fn changes_since(&self, then: &EnvInfo) -> Vec<String> {
        let mut v = Vec::new();
        if self.toolchain_spec != then.toolchain_spec {
            v.push(format!(
                "lean-toolchain changed ({} → {})",
                then.toolchain_spec.as_deref().unwrap_or("none"),
                self.toolchain_spec.as_deref().unwrap_or("none")
            ));
        }
        if self.lean_exe != then.lean_exe {
            v.push(match &self.lean_exe {
                Some(_) => "the installed lean executable of this toolchain changed".to_string(),
                None => "the toolchain is no longer installed".to_string(),
            });
        }
        if self.lake_manifest_sha256 != then.lake_manifest_sha256 {
            v.push("lake-manifest.json changed (dependency resolution)".into());
        }
        if self.lakefile != then.lakefile {
            v.push("the Lake configuration file changed".into());
        }
        if self.packages != then.packages && self.lake_manifest_sha256 == then.lake_manifest_sha256 {
            v.push("dependency revisions changed".into());
        }
        if self.extractor != then.extractor {
            v.push(format!("extractor changed ({} → {})", then.extractor, self.extractor));
        }
        v
    }
}

/// Environment as recorded by an indexing run: [`EnvInfo`] plus facts only
/// known once the toolchain was actually started.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EnvRecord {
    pub info: EnvInfo,
    /// Output of `lean --version`.
    pub lean_version: Option<String>,
    pub sandbox: String,
    pub server_command: String,
    pub created: String,
}

/// Module names imported by a file's header (`import`, `public import`,
/// `meta import`, `import all`, …), in order.
pub fn header_imports(text: &str, profile: LexProfile) -> Vec<String> {
    let mut lx = Lexer::new(text, profile);
    let mut out = Vec::new();
    let mut expect_module = false;
    while let Some(t) = lx.next_token() {
        if t.is_comment() {
            continue;
        }
        let s = t.text(text);
        match t.kind {
            TokKind::Ident if expect_module => match s {
                "all" | "meta" | "public" | "private" | "runtime" => {}
                m => {
                    out.push(m.to_string());
                    expect_module = false;
                }
            },
            TokKind::Ident => match s {
                "import" => expect_module = true,
                "module" | "prelude" | "public" | "meta" | "private" => {}
                _ => break,
            },
            _ => break,
        }
    }
    out
}

#[derive(Clone, Debug)]
struct Entry {
    size: u64,
    mtime_ns: i64,
    sha256: String,
    /// Imports that resolve to files of this project.
    imports: Vec<String>,
    /// How this entry was established in this process.
    hashed_this_call: bool,
}

/// A file's transitive local imports and their state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Closure {
    /// Relative paths, sorted, without the file itself.
    pub files: Vec<String>,
    /// SHA-256 over (path, source SHA-256) of every file of the closure.
    pub digest: String,
    /// False when some file was accepted from its size and modification
    /// time (a hint) instead of being hashed during this computation.
    pub content_verified: bool,
}

/// State of the compiled artifacts of a closure.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Artifacts {
    /// SHA-256 over (path, .olean size, .olean mtime); empty when there is
    /// no build directory.
    pub digest: String,
    /// Imports whose `.olean` is older than their source.
    pub older_than_source: Vec<String>,
    pub missing: Vec<String>,
}

/// Reads sources and headers of a project to follow local imports. Hashes
/// are remembered per (size, mtime) within one tracker.
pub struct DepTracker {
    root: PathBuf,
    profile: LexProfile,
    src_dirs: Vec<String>,
    memo: HashMap<String, Entry>,
}

/// Where Lake put the project's `.olean` files, if it was built: newer
/// Lake versions use `.lake/build/lib/lean`, older ones `.lake/build/lib`.
pub fn build_lib_dir(root: &Path) -> Option<PathBuf> {
    [".lake/build/lib/lean", ".lake/build/lib", "build/lib"].iter().map(|d| root.join(d)).find(|p| p.is_dir())
}

/// Lake source directories declared in `lakefile.toml` (read as data);
/// always includes the package root.
pub fn src_dirs(root: &Path) -> Vec<String> {
    let mut dirs = BTreeSet::new();
    dirs.insert(String::new());
    if let Some(t) = std::fs::read_to_string(root.join("lakefile.toml")).ok().and_then(|s| s.parse::<toml::Table>().ok()) {
        let mut add = |v: Option<&toml::Value>| {
            if let Some(s) = v.and_then(|v| v.as_str()) {
                let s = s.trim_matches('/').trim_start_matches("./");
                if !s.is_empty() && !s.split('/').any(|c| c == "..") {
                    dirs.insert(format!("{s}/"));
                }
            }
        };
        add(t.get("srcDir"));
        for lib in t.get("lean_lib").and_then(|l| l.as_array()).into_iter().flatten() {
            add(lib.get("srcDir"));
        }
    }
    dirs.into_iter().collect()
}

impl DepTracker {
    pub fn new(root: &Path, toolchain: Option<&str>) -> DepTracker {
        DepTracker { root: root.to_path_buf(), profile: LexProfile::for_toolchain(toolchain.unwrap_or("")), src_dirs: src_dirs(root), memo: HashMap::new() }
    }

    /// Forgets remembered hashes: the next check reads every file again.
    pub fn forget(&mut self) {
        self.memo.clear();
    }

    fn resolve_module(&self, module: &str) -> Option<String> {
        if module.contains('«') || module.contains('/') {
            return None;
        }
        let rel = module.replace('.', "/") + ".lean";
        self.src_dirs.iter().map(|d| format!("{d}{rel}")).find(|p| self.root.join(p).is_file())
    }

    /// Module path of a source file below its source directory
    /// (`A/B.lean` for `src/A/B.lean`).
    fn module_rel<'a>(&self, rel: &'a str) -> &'a str {
        self.src_dirs.iter().rev().find_map(|d| rel.strip_prefix(d.as_str())).unwrap_or(rel)
    }

    fn entry(&mut self, rel: &str, known_text: Option<&str>) -> Option<&Entry> {
        let path = self.root.join(rel);
        let md = std::fs::metadata(&path).ok()?;
        let (size, mt) = (md.len(), mtime_ns(&md));
        let reuse = self.memo.get(rel).is_some_and(|e| e.size == size && e.mtime_ns == mt);
        if reuse {
            self.memo.get_mut(rel).unwrap().hashed_this_call = false;
        } else {
            let owned;
            let text: &str = match known_text {
                Some(t) => t,
                None => {
                    if size > crate::sourceview::MAX_FILE_BYTES {
                        return None;
                    }
                    owned = String::from_utf8_lossy(&std::fs::read(&path).ok()?).into_owned();
                    &owned
                }
            };
            let imports = header_imports(text, self.profile).iter().filter_map(|m| self.resolve_module(m)).collect();
            self.memo.insert(rel.to_string(), Entry { size, mtime_ns: mt, sha256: sha256_hex(text.as_bytes()), imports, hashed_this_call: true });
        }
        self.memo.get(rel)
    }

    /// Transitive local imports of `rel`. `text` is the file's own content
    /// when the caller already holds it.
    pub fn closure(&mut self, rel: &str, text: Option<&str>) -> Closure {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut verified = true;
        let mut stack: Vec<String> = match self.entry(rel, text) {
            Some(e) => e.imports.clone(),
            None => Vec::new(),
        };
        let mut parts: Vec<(String, String)> = Vec::new();
        while let Some(f) = stack.pop() {
            if f == rel || !seen.insert(f.clone()) {
                continue;
            }
            match self.entry(&f, None) {
                Some(e) => {
                    verified &= e.hashed_this_call;
                    parts.push((f.clone(), e.sha256.clone()));
                    stack.extend(e.imports.iter().cloned());
                }
                None => parts.push((f.clone(), "missing".into())),
            }
        }
        parts.sort();
        let mut buf = String::new();
        for (p, h) in &parts {
            buf.push_str(p);
            buf.push('\0');
            buf.push_str(h);
            buf.push('\n');
        }
        Closure { files: seen.into_iter().collect(), digest: sha256_hex(buf.as_bytes()), content_verified: verified }
    }

    /// Path of the `.olean` Lake builds for a source file.
    pub fn olean_path(&self, rel: &str) -> PathBuf {
        let m = self.module_rel(rel);
        let lib = build_lib_dir(&self.root).unwrap_or_else(|| self.root.join(".lake/build/lib/lean"));
        lib.join(m.strip_suffix(".lean").unwrap_or(m).to_string() + ".olean")
    }

    pub fn has_build_dir(&self) -> bool {
        build_lib_dir(&self.root).is_some()
    }

    /// State of the compiled artifacts of `files` (metadata only).
    pub fn artifacts(&self, files: &[String]) -> Artifacts {
        if !self.has_build_dir() {
            return Artifacts::default();
        }
        let mut a = Artifacts::default();
        let mut buf = String::new();
        for f in files {
            let src = std::fs::metadata(self.root.join(f)).ok();
            match std::fs::metadata(self.olean_path(f)) {
                Ok(o) => {
                    buf.push_str(&format!("{f}\0{}\0{}\n", o.len(), mtime_ns(&o)));
                    if src.is_some_and(|s| mtime_ns(&s) > mtime_ns(&o)) {
                        a.older_than_source.push(f.clone());
                    }
                }
                Err(_) => {
                    buf.push_str(&format!("{f}\0missing\n"));
                    a.missing.push(f.clone());
                }
            }
        }
        a.digest = sha256_hex(buf.as_bytes());
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn header_imports_follow_the_header_syntax_only() {
        let p = LexProfile::for_toolchain("leanprover/lean4:v4.33.1");
        let t = "/- import Not.This -/\nmodule\n\npublic import A.B\nimport all C -- import D\nmeta import E.«F»\n\n/-- doc -/\ndef import := 1\nimport Late";
        assert_eq!(header_imports(t, p), vec!["A.B", "C", "E.«F»"]);
        assert_eq!(header_imports("prelude\nimport Init.Core\nopen X", p), vec!["Init.Core"]);
        assert!(header_imports("def x := 1\nimport A", p).is_empty());
        assert!(header_imports("", p).is_empty());
    }

    #[test]
    fn a_change_invalidates_importers_but_not_unrelated_modules() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(r, "lakefile.toml", "name = \"p\"\n[[lean_lib]]\nname = \"P\"\n");
        write(r, "P/A.lean", "import P.B\nimport Mathlib.Data.Nat.Basic\ndef a := b");
        write(r, "P/B.lean", "import P.C\ndef b := c");
        write(r, "P/C.lean", "def c := 1");
        write(r, "P/D.lean", "def d := 1");
        let mut t = DepTracker::new(r, None);
        let a0 = t.closure("P/A.lean", None);
        let d0 = t.closure("P/D.lean", None);
        // External imports (Mathlib) are not local files: they are bound by
        // the environment fingerprint instead.
        assert_eq!(a0.files, vec!["P/B.lean", "P/C.lean"]);
        assert!(a0.content_verified);
        assert!(d0.files.is_empty());
        // A second look reuses size+mtime: same digest, not content-verified.
        let a1 = t.closure("P/A.lean", None);
        assert_eq!(a1.digest, a0.digest);
        assert!(!a1.content_verified);
        // C changes: A and B are affected, D is not.
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(r, "P/C.lean", "def c := 22");
        assert_ne!(t.closure("P/A.lean", None).digest, a0.digest);
        assert_eq!(t.closure("P/D.lean", None).digest, d0.digest);
        // An import cycle terminates.
        write(r, "P/C.lean", "import P.A\ndef c := 1");
        assert_eq!(t.closure("P/A.lean", None).files, vec!["P/B.lean", "P/C.lean"]);
        // An exact rescan hashes everything again.
        t.forget();
        assert!(t.closure("P/A.lean", None).content_verified);
    }

    #[test]
    fn artifacts_report_stale_and_missing_oleans() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(r, "lakefile.toml", "name = \"p\"\nsrcDir = \"src\"\n");
        write(r, "src/P/B.lean", "def b := 1");
        write(r, "src/P/C.lean", "def c := 1");
        let t = DepTracker::new(r, None);
        assert_eq!(t.artifacts(&["src/P/B.lean".into()]), Artifacts::default(), "no build directory: unknown");
        write(r, ".lake/build/lib/lean/P/B.olean", "x");
        assert!(t.olean_path("src/P/B.lean").ends_with(".lake/build/lib/lean/P/B.olean"));
        let files = vec!["src/P/B.lean".to_string(), "src/P/C.lean".to_string()];
        let a = t.artifacts(&files);
        assert_eq!(a.missing, vec!["src/P/C.lean"]);
        assert!(a.older_than_source.is_empty());
        // B's source is edited after its .olean was built.
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(r, "src/P/B.lean", "def b := 2");
        let b = t.artifacts(&files);
        assert_eq!(b.older_than_source, vec!["src/P/B.lean"]);
        assert_eq!(b.digest, a.digest, "the artifact itself did not change");
    }

    #[test]
    fn environment_changes_are_named() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(r, "lean-toolchain", "leanprover/lean4:v0.0.0-not-installed\n");
        write(r, "lakefile.toml", "name = \"p\"\n");
        write(r, "lake-manifest.json", r#"{"packages":[{"name":"mathlib","rev":"abc"}]}"#);
        let e0 = EnvInfo::current(r);
        assert_eq!(e0.packages, vec![("mathlib".to_string(), "abc".to_string())]);
        assert!(e0.lean_exe.is_none());
        assert!(EnvInfo::current(r).changes_since(&e0).is_empty());
        write(r, "lake-manifest.json", r#"{"packages":[{"name":"mathlib","rev":"def"}]}"#);
        write(r, "lean-toolchain", "leanprover/lean4:v0.0.1-not-installed\n");
        let e1 = EnvInfo::current(r);
        assert_ne!(e1.fingerprint(), e0.fingerprint());
        let ch = e1.changes_since(&e0).join("; ");
        assert!(ch.contains("lean-toolchain changed") && ch.contains("lake-manifest.json changed"), "{ch}");
    }
}
