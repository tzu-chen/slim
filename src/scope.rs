//! The context a declaration is written in: the `variable` and `open`
//! commands in force where it stands, read from the source text (see
//! [`crate::source::scope`]). Lexical: Lean decides which variables a
//! declaration really takes; here it is those whose names it writes.

use crate::db::query::DeclDetail;
use crate::model::DeclKind;
use crate::source::lexer::{LexProfile, Lexer, TokKind};
pub use crate::source::scope::CmdKind;
use crate::source::scope::{self, EventKind, FileScope};
use crate::sourceview::{SourceCache, SourceFileText};
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

/// At most this much of a declaration is read for the names it writes.
const MAX_USE_BYTES: usize = 16 << 10;
const MAX_LINE_BYTES: usize = 400;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeLine {
    pub kind: CmdKind,
    pub text: String,
    /// Where the command is written (1-based).
    pub line: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeclScope {
    /// `variable` lines first, then `open` lines, each in file order.
    pub lines: Vec<ScopeLine>,
    /// Variable binders in scope that the declaration does not write.
    pub hidden: usize,
}

impl DeclScope {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

/// The file of a module and what stands between its declarations. `None`
/// when there is no file to read or it is not the text that was indexed.
pub fn module_scope(conn: &Connection, root: Option<&Path>, cache: &mut SourceCache, module_id: i64) -> Result<Option<(SourceFileText, Arc<FileScope>)>> {
    let Some(root) = root else { return Ok(None) };
    let Some((path, sha)) = conn
        .query_row("SELECT path, sha256 FROM modules WHERE id = ?1", [module_id], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)))
        .optional()?
    else {
        return Ok(None);
    };
    let Some(path) = path else { return Ok(None) };
    let Ok(f) = cache.get(root, &path) else { return Ok(None) };
    if sha.is_some_and(|h| h != f.sha256) {
        return Ok(None);
    }
    if let Some(s) = cache.scope(module_id, &f.sha256) {
        return Ok(Some((f, s)));
    }
    let s = Arc::new(scope::file_scope(&f.text, &f.line_starts, &all_extents(conn, &f, module_id)?));
    cache.put_scope(module_id, &f.sha256, s.clone());
    Ok(Some((f, s)))
}

/// Byte extents from a source-only scan, else the line ranges of a snapshot.
fn extent(f: &SourceFileText, sb: Option<i64>, eb: Option<i64>, l0: i64, l1: Option<i64>) -> (usize, usize) {
    match (sb, eb) {
        (Some(a), Some(z)) => (a as usize, z as usize),
        _ => line_extent(f, l0 as usize, l1.unwrap_or(l0) as usize),
    }
}

fn all_extents(conn: &Connection, f: &SourceFileText, module_id: i64) -> Result<Vec<(usize, usize)>> {
    let mut st = conn.prepare_cached("SELECT start_byte, end_byte, start_line, end_line FROM decls WHERE module_id = ?1 AND start_line IS NOT NULL ORDER BY start_line, id")?;
    let rows = st.query_map([module_id], |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, i64>(2)?, r.get::<_, Option<i64>>(3)?)))?;
    let mut out = Vec::new();
    for r in rows {
        let (sb, eb, l0, l1) = r?;
        out.push(extent(f, sb, eb, l0, l1));
    }
    Ok(out)
}

fn line_extent(f: &SourceFileText, l0: usize, l1: usize) -> (usize, usize) {
    let l0 = l0.max(1);
    let a = f.line_starts.get(l0 - 1).copied().unwrap_or(f.text.len());
    let z = f.line_starts.get(l1.max(l0)).copied().unwrap_or(f.text.len());
    (a, z)
}

/// Where a declaration starts, in the units [`module_scope`] used.
pub fn start_of(d: &DeclDetail, f: &SourceFileText) -> Option<usize> {
    match (d.pos.start_byte, d.pos.end_byte, d.pos.start_line) {
        (Some(a), Some(_), _) => Some(a as usize),
        (_, _, Some(l)) => Some(line_extent(f, l as usize, l as usize).0),
        _ => None,
    }
}

/// The namespaces opened where the declaration stands.
pub fn opens(conn: &Connection, root: Option<&Path>, cache: &mut SourceCache, d: &DeclDetail) -> Vec<String> {
    let Some(mid) = d.module_id else { return Vec::new() };
    let Ok(Some((f, fs))) = module_scope(conn, root, cache, mid) else { return Vec::new() };
    let Some(at) = start_of(d, &f) else { return Vec::new() };
    let mut out = Vec::new();
    for (e, kind) in fs.in_force(at) {
        if kind == CmdKind::Open {
            for n in scope::opened(f.text.get(e.start..e.end).unwrap_or("")) {
                if !out.contains(&n) {
                    out.push(n);
                }
            }
        }
    }
    out
}

/// The identifiers written in `text`, dotted ones by their first component too.
fn written(text: &str) -> HashSet<&str> {
    let mut out = HashSet::new();
    let mut lx = Lexer::new(text, LexProfile::NEWEST);
    while let Some(t) = lx.next_token() {
        if t.kind == TokKind::Ident {
            let w = t.text(text);
            out.insert(w);
            if let Some((h, _)) = w.split_once('.') {
                out.insert(h);
            }
        }
    }
    out
}

fn cut(mut s: String) -> String {
    if s.len() > MAX_LINE_BYTES {
        let mut end = MAX_LINE_BYTES;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
        s.push_str(" …");
    }
    s
}

/// The context of declaration `d`. With `variables`, the `variable`
/// binders whose names it writes (an elaborated statement already shows
/// its binders, so a semantic index asks for the `open` lines only).
pub fn decl_scope(conn: &Connection, root: Option<&Path>, cache: &mut SourceCache, d: &DeclDetail, variables: bool) -> Result<DeclScope> {
    let mut out = DeclScope::default();
    let Some(mid) = d.module_id else { return Ok(out) };
    let Some((f, fs)) = module_scope(conn, root, cache, mid)? else { return Ok(out) };
    let Some(at) = start_of(d, &f) else { return Ok(out) };
    // A theorem takes the variables its statement writes; a definition
    // also those of its body.
    let used = if variables {
        let from = d.pos.kw_byte.map(|x| x as usize).unwrap_or(at);
        let to = match d.row.kind {
            DeclKind::Theorem => d.pos.header_end_byte.or(d.pos.end_byte),
            _ => d.pos.end_byte,
        }
        .map(|x| x as usize)
        .unwrap_or(from);
        let mut to = to.min(from + MAX_USE_BYTES).min(f.text.len());
        while to > from && !f.text.is_char_boundary(to) {
            to -= 1;
        }
        f.text.get(from..to).map(written).unwrap_or_default()
    } else {
        HashSet::new()
    };
    let mut open_lines = Vec::new();
    // Every binder group in scope decides together which are taken.
    let mut cmds: Vec<(u32, String, Vec<scope::Group>)> = Vec::new();
    for (e, kind) in fs.in_force(at) {
        let text = scope::one_line(&f.text, e);
        match kind {
            CmdKind::Open => open_lines.push(ScopeLine { kind, text: cut(text), line: e.line }),
            CmdKind::Variable if variables => {
                let g = scope::groups(&text);
                cmds.push((e.line, text, g));
            }
            _ => {}
        }
    }
    let all: Vec<scope::Group> = cmds.iter().flat_map(|c| c.2.iter().cloned()).collect();
    let on = scope::taken(&all, &used);
    let mut k = 0;
    for (line, text, g) in cmds {
        if g.is_empty() {
            // Not a list of binders: shown as written.
            out.lines.push(ScopeLine { kind: CmdKind::Variable, text: cut(text), line });
            continue;
        }
        let kept: Vec<&str> = g.iter().enumerate().filter(|(j, _)| on[k + j]).map(|(_, x)| x.text.as_str()).collect();
        out.hidden += g.len() - kept.len();
        k += g.len();
        if !kept.is_empty() {
            out.lines.push(ScopeLine { kind: CmdKind::Variable, text: cut(format!("variable {}", kept.join(" "))), line });
        }
    }
    out.lines.extend(open_lines);
    Ok(out)
}

/// A module-doc block as a reader skims it: each heading with the lead of
/// the text under it, and how many lines were left out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prose {
    pub line: u32,
    pub parts: Vec<ProsePart>,
    pub folded: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProsePart {
    /// A Markdown heading: level, text, line.
    pub heading: Option<(u8, String, u32)>,
    /// The first paragraph below it, on one line.
    pub lead: Option<String>,
    pub lead_cut: bool,
}

const MAX_LEAD_BYTES: usize = 280;

pub fn prose(f: &SourceFileText, e: &scope::Event) -> Prose {
    let raw = f.text.get(e.start..e.end).unwrap_or("");
    let body = crate::source::scan::strip_doc(raw);
    // `strip_doc` keeps the lines of the block; its first line is where the
    // text starts after `/-!`.
    let lead_ws = raw.strip_prefix("/-!").map(|r| r.len() - r.trim_start().len()).unwrap_or(0);
    let first_line = e.line + raw[..(3 + lead_ws).min(raw.len())].matches('\n').count() as u32;
    let mut parts: Vec<ProsePart> = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    // 0: before the part's paragraph, 1: in it, 2: after it.
    let mut state = 0u8;
    let mut shown = 0u32;
    let close = |parts: &mut Vec<ProsePart>, para: &mut Vec<&str>| {
        if para.is_empty() {
            return;
        }
        let joined = para.join(" ");
        para.clear();
        let mut end = joined.len().min(MAX_LEAD_BYTES);
        while !joined.is_char_boundary(end) {
            end -= 1;
        }
        if parts.is_empty() {
            parts.push(ProsePart::default());
        }
        let p = parts.last_mut().expect("a part was pushed");
        p.lead_cut = end < joined.len();
        p.lead = Some(joined[..end].trim_end().to_string());
    };
    for (i, l) in body.lines().enumerate() {
        let t = l.trim();
        if t.starts_with('#') {
            close(&mut parts, &mut para);
            let level = t.chars().take_while(|c| *c == '#').count().min(6) as u8;
            let title = t.trim_start_matches('#').trim();
            if !title.is_empty() {
                parts.push(ProsePart { heading: Some((level, title.to_string(), first_line + i as u32)), ..Default::default() });
                shown += 1;
            }
            state = 0;
        } else if t.is_empty() {
            if state == 1 {
                close(&mut parts, &mut para);
                state = 2;
            }
        } else if state < 2 {
            state = 1;
            para.push(t);
            shown += 1;
        }
    }
    close(&mut parts, &mut para);
    let lines = body.lines().filter(|l| !l.trim().is_empty()).count() as u32;
    Prose { line: e.line, parts, folded: lines.saturating_sub(shown) }
}

/// The section headings of a module's doc blocks, for the outline.
pub fn sections(conn: &Connection, root: Option<&Path>, cache: &mut SourceCache, module_id: i64) -> Result<Vec<(u8, String, u32)>> {
    let Some((f, fs)) = module_scope(conn, root, cache, module_id)? else { return Ok(Vec::new()) };
    Ok(fs.events.iter().filter(|e| e.kind == EventKind::Prose).flat_map(|e| prose(&f, e).parts).filter_map(|p| p.heading).collect())
}
