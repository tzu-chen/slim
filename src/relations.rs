//! Typed relations between declarations, for the goal explorer.
//!
//! Three kinds of evidence are kept apart and every edge carries its label:
//!
//! * **curated**: a relation or argument step someone wrote in the project
//!   map. An explanation, not a Lean fact;
//! * **lexical**: an identifier in the source text equals an indexed name.
//!   Approximate, possibly wrong, never stored;
//! * **Lean**: a resolved constant in the elaborated type (T), value (V) or
//!   constructor list (C) of a declaration, from a semantic snapshot.
//!
//! The direction convention is fixed: "A uses B" runs from a statement A to
//! its prerequisite B; "used by" is the reverse. Queries are direct (one
//! step) and paged; the bounded connection search never materialises a
//! transitive closure.

use crate::db::query::{DeclRow, Direction};
use crate::db::IndexMode;
use crate::map::{ProjectMap, Provenance};
use crate::model::{flags, DeclKind, EdgeKind, Origin};
use crate::source::lexer::{LexProfile, Lexer, TokKind};
use crate::sourceview::SourceCache;
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hash;
use std::path::Path;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RelMode {
    Curated,
    Mentions,
    Lean,
    /// All three at once; every edge still shows its own label.
    Mixed,
}

impl RelMode {
    pub fn next(self) -> RelMode {
        match self {
            RelMode::Curated => RelMode::Mentions,
            RelMode::Mentions => RelMode::Lean,
            RelMode::Lean => RelMode::Mixed,
            RelMode::Mixed => RelMode::Curated,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            RelMode::Curated => "Curated argument",
            RelMode::Mentions => "Source mentions (lexical)",
            RelMode::Lean => "Lean dependencies",
            RelMode::Mixed => "All kinds (each edge labelled)",
        }
    }
    pub fn short(self) -> &'static str {
        match self {
            RelMode::Curated => "curated",
            RelMode::Mentions => "mentions",
            RelMode::Lean => "Lean deps",
            RelMode::Mixed => "all, labelled",
        }
    }
    pub fn explain(self) -> &'static str {
        match self {
            RelMode::Curated => "curated in the project map: an author's explanation, not checked by Lean",
            RelMode::Mentions => "lexical: identifiers in the source text that equal an indexed name; not resolved by Lean",
            RelMode::Lean => "Lean-derived: constants in the elaborated statement (T), proof term or body (V) or constructor list (C)",
            RelMode::Mixed => "all kinds, each labelled: [cur] curated · [lex] lexical · [T] [V] [C] Lean-derived",
        }
    }
}

/// Which Lean edges to follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TypeBody {
    All,
    /// Only references in elaborated statements (types).
    Type,
    /// Only references in proof terms and definition bodies.
    Body,
}

impl TypeBody {
    pub fn next(self) -> TypeBody {
        match self {
            TypeBody::All => TypeBody::Type,
            TypeBody::Type => TypeBody::Body,
            TypeBody::Body => TypeBody::All,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            TypeBody::All => "statement + proof/body",
            TypeBody::Type => "statement only (T)",
            TypeBody::Body => "proof/body only (V)",
        }
    }
    fn kinds(self) -> &'static [EdgeKind] {
        match self {
            TypeBody::All => &[EdgeKind::Type, EdgeKind::Value, EdgeKind::Ctor],
            TypeBody::Type => &[EdgeKind::Type],
            TypeBody::Body => &[EdgeKind::Value],
        }
    }
}

/// Why an edge is shown.
#[derive(Clone, Debug, PartialEq)]
pub enum EdgeLabel {
    Curated { kind: String, origin: Provenance, step: Option<u32> },
    Lexical { ident: String },
    Lean { type_: bool, body: bool, ctor: bool },
}

impl EdgeLabel {
    /// Short tag shown on every row; never colour alone.
    pub fn tag(&self) -> String {
        match self {
            EdgeLabel::Curated { step: Some(n), .. } => format!("cur {n}"),
            EdgeLabel::Curated { .. } => "cur".into(),
            EdgeLabel::Lexical { .. } => "lex".into(),
            EdgeLabel::Lean { type_, body, ctor } => {
                let mut v = Vec::new();
                if *type_ {
                    v.push("T");
                }
                if *body {
                    v.push("V");
                }
                if *ctor {
                    v.push("C");
                }
                v.join("+")
            }
        }
    }

    /// One sentence saying what the edge means, with `a` using `b`.
    pub fn explain(&self, a: &str, b: &str) -> String {
        match self {
            EdgeLabel::Curated { kind, origin, step } => {
                let s = step.map(|n| format!("step {n} of a curated argument")).unwrap_or_else(|| format!("curated relation \"{kind}\""));
                format!("{s}: the map says {a} uses {b} ({}); not Lean-derived", origin.label())
            }
            EdgeLabel::Lexical { ident } => {
                format!("lexical: the text of {a} contains `{ident}`, the name of {b}; not resolved by Lean")
            }
            EdgeLabel::Lean { type_, body, ctor } => {
                let mut v = Vec::new();
                if *type_ {
                    v.push("the elaborated statement");
                }
                if *body {
                    v.push("the proof term / body");
                }
                if *ctor {
                    v.push("the constructor list");
                }
                format!("Lean-derived: {b} occurs in {} of {a}", v.join(" and "))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Child {
    pub name: String,
    /// `None` when the name is not in the index.
    pub row: Option<DeclRow>,
    pub labels: Vec<EdgeLabel>,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChildPage {
    pub children: Vec<Child>,
    /// Lean edges of the node by kind (T, V, C), before any filter. `None`
    /// when Lean edges were not asked for or are unavailable.
    pub lean_counts: Option<[u64; 3]>,
    /// Excluded by the display filters (each capped at `COUNT_CAP`).
    pub hidden_external: u64,
    pub hidden_generated: u64,
    pub hidden_kind: u64,
    /// Offset of the next page of Lean edges, when there is one.
    pub more: Option<usize>,
    /// Lean edges passing the filters that are not shown yet (a lower bound
    /// when `remaining_capped`).
    pub remaining: u64,
    pub remaining_capped: bool,
    /// Explanations of what is unavailable or approximate (text, is-warning).
    pub notes: Vec<(String, bool)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChildQuery {
    pub name: String,
    pub mode: RelMode,
    pub dir: Direction,
    pub local_only: bool,
    pub show_generated: bool,
    pub tb: TypeBody,
    pub offset: usize,
    pub limit: usize,
}

pub const COUNT_CAP: u64 = 10_000;

/// What relation queries read: an index, optionally a map and source files.
pub struct Env<'a> {
    pub conn: &'a Connection,
    pub mode: IndexMode,
    pub map: Option<&'a ProjectMap>,
    pub source_root: Option<&'a Path>,
    pub cache: &'a mut SourceCache,
}

/// Where an identifier occurs in the source file: 1-based line and the
/// byte range within that line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Occurrence {
    pub line: u32,
    pub start: u32,
    pub end: u32,
}

/// At most this many places are recorded per identifier.
pub const MAX_OCCURRENCES: usize = 200;

#[derive(Clone, Debug)]
pub struct Mention {
    /// The identifier as first written.
    pub ident: String,
    /// One row per distinct name it may stand for (almost always one).
    pub candidates: Vec<DeclRow>,
    /// For each candidate: how many other declarations have its name
    /// (copies of a file, a reference beside its solution).
    pub same_name: Vec<u32>,
    /// Every place the identifier is written in the declaration's text.
    pub occurrences: Vec<Occurrence>,
}

fn row_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<DeclRow> {
    Ok(DeclRow {
        id: r.get(0)?,
        name: r.get(1)?,
        display: r.get(2)?,
        kind: DeclKind::from_u8(r.get::<_, u8>(3)?),
        origin: Origin::from_u8(r.get::<_, u8>(4)?),
        flags: r.get::<_, i64>(5)? as u32,
        module: r.get(6)?,
    })
}

const ROW_COLS: &str = "d.id, d.name, d.display, d.kind, d.origin, d.flags, m.name";

fn is_keyword(w: &str) -> bool {
    matches!(
        w,
        "theorem" | "lemma" | "def" | "fun" | "by" | "have" | "show" | "from" | "intro" | "exact" | "apply" | "simp" | "rw"
            | "at" | "with" | "match" | "if" | "then" | "else" | "let" | "in" | "do" | "where" | "Type" | "Prop" | "Sort"
            | "sorry" | "rfl" | "cases" | "induction" | "obtain" | "rcases" | "refine" | "constructor" | "omega" | "decide"
            | "instance" | "structure" | "class" | "inductive" | "abbrev" | "calc" | "using" | "only" | "this"
    )
}

/// Source "mentions": identifiers in the declaration's text that match
/// indexed names the way Lean looks a name up: under the enclosing
/// namespaces, innermost first; failing those, as written and under each
/// namespace opened where the declaration stands. Purely lexical. A
/// declaration is listed once, however often and however it is written;
/// of several declarations with one name the nearest file is listed.
pub fn lexical_mentions(conn: &Connection, root: Option<&Path>, cache: &mut SourceCache, id: i64) -> Result<Vec<Mention>> {
    let d = crate::db::query::detail(conn, id)?.ok_or_else(|| anyhow::anyhow!("declaration not found"))?;
    let (Some(root), Some(rel)) = (root, d.module_path.as_ref()) else { return Ok(Vec::new()) };
    let f = cache.get(root, rel)?;
    // Byte extent from a source-only scan, else the line range of a snapshot.
    let (sb, eb) = match (d.pos.kw_byte.or(d.pos.start_byte), d.pos.end_byte, d.pos.start_line) {
        (Some(sb), Some(eb), _) => (sb as usize, (eb as usize).min(f.text.len())),
        (_, _, Some(l0)) => {
            let l0 = (l0 as usize).max(1);
            let l1 = d.pos.end_line.map(|x| x as usize).unwrap_or(l0).max(l0);
            let sb = f.line_starts.get(l0 - 1).copied().unwrap_or(f.text.len());
            let eb = f.line_starts.get(l1).copied().unwrap_or(f.text.len());
            (sb, eb)
        }
        _ => return Ok(Vec::new()),
    };
    if sb >= eb || !f.text.is_char_boundary(sb) || !f.text.is_char_boundary(eb) {
        return Ok(Vec::new());
    }
    let text = &f.text[sb..eb];
    let mut lx = Lexer::new(text, LexProfile::NEWEST);
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut idents = Vec::new();
    let mut places: Vec<Vec<Occurrence>> = Vec::new();
    while let Some(t) = lx.next_token() {
        if t.kind == TokKind::Ident {
            let w = t.text(text);
            if w.len() <= 1 || is_keyword(w) {
                continue;
            }
            // The first 300 distinct identifiers are looked up; all their
            // occurrences are kept (bounded per identifier).
            let k = match seen.get(w) {
                Some(&k) => k,
                None if idents.len() < 300 => {
                    seen.insert(w.to_string(), idents.len());
                    idents.push(w.to_string());
                    places.push(Vec::new());
                    idents.len() - 1
                }
                None => continue,
            };
            if places[k].len() < MAX_OCCURRENCES {
                let abs = sb + t.start as usize;
                let line = f.line_of_byte(abs);
                let col = abs - f.line_starts.get(line.saturating_sub(1)).copied().unwrap_or(0);
                places[k].push(Occurrence { line: line as u32, start: col as u32, end: (col + (t.end - t.start) as usize) as u32 });
            }
        }
    }
    // The enclosing namespaces, innermost first.
    let ns = crate::model::namespace_of(&d.row.name).to_string();
    let comps = crate::model::name_components(&ns);
    let inner: Vec<String> = if ns.is_empty() { Vec::new() } else { (1..=comps.len()).rev().map(|k| comps[..k].join(".") + ".").collect() };
    // The root and the opened namespaces. `open B` inside `namespace A`
    // opens `A.B` when there is one; a namespace the index does not have
    // (a library) cannot match anything and is left out.
    let mut outer: Vec<String> = vec![String::new()];
    let opens = crate::scope::opens(conn, Some(root), cache, &d);
    cache.names.trim();
    {
        let mut has = conn.prepare_cached("SELECT 1 FROM decls WHERE name >= ?1 AND name < ?2 LIMIT 1")?;
        for o in opens.into_iter().take(16) {
            for p in inner.iter().map(|s| s.as_str()).chain(std::iter::once("")) {
                let full = format!("{p}{o}");
                let there = match cache.names.namespaces.get(&full) {
                    Some(&x) => x,
                    None => {
                        let x = has.exists(rusqlite::params![format!("{full}."), format!("{full}/")])?;
                        cache.names.namespaces.insert(full.clone(), x);
                        x
                    }
                };
                if there {
                    let full = full + ".";
                    if !outer.contains(&full) && !inner.contains(&full) {
                        outer.push(full);
                    }
                }
            }
        }
    }
    let mut wanted: Vec<String> = Vec::new();
    for ident in &idents {
        for p in inner.iter().chain(outer.iter()) {
            let full = format!("{p}{ident}");
            if !cache.names.rows.contains_key(&full) {
                wanted.push(full);
            }
        }
    }
    wanted.sort();
    wanted.dedup();
    // One indexed IN-query per 400 names instead of one query per name;
    // names looked up before are remembered.
    for chunk in wanted.chunks(400) {
        let marks = vec!["?"; chunk.len()].join(",");
        let mut st = conn.prepare(&format!("SELECT {ROW_COLS} FROM decls d LEFT JOIN modules m ON m.id = d.module_id WHERE d.name IN ({marks})"))?;
        let rows = st.query_map(rusqlite::params_from_iter(chunk.iter()), row_from)?;
        for r in rows {
            let r = r?;
            cache.names.rows.entry(r.name.clone()).or_default().push(r);
        }
    }
    for w in wanted {
        cache.names.rows.entry(w).or_default();
    }
    let memo = &cache.names.rows;
    // The declaration itself is not one of its mentions.
    let found = |full: &str| -> Option<Vec<DeclRow>> {
        let rows: Vec<DeclRow> = memo.get(full)?.iter().filter(|r| r.id != id).cloned().collect();
        (!rows.is_empty()).then_some(rows)
    };
    // Of several declarations with one name: the one in the nearest file.
    let here: Vec<&str> = d.row.module.as_deref().map(crate::model::name_components).unwrap_or_default();
    let nearest = |rows: &[DeclRow]| -> DeclRow {
        let near = |r: &DeclRow| -> usize {
            let m = r.module.as_deref().map(crate::model::name_components).unwrap_or_default();
            let shared = here.iter().zip(m.iter()).take_while(|(a, b)| a == b).count();
            // The same file beats any file beside it.
            shared * 2 + (m.len() == here.len() && shared == here.len()) as usize
        };
        rows.iter().max_by_key(|r| (near(r), std::cmp::Reverse(r.id))).cloned().expect("a found name has a row")
    };
    let mut out: Vec<Mention> = Vec::new();
    let mut by_target: HashMap<i64, usize> = HashMap::new();
    for (ident, occurrences) in idents.into_iter().zip(places) {
        let hit = |p: &String| found(&format!("{p}{ident}"));
        let mut groups: Vec<Vec<DeclRow>> = inner.iter().find_map(hit).into_iter().collect();
        if groups.is_empty() {
            groups = outer.iter().filter_map(hit).take(5).collect();
        }
        if groups.is_empty() {
            continue;
        }
        let candidates: Vec<DeclRow> = groups.iter().map(|g| nearest(g)).collect();
        let same_name: Vec<u32> = groups.iter().map(|g| g.len() as u32 - 1).collect();
        // Another spelling of a declaration already listed joins its row.
        if let [only] = candidates.as_slice() {
            if let Some(&k) = by_target.get(&only.id) {
                let m = &mut out[k];
                m.occurrences.extend(occurrences);
                m.occurrences.sort_by_key(|o| (o.line, o.start));
                m.occurrences.truncate(MAX_OCCURRENCES);
                continue;
            }
            by_target.insert(only.id, out.len());
        }
        out.push(Mention { ident, candidates, same_name, occurrences });
    }
    Ok(out)
}

fn push_child(page: &mut ChildPage, c: Child) {
    match page.children.iter_mut().find(|x| x.name == c.name) {
        Some(x) => {
            for l in c.labels {
                if !x.labels.contains(&l) {
                    x.labels.push(l);
                }
            }
            if x.note.is_none() {
                x.note = c.note;
            }
        }
        None => page.children.push(c),
    }
}

fn row_by_name(conn: &Connection, name: &str) -> Result<Option<DeclRow>> {
    Ok(conn
        .query_row(
            &format!("SELECT {ROW_COLS} FROM decls d LEFT JOIN modules m ON m.id = d.module_id WHERE d.name = ?1 ORDER BY d.origin, d.id LIMIT 1"),
            [name],
            row_from,
        )
        .optional()?)
}

fn cols(dir: Direction) -> (&'static str, &'static str) {
    match dir {
        Direction::Uses => ("src", "dst"),
        Direction::UsedBy => ("dst", "src"),
    }
}

fn kind_list(kinds: &[EdgeKind]) -> String {
    kinds.iter().map(|k| (*k as u8).to_string()).collect::<Vec<_>>().join(",")
}

/// Whether an edge of `kind` joins the two declarations (in `dir`).
fn has_edge(conn: &Connection, dir: Direction, node: i64, other: i64, kind: EdgeKind) -> Result<bool> {
    let (src, dst) = if dir == Direction::Uses { (node, other) } else { (other, node) };
    let mut st = conn.prepare_cached("SELECT 1 FROM edges WHERE src = ?1 AND kind = ?2 AND dst = ?3")?;
    Ok(st.query_row(params![src, kind as i64, dst], |_| Ok(())).optional()?.is_some())
}

fn lean_label(conn: &Connection, dir: Direction, node: i64, other: i64) -> Result<EdgeLabel> {
    Ok(EdgeLabel::Lean {
        type_: has_edge(conn, dir, node, other, EdgeKind::Type)?,
        body: has_edge(conn, dir, node, other, EdgeKind::Value)?,
        ctor: has_edge(conn, dir, node, other, EdgeKind::Ctor)?,
    })
}

fn lean_page(conn: &Connection, id: i64, q: &ChildQuery, page: &mut ChildPage) -> Result<()> {
    let (self_col, other_col) = cols(q.dir);
    let kinds = q.tb.kinds();
    let kl = kind_list(kinds);
    let mut counts = [0u64; 3];
    {
        let mut st = conn.prepare_cached(&format!("SELECT kind, count(*) FROM edges WHERE {self_col} = ?1 GROUP BY kind"))?;
        let rows = st.query_map([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
        for r in rows {
            let (k, n) = r?;
            if (1..=3).contains(&k) {
                counts[k as usize - 1] = n as u64;
            }
        }
    }
    page.lean_counts = Some(counts);
    let mut fs = String::new();
    if q.local_only {
        fs.push_str(" AND d.origin IN (0, 3)");
    }
    if !q.show_generated {
        fs.push_str(&format!(" AND (d.flags & {}) = 0", flags::GENERATED));
    }
    let capped = |extra: &str| -> Result<u64> {
        let n: i64 = conn.query_row(
            &format!(
                "SELECT count(*) FROM (SELECT 1 FROM edges e JOIN decls d ON d.id = e.{other_col}
                  WHERE e.{self_col} = ?1 AND e.kind IN ({kl}) {extra} LIMIT {})",
                COUNT_CAP + 1
            ),
            [id],
            |r| r.get(0),
        )?;
        Ok(n as u64)
    };
    if q.offset == 0 {
        if q.local_only {
            page.hidden_external = capped(" AND d.origin NOT IN (0, 3)")?.min(COUNT_CAP);
        }
        if !q.show_generated {
            page.hidden_generated = capped(&format!(" AND (d.flags & {}) != 0", flags::GENERATED))?.min(COUNT_CAP);
        }
        page.hidden_kind = EdgeKind::ALL.iter().filter(|k| !kinds.contains(k)).map(|k| counts[*k as usize - 1]).sum();
    }
    let mut st = conn.prepare_cached(&format!(
        "SELECT {ROW_COLS}, e.kind FROM edges e JOIN decls d ON d.id = e.{other_col} LEFT JOIN modules m ON m.id = d.module_id
          WHERE e.{self_col} = ?1 AND e.kind IN ({kl}) {fs} ORDER BY e.kind, e.{other_col} LIMIT ?2 OFFSET ?3"
    ))?;
    let rows: Vec<(DeclRow, u8)> = st
        .query_map(params![id, q.limit as i64 + 1, q.offset as i64], |r| Ok((row_from(r)?, r.get::<_, u8>(7)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let more = rows.len() > q.limit;
    for (row, k) in rows.into_iter().take(q.limit) {
        let kind = EdgeKind::from_u8(k).unwrap_or(EdgeKind::Value);
        // A declaration referenced in both the statement and the proof is
        // listed once, under the first kind, with a combined label.
        let earlier = kinds.iter().take_while(|x| **x != kind);
        let mut listed = false;
        for e in earlier {
            if has_edge(conn, q.dir, id, row.id, *e)? {
                listed = true;
                break;
            }
        }
        if listed {
            continue;
        }
        let label = if q.tb == TypeBody::All {
            lean_label(conn, q.dir, id, row.id)?
        } else {
            EdgeLabel::Lean { type_: kind == EdgeKind::Type, body: kind == EdgeKind::Value, ctor: kind == EdgeKind::Ctor }
        };
        push_child(page, Child { name: row.name.clone(), row: Some(row), labels: vec![label], note: None });
    }
    if more {
        let shown = (q.offset + q.limit) as u64;
        let matching = capped(&fs)?;
        page.more = Some(q.offset + q.limit);
        page.remaining = matching.min(COUNT_CAP).saturating_sub(shown);
        page.remaining_capped = matching > COUNT_CAP;
    }
    Ok(())
}

/// Direct relations of one declaration, of the asked kind(s) and direction.
pub fn children(env: &mut Env, q: &ChildQuery) -> Result<ChildPage> {
    let mut page = ChildPage::default();
    let me = row_by_name(env.conn, &q.name)?;
    let uses = q.dir == Direction::Uses;
    let (curated, lexical, lean) = match q.mode {
        RelMode::Curated => (true, false, false),
        RelMode::Mentions => (false, true, false),
        RelMode::Lean => (false, false, true),
        RelMode::Mixed => (true, true, true),
    };
    if curated && q.offset == 0 {
        match env.map {
            None => page.notes.push(("no project map: nothing curated".into(), false)),
            Some(m) => {
                let rels = m.curated(&q.name, uses);
                if rels.is_empty() && q.mode == RelMode::Curated {
                    page.notes.push((
                        format!("no curated {} in the map", if uses { "prerequisites" } else { "dependents" }),
                        false,
                    ));
                }
                for r in rels {
                    let other = if uses { &r.to } else { &r.from };
                    let row = row_by_name(env.conn, other)?;
                    push_child(
                        &mut page,
                        Child {
                            name: other.clone(),
                            row,
                            labels: vec![EdgeLabel::Curated { kind: r.kind.clone(), origin: r.origin, step: r.step }],
                            note: r.note.clone(),
                        },
                    );
                }
            }
        }
    }
    if lean {
        match (&me, env.mode) {
            (_, IndexMode::Source) => page.notes.push((
                "Lean dependencies are unavailable in a source-only index".into(),
                true,
            )),
            (None, _) => page.notes.push((format!("{} is not in this index", q.name), true)),
            (Some(r), _) if r.origin == Origin::Unresolved => page.notes.push((
                "named by the snapshot but not described in it: no dependencies extracted (coverage gap)".into(),
                true,
            )),
            (Some(r), IndexMode::Semantic) => {
                lean_page(env.conn, r.id, q, &mut page)?;
                if uses && q.offset == 0 {
                    let cov: Option<(u8, u8)> =
                        env.conn.query_row("SELECT type_cov, value_cov FROM decls WHERE id = ?1", [r.id], |x| Ok((x.get(0)?, x.get(1)?))).optional()?;
                    if let Some((t, v)) = cov {
                        use crate::model::Coverage;
                        if Coverage::from_u8(t).is_incomplete() || (r.kind.has_value() && Coverage::from_u8(v).is_incomplete()) {
                            page.notes.push(("coverage incomplete: missing edges are not evidence of no dependency".into(), true));
                        }
                    }
                }
            }
        }
    }
    if lexical && q.offset == 0 {
        match (&me, uses) {
            (_, false) => page.notes.push((
                "source mentions run outward only: there is no \"mentioned by\"".into(),
                q.mode == RelMode::Mentions,
            )),
            (None, _) => page.notes.push((format!("{} is not in this index", q.name), true)),
            (Some(r), true) => {
                let ms = lexical_mentions(env.conn, env.source_root, env.cache, r.id)?;
                if env.source_root.is_none() {
                    page.notes.push(("no source root for this index: source mentions cannot be computed".into(), true));
                }
                let mut hidden = 0u64;
                let mut n = 0usize;
                for m in ms {
                    for row in m.candidates {
                        if q.local_only && !row.origin.is_local() {
                            hidden += 1;
                            continue;
                        }
                        n += 1;
                        if n > 200 {
                            continue;
                        }
                        push_child(&mut page, Child { name: row.name.clone(), row: Some(row), labels: vec![EdgeLabel::Lexical { ident: m.ident.clone() }], note: None });
                    }
                }
                page.hidden_external += hidden;
                if n > 200 {
                    page.notes.push((format!("{} further lexical candidates are not listed.", n - 200), false));
                }
            }
        }
    }
    Ok(page)
}

// ---------------------------------------------------------------------------
// Bounded connection search
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Stop after visiting this many declarations.
    pub max_nodes: usize,
    pub max_ms: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_nodes: 200_000, max_ms: 1500 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Bfs<N> {
    Found(Vec<N>),
    /// Everything reachable was visited and the target is not among it.
    Exhausted,
    Truncated { by_time: bool },
    Cancelled,
}

/// Breadth-first search for a shortest path, bounded by work and time and
/// cancellable between expansions. Returns the outcome and how many nodes
/// were visited.
pub fn bfs<N: Clone + Eq + Hash>(
    start: N,
    target: &N,
    mut next: impl FnMut(&N) -> Result<Vec<N>>,
    lim: &Limits,
    cancel: &dyn Fn() -> bool,
) -> Result<(Bfs<N>, usize)> {
    if &start == target {
        return Ok((Bfs::Found(vec![start]), 1));
    }
    let t0 = Instant::now();
    let mut prev: HashMap<N, N> = HashMap::new();
    let mut seen: HashSet<N> = HashSet::new();
    seen.insert(start.clone());
    let mut queue: VecDeque<N> = VecDeque::new();
    queue.push_back(start.clone());
    while let Some(n) = queue.pop_front() {
        if cancel() {
            return Ok((Bfs::Cancelled, seen.len()));
        }
        if t0.elapsed().as_millis() as u64 > lim.max_ms {
            return Ok((Bfs::Truncated { by_time: true }, seen.len()));
        }
        for m in next(&n)? {
            if !seen.insert(m.clone()) {
                continue;
            }
            prev.insert(m.clone(), n.clone());
            if &m == target {
                let mut path = vec![m];
                while let Some(p) = prev.get(path.last().unwrap()) {
                    path.push(p.clone());
                }
                path.reverse();
                return Ok((Bfs::Found(path), seen.len()));
            }
            if seen.len() >= lim.max_nodes {
                return Ok((Bfs::Truncated { by_time: false }, seen.len()));
            }
            queue.push_back(m);
        }
    }
    Ok((Bfs::Exhausted, seen.len()))
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConnQuery {
    pub from: String,
    pub to: String,
    pub mode: RelMode,
    pub dir: Direction,
    pub tb: TypeBody,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConnStep {
    pub name: String,
    pub row: Option<DeclRow>,
    /// Evidence for the step from this declaration to the next one.
    pub next: Option<EdgeLabel>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConnOutcome {
    Path(Vec<ConnStep>),
    /// The whole reachable set was searched: no path exists in this data.
    NoPath,
    /// Stopped at a limit: no conclusion either way.
    Truncated { by_time: bool },
    /// The asked relation kind cannot be searched here.
    Unavailable(String),
    Cancelled,
    Error(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConnResult {
    pub query: ConnQuery,
    pub outcome: ConnOutcome,
    pub explored: usize,
    pub ms: u64,
}

impl ConnQuery {
    /// "A uses … uses B" / "A is used by … B" with the relation kind spelled out.
    pub fn describe(&self) -> String {
        let rel = match self.mode {
            RelMode::Lean => format!("Lean dependencies, {}", self.tb.label()),
            m => m.label().to_string(),
        };
        format!(
            "{} {} {} — via {rel}",
            self.from,
            if self.dir == Direction::Uses { "uses (transitively)" } else { "is used by (transitively)" },
            self.to
        )
    }
}

/// Searches for a path between two declarations along one relation kind in
/// one direction.
pub fn connection(env: &mut Env, q: &ConnQuery, lim: &Limits, cancel: &dyn Fn() -> bool) -> ConnResult {
    let t0 = Instant::now();
    let (outcome, explored) = match connection_inner(env, q, lim, cancel) {
        Ok(x) => x,
        Err(e) => (ConnOutcome::Error(format!("{e:#}")), 0),
    };
    ConnResult { query: q.clone(), outcome, explored, ms: t0.elapsed().as_millis() as u64 }
}

fn connection_inner(env: &mut Env, q: &ConnQuery, lim: &Limits, cancel: &dyn Fn() -> bool) -> Result<(ConnOutcome, usize)> {
    let uses = q.dir == Direction::Uses;
    match q.mode {
        RelMode::Mixed => Ok((
            ConnOutcome::Unavailable("a connection query follows one relation kind (curated, source mentions or Lean dependencies)".into()),
            0,
        )),
        RelMode::Curated => {
            let Some(map) = env.map else {
                return Ok((ConnOutcome::Unavailable("no project map, so no curated relations to follow".into()), 0));
            };
            let (b, n) = bfs(
                q.from.clone(),
                &q.to,
                |n| Ok(map.curated(n, uses).into_iter().map(|r| if uses { r.to.clone() } else { r.from.clone() }).collect()),
                lim,
                cancel,
            )?;
            let out = match b {
                Bfs::Found(path) => {
                    let mut steps = Vec::new();
                    for (i, name) in path.iter().enumerate() {
                        let next = path.get(i + 1).and_then(|nx| {
                            map.curated(name, uses).into_iter().find(|r| if uses { &r.to == nx } else { &r.from == nx }).map(|r| EdgeLabel::Curated {
                                kind: r.kind.clone(),
                                origin: r.origin,
                                step: r.step,
                            })
                        });
                        steps.push(ConnStep { name: name.clone(), row: row_by_name(env.conn, name)?, next });
                    }
                    ConnOutcome::Path(steps)
                }
                Bfs::Exhausted => ConnOutcome::NoPath,
                Bfs::Truncated { by_time } => ConnOutcome::Truncated { by_time },
                Bfs::Cancelled => ConnOutcome::Cancelled,
            };
            Ok((out, n))
        }
        RelMode::Lean | RelMode::Mentions => {
            if q.mode == RelMode::Lean && env.mode == IndexMode::Source {
                return Ok((
                    ConnOutcome::Unavailable("a source-only index has no Lean dependencies to search".into()),
                    0,
                ));
            }
            if q.mode == RelMode::Mentions && !uses {
                return Ok((ConnOutcome::Unavailable("source mentions run outward only (\"uses\")".into()), 0));
            }
            let (Some(a), Some(b)) = (row_by_name(env.conn, &q.from)?, row_by_name(env.conn, &q.to)?) else {
                let missing = if row_by_name(env.conn, &q.from)?.is_none() { &q.from } else { &q.to };
                return Ok((ConnOutcome::Unavailable(format!("{missing} is not in this index.")), 0));
            };
            let conn = env.conn;
            let (res, n) = if q.mode == RelMode::Lean {
                let (self_col, other_col) = cols(q.dir);
                let mut st = conn.prepare(&format!("SELECT {other_col} FROM edges WHERE {self_col} = ?1 AND kind IN ({})", kind_list(q.tb.kinds())))?;
                bfs(a.id, &b.id, |n| Ok(st.query_map([*n], |r| r.get(0))?.collect::<rusqlite::Result<Vec<i64>>>()?), lim, cancel)?
            } else {
                // Each expansion reads and lexes source text: a much smaller budget.
                let lim = Limits { max_nodes: lim.max_nodes.min(400), max_ms: lim.max_ms };
                let (root, cache) = (env.source_root, &mut *env.cache);
                bfs(
                    a.id,
                    &b.id,
                    |n| Ok(lexical_mentions(conn, root, cache, *n)?.into_iter().flat_map(|m| m.candidates.into_iter().map(|c| c.id)).collect()),
                    &lim,
                    cancel,
                )?
            };
            let out = match res {
                Bfs::Found(ids) => {
                    let mut steps = Vec::new();
                    for (i, id) in ids.iter().enumerate() {
                        let row = crate::db::query::row(conn, *id)?;
                        let next = match ids.get(i + 1) {
                            None => None,
                            Some(nx) if q.mode == RelMode::Lean => {
                                let l = lean_label(conn, q.dir, *id, *nx)?;
                                Some(match (q.tb, l) {
                                    (TypeBody::Type, _) => EdgeLabel::Lean { type_: true, body: false, ctor: false },
                                    (TypeBody::Body, _) => EdgeLabel::Lean { type_: false, body: true, ctor: false },
                                    (_, l) => l,
                                })
                            }
                            Some(nx) => {
                                let ident = lexical_mentions(conn, env.source_root, env.cache, *id)?
                                    .into_iter()
                                    .find(|m| m.candidates.iter().any(|c| c.id == *nx))
                                    .map(|m| m.ident)
                                    .unwrap_or_default();
                                Some(EdgeLabel::Lexical { ident })
                            }
                        };
                        steps.push(ConnStep { name: row.as_ref().map(|r| r.name.clone()).unwrap_or_default(), row, next });
                    }
                    ConnOutcome::Path(steps)
                }
                Bfs::Exhausted => ConnOutcome::NoPath,
                Bfs::Truncated { by_time } => ConnOutcome::Truncated { by_time },
                Bfs::Cancelled => ConnOutcome::Cancelled,
            };
            Ok((out, n))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn never() -> bool {
        false
    }

    #[test]
    fn bfs_finds_a_shortest_path_and_survives_cycles() {
        // 0 → 1 → 2 → 0 (cycle), 2 → 3, 0 → 4 → 3
        let g: HashMap<u32, Vec<u32>> = [(0, vec![1, 4]), (1, vec![2]), (2, vec![0, 3]), (4, vec![3])].into_iter().collect();
        let next = |n: &u32| Ok(g.get(n).cloned().unwrap_or_default());
        let (r, _) = bfs(0u32, &3, next, &Limits::default(), &never).unwrap();
        assert_eq!(r, Bfs::Found(vec![0, 4, 3]), "the two-step route, not the three-step one");
        // Direction matters: nothing leads from 3 back to 0.
        let (r, n) = bfs(3u32, &0, next, &Limits::default(), &never).unwrap();
        assert_eq!((r, n), (Bfs::Exhausted, 1));
        // A cycle with no exit terminates.
        let (r, n) = bfs(1u32, &99, next, &Limits::default(), &never).unwrap();
        assert_eq!((r, n), (Bfs::Exhausted, 5));
        let (r, _) = bfs(7u32, &7, next, &Limits::default(), &never).unwrap();
        assert_eq!(r, Bfs::Found(vec![7]));
    }

    #[test]
    fn bfs_reports_truncation_and_cancellation_distinctly() {
        // An adversarial hub: node 0 has 100,000 successors, none the target.
        let next = |n: &u64| Ok(if *n == 0 { (1..=100_000u64).collect() } else { vec![*n + 100_000] });
        let (r, n) = bfs(0u64, &u64::MAX, next, &Limits { max_nodes: 5_000, max_ms: 10_000 }, &never).unwrap();
        assert_eq!(r, Bfs::Truncated { by_time: false });
        assert_eq!(n, 5_000, "work is bounded by the node budget, not by the fan-out");
        // An endless chain stops at the time limit.
        let slow = |n: &u64| {
            std::thread::sleep(std::time::Duration::from_millis(2));
            Ok(vec![*n + 1])
        };
        let (r, _) = bfs(0u64, &u64::MAX, slow, &Limits { max_nodes: usize::MAX, max_ms: 30 }, &never).unwrap();
        assert_eq!(r, Bfs::Truncated { by_time: true });
        // Cancellation is checked between expansions.
        let calls = std::cell::Cell::new(0);
        let cancel = || {
            calls.set(calls.get() + 1);
            calls.get() > 3
        };
        let (r, _) = bfs(0u64, &u64::MAX, |n: &u64| Ok(vec![*n + 1]), &Limits::default(), &cancel).unwrap();
        assert_eq!(r, Bfs::Cancelled);
    }

    #[test]
    fn labels_never_blur_kinds() {
        let cur = EdgeLabel::Curated { kind: "uses".into(), origin: Provenance::Authored, step: Some(2) };
        let lex = EdgeLabel::Lexical { ident: "foo".into() };
        let lean = EdgeLabel::Lean { type_: true, body: true, ctor: false };
        assert_eq!((cur.tag(), lex.tag(), lean.tag()), ("cur 2".to_string(), "lex".to_string(), "T+V".to_string()));
        assert!(cur.explain("A", "B").contains("not Lean-derived"));
        assert!(lex.explain("A", "B").contains("not resolved by Lean"));
        assert!(lean.explain("A", "B").starts_with("Lean-derived"));
        // Only the Lean mode is called "Lean dependencies".
        for m in [RelMode::Curated, RelMode::Mentions, RelMode::Mixed] {
            assert!(!m.label().contains("Lean dep") && !m.label().to_lowercase().contains("proof dep"));
        }
    }
}
