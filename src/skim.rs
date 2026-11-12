//! Skimming: the declarations of a file, a directory, a namespace or a map
//! group as statements with the first line of their documentation, proofs
//! and bodies left out.
//!
//! In a source-only index a statement is the text of the declaration header
//! as written (keyword up to `:=`, `where` or the first `|`); in a semantic
//! index it is the elaborated signature. A skim is bounded: it stops after
//! [`MAX_DECLS`] declarations or [`MAX_FILES`] files and says so.

use crate::db::query::{DeclRow, ModStats, Signature};
use crate::db::IndexMode;
use crate::map::ProjectMap;
use crate::model::{flags, DeclKind, Origin};
use crate::snapshot::Binder;
use crate::source::scope::{CmdKind, EventKind};
use crate::sourceview::{SourceCache, SourceFileText};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

pub const MAX_DECLS: usize = 1500;
pub const MAX_FILES: usize = 150;
/// A longer statement is cut here (Enter shows all of it).
pub const MAX_SIG_BYTES: usize = 1600;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SkimScope {
    /// One file, by module name.
    Module(String),
    /// Every file at or below a dotted module-name prefix.
    Dir(String),
    /// The declaration named so and everything below it.
    Namespace(String),
    /// The landmarks of a group of the project map (and of its subgroups).
    Group(String),
}

impl SkimScope {
    pub fn label(&self) -> String {
        match self {
            SkimScope::Module(m) => format!("file {m}"),
            SkimScope::Dir(d) => format!("files under {d}"),
            SkimScope::Namespace(n) => format!("namespace {n}"),
            SkimScope::Group(g) => format!("map group {g}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SkimDecl {
    pub row: DeclRow,
    pub line: Option<u32>,
    /// The statement, possibly several lines.
    pub sig: String,
    pub sig_cut: bool,
    /// The lead of the documentation (its first paragraph, cut).
    pub doc: Option<String>,
    /// Source-only: a `sorry`/`admit` token occurs in its text.
    pub sorry: bool,
    /// Lines left out below the statement (source-only; 0 unknown).
    pub folded: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SkimItem {
    /// A file (or a map group) the following declarations belong to.
    Header { title: String, sub: String, doc: Option<String>, warn: Option<String> },
    Decl(SkimDecl),
    Note { text: String, warn: bool },
    /// A module-doc block between declarations: its headings and lead.
    Prose(crate::scope::Prose),
    /// A `variable`, `open` or closing `end` line, where it is written
    /// (source-only: the statements below it are read with it).
    Scope { text: String },
}

/// One declaration as a card: its statement, the lead of its documentation
/// and where it is written.
#[derive(Clone, Debug, PartialEq)]
pub struct Peek {
    pub decl: SkimDecl,
    pub path: Option<String>,
    /// For a declaration outside the index: its file below the project
    /// root and the line it is declared on, when it lies inside the project.
    pub source: Option<(String, usize)>,
}

/// A statement cut to the length a skim shows.
pub fn cut_statement(s: &str) -> (String, bool) {
    cut(s, MAX_SIG_BYTES)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Skim {
    pub scope: SkimScope,
    pub title: String,
    pub items: Vec<SkimItem>,
    pub decls: usize,
    pub files: usize,
    /// Counts of the files covered (file and directory scopes).
    pub stats: Option<ModStats>,
    /// Why the skim is not the whole scope, when it is not.
    pub truncated: Option<String>,
}

pub struct Env<'a> {
    pub conn: &'a Connection,
    pub mode: IndexMode,
    pub source_root: Option<&'a Path>,
    pub cache: &'a mut SourceCache,
    pub map: Option<&'a ProjectMap>,
    pub show_generated: bool,
}

struct Raw {
    row: DeclRow,
    module_id: Option<i64>,
    line: Option<u32>,
    end_line: Option<u32>,
    kw: Option<u32>,
    header_end: Option<u32>,
    type_text: Option<String>,
    sig_json: Option<String>,
    doc: Option<String>,
    keyword: Option<String>,
    start: Option<u32>,
    end: Option<u32>,
}

const RAW_COLS: &str = "d.id, d.name, d.display, d.kind, d.origin, d.flags, m.name, d.module_id, d.start_line, d.end_line,
    d.kw_byte, d.header_end_byte, d.type_text, d.sig_json, d.doc, d.keyword, d.start_byte, d.end_byte";

fn raw_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<Raw> {
    Ok(Raw {
        row: DeclRow {
            id: r.get(0)?,
            name: r.get(1)?,
            display: r.get(2)?,
            kind: DeclKind::from_u8(r.get::<_, u8>(3)?),
            origin: Origin::from_u8(r.get::<_, u8>(4)?),
            flags: r.get::<_, i64>(5)? as u32,
            module: r.get(6)?,
        },
        module_id: r.get(7)?,
        line: r.get(8)?,
        end_line: r.get(9)?,
        kw: r.get(10)?,
        header_end: r.get(11)?,
        type_text: r.get(12)?,
        sig_json: r.get(13)?,
        doc: r.get(14)?,
        keyword: r.get(15)?,
        start: r.get(16)?,
        end: r.get(17)?,
    })
}

/// The lead of a doc comment: its first paragraph on one line, without
/// Markdown heading marks, cut to a couple of hundred characters.
pub fn doc_lead(doc: &str) -> Option<String> {
    let para: Vec<&str> = doc.lines().map(|l| l.trim()).skip_while(|l| l.is_empty()).take_while(|l| !l.is_empty()).collect();
    let joined = para.join(" ");
    let l = joined.trim_start_matches('#').trim();
    (!l.is_empty()).then(|| cut(l, 240).0)
}

fn cut(s: &str, max: usize) -> (String, bool) {
    if s.len() <= max {
        return (s.to_string(), false);
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    (s[..end].trim_end().to_string(), true)
}

/// The file of a module, when its text is exactly what the index saw.
struct ModFile {
    name: String,
    path: Option<String>,
    doc: Option<String>,
    file: Option<SourceFileText>,
    warn: Option<String>,
    stats: ModStats,
}

fn mod_file(env: &mut Env, id: i64) -> Result<Option<ModFile>> {
    let Some((name, path, sha, doc)) = env
        .conn
        .query_row("SELECT name, path, sha256, doc FROM modules WHERE id = ?1", [id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?))
        })
        .optional()?
    else {
        return Ok(None);
    };
    let stats = crate::db::query::module_stats(env.conn, id)?.unwrap_or_default();
    let mut mf = ModFile { name, path: path.clone(), doc, file: None, warn: None, stats };
    // Semantic statements come from the snapshot; only source-only headers
    // are cut out of the file, by byte offsets valid for the indexed text.
    if env.mode == IndexMode::Source {
        match (env.source_root, &path) {
            (Some(root), Some(rel)) => match env.cache.get(root, rel) {
                Ok(f) if sha.as_deref().is_none_or(|h| h == f.sha256) => mf.file = Some(f),
                Ok(_) => mf.warn = Some("changed since it was indexed: names only until the index is refreshed (R)".into()),
                Err(e) => mf.warn = Some(format!("not readable: {e:#}")),
            },
            _ => mf.warn = Some("no source file available: names only".into()),
        }
    }
    Ok(Some(mf))
}

fn parse_sig(json: &str) -> Option<Signature> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let binders: Vec<Binder> = serde_json::from_value(v.get("binders")?.clone()).ok()?;
    Some(Signature { binders, conclusion: v.get("conclusion")?.as_str()?.to_string() })
}

fn decl_item(mode: IndexMode, r: Raw, file: Option<&SourceFileText>) -> SkimDecl {
    let kw = r.keyword.clone().unwrap_or_else(|| r.row.kind.as_str().to_string());
    let mut folded = 0u32;
    let full = match mode {
        IndexMode::Source => {
            let from_file = match (file, r.kw, r.header_end) {
                (Some(f), Some(a), Some(z)) => {
                    let (a, z) = (a as usize, z as usize);
                    (a < z && z <= f.text.len() && f.text.is_char_boundary(a) && f.text.is_char_boundary(z)).then(|| {
                        if let Some(end) = r.end_line {
                            folded = (end as usize).saturating_sub(f.line_of_byte(z.saturating_sub(1))) as u32;
                        }
                        f.text[a..z].trim_end().to_string()
                    })
                }
                _ => None,
            };
            from_file.unwrap_or_else(|| format!("{kw} {}", r.row.display))
        }
        IndexMode::Semantic => match (r.sig_json.as_deref().and_then(parse_sig), &r.type_text) {
            (Some(sig), _) => {
                let (groups, _) = sig.binder_groups(true);
                let head = if groups.is_empty() { format!("{kw} {} :", r.row.display) } else { format!("{kw} {} {} :", r.row.display, groups.join(" ")) };
                format!("{head}\n    {}", sig.conclusion.replace('\n', "\n    "))
            }
            (None, Some(t)) => format!("{kw} {} :\n    {}", r.row.display, t.replace('\n', "\n    ")),
            _ => format!("{kw} {}", r.row.display),
        },
    };
    let (sig, sig_cut) = cut(&full, MAX_SIG_BYTES);
    SkimDecl {
        line: r.line,
        sig,
        sig_cut,
        doc: r.doc.as_deref().and_then(doc_lead),
        sorry: r.row.flags & flags::LEX_SORRY != 0,
        folded,
        row: r.row,
    }
}

fn file_sub(mf: &ModFile, mode: IndexMode) -> String {
    let st = &mf.stats;
    let mut v = Vec::new();
    if let Some(p) = &mf.path {
        v.push(p.clone());
    }
    v.push(counts_line(st, mode));
    v.join(" · ")
}

/// `12 thm · 3 def …`, the parts that are not zero.
pub fn counts_line(st: &ModStats, mode: IndexMode) -> String {
    let mut v: Vec<String> = Vec::new();
    for (n, what) in [(st.thm, "thm"), (st.def, "def"), (st.typ, "types"), (st.inst, "inst"), (st.axiom, "axioms")] {
        if n > 0 {
            v.push(format!("{n} {what}"));
        }
    }
    if v.is_empty() {
        v.push("no declarations".into());
    }
    if st.sorry > 0 {
        v.push(match mode {
            IndexMode::Source => format!("{} with a sorry token", st.sorry),
            IndexMode::Semantic => format!("{} depend on sorryAx", st.sorry),
        });
    }
    v.join(" · ")
}

struct Builder {
    /// A skim of one file (a semantic index reads its prose from the file).
    single_file: bool,
    items: Vec<SkimItem>,
    decls: usize,
    files: usize,
    stats: ModStats,
    truncated: Option<String>,
}

impl Builder {
    fn full(&self) -> bool {
        self.decls >= MAX_DECLS
    }

    /// Adds one file: a header, then its declarations in source order.
    fn file(&mut self, env: &mut Env, id: i64, only: Option<&[i64]>) -> Result<()> {
        let Some(mf) = mod_file(env, id)? else { return Ok(()) };
        self.files += 1;
        self.stats.add(&mf.stats);
        let header = self.items.len();
        self.items.push(SkimItem::Header {
            title: mf.name.clone(),
            sub: file_sub(&mf, env.mode),
            doc: mf.doc.as_deref().and_then(doc_lead),
            warn: mf.warn.clone(),
        });
        let gen = if env.show_generated { String::new() } else { format!(" AND (d.flags & {}) = 0", flags::GENERATED) };
        let mut st = env.conn.prepare_cached(&format!(
            "SELECT {RAW_COLS} FROM decls d LEFT JOIN modules m ON m.id = d.module_id
              WHERE d.module_id = ?1 {gen} ORDER BY d.start_line IS NULL, d.start_line, d.id LIMIT ?2"
        ))?;
        let room = MAX_DECLS - self.decls.min(MAX_DECLS);
        // A selection is filtered below, so the whole file is read for it.
        let limit = if only.is_some() { 50_000 } else { room + 1 };
        let rows: Vec<Raw> = st.query_map(params![id, limit as i64], raw_from)?.collect::<rusqlite::Result<_>>()?;
        drop(st);
        // What stands between the declarations, when the file is read from
        // top to bottom: module-doc blocks, and in a source-only index the
        // `variable` and `open` lines its statements are written under.
        let read;
        let between: Option<(&SourceFileText, Vec<crate::source::scope::Event>)> = match (only, env.mode, &mf.file) {
            (None, IndexMode::Source, Some(f)) => {
                let mut extents: Vec<(usize, usize)> = rows.iter().take(room).filter_map(|r| Some((r.start? as usize, r.end? as usize))).collect();
                // A skim that stops inside the file does not read the rest of it.
                if rows.len() > room {
                    let cut = extents.iter().map(|x| x.1).max().unwrap_or(0);
                    extents.push((cut, f.text.len()));
                }
                Some((f, crate::source::scope::file_scope(&f.text, &f.line_starts, &extents).events))
            }
            (None, IndexMode::Semantic, _) if self.single_file => {
                read = crate::scope::module_scope(env.conn, env.source_root, env.cache, id)?;
                read.as_ref().map(|(f, s)| (f, s.events.iter().filter(|e| e.kind == EventKind::Prose).copied().collect()))
            }
            _ => None,
        };
        let mut next = 0usize;
        let mut emit = |items: &mut Vec<SkimItem>, upto: usize| {
            let Some((f, ev)) = &between else { return };
            while next < ev.len() && ev[next].start < upto {
                let e = &ev[next];
                next += 1;
                match e.kind {
                    EventKind::Prose => {
                        // The header's lead is the first block: said once.
                        if let SkimItem::Header { doc, .. } = &mut items[header] {
                            *doc = None;
                        }
                        items.push(SkimItem::Prose(crate::scope::prose(f, e)));
                    }
                    EventKind::Cmd { kind: CmdKind::Universe, .. } => {}
                    _ => items.push(SkimItem::Scope { text: crate::source::scope::one_line(&f.text, e) }),
                }
            }
        };
        let mut n = 0;
        for r in rows {
            if only.is_some_and(|o| !o.contains(&r.row.id)) {
                continue;
            }
            if n >= room {
                self.truncated = Some(format!("stopped after {MAX_DECLS} declarations"));
                break;
            }
            let at = match (&between, r.start, r.line) {
                (Some(_), Some(b), _) => b as usize,
                (Some((f, _)), None, Some(l)) => f.line_starts.get((l as usize).saturating_sub(1)).copied().unwrap_or(usize::MAX),
                _ => 0,
            };
            emit(&mut self.items, at);
            self.items.push(SkimItem::Decl(decl_item(env.mode, r, mf.file.as_ref())));
            n += 1;
        }
        if self.truncated.is_none() {
            emit(&mut self.items, usize::MAX);
        }
        self.decls += n;
        // A file that declares nothing says so in its header already.
        if n == 0 && mf.stats.named() > 0 {
            self.items.push(SkimItem::Note { text: "its declarations are hidden by the current filters".into(), warn: false });
        }
        Ok(())
    }
}

fn module_ids(conn: &Connection, prefix: &str, limit: usize) -> Result<Vec<i64>> {
    let mut st = conn.prepare_cached(
        "SELECT id FROM modules WHERE is_local = 1 AND (name = ?1 OR (name >= ?1 || '.' AND name < ?1 || '/')) ORDER BY name LIMIT ?2",
    )?;
    let v = st.query_map(params![prefix, limit as i64], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(v)
}

pub fn skim(env: &mut Env, scope: &SkimScope) -> Result<Skim> {
    let mut b = Builder { single_file: matches!(scope, SkimScope::Module(_)), items: Vec::new(), decls: 0, files: 0, stats: ModStats::default(), truncated: None };
    let mut with_stats = false;
    match scope {
        SkimScope::Module(name) => {
            let id: Option<i64> = env.conn.query_row("SELECT id FROM modules WHERE name = ?1", [name], |r| r.get(0)).optional()?;
            match id {
                Some(id) => {
                    b.file(env, id, None)?;
                    with_stats = true;
                }
                None => b.items.push(SkimItem::Note { text: format!("{name} is not a module of this index"), warn: true }),
            }
        }
        SkimScope::Dir(prefix) => {
            let ids = module_ids(env.conn, prefix, MAX_FILES + 1)?;
            if ids.is_empty() {
                b.items.push(SkimItem::Note { text: format!("no local file is named {prefix} or lies below it"), warn: true });
            }
            for (k, id) in ids.iter().enumerate() {
                if k >= MAX_FILES {
                    b.truncated = Some(format!("stopped after {MAX_FILES} files"));
                    break;
                }
                if b.full() {
                    b.truncated = Some(format!("stopped after {MAX_DECLS} declarations"));
                    break;
                }
                b.file(env, *id, None)?;
            }
            with_stats = b.truncated.is_none();
        }
        SkimScope::Namespace(ns) => {
            let gen = if env.show_generated { String::new() } else { format!(" AND (d.flags & {}) = 0", flags::GENERATED) };
            let mut st = env.conn.prepare_cached(&format!(
                "SELECT d.id, d.module_id FROM decls d
                  WHERE (d.name = ?1 OR (d.name >= ?1 || '.' AND d.name < ?1 || '/')) {gen} ORDER BY d.name LIMIT ?2"
            ))?;
            let found: Vec<(i64, Option<i64>)> =
                st.query_map(params![ns, MAX_DECLS as i64 + 1], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
            if found.is_empty() {
                b.items.push(SkimItem::Note { text: format!("nothing is named {ns} or lies below it"), warn: true });
            }
            if found.len() > MAX_DECLS {
                b.truncated = Some(format!("stopped after {MAX_DECLS} declarations (in name order)"));
            }
            // Read file by file, in source order within each file.
            let mut by_mod: Vec<(i64, Vec<i64>)> = Vec::new();
            let mut loose: Vec<i64> = Vec::new();
            for (id, m) in found.into_iter().take(MAX_DECLS) {
                match m {
                    Some(m) => match by_mod.iter_mut().find(|x| x.0 == m) {
                        Some(x) => x.1.push(id),
                        None => by_mod.push((m, vec![id])),
                    },
                    None => loose.push(id),
                }
            }
            let mut names: Vec<(String, i64, Vec<i64>)> = Vec::new();
            for (m, ids) in by_mod {
                let n: String = env.conn.query_row("SELECT name FROM modules WHERE id = ?1", [m], |r| r.get(0))?;
                names.push((n, m, ids));
            }
            names.sort();
            for (k, (_, m, ids)) in names.iter().enumerate() {
                if k >= MAX_FILES {
                    b.truncated = Some(format!("stopped after {MAX_FILES} files"));
                    break;
                }
                b.file(env, *m, Some(ids))?;
            }
            if !loose.is_empty() {
                b.items.push(SkimItem::Header { title: "not in any module of this index".into(), sub: String::new(), doc: None, warn: None });
                for id in loose {
                    if let Some(r) = raw_by_id(env.conn, id)? {
                        b.items.push(SkimItem::Decl(decl_item(env.mode, r, None)));
                        b.decls += 1;
                    }
                }
            }
        }
        SkimScope::Group(gid) => match env.map.and_then(|m| m.group_by_id(gid).map(|k| (m, k))) {
            None => b.items.push(SkimItem::Note { text: format!("the project map has no group {gid:?}"), warn: true }),
            Some((m, k)) => {
                let mut stack = vec![k];
                while let Some(g) = stack.pop() {
                    let grp = &m.groups[g];
                    b.items.push(SkimItem::Header {
                        title: grp.title.clone(),
                        sub: format!("map group · {} landmarks listed here", grp.landmarks.len()),
                        doc: grp.purpose.as_deref().and_then(doc_lead),
                        warn: None,
                    });
                    for p in &grp.landmarks {
                        if b.full() {
                            b.truncated = Some(format!("stopped after {MAX_DECLS} declarations"));
                            break;
                        }
                        let id = crate::db::query::decl_by_name(env.conn, &p.decl)?;
                        match id.map(|id| raw_by_id(env.conn, id)).transpose()?.flatten() {
                            Some(r) => {
                                let file = match r.module_id {
                                    Some(mid) => mod_file(env, mid)?.and_then(|f| f.file),
                                    None => None,
                                };
                                b.items.push(SkimItem::Decl(decl_item(env.mode, r, file.as_ref())));
                                b.decls += 1;
                            }
                            None => b.items.push(SkimItem::Note { text: format!("{} is not in this index", p.decl), warn: true }),
                        }
                    }
                    // Subgroups in the map's order.
                    stack.extend(grp.children.iter().rev());
                }
            }
        },
    }
    let title = match scope {
        SkimScope::Group(gid) => env.map.and_then(|m| m.group_by_id(gid).map(|k| format!("map group {}", m.groups[k].title))).unwrap_or_else(|| scope.label()),
        _ => scope.label(),
    };
    Ok(Skim { scope: scope.clone(), title, decls: b.decls, files: b.files, stats: with_stats.then_some(b.stats), truncated: b.truncated, items: b.items })
}

/// The card of one declaration.
pub fn peek(env: &mut Env, id: i64) -> Result<Option<Peek>> {
    let Some(r) = raw_by_id(env.conn, id)? else { return Ok(None) };
    let mut path = None;
    let mut file = None;
    if let Some(mid) = r.module_id {
        let (p, sha): (Option<String>, Option<String>) = env.conn.query_row("SELECT path, sha256 FROM modules WHERE id = ?1", [mid], |x| Ok((x.get(0)?, x.get(1)?)))?;
        // Source-only headers are cut out of the file the index saw.
        if let (IndexMode::Source, Some(root), Some(rel)) = (env.mode, env.source_root, &p) {
            file = env.cache.get(root, rel).ok().filter(|f| sha.as_deref().is_none_or(|h| h == f.sha256));
        }
        path = p;
    }
    Ok(Some(Peek { decl: decl_item(env.mode, r, file.as_ref()), path, source: None }))
}

fn raw_by_id(conn: &Connection, id: i64) -> Result<Option<Raw>> {
    Ok(conn
        .query_row(&format!("SELECT {RAW_COLS} FROM decls d LEFT JOIN modules m ON m.id = d.module_id WHERE d.id = ?1"), [id], raw_from)
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Project;
    use crate::source::index::{build_source_index, ScanConfig};

    const A: &str = "/-! # Closure\n\nReflexive transitive closure. -/\nnamespace P\n\n/-- The closure of `r`.\nSecond line. -/\ninductive Star (r : Nat → Nat → Prop) : Nat → Nat → Prop\n  | refl (a : Nat) : Star r a a\n\n/-- Transitivity. -/\ntheorem Star.trans {r : Nat → Nat → Prop} {a b c : Nat}\n    (h : Star r a b) (k : Star r b c) : Star r a c := by\n  induction k with\n  | refl => exact h\n\ntheorem Star.todo : True := by\n  sorry\n\nend P\n";
    const B: &str = "import P.A\nnamespace P\ndef two : Nat := 2\ntheorem Star.self (r : Nat → Nat → Prop) (a : Nat) : Star r a a := Star.refl a\nend P\n";

    fn project() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("proj");
        for (rel, text) in [("lakefile.toml", "name = \"p\"\n"), ("P/A.lean", A), ("P/B.lean", B)] {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        let project = Project::open(&root).unwrap();
        let idx = d.path().join("i.sqlite");
        let cfg = ScanConfig { threads: 1, max_file_bytes: 1 << 20, profile: crate::source::lexer::LexProfile::NEWEST };
        build_source_index(&project, &idx, &cfg, &std::sync::atomic::AtomicBool::new(false), None).unwrap();
        (d, root, idx)
    }

    fn decls(s: &Skim) -> Vec<&SkimDecl> {
        s.items.iter().filter_map(|i| if let SkimItem::Decl(d) = i { Some(d) } else { None }).collect()
    }

    #[test]
    fn a_file_is_skimmed_as_headers_without_proofs() {
        let (_d, root, idx) = project();
        let conn = crate::db::open_read(&idx).unwrap();
        let mut cache = SourceCache::default();
        let mut env = Env { conn: &conn, mode: IndexMode::Source, source_root: Some(&root), cache: &mut cache, map: None, show_generated: false };
        let s = skim(&mut env, &SkimScope::Module("P.A".into())).unwrap();
        match &s.items[0] {
            SkimItem::Header { title, sub, doc, warn } => {
                assert_eq!(title, "P.A");
                assert!(sub.starts_with("P/A.lean · 2 thm · 1 types"), "{sub}");
                assert!(sub.ends_with("1 with a sorry token"), "{sub}");
                assert!(doc.is_none(), "the module doc is the first block of prose");
                assert!(warn.is_none());
            }
            other => panic!("{other:?}"),
        }
        match &s.items[1] {
            SkimItem::Prose(p) => {
                let part = &p.parts[0];
                assert_eq!((part.heading.as_ref().map(|h| h.1.as_str()), part.lead.as_deref(), p.line, p.folded), (Some("Closure"), Some("Reflexive transitive closure."), 1, 0));
            }
            other => panic!("{other:?}"),
        }
        let ds = decls(&s);
        let trans = ds.iter().find(|d| d.row.name == "P.Star.trans").unwrap();
        assert!(trans.sig.starts_with("theorem Star.trans {r : Nat → Nat → Prop} {a b c : Nat}\n"), "{}", trans.sig);
        assert!(trans.sig.ends_with("Star r a c"), "the statement stops before `:=`: {:?}", trans.sig);
        assert!(!trans.sig.contains("induction"));
        assert_eq!(trans.doc.as_deref(), Some("Transitivity."));
        assert_eq!(trans.folded, 2, "two proof lines below the statement");
        let star = ds.iter().find(|d| d.row.name == "P.Star").unwrap();
        assert_eq!(star.doc.as_deref(), Some("The closure of `r`. Second line."), "the first paragraph, on one line");
        assert!(ds.iter().find(|d| d.row.name == "P.Star.todo").unwrap().sorry);
        assert_eq!(s.stats.unwrap().thm, 2);
        assert!(s.truncated.is_none());
        // Source order, not name order.
        let lines: Vec<u32> = ds.iter().filter_map(|d| d.line).collect();
        assert!(lines.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn directories_and_namespaces_group_by_file() {
        let (_d, root, idx) = project();
        let conn = crate::db::open_read(&idx).unwrap();
        let mut cache = SourceCache::default();
        let mut env = Env { conn: &conn, mode: IndexMode::Source, source_root: Some(&root), cache: &mut cache, map: None, show_generated: false };
        let s = skim(&mut env, &SkimScope::Dir("P".into())).unwrap();
        let headers: Vec<&str> = s.items.iter().filter_map(|i| if let SkimItem::Header { title, .. } = i { Some(title.as_str()) } else { None }).collect();
        assert_eq!(headers, vec!["P.A", "P.B"]);
        assert_eq!((s.files, s.stats.unwrap().thm, s.stats.unwrap().def), (2, 3, 1));
        // The namespace P.Star spans both files; `P.two` is not in it.
        let s = skim(&mut env, &SkimScope::Namespace("P.Star".into())).unwrap();
        let names: Vec<&str> = decls(&s).iter().map(|d| d.row.name.as_str()).collect();
        assert_eq!(names, vec!["P.Star", "P.Star.refl", "P.Star.trans", "P.Star.todo", "P.Star.self"]);
        assert!(s.stats.is_none(), "file counts do not describe a namespace");
        assert!(matches!(&skim(&mut env, &SkimScope::Namespace("Nope".into())).unwrap().items[0], SkimItem::Note { warn: true, .. }));
        assert!(matches!(&skim(&mut env, &SkimScope::Module("P.Missing".into())).unwrap().items[0], SkimItem::Note { warn: true, .. }));
    }

    #[test]
    fn a_changed_file_shows_names_only_and_says_why() {
        let (_d, root, idx) = project();
        std::fs::write(root.join("P/B.lean"), format!("-- edited\n{B}")).unwrap();
        let conn = crate::db::open_read(&idx).unwrap();
        let mut cache = SourceCache::default();
        let mut env = Env { conn: &conn, mode: IndexMode::Source, source_root: Some(&root), cache: &mut cache, map: None, show_generated: false };
        let s = skim(&mut env, &SkimScope::Module("P.B".into())).unwrap();
        assert!(matches!(&s.items[0], SkimItem::Header { warn: Some(w), .. } if w.contains("changed since")));
        assert_eq!(decls(&s)[0].sig, "def P.two", "byte offsets of the old text are not applied to the new one");
    }

    #[test]
    fn semantic_statements_come_from_the_snapshot() {
        let d = decl_item(
            IndexMode::Semantic,
            Raw {
                row: DeclRow { id: 1, name: "T.x".into(), display: "T.x".into(), kind: DeclKind::Theorem, origin: Origin::Local, flags: 0, module: None },
                module_id: None,
                line: Some(3),
                end_line: Some(9),
                kw: None,
                header_end: None,
                type_text: Some("∀ (n : Nat), n = n".into()),
                sig_json: Some(r#"{"binders":[{"name":"n","info":"default","type":"Nat"}],"conclusion":"n = n"}"#.into()),
                doc: Some("\n# Heading\n\nmore".into()),
                keyword: None,
                start: None,
                end: None,
            },
            None,
        );
        assert_eq!(d.sig, "theorem T.x (n : Nat) :\n    n = n");
        assert_eq!(d.doc.as_deref(), Some("Heading"));
        let long = "x".repeat(MAX_SIG_BYTES * 2);
        assert!(cut(&long, MAX_SIG_BYTES).1);
        assert_eq!(cut("αβγ", 3).0, "α", "cut at a character boundary");
    }
}
