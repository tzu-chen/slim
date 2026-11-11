//! Read-only queries used by the UI workers. All neighbour queries are
//! bounded (`LIMIT n + 1` to detect truncation) and walk indexes in id
//! order, which the importer made a stable sort order (local first, then by
//! name), so no query sorts a large result set.

use crate::model::{AxiomCategory, Coverage, DeclKind, EdgeKind, KindFilter, Origin};
use crate::snapshot::Binder;
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

#[derive(Clone, Debug, PartialEq)]
pub struct DeclRow {
    pub id: i64,
    pub name: String,
    pub display: String,
    pub kind: DeclKind,
    pub origin: Origin,
    pub flags: u32,
    pub module: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Signature {
    pub binders: Vec<Binder>,
    pub conclusion: String,
}

impl Signature {
    /// Binders as written groups, `(a b : T)`, `{x : U}`, `[Inst]`; the flag
    /// says whether implicit or instance arguments were left out.
    pub fn binder_groups(&self, show_implicit: bool) -> (Vec<String>, bool) {
        let b = &self.binders;
        let mut groups: Vec<String> = Vec::new();
        let mut hidden = false;
        let mut i = 0;
        while i < b.len() {
            let x = &b[i];
            let implicitish = x.info != "default";
            if implicitish && !show_implicit {
                hidden = true;
                i += 1;
                continue;
            }
            let mut names = vec![x.name.clone()];
            let mut j = i + 1;
            while j < b.len() && b[j].info == x.info && b[j].type_ == x.type_ && x.info != "inst_implicit" {
                names.push(b[j].name.clone());
                j += 1;
            }
            let (o, c) = match x.info.as_str() {
                "implicit" => ("{", "}"),
                "strict_implicit" => ("⦃", "⦄"),
                "inst_implicit" => ("[", "]"),
                _ => ("(", ")"),
            };
            let g = if x.info == "inst_implicit" && x.name.ends_with('✝') {
                format!("{o}{}{c}", x.type_)
            } else {
                format!("{o}{} : {}{c}", names.join(" "), x.type_)
            };
            groups.push(g);
            i = j;
        }
        (groups, hidden)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AxiomSummary {
    pub axiom_id: i64,
    pub name: String,
    pub category: AxiomCategory,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Pos {
    pub start_line: Option<u32>,
    pub start_col: Option<u32>,
    pub end_line: Option<u32>,
    pub end_col: Option<u32>,
    pub sel_line: Option<u32>,
    pub sel_col: Option<u32>,
    pub start_byte: Option<u32>,
    pub kw_byte: Option<u32>,
    pub header_end_byte: Option<u32>,
    pub end_byte: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeclDetail {
    pub row: DeclRow,
    pub module_id: Option<i64>,
    pub module_path: Option<String>,
    pub module_status: Option<String>,
    pub module_sha256: Option<String>,
    pub type_cov: Coverage,
    pub value_cov: Coverage,
    pub pos: Pos,
    pub type_text: Option<String>,
    pub sig: Option<Signature>,
    pub doc: Option<String>,
    pub attrs: Option<String>,
    pub generated: Option<String>,
    pub keyword: Option<String>,
    pub sorry_line: Option<u32>,
    /// out_type, out_value, out_ctor, in_type, in_value, in_ctor
    pub degrees: [u64; 6],
    pub axioms: Vec<AxiomSummary>,
    pub lean_axioms: Option<Vec<String>>,
    /// Other declarations sharing this exact name (source mode ambiguity).
    pub same_name: u64,
}

const ROW_COLS: &str = "d.id, d.name, d.display, d.kind, d.origin, d.flags, m.name";

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

pub fn row(conn: &Connection, id: i64) -> Result<Option<DeclRow>> {
    Ok(conn
        .query_row(
            &format!("SELECT {ROW_COLS} FROM decls d LEFT JOIN modules m ON m.id = d.module_id WHERE d.id = ?1"),
            [id],
            row_from,
        )
        .optional()?)
}

pub fn rows_by_ids(conn: &Connection, ids: &[i64]) -> Result<Vec<DeclRow>> {
    let mut st = conn.prepare_cached(&format!(
        "SELECT {ROW_COLS} FROM decls d LEFT JOIN modules m ON m.id = d.module_id WHERE d.id = ?1"
    ))?;
    let mut out = Vec::with_capacity(ids.len());
    for &id in ids {
        if let Some(r) = st.query_row([id], row_from).optional()? {
            out.push(r);
        }
    }
    Ok(out)
}

pub fn decl_by_name(conn: &Connection, name: &str) -> Result<Option<i64>> {
    Ok(conn
        .query_row("SELECT id FROM decls WHERE name = ?1 ORDER BY origin, id LIMIT 1", [name], |r| r.get(0))
        .optional()?)
}

pub fn detail(conn: &Connection, id: i64) -> Result<Option<DeclDetail>> {
    let d = conn
        .query_row(
            &format!(
                "SELECT {ROW_COLS}, d.module_id, m.path, m.status, m.sha256, d.type_cov, d.value_cov,
                    d.start_line, d.start_col, d.end_line, d.end_col, d.sel_line, d.sel_col,
                    d.start_byte, d.kw_byte, d.header_end_byte, d.end_byte,
                    d.type_text, d.sig_json, d.doc, d.attrs, d.generated, d.keyword, d.sorry_line,
                    d.out_type, d.out_value, d.out_ctor, d.in_type, d.in_value, d.in_ctor
                 FROM decls d LEFT JOIN modules m ON m.id = d.module_id WHERE d.id = ?1"
            ),
            [id],
            |r| {
                let sig: Option<String> = r.get(24)?;
                let sig = sig.and_then(|s| {
                    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
                    let binders: Vec<Binder> = serde_json::from_value(v.get("binders")?.clone()).ok()?;
                    let conclusion = v.get("conclusion")?.as_str()?.to_string();
                    Some(Signature { binders, conclusion })
                });
                let mut degrees = [0u64; 6];
                for (k, slot) in degrees.iter_mut().enumerate() {
                    *slot = r.get::<_, i64>(30 + k)? as u64;
                }
                Ok(DeclDetail {
                    row: row_from(r)?,
                    module_id: r.get(7)?,
                    module_path: r.get(8)?,
                    module_status: r.get(9)?,
                    module_sha256: r.get(10)?,
                    type_cov: Coverage::from_u8(r.get::<_, u8>(11)?),
                    value_cov: Coverage::from_u8(r.get::<_, u8>(12)?),
                    pos: Pos {
                        start_line: r.get(13)?,
                        start_col: r.get(14)?,
                        end_line: r.get(15)?,
                        end_col: r.get(16)?,
                        sel_line: r.get(17)?,
                        sel_col: r.get(18)?,
                        start_byte: r.get(19)?,
                        kw_byte: r.get(20)?,
                        header_end_byte: r.get(21)?,
                        end_byte: r.get(22)?,
                    },
                    type_text: r.get(23)?,
                    sig,
                    doc: r.get(25)?,
                    attrs: r.get(26)?,
                    generated: r.get(27)?,
                    keyword: r.get(28)?,
                    sorry_line: r.get(29)?,
                    degrees,
                    axioms: Vec::new(),
                    lean_axioms: None,
                    same_name: 0,
                })
            },
        )
        .optional()?;
    let Some(mut d) = d else { return Ok(None) };
    let mut st = conn.prepare_cached(
        "SELECT a.id, a.name, a.category FROM decl_axioms da JOIN axioms a ON a.id = da.axiom_id
          WHERE da.decl_id = ?1 ORDER BY a.category, a.name",
    )?;
    d.axioms = st
        .query_map([id], |r| {
            Ok(AxiomSummary { axiom_id: r.get(0)?, name: r.get(1)?, category: AxiomCategory::from_u8(r.get::<_, u8>(2)?) })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut st = conn.prepare_cached("SELECT axiom_name FROM lean_axioms WHERE decl_id = ?1 ORDER BY axiom_name")?;
    let la: Vec<String> = st.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let has_lean: bool = conn
        .query_row("SELECT EXISTS(SELECT 1 FROM lean_axioms LIMIT 1)", [], |r| r.get(0))
        .unwrap_or(false);
    if has_lean && d.row.origin == Origin::Local {
        d.lean_axioms = Some(la);
    }
    d.same_name = conn.query_row("SELECT count(*) - 1 FROM decls WHERE name = ?1", [&d.row.name], |r| r.get::<_, i64>(0))? as u64;
    Ok(Some(d))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// What this declaration refers to.
    Uses,
    /// What refers to this declaration.
    UsedBy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NeighborFilter {
    pub local_only: bool,
    pub show_generated: bool,
    pub kinds: KindFilter,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NeighborGroup {
    pub kind: EdgeKind,
    /// Total neighbours of this edge kind (unfiltered, exact).
    pub total: u64,
    /// Neighbours passing the filter, counted up to `count_cap`.
    pub matching: u64,
    pub matching_capped: bool,
    pub rows: Vec<DeclRow>,
    pub truncated: bool,
}

pub const COUNT_CAP: u64 = 10_000;

fn filter_sql(f: &NeighborFilter) -> String {
    let mut s = String::new();
    if f.local_only {
        s.push_str(" AND d.origin IN (0, 3)");
    }
    if !f.show_generated {
        s.push_str(&format!(" AND (d.flags & {}) = 0", crate::model::flags::GENERATED));
    }
    let kinds: Vec<u8> = DeclKind::ALL.iter().filter(|k| f.kinds.accepts(**k)).map(|k| *k as u8).collect();
    if kinds.len() != DeclKind::ALL.len() {
        let list: Vec<String> = kinds.iter().map(|k| k.to_string()).collect();
        s.push_str(&format!(" AND d.kind IN ({})", list.join(",")));
    }
    s
}

/// Direct neighbours grouped by edge kind, each group bounded by `limit`.
pub fn neighbors(conn: &Connection, id: i64, dir: Direction, f: &NeighborFilter, limit: usize, offset: usize) -> Result<Vec<NeighborGroup>> {
    let (self_col, other_col) = match dir {
        Direction::Uses => ("src", "dst"),
        Direction::UsedBy => ("dst", "src"),
    };
    let fs = filter_sql(f);
    let mut groups = Vec::new();
    for ek in EdgeKind::ALL {
        let total: i64 = conn.query_row(
            &format!("SELECT count(*) FROM edges WHERE {self_col} = ?1 AND kind = ?2"),
            params![id, ek as i64],
            |r| r.get(0),
        )?;
        if total == 0 {
            continue;
        }
        let matching: i64 = conn.query_row(
            &format!(
                "SELECT count(*) FROM (SELECT 1 FROM edges e JOIN decls d ON d.id = e.{other_col}
                  WHERE e.{self_col} = ?1 AND e.kind = ?2 {fs} LIMIT {})",
                COUNT_CAP + 1
            ),
            params![id, ek as i64],
            |r| r.get(0),
        )?;
        let mut st = conn.prepare_cached(&format!(
            "SELECT {ROW_COLS} FROM edges e JOIN decls d ON d.id = e.{other_col} LEFT JOIN modules m ON m.id = d.module_id
              WHERE e.{self_col} = ?1 AND e.kind = ?2 {fs} ORDER BY e.{other_col} LIMIT ?3 OFFSET ?4"
        ))?;
        let mut rows: Vec<DeclRow> = st
            .query_map(params![id, ek as i64, limit as i64 + 1, offset as i64], row_from)?
            .collect::<rusqlite::Result<_>>()?;
        let truncated = rows.len() > limit;
        rows.truncate(limit);
        groups.push(NeighborGroup {
            kind: ek,
            total: total as u64,
            matching: (matching as u64).min(COUNT_CAP),
            matching_capped: matching as u64 > COUNT_CAP,
            rows,
            truncated,
        });
    }
    Ok(groups)
}

#[derive(Clone, Debug, PartialEq)]
pub struct EvidenceStep {
    pub row: DeclRow,
    /// Edge kind used to reach the *next* step.
    pub next_kind: Option<EdgeKind>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AxiomEvidence {
    pub axiom: AxiomSummary,
    pub path: Vec<EvidenceStep>,
    pub path_truncated: bool,
}

pub const MAX_PATH: usize = 256;

/// For every axiom (and the coverage-gap pseudo-axiom) reachable from `id`,
/// a witness path following the recorded `via` pointers.
pub fn axiom_evidence(conn: &Connection, id: i64) -> Result<Vec<AxiomEvidence>> {
    let mut st = conn.prepare_cached(
        "SELECT a.id, a.name, a.category FROM decl_axioms da JOIN axioms a ON a.id = da.axiom_id
          WHERE da.decl_id = ?1 ORDER BY a.category, a.name",
    )?;
    let axs: Vec<AxiomSummary> = st
        .query_map([id], |r| {
            Ok(AxiomSummary { axiom_id: r.get(0)?, name: r.get(1)?, category: AxiomCategory::from_u8(r.get::<_, u8>(2)?) })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut via_st = conn.prepare_cached("SELECT via, via_kind FROM decl_axioms WHERE decl_id = ?1 AND axiom_id = ?2")?;
    let mut out = Vec::new();
    for ax in axs {
        let mut path = Vec::new();
        let mut cur = id;
        let mut truncated = false;
        let mut seen = std::collections::HashSet::new();
        loop {
            let Some(r) = row(conn, cur)? else { break };
            let step: Option<(Option<i64>, Option<u8>)> =
                via_st.query_row(params![cur, ax.axiom_id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
            let (via, vk) = step.unwrap_or((None, None));
            path.push(EvidenceStep { row: r, next_kind: vk.and_then(EdgeKind::from_u8) });
            match via {
                Some(v) if seen.insert(v) && path.len() < MAX_PATH => cur = v,
                Some(_) => {
                    truncated = true;
                    break;
                }
                None => break,
            }
        }
        out.push(AxiomEvidence { axiom: ax, path, path_truncated: truncated });
    }
    Ok(out)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModuleRow {
    pub id: i64,
    pub name: String,
    pub path: Option<String>,
    pub is_local: bool,
    pub decl_count: u64,
    pub status: String,
    pub status_detail: Option<String>,
    pub doc: Option<String>,
}

/// Counts recorded per file when the index was built. The outline sums them
/// over directories; nothing here is computed while browsing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModStats {
    pub files: u32,
    /// Files that were not read or parsed cleanly.
    pub partial: u32,
    pub lines: u32,
    pub thm: u32,
    /// `def`, `abbrev`, `opaque`.
    pub def: u32,
    /// `structure`, `class`, `inductive`.
    pub typ: u32,
    pub inst: u32,
    pub axiom: u32,
    /// Of the declarations counted above, those with a doc comment.
    pub doc: u32,
    /// Source-only: declarations with a `sorry`/`admit` token in their text.
    /// Semantic: declarations whose dependency closure reaches `sorryAx`.
    pub sorry: u32,
    /// Source-only: lines from the end of a theorem's statement to the end
    /// of its proof, summed. Not recorded for semantic indexes.
    pub proof_lines: u32,
    /// Newest modification time of a file (ns since the epoch; 0 unknown).
    pub mtime_ns: i64,
}

impl ModStats {
    pub fn add(&mut self, o: &ModStats) {
        self.files += o.files;
        self.partial += o.partial;
        self.lines = self.lines.saturating_add(o.lines);
        self.thm += o.thm;
        self.def += o.def;
        self.typ += o.typ;
        self.inst += o.inst;
        self.axiom += o.axiom;
        self.doc += o.doc;
        self.sorry += o.sorry;
        self.proof_lines = self.proof_lines.saturating_add(o.proof_lines);
        self.mtime_ns = self.mtime_ns.max(o.mtime_ns);
    }

    /// Theorems, definitions, types, instances and axioms: what `doc` is a
    /// share of (constructors, fields and generated declarations are not).
    pub fn named(&self) -> u32 {
        self.thm + self.def + self.typ + self.inst + self.axiom
    }
}

const STAT_COLS: &str = "status, coalesce(line_count, 0), n_thm, n_def, n_type, n_inst, n_axiom, n_doc, n_sorry, proof_lines, coalesce(mtime_ns, 0)";

fn stats_from(r: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<ModStats> {
    let n = |k: usize| -> rusqlite::Result<u32> { Ok(r.get::<_, i64>(at + k)?.clamp(0, u32::MAX as i64) as u32) };
    Ok(ModStats {
        files: 1,
        partial: (r.get::<_, String>(at)? != "ok") as u32,
        lines: n(1)?,
        thm: n(2)?,
        def: n(3)?,
        typ: n(4)?,
        inst: n(5)?,
        axiom: n(6)?,
        doc: n(7)?,
        sorry: n(8)?,
        proof_lines: n(9)?,
        mtime_ns: r.get(at + 10)?,
    })
}

/// The counts of one module.
pub fn module_stats(conn: &Connection, id: i64) -> Result<Option<ModStats>> {
    Ok(conn.query_row(&format!("SELECT {STAT_COLS} FROM modules WHERE id = ?1"), [id], |r| stats_from(r, 0)).optional()?)
}

/// Modules in name order, each with its recorded counts.
pub fn modules_with_stats(conn: &Connection, local_only: bool) -> Result<Vec<(ModuleRow, ModStats)>> {
    let mut st = conn.prepare(&format!(
        "SELECT id, name, path, is_local, decl_count, status, status_detail, {STAT_COLS} FROM modules {} ORDER BY name",
        if local_only { "WHERE is_local = 1" } else { "" }
    ))?;
    let v = st
        .query_map([], |r| {
            Ok((
                ModuleRow {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    path: r.get(2)?,
                    is_local: r.get::<_, i64>(3)? != 0,
                    decl_count: r.get::<_, i64>(4)? as u64,
                    status: r.get(5)?,
                    status_detail: r.get(6)?,
                    doc: None,
                },
                stats_from(r, 7)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(v)
}

pub fn modules(conn: &Connection, local_only: bool) -> Result<Vec<ModuleRow>> {
    let mut st = conn.prepare(&format!(
        "SELECT id, name, path, is_local, decl_count, status, status_detail, NULL FROM modules {} ORDER BY name",
        if local_only { "WHERE is_local = 1" } else { "" }
    ))?;
    let v = st
        .query_map([], |r| {
            Ok(ModuleRow {
                id: r.get(0)?,
                name: r.get(1)?,
                path: r.get(2)?,
                is_local: r.get::<_, i64>(3)? != 0,
                decl_count: r.get::<_, i64>(4)? as u64,
                status: r.get(5)?,
                status_detail: r.get(6)?,
                doc: r.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(v)
}

pub fn module(conn: &Connection, id: i64) -> Result<Option<ModuleRow>> {
    Ok(conn
        .query_row(
            "SELECT id, name, path, is_local, decl_count, status, status_detail, doc FROM modules WHERE id = ?1",
            [id],
            |r| {
                Ok(ModuleRow {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    path: r.get(2)?,
                    is_local: r.get::<_, i64>(3)? != 0,
                    decl_count: r.get::<_, i64>(4)? as u64,
                    status: r.get(5)?,
                    status_detail: r.get(6)?,
                    doc: r.get(7)?,
                })
            },
        )
        .optional()?)
}

#[derive(Clone, Debug, PartialEq)]
pub struct OutlineItem {
    pub row: DeclRow,
    pub line: Option<u32>,
}

/// Declarations of a module in source order.
pub fn module_decls(conn: &Connection, module_id: i64, show_generated: bool, limit: usize) -> Result<(Vec<OutlineItem>, bool)> {
    let gen = if show_generated { String::new() } else { format!(" AND (d.flags & {}) = 0", crate::model::flags::GENERATED) };
    let mut st = conn.prepare_cached(&format!(
        "SELECT {ROW_COLS}, d.start_line FROM decls d LEFT JOIN modules m ON m.id = d.module_id
          WHERE d.module_id = ?1 {gen} ORDER BY d.start_line IS NULL, d.start_line, d.id LIMIT ?2"
    ))?;
    let mut v: Vec<OutlineItem> = st
        .query_map(params![module_id, limit as i64 + 1], |r| Ok(OutlineItem { row: row_from(r)?, line: r.get(7)? }))?
        .collect::<rusqlite::Result<_>>()?;
    let trunc = v.len() > limit;
    v.truncate(limit);
    Ok((v, trunc))
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImportEdge {
    pub module: ModuleRow,
    pub flags: i64,
}

/// (imports, imported-by) of a module, each bounded.
pub fn module_import_edges(conn: &Connection, id: i64, limit: usize) -> Result<(Vec<ImportEdge>, u64, Vec<ImportEdge>, u64)> {
    let q = |sql: &str| -> Result<Vec<ImportEdge>> {
        let mut st = conn.prepare_cached(sql)?;
        let v = st
            .query_map(params![id, limit as i64], |r| {
                Ok(ImportEdge {
                    module: ModuleRow {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        path: r.get(2)?,
                        is_local: r.get::<_, i64>(3)? != 0,
                        decl_count: r.get::<_, i64>(4)? as u64,
                        status: r.get(5)?,
                        status_detail: None,
                        doc: None,
                    },
                    flags: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v)
    };
    let imports = q("SELECT m.id, m.name, m.path, m.is_local, m.decl_count, m.status, i.flags FROM module_imports i
                     JOIN modules m ON m.id = i.dst WHERE i.src = ?1 ORDER BY m.name LIMIT ?2")?;
    let importers = q("SELECT m.id, m.name, m.path, m.is_local, m.decl_count, m.status, i.flags FROM module_imports i
                       JOIN modules m ON m.id = i.src WHERE i.dst = ?1 ORDER BY m.name LIMIT ?2")?;
    let n_imp: i64 = conn.query_row("SELECT count(*) FROM module_imports WHERE src = ?1", [id], |r| r.get(0))?;
    let n_by: i64 = conn.query_row("SELECT count(*) FROM module_imports WHERE dst = ?1", [id], |r| r.get(0))?;
    Ok((imports, n_imp as u64, importers, n_by as u64))
}

/// Converts user words to an FTS5 query: each word becomes a quoted prefix
/// term; FTS syntax characters in the input are neutralised.
pub fn fts_query(words: &str) -> Option<String> {
    let terms: Vec<String> = words
        .split(|c: char| c.is_whitespace() || matches!(c, '"' | '*' | '(' | ')' | ':' | '^' | '-' | '+' | '.'))
        .filter(|t| !t.is_empty())
        .take(8)
        .map(|t| format!("\"{}\"*", t.replace('"', "")))
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" AND "))
    }
}

/// Documentation / statement text search (bm25-ranked), bounded.
pub fn text_search(conn: &Connection, words: &str, limit: usize) -> Result<Vec<i64>> {
    let Some(q) = fts_query(words) else { return Ok(Vec::new()) };
    let mut st = conn.prepare_cached("SELECT rowid FROM fts WHERE fts MATCH ?1 ORDER BY bm25(fts, 4.0, 2.0, 1.0) LIMIT ?2")?;
    let v = st.query_map(params![q, limit as i64], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(v)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct IndexInfo {
    pub mode: String,
    pub meta: Vec<(String, String)>,
    pub decls: u64,
    pub local: u64,
    pub modules: u64,
    pub local_modules: u64,
    pub edges: u64,
}

impl IndexInfo {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.meta.iter().find(|(a, _)| a == k).map(|(_, b)| b.as_str())
    }
}

pub fn index_info(conn: &Connection) -> Result<IndexInfo> {
    let mut st = conn.prepare("SELECT key, value FROM meta ORDER BY key")?;
    let meta: Vec<(String, String)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    let get = |k: &str| meta.iter().find(|(a, _)| a == k).map(|(_, b)| b.clone());
    let decls = get("decl_count").and_then(|s| s.parse().ok()).unwrap_or_else(|| {
        conn.query_row("SELECT count(*) FROM decls", [], |r| r.get::<_, i64>(0)).unwrap_or(0) as u64
    });
    // Counts recorded at index time avoid scanning millions of rows on open.
    let local: i64 = match get("local_decl_count").and_then(|s| s.parse().ok()) {
        Some(n) => n,
        None => conn.query_row("SELECT count(*) FROM decls WHERE origin IN (0, 3)", [], |r| r.get(0))?,
    };
    let modules: i64 = conn.query_row("SELECT count(*) FROM modules", [], |r| r.get(0))?;
    let local_modules: i64 = conn.query_row("SELECT count(*) FROM modules WHERE is_local = 1", [], |r| r.get(0))?;
    let edges = get("edge_count").and_then(|s| s.parse().ok()).unwrap_or(0);
    Ok(IndexInfo {
        mode: get("mode").unwrap_or_default(),
        meta,
        decls,
        local: local as u64,
        modules: modules as u64,
        local_modules: local_modules as u64,
        edges,
    })
}

/// Bounded breadth-first count of the transitive neighbourhood.
pub fn transitive_count(conn: &Connection, id: i64, dir: Direction, max_nodes: usize, max_ms: u64) -> Result<(u64, bool)> {
    let (self_col, other_col) = match dir {
        Direction::Uses => ("src", "dst"),
        Direction::UsedBy => ("dst", "src"),
    };
    let mut st = conn.prepare_cached(&format!("SELECT {other_col} FROM edges WHERE {self_col} = ?1"))?;
    let t0 = std::time::Instant::now();
    let mut seen = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    seen.insert(id);
    queue.push_back(id);
    while let Some(v) = queue.pop_front() {
        let next: Vec<i64> = st.query_map([v], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        for w in next {
            if seen.insert(w) {
                queue.push_back(w);
                if seen.len() > max_nodes {
                    return Ok((seen.len() as u64 - 1, true));
                }
            }
        }
        if t0.elapsed().as_millis() as u64 > max_ms {
            return Ok((seen.len() as u64 - 1, true));
        }
    }
    Ok((seen.len() as u64 - 1, false))
}
