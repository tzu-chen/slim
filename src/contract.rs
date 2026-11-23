//! A statement read as a contract: what is given, what follows, and the
//! project's own vocabulary unfolded where it is used.
//!
//! A large formalization states its upper-level theorems in names it
//! defined itself (`(hV : Top.VWin) : EveryCharacter`). The names are short
//! definitions a file or two away; the statement is unreadable until they
//! are unfolded and plain once they are. This module lays a statement out
//! as *given* / *then* and puts under each part the definitions of the
//! project names written in it, level by level, stopping at library names.
//!
//! Everything is read from the source text. Which name a word stands for
//! comes from [`crate::relations::mentions`]: Lean's references when the
//! build artifacts are of this text, else the lexical lookup; the result
//! says which.

use crate::db::query::{self, DeclRow};
use crate::model::{flags, DeclKind, Origin};
use crate::relations::{Mention, Part as Where, TypeBody};
use crate::source::lexer::{LexProfile, Lexer, TokKind};
use crate::sourceview::SourceCache;
use anyhow::{Context, Result};
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Levels of vocabulary to unfold: 0 lays the statement out only.
    pub depth: u8,
    /// At most this many definitions are unfolded in all.
    pub max_items: usize,
    /// Lines shown of one unfolded definition.
    pub max_def_lines: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options { depth: 1, max_items: 40, max_def_lines: 14 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartKind {
    /// `(x : T)`
    Explicit,
    /// `{x : T}` or `⦃x : T⦄`
    Implicit,
    /// `[inst : C]`
    Instance,
    /// What a theorem concludes, or the type of a definition.
    Then,
    /// Text shown as written (a definition's body, a structure).
    Text,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub kind: PartKind,
    pub text: String,
    /// Definitions unfolded under this part: indexes into [`Contract::items`].
    pub vocab: Vec<usize>,
    /// Project names of this part that are unfolded further up.
    pub above: Vec<String>,
    /// Project names of this part left folded (the depth or the budget).
    pub folded: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub row: DeclRow,
    pub keyword: String,
    pub path: Option<String>,
    pub line: Option<u32>,
    pub level: u8,
    pub parts: Vec<Part>,
    /// Lines of its text that are not shown.
    pub cut_lines: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Contract {
    /// `items[0]` is the declaration asked about.
    pub items: Vec<Item>,
    /// The names were resolved by Lean (build artifacts), not lexically.
    pub lean: bool,
    pub depth: u8,
    /// Something was left folded because of the budget.
    pub truncated: bool,
    /// Whether another level would unfold anything.
    pub deeper: bool,
}

/// The top-level pieces of a declaration header after its name: binder
/// groups and what follows the first top-level `:`. Byte ranges of `text`.
fn split_header(text: &str, keyword_words: usize, has_name: bool) -> (Vec<(PartKind, usize, usize)>, Option<(usize, usize)>) {
    let mut lx = Lexer::new(text, LexProfile::NEWEST);
    let mut toks = Vec::new();
    while let Some(t) = lx.next_token() {
        if !t.is_comment() {
            toks.push(t);
        }
    }
    let mut k = (keyword_words + has_name as usize).min(toks.len());
    // Universe parameters after the name: `.{u, v}`.
    if k + 1 < toks.len() && t_is(text, &toks[k], ".") && t_is(text, &toks[k + 1], "{") {
        let mut d = 0i32;
        while k < toks.len() {
            match toks[k].kind {
                TokKind::Open => d += 1,
                TokKind::Close => {
                    d -= 1;
                    if d == 0 {
                        k += 1;
                        break;
                    }
                }
                _ => {}
            }
            k += 1;
        }
    }
    let mut groups = Vec::new();
    let mut depth = 0i32;
    let mut start: Option<(usize, PartKind)> = None;
    let mut loose: Option<usize> = None;
    while k < toks.len() {
        let t = &toks[k];
        let w = t.text(text);
        match t.kind {
            TokKind::Open => {
                if depth == 0 {
                    if let Some(a) = loose.take() {
                        groups.push((PartKind::Explicit, a, t.start as usize));
                    }
                    let kind = match w {
                        "{" | "⦃" => PartKind::Implicit,
                        "[" => PartKind::Instance,
                        _ => PartKind::Explicit,
                    };
                    start = Some((t.start as usize, kind));
                }
                depth += 1;
            }
            TokKind::Close => {
                depth -= 1;
                if depth == 0 {
                    if let Some((a, kind)) = start.take() {
                        groups.push((kind, a, t.end as usize));
                    }
                }
                depth = depth.max(0);
            }
            TokKind::Sym if depth == 0 && w == ":" => {
                if let Some(a) = loose.take() {
                    groups.push((PartKind::Explicit, a, t.start as usize));
                }
                return (groups, Some((t.end as usize, text.len())));
            }
            _ if depth == 0 && loose.is_none() => loose = Some(t.start as usize),
            _ => {}
        }
        k += 1;
    }
    if let Some(a) = loose {
        groups.push((PartKind::Explicit, a, text.len()));
    }
    (groups, None)
}

fn t_is(text: &str, t: &crate::source::lexer::Token, w: &str) -> bool {
    t.text(text) == w
}

/// `text` with the indentation its lines share removed and trailing space
/// trimmed; at most `max` lines, with how many were left out.
fn tidy(text: &str, max: usize) -> (String, u32) {
    let lines: Vec<&str> = text.trim().lines().collect();
    let indent = lines.iter().skip(1).filter(|l| !l.trim().is_empty()).map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
    let mut out = String::new();
    for (i, l) in lines.iter().take(max).enumerate() {
        if i > 0 {
            out.push('\n');
            out.push_str(l.get(indent.min(l.len() - l.trim_start().len())..).unwrap_or(l).trim_end());
        } else {
            out.push_str(l.trim());
        }
    }
    (out, lines.len().saturating_sub(max) as u32)
}

/// Whether a mentioned declaration is vocabulary worth unfolding: a
/// definition or a type of the project, not a field or a constructor of
/// one (those come with their structure).
fn unfoldable(conn: &Connection, r: &DeclRow) -> bool {
    if r.id <= 0 || matches!(r.origin, Origin::Library) || r.flags & (flags::PROJECTION | flags::GENERATED) != 0 {
        return false;
    }
    if !matches!(r.kind, DeclKind::Definition | DeclKind::Abbrev | DeclKind::Structure | DeclKind::Class | DeclKind::Inductive | DeclKind::Opaque | DeclKind::Axiom) {
        return false;
    }
    let kw: Option<String> = conn.query_row("SELECT keyword FROM decls WHERE id = ?1", [r.id], |x| x.get(0)).ok().flatten();
    !matches!(kw.as_deref(), Some("field") | Some("constructor"))
}

struct Loaded {
    item: Item,
    /// Byte range of each part in the file, for placing mentions.
    ranges: Vec<(usize, usize)>,
    mentions: Vec<(usize, DeclRow)>,
    lean: bool,
}

fn load(conn: &Connection, root: &Path, cache: &mut SourceCache, id: i64, level: u8, opts: &Options) -> Result<Loaded> {
    let d = query::detail(conn, id)?.context("declaration not found")?;
    let rel = d.module_path.clone().context("the declaration has no source file in this index")?;
    let f = cache.get(root, &rel)?;
    let (Some(kw), Some(end)) = (d.pos.kw_byte.or(d.pos.start_byte), d.pos.end_byte) else {
        // A semantic index: the elaborated statement, already split.
        let mut parts = Vec::new();
        if let Some(sig) = &d.sig {
            for b in &sig.binders {
                let (kind, o, c) = match b.info.as_str() {
                    "implicit" | "strict_implicit" => (PartKind::Implicit, "{", "}"),
                    "inst_implicit" => (PartKind::Instance, "[", "]"),
                    _ => (PartKind::Explicit, "(", ")"),
                };
                parts.push(Part { kind, text: format!("{o}{} : {}{c}", b.name, b.type_), vocab: vec![], above: vec![], folded: vec![] });
            }
            parts.push(Part { kind: PartKind::Then, text: sig.conclusion.clone(), vocab: vec![], above: vec![], folded: vec![] });
        } else if let Some(t) = &d.type_text {
            parts.push(Part { kind: PartKind::Then, text: t.clone(), vocab: vec![], above: vec![], folded: vec![] });
        }
        let n = parts.len();
        let item = Item { row: d.row.clone(), keyword: d.row.kind.as_str().to_string(), path: Some(rel), line: d.pos.sel_line.or(d.pos.start_line), level, parts, cut_lines: 0 };
        return Ok(Loaded { item, ranges: vec![(0, 0); n], mentions: Vec::new(), lean: false });
    };
    let (kw, end) = (kw as usize, (end as usize).min(f.text.len()));
    let hend = d.pos.header_end_byte.map(|h| (h as usize).clamp(kw, end)).unwrap_or(end);
    let keyword = d.keyword.clone().unwrap_or_else(|| d.row.kind.as_str().to_string());
    let theorem = d.row.kind == DeclKind::Theorem;
    let mut parts: Vec<Part> = Vec::new();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut cut_lines = 0u32;
    let part = |kind, text: String| Part { kind, text, vocab: vec![], above: vec![], folded: vec![] };
    let header = f.text.get(kw..hend).unwrap_or("");
    if level == 0 && matches!(d.row.kind, DeclKind::Theorem | DeclKind::Definition | DeclKind::Abbrev | DeclKind::Instance | DeclKind::Opaque | DeclKind::Axiom) && keyword != "field" {
        let has_name = d.row.flags & flags::ANONYMOUS == 0;
        let (groups, then) = split_header(header, keyword.split_whitespace().count().max(1), has_name);
        for (kind, a, z) in groups {
            parts.push(part(kind, tidy(&header[a..z], 40).0));
            ranges.push((kw + a, kw + z));
        }
        if let Some((a, z)) = then {
            parts.push(part(PartKind::Then, tidy(&header[a..z], 60).0));
            ranges.push((kw + a, kw + z));
        }
        if !theorem && hend < end {
            let (t, cut) = tidy(f.text.get(hend..end).unwrap_or(""), opts.max_def_lines.max(1) * 2);
            cut_lines = cut;
            parts.push(part(PartKind::Text, t));
            ranges.push((hend, end));
        }
    } else {
        // An unfolded definition is shown as it is written; a theorem that
        // a statement names is shown by its own statement.
        let z = if theorem { hend } else { end };
        let (t, cut) = tidy(f.text.get(kw..z).unwrap_or(""), opts.max_def_lines.max(1));
        cut_lines = cut;
        parts.push(part(PartKind::Text, t));
        ranges.push((kw, z));
    }
    // The project names it states, where each is first written.
    let m = crate::relations::mentions(conn, Some(root), cache, id)?;
    let mut mentions: Vec<(usize, DeclRow)> = Vec::new();
    for Mention { candidates, part, occurrences, .. } in m.items {
        if !TypeBody::Says.takes(theorem, part) && part != Where::Unknown {
            continue;
        }
        let Some(row) = candidates.into_iter().next() else { continue };
        if row.id == id || !unfoldable(conn, &row) {
            continue;
        }
        let at = occurrences.iter().map(|o| f.line_starts.get(o.line as usize - 1).copied().unwrap_or(0) + o.start as usize).filter(|&b| !theorem || b < hend).min();
        if let Some(at) = at {
            mentions.push((at, row));
        }
    }
    mentions.sort_by_key(|x| x.0);
    let item = Item { row: d.row.clone(), keyword, path: Some(rel), line: d.pos.sel_line.or(d.pos.start_line), level, parts, cut_lines };
    Ok(Loaded { item, ranges, mentions, lean: m.lean })
}

/// The contract of declaration `id`, with `opts.depth` levels unfolded.
pub fn contract(conn: &Connection, root: &Path, cache: &mut SourceCache, id: i64, opts: &Options) -> Result<Contract> {
    let first = load(conn, root, cache, id, 0, opts)?;
    let mut out = Contract { items: Vec::new(), lean: first.lean, depth: opts.depth, truncated: false, deeper: false };
    let mut index: HashMap<i64, usize> = HashMap::new();
    index.insert(id, 0);
    out.items.push(first.item);
    let mut queue: std::collections::VecDeque<(usize, Vec<(usize, usize)>, Vec<(usize, DeclRow)>)> = std::collections::VecDeque::new();
    queue.push_back((0, first.ranges, first.mentions));
    while let Some((k, ranges, mentions)) = queue.pop_front() {
        let level = out.items[k].level;
        for (at, row) in mentions {
            // The part the name is first written in (the last one when it
            // stands in none, as with a range read from another text).
            let p = ranges.iter().position(|&(a, z)| a <= at && at < z).unwrap_or(ranges.len().saturating_sub(1));
            if out.items[k].parts.is_empty() {
                break;
            }
            let name = crate::app::state::short_name(&row.display);
            if let Some(&there) = index.get(&row.id) {
                let part = &mut out.items[k].parts[p];
                if there != k && !part.vocab.contains(&there) && !part.above.contains(&name) {
                    part.above.push(name);
                }
                continue;
            }
            if level >= opts.depth {
                out.deeper = true;
                out.items[k].parts[p].folded.push(name);
                continue;
            }
            if out.items.len() >= opts.max_items {
                out.truncated = true;
                out.items[k].parts[p].folded.push(name);
                continue;
            }
            let Ok(l) = load(conn, root, cache, row.id, level + 1, opts) else {
                out.items[k].parts[p].folded.push(name);
                continue;
            };
            let j = out.items.len();
            index.insert(row.id, j);
            out.items.push(l.item);
            out.items[k].parts[p].vocab.push(j);
            queue.push_back((j, l.ranges, l.mentions));
        }
    }
    Ok(out)
}

/// What a rendered line is, for styling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Head,
    Label,
    Explicit,
    Quiet,
    Then,
    Text,
    DefHead,
    Note,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RLine {
    /// Cells of indentation (the rail of the definition it belongs to).
    pub indent: u16,
    /// `given`, `then`, `is` or nothing.
    pub label: &'static str,
    pub role: Role,
    pub text: String,
    /// The declaration the line opens (a head line), for Enter.
    pub decl: Option<i64>,
}

fn label_of(kind: PartKind, theorem: bool) -> &'static str {
    match kind {
        PartKind::Explicit | PartKind::Implicit | PartKind::Instance => "given",
        PartKind::Then if theorem => "then",
        PartKind::Then => "is",
        PartKind::Text => "",
    }
}

fn render_item(c: &Contract, k: usize, indent: u16, out: &mut Vec<RLine>) {
    let it = &c.items[k];
    // The declaration asked about with its whole path; an unfolded
    // definition with the end of it.
    let place = match (&it.path, it.line) {
        (Some(p), Some(l)) if k == 0 => format!("  {p}:{l}"),
        (Some(p), Some(l)) => {
            let comps: Vec<&str> = p.split('/').collect();
            let tail = comps[comps.len().saturating_sub(2)..].join("/");
            format!("  ({}{tail}:{l})", if comps.len() > 2 { "…/" } else { "" })
        }
        _ => String::new(),
    };
    if k == 0 {
        out.push(RLine { indent, label: "", role: Role::Head, text: format!("{} {}{place}", it.keyword, it.row.display), decl: Some(it.row.id) });
    }
    let theorem = it.row.kind == DeclKind::Theorem;
    let mut last_label = "";
    for p in &it.parts {
        let label = label_of(p.kind, theorem);
        let role = match p.kind {
            PartKind::Explicit => Role::Explicit,
            PartKind::Implicit | PartKind::Instance => Role::Quiet,
            PartKind::Then => Role::Then,
            PartKind::Text if k == 0 => Role::Text,
            PartKind::Text => Role::DefHead,
        };
        for (i, l) in p.text.lines().enumerate() {
            let first = i == 0;
            // An unfolded definition: its first line carries where it is.
            let text = if k != 0 && first && p.kind == PartKind::Text {
                format!("{l}{place}")
            } else if k != 0 {
                format!("  {l}")
            } else {
                l.to_string()
            };
            let shown = if first && label != last_label { label } else { "" };
            out.push(RLine { indent, label: shown, role: if k != 0 && !first { Role::Text } else { role }, text, decl: (k != 0 && first).then_some(it.row.id) });
            if first {
                last_label = label;
            }
        }
        if p.kind == PartKind::Text && it.cut_lines > 0 {
            out.push(RLine { indent, label: "", role: Role::Note, text: format!("… {} more lines", it.cut_lines), decl: None });
        }
        for &j in &p.vocab {
            render_item(c, j, indent + 4, out);
        }
        if !p.above.is_empty() {
            out.push(RLine { indent: indent + 4, label: "", role: Role::Note, text: format!("↑ {} unfolded above", p.above.join(", ")), decl: None });
        }
        if !p.folded.is_empty() {
            out.push(RLine { indent: indent + 4, label: "", role: Role::Note, text: format!("⋯ {} (folded)", p.folded.join(", ")), decl: None });
        }
    }
}

/// The contract as lines, the unfolded definitions indented under the
/// part that names them.
pub fn render(c: &Contract) -> Vec<RLine> {
    let mut out = Vec::new();
    if !c.items.is_empty() {
        render_item(c, 0, 0, &mut out);
    }
    out
}

/// The lines as plain text: the parts of the declaration in a labelled
/// column, each unfolded definition on a rail under the part that names it.
pub fn to_text(lines: &[RLine]) -> String {
    let mut s = String::new();
    for l in lines {
        let line = if l.role == Role::Head {
            l.text.clone()
        } else if l.indent == 0 {
            format!("  {:<6}{}", l.label, l.text)
        } else {
            format!("{}│ {}", " ".repeat(4 + l.indent as usize), l.text)
        };
        s.push_str(line.trim_end());
        s.push('\n');
    }
    s
}

/// One line saying where the names came from and what is left.
pub fn footer(c: &Contract) -> String {
    let n = c.items.len().saturating_sub(1);
    let how = if c.lean { "names as Lean resolved them (build artifacts)" } else { "names by lexical lookup (not resolved by Lean)" };
    let more = match (c.deeper, c.truncated) {
        (_, true) => "; the rest is folded: too much for one page",
        (true, false) => "; more can be unfolded",
        (false, false) => "; nothing of the project is left folded",
    };
    format!("{n} definition(s) unfolded to depth {}, {how}{more}", c.depth)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_header_splits_into_binder_groups_and_what_follows_the_colon() {
        let h = "theorem foo.{u} (F : Fam) {θ δ : ℝ} [inst : C θ] (h : ∀ y : ℝ, g y ≤ 1) : ZeroFree F θ";
        let (g, then) = split_header(h, 1, true);
        let texts: Vec<(PartKind, &str)> = g.iter().map(|&(k, a, z)| (k, &h[a..z])).collect();
        assert_eq!(texts, vec![(PartKind::Explicit, "(F : Fam)"), (PartKind::Implicit, "{θ δ : ℝ}"), (PartKind::Instance, "[inst : C θ]"), (PartKind::Explicit, "(h : ∀ y : ℝ, g y ≤ 1)")]);
        let (a, z) = then.unwrap();
        assert_eq!(h[a..z].trim(), "ZeroFree F θ");
        // No binders, and binders without brackets.
        let h2 = "theorem bar : T";
        assert_eq!(split_header(h2, 1, true).0.len(), 0);
        let h3 = "def f x y : Nat";
        let (g3, t3) = split_header(h3, 1, true);
        assert_eq!(g3.iter().map(|&(_, a, z)| h3[a..z].trim()).collect::<Vec<_>>(), vec!["x y"]);
        assert!(t3.is_some());
    }

    #[test]
    fn tidy_dedents_and_counts_what_it_leaves_out() {
        let (t, cut) = tidy("def a :=\n    one\n      two\n    three\n", 3);
        assert_eq!(t, "def a :=\none\n  two");
        assert_eq!(cut, 1);
    }
}
