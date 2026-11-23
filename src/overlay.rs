//! Bringing other people's reading of a project into slim, and sending a
//! reader's own out.
//!
//! A project without documentation is documented by its readers. This
//! module converts a published reading model into a project map
//! (`lean-view.toml`), every entry marked `origin = "imported"` with its
//! source cited, and writes a reader's glosses as a small overlay file that
//! others can merge into theirs. slim reads local files only: fetching a
//! model is the reader's step.
//!
//! Supported input: the JSON model of the *riemann-rev* reading project
//! (`schema: "riemann-rev-model/1"`: strata, components, nodes, edges,
//! itineraries), and the `reading.json` of its proof atlas (nodes, edges
//! and itineraries only).

use crate::map::toml_str;
use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

fn text(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(|x| x.as_str()).map(|s| s.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|s| !s.is_empty())
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').chars().take(48).collect()
}

/// What an import did, for the person running it.
#[derive(Debug, Default, PartialEq)]
pub struct ImportReport {
    pub nodes: usize,
    pub resolved: usize,
    /// Resolved by the end of the name only (the model gave a short name).
    pub by_suffix: Vec<(String, String)>,
    pub unresolved: Vec<String>,
    pub groups: usize,
    pub relations: usize,
    pub trails: usize,
}

struct Node {
    id: String,
    title: Option<String>,
    contract: Option<String>,
    idea: Option<String>,
    component: Option<String>,
    kind: Option<String>,
    decl: Option<String>,
    assumptions: Vec<String>,
}

fn nodes_of(model: &Value) -> Vec<(String, &Value)> {
    match model.get("nodes") {
        Some(Value::Array(a)) => a.iter().filter_map(|n| Some((n.get("id")?.as_str()?.to_string(), n))).collect(),
        Some(Value::Object(o)) => o.iter().map(|(k, v)| (k.clone(), v)).collect(),
        _ => Vec::new(),
    }
}

/// The full name in the index of a declaration the model names: exactly,
/// or as the end of exactly one name.
fn resolve(conn: &Connection, name: &str) -> Result<Option<(String, bool)>> {
    if crate::db::query::decl_by_name(conn, name)?.is_some() {
        return Ok(Some((name.to_string(), false)));
    }
    let pat = format!("%.{}", name.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
    let mut st = conn.prepare_cached("SELECT DISTINCT name FROM decls WHERE name LIKE ?1 ESCAPE '\\' LIMIT 2")?;
    let hits: Vec<String> = st.query_map([pat], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(match hits.as_slice() {
        [one] => Some((one.clone(), true)),
        _ => None,
    })
}

/// A project map for `project` from a reading model. `origin_note`: where
/// the model file came from, as the reader wants it cited.
pub fn map_from_model(model: &Value, conn: &Connection, project: &str, origin_note: Option<&str>) -> Result<(String, ImportReport)> {
    let schema = model.get("schema").and_then(|s| s.as_str()).unwrap_or("");
    let raw_nodes = nodes_of(model);
    if raw_nodes.is_empty() {
        bail!("the file has no `nodes`: it is not a reading model slim knows (riemann-rev-model/1, or the atlas reading.json)");
    }
    if !schema.is_empty() && schema != "riemann-rev-model/1" {
        bail!("unknown model schema {schema:?} (known: riemann-rev-model/1)");
    }
    let mut rep = ImportReport::default();
    let mut nodes: Vec<Node> = Vec::new();
    for (id, n) in &raw_nodes {
        let named = text(n, "declaration").or_else(|| text(n, "name"));
        let decl = match &named {
            Some(name) => match resolve(conn, name)? {
                Some((full, suffix)) => {
                    rep.resolved += 1;
                    if suffix {
                        rep.by_suffix.push((name.clone(), full.clone()));
                    }
                    Some(full)
                }
                None => {
                    rep.unresolved.push(name.clone());
                    None
                }
            },
            None => None,
        };
        let assumptions = n.get("assumptions").and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).map(|s| s.to_string()).collect()).unwrap_or_default();
        nodes.push(Node { id: id.clone(), title: text(n, "title"), contract: text(n, "contract"), idea: text(n, "idea"), component: text(n, "component"), kind: text(n, "kind"), decl, assumptions });
    }
    rep.nodes = nodes.len();
    let by_id: HashMap<&str, &Node> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let pins = model.get("pins");
    let status = model.get("status");
    let as_of = status.and_then(|s| text(s, "as_of")).unwrap_or_default();
    let mirror = pins.and_then(|p| text(p, "mirror"));
    let commit = pins.and_then(|p| text(p, "mirror_commit"));
    let title = text(model, "title").unwrap_or_else(|| "Imported reading model".into());
    let mut s = String::new();
    s.push_str("# Project map written by `slim map import` from a third-party reading model.\n");
    s.push_str("# Every entry is marked `origin = \"imported\"`: it is what the cited source says,\n");
    s.push_str("# taken over as it stands and not reviewed by whoever keeps this file. It is a\n");
    s.push_str("# reading aid, not a statement about the mathematics, and slim checks none of it.\n");
    s.push_str("# Edit freely; slim never rewrites this file. Schema: docs/map.md.\n\n");
    s.push_str(&format!("schema = {}\n\n[project]\nname = {}\ntitle = {}\n", crate::map::SCHEMA_VERSION, toml_str(project), toml_str(&title)));
    let mut desc = String::from("Imported from the riemann-rev reading project (B. Naskręcki), an independent reading aid that is neither a proof, a referee report nor an official companion to the announcement.");
    if let Some(h) = status.and_then(|x| text(x, "headline")) {
        desc.push_str(&format!(" Its record of {as_of} says: \"{h}\" That sentence is the reading project's; slim has verified nothing."));
    }
    if let Some(n) = origin_note {
        desc.push_str(&format!(" Model file: {n}."));
    }
    s.push_str(&format!("description = {}\n", toml_str(&desc)));
    if let Some(c) = &commit {
        s.push_str(&format!("revision = {}\n", toml_str(c)));
    }
    s.push_str("\n[[source]]\nid = \"reading\"\n");
    s.push_str(&format!("citation = {}\nurl = \"https://nasqret.github.io/riemann-rev/\"\n", toml_str(&format!("B. Naskręcki, {title} (riemann-rev reading project{}); 0BSD licence", if as_of.is_empty() { String::new() } else { format!(", record of {as_of}") }))));
    if let (Some(m), Some(c)) = (&mirror, &commit) {
        s.push_str(&format!("\n[[source]]\nid = \"mirror\"\ncitation = {}\nurl = {}\n", toml_str(&format!("Public mirror of the Lean archive at commit {c}")), toml_str(&format!("{m}/tree/{c}/src"))));
    }
    // Objectives: what the model calls exports (or, without kinds, nothing).
    let mut objective: HashSet<&str> = HashSet::new();
    for n in nodes.iter().filter(|n| n.kind.as_deref() == Some("export") || n.id.starts_with("main-")) {
        let Some(d) = &n.decl else { continue };
        if !objective.insert(d.as_str()) {
            continue;
        }
        s.push_str(&format!("\n[[objective]]\ndecl = {}\n", toml_str(d)));
        if let Some(t) = &n.title {
            s.push_str(&format!("title = {}\n", toml_str(t)));
        }
        if let Some(c) = &n.contract {
            s.push_str(&format!("purpose = {}\n", toml_str(c)));
        }
        if let Some(i) = &n.idea {
            s.push_str(&format!("explanation = {}\n", toml_str(i)));
        }
        s.push_str(&format!("paper = [{{ text = {}, source = \"reading\" }}]\norigin = \"imported\"\n", toml_str(&format!("node {}", n.id))));
    }
    // Groups: strata, and the components below them.
    let tiers = model.get("tiers");
    let strata: Vec<&Value> = model.get("strata").and_then(|x| x.as_array()).map(|a| a.iter().collect()).unwrap_or_default();
    for (k, st) in strata.iter().enumerate() {
        let Some(id) = text(st, "id") else { continue };
        s.push_str(&format!("\n[[group]]\nid = {}\ntitle = {}\norder = {}\n", toml_str(&format!("stratum-{}", slug(&id))), toml_str(&text(st, "title").unwrap_or(id.clone())), (k + 1) * 10));
        if let Some(b) = text(st, "blurb") {
            s.push_str(&format!("purpose = {}\n", toml_str(&b)));
        }
        s.push_str("origin = \"imported\"\nreviewed = false\npaper = [{ text = \"stratum\", source = \"reading\" }]\n");
        rep.groups += 1;
    }
    let comps: Vec<&Value> = model.get("components").and_then(|x| x.as_array()).map(|a| a.iter().collect()).unwrap_or_default();
    for (k, c) in comps.iter().enumerate() {
        let Some(id) = text(c, "id") else { continue };
        let gid = slug(&id);
        let n = text(c, "n").map(|n| format!("{n} · ")).unwrap_or_default();
        s.push_str(&format!("\n[[group]]\nid = {}\ntitle = {}\n", toml_str(&gid), toml_str(&format!("{n}{}", text(c, "title").unwrap_or(id.clone())))));
        if let Some(st) = text(c, "stratum") {
            s.push_str(&format!("parent = {}\n", toml_str(&format!("stratum-{}", slug(&st)))));
        }
        s.push_str(&format!("order = {}\n", (k + 1) * 10));
        if let Some(g) = text(c, "gist") {
            s.push_str(&format!("purpose = {}\n", toml_str(&g)));
        }
        let mut ex: Vec<String> = Vec::new();
        if let Some(st) = text(c, "statement") {
            ex.push(format!("In symbols (LaTeX): {st}"));
        }
        for e in c.get("explanation").and_then(|x| x.as_array()).into_iter().flatten().filter_map(|x| x.as_str()) {
            ex.push(e.split_whitespace().collect::<Vec<_>>().join(" "));
        }
        if !ex.is_empty() {
            s.push_str(&format!("explanation = {}\n", toml_str(&ex.join("\n\n"))));
        }
        // What the reading project recorded about checking, in its words.
        let mut lim: Vec<String> = Vec::new();
        if let Some(t) = text(c, "tier") {
            let label = tiers.and_then(|ts| ts.get(&t)).and_then(|x| text(x, "label")).unwrap_or(t);
            lim.push(format!("Recorded by the reading project{}: {label}. {}", if as_of.is_empty() { String::new() } else { format!(" ({as_of})") }, text(c, "status").unwrap_or_default()));
        }
        if let Some(ch) = text(c, "check") {
            lim.push(ch);
        }
        if !lim.is_empty() {
            s.push_str(&format!("limitations = [{}]\n", lim.iter().map(|l| toml_str(l)).collect::<Vec<_>>().join(", ")));
        }
        let marks: Vec<String> = nodes
            .iter()
            .filter(|n| n.component.as_deref() == Some(id.as_str()))
            .filter_map(|n| Some(format!("{{ decl = {}, role = {} }}", toml_str(n.decl.as_ref()?), toml_str(n.title.as_deref().unwrap_or(&n.id)))))
            .collect();
        s.push_str(&format!("origin = \"imported\"\nreviewed = false\npaper = [{{ text = {}, source = \"reading\" }}]\nlandmarks = [\n  {}\n]\n", toml_str(&format!("component {id}")), marks.join(",\n  ")));
        rep.groups += 1;
    }
    // Landmarks: one annotation per declaration (the first node that names it).
    let mut seen: HashSet<&str> = HashSet::new();
    for n in &nodes {
        let Some(d) = &n.decl else { continue };
        if !seen.insert(d.as_str()) || objective.contains(d.as_str()) {
            continue;
        }
        s.push_str(&format!("\n[[landmark]]\ndecl = {}\n", toml_str(d)));
        let purpose = match (&n.title, &n.contract) {
            (Some(t), Some(c)) => Some(format!("{t}. {c}")),
            (t, c) => t.clone().or(c.clone()),
        };
        if let Some(p) = purpose {
            s.push_str(&format!("purpose = {}\n", toml_str(&p)));
        }
        let mut ex = n.idea.clone().unwrap_or_default();
        if !n.assumptions.is_empty() {
            ex.push_str(&format!("{}Assumptions it names: {}.", if ex.is_empty() { "" } else { "\n\n" }, n.assumptions.join("; ")));
        }
        if !ex.is_empty() {
            s.push_str(&format!("explanation = {}\n", toml_str(&ex)));
        }
        s.push_str(&format!("paper = [{{ text = {}, source = \"reading\" }}]\norigin = \"imported\"\n", toml_str(&format!("node {}", n.id))));
    }
    // Relations: the model's edges between declarations.
    let mut rel_seen: HashSet<(String, String)> = HashSet::new();
    for e in model.get("edges").and_then(|x| x.as_array()).into_iter().flatten() {
        let (Some(a), Some(b)) = (text(e, "from"), text(e, "to")) else { continue };
        let (Some(a), Some(b)) = (by_id.get(a.as_str()).and_then(|n| n.decl.clone()), by_id.get(b.as_str()).and_then(|n| n.decl.clone())) else { continue };
        if a == b || !rel_seen.insert((a.clone(), b.clone())) {
            continue;
        }
        s.push_str(&format!("\n[[relation]]\nfrom = {}\nto = {}\nkind = {}\n", toml_str(&a), toml_str(&b), toml_str(&text(e, "relation").unwrap_or_else(|| "uses".into()))));
        if let Some(w) = text(e, "witness").or_else(|| text(e, "token")) {
            s.push_str(&format!("note = {}\n", toml_str(&w)));
        }
        s.push_str("origin = \"imported\"\n");
        rep.relations += 1;
    }
    // An argument for the first objective: the model's first routes, in
    // their order, as steps a reader can unfold under the goal.
    let goal = nodes.iter().find(|n| n.decl.as_deref().is_some_and(|d| objective.contains(d))).and_then(|n| n.decl.clone());
    if let Some(goal) = goal {
        let mut steps: Vec<String> = Vec::new();
        let mut used: HashSet<&str> = HashSet::new();
        used.insert(goal.as_str());
        for it in model.get("itineraries").and_then(|x| x.as_array()).into_iter().flatten().take(3) {
            for k in it.get("nodes").and_then(|x| x.as_array()).into_iter().flatten().filter_map(|x| x.as_str()) {
                let Some(n) = by_id.get(k) else { continue };
                let Some(d) = n.decl.as_deref() else { continue };
                if !used.insert(d) {
                    continue;
                }
                let why = match (&n.title, &n.contract) {
                    (Some(t), Some(c)) => format!("{t}. {c}"),
                    (t, c) => t.clone().or(c.clone()).unwrap_or_default(),
                };
                steps.push(format!("{{ decl = {}, why = {} }}", toml_str(d), toml_str(&why)));
            }
        }
        if !steps.is_empty() {
            s.push_str(&format!(
                "\n[[argument]]\ngoal = {}\ntitle = \"From the endpoint backwards (the reading project's first routes)\"\nsummary = \"The steps of the reading project's first three routes, in its order. An explanation someone wrote, not a list of Lean dependencies.\"\norigin = \"imported\"\nsteps = [\n  {}\n]\n",
                toml_str(&goal),
                steps.join(",\n  ")
            ));
        }
    }
    // Trails: the model's itineraries, each with its question.
    let mut ids: BTreeMap<String, usize> = BTreeMap::new();
    for it in model.get("itineraries").and_then(|x| x.as_array()).into_iter().flatten() {
        let Some(t) = text(it, "title") else { continue };
        let mut id = slug(&t);
        let k = ids.entry(id.clone()).or_insert(0);
        *k += 1;
        if *k > 1 {
            id = format!("{id}-{k}");
        }
        let stops: Vec<String> = it
            .get("nodes")
            .and_then(|x| x.as_array())
            .into_iter()
            .flatten()
            .filter_map(|x| x.as_str())
            .filter_map(|k| by_id.get(k))
            .filter_map(|n| {
                let note = match (&n.title, &n.contract) {
                    (Some(t), Some(c)) => format!("{t}. {c}"),
                    (t, c) => t.clone().or(c.clone()).unwrap_or_default(),
                };
                Some(format!("{{ decl = {}, note = {} }}", toml_str(n.decl.as_ref()?), toml_str(&note)))
            })
            .collect();
        if stops.is_empty() {
            continue;
        }
        s.push_str(&format!("\n[[trail]]\nid = {}\ntitle = {}\n", toml_str(&id), toml_str(&t)));
        if let Some(q) = text(it, "question") {
            s.push_str(&format!("description = {}\n", toml_str(&format!("{q} (A route of the reading project, imported.)"))));
        }
        s.push_str(&format!("stops = [\n  {}\n]\n", stops.join(",\n  ")));
        rep.trails += 1;
    }
    Ok((s, rep))
}

/// Reads a model file (bounded) and converts it.
pub fn import_file(path: &std::path::Path, conn: &Connection, project: &str) -> Result<(String, ImportReport)> {
    let md = std::fs::metadata(path).with_context(|| format!("model file {}", path.display()))?;
    if md.len() > 64 << 20 {
        bail!("{} is too large to be a reading model", path.display());
    }
    let model: Value = serde_json::from_slice(&std::fs::read(path)?).with_context(|| format!("{} is not JSON", path.display()))?;
    let note = path.file_name().map(|f| f.to_string_lossy().to_string());
    map_from_model(&model, conn, project, note.as_deref())
}

/// Landmark candidates nobody chose: the declarations the reference graph
/// says a large part of the proof passes through, and the statements that
/// read with the libraries' vocabulary alone. As TOML to append to a map,
/// every entry `origin = "generated"`.
pub fn generated_landmarks(conn: &Connection, g: &crate::graph::Graph, cone: &crate::graph::Cone, top: usize) -> Result<String> {
    use crate::graph::nf;
    let mut s = String::new();
    let n = g.len() as u32;
    // Bottlenecks: the largest exclusive subtrees, relays folded into
    // the declaration they hand over to.
    let mut by: Vec<u32> = (0..n).filter(|&v| cone.contains(v) && cone.excl[v as usize] >= 50 && !g.has(v, nf::RELAY)).collect();
    by.sort_by_key(|&v| (std::cmp::Reverse(cone.excl[v as usize]), v));
    by.truncate(top);
    let rows = crate::reading::rows(conn, g, &by)?;
    s.push_str(&format!("\n# Bottlenecks of the reference graph ({}): the declarations through which the\n# largest parts of what the endpoints reach must pass. Generated; not chosen by anyone.\n", g.stats.label()));
    s.push_str("[[group]]\nid = \"generated-bottlenecks\"\ntitle = \"Bottlenecks (generated)\"\norder = 9000\norigin = \"generated\"\nreviewed = false\n");
    s.push_str("purpose = \"Declarations that a large part of the proof is reached only through, largest first. From the reference graph, not from a reader.\"\nlandmarks = [\n");
    for v in &by {
        if let Some(r) = rows.get(v) {
            s.push_str(&format!("  {{ decl = {}, role = {} }},\n", toml_str(&r.name), toml_str(&format!("{} declarations are reached only through it", crate::reading::thousands(cone.excl[*v as usize] as u64)))));
        }
    }
    s.push_str("]\n");
    // Statements in library vocabulary, by how much rests on them.
    let mut lib: Vec<u32> = (0..n).filter(|&v| cone.contains(v) && g.has(v, nf::LIB_ONLY) && !g.has(v, nf::RELAY) && g.users(v).len() >= 2).collect();
    lib.sort_by_key(|&v| (std::cmp::Reverse(g.users(v).len()), v));
    lib.truncate(top);
    let rows = crate::reading::rows(conn, g, &lib)?;
    s.push_str("\n# Theorems whose statement names nothing of the project: they read with the libraries'\n# vocabulary alone. Most used first. Generated.\n");
    s.push_str("[[group]]\nid = \"generated-library-statements\"\ntitle = \"Statements in library vocabulary (generated)\"\norder = 9010\norigin = \"generated\"\nreviewed = false\n");
    s.push_str("purpose = \"Theorems the endpoints reach whose statements use only library names: readable without learning the project's definitions.\"\nlandmarks = [\n");
    for v in &lib {
        if let Some(r) = rows.get(v) {
            s.push_str(&format!("  {{ decl = {}, role = {} }},\n", toml_str(&r.name), toml_str(&format!("referred to by {} declarations", g.users(*v).len()))));
        }
    }
    s.push_str("]\n");
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_ids_a_map_accepts() {
        assert_eq!(slug("Read the last page of the proof first"), "read-the-last-page-of-the-proof-first");
        assert_eq!(slug("  ·· A/B  "), "a-b");
    }
}

// ---------------------------------------------------------------------------
// A reader's own annotations, as text others can take in.

/// The reader's glosses and trails as map entries (`[[landmark]]` and
/// `[[trail]]`, `origin = "authored"`): to paste into a `lean-view.toml`
/// or to hand to another reader, who merges them with `slim gloss import`.
pub fn reader_overlay(project: &str, glosses: &[(String, String)], trails: &[crate::userdata::Trail]) -> String {
    let mut s = String::new();
    s.push_str("# A reader's glosses and trails, written by `slim gloss export`.\n");
    s.push_str("# The entries have the form of a project map (docs/map.md): paste them into a\n");
    s.push_str("# lean-view.toml, or take the glosses in with `slim gloss import <PROJECT_DIR> <FILE>`.\n\n");
    s.push_str(&format!("schema = {}\n\n[project]\nname = {}\n", crate::map::SCHEMA_VERSION, toml_str(project)));
    for (name, text) in glosses {
        s.push_str(&format!("\n[[landmark]]\ndecl = {}\npurpose = {}\norigin = \"authored\"\n", toml_str(name), toml_str(text)));
    }
    for t in trails {
        let stops: Vec<String> = t
            .stops
            .iter()
            .filter_map(|st| {
                let key = match st.ref_kind {
                    crate::userdata::RefKind::Group => "group",
                    _ => "decl",
                };
                let mut e = format!("{{ {key} = {}", toml_str(&st.name));
                if let Some(n) = st.note.as_deref().filter(|n| !n.is_empty()) {
                    e.push_str(&format!(", note = {}", toml_str(n)));
                }
                if let Some(q) = st.question.as_deref().filter(|q| !q.is_empty()) {
                    e.push_str(&format!(", question = {}", toml_str(q)));
                }
                e.push_str(" }");
                Some(e)
            })
            .collect();
        if stops.is_empty() {
            continue;
        }
        let id = format!("{}-{}", slug(&t.title), t.id);
        s.push_str(&format!("\n[[trail]]\nid = {}\ntitle = {}\n", toml_str(&id), toml_str(&t.title)));
        if let Some(d) = t.description.as_deref().filter(|d| !d.is_empty()) {
            s.push_str(&format!("description = {}\n", toml_str(d)));
        }
        s.push_str(&format!("stops = [\n  {}\n]\n", stops.join(",\n  ")));
    }
    s
}

/// `slim gloss export|import|list`.
pub fn cmd_gloss(args: &crate::cli::Args) -> Result<i32> {
    let sub = args.positional.first().map(|s| s.as_str()).unwrap_or("");
    if !matches!(sub, "export" | "import" | "list") {
        bail!("usage: slim gloss list <PROJECT_DIR> | slim gloss export <PROJECT_DIR> [-o FILE.toml] | slim gloss import <PROJECT_DIR> <FILE.toml> [--replace]");
    }
    let dir = args.positional.get(1).map(|s| s.as_str()).unwrap_or(".");
    let project = crate::paths::Project::open(std::path::Path::new(dir))?;
    let key = project.user_key();
    let store = crate::userdata::UserStore::open()?;
    let glosses = store.glosses(&key)?;
    match sub {
        "list" => {
            for (n, t) in &glosses {
                println!("{n}\t{t}");
            }
            if glosses.is_empty() {
                println!("no glosses for {}", project.name);
            }
        }
        "export" => {
            let text = reader_overlay(&project.name, &glosses, &store.trails(&key)?);
            match args.path("o").or(args.path("out")) {
                Some(p) => {
                    crate::userdata::write_atomic(&p, text.as_bytes())?;
                    eprintln!("wrote {} glosses and the trails of {} to {}", glosses.len(), project.name, p.display());
                }
                None => print!("{text}"),
            }
        }
        _ => {
            let file = args.positional.get(2).context("slim gloss import needs the file to read")?;
            let m = match crate::map::load(std::path::Path::new(file)) {
                crate::map::MapLoad::Loaded(m) => m,
                crate::map::MapLoad::Malformed { issues, .. } => bail!("{file} cannot be read as map entries: {}", issues.iter().map(|i| i.line()).collect::<Vec<_>>().join("; ")),
                crate::map::MapLoad::Absent { .. } => bail!("{file} does not exist"),
            };
            let have: HashMap<&str, &str> = glosses.iter().map(|(n, t)| (n.as_str(), t.as_str())).collect();
            let (mut added, mut kept, mut replaced) = (0, 0, 0);
            for l in &m.landmarks {
                let Some(text) = l.purpose.as_deref().filter(|t| !t.trim().is_empty()) else { continue };
                // A gloss is one line.
                let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
                match have.get(l.decl.as_str()) {
                    Some(mine) if *mine == text => kept += 1,
                    Some(_) if !args.has("replace") => kept += 1,
                    Some(_) => {
                        store.set_gloss(&key, &l.decl, Some(&text))?;
                        replaced += 1;
                    }
                    None => {
                        store.set_gloss(&key, &l.decl, Some(&text))?;
                        added += 1;
                    }
                }
            }
            println!("{added} glosses added, {replaced} replaced, {kept} of your own kept{}", if kept > 0 && !args.has("replace") { " (--replace takes the file's text instead)" } else { "" });
        }
    }
    Ok(0)
}
