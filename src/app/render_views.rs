//! Drawing of the architecture map, the goal explorer and reading trails:
//! an outline on the left and a detail pane on the right, or one pane at a
//! time on narrow terminals. Outlines render only the rows on screen.

use super::render::{age, block, chrome, clean, compact, evidence, heading, now_ns, outline_column, outline_metric, pad, statement_lines, wrapped};
use super::state::*;
use super::views::*;
use super::worker::SourceStatus;
use crate::db::query::{Direction, ModStats};
use crate::db::IndexMode;
use crate::imports::ImportGraph;
use crate::map::{MapLoad, PaperRef, Provenance, RefStatus};
use crate::model::{flags, AxiomCategory, DeclKind, Origin};
use crate::relations::RelMode;
use crate::text::{sanitize, truncate, truncate_left, width as text_width, wrap_line};
use crate::userdata::{RefKind, StopStatus};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

pub fn draw_view(f: &mut Frame, body: Rect, app: &mut App) {
    let single = body.width < SINGLE_PANE_BELOW;
    app.v.single_pane = single;
    app.hit.view_list = Rect::default();
    app.hit.view_detail = Rect::default();
    if single {
        match app.v.focus {
            VFocus::Outline => draw_outline(f, body, app),
            VFocus::Detail => draw_detail(f, body, app),
        }
    } else {
        let lw = (body.width as u32 * 42 / 100).clamp(34, 64) as u16;
        let [l, r] = Layout::horizontal([Constraint::Length(lw), Constraint::Min(30)]).areas(body);
        draw_outline(f, l, app);
        draw_detail(f, r, app);
    }
}

/// The line under the title bar: which view, and the pinned goal.
pub fn view_crumbs(app: &App, width: usize) -> Line<'static> {
    let t = &app.theme;
    let mut spans = vec![Span::styled(format!("{} ", app.v.view.label()), t.heading)];
    let goal = match &app.v.goal.goal {
        Some(g) => format!("{} goal: {}", app.sym.goal, clean(app, &g.display)),
        None => "goal: none".to_string(),
    };
    spans.push(Span::styled(format!("{} {goal} ", app.sym.sep), if app.v.goal.goal.is_some() { t.accent } else { t.dim }));
    // Secondary chrome goes first when space is short.
    if width >= 70 {
        let scope = match app.v.view {
            View::Map => match app.v.map.bundle.as_ref().map(|b| &b.load) {
                Some(MapLoad::Loaded(m)) => format!("map: {}", m.title.clone().or(m.project_name.clone()).unwrap_or_else(|| "lean-view.toml".into())),
                Some(MapLoad::Malformed { .. }) => "map: file has errors".into(),
                _ => "map: none (generated outline only)".into(),
            },
            View::Goal => format!("{} {} {}", app.v.goal.mode.short(), app.sym.sep, if app.v.goal.dir == Direction::Uses { "uses" } else { "used by" }),
            View::Trails => match app.v.trails.active().and_then(|id| app.v.trails.reading.trails.iter().find(|t| t.id == id)) {
                Some(tr) => format!("active trail: {}", tr.title),
                None => "no active trail".into(),
            },
            View::Imports => "from import lines: build order, not mathematics".into(),
            View::Browse => String::new(),
        };
        spans.push(Span::styled(format!("{} {} ", app.sym.sep, clean(app, &scope)), t.dim));
    }
    if width >= 96 {
        spans.push(Span::styled(format!("{} / search {} Esc declarations", app.sym.sep, app.sym.sep), t.dim));
    }
    Line::from(spans)
}

fn sel_style(app: &App, sel: bool, focused: bool, base: Style) -> Style {
    if !sel {
        base
    } else if focused {
        app.theme.selected
    } else {
        app.theme.selected_unfocused
    }
}

/// A row with left text and a right-aligned marker, cut to `w` cells.
fn row_line(app: &App, left: &str, right: &str, w: usize, style: Style, right_style: Style, sel: bool) -> Line<'static> {
    let right = if right.is_empty() { String::new() } else { format!(" {right}") };
    // The left text has priority; the marker keeps at least a third of the row.
    let rw = text_width(&right).min(w.saturating_sub(text_width(left)).max(w / 3));
    let right = truncate(&right, rw, app.ascii);
    let avail = w.saturating_sub(text_width(&right));
    Line::from(vec![Span::styled(pad(&truncate(left, avail, app.ascii), avail), style), Span::styled(right, if sel { style } else { right_style })])
}

fn short(name: &str) -> String {
    short_name(name)
}

fn draw_outline(f: &mut Frame, area: Rect, app: &mut App) {
    let t = app.theme.clone();
    let focused = app.v.focus == VFocus::Outline;
    let title = match app.v.view {
        View::Map => " Architecture map ".to_string(),
        View::Goal => " Goal explorer ".to_string(),
        View::Trails => " Reading trails ".to_string(),
        View::Imports => " Import layers ".to_string(),
        View::Browse => String::new(),
    };
    let b = block(app, Line::from(Span::styled(title, t.title)), focused);
    let inner = b.inner(area);
    f.render_widget(b, area);
    if inner.height == 0 || inner.width < 6 {
        return;
    }
    let w = inner.width as usize;
    let mut list = inner;
    // The goal explorer keeps its relation kind, direction and filters in view.
    if app.v.view == View::Goal && inner.height > 6 {
        let [head, rest] = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(inner);
        list = rest;
        let g = &app.v.goal;
        let l1 = format!("relation: {}", g.mode.label());
        let l2 = if g.dir == Direction::Uses { "direction: uses (A lists what A needs)" } else { "direction: used by (what needs A)" };
        let mut fl = Vec::new();
        if g.reveal_hidden {
            fl.push("everything".to_string());
        } else {
            fl.push(if app.filters.local_only { "local only".into() } else { "local+ext".to_string() });
            fl.push(if app.filters.show_generated { "+generated".into() } else { "generated off".to_string() });
            if matches!(g.mode, RelMode::Lean | RelMode::Mixed) {
                fl.push(match g.tb {
                    crate::relations::TypeBody::All => "T+V+C",
                    crate::relations::TypeBody::Type => "T only",
                    crate::relations::TypeBody::Body => "V only",
                }
                .to_string());
            }
        }
        let l3 = format!("show: {}", fl.join(&format!(" {} ", app.sym.sep)));
        let unavailable = g.mode == RelMode::Lean && app.ctx.mode == IndexMode::Source;
        f.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(truncate(&chrome(app, &l1), w, app.ascii), if unavailable { t.warn } else { t.accent })),
                Line::from(Span::styled(truncate(l2, w, app.ascii), t.dim)),
                Line::from(Span::styled(truncate(&chrome(app, &l3), w, app.ascii), t.dim)),
            ]),
            head,
        );
    }
    app.hit.view_list = list;
    let h = list.height as usize;
    app.v.list_height = h;
    let lines: Vec<Line> = match app.v.view {
        View::Map => {
            let n = app.v.map.rows.len();
            app.v.map.list.clamp(n, h);
            let (sel, scroll) = (app.v.map.list.selected, app.v.map.list.scroll);
            (scroll..n.min(scroll + h)).map(|i| map_row_line(app, i, w, i == sel, focused)).collect()
        }
        View::Goal => {
            let n = app.v.goal.rows.len();
            app.v.goal.list.clamp(n, h);
            let (sel, scroll) = (app.v.goal.list.selected, app.v.goal.list.scroll);
            (scroll..n.min(scroll + h)).map(|i| goal_row_line(app, i, w, i == sel, focused)).collect()
        }
        View::Trails => {
            let n = app.v.trails.rows.len();
            app.v.trails.list.clamp(n, h);
            let (sel, scroll) = (app.v.trails.list.selected, app.v.trails.list.scroll);
            (scroll..n.min(scroll + h)).map(|i| trail_row_line(app, i, w, i == sel, focused)).collect()
        }
        View::Imports => {
            let n = app.v.imports.rows.len();
            app.v.imports.list.clamp(n, h);
            let (sel, scroll) = (app.v.imports.list.selected, app.v.imports.list.scroll);
            (scroll..n.min(scroll + h)).map(|i| import_row_line(app, i, w, i == sel, focused)).collect()
        }
        View::Browse => Vec::new(),
    };
    f.render_widget(Paragraph::new(lines), list);
}

fn decl_markers(app: &App, name: &str) -> (String, Style) {
    let t = &app.theme;
    let Some(b) = &app.v.map.bundle else { return (String::new(), t.dim) };
    let mut v: Vec<String> = Vec::new();
    let mut style = t.dim;
    match b.resolution.refs.get(name).map(|i| &i.status) {
        Some(RefStatus::Missing { .. }) => {
            v.push("MISSING".into());
            style = t.warn;
        }
        Some(RefStatus::Ambiguous { count }) => {
            v.push(format!("ambiguous x{count}"));
            style = t.warn;
        }
        _ => {}
    }
    if b.resolution.is_external(name) {
        v.push("lib".into());
    }
    if let Some(m) = b.load.map() {
        if m.landmark(name).is_some_and(|l| b.resolution.is_stale(l)) {
            v.push("STALE".into());
            style = t.warn;
        }
        if let Some(l) = m.landmark(name) {
            if l.origin != Provenance::Authored {
                v.push(format!("[{}]", l.origin.tag()));
            }
        }
    }
    if app.v.goal.goal.as_ref().is_some_and(|g| g.name == name) {
        v.push(app.sym.goal.to_string());
    }
    if app.bookmarked.contains(name) {
        v.push(app.sym.bookmark.to_string());
    }
    (v.join(" "), style)
}

fn kind_tag(app: &App, name: &str) -> &'static str {
    app.v.map.bundle.as_ref().and_then(|b| b.resolution.row(name)).map(|r| r.kind.short().trim_end()).unwrap_or("?")
}

fn map_row_line(app: &App, i: usize, w: usize, sel: bool, focused: bool) -> Line<'static> {
    let t = &app.theme;
    let s = &app.sym;
    let row = &app.v.map.rows[i];
    let ind = "  ".repeat(row.depth as usize);
    let mark = |e: bool| if e { s.expanded } else { s.collapsed };
    match &row.kind {
        MapRowKind::Section { title, count, expanded } => {
            let st = sel_style(app, sel, focused, t.heading);
            row_line(app, &format!("{} {}", mark(*expanded), chrome(app, title)), &chrome(app, count), w, st, t.dim, sel)
        }
        MapRowKind::Objective { decl, title } => {
            let label = title.clone().unwrap_or_else(|| short(decl));
            let (m, ms) = decl_markers(app, decl);
            let left = format!("{ind}{} {} {}", s.objective, kind_tag(app, decl), clean(app, &label));
            row_line(app, &left, &m, w, sel_style(app, sel, focused, t.text), ms, sel)
        }
        MapRowKind::Group { idx, expanded } => {
            let Some(m) = app.map() else { return Line::from("") };
            let g = &m.groups[*idx];
            let mut right = format!("{} landmarks", m.group_landmark_count(*idx));
            // Generated or unreviewed organization is said so on the row.
            if g.origin != Provenance::Authored {
                right.push_str(&format!(" [{}]", g.origin.tag()));
            } else if !g.reviewed {
                right.push_str(" [unreviewed]");
            }
            let left = format!("{ind}{} {}", mark(*expanded), clean(app, &g.title));
            row_line(app, &left, &right, w, sel_style(app, sel, focused, t.accent), if g.origin != Provenance::Authored || !g.reviewed { t.warn } else { t.dim }, sel)
        }
        MapRowKind::Landmark { decl, .. } => {
            let (m, ms) = decl_markers(app, decl);
            let left = format!("{ind}  {} {}", kind_tag(app, decl), clean(app, &short(decl)));
            row_line(app, &left, &m, w, sel_style(app, sel, focused, t.text), ms, sel)
        }
        MapRowKind::Unresolved(u) => {
            let what = if u.why.starts_with("ambiguous") { "ambiguous" } else { "missing" };
            row_line(app, &format!("{ind}{} {}", s.question, clean(app, &u.name)), what, w, sel_style(app, sel, focused, t.warn), t.warn, sel)
        }
        MapRowKind::Issue { text, error } => {
            row_line(app, &format!("{ind}{} {}", s.warn, clean(app, text)), "", w, sel_style(app, sel, focused, if *error { t.danger } else { t.warn }), t.dim, sel)
        }
        MapRowKind::Module { node, expanded } => {
            let Some(n) = app.outline.nodes.get(*node) else { return Line::from("") };
            let mk = if n.children.is_empty() { " " } else { mark(*expanded) };
            let warn = if n.roll.partial > 0 && n.module.is_some() && n.children.is_empty() { format!(" {}", s.warn) } else { String::new() };
            row_line(app, &format!("{ind}{mk} {}{warn}", clean(app, &n.label)), &outline_metric(app, n, now_ns()), w, sel_style(app, sel, focused, t.text), t.dim, sel)
        }
        MapRowKind::Note(text) => row_line(app, &format!("{ind}{}", chrome(app, &clean(app, text))), "", w, sel_style(app, sel, focused, t.dim), t.dim, sel),
    }
}

/// `1 file`, `3 files`.
fn count(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn depth_range(lo: u32, hi: u32) -> String {
    if lo == hi {
        format!("depth {lo}")
    } else {
        format!("depth {lo}–{hi}")
    }
}

fn import_row_line(app: &App, i: usize, w: usize, sel: bool, focused: bool) -> Line<'static> {
    let t = &app.theme;
    let s = &app.sym;
    let iv = &app.v.imports;
    let row = &iv.rows[i];
    let ind = "  ".repeat(row.depth as usize);
    let mark = |e: bool| if e { s.expanded } else { s.collapsed };
    let plain = |text: &str, style: Style| row_line(app, &format!("{ind}{}", chrome(app, &clean(app, text))), "", w, sel_style(app, sel, focused, style), t.dim, sel);
    // Module names are cut on the left: the end of a dotted name says most.
    let named = |lead: &str, name: &str, right: &str| -> String {
        let room = w.saturating_sub(text_width(lead) + text_width(right) + 1).max(8);
        format!("{lead}{}", truncate_left(&clean(app, name), room, app.ascii))
    };
    match (&row.kind, &iv.graph) {
        (IRowKind::Section { title, count, expanded }, _) => {
            row_line(app, &format!("{} {}", mark(*expanded), chrome(app, title)), &chrome(app, count), w, sel_style(app, sel, focused, t.heading), t.dim, sel)
        }
        (IRowKind::Note { text, warn }, _) => plain(text, if *warn { t.warn } else { t.dim }),
        (IRowKind::Dir { dir, expanded }, Some(g)) => {
            let n = &g.dirs[*dir as usize];
            let file = n.children.is_empty();
            let right = if file {
                format!("depth {} {} {} decl", n.dmax, s.sep, n.decls)
            } else {
                format!("{} {} {}", depth_range(n.dmin, n.dmax), s.sep, count(n.files as u64, "file", "files"))
            };
            let mk = if file { " " } else { mark(*expanded) };
            row_line(app, &format!("{ind}{mk} {}", clean(app, &n.label)), &chrome(app, &right), w, sel_style(app, sel, focused, if file { t.text } else { t.accent }), t.dim, sel)
        }
        (IRowKind::Module { node, what }, Some(g)) => {
            let v = *node as usize;
            let right = match what {
                IList::Endpoint => format!("depth {} {} {} decl", g.depth[v], s.sep, g.nodes[v].decls),
                IList::Hub => format!("imported by {}", g.local_importers[v]),
                IList::ImportOnly => format!("imports {}", g.imports_of(*node).len()),
            };
            let right = chrome(app, &right);
            row_line(app, &named(&format!("{ind}  "), &g.nodes[v].name, &right), &right, w, sel_style(app, sel, focused, t.text), t.dim, sel)
        }
        (IRowKind::PathStep { name, step }, Some(g)) => {
            let last = matches!(&iv.path, Some(crate::imports::PathAnswer::Path(p)) if step + 1 == p.len());
            let ext = g.node(name).is_some_and(|n| !g.nodes[n as usize].local);
            let right = match (last, ext) {
                (true, true) => "external".to_string(),
                (true, false) => String::new(),
                (false, _) => format!("imports {}", s.arrow),
            };
            row_line(app, &named(&format!("{ind}{}. ", step + 1), name, &right), &right, w, sel_style(app, sel, focused, t.text), t.accent, sel)
        }
        _ => Line::from(""),
    }
}

/// The counts of a file or of everything below a directory, spelled out.
fn rollup_lines(o: &mut Out, st: &ModStats, decls: u64) {
    let app = o.app;
    let t = &app.theme;
    let source = app.ctx.mode == IndexMode::Source;
    o.head("Counts");
    o.text(&format!("{} · {} indexed declarations{}", count(st.files as u64, "file", "files"), decls, if source { " (lexical candidates)" } else { "" }), t.text);
    o.text(&format!("theorems {} · definitions {} · types {} · instances {} · axioms {}", st.thm, st.def, st.typ, st.inst, st.axiom), t.text);
    let named = st.named();
    if named > 0 {
        o.text(&format!("documented: {} of {named} ({}%)", st.doc, st.doc as u64 * 100 / named as u64), t.text);
    }
    if st.lines > 0 {
        let share = if st.proof_lines > 0 { format!(" · {} in theorem proofs ({}%)", compact(st.proof_lines as u64), st.proof_lines as u64 * 100 / st.lines.max(1) as u64) } else { String::new() };
        o.text(&format!("{} lines{share}", compact(st.lines as u64)), t.text);
    }
    if source {
        if st.sorry > 0 {
            o.text(&format!("`sorry`/`admit` token in {} declaration{} (lexical)", st.sorry, if st.sorry == 1 { "" } else { "s" }), t.warn);
        } else {
            o.text("no `sorry` token (proves nothing)", t.dim);
        }
    } else if st.sorry > 0 {
        o.text(&format!("{} declaration{} depend on sorryAx", st.sorry, if st.sorry == 1 { "" } else { "s" }), t.warn);
    } else {
        o.text("no declaration here depends on sorryAx", t.dim);
    }
    if st.partial > 0 {
        o.text(&format!("{} file{} partially parsed", st.partial, if st.partial == 1 { "" } else { "s" }), t.warn);
    }
    if st.mtime_ns > 0 {
        o.text(&format!("last modified {} ago (as of the {})", age(now_ns(), st.mtime_ns), if source { "index" } else { "snapshot" }), t.dim);
    }
}

/// One file of the import graph: its depth with a witness chain, what it
/// imports and what imports it.
fn import_module_lines(o: &mut Out, g: &ImportGraph, v: u32) {
    let app = o.app;
    let t = &app.theme;
    let n = &g.nodes[v as usize];
    let name = |i: u32| g.nodes[i as usize].name.clone();
    if n.local {
        let d = g.depth[v as usize];
        o.head(&format!("Depth {d}"));
        if g.cyclic[v as usize] {
            o.text("on or above an import cycle: a lower bound", t.warn);
        }
        if d == 0 {
            o.text("imports no file of this project", t.text);
        } else {
            o.text("a longest chain of local imports below it:", t.dim);
            let chain = g.chain_below(v);
            o.text(&chain.iter().map(|&i| name(i)).collect::<Vec<_>>().join(&format!(" {} ", app.sym.arrow)), t.text);
        }
    } else {
        o.text("an external module: not a file of this project", t.dim);
    }
    let list = |o: &mut Out, title: &str, ids: &[u32], show_depth: bool| {
        o.head(&format!("{title} ({})", ids.len()));
        let mut ids: Vec<u32> = ids.to_vec();
        // Local files first, deepest first; then external modules by name.
        ids.sort_by_key(|&i| (!g.nodes[i as usize].local, std::cmp::Reverse(g.depth[i as usize]), i));
        for &i in ids.iter().take(40) {
            let m = &g.nodes[i as usize];
            let tag = if !m.local { " (external)".to_string() } else if show_depth { format!(" — depth {}", g.depth[i as usize]) } else { String::new() };
            o.item(app.sym.bullet, format!("{}{tag}", m.name), if m.local { t.text } else { t.dim });
        }
        if ids.len() > 40 {
            o.text(&format!("… {} more", ids.len() - 40), t.dim);
        }
        if ids.is_empty() {
            o.text("none", t.dim);
        }
    };
    list(o, "Imports", g.imports_of(v), true);
    list(o, "Imported by", g.importers_of(v), true);
}

fn label_tags(labels: &[crate::relations::EdgeLabel]) -> String {
    if labels.is_empty() {
        String::new()
    } else {
        format!("[{}] ", labels.iter().map(|l| l.tag()).collect::<Vec<_>>().join(" "))
    }
}

fn goal_row_line(app: &App, i: usize, w: usize, sel: bool, focused: bool) -> Line<'static> {
    let t = &app.theme;
    let s = &app.sym;
    let g = &app.v.goal;
    match &g.rows[i] {
        GRow::Node(k) => {
            let n = &g.nodes[*k];
            let ind = "  ".repeat(n.depth as usize);
            let display = n.row.as_ref().map(|r| r.display.clone()).unwrap_or_else(|| n.name.clone());
            let kind = n.row.as_ref().map(|r| r.kind.short().trim_end()).unwrap_or("?");
            let mut right: Vec<String> = Vec::new();
            let mut rs = t.dim;
            if n.backref {
                right.push(s.cycle.to_string());
                rs = t.warn;
            } else if n.repeat {
                right.push(s.link.to_string());
            }
            match &n.row {
                None if n.depth > 0 => {
                    right.push("not indexed".into());
                    rs = t.warn;
                }
                Some(r) if r.origin == Origin::Library => right.push("lib".into()),
                Some(r) if r.origin == Origin::Unresolved => right.push("unres".into()),
                _ => {}
            }
            if n.loading {
                right.push(s.ellipsis.to_string());
            }
            let left = if n.depth == 0 {
                let display = g.goal.as_ref().map(|x| x.display.clone()).unwrap_or(display);
                format!("{} {} {}", if n.expanded { s.expanded } else { s.collapsed }, s.goal, clean(app, &display))
            } else {
                let mark = if n.backref { " " } else if n.expanded { s.expanded } else { s.collapsed };
                format!("{ind}{mark} {}{kind} {}", label_tags(&n.labels), clean(app, &short(&display)))
            };
            row_line(app, &left, &right.join(" "), w, sel_style(app, sel, focused, if n.depth == 0 { t.decl_name } else { t.text }), rs, sel)
        }
        GRow::More { node, remaining, capped, .. } => {
            let ind = "  ".repeat(g.nodes[*node].depth as usize + 1);
            let text = format!("{ind}{} {}{} more Lean edges (⏎ loads)", s.ellipsis, if *capped { "≥" } else { "" }, remaining);
            row_line(app, &chrome(app, &text), "", w, sel_style(app, sel, focused, t.accent), t.dim, sel)
        }
        GRow::Note { node, text, warn } => {
            let ind = "  ".repeat(g.nodes[*node].depth as usize + 1);
            row_line(app, &format!("{ind}{}", chrome(app, &clean(app, text))), "", w, sel_style(app, sel, focused, if *warn { t.warn } else { t.dim }), t.dim, sel)
        }
        GRow::ConnHeader { text, warn } => row_line(app, &chrome(app, &clean(app, text)), "", w, sel_style(app, sel, focused, if *warn { t.warn } else { t.heading }), t.dim, sel),
        GRow::ConnStep(k) => {
            let step = match g.conn.as_ref().and_then(|c| c.result.as_ref()).map(|r| &r.outcome) {
                Some(crate::relations::ConnOutcome::Path(p)) => p.get(*k),
                _ => None,
            };
            let Some(st) = step else { return Line::from("") };
            let right = st.next.as_ref().map(|l| format!("[{}] {}", l.tag(), s.arrow)).unwrap_or_else(|| "end".into());
            let kind = st.row.as_ref().map(|r| r.kind.short().trim_end()).unwrap_or("?");
            row_line(app, &format!("  {}. {kind} {}", k + 1, clean(app, &short(&st.name))), &right, w, sel_style(app, sel, focused, t.text), t.accent, sel)
        }
        GRow::Pick(k) => {
            let label = app.map().and_then(|m| m.objectives.get(*k)).map(|o| o.title.clone().unwrap_or_else(|| short(&o.decl))).unwrap_or_default();
            row_line(app, &format!("  {} {}", s.objective, clean(app, &label)), &chrome(app, "⏎ pin"), w, sel_style(app, sel, focused, t.text), t.dim, sel)
        }
        GRow::Info(text) => row_line(app, &chrome(app, &clean(app, text)), "", w, sel_style(app, sel, focused, t.dim), t.dim, sel),
    }
}

fn trail_row_line(app: &App, i: usize, w: usize, sel: bool, focused: bool) -> Line<'static> {
    let t = &app.theme;
    let s = &app.sym;
    let v = &app.v.trails;
    let rd = &v.reading;
    match &v.rows[i] {
        TRow::Header(text) => row_line(app, &chrome(app, text), "", w, sel_style(app, sel, focused, t.heading), t.dim, sel),
        TRow::Note(text) => row_line(app, &format!("    {}", chrome(app, text)), "", w, sel_style(app, sel, focused, t.dim), t.dim, sel),
        TRow::Trail(k) => {
            let tr = &rd.trails[*k];
            let exp = v.expanded.contains(&tr.id);
            let active = if v.active() == Some(tr.id) { " (active)" } else { "" };
            let shared = if tr.origin.starts_with("shared:") { " [from map]" } else { "" };
            let bad = tr.stops.iter().filter(|x| matches!(rd.statuses.get(&x.id), Some(StopStatus::Stale(_)) | Some(StopStatus::Unresolved(_)))).count();
            let mut right = format!("{}/{} read", tr.done(), tr.stops.len());
            if bad > 0 {
                right.push_str(&format!(" {} {bad} stale", s.warn));
            }
            let left = format!("{} {}{active}{shared}", if exp { s.expanded } else { s.collapsed }, clean(app, &tr.title));
            row_line(app, &left, &right, w, sel_style(app, sel, focused, t.accent), if bad > 0 { t.warn } else { t.dim }, sel)
        }
        TRow::Stop { trail, stop } => {
            let st = &rd.trails[*trail].stops[*stop];
            let (right, rs) = match rd.statuses.get(&st.id) {
                Some(StopStatus::Stale(_)) => ("STALE".to_string(), t.warn),
                Some(StopStatus::Unresolved(_)) => ("UNRESOLVED".to_string(), t.warn),
                _ => (String::new(), t.dim),
            };
            let kind = if st.ref_kind == RefKind::Group { "group".to_string() } else { DeclKind::parse(&st.kind).map(|k| k.short().trim_end().to_string()).unwrap_or_else(|| st.kind.clone()) };
            let q = if st.question.is_some() { format!(" {}", s.question) } else { String::new() };
            let note = st.note.as_deref().and_then(|n| n.lines().next()).map(|n| format!(" — {n}")).unwrap_or_default();
            let left = format!("   {:>2}. {} {kind} {}{q}{}", stop + 1, st.state.tag(), clean(app, &short(&st.display)), chrome(app, &clean(app, &note)));
            row_line(app, &left, &right, w, sel_style(app, sel, focused, t.text), rs, sel)
        }
        TRow::Shared(k) => {
            let (title, n) = app.map().and_then(|m| m.trails.get(*k)).map(|x| (x.title.clone(), x.stops.len())).unwrap_or_default();
            row_line(app, &format!("  + {}", clean(app, &title)), &format!("{n} stops, not started"), w, sel_style(app, sel, focused, t.text), t.dim, sel)
        }
        TRow::Question(k) => {
            let q = &rd.questions[*k];
            let left = format!("  {} {}: {}", if q.resolved { s.ok } else { s.question }, clean(app, &short(&q.display)), clean(app, &q.text));
            row_line(app, &left, if q.resolved { "resolved" } else { "open" }, w, sel_style(app, sel, focused, if q.resolved { t.dim } else { t.text }), t.dim, sel)
        }
    }
}

// ---------------------------------------------------------------------------
// Detail pane
// ---------------------------------------------------------------------------

/// Cuts a styled line to the cells `[off, off + w)`.
fn hslice(line: &Line<'static>, off: usize, w: usize) -> Line<'static> {
    let mut out: Vec<Span<'static>> = Vec::new();
    let (mut pos, end) = (0usize, off + w);
    for sp in &line.spans {
        let mut s = String::new();
        for c in sp.content.chars() {
            let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            if pos >= off && pos + cw <= end {
                s.push(c);
            }
            pos += cw;
        }
        if !s.is_empty() {
            out.push(Span::styled(s, sp.style));
        }
    }
    Line::from(out)
}

fn draw_detail(f: &mut Frame, area: Rect, app: &mut App) {
    let t = app.theme.clone();
    let focused = app.v.focus == VFocus::Detail;
    let b = block(app, Line::from(Span::styled(" Detail ", t.title)), focused);
    let inner = b.inner(area);
    f.render_widget(b, area);
    app.hit.view_detail = area;
    if inner.width < 10 || inner.height == 0 {
        return;
    }
    app.v.detail.height = inner.height as usize;
    let key = (inner.width, app.v.detail.version);
    if app.v.detail.cache.as_ref().is_none_or(|(w, v, _)| (*w, *v) != key) {
        let w = if app.v.detail.wrap { inner.width as usize } else { 4000 };
        let lines = detail_lines(app, w);
        app.v.detail.cache = Some((inner.width, app.v.detail.version, lines));
    }
    let lines = &app.v.detail.cache.as_ref().unwrap().2;
    let max_scroll = lines.len().saturating_sub(inner.height as usize);
    app.v.detail.scroll = app.v.detail.scroll.min(max_scroll);
    let (wrap, off) = (app.v.detail.wrap, app.v.detail.hscroll);
    let visible: Vec<Line> = lines
        .iter()
        .skip(app.v.detail.scroll)
        .take(inner.height as usize)
        .map(|l| if wrap { l.clone() } else { hslice(l, off, inner.width as usize) })
        .collect();
    let total = lines.len();
    f.render_widget(Paragraph::new(visible), inner);
    if (total > inner.height as usize || !wrap) && inner.height > 2 {
        let pct = (app.v.detail.scroll + inner.height as usize) * 100 / total.max(1);
        let ind = if wrap { format!(" {}% ", pct.min(100)) } else { format!(" {}% col {} ", pct.min(100), off + 1) };
        let iw = ind.len() as u16;
        if area.width > iw + 4 {
            f.render_widget(Paragraph::new(Span::styled(ind, t.dim)), Rect { x: area.x + area.width - iw - 2, y: area.y + area.height - 1, width: iw, height: 1 });
        }
    }
}

struct Out<'a> {
    app: &'a App,
    w: usize,
    lines: Vec<Line<'static>>,
}

impl Out<'_> {
    fn head(&mut self, s: &str) {
        if !self.lines.is_empty() {
            self.lines.push(Line::from(""));
        }
        self.lines.push(heading(self.app, s, self.w.min(120)));
    }
    fn text(&mut self, s: &str, style: Style) {
        self.lines.extend(wrapped(self.app, &chrome(self.app, s), self.w, style, 1));
    }
    fn para(&mut self, s: &str, style: Style) {
        // Authored prose: paragraphs are reflowed, never altered otherwise.
        for p in super::render::reflow_doc(s.trim()) {
            for r in wrap_line(&sanitize(&format!(" {p}"), self.app.ascii), self.w, 1) {
                self.lines.push(Line::from(Span::styled(r, style)));
            }
        }
    }
    fn item(&mut self, sym: &str, s: String, style: Style) {
        self.lines.extend(evidence(self.app, sym, s, style, self.w));
    }
    fn papers(&mut self, refs: &[PaperRef]) {
        let app = self.app;
        let t = &app.theme;
        for p in refs {
            let src = p.source.as_ref().and_then(|id| app.map().and_then(|m| m.sources.iter().find(|s| &s.id == id).cloned()));
            let s = match src {
                Some(s) => format!("{} — {} (source \"{}\", supplied by the map's author)", p.text, s.citation, s.id),
                None => format!("\"{}\" — literal text; no identified source was supplied", p.text),
            };
            self.item(app.sym.bullet, s, t.text);
            if let Some(n) = &p.note {
                self.text(&format!("   {n}"), t.dim);
            }
        }
    }
}

pub fn detail_lines(app: &App, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let mut o = Out { app, w, lines: Vec::new() };
    match app.subject() {
        Subject::None => o.text("Nothing selected.", t.dim),
        Subject::Section(title) => section_lines(&mut o, &title),
        Subject::Issue(text) => {
            o.head("Map file");
            o.text(&text, t.warn);
            o.text("lean-view.toml is read, never rewritten", t.dim);
        }
        Subject::Group(k) => group_lines(&mut o, k),
        Subject::Unresolved(u) => {
            o.lines.push(Line::from(vec![Span::styled("unresolved reference ", t.warn), Span::styled(clean(app, &u.name), t.decl_name)]));
            o.text(&u.why, t.warn);
            o.head("Used in the map");
            for x in &u.used_in {
                o.item(app.sym.bullet, x.clone(), t.text);
            }
            if !u.suggestions.is_empty() {
                o.head("Declarations with the same last name component");
                for x in &u.suggestions {
                    o.item(app.sym.bullet, x.clone(), t.text);
                }
                o.text("hints only: nothing is re-attached automatically", t.dim);
            }
        }
        Subject::Module(i) => {
            let Some(n) = app.outline.nodes.get(i) else { return o.lines };
            o.lines.push(Line::from(vec![Span::styled(if n.module.is_some() { "module " } else { "directory / namespace prefix " }, t.kind), Span::styled(clean(app, &n.full), t.decl_name)]));
            o.text("generated from the file tree, not curated", t.dim);
            if let Some(m) = &n.module {
                if let Some(p) = &m.path {
                    o.text(&format!("file: {p}"), t.dim);
                }
            }
            rollup_lines(&mut o, &n.roll, n.decls_total);
        }
        Subject::ImportDir(d) => {
            let Some(g) = app.v.imports.graph.clone() else { return o.lines };
            let Some(n) = g.dirs.get(d as usize) else { return o.lines };
            let is_dir = !n.children.is_empty();
            o.lines.push(Line::from(vec![Span::styled(if is_dir { "directory / namespace prefix " } else { "file " }, t.kind), Span::styled(clean(app, &n.full), t.decl_name)]));
            if is_dir {
                o.text(&format!("{} · {} indexed declarations below", count(n.files as u64, "file", "files"), n.decls), t.text);
                o.text(&format!("{} over its declaring files", depth_range(n.dmin, n.dmax)), t.text);
                match app.v.imports.dir.as_ref().filter(|s| s.full == n.full) {
                    None => o.text("reading its imports…", t.dim),
                    Some(s) => {
                        let part = |o: &mut Out, title: String, rows: &[(String, u32)], none: &str| {
                            o.head(&title);
                            for (name, k) in rows {
                                o.item(app.sym.bullet, format!("{name}  ({k})"), t.text);
                            }
                            if rows.is_empty() {
                                o.text(none, t.dim);
                            }
                        };
                        let lines = |n: u32| format!("{n} import line{}", if n == 1 { "" } else { "s" });
                        part(&mut o, format!("Imports from the rest of the project ({})", lines(s.out_total)), &s.imports_from, "nothing outside itself");
                        part(&mut o, format!("Imported by ({})", lines(s.in_total)), &s.imported_by, "no file outside it imports a file below it");
                        part(&mut o, "External modules imported, by first name component".into(), &s.external, "none");
                        o.text(&format!("{} inside it · grouped by sibling directory, most used first", lines(s.inside)), t.dim);
                    }
                }
            }
            if let Some(m) = n.module {
                if is_dir {
                    o.head(&format!("The file {}", n.full));
                }
                import_module_lines(&mut o, &g, m);
            }
        }
        Subject::ImportModule(v) => {
            let Some(g) = app.v.imports.graph.clone() else { return o.lines };
            let Some(n) = g.nodes.get(v as usize) else { return o.lines };
            o.lines.push(Line::from(vec![Span::styled(if n.local { "file " } else { "external module " }, t.kind), Span::styled(clean(app, &n.name), t.decl_name)]));
            if n.local {
                o.text(&format!("{} indexed declarations", n.decls), t.text);
            }
            import_module_lines(&mut o, &g, v);
        }
        Subject::Trail(k) => trail_lines(&mut o, k),
        Subject::Shared(k) => {
            if let Some(sh) = app.map().and_then(|m| m.trails.get(k)) {
                o.lines.push(Line::from(vec![Span::styled("shared trail ", t.kind), Span::styled(clean(app, &sh.title), t.decl_name)]));
                o.text("shipped with the project map; starting it makes your own copy", t.dim);
                if let Some(d) = &sh.description {
                    o.head("Description (authored)");
                    o.para(d, t.text);
                }
                o.head(&format!("Stops ({})", sh.stops.len()));
                for (i, s) in sh.stops.iter().enumerate() {
                    let name = match &s.target {
                        crate::map::StopRef::Decl(d) => d.clone(),
                        crate::map::StopRef::Group(g) => format!("group {g}"),
                    };
                    o.text(&format!("{}. {name}{}", i + 1, s.note.as_ref().map(|n| format!(" — {n}")).unwrap_or_default()), t.text);
                }
            }
        }
        Subject::Question(k) => {
            if let Some(q) = app.v.trails.reading.questions.get(k) {
                o.lines.push(Line::from(vec![Span::styled(if q.resolved { "resolved question " } else { "open question " }, t.kind), Span::styled(clean(app, &q.display), t.decl_name)]));
                o.para(&q.text, t.text);
            }
        }
        Subject::GroupStop { trail, stop } => {
            let st = app.v.trails.reading.trails.get(trail).and_then(|x| x.stops.get(stop)).cloned();
            if let Some(st) = st {
                match app.map().and_then(|m| m.group_by_id(&st.name)) {
                    Some(k) => group_lines(&mut o, k),
                    None => {
                        o.lines.push(Line::from(vec![Span::styled("map group ", t.kind), Span::styled(clean(app, &st.display), t.decl_name)]));
                        o.text("UNRESOLVED: not in the loaded project map (the stop and its notes are kept)", t.warn);
                    }
                }
                stop_lines(&mut o, trail, stop);
            }
        }
        Subject::Decl { name, role, stop, gnode, conn_step } => decl_lines(&mut o, &name, role.as_deref(), stop, gnode, conn_step),
    }
    o.lines
}

fn section_lines(o: &mut Out, title: &str) {
    let app = o.app;
    let t = &app.theme;
    o.lines.push(Line::from(Span::styled(chrome(app, &clean(app, title)), t.heading)));
    // One line on what the section is; the longer account is in the docs.
    match app.v.view {
        View::Map => {
            let explain = if title.starts_with("Main results") {
                "chosen in the map, never inferred"
            } else if title.starts_with("Mathematical components") {
                "curated groups; landmark counts are curated entries, not verified results"
            } else if title.starts_with("External foundations") {
                "library declarations listed in the map"
            } else if title.starts_with("Unresolved") {
                "names this index cannot resolve (renamed, removed, not indexed or ambiguous); annotations kept"
            } else if title.starts_with("Files and namespaces") {
                "generated from the file tree, not curated"
            } else {
                "lean-view.toml is read, never rewritten"
            };
            o.text(explain, t.dim);
            if title.starts_with("Files and namespaces") {
                if let Some(i) = &app.ctx.info {
                    o.text(
                        &match app.ctx.mode {
                            IndexMode::Semantic => format!("{} local modules · {} local declarations (semantic snapshot)", i.local_modules, i.local),
                            IndexMode::Source => format!("{} files · {} declaration candidates (lexical scan)", i.local_modules, i.local),
                        },
                        t.text,
                    );
                }
                o.text(&format!("right column: {}", outline_column(app)), t.dim);
            }
            if let Some(MapLoad::Loaded(m)) = app.v.map.bundle.as_ref().map(|b| &b.load) {
                o.head("Project map");
                o.text(&format!("file: {}", m.path.display()), t.dim);
                if let Some(d) = &m.description {
                    o.para(d, t.text);
                }
                if let Some(r) = &m.revision {
                    o.text(&format!("written against revision {r} (as stated; not checked)"), t.dim);
                }
                let indexed = app.ctx.info.as_ref().map(|i| format!("{} indexed local declarations", i.local)).unwrap_or_default();
                o.text(&format!("{} chosen objectives · {} groups · {} curated landmarks · {} curated relations · {indexed}", m.objectives.len(), m.groups.len(), m.curated_landmark_count(), m.relations.len()), t.text);
            }
        }
        View::Goal => {
            o.text(app.v.goal.mode.explain(), t.dim);
            o.text("A uses B: from a statement A to its prerequisite B", t.dim);
        }
        View::Imports => {
            let explain = if title.starts_with("Layers") {
                "depth of a file: the longest chain of local imports below it; a directory shows the range over its declaring files"
            } else if title.starts_with("Endpoints") {
                "files with declarations that no declaring file imports (import-only files do not count as importers)"
            } else if title.starts_with("Hubs") {
                "the most imported files, by direct local importers"
            } else if title.starts_with("Import-only") {
                "files that declare nothing and only import"
            } else if title.starts_with("Import path") {
                "a shortest chain of import lines; the search is exhaustive"
            } else {
                ""
            };
            if !explain.is_empty() {
                o.text(explain, t.dim);
            }
            o.text(
                match app.ctx.mode {
                    IndexMode::Source => "from `import` lines (lexical): build order, not mathematics. Module names are paths from the project root, so a Lake srcDir shows as external",
                    IndexMode::Semantic => "from the snapshot's module records (.olean headers): build order, not mathematics",
                },
                t.dim,
            );
            if let Some(g) = &app.v.imports.graph {
                o.head("This index");
                o.text(
                    &format!(
                        "{} · {} between them · {} of external modules · deepest chain {}",
                        count(g.local_count as u64, "local file", "local files"),
                        count(g.local_edges as u64, "import line", "import lines"),
                        count(g.external_edges as u64, "import", "imports"),
                        g.max_depth
                    ),
                    t.text,
                );
                o.text(
                    &format!(
                        "{} · {} · read in {:.0} ms · about {:.1} MiB in memory",
                        count(g.endpoints.len() as u64, "endpoint", "endpoints"),
                        count(g.import_only.len() as u64, "import-only file", "import-only files"),
                        g.load_ms,
                        g.memory_bytes() as f64 / 1048576.0
                    ),
                    t.dim,
                );
            }
        }
        _ => o.text("ordered by you, not by dependency; states are your annotations", t.dim),
    }
}

fn group_lines(o: &mut Out, k: usize) {
    let app = o.app;
    let t = &app.theme;
    let Some(m) = app.map().cloned() else { return };
    let Some(g) = m.groups.get(k) else { return };
    let res = app.v.map.bundle.as_ref().map(|b| &b.resolution);
    o.lines.push(Line::from(vec![Span::styled(if g.kind == crate::map::GroupKind::Foundation { "foundation group " } else { "group " }, t.kind), Span::styled(clean(app, &g.title), t.decl_name)]));
    let path = m.group_path(k);
    if path.len() > 1 {
        o.text(&format!("in: {}", path[..path.len() - 1].join(&format!(" {} ", app.sym.crumb))), t.dim);
    }
    let prov = if g.origin == Provenance::Authored && !g.reviewed { "authored, marked unreviewed".to_string() } else { g.origin.label().to_string() };
    o.text(&format!("organization: {prov}"), if g.origin == Provenance::Authored && g.reviewed { t.dim } else { t.warn });
    o.head("Purpose");
    match &g.purpose {
        Some(p) => o.para(p, t.text),
        None => o.text("not annotated", t.dim),
    }
    if let Some(e) = &g.explanation {
        o.head("Explanation");
        o.para(e, t.text);
    }
    let missing = g.landmarks.iter().filter(|p| res.is_some_and(|r| r.is_missing(&p.decl))).count();
    o.head(&format!("Landmarks ({} curated here{})", g.landmarks.len(), if missing > 0 { format!(", {missing} unresolved") } else { String::new() }));
    for p in &g.landmarks {
        let kind = res.and_then(|r| r.row(&p.decl)).map(|r| r.kind.short().trim_end()).unwrap_or("?");
        let role = p.role.clone().or_else(|| m.landmark(&p.decl).and_then(|l| l.purpose.clone())).map(|r| format!(" — {r}")).unwrap_or_default();
        let st = if res.is_some_and(|r| r.is_missing(&p.decl)) { t.warn } else { t.text };
        o.text(&format!("{kind} {}{role}", p.decl), st);
    }
    if g.landmarks.is_empty() {
        o.text("none listed", t.dim);
    }
    if !g.children.is_empty() {
        o.head(&format!("Subgroups ({})", g.children.len()));
        for &c in &g.children {
            let cg = &m.groups[c];
            o.text(&format!("{}{}", cg.title, cg.purpose.as_ref().map(|p| format!(" — {p}")).unwrap_or_default()), t.text);
        }
    }
    if !g.modules.is_empty() {
        o.head("Files (locators, not identity)");
        o.text(&g.modules.join(", "), t.dim);
    }
    let names: std::collections::HashSet<&str> = g.landmarks.iter().map(|p| p.decl.as_str()).collect();
    let cur = m.relations.iter().filter(|r| names.contains(r.from.as_str()) || names.contains(r.to.as_str())).count();
    o.head("Relations");
    o.text(
        &format!(
            "{cur} curated relation(s) touch these landmarks · Lean-derived: {}",
            if app.ctx.mode == IndexMode::Semantic { "available per declaration in the goal explorer (A)" } else { "none (source-only index)" }
        ),
        t.text,
    );
    if !g.paper.is_empty() {
        o.head("Paper");
        o.papers(&g.paper);
    }
    if !g.limitations.is_empty() {
        o.head("Known limitations");
        for l in &g.limitations {
            o.item(app.sym.bullet, l.clone(), t.text);
        }
    }
}

fn trail_lines(o: &mut Out, k: usize) {
    let app = o.app;
    let t = &app.theme;
    let rd = &app.v.trails.reading;
    let Some(tr) = rd.trails.get(k) else { return };
    o.lines.push(Line::from(vec![Span::styled("trail ", t.kind), Span::styled(clean(app, &tr.title), t.decl_name)]));
    let origin = match tr.origin.strip_prefix("shared:") {
        Some(id) => format!("your copy of the shared trail \"{id}\" from the project map"),
        None => "personal".to_string(),
    };
    o.text(&format!("{origin} · {} stops · {} read or understood", tr.stops.len(), tr.done()), t.dim);
    if let Some(d) = &tr.description {
        o.para(d, t.text);
    }
    let stale = tr.stops.iter().filter(|x| matches!(rd.statuses.get(&x.id), Some(StopStatus::Stale(_)))).count();
    let unres = tr.stops.iter().filter(|x| matches!(rd.statuses.get(&x.id), Some(StopStatus::Unresolved(_)))).count();
    if stale + unres > 0 {
        o.item(app.sym.warn, format!("{stale} stale and {unres} unresolved stop(s) against the open index"), t.warn);
    }
    match tr.resume_at() {
        Some(i) => o.text(&format!("next unread: stop {}, {}", i + 1, tr.stops[i].display), t.text),
        None if tr.stops.is_empty() => o.text("no stops yet", t.dim),
        None => o.text("every stop is read or understood", t.text),
    }
}

fn stop_lines(o: &mut Out, trail: usize, stop: usize) {
    let app = o.app;
    let t = &app.theme;
    let rd = &app.v.trails.reading;
    let Some(tr) = rd.trails.get(trail) else { return };
    let Some(st) = tr.stops.get(stop) else { return };
    o.head(&format!("Stop {} of {} in \"{}\"", stop + 1, tr.stops.len(), tr.title));
    o.text(&format!("state: {} (your annotation)", st.state.as_str()), t.text);
    match rd.statuses.get(&st.id) {
        Some(StopStatus::Stale(why)) => o.item(app.sym.warn, format!("STALE: {why}"), t.warn),
        Some(StopStatus::Unresolved(why)) => o.item(app.sym.warn, format!("UNRESOLVED: {why}"), t.warn),
        _ => {}
    }
    match &st.note {
        Some(n) => {
            o.text("why it is on the route:", t.dim);
            o.para(n, t.text);
        }
        None => o.text("no explanation", t.dim),
    }
    match &st.question {
        Some(q) => o.item(app.sym.question, format!("question: {q}"), t.accent),
        None => o.text("no question", t.dim),
    }
    if st.name.starts_with("_private.") {
        o.text("private: its name embeds the module path, so it breaks when the file moves", t.dim);
    }
}

fn decl_lines(o: &mut Out, name: &str, role: Option<&str>, stop: Option<(usize, usize)>, gnode: Option<usize>, conn_step: Option<usize>) {
    let app = o.app;
    let t = &app.theme;
    let s = &app.sym;
    let map = app.map().cloned();
    let lm = map.as_ref().and_then(|m| m.landmark(name).cloned());
    let res = app.v.map.bundle.as_ref().map(|b| &b.resolution);
    let bundle = app.subject_bundle();

    // Title.
    match bundle {
        Some(b) => {
            let mut title = vec![Span::styled(format!("{} ", b.d.row.kind.as_str()), t.kind), Span::styled(clean(app, &b.d.row.display), t.decl_name)];
            if b.bookmarked || app.bookmarked.contains(name) {
                title.push(Span::styled(format!(" {}", s.bookmark), t.warn));
            }
            o.lines.push(Line::from(title));
            let mut loc = String::new();
            if let Some(m) = &b.d.row.module {
                loc.push_str(&format!("module {m}"));
            }
            if let Some(p) = &b.d.module_path {
                loc.push_str(&format!(" · {p}{}", b.d.pos.start_line.map(|l| format!(":{l}")).unwrap_or_default()));
            }
            loc.push_str(&format!(" · {}", b.d.row.origin.label()));
            o.text(&loc, t.dim);
        }
        None => {
            o.lines.push(Line::from(Span::styled(clean(app, name), t.decl_name)));
            if app.v.detail.missing {
                o.item(s.warn, "not in this index (renamed, removed or not indexed); its annotations and stops are kept".into(), t.warn);
            } else {
                o.text("loading…", t.dim);
            }
        }
    }
    let goal = app.v.goal.goal.clone();
    let is_goal = goal.as_ref().is_some_and(|g| g.name == name);
    if is_goal {
        o.item(s.goal, "the pinned goal".into(), t.accent);
    }

    // On a trail, the reader's own stop comes first.
    if let Some((tr, st)) = stop {
        stop_lines(o, tr, st);
    }

    // How the reader got here in the goal outline, or along a connection.
    if let Some(gi) = gnode {
        let g = &app.v.goal;
        let mut chain: Vec<usize> = Vec::new();
        let mut cur = Some(gi);
        while let Some(c) = cur {
            chain.push(c);
            cur = g.nodes.get(c).and_then(|n| n.parent);
        }
        chain.reverse();
        if chain.len() > 1 {
            o.head("Path from the goal");
            let rel = if g.dir == Direction::Uses { "uses" } else { "is used by" };
            for (i, &c) in chain.iter().enumerate() {
                let n = &g.nodes[c];
                let tags = if i == 0 { String::new() } else { format!("{rel} {}", label_tags(&n.labels)) };
                o.text(&format!("{}{tags}{}", "  ".repeat(i), n.name), if c == gi { t.text } else { t.dim });
            }
            let n = &g.nodes[gi];
            let parent = n.parent.map(|p| g.nodes[p].name.clone()).unwrap_or_default();
            o.head("Why this edge is shown");
            for l in &n.labels {
                let (a, b) = if g.dir == Direction::Uses { (parent.as_str(), n.name.as_str()) } else { (n.name.as_str(), parent.as_str()) };
                o.item(s.bullet, format!("[{}] {}", l.tag(), l.explain(&short(a), &short(b))), t.text);
            }
            if let Some(note) = &n.note {
                o.text("the map's author says:", t.dim);
                o.para(note, t.text);
            }
            if n.backref {
                o.item(s.warn, "already on this path (cycle): not unfolded again".into(), t.warn);
            } else if n.repeat {
                o.item(s.bullet, "also appears elsewhere in this outline".into(), t.dim);
            }
        }
    }
    if let Some(n) = gnode.and_then(|gi| app.v.goal.nodes.get(gi)).filter(|n| n.loaded) {
        let p = &n.page;
        if p.hidden_external + p.hidden_generated + p.hidden_kind > 0 || p.lean_counts.is_some() {
            o.head("Its relations in this outline");
            if let Some(c) = p.lean_counts {
                o.text(&format!("Lean edges before filters: T {} · V {} · C {}", c[0], c[1], c[2]), t.text);
            }
            let cap = |x: u64| if x >= crate::relations::COUNT_CAP { format!("at least {x}") } else { x.to_string() };
            if p.hidden_external > 0 {
                o.item(s.bullet, format!("hidden: {} external (L, H)", cap(p.hidden_external)), t.text);
            }
            if p.hidden_generated > 0 {
                o.item(s.bullet, format!("hidden: {} Lean-generated (G, H)", cap(p.hidden_generated)), t.text);
            }
            if p.hidden_kind > 0 {
                o.item(s.bullet, format!("hidden: {} edges of the other kind (y, H)", p.hidden_kind), t.text);
            }
        }
    }
    if let Some(i) = conn_step {
        if let Some(crate::relations::ConnOutcome::Path(p)) = app.v.goal.conn.as_ref().and_then(|c| c.result.as_ref()).map(|r| &r.outcome) {
            o.head(&format!("Step {} of {} on the connection", i + 1, p.len()));
            let uses = app.v.goal.conn.as_ref().is_some_and(|c| c.query.dir == Direction::Uses);
            if let (Some(l), Some(nx)) = (&p[i].next, p.get(i + 1)) {
                let (a, b) = if uses { (name, nx.name.as_str()) } else { (nx.name.as_str(), name) };
                o.item(s.bullet, format!("[{}] {}", l.tag(), l.explain(&short(a), &short(b))), t.text);
            } else {
                o.text("the end of the path", t.dim);
            }
        }
    }

    o.head("Purpose");
    match lm.as_ref().and_then(|l| l.purpose.clone()) {
        Some(p) => {
            o.para(&p, t.text);
            if let Some(l) = &lm {
                let stale = res.is_some_and(|r| r.is_stale(l));
                o.text(&format!("annotation: {}{}", l.origin.label(), if stale { " — STALE: its source file changed since the annotation" } else { "" }), if stale || l.origin != Provenance::Authored { t.warn } else { t.dim });
            }
        }
        None => o.text("not annotated", t.dim),
    }
    if let Some(r) = role {
        o.text(&format!("role in this group: {r}"), t.text);
    }
    if let Some(e) = lm.as_ref().and_then(|l| l.explanation.clone()) {
        o.head("Explanation");
        o.para(&e, t.text);
    }

    if let Some(b) = bundle {
        o.head(if app.ctx.mode == IndexMode::Semantic { "Statement (elaborated)" } else { "Statement (source text, not elaborated)" });
        o.lines.extend(statement_lines(app, b, o.w, None));
    }

    if let Some(m) = &map {
        let places = m.placements(name);
        if m.is_objective(name) || !places.is_empty() {
            o.head("In the map");
            if let Some(ob) = m.objectives.iter().find(|x| x.decl == name) {
                o.item(s.objective, format!("chosen as a main result{}", ob.title.as_ref().map(|x| format!(": {x}")).unwrap_or_default()), t.text);
            }
            for k in &places {
                let role = m.groups[*k].landmarks.iter().find(|p| p.decl == name).and_then(|p| p.role.clone()).map(|r| format!(" — {r}")).unwrap_or_default();
                o.item(s.bullet, format!("{}{role}", m.group_path(*k).join(&format!(" {} ", s.crumb))), t.text);
            }
        }
        let uses = m.curated(name, true);
        let used = m.curated(name, false);
        o.head("Prerequisites and dependents");
        for r in &uses {
            o.item(s.arrow, format!("[cur] uses {}{}", r.to, r.note.as_ref().map(|n| format!(" — {n}")).unwrap_or_default()), t.text);
        }
        for r in &used {
            o.item(s.larrow, format!("[cur] used by {}{}", r.from, r.note.as_ref().map(|n| format!(" — {n}")).unwrap_or_default()), t.text);
        }
        if uses.is_empty() && used.is_empty() {
            o.text("no curated relations", t.dim);
        }
    } else {
        o.head("Prerequisites and dependents");
        o.text("no project map: nothing curated", t.dim);
    }
    if let Some(b) = bundle {
        if app.ctx.mode == IndexMode::Semantic && b.d.row.origin != Origin::Unresolved {
            let d = &b.d.degrees;
            o.text(&format!("Lean-derived: {} constants in its statement, {} in its proof/body; used by {}", d[0], d[1], d[3] + d[4]), t.text);
        } else if app.ctx.mode == IndexMode::Source {
            o.text("Lean-derived: unavailable in a source-only index", t.dim);
        }
    }

    if let Some(g) = goal.as_ref().filter(|_| !is_goal) {
        o.head("Contribution to the goal");
        match map.as_ref().and_then(|m| m.curated_path(&g.name, name, 5000)) {
            Some(path) => {
                o.text(&format!("curated route from {}:", short(&g.display)), t.text);
                for (i, (n, rel)) in path.iter().enumerate() {
                    let why = rel.as_ref().and_then(|r| r.note.clone()).map(|n| format!(" — {n}")).unwrap_or_default();
                    o.text(&format!("{}{}{}{why}", "  ".repeat(i), if i == 0 { "" } else { "uses " }, short(n)), t.text);
                }
                o.text("curated in the map, not Lean-derived", t.dim);
            }
            None => o.text(&format!("no curated route from {}", short(&g.display)), t.dim),
        }
    }

    // Paper correspondence: authored references, then literal candidates.
    let authored: Vec<PaperRef> = lm.as_ref().map(|l| l.paper.clone()).unwrap_or_default();
    let mut cands: Vec<String> = Vec::new();
    if let Some(b) = bundle {
        let mut text = b.d.doc.clone().unwrap_or_default();
        for (_, l) in &b.excerpt {
            if l.contains("--") || l.contains("/-") {
                text.push('\n');
                text.push_str(l);
            }
        }
        cands = crate::map::paper_candidates(&text).into_iter().filter(|c| !authored.iter().any(|a| a.text == *c)).collect();
    }
    if !authored.is_empty() || !cands.is_empty() {
        o.head("Paper");
        o.papers(&authored);
        if !cands.is_empty() {
            o.text("candidates from its documentation or comments (literal text, no identified source):", t.dim);
            for c in cands {
                o.item(s.bullet, format!("\"{c}\""), t.text);
            }
        }
    }
    if let Some(l) = lm.as_ref().filter(|l| !l.limitations.is_empty()) {
        o.head("Known limitations");
        for x in &l.limitations {
            o.item(s.bullet, x.clone(), t.text);
        }
    }

    if let Some(b) = bundle {
        let d = &b.d;
        o.head("Evidence and freshness");
        match (app.ctx.mode, d.row.origin) {
            (IndexMode::Source, _) | (_, Origin::SourceCandidate) => {
                o.item(s.bullet, "source candidate (lexical scan, not elaborated)".into(), t.text);
                if d.row.flags & flags::LEX_SORRY != 0 {
                    o.item(s.warn, "`sorry`/`admit` token in its text (lexical)".into(), t.danger);
                } else {
                    o.item(s.bullet, "no `sorry` token in its text (proves nothing)".into(), t.dim);
                }
                o.item(s.bullet, "axioms and proof status: unknown without a semantic snapshot".into(), t.dim);
            }
            (IndexMode::Semantic, Origin::Unresolved) => o.item(s.warn, "referenced by the snapshot, not described in it (coverage gap)".into(), t.warn),
            (IndexMode::Semantic, _) => {
                let info = app.ctx.info.as_ref();
                o.item(
                    s.ok,
                    format!(
                        "Lean {} snapshot of {}; build not run or verified by slim",
                        info.and_then(|i| i.get("lean_version")).unwrap_or("?"),
                        info.and_then(|i| i.get("created_at")).unwrap_or("?")
                    ),
                    t.ok,
                );
                let gap = d.axioms.iter().any(|a| a.category == AxiomCategory::Incomplete);
                let mut any = false;
                for a in &d.axioms {
                    match a.category {
                        AxiomCategory::Sorry => o.item(s.warn, "depends on sorryAx (a placeholder proof in its dependency closure)".into(), t.danger),
                        AxiomCategory::Custom => o.item(s.warn, format!("depends on the custom axiom {}", a.name), t.warn),
                        AxiomCategory::CompilerTrust => o.item(s.warn, format!("trusts compiler evaluation: {}", a.name), t.warn),
                        _ => continue,
                    }
                    any = true;
                }
                let std: Vec<&str> = d.axioms.iter().filter(|a| a.category == AxiomCategory::Standard).map(|a| a.name.as_str()).collect();
                if !std.is_empty() {
                    o.item(s.bullet, format!("standard axioms: {}", std.join(", ")), t.text);
                }
                if gap {
                    o.item(s.warn, "dependency data incomplete: the axiom list is a lower bound".into(), t.warn);
                } else if !any && std.is_empty() {
                    o.item(s.ok, "no axioms in its dependency closure".into(), t.ok);
                }
            }
        }
        match &b.source_status {
            SourceStatus::Changed if app.ctx.mode == IndexMode::Semantic => {
                o.item(s.warn, "SOURCE CHANGED since the snapshot: the semantic data describes the old source".into(), t.warn)
            }
            SourceStatus::Changed => o.item(s.warn, "source changed since indexing".into(), t.warn),
            SourceStatus::Ok if d.module_path.is_some() => o.item(s.ok, "source file matches the index (SHA-256)".into(), t.ok),
            SourceStatus::Missing(e) => o.item(s.warn, format!("source unavailable: {e}"), t.warn),
            _ => {}
        }
    }

    // The reader's own data.
    let rd = &app.v.trails.reading;
    let qs: Vec<&crate::userdata::Question> = rd.questions.iter().filter(|q| q.name == name && q.ref_kind == RefKind::Decl).collect();
    let on: Vec<(String, usize, &crate::userdata::Stop)> = rd
        .trails
        .iter()
        .flat_map(|tr| tr.stops.iter().enumerate().filter(|(_, x)| x.name == name && x.ref_kind == RefKind::Decl).map(move |(i, x)| (tr.title.clone(), i, x)))
        .collect();
    if stop.is_none() && (!qs.is_empty() || !on.is_empty()) {
        o.head("Your notes");
        for q in qs {
            o.item(if q.resolved { s.ok } else { s.question }, format!("{}{}", q.text, if q.resolved { " (resolved)" } else { "" }), t.text);
        }
        for (title, i, x) in on {
            o.item(s.bullet, format!("stop {} of \"{title}\" — {}{}", i + 1, x.state.as_str(), x.note.as_ref().map(|n| format!(" — {n}")).unwrap_or_default()), t.text);
        }
    }
    if let Some(doc) = bundle.and_then(|b| b.d.doc.as_ref()).filter(|x| !x.trim().is_empty()) {
        o.head("Documentation (from the source)");
        o.para(doc, t.text);
    }
}
