//! Lean's own reference information, read from the build artifacts.
//!
//! When Lake builds a module it writes, beside the `.olean`, an `.ilean`
//! file: for every constant the module refers to, the module that declares
//! it, its full name and the source range of each use, as the elaborator
//! resolved it. The `.trace` file beside it records what the artifacts were
//! built from, including a hash of the source file and of the `.ilean`.
//!
//! slim reads these as data. Nothing is run and nothing is written beside
//! them. A file's references are used only when Lake's own hashes say that
//! the `.ilean` was built from exactly the text being shown.

use crate::export::toolchain_dir;
use crate::highlight::env::{build_lib_dir, src_dirs};
use crate::model::{DeclKind, Origin};
use crate::skim::{Peek, SkimDecl};
use crate::sourceview::{SourceCache, SourceFileText};
use crate::text::PosEncoding;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// An `.ilean` larger than this is not read.
const MAX_ILEAN_BYTES: u64 = 64 << 20;

/// The 64-bit hash Lake records for a file's bytes: Lean's `hash` of them
/// (MurmurHash64A, seed 11) mixed into `Hash.nil` (1723). Checked against
/// the `.trace` files of Lean 4.33.1.
pub fn lake_hash(bytes: &[u8]) -> u64 {
    const M: u64 = 0xc6a4_a793_5bd1_e995;
    const R: u32 = 47;
    let mut h: u64 = 11 ^ (bytes.len() as u64).wrapping_mul(M);
    let mut chunks = bytes.chunks_exact(8);
    for c in &mut chunks {
        let mut k = u64::from_le_bytes(c.try_into().expect("eight bytes"));
        k = k.wrapping_mul(M);
        k ^= k >> R;
        k = k.wrapping_mul(M);
        h ^= k;
        h = h.wrapping_mul(M);
    }
    let tail = chunks.remainder();
    if !tail.is_empty() {
        let mut k = [0u8; 8];
        k[..tail.len()].copy_from_slice(tail);
        h ^= u64::from_le_bytes(k);
        h = h.wrapping_mul(M);
    }
    h ^= h >> R;
    h = h.wrapping_mul(M);
    h ^= h >> R;
    // `mixHash 1723 h`
    let mut k = h.wrapping_mul(M);
    k ^= k >> R;
    k ^= M;
    (1723u64 ^ k).wrapping_mul(M)
}

/// Lake's hash of a source file: of its text with `\r\n` read as `\n`.
pub fn lake_text_hash(text: &str) -> u64 {
    if text.contains("\r\n") {
        lake_hash(text.replace("\r\n", "\n").as_bytes())
    } else {
        lake_hash(text.as_bytes())
    }
}

/// Why a file's Lean references are used, or are not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefStatus {
    /// Lake's hashes tie the `.ilean` to the text shown.
    Verified,
    NoBuild,
    NoArtifact,
    NoTrace,
    /// The `.trace` has a layout this slim does not read.
    TraceFormat,
    SourceDiffers,
    ArtifactDiffers,
    Unreadable(String),
}

impl RefStatus {
    pub fn explain(&self) -> String {
        match self {
            RefStatus::Verified => "from the build artifacts (.ilean); Lake's hashes match this text".into(),
            RefStatus::NoBuild => "none: the project has no build directory".into(),
            RefStatus::NoArtifact => "none: no .ilean for this file".into(),
            RefStatus::NoTrace => "not used: no .trace beside the .ilean".into(),
            RefStatus::TraceFormat => "not used: the .trace of this Lake version is not read".into(),
            RefStatus::SourceDiffers => "not used: the file changed since it was built".into(),
            RefStatus::ArtifactDiffers => "not used: the .ilean is not the one its .trace records".into(),
            RefStatus::Unreadable(e) => format!("not used: {e}"),
        }
    }
}

/// One constant a file refers to, and where (byte ranges in the file).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// The module that declares it.
    pub module: String,
    pub name: String,
    /// In reading order.
    pub uses: Vec<(usize, usize)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileRefs {
    pub targets: Vec<Target>,
}

/// The Lake package a file belongs to: the nearest directory at or above
/// it (inside the opened root) that has a lakefile or a build directory.
/// A repository often keeps its package in a subdirectory.
#[derive(Clone, Debug)]
pub struct Package {
    pub dir: PathBuf,
    /// The package directory below the opened root: `""` or `"lean/"`.
    pub rel: String,
    lib: Option<PathBuf>,
    src_dirs: Vec<String>,
}

impl Package {
    /// `Dir/File` of a source file (given below the opened root).
    fn module_rel<'a>(&self, rel: &'a str) -> &'a str {
        module_rel(&self.src_dirs, rel.strip_prefix(self.rel.as_str()).unwrap_or(rel))
    }

    /// The Lean module name of a source file below the opened root.
    pub fn module_name(&self, rel: &str) -> String {
        self.module_rel(rel).replace('/', ".")
    }
}

/// What is remembered about build artifacts while an index is open.
#[derive(Default)]
pub struct LeanMemo {
    /// The package of the files of a directory (below the opened root).
    owners: HashMap<String, Arc<Package>>,
    /// Recently read files: path, content hash, what was found.
    files: VecDeque<(PathBuf, String, Option<Arc<FileRefs>>, RefStatus)>,
    /// The packages under `.lake/packages` (name, directory).
    packages: Option<(PathBuf, Vec<(String, PathBuf)>)>,
    /// Who uses what, over the whole project (read when first asked).
    index: Option<Arc<RefIndex>>,
}

/// The package that owns the file `rel` below `root`.
pub fn package_of(cache: &mut SourceCache, root: &Path, rel: &str) -> Arc<Package> {
    let dir = rel.rsplit_once('/').map(|x| x.0).unwrap_or("");
    if let Some(p) = cache.lean.owners.get(dir) {
        return p.clone();
    }
    // From the file's directory up to the opened root.
    let mut at = dir;
    let found = loop {
        let d = if at.is_empty() { root.to_path_buf() } else { root.join(at) };
        if d.join("lakefile.toml").is_file() || d.join("lakefile.lean").is_file() || build_lib_dir(&d).is_some() || at.is_empty() {
            break (d, if at.is_empty() { String::new() } else { format!("{at}/") });
        }
        at = at.rsplit_once('/').map(|x| x.0).unwrap_or("");
    };
    let p = Arc::new(Package { lib: build_lib_dir(&found.0), src_dirs: src_dirs(&found.0), dir: found.0, rel: found.1 });
    if cache.lean.owners.len() > 4096 {
        cache.lean.owners.clear();
    }
    cache.lean.owners.insert(dir.to_string(), p.clone());
    p
}

/// `Dir/File` of a source file below a Lake source directory.
fn module_rel<'a>(src_dirs: &[String], rel: &'a str) -> &'a str {
    let rel = rel.strip_suffix(".lean").unwrap_or(rel);
    src_dirs.iter().filter(|d| !d.is_empty()).filter_map(|d| rel.strip_prefix(d.as_str())).min_by_key(|r| r.len()).unwrap_or(rel)
}

/// The hash recorded for the module's source and for its `.ilean`.
fn trace_hashes(trace: &serde_json::Value) -> Option<(u64, Option<u64>)> {
    fn source(v: &serde_json::Value) -> Option<u64> {
        let a = v.as_array()?;
        if let [k, h] = a.as_slice() {
            if let (Some(k), Some(h)) = (k.as_str(), h.as_str()) {
                if k.ends_with(".lean") {
                    return u64::from_str_radix(h, 16).ok();
                }
            }
        }
        a.iter().find_map(source)
    }
    let src = source(trace.get("inputs")?)?;
    let ilean = trace.get("outputs").and_then(|o| o.get("i")).and_then(|i| i.as_str()).and_then(|s| u64::from_str_radix(s.strip_suffix(".ilean").unwrap_or(s), 16).ok());
    Some((src, ilean))
}

#[derive(serde::Deserialize)]
struct Ilean {
    #[serde(default)]
    references: HashMap<String, RefInfo>,
    #[serde(default)]
    decls: HashMap<String, Vec<u32>>,
}

#[derive(serde::Deserialize)]
struct RefInfo {
    #[serde(default)]
    definition: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    usages: Vec<Vec<serde_json::Value>>,
}

/// `{"c":{"m":"Module","n":"Name"}}`, in either key order.
fn ref_ident(key: &str) -> Option<(String, String)> {
    let v: serde_json::Value = serde_json::from_str(key).ok()?;
    let c = v.get("c")?;
    Some((c.get("m")?.as_str()?.to_string(), c.get("n")?.as_str()?.to_string()))
}

fn range_of(v: &[serde_json::Value]) -> Option<[usize; 4]> {
    let n = |k: usize| v.get(k).and_then(|x| x.as_u64()).map(|x| x as usize);
    Some([n(0)?, n(1)?, n(2)?, n(3)?])
}

/// A range of the `.ilean` (0-based lines, UTF-16 columns) as bytes of `f`.
fn bytes_of(f: &SourceFileText, r: [usize; 4]) -> Option<(usize, usize)> {
    if r[0] >= f.line_count() || r[2] >= f.line_count() {
        return None;
    }
    let a = f.byte_of(r[0] + 1, r[1], PosEncoding::Utf16);
    let z = f.byte_of(r[2] + 1, r[3], PosEncoding::Utf16);
    (a < z && z <= f.text.len() && f.text.is_char_boundary(a) && f.text.is_char_boundary(z)).then_some((a, z))
}

fn read_bounded(p: &Path) -> Result<Vec<u8>, String> {
    let md = std::fs::metadata(p).map_err(|e| e.to_string())?;
    if md.len() > MAX_ILEAN_BYTES {
        return Err(format!("{} is too large", p.display()));
    }
    std::fs::read(p).map_err(|e| e.to_string())
}

fn read_refs(lib: &Path, mrel: &str, f: &SourceFileText) -> (Option<FileRefs>, RefStatus) {
    let ilean_path = lib.join(format!("{mrel}.ilean"));
    if !ilean_path.is_file() {
        return (None, RefStatus::NoArtifact);
    }
    let Ok(trace) = std::fs::read(lib.join(format!("{mrel}.trace"))) else { return (None, RefStatus::NoTrace) };
    let Some((src_hash, ilean_hash)) = serde_json::from_slice::<serde_json::Value>(&trace).ok().as_ref().and_then(trace_hashes) else {
        return (None, RefStatus::TraceFormat);
    };
    if lake_text_hash(&f.text) != src_hash {
        return (None, RefStatus::SourceDiffers);
    }
    let bytes = match read_bounded(&ilean_path) {
        Ok(b) => b,
        Err(e) => return (None, RefStatus::Unreadable(e)),
    };
    if ilean_hash.is_some_and(|h| h != lake_hash(&bytes)) {
        return (None, RefStatus::ArtifactDiffers);
    }
    let il: Ilean = match serde_json::from_slice(&bytes) {
        Ok(x) => x,
        Err(e) => return (None, RefStatus::Unreadable(format!("the .ilean is not read: {e}"))),
    };
    let mut targets = Vec::with_capacity(il.references.len());
    for (k, info) in il.references {
        let Some((module, name)) = ref_ident(&k) else { continue };
        let mut uses: Vec<(usize, usize)> = info.usages.iter().filter_map(|u| bytes_of(f, range_of(u)?)).collect();
        if uses.is_empty() {
            continue;
        }
        uses.sort_unstable();
        uses.dedup();
        targets.push(Target { module, name, uses });
    }
    targets.sort_by(|a, b| a.uses[0].cmp(&b.uses[0]).then_with(|| a.name.cmp(&b.name)));
    (Some(FileRefs { targets }), RefStatus::Verified)
}

/// The constants the file `rel` (below `root`) refers to, as Lean resolved
/// them when the project was built, if the artifacts are of this text.
pub fn file_refs(root: &Path, rel: &str, f: &SourceFileText, cache: &mut SourceCache) -> (Option<Arc<FileRefs>>, RefStatus) {
    if let Some(pos) = cache.lean.files.iter().position(|e| e.0 == f.path && e.1 == f.sha256) {
        let e = cache.lean.files.remove(pos).expect("found");
        let out = (e.2.clone(), e.3.clone());
        cache.lean.files.push_front(e);
        return out;
    }
    let pkg = package_of(cache, root, rel);
    let (refs, status) = match &pkg.lib {
        None => (None, RefStatus::NoBuild),
        Some(lib) => read_refs(lib, pkg.module_rel(rel), f),
    };
    let refs = refs.map(Arc::new);
    cache.lean.files.push_front((f.path.clone(), f.sha256.clone(), refs.clone(), status.clone()));
    cache.lean.files.truncate(24);
    (refs, status)
}

/// Together, the `.ilean` files of a project larger than this are not read
/// in one go for the "used by" index.
pub const MAX_INDEX_BYTES: u64 = 96 << 20;

/// One use of a constant: the file it is in (an index into
/// [`RefIndex::files`]), the line, and the module that declares the constant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Use {
    file: u32,
    line: u32,
    declared_in: u32,
}

/// Every use of every constant in the project's files, as Lean resolved
/// them at build time: the `.ilean` files turned around.
#[derive(Debug, Default)]
pub struct RefIndex {
    /// Module ids of the index, for the files whose artifacts were read.
    files: Vec<i64>,
    modules: Vec<String>,
    by_name: HashMap<String, Vec<Use>>,
    /// Local files in the index, and how many had artifacts tied to their text.
    pub local_files: usize,
    pub read_files: usize,
    pub uses: usize,
    pub ms: f64,
    /// Why nothing was read, when nothing was.
    pub unavailable: Option<String>,
}

/// A declaration that uses the one asked about.
#[derive(Clone, Debug, PartialEq)]
pub struct User {
    pub row: crate::db::query::DeclRow,
    pub uses: u32,
    /// The first line it is used on.
    pub line: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsedBy {
    pub users: Vec<User>,
    /// Uses outside any indexed declaration (in a macro, an `example`).
    pub loose: u32,
    pub more: usize,
    pub local_files: usize,
    pub read_files: usize,
    pub unavailable: Option<String>,
}

fn build_index(conn: &rusqlite::Connection, root: &Path, cache: &mut SourceCache) -> anyhow::Result<RefIndex> {
    let t = std::time::Instant::now();
    let mut ix = RefIndex::default();
    let mut st = conn.prepare("SELECT id, path FROM modules WHERE is_local = 1 AND path IS NOT NULL ORDER BY id")?;
    let mods: Vec<(i64, String)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    ix.local_files = mods.len();
    // Each file with the build directory of the package it belongs to.
    let mods: Vec<(i64, String, PathBuf, String)> = mods
        .into_iter()
        .filter_map(|(id, rel)| {
            let pkg = package_of(cache, root, &rel);
            let mrel = pkg.module_rel(&rel).to_string();
            Some((id, rel, pkg.lib.clone()?, mrel))
        })
        .collect();
    if mods.is_empty() {
        ix.unavailable = Some("no package of this project has a build directory".into());
        return Ok(ix);
    }
    let total: u64 = mods.iter().filter_map(|(_, _, lib, mrel)| std::fs::metadata(lib.join(format!("{mrel}.ilean"))).ok()).map(|m| m.len()).sum();
    if total > MAX_INDEX_BYTES {
        ix.unavailable = Some(format!("its build artifacts hold {} MiB of references: too much to read in one go", total >> 20));
        return Ok(ix);
    }
    let mut module_ids: HashMap<String, u32> = HashMap::new();
    for (id, rel, lib, mrel) in mods {
        let ilean_path = lib.join(format!("{mrel}.ilean"));
        let Ok(bytes) = read_bounded(&ilean_path) else { continue };
        // Only artifacts Lake's hashes tie to the file as it is now.
        let Some((src_hash, ilean_hash)) = std::fs::read(lib.join(format!("{mrel}.trace"))).ok().and_then(|t| serde_json::from_slice::<serde_json::Value>(&t).ok()).as_ref().and_then(trace_hashes) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(root.join(&rel)) else { continue };
        if lake_text_hash(&text) != src_hash || ilean_hash.is_some_and(|h| h != lake_hash(&bytes)) {
            continue;
        }
        let Ok(il) = serde_json::from_slice::<Ilean>(&bytes) else { continue };
        let file = ix.files.len() as u32;
        ix.files.push(id);
        ix.read_files += 1;
        for (k, info) in il.references {
            let Some((module, name)) = ref_ident(&k) else { continue };
            if info.usages.is_empty() {
                continue;
            }
            let declared_in = match module_ids.get(&module) {
                Some(&m) => m,
                None => {
                    let m = ix.modules.len() as u32;
                    ix.modules.push(module.clone());
                    module_ids.insert(module, m);
                    m
                }
            };
            let v = ix.by_name.entry(private_stripped(&name).to_string()).or_default();
            for u in &info.usages {
                if let Some(l) = u.first().and_then(|x| x.as_u64()) {
                    v.push(Use { file, line: l as u32 + 1, declared_in });
                    ix.uses += 1;
                }
            }
        }
    }
    ix.ms = t.elapsed().as_secs_f64() * 1000.0;
    Ok(ix)
}

/// `_private.Module.Path.0.Ns.name` as it is written: `Ns.name`.
pub fn private_stripped(name: &str) -> &str {
    name.strip_prefix("_private.").and_then(|r| r.split_once(".0.")).map(|(_, n)| n).unwrap_or(name)
}

/// The project-wide index, read once per open index.
pub fn ref_index(conn: &rusqlite::Connection, root: &Path, cache: &mut SourceCache) -> anyhow::Result<Arc<RefIndex>> {
    if let Some(ix) = &cache.lean.index {
        return Ok(ix.clone());
    }
    let ix = Arc::new(build_index(conn, root, cache)?);
    cache.lean.index = Some(ix.clone());
    Ok(ix)
}

/// The declarations that use `name` (declared in `module`, when known), by
/// the lines Lean recorded: each use is given to the declaration of the
/// index whose lines hold it.
pub fn used_by(conn: &rusqlite::Connection, root: &Path, cache: &mut SourceCache, name: &str, module: Option<&str>, self_id: i64, limit: usize) -> anyhow::Result<UsedBy> {
    let ix = ref_index(conn, root, cache)?;
    let mut out = UsedBy { local_files: ix.local_files, read_files: ix.read_files, unavailable: ix.unavailable.clone(), ..Default::default() };
    let Some(all) = ix.by_name.get(name) else { return Ok(out) };
    // Two private declarations may share a name: the declaring module tells them apart.
    let want = module.and_then(|m| ix.modules.iter().position(|x| x == m)).map(|m| m as u32);
    let uses: Vec<&Use> = match want {
        Some(m) if all.iter().any(|u| u.declared_in == m) => all.iter().filter(|u| u.declared_in == m).collect(),
        _ => all.iter().collect(),
    };
    let mut st = conn.prepare_cached(
        "SELECT d.id, d.name, d.display, d.kind, d.origin, d.flags, m.name, d.end_line FROM decls d LEFT JOIN modules m ON m.id = d.module_id
          WHERE d.module_id = ?1 AND d.start_line <= ?2 ORDER BY d.start_line DESC, d.id DESC LIMIT 6",
    )?;
    let mut users: Vec<User> = Vec::new();
    let mut at: HashMap<i64, usize> = HashMap::new();
    for u in uses {
        let rows = st.query_map(rusqlite::params![ix.files[u.file as usize], u.line], |r| {
            Ok((
                crate::db::query::DeclRow {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    display: r.get(2)?,
                    kind: DeclKind::from_u8(r.get::<_, u8>(3)?),
                    origin: Origin::from_u8(r.get::<_, u8>(4)?),
                    flags: r.get::<_, i64>(5)? as u32,
                    module: r.get(6)?,
                },
                r.get::<_, Option<u32>>(7)?,
            ))
        })?;
        // The innermost declaration whose lines hold the use.
        let mut holder = None;
        for r in rows {
            let (row, end) = r?;
            if end.is_some_and(|e| e >= u.line) {
                holder = Some(row);
                break;
            }
        }
        match holder {
            Some(row) if row.id == self_id => {}
            Some(row) => match at.get(&row.id) {
                Some(&k) => {
                    users[k].uses += 1;
                    users[k].line = users[k].line.min(u.line);
                }
                None => {
                    at.insert(row.id, users.len());
                    users.push(User { row, uses: 1, line: u.line });
                }
            },
            None => out.loose += 1,
        }
    }
    users.sort_by(|a, b| a.row.module.cmp(&b.row.module).then(a.line.cmp(&b.line)));
    out.more = users.len().saturating_sub(limit);
    users.truncate(limit);
    out.users = users;
    Ok(out)
}

/// A stable stand-in for a row id: a declaration the index does not hold
/// (a library's, or one Lean generated) is known by its name alone.
pub fn synthetic_id(name: &str) -> i64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    -((h.finish() >> 2) as i64) - 1
}

fn last_component(name: &str) -> &str {
    let n = crate::model::name_components(name);
    n.last().copied().unwrap_or(name).trim_matches(['«', '»'])
}

/// Whether the text written at a use is the constant's own name (and not a
/// notation, an anonymous constructor or a projection such as `.1`).
pub fn written_as_name(written: &str, name: &str) -> bool {
    let w = written.trim_start_matches(['@', '.']);
    !w.is_empty() && last_component(w) == last_component(name)
}

/// Where the build artifacts and the source of a library module are.
struct LibModule {
    ilean: PathBuf,
    /// The directory `SourceCache` reads below, and the path below it.
    base: PathBuf,
    rel: String,
    /// Shown to the reader: `mathlib: Mathlib/Order/Basic.lean`.
    shown: String,
    /// The path below the project root, when the file lies inside it.
    in_project: Option<String>,
}

fn packages<'a>(memo: &'a mut LeanMemo, root: &Path) -> &'a [(String, PathBuf)] {
    if memo.packages.as_ref().is_none_or(|p| p.0 != root) {
        let mut v: Vec<(String, PathBuf)> = std::fs::read_dir(root.join(".lake/packages"))
            .map(|d| d.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()).map(|e| (e.file_name().to_string_lossy().into_owned(), e.path())).collect())
            .unwrap_or_default();
        v.sort();
        memo.packages = Some((root.to_path_buf(), v));
    }
    &memo.packages.as_ref().expect("just set").1
}

/// A module name as a path: every component is a plain file name.
fn module_path(module: &str) -> Option<String> {
    let comps = crate::model::name_components(module);
    let ok = |c: &&str| !c.is_empty() && *c != "." && *c != ".." && !c.contains(['/', '\\', '\0']);
    comps.iter().all(|c| ok(c)).then(|| comps.join("/"))
}

/// `pkg` is the package of the file the name was read in: its own build,
/// its dependencies under `.lake/packages`, and the toolchain it names.
fn locate(root: &Path, pkg: &Package, module: &str, cache: &mut SourceCache) -> Option<LibModule> {
    let mrel = module_path(module)?;
    let source_in = |dir: &Path| -> Option<String> { src_dirs(dir).into_iter().map(|d| format!("{d}{mrel}.lean")).find(|r| dir.join(r).is_file()) };
    // The package itself (a declaration the scanner did not index).
    if let Some(lib) = &pkg.lib {
        let ilean = lib.join(format!("{mrel}.ilean"));
        if ilean.is_file() {
            if let Some(rel) = source_in(&pkg.dir) {
                let below = format!("{}{rel}", pkg.rel);
                return Some(LibModule { ilean, base: root.to_path_buf(), shown: below.clone(), in_project: Some(below.clone()), rel: below });
            }
        }
    }
    for (name, dir) in packages(&mut cache.lean, &pkg.dir).to_vec() {
        let Some(lib) = build_lib_dir(&dir) else { continue };
        let ilean = lib.join(format!("{mrel}.ilean"));
        if !ilean.is_file() {
            continue;
        }
        let rel = source_in(&dir)?;
        let below = format!("{}.lake/packages/{name}/{rel}", pkg.rel);
        return Some(LibModule { ilean, base: root.to_path_buf(), shown: format!("{name}: {rel}"), in_project: Some(below.clone()), rel: below });
    }
    // Lean's own library, in the toolchain the package names.
    let toolchain = std::fs::read_to_string(pkg.dir.join("lean-toolchain")).ok();
    let tc = toolchain_dir(toolchain.as_deref()?.trim());
    let ilean = tc.join("lib/lean").join(format!("{mrel}.ilean"));
    let rel = format!("src/lean/{mrel}.lean");
    (ilean.is_file() && tc.join(&rel).is_file()).then(|| LibModule { ilean, base: tc, shown: format!("Lean: {mrel}.lean"), in_project: None, rel })
}

/// The card of a declaration the index does not hold: found through the
/// `.ilean` of the module that declares it, read from that module's source.
/// `seen_in` is the file (below `root`) whose references name it.
pub fn lib_peek(root: &Path, seen_in: &str, module: &str, name: &str, cache: &mut SourceCache) -> Result<Option<Peek>, String> {
    let pkg = package_of(cache, root, seen_in);
    let Some(lm) = locate(root, &pkg, module, cache) else { return Ok(None) };
    let il: Ilean = serde_json::from_slice(&read_bounded(&lm.ilean)?).map_err(|e| format!("{}: {e}", lm.ilean.display()))?;
    // Where its name is written: the selection range of its declaration.
    let sel = match il.decls.get(name) {
        Some(d) if d.len() >= 8 => Some([d[4] as usize, d[5] as usize, d[6] as usize, d[7] as usize]),
        _ => il.references.iter().find(|(k, _)| ref_ident(k).is_some_and(|(m, n)| m == module && n == name)).and_then(|(_, r)| range_of(r.definition.as_deref()?)),
    };
    let Some(sel) = sel else { return Ok(None) };
    let f = cache.get(&lm.base, &lm.rel).map_err(|e| format!("{e:#}"))?;
    let row = crate::db::query::DeclRow {
        id: synthetic_id(name),
        name: name.to_string(),
        display: name.to_string(),
        kind: DeclKind::Unknown,
        origin: if lm.base == root && !lm.rel.contains(".lake/") { Origin::SourceCandidate } else { Origin::Library },
        flags: 0,
        module: Some(module.to_string()),
    };
    let line = sel[0] as u32 + 1;
    let mut decl = SkimDecl { row, line: Some(line), sig: name.to_string(), sig_cut: false, doc: None, sorry: false, folded: 0 };
    // A library's artifacts carry no hash of its source (Mathlib's come
    // from a cache, Lean's with the toolchain): the recorded place must at
    // least hold a name.
    let is_name = |s: &str| {
        let mut lx = crate::source::lexer::Lexer::new(s, crate::source::lexer::LexProfile::NEWEST);
        lx.next_token().is_some_and(|t| t.kind == crate::source::lexer::TokKind::Ident && t.end as usize == s.len())
    };
    let Some((a, z)) = bytes_of(&f, sel).filter(|&(a, z)| is_name(&f.text[a..z])) else {
        decl.doc = Some("its source is not the text its build artifacts were made from".into());
        return Ok(Some(Peek { decl, path: Some(lm.shown), source: None }));
    };
    let written = &f.text[a..z];
    // The statement and the documentation, as the scanner reads the file.
    let scan = crate::source::scan::scan_source(&f.text, crate::source::lexer::LexProfile::NEWEST);
    let col = a - f.line_starts.get(line as usize - 1).copied().unwrap_or(0);
    // The declaration whose name stands there; else the one the place lies
    // in: an attribute (`to_additive`, `simps`) or a macro made this name
    // from that declaration, and Lean records the attribute's place.
    let own = scan.decls.iter().find(|c| c.name_line == line && c.name_col as usize == col).filter(|_| written_as_name(written, name));
    let host = || scan.decls.iter().filter(|c| (c.start as usize) <= a && a < c.end as usize).min_by_key(|c| c.end - c.start);
    match own.or_else(host) {
        Some(c) => {
            let (kw, he) = (c.kw as usize, (c.header_end as usize).max(c.kw as usize));
            let (sig, cut) = crate::skim::cut_statement(f.text.get(kw..he).unwrap_or(name).trim_end());
            decl.sig = sig;
            decl.sig_cut = cut;
            decl.row.kind = c.kind;
            decl.folded = c.end_line.saturating_sub(f.line_of_byte(he.saturating_sub(1)) as u32);
            decl.doc = match own {
                Some(_) => c.doc.as_deref().and_then(crate::skim::doc_lead),
                None => {
                    let from = last_component(&c.name);
                    let by = if written == from { String::new() } else { format!(" by `{written}`") };
                    Some(format!("made from `{from}`{by}: the statement shown is that of `{from}`"))
                }
            };
        }
        // Nothing the scanner knows stands there: the line as written.
        None => decl.sig = f.line(line as usize).trim().to_string(),
    }
    Ok(Some(Peek { decl, path: Some(lm.shown), source: lm.in_project.map(|p| (p, line as usize)) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_lakes() {
        assert_eq!(lake_text_hash("a\r\nb\n"), lake_text_hash("a\nb\n"));
        assert_ne!(lake_text_hash("a\nb\n"), lake_text_hash("a\nb"));
        // From an independent implementation of the same hash that
        // reproduced the source hashes Lake 4.33.1 recorded for 1,373 files.
        assert_eq!(format!("{:016x}", lake_text_hash(SAMPLE)), SAMPLE_HASH);
    }

    const SAMPLE: &str = "theorem t : True := trivial\n";
    const SAMPLE_HASH: &str = "b5b2382a4334fbf8";

    #[test]
    fn names_and_notations_are_told_apart() {
        assert!(written_as_name("sum_comm", "Finset.sum_comm"));
        assert!(written_as_name("Finset.sum_comm", "Finset.sum_comm"));
        assert!(written_as_name("h.symm", "Eq.symm"));
        assert!(written_as_name("@foo", "N.foo"));
        assert!(!written_as_name("1", "And.left"));
        assert!(!written_as_name("⟨", "And.intro"));
        assert!(!written_as_name("a + b", "«term_+_»"));
        assert_eq!(module_path("Mathlib.Order.Basic").as_deref(), Some("Mathlib/Order/Basic"));
        assert!(module_path("A...B").is_none() && module_path("A.«../x»").is_none());
        assert!(synthetic_id("Nat.add") < 0 && synthetic_id("Nat.add") == synthetic_id("Nat.add"));
    }
}
