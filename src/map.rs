//! The project map sidecar (`lean-view.toml`): a small, versioned, hand-editable
//! description of a formalization's architecture.
//!
//! The map is *shareable annotation*, kept apart from the regenerable index
//! and from personal reading data. slim only ever reads it (the one
//! exception is `slim map init`, which refuses to overwrite a file), so a
//! malformed sidecar can never be clobbered by defaults and hand edits always
//! round-trip. Declarations are referenced by fully qualified Lean name;
//! files and line numbers are locators, never identities.
//!
//! Everything a map says is curation, not verification: a group is not a
//! proved chapter, an objective is whatever its author chose, and a curated
//! relation "A uses B" is an explanation someone wrote, not a Lean edge.

use crate::db::query::DeclRow;
use crate::model::{DeclKind, Origin};
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};
use serde::Deserialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const FILE_NAME: &str = "lean-view.toml";
pub const SCHEMA_VERSION: u32 = 1;
/// A sidecar larger than this is refused rather than parsed.
pub const MAX_BYTES: u64 = 4 << 20;

/// Who is responsible for an annotation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Provenance {
    /// Written by a person (the default).
    #[default]
    Authored,
    /// Derived mechanically from files and namespaces; not a mathematical claim.
    Generated,
    /// Proposed by an agent and not accepted by anyone.
    AiSuggestion,
    /// Proposed by an agent and accepted by a person.
    AiAccepted,
}

impl Provenance {
    pub fn label(self) -> &'static str {
        match self {
            Provenance::Authored => "authored",
            Provenance::Generated => "generated organization (not reviewed)",
            Provenance::AiSuggestion => "AI suggestion (not accepted)",
            Provenance::AiAccepted => "accepted AI suggestion",
        }
    }
    pub fn tag(self) -> &'static str {
        match self {
            Provenance::Authored => "authored",
            Provenance::Generated => "generated",
            Provenance::AiSuggestion => "suggestion",
            Provenance::AiAccepted => "ai-accepted",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// A problem in the sidecar, tied to the field that caused it.
#[derive(Clone, Debug, PartialEq)]
pub struct MapIssue {
    pub severity: Severity,
    pub field: String,
    pub message: String,
}

impl MapIssue {
    fn err(field: impl Into<String>, message: impl Into<String>) -> MapIssue {
        MapIssue { severity: Severity::Error, field: field.into(), message: message.into() }
    }
    fn warn(field: impl Into<String>, message: impl Into<String>) -> MapIssue {
        MapIssue { severity: Severity::Warning, field: field.into(), message: message.into() }
    }
    pub fn line(&self) -> String {
        let s = if self.severity == Severity::Error { "error" } else { "warning" };
        if self.field.is_empty() {
            format!("{s}: {}", self.message)
        } else {
            format!("{s}: {}: {}", self.field, self.message)
        }
    }
}

// ---------------------------------------------------------------------------
// Raw (file) schema
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawMap {
    schema: Option<u32>,
    #[serde(default)]
    project: RawProject,
    #[serde(default)]
    source: Vec<RawSource>,
    #[serde(default)]
    objective: Vec<RawObjective>,
    #[serde(default)]
    group: Vec<RawGroup>,
    #[serde(default)]
    landmark: Vec<RawLandmark>,
    #[serde(default)]
    relation: Vec<RawRelation>,
    #[serde(default)]
    argument: Vec<RawArgument>,
    #[serde(default)]
    trail: Vec<RawTrail>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawProject {
    name: Option<String>,
    title: Option<String>,
    description: Option<String>,
    revision: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSource {
    id: String,
    citation: String,
    url: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawPaper {
    Text(String),
    Full {
        text: String,
        #[serde(default)]
        source: Option<String>,
        #[serde(default)]
        note: Option<String>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawObjective {
    decl: String,
    title: Option<String>,
    purpose: Option<String>,
    explanation: Option<String>,
    #[serde(default)]
    paper: Vec<RawPaper>,
    #[serde(default)]
    limitations: Vec<String>,
    #[serde(default)]
    origin: Provenance,
    source_sha256: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawPlacement {
    Name(String),
    Full {
        decl: String,
        #[serde(default)]
        role: Option<String>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGroup {
    id: String,
    title: Option<String>,
    parent: Option<String>,
    #[serde(default)]
    order: i64,
    kind: Option<String>,
    purpose: Option<String>,
    explanation: Option<String>,
    #[serde(default)]
    origin: Provenance,
    reviewed: Option<bool>,
    #[serde(default)]
    paper: Vec<RawPaper>,
    #[serde(default)]
    limitations: Vec<String>,
    #[serde(default)]
    landmarks: Vec<RawPlacement>,
    #[serde(default)]
    modules: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLandmark {
    decl: String,
    purpose: Option<String>,
    explanation: Option<String>,
    #[serde(default)]
    paper: Vec<RawPaper>,
    #[serde(default)]
    limitations: Vec<String>,
    #[serde(default)]
    origin: Provenance,
    #[serde(default)]
    groups: Vec<String>,
    source_sha256: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRelation {
    from: String,
    to: String,
    kind: Option<String>,
    note: Option<String>,
    #[serde(default)]
    origin: Provenance,
    evidence: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStep {
    decl: String,
    why: Option<String>,
    /// Hang this step under another declaration of the argument instead of
    /// directly under the goal.
    #[serde(rename = "for")]
    parent: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawArgument {
    goal: String,
    title: Option<String>,
    summary: Option<String>,
    #[serde(default)]
    origin: Provenance,
    #[serde(default)]
    steps: Vec<RawStep>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStop {
    decl: Option<String>,
    group: Option<String>,
    note: Option<String>,
    question: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTrail {
    id: String,
    title: Option<String>,
    description: Option<String>,
    #[serde(default)]
    stops: Vec<RawStop>,
}

// ---------------------------------------------------------------------------
// Validated model
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub id: String,
    pub citation: String,
    pub url: Option<String>,
}

/// A reference into a paper. `text` is kept literally; `source` is set only
/// when the author named an identified source.
#[derive(Clone, Debug, PartialEq)]
pub struct PaperRef {
    pub text: String,
    pub source: Option<String>,
    pub note: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupKind {
    Component,
    /// External library foundations, shown as their own section.
    Foundation,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub decl: String,
    pub role: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Group {
    pub id: String,
    pub title: String,
    pub kind: GroupKind,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub order: i64,
    pub purpose: Option<String>,
    pub explanation: Option<String>,
    pub origin: Provenance,
    pub reviewed: bool,
    pub paper: Vec<PaperRef>,
    pub limitations: Vec<String>,
    pub landmarks: Vec<Placement>,
    pub modules: Vec<String>,
}

/// Annotation of one declaration. A declaration has one annotation however
/// many groups it is placed in.
#[derive(Clone, Debug, Default)]
pub struct Landmark {
    pub decl: String,
    pub purpose: Option<String>,
    pub explanation: Option<String>,
    pub paper: Vec<PaperRef>,
    pub limitations: Vec<String>,
    pub origin: Provenance,
    /// Fingerprint (SHA-256 prefix) of the declaration's source file when the
    /// annotation was written; a mismatch marks the annotation stale.
    pub source_sha256: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Objective {
    pub decl: String,
    pub title: Option<String>,
}

/// A curated relation: `from` uses `to` (navigation runs from a statement to
/// its prerequisite).
#[derive(Clone, Debug, PartialEq)]
pub struct Relation {
    pub from: String,
    pub to: String,
    pub kind: String,
    pub note: Option<String>,
    pub origin: Provenance,
    pub evidence: Option<String>,
    /// Position in a curated argument (1-based), if it came from one.
    pub step: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct Argument {
    pub goal: String,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub origin: Provenance,
    pub steps: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StopRef {
    Decl(String),
    Group(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SharedStop {
    pub target: StopRef,
    pub note: Option<String>,
    pub question: Option<String>,
}

/// A reading trail shipped with the map. It is a template: a reader starts
/// it, which copies it into their personal data.
#[derive(Clone, Debug)]
pub struct SharedTrail {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub stops: Vec<SharedStop>,
}

#[derive(Clone, Debug, Default)]
pub struct ProjectMap {
    pub path: PathBuf,
    pub sha256: String,
    pub project_name: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub revision: Option<String>,
    pub sources: Vec<Source>,
    pub objectives: Vec<Objective>,
    pub groups: Vec<Group>,
    /// Top-level groups in display order.
    pub roots: Vec<usize>,
    pub landmarks: Vec<Landmark>,
    pub landmark_index: HashMap<String, usize>,
    pub relations: Vec<Relation>,
    pub arguments: Vec<Argument>,
    pub trails: Vec<SharedTrail>,
    pub issues: Vec<MapIssue>,
}

#[derive(Clone, Debug)]
pub enum MapLoad {
    /// No sidecar: the browser and a generated outline still work.
    Absent { looked_at: Option<PathBuf> },
    Loaded(Arc<ProjectMap>),
    /// The file exists but cannot be used. It is left untouched.
    Malformed { path: PathBuf, issues: Vec<MapIssue> },
}

impl MapLoad {
    pub fn map(&self) -> Option<&Arc<ProjectMap>> {
        match self {
            MapLoad::Loaded(m) => Some(m),
            _ => None,
        }
    }
}

fn clean_opt(s: Option<String>) -> Option<String> {
    s.map(|x| x.trim().to_string()).filter(|x| !x.is_empty())
}

fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 80 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

fn valid_name(s: &str) -> bool {
    !s.trim().is_empty() && s.len() <= 1000 && !s.chars().any(|c| c.is_control())
}

fn papers(raw: Vec<RawPaper>, field: &str, sources: &[Source], issues: &mut Vec<MapIssue>) -> Vec<PaperRef> {
    let mut out = Vec::new();
    for (i, p) in raw.into_iter().enumerate() {
        let (text, source, note) = match p {
            RawPaper::Text(t) => (t, None, None),
            RawPaper::Full { text, source, note } => (text, source, note),
        };
        if text.trim().is_empty() {
            issues.push(MapIssue::err(format!("{field}.paper[{i}]"), "empty reference text"));
            continue;
        }
        let source = match source {
            Some(s) if sources.iter().any(|x| x.id == s) => Some(s),
            Some(s) => {
                issues.push(MapIssue::warn(
                    format!("{field}.paper[{i}].source"),
                    format!("unknown source id {s:?}: the reference is kept as literal text with no identified source"),
                ));
                None
            }
            None => None,
        };
        out.push(PaperRef { text: text.trim().to_string(), source, note: clean_opt(note) });
    }
    out
}

/// Parses and validates a sidecar. `Err` means the file as a whole cannot be
/// used (syntax error, unknown field, unsupported schema); problems confined
/// to one entry drop that entry and are reported in `ProjectMap::issues`.
pub fn parse(text: &str, path: &Path) -> Result<ProjectMap, Vec<MapIssue>> {
    let raw: RawMap = match toml::from_str(text) {
        Ok(r) => r,
        Err(e) => {
            let line = e.span().map(|s| text[..s.start.min(text.len())].matches('\n').count() + 1);
            let field = line.map(|l| format!("line {l}")).unwrap_or_default();
            return Err(vec![MapIssue::err(field, e.message().to_string())]);
        }
    };
    match raw.schema {
        Some(SCHEMA_VERSION) => {}
        Some(v) => {
            return Err(vec![MapIssue::err(
                "schema",
                format!("unsupported map schema version {v} (this slim reads version {SCHEMA_VERSION}); the file was not changed"),
            )])
        }
        None => return Err(vec![MapIssue::err("schema", format!("missing: add `schema = {SCHEMA_VERSION}` at the top"))]),
    }
    let mut issues: Vec<MapIssue> = Vec::new();
    let mut m = ProjectMap {
        path: path.to_path_buf(),
        sha256: crate::paths::sha256_hex(text.as_bytes()),
        project_name: clean_opt(raw.project.name),
        title: clean_opt(raw.project.title),
        description: clean_opt(raw.project.description),
        revision: clean_opt(raw.project.revision),
        ..Default::default()
    };

    for (i, s) in raw.source.into_iter().enumerate() {
        if !valid_id(&s.id) {
            issues.push(MapIssue::err(format!("source[{i}].id"), "ids use letters, digits, '-', '_' and '.'"));
        } else if m.sources.iter().any(|x| x.id == s.id) {
            issues.push(MapIssue::err(format!("source[{i}].id"), format!("duplicate source id {:?}", s.id)));
        } else if s.citation.trim().is_empty() {
            issues.push(MapIssue::err(format!("source[{i}].citation"), "a source needs a citation"));
        } else {
            m.sources.push(Source { id: s.id, citation: s.citation.trim().to_string(), url: clean_opt(s.url) });
        }
    }

    // Groups: ids first, then parents, so order in the file does not matter.
    let mut raw_groups: Vec<(usize, RawGroup)> = Vec::new();
    let mut ids: HashMap<String, usize> = HashMap::new();
    for (i, g) in raw.group.into_iter().enumerate() {
        if !valid_id(&g.id) {
            issues.push(MapIssue::err(format!("group[{i}].id"), "ids use letters, digits, '-', '_' and '.'"));
            continue;
        }
        if ids.contains_key(&g.id) {
            issues.push(MapIssue::err(format!("group[{i}].id"), format!("duplicate group id {:?} (entry dropped)", g.id)));
            continue;
        }
        ids.insert(g.id.clone(), raw_groups.len());
        raw_groups.push((i, g));
    }
    let mut parents: Vec<Option<usize>> = Vec::new();
    for (i, g) in &raw_groups {
        let p = match &g.parent {
            None => None,
            Some(p) => match ids.get(p) {
                Some(&k) => Some(k),
                None => {
                    issues.push(MapIssue::err(format!("group[{i}].parent"), format!("unknown group {p:?}: shown at the top level")));
                    None
                }
            },
        };
        parents.push(p);
    }
    // Break parent cycles (a group reachable from itself goes to the top level).
    for k in 0..parents.len() {
        let mut seen = HashSet::new();
        let mut cur = Some(k);
        while let Some(c) = cur {
            if !seen.insert(c) {
                issues.push(MapIssue::err(
                    format!("group[{}].parent", raw_groups[k].0),
                    format!("parent cycle through {:?}: shown at the top level", raw_groups[k].1.id),
                ));
                parents[k] = None;
                break;
            }
            cur = parents[c];
        }
    }
    for (k, (i, g)) in raw_groups.into_iter().enumerate() {
        let field = format!("group[{i}]");
        let kind = match g.kind.as_deref() {
            None | Some("component") => GroupKind::Component,
            Some("foundation") => GroupKind::Foundation,
            Some(other) => {
                issues.push(MapIssue::warn(format!("{field}.kind"), format!("unknown kind {other:?} (expected component or foundation); treated as component")));
                GroupKind::Component
            }
        };
        let mut landmarks = Vec::new();
        for (j, p) in g.landmarks.into_iter().enumerate() {
            let (decl, role) = match p {
                RawPlacement::Name(n) => (n, None),
                RawPlacement::Full { decl, role } => (decl, role),
            };
            if !valid_name(&decl) {
                issues.push(MapIssue::err(format!("{field}.landmarks[{j}]"), "not a declaration name"));
            } else if landmarks.iter().any(|x: &Placement| x.decl == decl) {
                issues.push(MapIssue::warn(format!("{field}.landmarks[{j}]"), format!("{decl} is listed twice in this group")));
            } else {
                landmarks.push(Placement { decl: decl.trim().to_string(), role: clean_opt(role) });
            }
        }
        let paper = papers(g.paper, &field, &m.sources, &mut issues);
        m.groups.push(Group {
            title: clean_opt(g.title).unwrap_or_else(|| g.id.clone()),
            id: g.id,
            kind,
            parent: parents[k],
            children: Vec::new(),
            order: g.order,
            purpose: clean_opt(g.purpose),
            explanation: clean_opt(g.explanation),
            origin: g.origin,
            // Generated organization is unreviewed unless someone says otherwise.
            reviewed: g.reviewed.unwrap_or(g.origin == Provenance::Authored),
            paper,
            limitations: g.limitations.into_iter().filter(|l| !l.trim().is_empty()).collect(),
            landmarks,
            modules: g.modules.into_iter().filter(|x| valid_name(x)).collect(),
        });
    }
    let mut order: Vec<usize> = (0..m.groups.len()).collect();
    order.sort_by_key(|&k| (m.groups[k].order, k));
    for k in order {
        match m.groups[k].parent {
            Some(p) => m.groups[p].children.push(k),
            None => m.roots.push(k),
        }
    }

    // Declaration annotations: `[[landmark]]` entries, then objectives.
    let annotate = |m: &mut ProjectMap, lm: Landmark, field: &str, issues: &mut Vec<MapIssue>| match m.landmark_index.get(&lm.decl) {
        Some(&k) => {
            let old = &mut m.landmarks[k];
            if old.purpose.is_some() && lm.purpose.is_some() && old.purpose != lm.purpose {
                issues.push(MapIssue::warn(format!("{field}.purpose"), format!("{} already has a different purpose; the first one is kept", lm.decl)));
            }
            old.purpose = old.purpose.take().or(lm.purpose);
            old.explanation = old.explanation.take().or(lm.explanation);
            old.paper.extend(lm.paper);
            old.limitations.extend(lm.limitations);
            old.source_sha256 = old.source_sha256.take().or(lm.source_sha256);
        }
        None => {
            m.landmark_index.insert(lm.decl.clone(), m.landmarks.len());
            m.landmarks.push(lm);
        }
    };
    for (i, l) in raw.landmark.into_iter().enumerate() {
        let field = format!("landmark[{i}]");
        if !valid_name(&l.decl) {
            issues.push(MapIssue::err(format!("{field}.decl"), "not a declaration name"));
            continue;
        }
        let decl = l.decl.trim().to_string();
        for (j, gid) in l.groups.iter().enumerate() {
            match m.groups.iter_mut().find(|g| &g.id == gid) {
                Some(g) => {
                    if !g.landmarks.iter().any(|p| p.decl == decl) {
                        g.landmarks.push(Placement { decl: decl.clone(), role: None });
                    }
                }
                None => issues.push(MapIssue::err(format!("{field}.groups[{j}]"), format!("unknown group {gid:?}"))),
            }
        }
        let paper = papers(l.paper, &field, &m.sources, &mut issues);
        let lm = Landmark {
            decl,
            purpose: clean_opt(l.purpose),
            explanation: clean_opt(l.explanation),
            paper,
            limitations: l.limitations,
            origin: l.origin,
            source_sha256: clean_opt(l.source_sha256),
        };
        annotate(&mut m, lm, &field, &mut issues);
    }
    for (i, o) in raw.objective.into_iter().enumerate() {
        let field = format!("objective[{i}]");
        if !valid_name(&o.decl) {
            issues.push(MapIssue::err(format!("{field}.decl"), "not a declaration name"));
            continue;
        }
        let decl = o.decl.trim().to_string();
        if m.objectives.iter().any(|x| x.decl == decl) {
            issues.push(MapIssue::warn(format!("{field}.decl"), format!("{decl} is already an objective")));
            continue;
        }
        let paper = papers(o.paper, &field, &m.sources, &mut issues);
        let lm = Landmark {
            decl: decl.clone(),
            purpose: clean_opt(o.purpose),
            explanation: clean_opt(o.explanation),
            paper,
            limitations: o.limitations,
            origin: o.origin,
            source_sha256: clean_opt(o.source_sha256),
        };
        annotate(&mut m, lm, &field, &mut issues);
        m.objectives.push(Objective { decl, title: clean_opt(o.title) });
    }

    for (i, r) in raw.relation.into_iter().enumerate() {
        let field = format!("relation[{i}]");
        if !valid_name(&r.from) || !valid_name(&r.to) {
            issues.push(MapIssue::err(field, "`from` and `to` must be declaration names"));
            continue;
        }
        if r.from == r.to {
            issues.push(MapIssue::err(field, "a declaration cannot be related to itself"));
            continue;
        }
        m.relations.push(Relation {
            from: r.from.trim().to_string(),
            to: r.to.trim().to_string(),
            kind: clean_opt(r.kind).unwrap_or_else(|| "uses".into()),
            note: clean_opt(r.note),
            origin: r.origin,
            evidence: clean_opt(r.evidence),
            step: None,
        });
    }
    for (i, a) in raw.argument.into_iter().enumerate() {
        let field = format!("argument[{i}]");
        if !valid_name(&a.goal) {
            issues.push(MapIssue::err(format!("{field}.goal"), "not a declaration name"));
            continue;
        }
        let goal = a.goal.trim().to_string();
        let mut steps: Vec<String> = Vec::new();
        for (j, s) in a.steps.into_iter().enumerate() {
            if !valid_name(&s.decl) {
                issues.push(MapIssue::err(format!("{field}.steps[{j}].decl"), "not a declaration name"));
                continue;
            }
            let decl = s.decl.trim().to_string();
            let from = match s.parent {
                None => goal.clone(),
                Some(p) if p == goal || steps.contains(&p) => p,
                Some(p) => {
                    issues.push(MapIssue::err(
                        format!("{field}.steps[{j}].for"),
                        format!("{p:?} is neither the goal nor an earlier step: the step is placed under the goal"),
                    ));
                    goal.clone()
                }
            };
            if from == decl {
                issues.push(MapIssue::err(format!("{field}.steps[{j}]"), "a step cannot support itself"));
                continue;
            }
            m.relations.push(Relation {
                from,
                to: decl.clone(),
                kind: "argument step".into(),
                note: clean_opt(s.why),
                origin: a.origin,
                evidence: None,
                step: Some(j as u32 + 1),
            });
            if !steps.contains(&decl) {
                steps.push(decl);
            }
        }
        m.arguments.push(Argument { goal, title: clean_opt(a.title), summary: clean_opt(a.summary), origin: a.origin, steps });
    }

    for (i, t) in raw.trail.into_iter().enumerate() {
        let field = format!("trail[{i}]");
        if !valid_id(&t.id) || m.trails.iter().any(|x| x.id == t.id) {
            issues.push(MapIssue::err(format!("{field}.id"), "missing, malformed or duplicate trail id (entry dropped)"));
            continue;
        }
        let mut stops = Vec::new();
        for (j, s) in t.stops.into_iter().enumerate() {
            let target = match (s.decl, s.group) {
                (Some(d), None) if valid_name(&d) => StopRef::Decl(d.trim().to_string()),
                (None, Some(g)) if m.groups.iter().any(|x| x.id == g) => StopRef::Group(g),
                (None, Some(g)) => {
                    issues.push(MapIssue::err(format!("{field}.stops[{j}].group"), format!("unknown group {g:?}")));
                    continue;
                }
                _ => {
                    issues.push(MapIssue::err(format!("{field}.stops[{j}]"), "a stop names exactly one of `decl` or `group`"));
                    continue;
                }
            };
            stops.push(SharedStop { target, note: clean_opt(s.note), question: clean_opt(s.question) });
        }
        m.trails.push(SharedTrail { title: clean_opt(t.title).unwrap_or_else(|| t.id.clone()), id: t.id, description: clean_opt(t.description), stops });
    }
    m.issues = issues;
    Ok(m)
}

/// Reads the sidecar at `path`. Never writes.
pub fn load(path: &Path) -> MapLoad {
    let md = match std::fs::metadata(path) {
        Ok(md) => md,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return MapLoad::Absent { looked_at: Some(path.to_path_buf()) },
        Err(e) => return MapLoad::Malformed { path: path.to_path_buf(), issues: vec![MapIssue::err("", format!("cannot read: {e}"))] },
    };
    let bad = |m: String| MapLoad::Malformed { path: path.to_path_buf(), issues: vec![MapIssue::err("", m)] };
    if !md.is_file() {
        return bad("not a regular file".into());
    }
    if md.len() > MAX_BYTES {
        return bad(format!("{} bytes exceeds the {} byte limit for a map", md.len(), MAX_BYTES));
    }
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => return bad(format!("cannot read as UTF-8 text: {e}")),
    };
    match parse(&text, path) {
        Ok(m) => MapLoad::Loaded(Arc::new(m)),
        Err(issues) => MapLoad::Malformed { path: path.to_path_buf(), issues },
    }
}

/// The sidecar of a project root (contained in the root: a symlink leading
/// outside it is refused).
pub fn load_for_root(root: &Path) -> MapLoad {
    let direct = root.join(FILE_NAME);
    if std::fs::symlink_metadata(&direct).is_err() {
        return MapLoad::Absent { looked_at: Some(direct) };
    }
    match crate::paths::resolve_contained(root, FILE_NAME) {
        Ok(p) => load(&p),
        Err(e) => MapLoad::Malformed { path: direct, issues: vec![MapIssue::err("", format!("{e:#}"))] },
    }
}

/// A match of a search query against the map.
#[derive(Clone, Debug, PartialEq)]
pub struct MapHit {
    /// Key of the map row to reveal (see `app::views`).
    pub key: String,
    pub label: String,
    pub what: &'static str,
}

impl ProjectMap {
    pub fn landmark(&self, decl: &str) -> Option<&Landmark> {
        self.landmark_index.get(decl).map(|&k| &self.landmarks[k])
    }

    pub fn group_by_id(&self, id: &str) -> Option<usize> {
        self.groups.iter().position(|g| g.id == id)
    }

    /// Groups a declaration is placed in.
    pub fn placements(&self, decl: &str) -> Vec<usize> {
        (0..self.groups.len()).filter(|&k| self.groups[k].landmarks.iter().any(|p| p.decl == decl)).collect()
    }

    pub fn is_objective(&self, decl: &str) -> bool {
        self.objectives.iter().any(|o| o.decl == decl)
    }

    /// Titles from the top level down to group `k`.
    pub fn group_path(&self, k: usize) -> Vec<String> {
        let mut v = Vec::new();
        let mut cur = Some(k);
        while let Some(c) = cur {
            v.push(self.groups[c].title.clone());
            cur = self.groups[c].parent;
            if v.len() > 64 {
                break;
            }
        }
        v.reverse();
        v
    }

    /// Distinct declarations placed in at least one group or chosen as an objective.
    pub fn curated_landmark_count(&self) -> usize {
        let mut s: HashSet<&str> = self.objectives.iter().map(|o| o.decl.as_str()).collect();
        for g in &self.groups {
            s.extend(g.landmarks.iter().map(|p| p.decl.as_str()));
        }
        s.len()
    }

    /// Distinct landmarks in a group and all its subgroups.
    pub fn group_landmark_count(&self, k: usize) -> usize {
        let mut s: HashSet<&str> = HashSet::new();
        let mut stack = vec![k];
        while let Some(c) = stack.pop() {
            s.extend(self.groups[c].landmarks.iter().map(|p| p.decl.as_str()));
            stack.extend(&self.groups[c].children);
        }
        s.len()
    }

    /// Every declaration name the map mentions, with where it is mentioned.
    pub fn decl_refs(&self) -> Vec<(String, Vec<String>)> {
        let mut order: Vec<String> = Vec::new();
        let mut uses: HashMap<String, Vec<String>> = HashMap::new();
        let mut add = |name: &str, w: String| {
            let e = uses.entry(name.to_string()).or_insert_with(|| {
                order.push(name.to_string());
                Vec::new()
            });
            if !e.contains(&w) && e.len() < 6 {
                e.push(w);
            }
        };
        for o in &self.objectives {
            add(&o.decl, "objective".into());
        }
        for g in &self.groups {
            for p in &g.landmarks {
                add(&p.decl, format!("group {}", g.id));
            }
        }
        for l in &self.landmarks {
            add(&l.decl, "landmark annotation".into());
        }
        for r in &self.relations {
            let w = if r.step.is_some() { "curated argument" } else { "curated relation" };
            add(&r.from, w.into());
            add(&r.to, w.into());
        }
        for a in &self.arguments {
            add(&a.goal, "curated argument (goal)".into());
        }
        for t in &self.trails {
            for s in &t.stops {
                if let StopRef::Decl(d) = &s.target {
                    add(d, format!("shared trail {}", t.id));
                }
            }
        }
        order.into_iter().map(|n| (n.clone(), uses.remove(&n).unwrap_or_default())).collect()
    }

    /// Curated relations leaving (`uses`) or entering (`used by`) a declaration,
    /// argument steps first and in their written order.
    pub fn curated(&self, name: &str, uses: bool) -> Vec<&Relation> {
        let mut v: Vec<&Relation> = self.relations.iter().filter(|r| if uses { r.from == name } else { r.to == name }).collect();
        v.sort_by_key(|r| r.step.unwrap_or(u32::MAX));
        v
    }

    /// A shortest route `from → … → to` through curated relations ("uses"
    /// direction), bounded. Each element is (name, relation leading to the next).
    pub fn curated_path(&self, from: &str, to: &str, max_nodes: usize) -> Option<Vec<(String, Option<Relation>)>> {
        if from == to {
            return Some(vec![(from.to_string(), None)]);
        }
        let mut prev: HashMap<&str, &Relation> = HashMap::new();
        let mut queue: VecDeque<&str> = VecDeque::new();
        queue.push_back(from);
        let mut found = false;
        while let Some(n) = queue.pop_front() {
            for r in self.relations.iter().filter(|r| r.from == n) {
                if r.to != from && !prev.contains_key(r.to.as_str()) {
                    prev.insert(&r.to, r);
                    if r.to == to {
                        found = true;
                        break;
                    }
                    queue.push_back(&r.to);
                }
            }
            if found || prev.len() > max_nodes {
                break;
            }
        }
        if !found {
            return None;
        }
        let mut rev: Vec<(String, Option<Relation>)> = vec![(to.to_string(), None)];
        let mut cur = to;
        while cur != from {
            let r = prev.get(cur)?;
            rev.push((r.from.clone(), Some((*r).clone())));
            cur = &r.from;
        }
        rev.reverse();
        Some(rev)
    }

    /// Groups and landmarks matching every word of `query` (case-insensitive).
    pub fn search(&self, query: &str, limit: usize) -> Vec<MapHit> {
        let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
        if words.is_empty() {
            return Vec::new();
        }
        let hit = |hay: &str| {
            let h = hay.to_lowercase();
            words.iter().all(|w| h.contains(w))
        };
        let mut out = Vec::new();
        for g in &self.groups {
            if hit(&format!("{} {} {}", g.id, g.title, g.purpose.as_deref().unwrap_or(""))) {
                out.push(MapHit { key: format!("grp:{}", g.id), label: g.title.clone(), what: "group" });
            }
        }
        for o in &self.objectives {
            let lm = self.landmark(&o.decl);
            if hit(&format!("{} {} {}", o.decl, o.title.as_deref().unwrap_or(""), lm.and_then(|l| l.purpose.as_deref()).unwrap_or(""))) {
                out.push(MapHit { key: format!("obj:{}", o.decl), label: o.title.clone().unwrap_or_else(|| o.decl.clone()), what: "objective" });
            }
        }
        for g in &self.groups {
            for p in &g.landmarks {
                let lm = self.landmark(&p.decl);
                let hay = format!("{} {} {}", p.decl, p.role.as_deref().unwrap_or(""), lm.and_then(|l| l.purpose.as_deref()).unwrap_or(""));
                if hit(&hay) {
                    out.push(MapHit { key: format!("lm:{}:{}", g.id, p.decl), label: format!("{} ({})", p.decl, g.title), what: "landmark" });
                }
            }
        }
        out.truncate(limit);
        out
    }
}

// ---------------------------------------------------------------------------
// Resolution against an index
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum RefStatus {
    Resolved,
    /// Several indexed candidates share the name (source-only indexes).
    Ambiguous { count: u64 },
    /// Not in the index. Suggestions are candidates with the same last name
    /// component; they are never applied automatically.
    Missing { suggestions: Vec<String> },
}

#[derive(Clone, Debug)]
pub struct RefInfo {
    pub status: RefStatus,
    pub row: Option<DeclRow>,
    /// SHA-256 of the declaring module's source file, as recorded in the index.
    pub module_sha256: Option<String>,
}

/// How the names in a map correspond to the open index.
#[derive(Clone, Debug, Default)]
pub struct Resolution {
    pub refs: HashMap<String, RefInfo>,
    pub missing_modules: Vec<String>,
    /// The index is a source-only scan: it holds this project's files only,
    /// so library declarations cannot resolve.
    pub source_only: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Unresolved {
    pub name: String,
    pub why: String,
    pub used_in: Vec<String>,
    pub suggestions: Vec<String>,
}

fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

/// Looks up one name: the row slim navigates to, how many declarations
/// share the name, and the indexed fingerprint of its source file.
pub fn resolve_name(conn: &Connection, name: &str) -> Result<Option<(DeclRow, u64, Option<String>)>> {
    let r = conn
        .query_row(
            "SELECT d.id, d.name, d.display, d.kind, d.origin, d.flags, m.name, m.sha256,
                    (SELECT count(*) FROM decls x WHERE x.name = d.name)
               FROM decls d LEFT JOIN modules m ON m.id = d.module_id
              WHERE d.name = ?1 ORDER BY d.origin, d.id LIMIT 1",
            [name],
            |r| {
                Ok((
                    DeclRow {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        display: r.get(2)?,
                        kind: DeclKind::from_u8(r.get::<_, u8>(3)?),
                        origin: Origin::from_u8(r.get::<_, u8>(4)?),
                        flags: r.get::<_, i64>(5)? as u32,
                        module: r.get(6)?,
                    },
                    r.get::<_, i64>(8)? as u64,
                    r.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .optional()?;
    Ok(r)
}

pub fn resolve(map: &ProjectMap, conn: &Connection) -> Result<Resolution> {
    resolve_with(map, conn, true)
}

/// Resolution with or without rename hints. The hints scan the whole
/// declaration table once per missing name (bounded in number and time), so
/// an interactive caller resolves without them first and adds them after.
pub fn resolve_with(map: &ProjectMap, conn: &Connection, hints: bool) -> Result<Resolution> {
    let mut res = Resolution { source_only: crate::db::mode_of(conn).ok() == Some(crate::db::IndexMode::Source), ..Default::default() };
    let t0 = std::time::Instant::now();
    let mut suggested = 0usize;
    for (name, _) in map.decl_refs() {
        let info = match resolve_name(conn, &name)? {
            // A name the snapshot only refers to (no description) is not a
            // declaration a reader can open: report it as missing.
            Some((row, _, _)) if row.origin == Origin::Unresolved => RefInfo {
                status: RefStatus::Missing { suggestions: Vec::new() },
                row: Some(row),
                module_sha256: None,
            },
            Some((row, n, sha)) => RefInfo { status: if n > 1 { RefStatus::Ambiguous { count: n } } else { RefStatus::Resolved }, row: Some(row), module_sha256: sha },
            None => {
                // Rename hints are a full scan: bounded in number and time.
                let mut suggestions = Vec::new();
                if hints && suggested < 20 && t0.elapsed().as_millis() < 400 {
                    suggested += 1;
                    let last = &name[crate::model::last_component_start(&name)..];
                    // LIKE ignores ASCII case: fetch a few more and keep exact suffixes.
                    let mut st = conn.prepare_cached("SELECT DISTINCT name FROM decls WHERE name LIKE ?1 ESCAPE '\\' AND origin != 2 LIMIT 40")?;
                    let found: Vec<String> = st.query_map([format!("%.{}", like_escape(last))], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
                    let suffix = format!(".{last}");
                    suggestions = found.into_iter().filter(|n| n.ends_with(&suffix)).take(4).collect();
                }
                RefInfo { status: RefStatus::Missing { suggestions }, row: None, module_sha256: None }
            }
        };
        res.refs.insert(name, info);
    }
    let mut st = conn.prepare_cached("SELECT 1 FROM modules WHERE name = ?1")?;
    for g in &map.groups {
        for m in &g.modules {
            if st.query_row([m], |_| Ok(())).optional()?.is_none() && !res.missing_modules.contains(m) {
                res.missing_modules.push(m.clone());
            }
        }
    }
    Ok(res)
}

impl Resolution {
    pub fn row(&self, name: &str) -> Option<&DeclRow> {
        self.refs.get(name).filter(|i| !matches!(i.status, RefStatus::Missing { .. })).and_then(|i| i.row.as_ref())
    }

    pub fn has_missing(&self) -> bool {
        self.refs.values().any(|i| matches!(i.status, RefStatus::Missing { .. }) && i.row.is_none())
    }

    pub fn is_missing(&self, name: &str) -> bool {
        matches!(self.refs.get(name).map(|i| &i.status), Some(RefStatus::Missing { .. }))
    }

    /// Declared outside the project (an imported library).
    pub fn is_external(&self, name: &str) -> bool {
        self.row(name).is_some_and(|r| r.origin == Origin::Library)
    }

    /// An annotation is stale when it recorded a source fingerprint that no
    /// longer matches the file the index saw.
    pub fn is_stale(&self, lm: &Landmark) -> bool {
        match (&lm.source_sha256, self.refs.get(&lm.decl).and_then(|i| i.module_sha256.as_ref())) {
            (Some(want), Some(have)) => !have.starts_with(want.trim()),
            _ => false,
        }
    }

    pub fn unresolved(&self, map: &ProjectMap) -> Vec<Unresolved> {
        let mut out = Vec::new();
        for (name, used_in) in map.decl_refs() {
            match self.refs.get(&name).map(|i| &i.status) {
                Some(RefStatus::Missing { suggestions }) => out.push(Unresolved {
                    why: if self.source_only && !used_in.is_empty() && used_in.iter().all(|u| map.group_by_id(u.trim_start_matches("group ")).is_some_and(|k| map.groups[k].kind == GroupKind::Foundation)) {
                        "not in this source-only index: it holds the project's own files, so library declarations resolve only in a semantic snapshot".into()
                    } else {
                        "not in this index (renamed, removed or not indexed)".into()
                    },
                    name,
                    used_in,
                    suggestions: suggestions.clone(),
                }),
                Some(RefStatus::Ambiguous { count }) => out.push(Unresolved {
                    name,
                    why: format!("ambiguous: {count} indexed candidates share this name; slim opens the first"),
                    used_in,
                    suggestions: Vec::new(),
                }),
                _ => {}
            }
        }
        for m in &self.missing_modules {
            out.push(Unresolved { name: m.clone(), why: "module not in this index".into(), used_in: vec!["group modules".into()], suggestions: Vec::new() });
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Candidate paper references in documentation
// ---------------------------------------------------------------------------

/// Literal fragments such as "Lemma 3.9" or "equation (3.16)" found in a
/// documentation or source comment. They are candidates only: nothing says
/// which paper they refer to, so they are shown verbatim and never attached
/// to a source.
pub fn paper_candidates(text: &str) -> Vec<String> {
    const KEYS: &[&str] = &[
        "lemma", "theorem", "thm.", "proposition", "prop.", "corollary", "cor.", "definition", "defn.", "section", "sec.",
        "equation", "eq.", "eqn.", "remark", "example", "chapter", "figure", "fig.", "table", "conjecture", "claim", "§",
    ];
    let is_number = |w: &str| -> bool {
        let core = w.trim_matches(|c: char| matches!(c, '(' | ')' | ',' | ';' | ':' | '.' | '[' | ']' | '`' | '*'));
        !core.is_empty()
            && core.len() <= 12
            && core.chars().any(|c| c.is_ascii_digit())
            && core.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
            && core.chars().filter(|c| c.is_ascii_alphabetic()).count() <= 1
    };
    let mut out: Vec<String> = Vec::new();
    let words: Vec<&str> = text.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        let lw = w.trim_start_matches(|c: char| matches!(c, '(' | '[' | '*' | '`')).to_lowercase();
        let hit = if let Some(rest) = lw.strip_prefix('§') {
            // "§3.2" is one word.
            if is_number(rest) { Some(w.to_string()) } else if rest.is_empty() { words.get(i + 1).filter(|n| is_number(n)).map(|n| format!("{w} {n}")) } else { None }
        } else if KEYS.contains(&lw.as_str()) || KEYS.contains(&lw.trim_end_matches('s')) {
            words.get(i + 1).filter(|n| is_number(n)).map(|n| format!("{w} {n}"))
        } else {
            None
        };
        if let Some(h) = hit {
            let h = h.trim_matches(|c: char| matches!(c, ',' | ';' | ':' | '`' | '*')).trim_end_matches('.').to_string();
            if !out.contains(&h) {
                out.push(h);
                if out.len() >= 12 {
                    break;
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Starting skeleton
// ---------------------------------------------------------------------------

fn toml_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if c.is_control() => o.push_str(&format!("\\u{:04X}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// A starting sidecar derived from module names only. Every group it emits is
/// marked `generated` and unreviewed: directories are not mathematical chapters.
pub fn skeleton_toml(project: &str, modules: &[(String, u64)]) -> String {
    let comps: Vec<Vec<&str>> = modules.iter().map(|(n, _)| crate::model::name_components(n)).collect();
    // Skip the components shared by every module below the root (usually the
    // library name), so that groups are its top-level directories.
    let nested: Vec<&Vec<&str>> = comps.iter().filter(|c| c.len() >= 2).collect();
    let mut common = 0;
    while !nested.is_empty() && nested.iter().all(|c| c.len() > common + 2 && c[common] == nested[0][common]) {
        common += 1;
    }
    // (key, parent key, modules, declarations): top-level directories, and
    // one level of subdirectories below each.
    let mut groups: Vec<(String, Option<String>, Vec<String>, u64)> = Vec::new();
    let mut add = |key: String, parent: Option<String>, name: &str, n: u64| match groups.iter_mut().find(|g| g.0 == key) {
        Some(g) => {
            g.2.push(name.to_string());
            g.3 += n;
        }
        None => groups.push((key, parent, vec![name.to_string()], n)),
    };
    for (c, (name, n)) in comps.iter().zip(modules) {
        if c.len() <= common + 1 {
            continue; // a root module, not a directory
        }
        let top = c[..=common].join(".");
        add(top.clone(), None, name, *n);
        if c.len() > common + 2 {
            add(c[..=common + 1].join("."), Some(top), name, *n);
        }
    }
    // Subgroups only where they organise something: 2 to 40 per directory.
    let subs = |top: &str| groups.iter().filter(|g| g.1.as_deref() == Some(top)).count();
    let keep: Vec<bool> = groups.iter().map(|g| g.1.as_deref().is_none_or(|p| (2..=40).contains(&subs(p)))).collect();
    let groups: Vec<_> = groups.iter().zip(keep).filter(|(_, k)| *k).map(|(g, _)| g.clone()).collect();
    let slug = |key: &str| -> String { key.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect() };
    let mut s = String::new();
    s.push_str("# lean-view map (written by `slim map init`; edit freely, slim never rewrites it).\n");
    s.push_str("# The groups below mirror directories. They are a starting point, not a mathematical\n");
    s.push_str("# structure: rename, merge and reorder them, give each a purpose, list landmark\n");
    s.push_str("# declarations, then set `origin = \"authored\"` and `reviewed = true`.\n");
    s.push_str("# Schema and examples: docs/map.md.\n\n");
    s.push_str(&format!("schema = {SCHEMA_VERSION}\n\n[project]\nname = {}\n\n", toml_str(project)));
    s.push_str("# Main results are chosen by a person, never inferred. Example:\n# [[objective]]\n# decl = \"Fully.Qualified.name\"\n# title = \"What it says, in words\"\n\n");
    for (i, (key, parent, mods, n)) in groups.iter().enumerate() {
        let last = key.rsplit('.').next().unwrap_or(key);
        s.push_str(&format!("[[group]]\nid = {}\ntitle = {}\n", toml_str(&slug(key)), toml_str(last)));
        if let Some(p) = parent {
            s.push_str(&format!("parent = {}\n", toml_str(&slug(p))));
        }
        s.push_str(&format!(
            "order = {}\norigin = \"generated\"\nreviewed = false\n# {} indexed declaration candidates in {} module(s) when generated\nlandmarks = []\n",
            (i + 1) * 10,
            n,
            mods.len()
        ));
        if mods.len() <= 40 {
            s.push_str(&format!("modules = [{}]\n", mods.iter().map(|m| toml_str(m)).collect::<Vec<_>>().join(", ")));
        } else {
            s.push_str(&format!("modules = [{}]  # first 40 of {}\n", mods.iter().take(40).map(|m| toml_str(m)).collect::<Vec<_>>().join(", "), mods.len()));
        }
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
schema = 1
[project]
name = "demo"
[[source]]
id = "notes"
citation = "Author, Notes, 2020"
[[objective]]
decl = "Demo.main"
title = "Main"
purpose = "the point"
paper = [{ text = "Theorem 1.1", source = "notes" }, "Lemma 3.9"]
[[group]]
id = "b"
title = "Second"
order = 20
[[group]]
id = "a"
title = "First"
order = 10
landmarks = ["Demo.x", { decl = "Demo.shared", role = "used twice" }]
[[group]]
id = "a1"
parent = "a"
landmarks = ["Demo.shared", "Demo.«weird.name».ℛ"]
[[group]]
id = "lib"
kind = "foundation"
origin = "generated"
landmarks = ["Nat.add_comm"]
[[landmark]]
decl = "Demo.shared"
purpose = "glue"
groups = ["b"]
[[relation]]
from = "Demo.x"
to = "Demo.shared"
note = "by hand"
[[relation]]
from = "Demo.shared"
to = "Demo.x"
[[argument]]
goal = "Demo.main"
steps = [{ decl = "Demo.x", why = "first" }, { decl = "Demo.y", for = "Demo.x" }]
[[trail]]
id = "intro"
stops = [{ decl = "Demo.main", note = "start" }, { group = "a" }]
"#;

    fn good() -> ProjectMap {
        parse(GOOD, Path::new("lean-view.toml")).unwrap()
    }

    #[test]
    fn groups_nest_order_and_place_one_identity_many_times() {
        let m = good();
        assert!(m.issues.is_empty(), "{:?}", m.issues);
        let titles: Vec<&str> = m.roots.iter().map(|&k| m.groups[k].title.as_str()).collect();
        assert_eq!(titles, ["lib", "First", "Second"], "explicit order, file order among equals");
        let a = m.group_by_id("a").unwrap();
        assert_eq!(m.groups[a].children, vec![m.group_by_id("a1").unwrap()]);
        // One annotation, three placements (two listed, one via `groups`).
        let p: Vec<&str> = m.placements("Demo.shared").into_iter().map(|k| m.groups[k].id.as_str()).collect();
        assert_eq!(p, ["b", "a", "a1"]);
        assert_eq!(m.landmark("Demo.shared").unwrap().purpose.as_deref(), Some("glue"));
        assert_eq!(m.landmarks.iter().filter(|l| l.decl == "Demo.shared").count(), 1);
        assert_eq!(m.curated_landmark_count(), 5);
        assert_eq!(m.group_landmark_count(a), 3);
        assert_eq!(m.group_path(m.group_by_id("a1").unwrap()), ["First", "a1"]);
        // Unicode and quoted names are kept verbatim.
        assert!(m.decl_refs().iter().any(|(n, _)| n == "Demo.«weird.name».ℛ"));
        // Generated groups are unreviewed unless stated.
        assert!(!m.groups[m.group_by_id("lib").unwrap()].reviewed);
        assert!(m.groups[a].reviewed);
    }

    #[test]
    fn paper_references_keep_literal_text_and_only_named_sources() {
        let m = good();
        let p = &m.landmark("Demo.main").unwrap().paper;
        assert_eq!(p[0], PaperRef { text: "Theorem 1.1".into(), source: Some("notes".into()), note: None });
        assert_eq!(p[1], PaperRef { text: "Lemma 3.9".into(), source: None, note: None });
        // An unknown source id is not turned into a citation.
        let bad = GOOD.replace("source = \"notes\" }", "source = \"nowhere\" }");
        let m = parse(&bad, Path::new("x")).unwrap();
        assert_eq!(m.landmark("Demo.main").unwrap().paper[0].source, None);
        assert!(m.issues.iter().any(|i| i.field == "objective[0].paper[0].source" && i.severity == Severity::Warning));
    }

    #[test]
    fn curated_relations_have_a_direction_and_argument_order() {
        let m = good();
        let from_main: Vec<&str> = m.curated("Demo.main", true).iter().map(|r| r.to.as_str()).collect();
        assert_eq!(from_main, ["Demo.x"]);
        // `for` hangs a step under an earlier step.
        let from_x: Vec<(&str, Option<u32>)> = m.curated("Demo.x", true).iter().map(|r| (r.to.as_str(), r.step)).collect();
        assert_eq!(from_x, [("Demo.y", Some(2)), ("Demo.shared", None)]);
        let users: Vec<&str> = m.curated("Demo.x", false).iter().map(|r| r.from.as_str()).collect();
        assert_eq!(users, ["Demo.main", "Demo.shared"]);
        // A curated cycle (x ↔ shared) does not trap the path search.
        let p = m.curated_path("Demo.main", "Demo.shared", 100).unwrap();
        assert_eq!(p.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["Demo.main", "Demo.x", "Demo.shared"]);
        assert!(m.curated_path("Demo.shared", "Demo.main", 100).is_none());
        assert!(m.curated_path("Demo.y", "Demo.nowhere", 100).is_none());
    }

    #[test]
    fn malformed_files_report_the_field_and_are_not_usable() {
        // Syntax error: the line is reported.
        let e = parse("schema = 1\n[[group]]\nid = \n", Path::new("x")).unwrap_err();
        assert!(e[0].field.starts_with("line "), "{e:?}");
        // Unknown field (a typo) is an error, not silently ignored.
        let e = parse("schema = 1\n[[group]]\nid = \"a\"\npurpos = \"x\"\n", Path::new("x")).unwrap_err();
        assert!(e[0].message.contains("purpos"), "{e:?}");
        // Unsupported and missing schema versions.
        let e = parse("schema = 99\n", Path::new("x")).unwrap_err();
        assert!(e[0].field == "schema" && e[0].message.contains("99"));
        assert!(parse("[project]\nname = \"x\"\n", Path::new("x")).is_err());
        // Wrong type.
        assert!(parse("schema = 1\n[[group]]\nid = 3\n", Path::new("x")).is_err());
    }

    #[test]
    fn entry_level_problems_drop_the_entry_and_keep_the_rest() {
        let src = r#"
schema = 1
[[group]]
id = "a"
parent = "b"
[[group]]
id = "b"
parent = "a"
[[group]]
id = "a"
[[group]]
id = "c"
parent = "missing"
[[relation]]
from = "X"
to = "X"
[[trail]]
id = "t"
stops = [{ decl = "X", group = "a" }, { group = "zzz" }, { decl = "X" }]
[[landmark]]
decl = "X"
groups = ["nope"]
"#;
        let m = parse(src, Path::new("x")).unwrap();
        let fields: Vec<&str> = m.issues.iter().map(|i| i.field.as_str()).collect();
        assert!(fields.contains(&"group[2].id"), "duplicate id: {fields:?}");
        assert!(fields.contains(&"group[3].parent"));
        assert!(fields.iter().any(|f| f.ends_with(".parent") && (*f == "group[0].parent" || *f == "group[1].parent")), "cycle: {fields:?}");
        assert!(fields.contains(&"relation[0]"));
        assert!(fields.contains(&"trail[0].stops[0]") && fields.contains(&"trail[0].stops[1].group"));
        assert!(fields.contains(&"landmark[0].groups[0]"));
        assert_eq!(m.groups.len(), 3);
        assert_eq!(m.trails[0].stops.len(), 1);
        // Every group is reachable from a root exactly once (the cycle was broken).
        let mut seen = 0;
        let mut stack = m.roots.clone();
        while let Some(k) = stack.pop() {
            seen += 1;
            stack.extend(&m.groups[k].children);
        }
        assert_eq!(seen, 3);
    }

    #[test]
    fn load_never_writes_and_distinguishes_absent_from_malformed() {
        let d = tempfile::tempdir().unwrap();
        assert!(matches!(load_for_root(d.path()), MapLoad::Absent { .. }));
        let p = d.path().join(FILE_NAME);
        let broken = "schema = 1\n[[group]\n";
        std::fs::write(&p, broken).unwrap();
        match load_for_root(d.path()) {
            MapLoad::Malformed { issues, .. } => assert!(!issues.is_empty()),
            other => panic!("{other:?}"),
        }
        assert_eq!(std::fs::read_to_string(&p).unwrap(), broken, "a malformed sidecar is left exactly as it was");
        std::fs::write(&p, GOOD).unwrap();
        assert!(load_for_root(d.path()).map().is_some());
        // A sidecar symlinked from outside the root is refused.
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("m.toml"), GOOD).unwrap();
        std::fs::remove_file(&p).unwrap();
        std::os::unix::fs::symlink(outside.path().join("m.toml"), &p).unwrap();
        assert!(matches!(load_for_root(d.path()), MapLoad::Malformed { .. }));
    }

    #[test]
    fn candidate_references_are_literal() {
        let doc = "This is Lemma 3.9 of the paper; see equation (3.16) and §2.1, also Theorems 4 and Section A.3.\nNot a ref: theorem foo, lemma about 2-groups.";
        let c = paper_candidates(doc);
        assert_eq!(c, ["Lemma 3.9", "equation (3.16)", "§2.1", "Theorems 4", "Section A.3"]);
        assert!(paper_candidates("def 3 := by simp; theorem x").is_empty());
    }

    #[test]
    fn search_spans_groups_objectives_and_landmarks() {
        let m = good();
        let h = m.search("first", 10);
        assert_eq!(h[0], MapHit { key: "grp:a".into(), label: "First".into(), what: "group" });
        let h = m.search("shared", 10);
        assert_eq!(h.iter().filter(|x| x.what == "landmark").count(), 3, "one hit per placement");
        assert!(m.search("the point", 10).iter().any(|x| x.what == "objective"));
        assert!(m.search("zzz", 10).is_empty());
    }

    #[test]
    fn skeleton_is_valid_and_labelled_generated() {
        let mods = vec![("Lib".to_string(), 0), ("Lib.Alg.Basic".to_string(), 5), ("Lib.Alg.More".to_string(), 7), ("Lib.Geo.«odd name»".to_string(), 2)];
        let s = skeleton_toml("lib", &mods);
        let m = parse(&s, Path::new("x")).unwrap();
        assert!(m.issues.is_empty(), "{:?}", m.issues);
        assert_eq!(m.groups.len(), 2);
        assert!(m.groups.iter().all(|g| g.origin == Provenance::Generated && !g.reviewed));
        assert_eq!(m.groups[0].modules, ["Lib.Alg.Basic", "Lib.Alg.More"]);
        // Two to forty subdirectories become nested groups; a lone one does not.
        let deep: Vec<(String, u64)> = ["A.X.P.m1", "A.X.Q.m2", "A.X.Q.m3", "A.Y.R.m4", "A.Y.top"].iter().map(|n| (n.to_string(), 1)).collect();
        let m = parse(&skeleton_toml("a", &deep), Path::new("x")).unwrap();
        assert!(m.issues.is_empty(), "{:?}", m.issues);
        let ids: Vec<(&str, Option<&str>)> = m.groups.iter().map(|g| (g.id.as_str(), g.parent.map(|p| m.groups[p].id.as_str()))).collect();
        assert_eq!(ids, [("a-x", None), ("a-x-p", Some("a-x")), ("a-x-q", Some("a-x")), ("a-y", None)]);
        assert!(m.objectives.is_empty(), "objectives are never inferred");
    }
}
