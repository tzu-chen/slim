//! Locations of caches and user data, project identity, and safe resolution
//! of source paths inside an approved root.
//!
//! Generated files never go into the inspected project: indexes live under
//! the cache directory (`$SLIM_CACHE_DIR`, else `$XDG_CACHE_HOME/slim`,
//! else `~/.cache/slim`), and bookmarks/trails under the data directory
//! (`$SLIM_DATA_DIR`, else `$XDG_DATA_HOME/slim`, else
//! `~/.local/share/slim`).

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"))
}

pub fn cache_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("SLIM_CACHE_DIR") {
        return PathBuf::from(p);
    }
    if let Some(p) = std::env::var_os("XDG_CACHE_HOME").filter(|p| !p.is_empty()) {
        return PathBuf::from(p).join("slim");
    }
    home().join(".cache").join("slim")
}

pub fn data_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("SLIM_DATA_DIR") {
        return PathBuf::from(p);
    }
    if let Some(p) = std::env::var_os("XDG_DATA_HOME").filter(|p| !p.is_empty()) {
        return PathBuf::from(p).join("slim");
    }
    home().join(".local").join("share").join("slim")
}

pub fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Identity of a project directory as seen by slim.
#[derive(Clone, Debug)]
pub struct Project {
    pub root: PathBuf,
    /// Lake package name if found in `lakefile.toml`/`lakefile.lean`
    /// (read as text, never evaluated), else the directory name.
    pub name: String,
    pub toolchain: Option<String>,
    /// Stable key for cache directories: name + hash of the canonical root.
    pub key: String,
    pub lake_manifest_sha256: Option<String>,
}

impl Project {
    pub fn open(root: &Path) -> Result<Project> {
        let root = root.canonicalize().with_context(|| format!("project root {}", root.display()))?;
        if !root.is_dir() {
            bail!("{} is not a directory", root.display());
        }
        let dir_name = root.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "root".into());
        let name = read_package_name(&root).unwrap_or(dir_name);
        let toolchain = std::fs::read_to_string(root.join("lean-toolchain")).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && s.len() < 200);
        let lake_manifest_sha256 = std::fs::read(root.join("lake-manifest.json")).ok().map(|b| sha256_hex(&b));
        let key = project_key(&name, &root);
        Ok(Project { root, name, toolchain, key, lake_manifest_sha256 })
    }

    pub fn cache_dir(&self) -> PathBuf {
        cache_dir().join("projects").join(&self.key)
    }
    pub fn source_index(&self) -> PathBuf {
        self.cache_dir().join("source.sqlite")
    }
    pub fn semantic_index(&self) -> PathBuf {
        self.cache_dir().join("semantic.sqlite")
    }
    /// Project identity used for user data (bookmarks survive moving the checkout).
    pub fn user_key(&self) -> String {
        format!("lake:{}", self.name)
    }
}

pub fn project_key(name: &str, root: &Path) -> String {
    let safe: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .take(40)
        .collect();
    let h = sha256_hex(root.as_os_str().as_encoded_bytes());
    format!("{}-{}", if safe.is_empty() { "project" } else { &safe }, &h[..12])
}

/// Reads `name = "..."` from `lakefile.toml`, or `package «name»` /
/// `package name` from `lakefile.lean`, as plain text.
pub fn read_package_name(root: &Path) -> Option<String> {
    if let Ok(t) = std::fs::read_to_string(root.join("lakefile.toml")) {
        for line in t.lines() {
            let l = line.trim();
            if l.starts_with('[') {
                break; // only top-level keys
            }
            if let Some(rest) = l.strip_prefix("name") {
                let rest = rest.trim_start();
                if let Some(v) = rest.strip_prefix('=') {
                    let v = v.trim().trim_matches('"').trim_matches('\'');
                    if !v.is_empty() && v.len() < 100 {
                        return Some(v.to_string());
                    }
                }
            }
        }
    }
    if let Ok(t) = std::fs::read_to_string(root.join("lakefile.lean")) {
        for line in t.lines() {
            let l = line.trim();
            if let Some(rest) = l.strip_prefix("package ") {
                let v: String = rest
                    .trim()
                    .trim_start_matches('«')
                    .chars()
                    .take_while(|c| !c.is_whitespace() && *c != '»' && *c != '{' && *c != '(' && *c != ':')
                    .collect();
                let v = v.trim_matches('"').to_string();
                if !v.is_empty() && v.len() < 100 {
                    return Some(v);
                }
            }
        }
    }
    None
}

/// Resolves `rel` (a relative, forward-slash path from an index or
/// snapshot) inside `root`, refusing traversal and symlinks that escape the
/// root. Returns the canonical path.
pub fn resolve_contained(root: &Path, rel: &str) -> Result<PathBuf> {
    let canon_root = root.canonicalize().with_context(|| format!("source root {}", root.display()))?;
    resolve_in_canonical_root(&canon_root, rel)
}

/// Like [`resolve_contained`] for a root that is already canonical.
pub fn resolve_in_canonical_root(canon_root: &Path, rel: &str) -> Result<PathBuf> {
    crate::snapshot::check_rel_path(rel)?;
    for c in Path::new(rel).components() {
        if !matches!(c, Component::Normal(_)) {
            bail!("rejected path component in {rel:?}");
        }
    }
    let joined = canon_root.join(rel);
    let canon = joined.canonicalize().with_context(|| format!("source file {}", joined.display()))?;
    if !canon.starts_with(canon_root) {
        bail!("{rel:?} resolves outside the approved source root (symlink?)");
    }
    Ok(canon)
}

/// The commit a checkout is at, read from `.git` as plain text (git is never
/// run). `None` when there is no readable git metadata. It says nothing about
/// uncommitted changes.
pub fn git_revision(root: &Path) -> Option<String> {
    let mut dir = Some(root);
    let git = loop {
        let d = dir?;
        let g = d.join(".git");
        if g.exists() {
            break g;
        }
        dir = d.parent();
    };
    // A worktree or submodule has a `.git` file pointing at the real directory.
    let git = if git.is_file() {
        let t = std::fs::read_to_string(&git).ok()?;
        let p = t.trim().strip_prefix("gitdir:")?.trim();
        let p = Path::new(p);
        if p.is_absolute() { p.to_path_buf() } else { git.parent()?.join(p) }
    } else {
        git
    };
    let is_hash = |s: &str| (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit());
    let head = std::fs::read_to_string(git.join("HEAD")).ok()?;
    let head = head.trim();
    let Some(r) = head.strip_prefix("ref:") else {
        return is_hash(head).then(|| head.to_string());
    };
    let r = r.trim();
    if r.contains("..") || r.starts_with('/') {
        return None;
    }
    // Worktrees keep refs in the common directory.
    let common = std::fs::read_to_string(git.join("commondir")).ok().map(|c| git.join(c.trim())).unwrap_or_else(|| git.clone());
    for base in [&git, &common] {
        if let Ok(h) = std::fs::read_to_string(base.join(r)) {
            let h = h.trim();
            if is_hash(h) {
                return Some(h.to_string());
            }
        }
    }
    let packed = std::fs::read_to_string(common.join("packed-refs")).ok()?;
    packed.lines().find_map(|l| {
        let (h, name) = l.split_once(' ')?;
        (name.trim() == r && is_hash(h)).then(|| h.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_names_are_read_as_text() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("lakefile.toml"), "name = \"cfsg\"\n[[lean_lib]]\nname = \"Other\"\n").unwrap();
        assert_eq!(read_package_name(d.path()).as_deref(), Some("cfsg"));
        let e = tempfile::tempdir().unwrap();
        std::fs::write(e.path().join("lakefile.lean"), "import Lake\nopen Lake DSL\npackage «my-pkg» where\n").unwrap();
        assert_eq!(read_package_name(e.path()).as_deref(), Some("my-pkg"));
    }

    #[test]
    fn containment() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("root");
        std::fs::create_dir_all(root.join("A")).unwrap();
        std::fs::write(root.join("A/B.lean"), "x").unwrap();
        let outside = d.path().join("secret.lean");
        std::fs::write(&outside, "secret").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("A/Link.lean")).unwrap();
        assert!(resolve_contained(&root, "A/B.lean").is_ok());
        assert!(resolve_contained(&root, "../secret.lean").is_err());
        assert!(resolve_contained(&root, "A/Link.lean").is_err());
        assert!(resolve_contained(&root, "/etc/passwd").is_err());
    }

    #[test]
    fn git_revision_is_read_as_text() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(git_revision(d.path()), None);
        let g = d.path().join(".git");
        std::fs::create_dir_all(g.join("refs/heads")).unwrap();
        let h = "0123456789abcdef0123456789abcdef01234567";
        std::fs::write(g.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        assert_eq!(git_revision(d.path()), None, "unborn branch");
        std::fs::write(g.join("refs/heads/main"), format!("{h}\n")).unwrap();
        assert_eq!(git_revision(d.path()).as_deref(), Some(h));
        // Found from a subdirectory; packed refs; detached HEAD; hostile ref.
        let sub = d.path().join("a/b");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(git_revision(&sub).as_deref(), Some(h));
        std::fs::remove_file(g.join("refs/heads/main")).unwrap();
        std::fs::write(g.join("packed-refs"), format!("# pack-refs\n{h} refs/heads/main\n")).unwrap();
        assert_eq!(git_revision(d.path()).as_deref(), Some(h));
        std::fs::write(g.join("HEAD"), format!("{h}\n")).unwrap();
        assert_eq!(git_revision(d.path()).as_deref(), Some(h));
        std::fs::write(g.join("HEAD"), "ref: ../../etc/passwd\n").unwrap();
        assert_eq!(git_revision(d.path()), None);
    }

    #[test]
    fn keys_differ_by_location() {
        let a = project_key("x", Path::new("/a/x"));
        let b = project_key("x", Path::new("/b/x"));
        assert_ne!(a, b);
        assert!(a.starts_with("x-"));
    }
}
