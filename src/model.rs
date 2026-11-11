//! Core vocabulary shared by the index, importer and UI.
//!
//! Every enum here is persisted as a small integer in the index, so the
//! numeric values are part of the index schema (see `db::SCHEMA_VERSION`).

use std::fmt;

/// What kind of declaration a node is. Semantic kinds come from Lean's
/// `ConstantInfo` plus a few refinements (abbrev, instance, structure, class);
/// source-only candidates reuse the same kinds based on the keyword used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum DeclKind {
    Theorem = 0,
    Definition = 1,
    Abbrev = 2,
    Instance = 3,
    Structure = 4,
    Class = 5,
    Inductive = 6,
    Constructor = 7,
    Recursor = 8,
    Axiom = 9,
    Opaque = 10,
    Quot = 11,
    Unknown = 12,
}

impl DeclKind {
    pub const ALL: [DeclKind; 13] = [
        DeclKind::Theorem,
        DeclKind::Definition,
        DeclKind::Abbrev,
        DeclKind::Instance,
        DeclKind::Structure,
        DeclKind::Class,
        DeclKind::Inductive,
        DeclKind::Constructor,
        DeclKind::Recursor,
        DeclKind::Axiom,
        DeclKind::Opaque,
        DeclKind::Quot,
        DeclKind::Unknown,
    ];

    pub fn from_u8(v: u8) -> DeclKind {
        DeclKind::ALL.get(v as usize).copied().unwrap_or(DeclKind::Unknown)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            DeclKind::Theorem => "theorem",
            DeclKind::Definition => "def",
            DeclKind::Abbrev => "abbrev",
            DeclKind::Instance => "instance",
            DeclKind::Structure => "structure",
            DeclKind::Class => "class",
            DeclKind::Inductive => "inductive",
            DeclKind::Constructor => "constructor",
            DeclKind::Recursor => "recursor",
            DeclKind::Axiom => "axiom",
            DeclKind::Opaque => "opaque",
            DeclKind::Quot => "quot",
            DeclKind::Unknown => "unknown",
        }
    }

    /// Fixed-width (5 cell) tag used in dense lists.
    pub fn short(self) -> &'static str {
        match self {
            DeclKind::Theorem => "thm  ",
            DeclKind::Definition => "def  ",
            DeclKind::Abbrev => "abbr ",
            DeclKind::Instance => "inst ",
            DeclKind::Structure => "struc",
            DeclKind::Class => "class",
            DeclKind::Inductive => "induc",
            DeclKind::Constructor => "ctor ",
            DeclKind::Recursor => "rec  ",
            DeclKind::Axiom => "AXIOM",
            DeclKind::Opaque => "opaq ",
            DeclKind::Quot => "quot ",
            DeclKind::Unknown => "?    ",
        }
    }

    pub fn parse(s: &str) -> Option<DeclKind> {
        Some(match s {
            "theorem" | "lemma" => DeclKind::Theorem,
            "def" | "definition" => DeclKind::Definition,
            "abbrev" => DeclKind::Abbrev,
            "instance" => DeclKind::Instance,
            "structure" => DeclKind::Structure,
            "class" => DeclKind::Class,
            "inductive" => DeclKind::Inductive,
            "constructor" | "ctor" => DeclKind::Constructor,
            "recursor" | "rec" => DeclKind::Recursor,
            "axiom" => DeclKind::Axiom,
            "opaque" => DeclKind::Opaque,
            "quot" => DeclKind::Quot,
            "unknown" => DeclKind::Unknown,
            _ => return None,
        })
    }

    /// Whether this kind of constant carries a value (definition body or
    /// proof term) in Lean's kernel representation.
    pub fn has_value(self) -> bool {
        matches!(
            self,
            DeclKind::Theorem
                | DeclKind::Definition
                | DeclKind::Abbrev
                | DeclKind::Instance
                | DeclKind::Opaque
        )
    }

    /// Coarse groups used by the kind filter.
    pub fn group(self) -> KindGroup {
        match self {
            DeclKind::Theorem => KindGroup::Theorems,
            DeclKind::Definition | DeclKind::Abbrev | DeclKind::Opaque | DeclKind::Instance => {
                KindGroup::Definitions
            }
            DeclKind::Structure
            | DeclKind::Class
            | DeclKind::Inductive
            | DeclKind::Constructor
            | DeclKind::Recursor
            | DeclKind::Quot => KindGroup::Types,
            DeclKind::Axiom => KindGroup::Axioms,
            DeclKind::Unknown => KindGroup::Other,
        }
    }
}

impl fmt::Display for DeclKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KindGroup {
    Theorems,
    Definitions,
    Types,
    Axioms,
    Other,
}

/// Kind filter cycled by the user. `All` shows everything.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KindFilter {
    All,
    Theorems,
    Definitions,
    Types,
    Axioms,
}

impl KindFilter {
    pub fn next(self) -> KindFilter {
        match self {
            KindFilter::All => KindFilter::Theorems,
            KindFilter::Theorems => KindFilter::Definitions,
            KindFilter::Definitions => KindFilter::Types,
            KindFilter::Types => KindFilter::Axioms,
            KindFilter::Axioms => KindFilter::All,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            KindFilter::All => "all kinds",
            KindFilter::Theorems => "theorems",
            KindFilter::Definitions => "definitions",
            KindFilter::Types => "types",
            KindFilter::Axioms => "axioms",
        }
    }
    pub fn accepts(self, k: DeclKind) -> bool {
        match self {
            KindFilter::All => true,
            KindFilter::Theorems => k.group() == KindGroup::Theorems,
            KindFilter::Definitions => k.group() == KindGroup::Definitions,
            KindFilter::Types => k.group() == KindGroup::Types,
            KindFilter::Axioms => k.group() == KindGroup::Axioms,
        }
    }
}

/// Typed declaration-to-declaration edges. Module imports are kept in a
/// separate table; lexical mentions are never stored as edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum EdgeKind {
    /// The target constant occurs in the elaborated *type* (statement).
    Type = 1,
    /// The target constant occurs in the elaborated *value*: the proof term
    /// of a theorem or the body of a definition.
    Value = 2,
    /// Structural: the target is a constructor of the source inductive type.
    /// Lean's own axiom collector follows this relation.
    Ctor = 3,
}

impl EdgeKind {
    pub const ALL: [EdgeKind; 3] = [EdgeKind::Type, EdgeKind::Value, EdgeKind::Ctor];

    pub fn from_u8(v: u8) -> Option<EdgeKind> {
        match v {
            1 => Some(EdgeKind::Type),
            2 => Some(EdgeKind::Value),
            3 => Some(EdgeKind::Ctor),
            _ => None,
        }
    }
    pub fn tag(self) -> &'static str {
        match self {
            EdgeKind::Type => "T",
            EdgeKind::Value => "V",
            EdgeKind::Ctor => "C",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            EdgeKind::Type => "in statement (elaborated type)",
            EdgeKind::Value => "in proof term / definition body",
            EdgeKind::Ctor => "constructor of inductive type",
        }
    }
    pub fn reverse_label(self) -> &'static str {
        match self {
            EdgeKind::Type => "mention it in their statement",
            EdgeKind::Value => "use it in a proof term / body",
            EdgeKind::Ctor => "inductive type it constructs",
        }
    }
}

/// Coverage of one class of references for one declaration. Absence of
/// edges only means "no dependencies" when coverage is `Complete`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Coverage {
    Complete = 0,
    /// The term exists but was not available to the exporter.
    Unavailable = 1,
    /// Some references were recorded but the list is known to be incomplete.
    Partial = 2,
    /// This kind of constant has no such term (e.g. the value of an axiom).
    NotApplicable = 3,
    /// Not exported (library declaration outside the extraction scope, or an
    /// unresolved name referenced by the snapshot).
    NotExtracted = 4,
    /// Source-only candidate: no semantic information at all.
    SourceOnly = 5,
}

impl Coverage {
    pub fn from_u8(v: u8) -> Coverage {
        match v {
            0 => Coverage::Complete,
            1 => Coverage::Unavailable,
            2 => Coverage::Partial,
            3 => Coverage::NotApplicable,
            4 => Coverage::NotExtracted,
            _ => Coverage::SourceOnly,
        }
    }
    pub fn parse(s: &str) -> Option<Coverage> {
        Some(match s {
            "complete" => Coverage::Complete,
            "unavailable" => Coverage::Unavailable,
            "partial" => Coverage::Partial,
            "not_applicable" | "none" => Coverage::NotApplicable,
            "not_extracted" => Coverage::NotExtracted,
            "source_only" => Coverage::SourceOnly,
            _ => return None,
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Coverage::Complete => "complete",
            Coverage::Unavailable => "unavailable",
            Coverage::Partial => "partial",
            Coverage::NotApplicable => "not applicable",
            Coverage::NotExtracted => "not extracted",
            Coverage::SourceOnly => "source only",
        }
    }
    /// True when a missing edge might still exist in reality.
    pub fn is_incomplete(self) -> bool {
        matches!(
            self,
            Coverage::Unavailable | Coverage::Partial | Coverage::NotExtracted | Coverage::SourceOnly
        )
    }
}

/// Where a declaration node came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Origin {
    /// Declared in a module of the indexed project (semantic export).
    Local = 0,
    /// Declared in an imported library module (semantic export).
    Library = 1,
    /// Referenced by name in the snapshot but never described by it.
    Unresolved = 2,
    /// Conservative candidate found by the source-only scanner.
    SourceCandidate = 3,
}

impl Origin {
    pub fn from_u8(v: u8) -> Origin {
        match v {
            0 => Origin::Local,
            1 => Origin::Library,
            2 => Origin::Unresolved,
            _ => Origin::SourceCandidate,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Origin::Local => "local",
            Origin::Library => "library",
            Origin::Unresolved => "unresolved",
            Origin::SourceCandidate => "source candidate",
        }
    }
    pub fn is_local(self) -> bool {
        matches!(self, Origin::Local | Origin::SourceCandidate)
    }
}

/// Bit flags stored per declaration.
pub mod flags {
    pub const PRIVATE: u32 = 1 << 0;
    pub const PROTECTED: u32 = 1 << 1;
    /// Generated by Lean (auxiliary recursors, matchers, equation lemmas,
    /// `_private`/`_proof` names, ...). Hidden by default.
    pub const GENERATED: u32 = 1 << 2;
    pub const INSTANCE: u32 = 1 << 3;
    pub const UNSAFE: u32 = 1 << 4;
    pub const PARTIAL: u32 = 1 << 5;
    pub const NONCOMPUTABLE: u32 = 1 << 6;
    /// Source-only: the candidate name may not be the elaborated name
    /// (duplicate in index, macro context, unbalanced scopes...).
    pub const AMBIGUOUS: u32 = 1 << 7;
    /// Source-only: anonymous declaration (e.g. `instance : Foo`), the
    /// elaborated name is generated by Lean and unknown here.
    pub const ANONYMOUS: u32 = 1 << 8;
    /// Source-only lexical observation: a `sorry`/`admit` token occurs in
    /// the declaration text. Its absence proves nothing.
    pub const LEX_SORRY: u32 = 1 << 9;
    /// Source-only lexical observation: `native_decide` occurs in the text.
    pub const LEX_NATIVE: u32 = 1 << 10;
    pub const IN_MUTUAL: u32 = 1 << 11;
    pub const HAS_DOC: u32 = 1 << 12;
    pub const PROJECTION: u32 = 1 << 13;
    pub const CLASS: u32 = 1 << 14;
    pub const STRUCTURE: u32 = 1 << 15;
    /// Source-only: declared inside `mutual ... end` or a macro-heavy region
    /// where extents are approximate.
    pub const APPROX_EXTENT: u32 = 1 << 16;
}

/// Category of an axiom-like dependency shown in the evidence panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum AxiomCategory {
    /// Not an axiom: some declaration in the dependency closure has no
    /// available value/type data, so the axiom list is a lower bound.
    Incomplete = 0,
    /// `sorryAx`: a placeholder proof (explicit `sorry` or an elaboration error).
    Sorry = 1,
    /// A user/library axiom other than the standard ones.
    Custom = 2,
    /// Trust in the compiler (`Lean.ofReduceBool`, `Lean.trustCompiler`, ...),
    /// introduced by `native_decide` and similar.
    CompilerTrust = 3,
    /// `propext`, `Classical.choice`, `Quot.sound`.
    Standard = 4,
}

impl AxiomCategory {
    pub fn of_name(name: &str) -> AxiomCategory {
        match name {
            "propext" | "Classical.choice" | "Quot.sound" => AxiomCategory::Standard,
            "sorryAx" => AxiomCategory::Sorry,
            "Lean.ofReduceBool" | "Lean.ofReduceNat" | "Lean.trustCompiler" => {
                AxiomCategory::CompilerTrust
            }
            // Since Lean 4.33, `native_decide` adds one auxiliary axiom per use,
            // named `<decl>._native.native_decide.ax_<n>_<m>`.
            n if n.contains("._native.native_decide.") => AxiomCategory::CompilerTrust,
            _ => AxiomCategory::Custom,
        }
    }
    pub fn from_u8(v: u8) -> AxiomCategory {
        match v {
            0 => AxiomCategory::Incomplete,
            1 => AxiomCategory::Sorry,
            2 => AxiomCategory::Custom,
            3 => AxiomCategory::CompilerTrust,
            _ => AxiomCategory::Standard,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            AxiomCategory::Incomplete => "coverage gap",
            AxiomCategory::Sorry => "placeholder (sorryAx)",
            AxiomCategory::Custom => "custom axiom",
            AxiomCategory::CompilerTrust => "trusts compiler evaluation",
            AxiomCategory::Standard => "standard axiom",
        }
    }
    /// Conspicuous categories are rendered with a warning marker.
    pub fn conspicuous(self) -> bool {
        !matches!(self, AxiomCategory::Standard)
    }
    pub fn explanation(self, name: &str) -> String {
        match self {
            AxiomCategory::Standard => match name {
                "propext" => "propositional extensionality; one of Lean's three standard axioms".into(),
                "Classical.choice" => "the axiom of choice; one of Lean's three standard axioms".into(),
                "Quot.sound" => "soundness of quotients (implies function extensionality); standard".into(),
                _ => "standard axiom".into(),
            },
            AxiomCategory::Sorry => {
                "sorryAx is how Lean represents `sorry` and elaboration errors: the proof is a placeholder".into()
            }
            AxiomCategory::CompilerTrust => {
                "trusts the compiler/evaluator (e.g. `native_decide`); not checked by the kernel alone".into()
            }
            AxiomCategory::Custom => {
                "an axiom declared by a library or the project; it is assumed, not proved".into()
            }
            AxiomCategory::Incomplete => {
                "a declaration on this path has unavailable term data; more axioms may be reachable".into()
            }
        }
    }
}

/// Splits a Lean name at top-level dots, respecting `«...»` quoting.
pub fn name_components(name: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_quote = false;
    for (i, c) in name.char_indices() {
        match c {
            '«' => in_quote = true,
            '»' => in_quote = false,
            '.' if !in_quote => {
                out.push(&name[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&name[start..]);
    out
}

/// Byte offset where the last component of a dotted name starts.
pub fn last_component_start(name: &str) -> usize {
    let mut last = 0;
    let mut in_quote = false;
    for (i, c) in name.char_indices() {
        match c {
            '«' => in_quote = true,
            '»' => in_quote = false,
            '.' if !in_quote => last = i + 1,
            _ => {}
        }
    }
    if last > name.len() {
        0
    } else {
        last
    }
}

/// Namespace part of a declaration name (everything before the last component).
pub fn namespace_of(name: &str) -> &str {
    let start = last_component_start(name);
    if start == 0 {
        ""
    } else {
        &name[..start - 1]
    }
}

/// Strips Lean's private-name mangling `_private.<Module>.<n>.<name>`.
pub fn display_name(name: &str) -> String {
    if let Some(rest) = name.strip_prefix("_private.") {
        // `_private.A.B.0.X.y`: find the first purely numeric component.
        let comps = name_components(rest);
        if let Some(pos) = comps.iter().position(|c| !c.is_empty() && c.bytes().all(|b| b.is_ascii_digit())) {
            let tail: Vec<&str> = comps[pos + 1..].to_vec();
            if !tail.is_empty() {
                return tail.join(".");
            }
        }
    }
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_respect_quotes() {
        assert_eq!(name_components("Demo.«weird.name».x"), vec!["Demo", "«weird.name»", "x"]);
        assert_eq!(last_component_start("Demo.Rel.trans"), 9);
        assert_eq!(namespace_of("Demo.Rel.trans"), "Demo.Rel");
        assert_eq!(namespace_of("trans"), "");
        assert_eq!(namespace_of("A.«b.c»"), "A");
    }

    #[test]
    fn private_names_display() {
        assert_eq!(
            display_name("_private.SlimDemo.Fixtures.Special.0.Demo.Fixtures.helper"),
            "Demo.Fixtures.helper"
        );
        assert_eq!(display_name("Demo.x"), "Demo.x");
    }

    #[test]
    fn kinds_round_trip() {
        for k in DeclKind::ALL {
            assert_eq!(DeclKind::from_u8(k as u8), k);
            assert_eq!(DeclKind::parse(k.as_str()), Some(k));
        }
    }

    #[test]
    fn axiom_categories() {
        assert_eq!(AxiomCategory::of_name("propext"), AxiomCategory::Standard);
        assert_eq!(AxiomCategory::of_name("sorryAx"), AxiomCategory::Sorry);
        assert_eq!(AxiomCategory::of_name("Lean.ofReduceBool"), AxiomCategory::CompilerTrust);
        assert_eq!(AxiomCategory::of_name("Demo.oracle"), AxiomCategory::Custom);
        assert_eq!(
            AxiomCategory::of_name("Demo.Fixtures.sample_search_native._native.native_decide.ax_1_1"),
            AxiomCategory::CompilerTrust
        );
        assert!(AxiomCategory::Sorry.conspicuous());
        assert!(!AxiomCategory::Standard.conspicuous());
    }
}
