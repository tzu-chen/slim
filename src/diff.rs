//! Two editions of a project compared by declaration: what was added,
//! what was removed, whose statement changed and whose proof only.
//!
//! A long machine-written proof is expected to be shortened and published
//! again. A reader's glosses, trails and map attach to full names, so the
//! comparison also says which annotations still mean what they meant: one
//! on a declaration whose statement changed needs reading again; one on a
//! declaration whose proof alone changed stands.
//!
//! Everything is read from two source-only indexes and the files they
//! describe. Statements and proofs are compared as text with white space
//! collapsed: a reformatted declaration is unchanged, a renamed binder is
//! a change.

use crate::cli::Args;
use crate::model::DeclKind;
use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub kind: DeclKind,
    pub module: String,
    pub line: u32,
    stmt: u64,
    body: u64,
}

fn squash_hash(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for w in s.split_whitespace() {
        w.hash(&mut h);
    }
    h.finish()
}

/// Every declaration of an index by full name, with a hash of its
/// statement and of what follows it.
pub fn load_side(index: &Path, root: &Path) -> Result<BTreeMap<String, Vec<Entry>>> {
    let conn = crate::db::open_read(index)?;
    if crate::db::mode_of(&conn)? != crate::db::IndexMode::Source {
        bail!("{} is not a source-only index: editions are compared as text", index.display());
    }
    // Lake's module name where the index records one: an archive unpacked
    // one directory deeper is the same edition.
    let mut st = conn.prepare(
        "SELECT d.name, d.kind, coalesce(l.name, m.name), m.path, d.sel_line, d.kw_byte, d.header_end_byte, d.end_byte
           FROM decls d JOIN modules m ON m.id = d.module_id LEFT JOIN module_lake l ON l.module_id = m.id
          WHERE m.is_local = 1 AND m.path IS NOT NULL AND d.kw_byte IS NOT NULL AND d.end_byte IS NOT NULL ORDER BY m.path, d.kw_byte",
    )?;
    let mut rows = st.query([])?;
    let mut out: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    let mut cur: Option<(String, String)> = None;
    while let Some(r) = rows.next()? {
        let (name, kind, module, path): (String, u8, String, String) = (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
        let (line, kw, hend, end): (Option<u32>, u32, Option<u32>, u32) = (r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?);
        if cur.as_ref().is_none_or(|c| c.0 != path) {
            let bytes = std::fs::read(root.join(&path)).unwrap_or_default();
            cur = Some((path.clone(), crate::text::decode_lossy(&bytes).0.into_owned()));
        }
        let text = &cur.as_ref().expect("just set").1;
        let (kw, end) = (kw as usize, (end as usize).min(text.len()));
        let hend = hend.map(|h| (h as usize).clamp(kw, end)).unwrap_or(end);
        // What a definition states is all of it; a theorem's statement
        // ends where its proof begins.
        let theorem = DeclKind::from_u8(kind) == DeclKind::Theorem;
        let (stmt, body) = match (text.get(kw..hend), text.get(hend..end), text.get(kw..end)) {
            (Some(a), Some(b), _) if theorem => (squash_hash(a), squash_hash(b)),
            (_, _, Some(all)) => (squash_hash(all), 0),
            _ => (0, 0),
        };
        out.entry(name).or_default().push(Entry { kind: DeclKind::from_u8(kind), module, line: line.unwrap_or(1), stmt, body });
    }
    Ok(out)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Change {
    Removed,
    Statement,
    Proof,
    Moved,
    Added,
    Same,
}

impl Change {
    pub fn label(self) -> &'static str {
        match self {
            Change::Removed => "removed",
            Change::Statement => "statement or definition changed",
            Change::Proof => "proof or body changed, statement the same",
            Change::Moved => "moved to another file, text the same",
            Change::Added => "added",
            Change::Same => "unchanged",
        }
    }
}

/// How each name fares from `a` to `b`. Of several declarations with one
/// name, each is matched with the like one on the other side.
pub fn compare(a: &BTreeMap<String, Vec<Entry>>, b: &BTreeMap<String, Vec<Entry>>) -> BTreeMap<String, Change> {
    let mut out = BTreeMap::new();
    for (name, old) in a {
        let ch = match b.get(name) {
            None => Change::Removed,
            Some(new) => {
                let key = |e: &Entry| (e.stmt, e.body);
                let mut x: Vec<_> = old.iter().map(key).collect();
                let mut y: Vec<_> = new.iter().map(key).collect();
                x.sort_unstable();
                y.sort_unstable();
                if x == y {
                    let mut ma: Vec<&str> = old.iter().map(|e| e.module.as_str()).collect();
                    let mut mb: Vec<&str> = new.iter().map(|e| e.module.as_str()).collect();
                    ma.sort_unstable();
                    mb.sort_unstable();
                    if ma == mb { Change::Same } else { Change::Moved }
                } else {
                    let mut sx: Vec<u64> = old.iter().map(|e| e.stmt).collect();
                    let mut sy: Vec<u64> = new.iter().map(|e| e.stmt).collect();
                    sx.sort_unstable();
                    sy.sort_unstable();
                    if sx == sy { Change::Proof } else { Change::Statement }
                }
            }
        };
        out.insert(name.clone(), ch);
    }
    for name in b.keys() {
        out.entry(name.clone()).or_insert(Change::Added);
    }
    out
}

/// `slim diff OLD_DIR NEW_DIR [--limit N] [--map FILE]`.
pub fn cmd_diff(args: &Args) -> Result<i32> {
    let (Some(old), Some(new)) = (args.positional.first(), args.positional.get(1)) else {
        bail!("usage: slim diff <OLD_PROJECT_DIR> <NEW_PROJECT_DIR> [--limit N] [--map FILE]");
    };
    let side = |dir: &str| -> Result<(crate::paths::Project, BTreeMap<String, Vec<Entry>>)> {
        let p = crate::paths::Project::open(Path::new(dir))?;
        let idx = p.source_index();
        if !idx.exists() {
            bail!("no source index for {} yet; run `slim scan {}`", p.name, p.root.display());
        }
        let s = load_side(&idx, &p.root).with_context(|| format!("reading {}", p.root.display()))?;
        Ok((p, s))
    };
    let (pa, a) = side(old)?;
    let (pb, b) = side(new)?;
    let ch = compare(&a, &b);
    let mut counts: HashMap<Change, usize> = HashMap::new();
    for c in ch.values() {
        *counts.entry(*c).or_default() += 1;
    }
    println!("{} ({} names) → {} ({} names), compared by full name and text with white space collapsed", pa.root.display(), a.len(), pb.root.display(), b.len());
    for c in [Change::Same, Change::Moved, Change::Proof, Change::Statement, Change::Removed, Change::Added] {
        println!("  {:>9}  {}", counts.get(&c).copied().unwrap_or(0), c.label());
    }
    let limit = args.num::<usize>("limit")?.unwrap_or(15);
    for c in [Change::Statement, Change::Removed, Change::Added, Change::Proof] {
        let names: Vec<&String> = ch.iter().filter(|(_, x)| **x == c).map(|(n, _)| n).collect();
        if names.is_empty() {
            continue;
        }
        println!("\n{} ({}):", c.label(), names.len());
        for n in names.iter().take(limit) {
            let e = b.get(*n).or_else(|| a.get(*n)).and_then(|v| v.first());
            println!("  {} {}  [{}]", e.map(|e| e.kind.short()).unwrap_or("     "), n, e.map(|e| e.module.as_str()).unwrap_or("-"));
        }
        if names.len() > limit {
            println!("  … {} more (--limit N)", names.len() - limit);
        }
    }
    // What the reader's own notes and the project map say about names that changed.
    let mut noted: Vec<(String, &'static str)> = Vec::new();
    if let Ok(store) = crate::userdata::UserStore::open() {
        for key in [pa.user_key(), pb.user_key()] {
            for (name, _) in store.glosses(&key).unwrap_or_default() {
                noted.push((name, "gloss"));
            }
            for name in store.bookmark_names(&key).unwrap_or_default() {
                noted.push((name, "bookmark"));
            }
        }
    }
    let map = match args.path("map") {
        Some(p) => crate::map::load(&p).map().cloned(),
        None => crate::map::load_for_root(&pb.root).map().cloned().or_else(|| crate::map::load_for_root(&pa.root).map().cloned()),
    };
    if let Some(m) = &map {
        for (name, _) in m.decl_refs() {
            noted.push((name, "map"));
        }
    }
    noted.sort();
    noted.dedup();
    if !noted.is_empty() {
        let mut by: BTreeMap<Change, Vec<String>> = BTreeMap::new();
        for (name, what) in &noted {
            let c = ch.get(name).copied().unwrap_or(Change::Removed);
            by.entry(c).or_default().push(format!("{name} ({what})"));
        }
        println!("\nannotations (glosses, bookmarks, map entries): {} names", noted.len());
        for (c, names) in by {
            match c {
                Change::Same | Change::Moved | Change::Proof => println!("  {} stand: {}", names.len(), c.label()),
                _ => {
                    println!("  {} to read again: {}", names.len(), c.label());
                    for n in names.iter().take(limit) {
                        println!("    {n}");
                    }
                }
            }
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(module: &str, stmt: &str, body: &str) -> Entry {
        Entry { kind: DeclKind::Theorem, module: module.into(), line: 1, stmt: squash_hash(stmt), body: squash_hash(body) }
    }

    #[test]
    fn names_are_classified_by_what_changed() {
        let mut a = BTreeMap::new();
        let mut b = BTreeMap::new();
        a.insert("same".to_string(), vec![e("M", "theorem same : P", ":= by simp")]);
        b.insert("same".to_string(), vec![e("M", "theorem same :   P", ":= by\n  simp")]);
        a.insert("moved".to_string(), vec![e("M", "s", "b")]);
        b.insert("moved".to_string(), vec![e("N", "s", "b")]);
        a.insert("proof".to_string(), vec![e("M", "s", "b")]);
        b.insert("proof".to_string(), vec![e("M", "s", "shorter")]);
        a.insert("stmt".to_string(), vec![e("M", "s", "b")]);
        b.insert("stmt".to_string(), vec![e("M", "s'", "b")]);
        a.insert("gone".to_string(), vec![e("M", "s", "b")]);
        b.insert("new".to_string(), vec![e("M", "s", "b")]);
        let c = compare(&a, &b);
        let got: Vec<(&str, Change)> = c.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        assert_eq!(got, vec![("gone", Change::Removed), ("moved", Change::Moved), ("new", Change::Added), ("proof", Change::Proof), ("same", Change::Same), ("stmt", Change::Statement)]);
    }
}
