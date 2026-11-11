//! The versioned semantic snapshot format (`slim-snapshot`, format 1).
//!
//! A snapshot is UTF-8 JSON Lines: one header record, then module and
//! declaration records in any order, then one footer record. Producers are
//! the Lean exporter (wrapped by `slim export`, which adds provenance and
//! source fingerprints) and the synthetic benchmark generator (which marks
//! its header `"synthetic": true`). Snapshots are untrusted input: every
//! field is validated and bounded on import, nothing in them is executed,
//! and paths are only ever resolved inside an approved source root.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const FORMAT: &str = "slim-snapshot";
pub const FORMAT_VERSION: u32 = 1;

/// Hard limits applied while reading a snapshot.
pub mod limits {
    pub const MAX_LINE_BYTES: usize = 64 << 20;
    pub const MAX_NAME_BYTES: usize = 4096;
    pub const MAX_TEXT_BYTES: usize = 1 << 20;
    pub const MAX_REFS_PER_CLASS: usize = 1_000_000;
    pub const MAX_BINDERS: usize = 10_000;
    pub const MAX_DECLS: usize = 50_000_000;
    pub const MAX_MODULES: usize = 5_000_000;
    pub const MAX_PATH_BYTES: usize = 4096;
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Producer {
    pub name: String,
    pub version: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct LeanInfo {
    pub version: String,
    #[serde(default)]
    pub githash: String,
    #[serde(default)]
    pub toolchain: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ProjectInfo {
    pub name: String,
    #[serde(default)]
    pub root_hint: Option<String>,
    #[serde(default)]
    pub lean_toolchain: Option<String>,
    #[serde(default)]
    pub lake_manifest_sha256: Option<String>,
    #[serde(default)]
    pub revision: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Header {
    pub record: String,
    pub format: String,
    pub format_version: u32,
    pub producer: Producer,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exporter: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lean: Option<LeanInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<Value>,
    #[serde(default)]
    pub synthetic: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ImportRec {
    pub module: String,
    #[serde(default)]
    pub flags: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Binder {
    pub name: String,
    /// `default`, `implicit`, `strict_implicit` or `inst_implicit`.
    pub info: String,
    #[serde(rename = "type")]
    pub type_: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Range {
    /// `[line, column]`, line 1-based, column in Unicode code points.
    pub start: [u32; 2],
    pub end: [u32; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_utf16: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_utf16: Option<u32>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Refs {
    #[serde(default, rename = "type")]
    pub type_: Vec<String>,
    #[serde(default)]
    pub value: Vec<String>,
    #[serde(default)]
    pub ctor: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CoverageRec {
    #[serde(rename = "type")]
    pub type_: String,
    pub value: String,
}

impl Default for CoverageRec {
    fn default() -> Self {
        CoverageRec { type_: "complete".into(), value: "complete".into() }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ModuleRec {
    pub name: String,
    #[serde(default)]
    pub local: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtime_ns: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub olean_mtime_ns: Option<i64>,
    #[serde(default)]
    pub imports: Vec<ImportRec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct DeclRec {
    pub name: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    #[serde(default)]
    pub local: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated: Option<String>,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binders: Option<Vec<Binder>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conclusion: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<Range>,
    #[serde(default)]
    pub refs: Refs,
    #[serde(default)]
    pub coverage: CoverageRec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lean_axioms: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Counts {
    pub modules: u64,
    pub decls: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Footer {
    pub counts: Counts,
    #[serde(default)]
    pub complete: bool,
}

/// One parsed line. A flat struct (rather than a tagged enum) keeps parsing
/// streaming and cheap; `record` selects which fields are meaningful.
#[derive(Deserialize, Default, Debug)]
#[serde(default)]
pub struct Line {
    pub record: String,
    // header
    pub format: Option<String>,
    pub format_version: Option<u32>,
    pub producer: Option<Producer>,
    pub exporter: Option<Value>,
    pub lean: Option<LeanInfo>,
    pub project: Option<ProjectInfo>,
    pub scope: Option<Value>,
    pub created_at: Option<String>,
    pub build: Option<Value>,
    pub isolation: Option<Value>,
    pub synthetic: Option<bool>,
    // module / decl
    pub name: Option<String>,
    pub local: Option<bool>,
    pub path: Option<String>,
    pub sha256: Option<String>,
    pub size: Option<u64>,
    pub mtime_ns: Option<i64>,
    pub olean_mtime_ns: Option<i64>,
    pub imports: Option<Vec<ImportRec>>,
    pub doc: Option<String>,
    pub kind: Option<String>,
    pub module: Option<String>,
    pub flags: Option<Vec<String>>,
    pub generated: Option<String>,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub binders: Option<Vec<Binder>>,
    pub conclusion: Option<String>,
    pub range: Option<Range>,
    pub selection: Option<Range>,
    pub refs: Option<Refs>,
    pub coverage: Option<CoverageRec>,
    pub lean_axioms: Option<Vec<String>>,
    // footer
    pub counts: Option<Counts>,
    pub complete: Option<bool>,
}

impl Line {
    pub fn into_header(self) -> Result<Header> {
        let format = self.format.unwrap_or_default();
        if format != FORMAT {
            bail!("not a slim snapshot (format {format:?})");
        }
        let fv = self.format_version.unwrap_or(0);
        if fv != FORMAT_VERSION {
            bail!("unsupported snapshot format version {fv} (supported: {FORMAT_VERSION})");
        }
        Ok(Header {
            record: "header".into(),
            format,
            format_version: fv,
            producer: self.producer.unwrap_or_default(),
            exporter: self.exporter,
            lean: self.lean,
            project: self.project,
            scope: self.scope,
            created_at: self.created_at,
            build: self.build,
            isolation: self.isolation,
            synthetic: self.synthetic.unwrap_or(false),
        })
    }

    pub fn into_module(self) -> Result<ModuleRec> {
        let name = self.name.unwrap_or_default();
        check_name(&name)?;
        if let Some(p) = &self.path {
            check_rel_path(p)?;
        }
        if let Some(d) = &self.doc {
            check_text(d, "module doc")?;
        }
        if let Some(h) = &self.sha256 {
            if h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
                bail!("module {name}: malformed sha256");
            }
        }
        let imports = self.imports.unwrap_or_default();
        for i in &imports {
            check_name(&i.module)?;
        }
        Ok(ModuleRec {
            name,
            local: self.local.unwrap_or(false),
            path: self.path,
            sha256: self.sha256,
            size: self.size,
            mtime_ns: self.mtime_ns,
            olean_mtime_ns: self.olean_mtime_ns,
            imports,
            doc: self.doc,
        })
    }

    pub fn into_decl(self) -> Result<DeclRec> {
        let name = self.name.unwrap_or_default();
        check_name(&name)?;
        let kind = self.kind.unwrap_or_else(|| "unknown".into());
        if kind.len() > 32 {
            bail!("{name}: malformed kind");
        }
        if let Some(m) = &self.module {
            check_name(m)?;
        }
        for t in [&self.type_, &self.doc, &self.conclusion, &self.generated].into_iter().flatten() {
            check_text(t, &name)?;
        }
        let refs = self.refs.unwrap_or_default();
        for list in [&refs.type_, &refs.value, &refs.ctor] {
            if list.len() > limits::MAX_REFS_PER_CLASS {
                bail!("{name}: too many references");
            }
            for r in list {
                check_name(r)?;
            }
        }
        if let Some(b) = &self.binders {
            if b.len() > limits::MAX_BINDERS {
                bail!("{name}: too many binders");
            }
            for x in b {
                check_text(&x.type_, &name)?;
                if x.name.len() > limits::MAX_NAME_BYTES {
                    bail!("{name}: binder name too long");
                }
            }
        }
        if let Some(fl) = &self.flags {
            if fl.len() > 64 || fl.iter().any(|f| f.len() > 64) {
                bail!("{name}: malformed flags");
            }
        }
        if let Some(ax) = &self.lean_axioms {
            for a in ax {
                check_name(a)?;
            }
        }
        Ok(DeclRec {
            name,
            kind,
            module: self.module,
            local: self.local.unwrap_or(false),
            flags: self.flags.unwrap_or_default(),
            generated: self.generated,
            type_: self.type_,
            binders: self.binders,
            conclusion: self.conclusion,
            doc: self.doc,
            range: self.range,
            selection: self.selection,
            refs,
            coverage: self.coverage.unwrap_or_default(),
            lean_axioms: self.lean_axioms,
        })
    }
}

pub fn check_name(n: &str) -> Result<()> {
    if n.is_empty() {
        bail!("empty name");
    }
    if n.len() > limits::MAX_NAME_BYTES {
        bail!("name too long ({} bytes)", n.len());
    }
    if n.contains('\0') || n.contains('\n') {
        bail!("name contains NUL or newline");
    }
    Ok(())
}

pub fn check_text(t: &str, ctx: &str) -> Result<()> {
    if t.len() > limits::MAX_TEXT_BYTES {
        bail!("{ctx}: text field too long ({} bytes)", t.len());
    }
    if t.contains('\0') {
        bail!("{ctx}: text contains NUL");
    }
    Ok(())
}

/// Accepts only relative paths made of normal components (no `..`, no
/// absolute or drive prefixes, no empty or `.` components, no NUL/backslash).
pub fn check_rel_path(p: &str) -> Result<()> {
    if p.is_empty() || p.len() > limits::MAX_PATH_BYTES {
        bail!("invalid source path length");
    }
    if p.starts_with('/') || p.contains('\\') || p.contains('\0') || p.contains(':') {
        bail!("rejected source path {p:?}: must be relative with forward slashes");
    }
    for comp in p.split('/') {
        if comp.is_empty() || comp == "." || comp == ".." {
            bail!("rejected source path {p:?}: traversal or empty component");
        }
    }
    Ok(())
}

pub fn header_line(h: &Header) -> String {
    serde_json::to_string(h).expect("header serializes")
}

#[derive(Serialize)]
struct Tagged<'a, T: Serialize> {
    record: &'a str,
    #[serde(flatten)]
    inner: &'a T,
}

pub fn module_line(m: &ModuleRec) -> String {
    serde_json::to_string(&Tagged { record: "module", inner: m }).expect("module serializes")
}

pub fn decl_line(d: &DeclRec) -> String {
    serde_json::to_string(&Tagged { record: "decl", inner: d }).expect("decl serializes")
}

pub fn footer_line(f: &Footer) -> String {
    serde_json::to_string(&Tagged { record: "footer", inner: f }).expect("footer serializes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_validation() {
        assert!(check_rel_path("SlimDemo/Relation/Basic.lean").is_ok());
        for bad in ["/etc/passwd", "../x.lean", "a/../../b", "a//b", "./a", "a\\b", "C:/x", ""] {
            assert!(check_rel_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn round_trip_decl_line() {
        let d = DeclRec {
            name: "Demo.x".into(),
            kind: "theorem".into(),
            module: Some("Demo".into()),
            local: true,
            refs: Refs { type_: vec!["Nat".into()], value: vec!["Nat.rec".into()], ctor: vec![] },
            ..Default::default()
        };
        let line = decl_line(&d);
        let l: Line = serde_json::from_str(&line).unwrap();
        assert_eq!(l.record, "decl");
        assert_eq!(l.into_decl().unwrap(), d);
    }

    #[test]
    fn header_rejects_wrong_format() {
        let l: Line = serde_json::from_str(r#"{"record":"header","format":"other","format_version":1}"#).unwrap();
        assert!(l.into_header().is_err());
        let l: Line = serde_json::from_str(r#"{"record":"header","format":"slim-snapshot","format_version":2}"#).unwrap();
        assert!(l.into_header().is_err());
    }
}
