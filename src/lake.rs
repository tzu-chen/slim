//! Where Lake says a source file's module lives.
//!
//! The index names a module by its path below the opened root
//! (`lean.src.Formalism.Basic`), which is unique and is what maps, trails
//! and bookmarks refer to. Lean names the same module below the source
//! directory of its Lake package (`Formalism.Basic`): that is the name
//! `import` lines, `.ilean` files and other tools use. This module finds
//! the package of a file and gives its Lake name. Lake configuration is
//! read as text and never evaluated.

use crate::highlight::env::src_dirs;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A Lake package inside the opened root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pkg {
    /// The package directory below the opened root: `""` or `"lean/"`.
    pub rel: String,
    /// Its source directories below `rel`: `""` and, say, `"src/"`.
    pub src_dirs: Vec<String>,
    /// False when no lakefile was found and the opened root stands in.
    pub lakefile: bool,
}

impl Pkg {
    /// `Dir/File` below the package's source directory, without `.lean`.
    pub fn module_rel<'a>(&self, rel: &'a str) -> &'a str {
        let rel = rel.strip_prefix(self.rel.as_str()).unwrap_or(rel);
        let rel = rel.strip_suffix(".lean").unwrap_or(rel);
        self.src_dirs.iter().filter(|d| !d.is_empty()).filter_map(|d| rel.strip_prefix(d.as_str())).min_by_key(|r| r.len()).unwrap_or(rel)
    }

    /// The module name Lean uses for a file given below the opened root.
    pub fn lake_name(&self, rel: &str) -> String {
        self.module_rel(rel).replace('/', ".")
    }

    /// The prefix the index puts before a Lake name of this package, when
    /// it has one source directory: `"lean.src."`.
    pub fn index_prefix(&self) -> String {
        let src = self.src_dirs.iter().find(|d| !d.is_empty()).map(|s| s.as_str()).unwrap_or("");
        format!("{}{}", self.rel, src).replace('/', ".")
    }
}

/// The packages of an opened root, found once per directory.
pub struct Layout {
    root: PathBuf,
    owners: HashMap<String, Arc<Pkg>>,
}

impl Layout {
    pub fn new(root: &Path) -> Layout {
        Layout { root: root.to_path_buf(), owners: HashMap::new() }
    }

    /// The package that owns the file `rel` (below the root): the nearest
    /// directory at or above it with a lakefile, else the root itself.
    pub fn package_of(&mut self, rel: &str) -> Arc<Pkg> {
        let dir = rel.rsplit_once('/').map(|x| x.0).unwrap_or("");
        if let Some(p) = self.owners.get(dir) {
            return p.clone();
        }
        let mut at = dir;
        let mut climbed: Vec<&str> = Vec::new();
        let pkg = loop {
            if let Some(p) = self.owners.get(at) {
                break p.clone();
            }
            climbed.push(at);
            let d = if at.is_empty() { self.root.clone() } else { self.root.join(at) };
            let lakefile = d.join("lakefile.toml").is_file() || d.join("lakefile.lean").is_file();
            if lakefile || at.is_empty() {
                break Arc::new(Pkg { rel: if at.is_empty() { String::new() } else { format!("{at}/") }, src_dirs: src_dirs(&d), lakefile });
            }
            at = at.rsplit_once('/').map(|x| x.0).unwrap_or("");
        };
        for c in climbed {
            self.owners.insert(c.to_string(), pkg.clone());
        }
        pkg
    }

    pub fn lake_name(&mut self, rel: &str) -> String {
        self.package_of(rel).lake_name(rel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nested_package_with_a_source_directory_names_its_modules_as_lake_does() {
        let d = tempfile::tempdir().unwrap();
        let w = |rel: &str, text: &str| {
            let p = d.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        w("lean/lakefile.toml", "name = \"tree\"\nsrcDir = \"src\"\n");
        w("lean/src/A/B.lean", "");
        w("notes/C.lean", "");
        w("other/lakefile.lean", "package other where\n  srcDir := \"lib\"\n");
        w("other/lib/D.lean", "");
        let mut l = Layout::new(d.path());
        assert_eq!(l.lake_name("lean/src/A/B.lean"), "A.B");
        assert_eq!(l.package_of("lean/src/A/B.lean").index_prefix(), "lean.src.");
        // No lakefile above it: the path below the root, as before.
        assert_eq!(l.lake_name("notes/C.lean"), "notes.C");
        assert!(!l.package_of("notes/C.lean").lakefile);
        assert_eq!(l.lake_name("other/lib/D.lean"), "D");
    }
}
