//! Drawing. Pure function of the application state; all text coming from an
//! index or a source file is sanitised before it reaches the terminal.

use super::state::*;
use super::worker::{OutlineNode, SourceStatus};
use crate::relations::Occurrence;
use crate::skim::SkimItem;
use crate::db::query::{DeclRow, Direction};
use crate::db::IndexMode;
use crate::model::{flags, AxiomCategory, Coverage, EdgeKind, Origin};
use crate::text::{sanitize, truncate, truncate_left, width as text_width, wrap_line};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

pub(super) fn clean(app: &App, s: &str) -> String {
    sanitize(s, app.ascii).replace('\n', " ")
}

/// UI-generated text in ASCII mode: typographic chrome becomes ASCII.
/// Never applied to statements, documentation or source text.
pub(super) fn chrome(app: &App, s: &str) -> String {
    if !app.ascii {
        return s.to_string();
    }
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '·' => o.push('|'),
            '“' | '”' => o.push('"'),
            '─' => o.push('-'),
            '≥' => o.push_str(">="),
            '…' => o.push_str("..."),
            '→' => o.push_str("->"),
            '←' => o.push_str("<-"),
            '└' => o.push_str("`-"),
            '✓' => o.push_str("ok"),
            '⚠' => o.push_str("!!"),
            '⏎' => o.push_str("Enter"),
            '★' => o.push('*'),
            '–' | '—' => o.push('-'),
            other => o.push(other),
        }
    }
    o
}

pub(super) fn block<'a>(app: &App, title: Line<'a>, focused: bool) -> Block<'a> {
    Block::default()
        .style(app.theme.base)
        .borders(Borders::ALL)
        .border_set(app.sym.border)
        .border_style(if focused { app.theme.border_focus } else { app.theme.border })
        .title(title)
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    // Themes with their own background paint it everywhere first.
    f.render_widget(Block::default().style(app.theme.base), area);
    if area.width < 30 || area.height < 8 {
        f.render_widget(Paragraph::new("terminal too small for slim (need 30x8)"), area);
        return;
    }
    let [title, crumbs, body, status] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(3), Constraint::Length(1)]).areas(area);
    app.last_body_height = body.height as usize;
    draw_title(f, title, app);
    draw_crumbs(f, crumbs, app);
    if app.building || (app.ctx.index.is_none()) {
        draw_building(f, body, app);
    } else {
        let w = body.width;
        app.card_under_list = w < 100;
        if app.v.view != super::views::View::Browse {
            super::render_views::draw_view(f, body, app);
        } else if w >= 100 {
            let lw = if w >= 140 {
                (w as u32 * 26 / 100).min(48) as u16
            } else if w >= 120 {
                30
            } else {
                26
            };
            let rw = if w >= 140 {
                (w as u32 * 30 / 100).min(64) as u16
            } else if w >= 120 {
                36
            } else {
                30
            };
            let [l, c, r] = Layout::horizontal([Constraint::Length(lw), Constraint::Min(30), Constraint::Length(rw)]).areas(body);
            draw_left(f, l, app);
            draw_center(f, c, app);
            draw_right(f, r, app);
        } else if w >= 60 {
            let lw = (w / 3).clamp(22, 34);
            let [l, m] = Layout::horizontal([Constraint::Length(lw), Constraint::Min(20)]).areas(body);
            draw_left(f, l, app);
            if app.focus == Focus::Right {
                draw_right(f, m, app);
            } else {
                draw_center(f, m, app);
            }
        } else {
            match app.focus {
                Focus::Left => draw_left(f, body, app),
                Focus::Center => draw_center(f, body, app),
                Focus::Right => draw_right(f, body, app),
            }
        }
    }
    draw_status(f, status, app);
    draw_overlay(f, area, app);
    if app.first_frame_ms.is_none() {
        app.first_frame_ms = Some(app.started.elapsed().as_millis());
    }
}

fn draw_title(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let mut spans = vec![Span::styled(" slim ", t.bar.add_modifier(Modifier::BOLD))];
    let mode = match app.ctx.mode {
        IndexMode::Semantic if app.ctx.synthetic => "SYNTHETIC DATA".to_string(),
        IndexMode::Semantic => {
            let v = app.ctx.info.as_ref().and_then(|i| i.get("lean_version").map(String::from)).unwrap_or_default();
            if v.is_empty() {
                "SEMANTIC snapshot".into()
            } else {
                format!("SEMANTIC snapshot · Lean {v}")
            }
        }
        IndexMode::Source => "SOURCE-ONLY (lexical, not elaborated)".into(),
    };
    spans.push(Span::styled(format!(" {} ", chrome(app, &mode)), if app.ctx.synthetic { t.warn } else { t.accent }));
    let fresh = match &app.fresh {
        Fresh::Unknown => Span::styled(format!("{} freshness unknown ", app.sym.sep), t.dim),
        Fresh::Checking => Span::styled(format!("{} checking sources… ", app.sym.sep), t.dim),
        Fresh::Fresh { .. } => Span::styled(format!("{} {} sources match index ", app.sym.sep, app.sym.ok), t.ok),
        Fresh::Stale(fr) => Span::styled(
            format!(
                "{} {} STALE: {} changed, {} deleted, {} new{} ",
                app.sym.sep,
                app.sym.warn,
                fr.changed.len(),
                fr.deleted.len(),
                fr.added.len(),
                if fr.env_changed.is_empty() { "" } else { ", toolchain/deps changed" }
            ),
            t.warn,
        ),
        Fresh::Error(_) => Span::styled(format!("{} freshness check failed ", app.sym.sep), t.warn),
    };
    spans.push(fresh);
    if let Some((label, done, total)) = &app.job {
        let pct = if *total > 0 { format!(" {:.0}%", *done as f64 * 100.0 / *total as f64) } else { String::new() };
        spans.push(Span::styled(format!("{} {label}{pct} ", app.sym.sep), t.accent));
    }
    if let Some(label) = app.hl_job_label() {
        spans.push(Span::styled(format!("{} {} ", app.sym.sep, chrome(app, &label)), t.accent));
    }
    spans.push(Span::styled(format!("{} {} ", app.sym.sep, clean(app, &app.ctx.title)), t.title));
    f.render_widget(Paragraph::new(Line::from(spans)).style(t.bar), area);
}

fn draw_crumbs(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    if app.v.view != super::views::View::Browse && !app.building && app.ctx.index.is_some() {
        f.render_widget(Paragraph::new(super::render_views::view_crumbs(app, area.width as usize)), area);
        return;
    }
    let mut spans: Vec<Span> = Vec::new();
    // A pinned goal stays in view while browsing declarations.
    if let Some(g) = &app.v.goal.goal {
        spans.push(Span::styled(format!("{} goal: {} {} ", app.sym.goal, clean(app, &short_name(&g.display)), app.sym.sep), t.accent));
    }
    if let Some(b) = &app.detail {
        let d = &b.d;
        if let Some(m) = &d.row.module {
            spans.push(Span::styled(clean(app, m), t.dim));
            spans.push(Span::styled(format!(" {} ", app.sym.crumb), t.dim));
        }
        let ns = crate::model::namespace_of(&d.row.display);
        if !ns.is_empty() {
            spans.push(Span::styled(clean(app, ns), t.accent));
            spans.push(Span::styled(format!(" {} ", app.sym.crumb), t.dim));
        }
        let last = &d.row.display[crate::model::last_component_start(&d.row.display)..];
        spans.push(Span::styled(clean(app, last), t.decl_name));
        let route = app.route(4);
        if route.len() > 1 || !app.current_how.is_empty() {
            spans.push(Span::styled(format!("   {} route: ", app.sym.sep), t.dim));
            for (i, (how, name)) in route.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::styled(format!(" {} ", app.sym.arrow), t.dim));
                }
                if !how.is_empty() {
                    spans.push(Span::styled(format!("{} ", chrome(app, &clean(app, how))), t.dim));
                }
                spans.push(Span::styled(clean(app, name), if i + 1 == route.len() { t.text } else { t.dim }));
            }
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_building(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let mut lines = vec![
        Line::from(Span::styled("Building the source-only index", t.heading)),
        Line::from(""),
        Line::from("slim is scanning this project's .lean files (read-only, no Lean, no Lake, no project code)."),
        Line::from("The index is written to the cache directory, never into the project."),
        Line::from(""),
    ];
    if let Some((label, done, total)) = &app.job {
        let pct = if *total > 0 { *done as f64 * 100.0 / *total as f64 } else { 0.0 };
        let w = (area.width as usize).saturating_sub(10).min(60);
        let filled = (pct / 100.0 * w as f64) as usize;
        let bar: String = if app.ascii {
            format!("[{}{}]", "#".repeat(filled), ".".repeat(w - filled))
        } else {
            format!("[{}{}]", "█".repeat(filled), "░".repeat(w - filled))
        };
        lines.push(Line::from(format!("{label}: {pct:.0}%")));
        lines.push(Line::from(bar));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("q quits (the scan is cancelled; any previous index stays intact)", t.dim)));
    let b = block(app, Line::from(" slim "), true);
    f.render_widget(Paragraph::new(lines).block(b), area);
}

/// `1234`, `12.3k`, `1.2M`.
pub(super) fn compact(n: u64) -> String {
    if n < 10_000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

/// How long ago `mtime_ns` was, in one unit (`5m`, `3h`, `2d`, `6w`, `2y`).
pub(super) fn age(now_ns: i64, mtime_ns: i64) -> String {
    if mtime_ns <= 0 {
        return "?".into();
    }
    let s = (now_ns - mtime_ns).max(0) / 1_000_000_000;
    match s {
        0..=59 => "<1m".into(),
        60..=3599 => format!("{}m", s / 60),
        3600..=86_399 => format!("{}h", s / 3600),
        86_400..=1_209_599 => format!("{}d", s / 86_400),
        1_209_600..=31_535_999 => format!("{}w", s / 604_800),
        _ => format!("{}y", s / 31_536_000),
    }
}

pub(super) fn now_ns() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as i64).unwrap_or(0)
}

/// What the column on the right of outline rows counts, for the title.
pub(super) fn outline_column(app: &App) -> &'static str {
    match (app.outline.order, app.ctx.mode) {
        (OutlineOrder::Name | OutlineOrder::Decls, _) => "declarations",
        (OutlineOrder::Theorems, _) => "theorems",
        (OutlineOrder::ProofLines, _) => "proof lines",
        (OutlineOrder::Sorry, IndexMode::Source) => "sorry tokens",
        (OutlineOrder::Sorry, IndexMode::Semantic) => "sorryAx dependents",
        (OutlineOrder::Modified, _) => "last modified",
    }
}

/// The right column of an outline row: the quantity the outline is ordered by.
pub(super) fn outline_metric(app: &App, n: &OutlineNode, now: i64) -> String {
    match app.outline.order {
        OutlineOrder::Name | OutlineOrder::Decls => compact(n.decls_total),
        OutlineOrder::Theorems => compact(n.roll.thm as u64),
        OutlineOrder::ProofLines => compact(n.roll.proof_lines as u64),
        OutlineOrder::Sorry => {
            if n.roll.sorry == 0 {
                app.sym.bullet.to_string()
            } else {
                compact(n.roll.sorry as u64)
            }
        }
        OutlineOrder::Modified => age(now, n.roll.mtime_ns),
    }
}

fn filter_summary(app: &App) -> String {
    let mut v = Vec::new();
    v.push(if app.filters.local_only { "local" } else { "local+lib" });
    if app.filters.show_generated {
        v.push("+generated");
    }
    let k = app.filters.kinds.label();
    let mut s = v.join(" ");
    if k != "all kinds" {
        s.push(' ');
        s.push_str(k);
    }
    s
}

fn draw_left(f: &mut Frame, area: Rect, app: &mut App) {
    let t = app.theme.clone();
    let focused = app.focus == Focus::Left;
    let title = match app.left_mode {
        LeftMode::Search => "Search",
        LeftMode::Outline => "Outline",
        LeftMode::Bookmarks => "Bookmarks",
        LeftMode::Trail => "Reading trail",
    };
    // The outline says what its right column counts; lists say what is filtered.
    let tag = if app.left_mode == LeftMode::Outline { outline_column(app).to_string() } else { filter_summary(app) };
    let b = block(app, Line::from(vec![Span::styled(format!(" {title} "), t.title), Span::styled(format!("[{tag}] "), t.dim)]), focused);
    let inner = b.inner(area);
    f.render_widget(b, area);
    app.hit.left = area;
    app.hit.left_list = inner;
    app.hit.left_input = Rect::default();
    if inner.height == 0 || inner.width < 4 {
        return;
    }
    let w = inner.width as usize;
    match app.left_mode {
        LeftMode::Search => {
            let scope_h = if app.search.scope.is_some() { 1 } else { 0 };
            let [input, scope, summary, list] =
                Layout::vertical([Constraint::Length(1), Constraint::Length(scope_h), Constraint::Length(1), Constraint::Min(1)]).areas(inner);
            app.hit.left_input = input;
            app.hit.left_list = list;
            if let Some(sc) = &app.search.scope {
                // Always visible while it applies: results are not the whole project.
                // The end of a dotted prefix says most; the hint goes first when short of room.
                let lead = if sc.kind == crate::nameindex::ScopeKind::Namespace { "in namespace " } else { "in files " };
                let name = truncate_left(&clean(app, &sc.prefix), w.saturating_sub(lead.len()).max(6), app.ascii);
                let mut text = format!("{lead}{name}");
                let hint = format!(" {} F clears", app.sym.sep);
                if text_width(&text) + text_width(&hint) <= w {
                    text.push_str(&hint);
                }
                f.render_widget(Paragraph::new(Span::styled(truncate(&text, w, app.ascii), t.warn)), scope);
            }
            let q = clean(app, &app.search.query);
            let prefix = "/ ";
            let shown = truncate(&q, w.saturating_sub(3), app.ascii);
            f.render_widget(Paragraph::new(Line::from(vec![Span::styled(prefix, t.accent), Span::styled(shown, t.input)])), input);
            if focused && app.search.editing && matches!(app.overlay, Overlay::None) {
                let before: String = app.search.query.chars().take(app.search.cursor).collect();
                let cx = (text_width(&clean(app, &before)) + 2).min(w.saturating_sub(1));
                f.set_cursor_position(Position { x: input.x + cx as u16, y: input.y });
            }
            let sum = if app.search.query.trim().is_empty() && app.search.scope.is_none() {
                match &app.search_ready {
                    None => "loading name index…".to_string(),
                    Some(Ok((n, ms, _))) => format!("{n} names ready ({ms} ms)"),
                    Some(Err(e)) => format!("search unavailable: {e}"),
                }
            } else if app.search.pending && app.search.items.is_empty() {
                "searching…".to_string()
            } else {
                app.search.summary.clone()
            };
            f.render_widget(Paragraph::new(Span::styled(truncate(&chrome(app, &sum), w, app.ascii), t.dim)), summary);
            let h = list.height as usize;
            app.search.list.clamp(app.search.items.len(), h);
            let mut lines = Vec::new();
            for (i, it) in app.search.items.iter().enumerate().skip(app.search.list.scroll).take(h) {
                let sel = i == app.search.list.selected;
                match it {
                    LeftItem::Section(s) => lines.push(Line::from(Span::styled(truncate(&chrome(app, &format!("── {s}")), w, app.ascii), t.heading))),
                    LeftItem::Result { row, tier } => {
                        lines.push(result_line(app, row, tier.map(|x| x.tag()), w, sel, focused));
                    }
                    LeftItem::Gloss { name, text } => {
                        let st = if sel { if focused { t.selected } else { t.selected_unfocused } } else { t.text };
                        let head = format!(" = {} ", short_name(name));
                        let avail = w.saturating_sub(text_width(&head));
                        lines.push(Line::from(vec![
                            Span::styled(truncate(&clean(app, &head), w, app.ascii), st),
                            Span::styled(pad(&truncate(&sanitize(text, app.ascii), avail, app.ascii), avail), if sel { st } else { t.dim }),
                        ]));
                    }
                    LeftItem::MapHit { label, what, .. } => {
                        let st = if sel { if focused { t.selected } else { t.selected_unfocused } } else { t.text };
                        let right = format!(" {what}");
                        let avail = w.saturating_sub(right.len());
                        lines.push(Line::from(vec![
                            Span::styled(pad(&truncate(&format!(" map   {}", clean(app, label)), avail, app.ascii), avail), st),
                            Span::styled(right, if sel { st } else { t.dim }),
                        ]));
                    }
                }
            }
            if app.search.items.is_empty() && (!app.search.query.trim().is_empty() || app.search.scope.is_some()) && !app.search.pending {
                lines.push(Line::from(Span::styled(if app.search.scope.is_some() { "no matches in this scope" } else { "no matches" }, t.dim)));
            }
            f.render_widget(Paragraph::new(lines), list);
        }
        LeftMode::Outline => {
            let h = inner.height as usize;
            if !app.outline.loaded {
                f.render_widget(Paragraph::new(Span::styled("loading modules…", t.dim)), inner);
                return;
            }
            app.outline.list.clamp(app.outline.rows.len(), h);
            let mut lines = Vec::new();
            let now = now_ns();
            for (i, r) in app.outline.rows.iter().enumerate().skip(app.outline.list.scroll).take(h) {
                let sel = i == app.outline.list.selected;
                let st = if sel { if focused { t.selected } else { t.selected_unfocused } } else { t.text };
                let line = match r {
                    OutlineRow::Node(n) => {
                        let node = &app.outline.nodes[*n];
                        let has = !node.children.is_empty() || node.module.is_some();
                        let mark = if !has { " " } else if node.expanded { app.sym.expanded } else { app.sym.collapsed };
                        let warn = node.module.as_ref().is_some_and(|m| m.status != "ok");
                        let text = format!(
                            "{}{} {}{}",
                            " ".repeat(node.depth as usize),
                            mark,
                            clean(app, &node.label),
                            if warn { format!(" {}", app.sym.warn) } else { String::new() }
                        );
                        let count = format!(" {}", outline_metric(app, node, now));
                        let avail = w.saturating_sub(count.chars().count());
                        Line::from(vec![Span::styled(pad(&truncate(&text, avail, app.ascii), avail), st), Span::styled(count, if sel { st } else { t.dim })])
                    }
                    OutlineRow::Decl { item, depth } => {
                        let last = &item.row.display[crate::model::last_component_start(&item.row.display)..];
                        let line_no = item.line.map(|l| format!(" :{l}")).unwrap_or_default();
                        let text = format!("{}{} {}", " ".repeat(*depth as usize + 1), item.row.kind.short().trim_end(), clean(app, last));
                        let avail = w.saturating_sub(line_no.len());
                        Line::from(vec![Span::styled(pad(&truncate(&text, avail, app.ascii), avail), st), Span::styled(line_no, if sel { st } else { t.dim })])
                    }
                    OutlineRow::Note { text, depth } => Line::from(Span::styled(format!("{}{}", " ".repeat(*depth as usize + 1), text), t.dim)),
                    OutlineRow::Section { title, depth } => {
                        let text = format!("{}{}", " ".repeat(*depth as usize + 1), clean(app, title));
                        Line::from(Span::styled(pad(&truncate(&text, w, app.ascii), w), if sel { st } else { t.heading }))
                    }
                };
                lines.push(line);
            }
            f.render_widget(Paragraph::new(lines), inner);
        }
        LeftMode::Bookmarks | LeftMode::Trail => {
            let is_b = app.left_mode == LeftMode::Bookmarks;
            let (marks, list) = (&app.bookmarks.marks, &mut app.bookmarks.list);
            let h = inner.height as usize;
            let mut lines = Vec::new();
            if marks.is_empty() {
                lines.push(Line::from(Span::styled(
                    if is_b { "no bookmarks" } else { "reading trails have their own view (T)" },
                    t.dim,
                )));
                lines.push(Line::from(Span::styled("Stored in your data directory, not in the project.", t.dim)));
            }
            list.clamp(marks.len(), h);
            let (sel_i, scroll) = (list.selected, list.scroll);
            for (i, m) in marks.iter().enumerate().skip(scroll).take(h) {
                let sel = i == sel_i;
                let st = if sel { if focused { t.selected } else { t.selected_unfocused } } else { t.text };
                let lead = if is_b { app.sym.bookmark.to_string() } else { format!("{}.", i + 1) };
                let note = m.note.as_deref().map(|n| format!(" — {}", n.lines().next().unwrap_or(""))).unwrap_or_default();
                let text = format!("{lead} {}{}", clean(app, &m.display), clean(app, &note));
                lines.push(Line::from(Span::styled(pad(&truncate(&text, w, app.ascii), w), st)));
            }
            if !marks.is_empty() && h > marks.len() + 1 {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    if is_b { "⏎ open · m toggles · n note" } else { "⏎ open · n note · d remove · E export Markdown" },
                    t.dim,
                )));
            }
            f.render_widget(Paragraph::new(lines), inner);
        }
    }
}

pub(super) fn pad(s: &str, w: usize) -> String {
    let cw = text_width(s);
    if cw >= w {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(w - cw))
    }
}

fn result_line(app: &App, row: &DeclRow, tier: Option<&str>, w: usize, sel: bool, focused: bool) -> Line<'static> {
    let t = &app.theme;
    let st = if sel { if focused { t.selected } else { t.selected_unfocused } } else { t.text };
    let mark = if app.bookmarked.contains(&row.name) { app.sym.bookmark } else { " " };
    let right = match (tier, row.origin) {
        (Some(ti), Origin::Library) => format!(" {ti}·lib"),
        (Some(ti), _) => format!(" {ti}"),
        (None, _) => " text".to_string(),
    };
    let head = format!("{mark}{} ", row.kind.short());
    let avail = w.saturating_sub(right.chars().count() + text_width(&head));
    let name = truncate_left(&clean(app, &row.display), avail, app.ascii);
    let mut spans = vec![Span::styled(pad(&format!("{head}{name}"), avail + text_width(&head)), st)];
    spans.push(Span::styled(right, if sel { st } else { t.dim }));
    Line::from(spans)
}

// ---------------------------------------------------------------------------
// Center
// ---------------------------------------------------------------------------

fn draw_center(f: &mut Frame, area: Rect, app: &mut App) {
    let t = app.theme.clone();
    let focused = app.focus == Focus::Center;
    // The declaration selected in the right pane shows its statement here,
    // under the one being read.
    let ch = card_height(app, area);
    let area = if ch > 0 {
        let [main, dock] = Layout::vertical([Constraint::Min(3), Constraint::Length(ch)]).areas(area);
        draw_card(f, dock, app);
        main
    } else {
        area
    };
    if app.center_mode == CenterMode::Source {
        draw_source(f, area, app, focused);
        return;
    }
    if app.center_mode == CenterMode::Skim {
        draw_skim(f, area, app, focused);
        return;
    }
    let b = block(app, Line::from(Span::styled(" Declaration ", t.title)), focused);
    let inner = b.inner(area);
    f.render_widget(b, area);
    app.hit.center = area;
    if inner.width < 10 {
        return;
    }
    app.center.height = inner.height as usize;
    let key = (inner.width, app.center.version);
    let rebuild = match &app.center.cache {
        Some((w, v, _)) => (*w, *v) != key,
        None => true,
    };
    if rebuild {
        let (lines, rows) = detail_layout(app, inner.width as usize);
        app.center.cache = Some((inner.width, app.center.version, lines));
        app.center.excerpt_rows = rows;
    }
    let lines = &app.center.cache.as_ref().unwrap().2;
    let max_scroll = lines.len().saturating_sub(inner.height as usize);
    // An occurrence the reader selected in the Mentions tab comes into view.
    if let Some(line) = app.center.reveal_line.take() {
        if let Some(&(_, row)) = app.center.excerpt_rows.iter().find(|(n, _)| *n == line) {
            let h = inner.height as usize;
            if row < app.center.scroll || row >= app.center.scroll + h {
                app.center.scroll = row.saturating_sub(h / 3);
            }
        }
    }
    app.center.scroll = app.center.scroll.min(max_scroll);
    let visible: Vec<Line> = lines.iter().skip(app.center.scroll).take(inner.height as usize).cloned().collect();
    f.render_widget(Paragraph::new(visible), inner);
    if lines.len() > inner.height as usize && inner.height > 2 {
        let pct = (app.center.scroll + inner.height as usize) * 100 / lines.len().max(1);
        let ind = format!(" {}% ", pct.min(100));
        let x = area.x + area.width.saturating_sub(ind.len() as u16 + 2);
        f.render_widget(Paragraph::new(Span::styled(ind, t.dim)), Rect { x, y: area.y + area.height - 1, width: 8, height: 1 });
    }
}

pub(super) fn heading(app: &App, s: &str, w: usize) -> Line<'static> {
    let s = chrome(app, s);
    let rule = app.sym.rule.repeat(w.saturating_sub(s.chars().count() + 2).min(200));
    Line::from(vec![Span::styled(format!("{s} "), app.theme.heading), Span::styled(rule, app.theme.dim)])
}

pub(super) fn wrapped(app: &App, text: &str, w: usize, style: Style, indent: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for raw in text.split('\n') {
        let line = sanitize(raw, app.ascii);
        let line = format!("{}{}", " ".repeat(indent), line);
        for r in wrap_line(&line, w, 4) {
            out.push(Line::from(Span::styled(r, style)));
        }
    }
    out
}

/// Reflows Markdown-ish documentation: lines inside a paragraph are joined,
/// while blank lines, list items, headings, indented and fenced code keep
/// their breaks.
pub fn reflow_doc(doc: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut para = String::new();
    let mut in_fence = false;
    let flush = |para: &mut String, out: &mut Vec<String>| {
        if !para.is_empty() {
            out.push(std::mem::take(para));
        }
    };
    for raw in doc.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            flush(&mut para, &mut out);
            in_fence = !in_fence;
            out.push(line.to_string());
            continue;
        }
        if in_fence || line.starts_with("    ") {
            flush(&mut para, &mut out);
            out.push(line.to_string());
            continue;
        }
        if trimmed.is_empty() {
            flush(&mut para, &mut out);
            out.push(String::new());
            continue;
        }
        let starts_block = trimmed.starts_with("* ")
            || trimmed.starts_with("- ")
            || trimmed.starts_with('#')
            || trimmed.starts_with('|')
            || trimmed.split_once(". ").is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
        if starts_block {
            flush(&mut para, &mut out);
            para.push_str(line);
        } else if para.is_empty() {
            para.push_str(trimmed);
        } else {
            para.push(' ');
            para.push_str(trimmed);
        }
    }
    flush(&mut para, &mut out);
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out
}

pub(super) fn evidence(app: &App, sym: &str, text: String, style: Style, w: usize) -> Vec<Line<'static>> {
    let full = format!(" {sym} {}", chrome(app, &text));
    wrap_line(&sanitize(&full, app.ascii), w, 4).into_iter().map(|r| Line::from(Span::styled(r, style))).collect()
}

/// Wrapped rows of one line of text, with byte ranges of it drawn in the
/// selection style (an identifier the reader selected elsewhere).
fn marked_text(app: &App, text: &str, marks: &[(u32, u32)], w: usize, style: Style, indent: usize) -> Vec<Line<'static>> {
    crate::highlight::render::styled_rows_marked(text, &[], marks, w.saturating_sub(indent).max(8), 4, app.ascii)
        .into_iter()
        .map(|row| {
            let mut spans = vec![Span::styled(" ".repeat(indent), style)];
            spans.extend(row.into_iter().map(|(s, _, m)| Span::styled(s, if m { app.theme.selected } else { style })));
            Line::from(spans)
        })
        .collect()
}

/// The byte ranges of occurrences on file line `line`, relative to `from`.
fn marks_on(occ: Option<&[Occurrence]>, line: usize, from: usize) -> Vec<(u32, u32)> {
    occ.unwrap_or(&[])
        .iter()
        .filter(|o| o.line as usize == line && o.start as usize >= from)
        .map(|o| (o.start - from as u32, o.end - from as u32))
        .collect()
}

pub(super) fn statement_lines(app: &App, b: &super::worker::DetailBundle, w: usize, occ: Option<&[Occurrence]>) -> Vec<Line<'static>> {
    let t = &app.theme;
    let d = &b.d;
    let mut out = Vec::new();
    let kw = d.keyword.clone().unwrap_or_else(|| d.row.kind.as_str().to_string());
    match (&d.sig, &d.type_text, &b.statement_src) {
        (Some(sig), _, _) => {
            let (groups, hidden) = sig.binder_groups(app.filters.show_implicit);
            let mut text_lines: Vec<String> = vec![format!("{kw} {}", d.row.display)];
            let mut cur = String::from("    ");
            for g in groups {
                let g = g.replace('\n', "\n      ");
                if g.contains('\n') {
                    if cur.trim().len() > 0 {
                        text_lines.push(cur.clone());
                    }
                    text_lines.extend(g.lines().map(|l| format!("    {l}")));
                    cur = String::from("    ");
                } else if text_width(&cur) + text_width(&g) + 1 > w && cur.trim().len() > 0 {
                    text_lines.push(cur.clone());
                    cur = format!("    {g}");
                } else {
                    if cur.trim().len() > 0 {
                        cur.push(' ');
                    }
                    cur.push_str(&g);
                }
            }
            if cur.trim().len() > 0 {
                text_lines.push(format!("{cur} :"));
            } else if let Some(last) = text_lines.last_mut() {
                last.push_str(" :");
            }
            for (i, l) in sig.conclusion.lines().enumerate() {
                text_lines.push(format!("{}{l}", if i == 0 { "    " } else { "      " }));
            }
            for l in text_lines {
                out.extend(wrapped(app, &l, w, t.code, 0));
            }
            if hidden {
                out.push(Line::from(Span::styled("    (implicit and instance arguments hidden)", t.dim)));
            }
        }
        (None, Some(ty), _) => {
            out.extend(wrapped(app, &format!("{kw} {} :", d.row.display), w, t.code, 0));
            out.extend(wrapped(app, ty, w, t.code, 4));
        }
        (None, None, Some(src)) => {
            for (i, l) in src.lines().enumerate() {
                // The header starts mid-line at `statement_at`; later lines are whole.
                let marks = match b.statement_at {
                    Some((line, col)) => marks_on(occ, line + i, if i == 0 { col } else { 0 }),
                    None => Vec::new(),
                };
                if marks.is_empty() {
                    out.extend(wrapped(app, l, w, t.code, 2));
                } else {
                    out.extend(marked_text(app, l, &marks, w, t.code, 2));
                }
            }
        }
        _ => out.push(Line::from(Span::styled("no statement available for this node", t.dim))),
    }
    out
}

pub fn detail_lines(app: &App, w: usize) -> Vec<Line<'static>> {
    detail_layout(app, w).0
}

/// The declaration view's rows, and for each source line of the excerpt the
/// row it starts on (to bring a line into view).
pub fn detail_layout(app: &App, w: usize) -> (Vec<Line<'static>>, Vec<(usize, usize)>) {
    let t = &app.theme;
    let s = &app.sym;
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut excerpt_rows: Vec<(usize, usize)> = Vec::new();
    // Where the identifier selected in the Mentions tab is written.
    let occ = app.mention_marks().map(|o| o.as_slice());
    let Some(b) = &app.detail else {
        if let Some(e) = &app.detail_error {
            out.extend(wrapped(app, &format!("error: {e}"), w, t.danger, 0));
        } else {
            out.push(Line::from(Span::styled("Open a declaration from the outline or search (/); ? lists every key.", t.dim)));
        }
        return (out, excerpt_rows);
    };
    let d = &b.d;
    // Title.
    let mut title = vec![
        Span::styled(format!("{} ", d.row.kind.as_str()), t.kind),
        Span::styled(clean(app, &d.row.display), t.decl_name),
    ];
    if b.bookmarked {
        title.push(Span::styled(format!(" {}", s.bookmark), t.warn));
    }
    out.push(Line::from(title));
    if d.row.name != d.row.display {
        out.extend(wrapped(app, &format!("canonical name: {}", d.row.name), w, t.dim, 0));
    }
    let mut loc = String::new();
    if let Some(m) = &d.row.module {
        loc.push_str(&format!("module {m}"));
    }
    if let Some(p) = &d.module_path {
        let line = b.decl_lines.map(|x| x.0 as u32).or(d.pos.start_line);
        loc.push_str(&format!(" · {p}{}", line.map(|l| format!(":{l}")).unwrap_or_default()));
    }
    loc.push_str(&format!(" · {}", d.row.origin.label()));
    out.extend(wrapped(app, &chrome(app, &loc), w, t.dim, 0));
    if let Some(g) = app.glosses.get(&d.row.name) {
        out.extend(wrapped(app, &format!("= {g}"), w, t.accent, 0));
    }
    out.push(Line::from(""));

    // Conspicuous alerts first, so a placeholder or custom axiom is never
    // below the fold.
    {
        let mode = app.ctx.mode;
        let mut alerts: Vec<(String, Style)> = Vec::new();
        if d.row.kind == crate::model::DeclKind::Axiom {
            alerts.push(("this declaration is an AXIOM (assumed, not proved)".into(), t.danger));
        }
        for a in &d.axioms {
            match a.category {
                AxiomCategory::Sorry => alerts.push(("depends on sorryAx (a placeholder proof in its dependency closure)".into(), t.danger)),
                AxiomCategory::Custom if d.row.kind != crate::model::DeclKind::Axiom || a.name != d.row.name => {
                    alerts.push((format!("depends on custom axiom {}", a.name), t.warn))
                }
                AxiomCategory::CompilerTrust => alerts.push((format!("trusts the compiler: {}", a.name), t.warn)),
                AxiomCategory::Incomplete => alerts.push(("dependency data incomplete: the axiom list is a lower bound".into(), t.warn)),
                _ => {}
            }
        }
        if mode == IndexMode::Source && d.row.flags & flags::LEX_SORRY != 0 {
            alerts.push(("`sorry` appears in its source text (lexical)".into(), t.danger));
        }
        if b.source_status == SourceStatus::Changed && mode == IndexMode::Semantic {
            alerts.push(("source changed since the snapshot: semantic data may be outdated".into(), t.warn));
        }
        if !alerts.is_empty() {
            for (text, style) in alerts {
                out.extend(evidence(app, s.warn, text, style, w));
            }
            out.push(Line::from(""));
        }
    }
    // Evidence (full), shown after the statement and documentation.
    let mut ev: Vec<Line<'static>> = Vec::new();
    let mode = app.ctx.mode;
    let info = app.ctx.info.as_ref();
    match (mode, d.row.origin) {
        (IndexMode::Semantic, Origin::Local) | (IndexMode::Semantic, Origin::Library) => {
            let lean = info.and_then(|i| i.get("lean_version")).unwrap_or("?");
            let when = info.and_then(|i| i.get("created_at")).unwrap_or("?");
            if app.ctx.synthetic {
                ev.extend(evidence(app, s.warn, "SYNTHETIC benchmark node: not produced by Lean".into(), t.warn, w));
            } else {
                ev.extend(evidence(
                    app,
                    s.ok,
                    format!("Lean {lean} snapshot of {when} ({})", if d.row.origin == Origin::Local { "project module" } else { "library module" }),
                    t.ok,
                    w,
                ));
                ev.extend(evidence(
                    app,
                    s.bullet,
                    "build: artifacts present at export; not run or verified by slim; not a kernel replay".into(),
                    t.dim,
                    w,
                ));
            }
        }
        (IndexMode::Semantic, Origin::Unresolved) => {
            ev.extend(evidence(app, s.warn, "referenced by the snapshot, not described in it (coverage gap)".into(), t.warn, w));
        }
        (IndexMode::Source, _) | (_, Origin::SourceCandidate) => {
            ev.extend(evidence(app, s.bullet, "source candidate (lexical scan, not elaborated)".into(), t.text, w));
            if d.row.flags & flags::ANONYMOUS != 0 {
                ev.extend(evidence(app, s.warn, "anonymous: Lean generates its name; shown by its type".into(), t.warn, w));
            }
            if d.same_name > 0 || d.row.flags & flags::AMBIGUOUS != 0 {
                ev.extend(evidence(app, s.warn, format!("ambiguous: {} other candidate(s) share this name", d.same_name), t.warn, w));
            }
            if d.row.flags & flags::LEX_SORRY != 0 {
                ev.extend(evidence(
                    app,
                    s.warn,
                    format!("`sorry`/`admit` token in its text, line {} (lexical)", d.sorry_line.map(|x| x.to_string()).unwrap_or_default()),
                    t.danger,
                    w,
                ));
            } else {
                ev.extend(evidence(app, s.bullet, "no `sorry` token in its text (proves nothing)".into(), t.dim, w));
            }
            if d.row.flags & flags::LEX_NATIVE != 0 {
                ev.extend(evidence(app, s.warn, "uses native_decide (trusts the compiler)".into(), t.warn, w));
            }
            ev.extend(evidence(app, s.bullet, "axioms and proof status: unknown without a semantic snapshot".into(), t.dim, w));
        }
    }
    if d.row.kind == crate::model::DeclKind::Axiom {
        ev.extend(evidence(app, s.warn, "an axiom: assumed, not proved".into(), t.danger, w));
    }
    if let Some(g) = &d.generated {
        ev.extend(evidence(app, s.bullet, format!("generated by Lean: {g}"), t.dim, w));
    }
    if mode == IndexMode::Semantic && d.row.origin != Origin::Unresolved {
        let cov = |c: Coverage| match c {
            Coverage::Complete => "complete".to_string(),
            other => other.as_str().to_string(),
        };
        let st = if d.type_cov.is_incomplete() || (d.row.kind.has_value() && d.value_cov.is_incomplete()) { t.warn } else { t.dim };
        ev.extend(evidence(
            app,
            s.bullet,
            format!(
                "references extracted: statement {} · proof/body {} · {} uses, {} users",
                cov(d.type_cov),
                cov(d.value_cov),
                d.degrees[0] + d.degrees[1] + d.degrees[2],
                d.degrees[3] + d.degrees[4] + d.degrees[5]
            ),
            st,
            w,
        ));
        // Axioms.
        let gap = d.axioms.iter().any(|a| a.category == AxiomCategory::Incomplete);
        let real: Vec<_> = d.axioms.iter().filter(|a| a.category != AxiomCategory::Incomplete).collect();
        for a in &real {
            let (sym, style) = match a.category {
                AxiomCategory::Sorry => (s.warn, t.danger),
                AxiomCategory::Custom | AxiomCategory::CompilerTrust => (s.warn, t.warn),
                _ => (s.bullet, t.text),
            };
            if a.category != AxiomCategory::Standard {
                ev.extend(evidence(app, sym, format!("depends on {} ({})", a.name, a.category.label()), style, w));
            }
        }
        let std: Vec<String> = real.iter().filter(|a| a.category == AxiomCategory::Standard).map(|a| a.name.clone()).collect();
        if !std.is_empty() {
            ev.extend(evidence(app, s.bullet, format!("standard axioms: {}", std.join(", ")), t.text, w));
        }
        if real.is_empty() && !gap {
            ev.extend(evidence(app, s.ok, "no axioms in its dependency closure".into(), t.ok, w));
        }
        if gap {
            ev.extend(evidence(app, s.warn, "dependency closure incomplete: the axiom list is a lower bound".into(), t.warn, w));
        }
        if let Some(la) = &d.lean_axioms {
            let mut a: Vec<String> = real.iter().map(|x| x.name.clone()).collect();
            a.sort();
            let mut l = la.clone();
            l.sort();
            if a == l {
                ev.extend(evidence(app, s.ok, "matches Lean's collectAxioms at export".into(), t.ok, w));
            } else {
                ev.extend(evidence(app, s.warn, format!("Lean's collectAxioms at export reported [{}]", la.join(", ")), t.warn, w));
            }
        }
    }
    // Source freshness.
    match &b.source_status {
        SourceStatus::Ok => {
            if mode == IndexMode::Semantic && d.module_path.is_some() {
                ev.extend(evidence(app, s.ok, "source file unchanged since the snapshot (SHA-256)".into(), t.ok, w));
            }
        }
        SourceStatus::Changed => ev.extend(evidence(
            app,
            s.warn,
            if mode == IndexMode::Semantic {
                "SOURCE CHANGED since the snapshot: the semantic data describes the old source".into()
            } else {
                "source changed since indexing".into()
            },
            t.warn,
            w,
        )),
        SourceStatus::Missing(e) => ev.extend(evidence(app, s.warn, format!("source unavailable: {e}"), t.warn, w)),
        SourceStatus::Rejected(e) => ev.extend(evidence(app, s.stop, format!("source path rejected: {e}"), t.danger, w)),
        SourceStatus::NoRoot => ev.extend(evidence(app, s.bullet, "no approved source root for this index".into(), t.dim, w)),
        SourceStatus::NoPath => {}
    }
    if d.module_status.as_deref() == Some("artifact-older-than-source") {
        ev.extend(evidence(app, s.warn, ".olean older than the source at export: the snapshot may not match the source".into(), t.warn, w));
    }
    ev.push(Line::from(""));

    // Statement.
    out.push(heading(app, if mode == IndexMode::Semantic { "Statement (elaborated)" } else { "Statement (source text, not elaborated)" }, w));
    out.extend(statement_lines(app, b, w, occ));
    out.push(Line::from(""));
    out.extend(scope_lines(app, d.row.id, w));

    // Documentation.
    if let Some(doc) = d.doc.as_ref().filter(|x| !x.trim().is_empty()) {
        out.push(heading(app, "Documentation", w));
        for para in reflow_doc(doc.trim()) {
            let hang = if para.trim_start().starts_with("* ") || para.trim_start().starts_with("- ") { 2 } else { 0 };
            for r in wrap_line(&sanitize(&format!(" {para}"), app.ascii), w, hang) {
                out.push(Line::from(Span::styled(r, t.text)));
            }
        }
        out.push(Line::from(""));
    }
    if let Some(a) = &d.attrs {
        out.extend(wrapped(app, &format!("attributes: @[{a}]"), w, t.dim, 0));
        out.push(Line::from(""));
    }

    out.push(heading(app, "Status & evidence", w));
    out.extend(ev);
    out.push(Line::from(""));
    // Source excerpt.
    if !b.excerpt.is_empty() {
        let rng = b.decl_lines.map(|(a, z)| format!(":{a}–{z}")).unwrap_or_default();
        out.push(heading(app, &format!("Source {}{rng}", d.module_path.clone().unwrap_or_default()), w));
        if b.source_status == SourceStatus::Changed {
            out.extend(evidence(
                app,
                s.warn,
                "file changed since indexing: these lines may not be the indexed declaration".into(),
                t.warn,
                w,
            ));
        }
        let gw = b.excerpt.last().map(|(n, _)| n.to_string().len()).unwrap_or(3).max(3);
        // Syntax colours for the declaration's own lines; the context lines
        // around it stay dim.
        let hl = match (&d.module_path, &b.source_sha256) {
            (Some(rel), Some(sha)) => app.hl_for(rel, sha).cloned(),
            _ => None,
        };
        for (n, text) in &b.excerpt {
            let inside = b.decl_lines.is_some_and(|(a, z)| *n >= a && *n <= z);
            let prefix = format!("{n:>gw$} {} ", s.gutter);
            let cont = format!("{} {} ", " ".repeat(gw), s.gutter);
            let body_w = w.saturating_sub(prefix.chars().count()).max(8);
            let rows = if inside {
                app.hl_rows_marked(hl.as_deref(), n - 1, text, body_w, t.code, false, &marks_on(occ, *n, 0))
            } else {
                wrap_line(&sanitize(text, app.ascii), body_w, 2).into_iter().map(|r| vec![Span::styled(r, t.dim)]).collect()
            };
            excerpt_rows.push((*n, out.len()));
            for (i, r) in rows.into_iter().enumerate() {
                let mut spans = vec![Span::styled(if i == 0 { prefix.clone() } else { cont.clone() }, t.dim)];
                spans.extend(r);
                out.push(Line::from(spans));
            }
        }
        if b.excerpt_truncated {
            out.push(Line::from(Span::styled("… excerpt truncated", t.dim)));
        }
    }
    (out, excerpt_rows)
}

/// The skim: statements of a file, directory, namespace or map group, with
/// the first line of their documentation and nothing else.
fn draw_skim(f: &mut Frame, area: Rect, app: &mut App, focused: bool) {
    let t = app.theme.clone();
    let room = (area.width as usize).saturating_sub(4);
    let title = match (&app.center.skim.data, &app.center.skim.scope) {
        (Some(s), _) => {
            // Statements only; whether they are source text or elaborated is said here.
            let what = if app.ctx.mode == IndexMode::Source { "source text" } else { "elaborated" };
            let tail = format!(" {} {} statements {} {what} ", app.sym.sep, s.decls, app.sym.sep);
            let head = truncate_left(&clean(app, &s.title), room.saturating_sub(6 + text_width(&tail)).max(10), app.ascii);
            vec![Span::styled(format!(" Skim {head}"), t.title), Span::styled(chrome(app, &tail), t.dim)]
        }
        (None, Some(sc)) => vec![Span::styled(format!(" Skim {} ", truncate_left(&clean(app, &sc.label()), room.saturating_sub(6).max(10), app.ascii)), t.title)],
        _ => vec![Span::styled(" Skim ", t.title)],
    };
    let b = block(app, Line::from(title), focused);
    let inner = b.inner(area);
    f.render_widget(b, area);
    app.hit.center = area;
    if inner.width < 10 || inner.height == 0 {
        return;
    }
    let h = inner.height as usize;
    app.center.skim.height = h;
    if app.center.skim.data.is_none() {
        let line = match &app.center.skim.error {
            Some(e) => Line::from(Span::styled(clean(app, &format!("cannot skim: {e}")), t.danger)),
            None => Line::from(Span::styled("loading…", t.dim)),
        };
        f.render_widget(Paragraph::new(line), inner);
        return;
    }
    let key = (inner.width, app.center.skim.version);
    if app.center.skim.cache.as_ref().is_none_or(|(w, v, _, _)| (*w, *v) != key) {
        let (lines, ranges) = skim_layout(app, inner.width as usize);
        app.center.skim.cache = Some((inner.width, app.center.skim.version, lines, ranges));
    }
    let sk = &mut app.center.skim;
    let (_, _, lines, ranges) = sk.cache.as_ref().unwrap();
    // Keep the selected declaration on screen, with the file header above
    // it when that fits.
    if let Some(&(a, z)) = ranges.get(sk.sel).filter(|_| !sk.from_top) {
        let mut top = a;
        let mut k = sk.sel;
        while k > 0 && !matches!(sk.data.as_ref().unwrap().items[k - 1], SkimItem::Decl(_)) {
            k -= 1;
            top = ranges[k].0;
        }
        if k == 0 {
            top = 0;
        }
        if top < sk.scroll {
            sk.scroll = if z - top <= h { top } else { a };
        } else if a < sk.scroll {
            sk.scroll = a;
        } else if z > sk.scroll + h {
            sk.scroll = (z.saturating_sub(h)).min(a);
        }
    }
    sk.scroll = sk.scroll.min(lines.len().saturating_sub(h.min(lines.len())));
    let sel = ranges.get(sk.sel).copied().filter(|_| matches!(sk.data.as_ref().unwrap().items.get(sk.sel), Some(SkimItem::Decl(_))));
    let mark_style = if focused { t.accent.add_modifier(Modifier::BOLD) } else { t.dim };
    let visible: Vec<Line> = lines
        .iter()
        .enumerate()
        .skip(sk.scroll)
        .take(h)
        .map(|(i, l)| match sel {
            Some((a, z)) if i >= a && i < z => {
                // The gutter is the first span of every row of a declaration.
                let mut l = l.clone();
                for sp in l.spans.iter_mut() {
                    sp.style = sp.style.patch(t.highlight_line);
                }
                if let Some(g) = l.spans.first_mut() {
                    if i == a {
                        *g = Span::styled(format!("{} ", app.sym.collapsed), mark_style.patch(t.highlight_line));
                    }
                }
                l
            }
            _ => l.clone(),
        })
        .collect();
    let total = lines.len();
    f.render_widget(Paragraph::new(visible), inner);
    let items = app.center.skim.decl_items();
    if !items.is_empty() && inner.height > 2 {
        let pos = items.partition_point(|&k| k < app.center.skim.sel).min(items.len() - 1) + 1;
        let ind = format!(" {pos}/{} ", items.len());
        let iw = ind.len() as u16;
        if area.width > iw + 4 && total > 0 {
            f.render_widget(Paragraph::new(Span::styled(ind, t.dim)), Rect { x: area.x + area.width - iw - 2, y: area.y + area.height - 1, width: iw, height: 1 });
        }
    }
}

/// Rows of the skim for a pane width, and the row range of each item.
fn skim_layout(app: &App, w: usize) -> (Vec<Line<'static>>, Vec<(usize, usize)>) {
    let t = &app.theme;
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let Some(sk) = &app.center.skim.data else { return (out, ranges) };
    if let Some(st) = &sk.stats {
        let mut line = crate::skim::counts_line(st, app.ctx.mode);
        if sk.files > 1 {
            line = format!("{} files · {line}", sk.files);
        }
        if st.lines > 0 {
            line.push_str(&format!(" · {} lines", compact(st.lines as u64)));
        }
        if st.proof_lines > 0 {
            line.push_str(&format!(", {} of them in theorem proofs", compact(st.proof_lines as u64)));
        }
        let named = st.named();
        if named > 0 {
            line.push_str(&format!(" · {} of {named} documented", st.doc));
        }
        out.extend(wrapped(app, &chrome(app, &line), w, t.dim, 1));
    }
    if let Some(why) = &sk.truncated {
        out.extend(evidence(app, app.sym.warn, format!("not the whole scope: {why}"), t.warn, w));
    }
    let body_w = w.saturating_sub(2).max(8);
    for item in &sk.items {
        let start = out.len();
        match item {
            SkimItem::Header { title, sub, doc, warn } => {
                out.push(Line::from(""));
                out.push(heading(app, &clean(app, title), w));
                if !sub.is_empty() {
                    out.extend(wrapped(app, &chrome(app, sub), w, t.dim, 1));
                }
                if let Some(d) = doc {
                    out.extend(wrapped(app, d, w, t.text, 1));
                }
                if let Some(x) = warn {
                    out.extend(evidence(app, app.sym.warn, x.clone(), t.warn, w));
                }
            }
            SkimItem::Decl(d) => {
                let nested = if matches!(d.row.kind, crate::model::DeclKind::Constructor) { 2 } else { 0 };
                out.extend(sig_rows(app, &d.sig, body_w, nested));
                if d.sig_cut {
                    out.push(Line::from(vec![Span::styled("  ", t.code), Span::styled(chrome(app, "    … statement cut"), t.dim)]));
                }
                if let Some(g) = app.glosses.get(&d.row.name) {
                    let text = truncate(&format!("    = {}", sanitize(g, app.ascii)), body_w, app.ascii);
                    out.push(Line::from(vec![Span::styled("  ", t.code), Span::styled(text, t.accent)]));
                }
                // One more row for what was left out: the first line of the
                // documentation, and how much text is folded away.
                let mut meta: Vec<String> = Vec::new();
                if d.sorry {
                    meta.push("sorry token".into());
                }
                if d.folded > 0 {
                    meta.push(format!("{} line{} folded", d.folded, if d.folded == 1 { "" } else { "s" }));
                }
                if d.doc.is_some() || !meta.is_empty() {
                    let right = if meta.is_empty() { String::new() } else { format!(" {}", meta.join(&format!(" {} ", app.sym.sep))) };
                    let avail = body_w.saturating_sub(text_width(&right) + 4);
                    let doc = truncate(&sanitize(d.doc.as_deref().unwrap_or(""), app.ascii), avail, app.ascii);
                    out.push(Line::from(vec![
                        Span::styled("  ", t.code),
                        Span::styled(pad(&format!("    {doc}"), avail + 4), t.dim),
                        Span::styled(right, if d.sorry { t.warn } else { t.dim }),
                    ]));
                }
            }
            SkimItem::Note { text, warn } => out.extend(wrapped(app, &chrome(app, text), w, if *warn { t.warn } else { t.dim }, 3)),
            SkimItem::Prose(p) => {
                for part in &p.parts {
                    if let Some((_, title, _)) = &part.heading {
                        out.push(Line::from(""));
                        out.extend(wrapped(app, title, w, t.heading, 2));
                    }
                    if let Some(l) = &part.lead {
                        let cut = if part.lead_cut { format!(" {}", app.sym.ellipsis) } else { String::new() };
                        out.extend(wrapped(app, &format!("{l}{cut}"), w, t.text, 2));
                    }
                }
                if p.folded > 0 {
                    out.push(Line::from(Span::styled(format!("  {} line{} folded", p.folded, if p.folded == 1 { "" } else { "s" }), t.dim)));
                }
            }
            SkimItem::Scope { text } => out.extend(sig_rows(app, text, body_w, 0).into_iter().map(|mut l| {
                // Context, not a statement: its text stays dim.
                if let Some(sp) = l.spans.last_mut() {
                    sp.style = t.dim;
                }
                l
            })),
        }
        ranges.push((start, out.len()));
    }
    (out, ranges)
}

/// What is said beside a name, on one row: the reader's gloss (`= …`).
/// A card also falls back on the purpose the project map gives the name
/// (`map: …`); lists stay as short as the reader's own words.
pub(super) fn gloss_row(app: &App, name: &str, w: usize, indent: usize, with_map: bool) -> Option<Line<'static>> {
    let (text, own) = app.gloss_of(name).filter(|(_, own)| *own || with_map)?;
    let s = format!("{}{}{}", " ".repeat(indent), if own { "= " } else { "map: " }, sanitize(text, app.ascii));
    Some(Line::from(Span::styled(truncate(&s, w, app.ascii), if own { app.theme.accent } else { app.theme.dim })))
}

/// A statement as rows of the pane: one that is written over several lines
/// is flowed as one paragraph, unless it holds a comment (joining lines
/// would pull the following text into it). Its keyword is coloured.
fn sig_rows(app: &App, sig: &str, body_w: usize, nested: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let mut out = Vec::new();
    let mut first = true;
    let flowed;
    let sig_lines: Vec<&str> = if sig.contains('\n') && !sig.contains("--") && !sig.contains("/-") {
        flowed = sig.split_whitespace().collect::<Vec<_>>().join(" ");
        vec![flowed.as_str()]
    } else {
        sig.lines().collect()
    };
    for src_line in sig_lines {
        let text = format!("{}{}", " ".repeat(nested), sanitize(src_line, app.ascii));
        for row in wrap_line(&text, body_w, 4) {
            let mut spans = vec![Span::styled("  ", t.code)];
            let kw_len = if first { row.trim_start().find(' ').map(|k| k + row.len() - row.trim_start().len()) } else { None };
            match kw_len {
                Some(k) => {
                    spans.push(Span::styled(row[..k].to_string(), t.kind));
                    spans.push(Span::styled(row[k..].to_string(), t.code));
                }
                None => spans.push(Span::styled(row, t.code)),
            }
            first = false;
            out.push(Line::from(spans));
        }
    }
    out
}

/// The `variable` and `open` lines a declaration is written under.
pub(super) fn scope_lines(app: &App, id: i64, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let Some(sc) = app.scopes.get(&id).filter(|s| !s.is_empty()) else { return Vec::new() };
    let mut out = vec![heading(app, "In scope (source text)", w)];
    for l in &sc.lines {
        out.extend(sig_rows(app, &l.text, w.saturating_sub(2).max(8), 0));
    }
    if sc.hidden > 0 {
        out.push(Line::from(Span::styled(format!("  {} more variable{} it does not write", sc.hidden, if sc.hidden == 1 { "" } else { "s" }), t.dim)));
    }
    out.push(Line::from(""));
    out
}

/// The card of the declaration selected in the right pane: where it is
/// written, its statement and the lead of its documentation.
struct Card {
    title: Line<'static>,
    body: Vec<Line<'static>>,
}

fn card(app: &App, w: usize, max: usize) -> Option<Card> {
    let t = &app.theme;
    let id = app.card_target()?;
    let row = match app.right.rows.get(app.right.list.selected)? {
        RRow::Item { row, .. } | RRow::Mention { row, .. } | RRow::Evidence { row, .. } => row,
        _ => return None,
    };
    let peek = app.cards.get(&id);
    let at = match peek {
        Some(p) => match (&p.path, p.decl.line) {
            (Some(path), Some(l)) => format!("{path}:{l}"),
            (Some(path), None) => path.clone(),
            (None, _) => row.module.clone().unwrap_or_default(),
        },
        None => row.module.clone().unwrap_or_default(),
    };
    let head = format!(" {} ", row.kind.short().trim_end());
    let room = w.saturating_sub(text_width(&head) + 4);
    let name = truncate_left(&clean(app, &row.name), room, app.ascii);
    let at_room = room.saturating_sub(text_width(&name) + 3);
    let mut title = vec![Span::styled(head, t.kind), Span::styled(format!("{name} "), t.decl_name)];
    if !at.is_empty() && at_room >= 12 {
        title.push(Span::styled(format!("{} {} ", app.sym.sep, truncate_left(&clean(app, &at), at_room, app.ascii)), t.dim));
    }
    let mut body: Vec<Line<'static>> = Vec::new();
    match peek {
        None => body.push(Line::from(Span::styled(chrome(app, "  …"), t.dim))),
        Some(p) => {
            body.extend(gloss_row(app, &row.name, w, 2, true));
            let max = max.saturating_sub(body.len()).max(1);
            let doc: Vec<Line<'static>> = p.decl.doc.as_deref().map(|d| wrapped(app, d, w, t.dim, 2)).unwrap_or_default();
            let mut sig = sig_rows(app, &p.decl.sig, w.saturating_sub(2).max(8), 0);
            // The statement first; the documentation keeps a row when both do not fit.
            let keep_doc = if sig.len() + doc.len() > max { doc.len().min(1) } else { doc.len() };
            let sig_room = max.saturating_sub(keep_doc).max(1);
            if sig.len() > sig_room || p.decl.sig_cut {
                sig.truncate(sig_room.saturating_sub(1).max(1));
                sig.push(Line::from(Span::styled(chrome(app, "      … statement cut"), t.dim)));
            }
            body.extend(sig);
            body.extend(doc.into_iter().take(keep_doc));
        }
    }
    body.truncate(max);
    Some(Card { title: Line::from(title), body })
}

/// Height of the card under the centre pane (borders included), when a
/// declaration is selected in the right pane beside it.
fn card_height(app: &App, area: Rect) -> u16 {
    if app.card_target().is_none() || area.height < 14 {
        return 0;
    }
    (area.height / 3).clamp(5, 10)
}

fn draw_card(f: &mut Frame, area: Rect, app: &App) {
    let Some(c) = card(app, area.width.saturating_sub(2) as usize, area.height.saturating_sub(2) as usize) else { return };
    let b = block(app, c.title, false);
    let inner = b.inner(area);
    f.render_widget(b, area);
    f.render_widget(Paragraph::new(c.body), inner);
}

fn draw_source(f: &mut Frame, area: Rect, app: &mut App, focused: bool) {
    let t = app.theme.clone();
    // Highlight data is used only for exactly the text on screen.
    let hl = app.center.source.as_ref().and_then(|s| app.hl_for(&s.rel_path, &s.sha256).cloned());
    let mut title = Vec::new();
    match &app.center.source {
        Some(src) => {
            // What the colours are must stay readable: the path gives way
            // first (keeping its end), then the label shortens.
            let (label, short, warn) = app.hl_label(hl.as_deref());
            let room = (area.width as usize).saturating_sub(4);
            let path = clean(app, &src.rel_path);
            let mut label = chrome(app, &label);
            if 9 + 14.min(text_width(&path)) + 3 + text_width(&label) > room {
                label = short.to_string();
            }
            let label_w = text_width(&label) + 3;
            let path = truncate_left(&path, room.saturating_sub(9 + label_w).max(8), app.ascii);
            title.push(Span::styled(format!(" Source {path} "), t.title));
            title.push(Span::styled(format!("{} {label} ", app.sym.sep), if warn { t.warn } else { t.dim }));
        }
        None => title.push(Span::styled(" Source ", t.title)),
    }
    let b = block(app, Line::from(title), focused);
    let inner = b.inner(area);
    f.render_widget(b, area);
    app.hit.center = area;
    let Some(src) = &app.center.source else {
        f.render_widget(Paragraph::new(Span::styled("loading…", t.dim)), inner);
        return;
    };
    let w = inner.width as usize;
    let gw = src.lines.len().to_string().len().max(3);
    let mut lines = Vec::new();
    if src.changed {
        lines.push(Line::from(Span::styled(format!("{} file changed since the index was built", app.sym.warn), t.warn)));
    }
    let occ = app.mention_marks().map(|o| o.as_slice());
    let mut n = app.center.source_scroll;
    while lines.len() < inner.height as usize && n < src.lines.len() {
        let lineno = n + 1;
        let inside = src.highlight.is_some_and(|(a, z)| lineno >= a && lineno <= z);
        let prefix = format!("{lineno:>gw$} {} ", app.sym.gutter);
        let body_w = w.saturating_sub(prefix.chars().count()).max(8);
        // Compositing: base text, syntax, semantic, then the line overlay's
        // background (kept under the token colours).
        let base = if inside { t.code.patch(t.highlight_line) } else { t.text };
        for (i, r) in app.hl_rows_marked(hl.as_deref(), n, &src.lines[n], body_w, base, inside, &marks_on(occ, lineno, 0)).into_iter().enumerate() {
            if lines.len() >= inner.height as usize {
                break;
            }
            let p = if i == 0 { prefix.clone() } else { format!("{} {} ", " ".repeat(gw), app.sym.gutter) };
            let mut spans = vec![Span::styled(p, if inside { t.accent } else { t.dim })];
            spans.extend(r);
            lines.push(Line::from(spans));
        }
        n += 1;
    }
    f.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------------
// Right
// ---------------------------------------------------------------------------

fn draw_right(f: &mut Frame, area: Rect, app: &mut App) {
    let t = app.theme.clone();
    let focused = app.focus == Focus::Right;
    let mut tabs: Vec<Span> = vec![Span::raw(" ")];
    let mut tab_ranges = Vec::new();
    let mut x = area.x + 2;
    // Pick the longest label set that fits in the top border.
    let fits = |level: u8| -> bool {
        let total: usize = RightTab::ALL
            .iter()
            .enumerate()
            .map(|(i, tab)| {
                let l = format!("{} {}", i + 1, tab.short_label(app.ctx.mode, level));
                text_width(&l) + if *tab == app.right_tab { 3 } else { 1 }
            })
            .sum();
        total + 1 <= area.width.saturating_sub(2) as usize
    };
    // Level 3 (last resort): only the active tab is named, the others are
    // shown by their key.
    let level = if fits(0) { 0 } else if fits(1) { 1 } else if fits(2) { 2 } else { 3 };
    for (i, tab) in RightTab::ALL.iter().enumerate() {
        let label = if level == 3 && *tab != app.right_tab {
            format!("{}", i + 1)
        } else {
            format!("{} {}", i + 1, tab.short_label(app.ctx.mode, level.min(2)))
        };
        let shown = if *tab == app.right_tab { format!("[{label}]") } else { label };
        let w = text_width(&shown) as u16;
        tab_ranges.push((x, x + w, *tab));
        x += w + 1;
        if *tab == app.right_tab {
            tabs.push(Span::styled(shown, t.tab_active));
        } else {
            tabs.push(Span::styled(shown, t.dim));
        }
        tabs.push(Span::raw(" "));
    }
    let b = block(app, Line::from(tabs), focused);
    let inner = b.inner(area);
    f.render_widget(b, area);
    app.hit.right = area;
    app.hit.right_tabs = tab_ranges;
    app.hit.right_list = Rect::default();
    app.hit.right_line_rows.clear();
    if inner.height < 3 || inner.width < 10 {
        return;
    }
    let w = inner.width as usize;
    // Without a centre pane beside it, the selected declaration's card
    // stands under the list.
    let under = if app.card_under_list { card(app, w, (inner.height as usize / 3).clamp(2, 9)) } else { None };
    let card_h = under.as_ref().map(|c| c.body.len() as u16 + 1).unwrap_or(0);
    let explain_h = if matches!(app.right_tab, RightTab::Uses | RightTab::UsedBy | RightTab::Axioms) { 4 + card_h } else { 0 };
    let [list_area, expl_area] = Layout::vertical([Constraint::Min(1), Constraint::Length(explain_h)]).areas(inner);
    app.hit.right_list = list_area;
    let h = list_area.height as usize;
    let mut lines: Vec<Line> = Vec::new();
    let mut line_rows: Vec<Option<usize>> = Vec::new();
    if app.current.is_none() && !matches!(app.right_tab, RightTab::Info) {
        lines.push(Line::from(Span::styled("No declaration selected.", t.dim)));
    }
    if app.right.loading && app.right.rows.is_empty() {
        lines.push(Line::from(Span::styled("loading…", t.dim)));
    }
    if let Some(e) = &app.right.error {
        lines.extend(wrapped(app, &format!("error: {e}"), w, t.danger, 0));
    }
    line_rows.resize(lines.len(), None);
    // Rows may wrap. Render only the rows that can be on screen: from the
    // scroll position through the selection (shifting the window until the
    // selection fits), then until the pane is full.
    let n = app.right.rows.len();
    let sel = app.right.list.selected.min(n.saturating_sub(1));
    let mut scroll = app.right.list.scroll.min(sel);
    let stale = app.right.stale;
    let render_row = |k: usize| -> Vec<Line<'static>> {
        let mut l = right_row_lines(app, &app.right.rows[k], w, k == sel && focused && !stale, k == sel && !stale);
        if stale {
            for line in l.iter_mut() {
                for sp in line.spans.iter_mut() {
                    sp.style = sp.style.patch(t.dim);
                }
            }
        }
        l
    };
    let mut window: std::collections::VecDeque<(usize, Vec<Line<'static>>)> = std::collections::VecDeque::new();
    let mut total = 0usize;
    let mut k = scroll;
    while n > 0 && k <= sel {
        let l = render_row(k);
        total += l.len();
        window.push_back((k, l));
        k += 1;
    }
    while total > h && scroll < sel {
        if let Some((_, l)) = window.pop_front() {
            total -= l.len();
        }
        scroll += 1;
    }
    while total < h && k < n {
        let l = render_row(k);
        total += l.len();
        window.push_back((k, l));
        k += 1;
    }
    app.right.list.scroll = scroll;
    for (k, rl) in window {
        for l in rl {
            if lines.len() >= h {
                break;
            }
            lines.push(l);
            line_rows.push(Some(k));
        }
        if lines.len() >= h {
            break;
        }
    }
    app.hit.right_line_rows = line_rows;
    f.render_widget(Paragraph::new(lines), list_area);
    if explain_h > 0 {
        let txt = explain_selected(app);
        let mut el = vec![Line::from(Span::styled(app.sym.rule.repeat(w), t.dim))];
        if let Some(c) = under {
            el.push(c.title);
            el.extend(c.body);
        }
        el.extend(wrap_line(&chrome(app, &sanitize(&txt, app.ascii)), w, 0).into_iter().take(3).map(|r| Line::from(Span::styled(r, t.dim))));
        f.render_widget(Paragraph::new(el), expl_area);
    }
}

fn tag_style(app: &App, k: EdgeKind) -> Style {
    match k {
        EdgeKind::Type => app.theme.tag_type,
        EdgeKind::Value => app.theme.tag_value,
        EdgeKind::Ctor => app.theme.tag_ctor,
    }
}

fn right_row_lines(app: &App, r: &RRow, w: usize, sel_focused: bool, sel: bool) -> Vec<Line<'static>> {
    let t = &app.theme;
    let st_sel = if sel_focused { t.selected } else if sel { t.selected_unfocused } else { t.text };
    match r {
        RRow::Header { text, kind } => {
            let style = match kind {
                Some(k) => tag_style(app, *k).add_modifier(Modifier::BOLD),
                None => t.heading,
            };
            wrap_line(&chrome(app, &sanitize(text, app.ascii)), w, 2).into_iter().map(|r| Line::from(Span::styled(r, style))).collect()
        }
        RRow::Text { text, warn } => wrap_line(&chrome(app, &sanitize(&format!(" {text}"), app.ascii)), w, 1)
            .into_iter()
            .map(|r| Line::from(Span::styled(r, if *warn { t.warn } else { t.dim })))
            .collect(),
        RRow::Item { row, kind, depth, expanded, cycle, .. } => {
            let ind = "  ".repeat(*depth as usize);
            let mark = if *cycle {
                app.sym.cycle.to_string()
            } else if *expanded {
                app.sym.expanded.to_string()
            } else {
                app.sym.collapsed.to_string()
            };
            let right = match row.origin {
                Origin::Local | Origin::SourceCandidate => String::new(),
                o => format!(" {}", origin_label(o)),
            };
            let bm = if app.bookmarked.contains(&row.name) { app.sym.bookmark } else { "" };
            let head = format!("{ind}{mark} {} {}{} ", kind.tag(), row.kind.short(), bm);
            let avail = w.saturating_sub(right.chars().count() + text_width(&head));
            let name = truncate_left(&clean(app, &row.display), avail, app.ascii);
            let mut v = vec![Line::from(vec![
                Span::styled(pad(&format!("{head}{name}"), avail + text_width(&head)), if sel { st_sel } else { t.text }),
                Span::styled(right, if sel { st_sel } else if row.origin == Origin::Unresolved { t.warn } else { t.dim }),
            ])];
            v.extend(gloss_row(app, &row.name, w, text_width(&ind) + 4, false));
            v
        }
        RRow::More { remaining, depth, .. } => {
            let text = format!("{}{} {} more — ⏎ to load", "  ".repeat(*depth as usize), app.sym.ellipsis, remaining);
            vec![Line::from(Span::styled(pad(&truncate(&text, w, app.ascii), w), if sel { st_sel } else { t.accent }))]
        }
        RRow::Evidence { row, step, kind_in, .. } => {
            let lead = match kind_in {
                Some(k) => format!("{}{} ", app.sym.child, k.tag()),
                None => format!("{} ", app.sym.bullet),
            };
            let head = format!(" {}{lead}", " ".repeat((*step).min(6)));
            let avail = w.saturating_sub(text_width(&head));
            let name = truncate_left(&clean(app, &row.display), avail, app.ascii);
            let mut v = vec![Line::from(Span::styled(pad(&format!("{head}{name}"), w), if sel { st_sel } else { t.text }))];
            v.extend(gloss_row(app, &row.name, w, text_width(&head) + 2, false));
            v
        }
        RRow::Mention { ident, row, also, .. } => {
            let text = format!("{} {} {} {}", clean(app, ident), app.sym.arrow, row.kind.short(), clean(app, &row.display));
            // Other declarations have this name: the nearest file is listed.
            let right = if *also > 0 { format!(" +{also}") } else { String::new() };
            let avail = w.saturating_sub(right.len());
            let mut v = vec![Line::from(vec![
                Span::styled(pad(&truncate(&text, avail, app.ascii), avail), if sel { st_sel } else { t.text }),
                Span::styled(right, if sel { st_sel } else { t.dim }),
            ])];
            v.extend(gloss_row(app, &row.name, w, 4, false));
            v
        }
        RRow::Module { module, current: true, .. } => {
            let text = format!(" {} show {} in the outline (⏎)", app.sym.arrow, clean(app, &module.name));
            vec![Line::from(Span::styled(pad(&truncate(&chrome(app, &text), w, app.ascii), w), if sel { st_sel } else { t.accent }))]
        }
        RRow::Module { module, flags, .. } => {
            let mut f = Vec::new();
            if flags & 1 != 0 {
                f.push("public");
            }
            if flags & 2 != 0 {
                f.push("all");
            }
            if flags & 4 != 0 {
                f.push("meta");
            }
            let tag = if module.is_local { "" } else { " (external)" };
            let text = format!(" {} {}{}{}", app.sym.bullet, clean(app, &module.name), tag, if f.is_empty() { String::new() } else { format!(" [{}]", f.join(" ")) });
            vec![Line::from(Span::styled(pad(&truncate(&text, w, app.ascii), w), if sel { st_sel } else { t.text }))]
        }
    }
}

/// "What does this arrow mean?" for the selected right-pane row.
fn explain_selected(app: &App) -> String {
    let cur = app.detail.as_ref().map(|d| short_name(&d.d.row.display)).unwrap_or_default();
    let parent_name = |pid: i64| -> String {
        if Some(pid) == app.current {
            return cur.clone();
        }
        for r in &app.right.rows {
            if let RRow::Item { row, .. } = r {
                if row.id == pid {
                    return short_name(&row.display);
                }
            }
        }
        "its parent".into()
    };
    match app.right.rows.get(app.right.list.selected) {
        Some(RRow::Item { row, kind, dir, parent_id, cycle, .. }) => {
            let (p, x) = (parent_name(*parent_id), short_name(&row.display));
            let mut s = match (dir, kind) {
                (Direction::Uses, EdgeKind::Type) => format!("{x} occurs in the statement of {p}"),
                (Direction::Uses, EdgeKind::Value) => format!("{x} occurs in the proof term or body of {p}"),
                (Direction::Uses, EdgeKind::Ctor) => format!("{x} is a constructor of {p}"),
                (Direction::UsedBy, EdgeKind::Type) => format!("the statement of {x} mentions {p}"),
                (Direction::UsedBy, EdgeKind::Value) => format!("the proof term or body of {x} uses {p}"),
                (Direction::UsedBy, EdgeKind::Ctor) => format!("{x} is the inductive type of constructor {p}"),
            };
            if *cycle {
                s.push_str(" (cycle: already on this branch)");
            }
            s
        }
        Some(RRow::Evidence { row, kind_next, step, axiom, category, .. }) => {
            let next = match kind_next {
                Some(EdgeKind::Type) => "its statement mentions the next step".to_string(),
                Some(EdgeKind::Value) => "its proof term or body uses the next step".to_string(),
                Some(EdgeKind::Ctor) => "the next step is one of its constructors".to_string(),
                None if *category == AxiomCategory::Incomplete => "its term data is unavailable (coverage gap)".to_string(),
                None => format!("the axiom: {}", category.explanation(axiom)),
            };
            format!("step {step}: {}: {next}", short_name(&row.display))
        }
        Some(RRow::More { .. }) => "neighbour lists load in pages".into(),
        Some(RRow::Mention { row, ident, occ, also }) => {
            // Where it is written: the same places the centre pane marks.
            let mut lines: Vec<u32> = occ.iter().map(|o| o.line).collect();
            lines.dedup();
            let at = match lines.len() {
                0 => String::new(),
                1 if occ.len() == 1 => format!(" on line {}", lines[0]),
                1 => format!(" {}× on line {}", occ.len(), lines[0]),
                n => format!(" {}× on lines {}{}", occ.len(), lines.iter().take(6).map(|l| l.to_string()).collect::<Vec<_>>().join(", "), if n > 6 { ", …" } else { "" }),
            };
            let more = match also {
                0 => String::new(),
                1 => "; 1 other declaration has this name (nearest file listed)".into(),
                n => format!("; {n} other declarations have this name (nearest file listed)"),
            };
            format!("`{ident}`{at}; lexical match with {}{more}", row.display)
        }
        _ => match app.right_tab {
            RightTab::Uses if app.ctx.mode == IndexMode::Semantic => "T statement · V proof term or body · C constructor".into(),
            RightTab::UsedBy if app.ctx.mode == IndexMode::Semantic => "declarations whose statement or proof term mentions this one".into(),
            RightTab::Axioms if app.ctx.mode == IndexMode::Semantic => "each axiom with a path of recorded edges".into(),
            _ => String::new(),
        },
    }
}

// ---------------------------------------------------------------------------
// Status bar and overlays
// ---------------------------------------------------------------------------

fn draw_status(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let line = if let Some((m, warn)) = &app.status {
        Line::from(Span::styled(format!(" {}", chrome(app, &clean(app, m))), if *warn { t.warn } else { t.accent }))
    } else {
        use super::views::{VFocus, View};
        let view_hints = match (app.v.view, app.v.focus, app.v.single_pane) {
            (View::Browse, _, _) => None,
            (_, VFocus::Detail, true) => Some("Tab back to the outline · j/k scroll · w wrap · ⏎ open · Esc declarations · ? help"),
            (_, VFocus::Detail, false) => Some("j/k scroll · w wrap · < > sideways · Tab outline · ⏎ open · Esc declarations · ? help"),
            (View::Map, _, true) => Some("⏎ open · Space expand · Tab detail · * goal · a trail · r reload · ? help"),
            (View::Map, _, false) => Some("⏎ open · Space expand · * goal · a trail · m bookmark · Q question · r reload map · A goal · T trails · ? help"),
            (View::Goal, _, true) => Some("Space unfold · ⏎ open · Tab detail · r kind · u direction · c connect · ? help"),
            (View::Goal, _, false) => Some("Space unfold · ⏎ open · r relation kind · u direction · y T/V · L G H filters · c connect · * goal · ? help"),
            (View::Trails, _, true) => Some("⏎ open · s state · n note · Q question · Tab detail · E export · ? help"),
            (View::Trails, _, false) => Some("⏎ open · s state · n note · Q question · - + move · d remove · N new · t active · r resume · E export · ? help"),
            (View::Imports, _, true) => Some("Space expand · ⏎ outline · Tab detail · p import path · S skim · ? help"),
            (View::Imports, _, false) => Some("Space expand · ⏎ show in the outline · p why it imports a module · S skim · f search within · Esc declarations · ? help"),
        };
        let hints = match (app.focus, app.left_mode, app.search.editing) {
            _ if view_hints.is_some() => view_hints.unwrap(),
            (Focus::Left, LeftMode::Search, true) => "type to search · ↓/⏎ results · Esc stop typing · Tab panes · ? help",
            (Focus::Left, LeftMode::Outline, _) => "⏎/→ expand · ← collapse · o order/column · S skim · f search within · / search · ? help",
            (Focus::Center, _, _) if app.center_mode == CenterMode::Skim => "j/k next/previous statement · ⏎ open · S file/namespace · f search within · Esc close · [ back · ? help",
            (Focus::Right, _, _) => "⏎ follow edge · →/← expand/collapse · ; gloss · 1-5 tabs · [ ] back/fwd · ? help · q quit",
            (Focus::Center, _, _) => "j/k scroll · s source · i implicit · m bookmark · a trail · * goal · M map · A goal view · [ ] back/fwd · ? help",
            _ => "/ search · O outline · B bookmarks · M map · A goal · T trails · : commands · [ ] back/fwd · ? help · q quit",
        };
        let hints = if app.ascii { hints.replace('↓', "Down").replace('⏎', "Enter").replace('→', "Right").replace('←', "Left").replace('·', "|") } else { hints.to_string() };
        Line::from(Span::styled(format!(" {hints}"), t.dim))
    };
    f.render_widget(Paragraph::new(line), area);
}

/// Breaks text into rows of at most `w` cells, at any character (for typed
/// input, where the cursor must stay where the text ends). Never empty.
fn hard_wrap(s: &str, w: usize) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut cur = 0;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if cur + cw > w && cur > 0 {
            out.push(String::new());
            cur = 0;
        }
        out.last_mut().unwrap().push(c);
        cur += cw;
    }
    out
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

pub const HELP: &[&str] = &[
    "slim — reading large Lean 4 projects",
    "",
    "MOVING",
    "  Tab / Shift-Tab   cycle panes: list · declaration · neighbours",
    "  j k ↑ ↓ PgUp PgDn Home End   move / scroll   (Space = page down)",
    "  ⏎                 open the selected item (follow an edge)",
    "  → l / ← h         expand / collapse (outline, neighbour trees)",
    "  [ or Backspace    back in history (restores panes, query, scroll)",
    "  ]                 forward",
    "  g                 go to a fully qualified name",
    "",
    "VIEWS",
    "  M                 architecture map: main results, components, landmarks;",
    "                    from a declaration it shows where the map places it",
    "  A                 goal explorer: unfold what the pinned goal rests on",
    "  T                 reading trails, stops and questions",
    "  I                 import layers: files by import depth, endpoints, hubs;",
    "                    from a declaration it shows where its file sits",
    "  / O B             the declaration browser (search, outline, bookmarks)",
    "  Esc               leave a view · Backspace or [ goes back through history",
    "  *                 pin the selected declaration as the goal; on the goal: clear",
    "  a  m  Q           add to the active trail · bookmark · ask a question",
    "  c                 show a connection from the goal to a declaration",
    "",
    "IN THE MAP, GOAL AND TRAIL VIEWS",
    "  j k ↑ ↓           move · Space or → expands · ← collapses or goes to the parent",
    "  ⏎                 open the declaration (or expand a group, load more)",
    "  Tab               outline / detail (one pane at a time below 80 columns)",
    "  w  < >            detail: wrap long lines, or scroll them sideways",
    "  Map    r reloads lean-view.toml after you edit it (slim never writes it)",
    "  Goal   r relation kind: curated · source mentions · Lean deps · all, labelled",
    "         u direction (uses / used by) · L G filters · y the part followed: all ·",
    "         what it states · statement only · proof/body only",
    "         H show everything the filters hide · c connection to a declaration",
    "  Trail  s state (unread, read, understood, revisit) · n explanation · Q question",
    "         - + move a stop · d remove · N new trail · e rename · t make active",
    "         r resume at the first unread stop · E export Markdown",
    "",
    "  Import p ask why a file imports another module: a chain of import lines",
    "",
    "THE BIG PICTURE",
    "  S                 skim: the statements of the selected file, directory or map",
    "                    group (else of the current declaration's file), each with the",
    "                    lead of its documentation and no proofs. j k move by statement,",
    "                    ⏎ opens one, S switches between its file and its namespace,",
    "                    Esc closes. Source-only: header text as written, not elaborated.",
    "                    A file is read top to bottom: its doc sections (/-! … -/) keep",
    "                    their headings and leads, and in a source-only index the",
    "                    variable and open lines stand where they are written",
    "  o                 outline: order and right-hand column — name, declarations,",
    "                    theorems, proof lines, sorry, last modified. Directories show",
    "                    the sum of their files (the map's file tree shows every count).",
    "                    An open file lists the headings of its doc sections among its",
    "                    declarations; ⏎ on one opens the declaration below it",
    "  I                 import layers. Depth of a file: the longest chain of local",
    "                    imports below it. Endpoint: a file with declarations that no",
    "                    declaring file imports. Hub: a file many files import. These",
    "                    come from import lines: build order, not mathematics",
    "  f  F              search within the selected directory or file, else within the",
    "                    current namespace; f again widens one level; F searches",
    "                    everywhere again (so does Backspace in the empty search box).",
    "                    Inside a scope an empty query lists what is there",
    "  Counts are sizes, not progress: there is no percent proved.",
    "",
    "WHAT THE GOAL EXPLORER SHOWS",
    "  A uses B: unfolding A lists its prerequisite B. Used by is the reverse.",
    "  [cur]        curated in the project map: an author's explanation, not a Lean fact",
    "  [lex]        the source text mentions an indexed name: approximate, unresolved;",
    "               s in the statement, b in the proof or body",
    "  What it states (y): the statement of a theorem, and all of a definition,",
    "  structure or inductive type; proofs are left out. Unfolding with it reads",
    "  a statement down to the definitions it is made of.",
    "  [T] [V] [C]  Lean-derived: in the elaborated statement / proof or body /",
    "               constructor list, from a semantic snapshot",
    "  ↺ cycle      already on the path from the goal: shown, never unfolded again",
    "  ↗ again      also appears elsewhere in the outline; it can be unfolded here too",
    "  Hiding external or generated declarations is a display choice (counts are",
    "  shown), not a claim that they are irrelevant.",
    "  A connection is a shortest path of one relation kind in one direction: it",
    "  shows connectivity, not the best reading order and not necessity.",
    "  Reading states such as understood are your annotations, not Lean verification.",
    "",
    "FINDING",
    "  /                 search: exact qualified name first, then exact name,",
    "                    prefix, word prefix, substring, all words, fuzzy;",
    "                    ≥3 characters also searches documentation text",
    "  O  B              outline (reveals the current declaration) · bookmarks",
    "                    search also lists matching map groups and landmarks",
    "  L  G  K           filters: local only · show generated · kind",
    "",
    "READING",
    "  i                 show/hide implicit and instance arguments",
    "  s                 whole source file (Esc returns), syntax-coloured",
    "  F4                highlighting: syntax + semantic / syntax / off",
    "  F5                syntax theme (VS Code Dark+, Light+, high contrast; follows the UI theme)",
    "  F6                highlighting details: what the colours come from, how fresh, indexing",
    "  1 2 3 4 5         uses · used by · axioms · module · index info",
    "  ;                 gloss: your own line about the selected name (the one selected",
    "                    in a list, else the open declaration), shown beside it in",
    "                    lists, cards, skims and the goal explorer, and found by",
    "                    search. It starts from the map's purpose or the lead of the",
    "                    documentation; empty removes it",
    "  In scope          under the statement: the variable binders it writes and the",
    "                    open lines in force where it stands, read from the file",
    "  Tab to the right  the selected name shows its card under the declaration:",
    "                    where it is written, its statement, the lead of its",
    "                    documentation. ⏎ opens it",
    "  t                 bounded count of the transitive neighbourhood",
    "  m  a  n  E        bookmark · add to trail (note) · bookmark note · export trail",
    "  e                 open in --editor (receives +LINE FILE; no shell)",
    "  R                 check sources against the index now",
    "  : or Ctrl-P       command palette        F2 theme · F3 ASCII chrome",
    "  Module tab (4)    ⏎ on a module shows it in the outline (expanded, selected)",
    "  Mouse             click selects, click again opens; click a tab to switch;",
    "                    wheel scrolls the pane under the pointer. Shift+drag still",
    "                    selects text in most terminals; --no-mouse turns it off",
    "  Holding j/k only moves the highlight; the declaration under the cursor",
    "  loads when you pause (Enter opens at once).",
    "  q / Ctrl-C        quit",
    "",
    "WHAT THE ARROWS MEAN (semantic snapshots)",
    "  T  the constant occurs in the elaborated statement (type)",
    "  V  it occurs in the elaborated proof term or definition body",
    "  C  inductive type → constructor (Lean's axiom collector follows it)",
    "  Edges come from Lean's elaborated terms; they are not a reading order",
    "  and dependency counts are not importance.",
    "",
    "EVIDENCE",
    "  ✓/ok  established by the stated source      ⚠/!!  needs attention",
    "  sorryAx = placeholder proof (sorry or an elaboration error)",
    "  standard axioms: propext, Classical.choice, Quot.sound (not errors)",
    "  compiler trust: native_decide (ofReduceBool / per-use auxiliary axioms)",
    "  coverage gap: some term was unavailable; the list is a lower bound",
    "  A successful build is not an independent kernel replay and says",
    "  nothing about how an informal statement was formalised.",
    "",
    "SOURCE-ONLY MODE",
    "  Declarations are candidates from a conservative lexical scan. There",
    "  are no dependency edges; the Mentions tab is a labelled lexical guess:",
    "  a name is looked up under the enclosing namespaces, then as written and",
    "  under the namespaces opened there. A declaration is listed once; +N says",
    "  that N other declarations have its name (the nearest file is listed).",
    "  What the statement writes comes first, then what only the proof or body",
    "  writes.",
    "  Moving through the Mentions tab (1) highlights where the selected",
    "  identifier is written, in the declaration view and in the whole file,",
    "  and scrolls the first place into view.",
    "  A missing `sorry` token proves nothing.",
    "",
    "Any key closes this help.",
];

/// The help text.
pub fn help_lines(_app: &App) -> Vec<String> {
    HELP.iter().map(|s| s.to_string()).collect()
}

fn draw_overlay(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    match &app.overlay {
        Overlay::None => {}
        Overlay::Help { scroll } => {
            let r = centered(area, 88, area.height.saturating_sub(2));
            f.render_widget(Clear, r);
            let b = block(app, Line::from(Span::styled(" Help ", t.title)), true);
            let inner = b.inner(r);
            // Lines wider than the box wrap under their description column.
            let iw = (inner.width as usize).max(8);
            let all: Vec<String> = help_lines(app)
                .into_iter()
                .flat_map(|l| {
                    let hang = if l.starts_with("  ") && iw > 40 { l.char_indices().skip(3).find(|(i, c)| *c != ' ' && l[..*i].ends_with("  ")).map(|(i, _)| l[..i].chars().count()).unwrap_or(4) } else { 0 };
                    if text_width(&l) <= iw { vec![l] } else { wrap_line(&l, iw, hang.min(iw / 2)) }
                })
                .collect();
            let lines: Vec<Line> = all
                .iter()
                .skip((*scroll).min(all.len().saturating_sub(inner.height as usize)))
                .map(|l| {
                    let l = if app.ascii {
                        l.replace("↺ cycle", "@cycle ").replace("↗ again", "^again ").replace('✓', "ok").replace('⚠', "!!").replace('→', "->").replace('←', "<-").replace('⏎', "Enter").replace('↑', "Up").replace('↓', "Down").replace('·', "|").replace('≥', ">=")
                    } else {
                        l.to_string()
                    };
                    let style = if l.chars().next().is_some_and(|c| c.is_ascii_uppercase()) && l.to_uppercase() == l { t.heading } else { t.text };
                    Line::from(Span::styled(l, style))
                })
                .collect();
            f.render_widget(b, r);
            f.render_widget(Paragraph::new(lines), inner);
        }
        Overlay::Palette { input, sel } => {
            let matches = palette_matches(input);
            // Long command names wrap onto further rows, and the list scrolls
            // so that the selected command is always on screen.
            let w = (area.width.saturating_sub(2).min(76) as usize).saturating_sub(2).max(8);
            let entries: Vec<Vec<Line>> = matches
                .iter()
                .enumerate()
                .map(|(i, (_, name, key))| {
                    let right = format!(" {key}");
                    let avail = w.saturating_sub(text_width(&right)).max(4);
                    let st = if i == *sel { t.selected } else { t.text };
                    wrap_line(&chrome(app, name), avail, 2)
                        .into_iter()
                        .enumerate()
                        .map(|(k, row)| {
                            let key = if k == 0 { right.clone() } else { " ".repeat(text_width(&right)) };
                            Line::from(vec![Span::styled(pad(&row, avail), st), Span::styled(key, if i == *sel { st } else { t.dim })])
                        })
                        .collect()
                })
                .collect();
            let total: usize = entries.iter().map(|e| e.len()).sum();
            let room = (area.height.saturating_sub(2) as usize).saturating_sub(3).max(1);
            let mut start = 0;
            while start < *sel && entries[start..=(*sel).min(entries.len().saturating_sub(1))].iter().map(|e| e.len()).sum::<usize>() > room {
                start += 1;
            }
            let r = centered(area, w as u16 + 2, (total.min(room) + 3) as u16);
            f.render_widget(Clear, r);
            let b = block(app, Line::from(Span::styled(" Commands ", t.title)), true);
            let inner = b.inner(r);
            f.render_widget(b, r);
            let mut lines = vec![Line::from(vec![Span::styled(": ", t.accent), Span::styled(sanitize(input, app.ascii).to_string(), t.input)])];
            lines.extend(entries.into_iter().skip(start).flatten().take(room));
            f.render_widget(Paragraph::new(lines), inner);
            f.set_cursor_position(Position { x: inner.x + (2 + text_width(input)).min(w.saturating_sub(1)) as u16, y: inner.y });
        }
        Overlay::HlStatus { scroll } => {
            let r = centered(area, 104, area.height.saturating_sub(2));
            f.render_widget(Clear, r);
            let b = block(app, Line::from(Span::styled(" Highlighting ", t.title)), true);
            let inner = b.inner(r);
            let iw = (inner.width as usize).max(8);
            let mut all: Vec<Line> = Vec::new();
            for (l, warn) in app.hl_status_lines() {
                let l = chrome(app, &sanitize(&l, app.ascii));
                let style = if warn {
                    t.warn
                } else if !l.is_empty() && !l.starts_with(' ') {
                    t.heading
                } else {
                    t.text
                };
                let hang = l.chars().take_while(|c| *c == ' ').count() + 2;
                all.extend(wrap_line(&l, iw, hang.min(iw / 2)).into_iter().map(|x| Line::from(Span::styled(x, style))));
            }
            all.push(Line::from(""));
            all.push(Line::from(Span::styled(chrome(app, "j/k scroll · F4 mode · F5 syntax theme · any other key closes"), t.dim)));
            let skip = (*scroll).min(all.len().saturating_sub(inner.height as usize));
            let lines: Vec<Line> = all.into_iter().skip(skip).collect();
            f.render_widget(b, r);
            f.render_widget(Paragraph::new(lines), inner);
        }
        Overlay::ConfirmExport { plan, typed } | Overlay::ConfirmHighlight { plan, typed } => {
            let highlight = matches!(&app.overlay, Overlay::ConfirmHighlight { .. });
            let r = centered(area, 110, area.height.saturating_sub(2));
            f.render_widget(Clear, r);
            let title = if highlight { " Semantic highlighting: explicit trust decision " } else { " Semantic export: explicit trust decision " };
            let b = block(app, Line::from(Span::styled(title, t.warn)), true);
            let inner = b.inner(r);
            f.render_widget(b, r);
            let w = inner.width as usize;
            let mut lines: Vec<Line> = Vec::new();
            for l in plan.lines() {
                for row in wrap_line(&sanitize(l, app.ascii), w, 15) {
                    lines.push(Line::from(Span::styled(row, t.text)));
                }
            }
            lines.push(Line::from(""));
            let explain: [&str; 4] = if highlight {
                [
                    "This starts the project's own Lean language server and lets it elaborate the files, which",
                    "runs project code (macros, tactics, elaborators), confined by the sandbox and limits above.",
                    "It can take long on a large project; browsing stays available, and the job can be paused or",
                    "cancelled. Nothing is installed, fetched or built. Trust is not remembered.",
                ]
            } else {
                [
                    "This runs slim's own exporter on the project's existing build artifacts.",
                    "Loading .olean files and (in notation mode) running the imported modules' initializers is",
                    "project code execution: it is confined by the sandbox and limits shown above.",
                    "Nothing is installed, fetched or built for the project. Trust is not remembered.",
                ]
            };
            for l in explain {
                lines.extend(wrap_line(l, w, 0).into_iter().map(|x| Line::from(Span::styled(x, t.dim))));
            }
            lines.push(Line::from(""));
            let ask = chrome(app, "Type yes and press ⏎ to run, Esc to cancel: ");
            let mut ask_rows = wrap_line(&ask, w, 0);
            let tail = ask_rows.pop().unwrap_or_default();
            lines.extend(ask_rows.into_iter().map(|x| Line::from(Span::styled(x, t.warn))));
            lines.push(Line::from(vec![Span::styled(format!("{tail} "), t.warn), Span::styled(typed.clone(), t.input)]));
            // The question stays on screen even when the plan is longer than the box.
            let over = lines.len().saturating_sub(inner.height as usize);
            let lines: Vec<Line> = lines.into_iter().skip(over).collect();
            f.render_widget(Paragraph::new(lines), inner);
        }
        Overlay::Input { prompt, text, .. } => {
            // Nothing is cut: the prompt wraps, the typed text wraps, and the
            // box grows with them (showing the end of the text if the screen
            // is too small for all of it).
            let w = (area.width.saturating_sub(2).min(80) as usize).saturating_sub(2).max(8);
            let prompt_lines = wrap_line(&chrome(app, &sanitize(prompt, app.ascii)), w, 2);
            let hint_lines = wrap_line(&chrome(app, "⏎ confirm · Esc cancel · Ctrl-U clear"), w, 0);
            let mut text_lines = hard_wrap(&sanitize(text, app.ascii), w.saturating_sub(2).max(1));
            let room = (area.height.saturating_sub(2) as usize).saturating_sub(2 + hint_lines.len());
            let prompt_shown = prompt_lines.len().min(room.saturating_sub(1).max(1));
            let text_room = room.saturating_sub(prompt_shown).max(1);
            if text_lines.len() > text_room {
                text_lines.drain(..text_lines.len() - text_room);
            }
            let h = prompt_shown + text_lines.len() + hint_lines.len() + 2;
            let r = centered(area, w as u16 + 2, h as u16);
            f.render_widget(Clear, r);
            let b = block(app, Line::from(Span::styled(" Input ", t.title)), true);
            let inner = b.inner(r);
            f.render_widget(b, r);
            let mut lines: Vec<Line> = prompt_lines.into_iter().take(prompt_shown).map(|l| Line::from(Span::styled(l, t.text))).collect();
            let first_text = lines.len();
            for (i, l) in text_lines.iter().enumerate() {
                lines.push(Line::from(Span::styled(format!("{}{l}", if i == 0 { "> " } else { "  " }), t.input)));
            }
            let last = text_lines.last().map(|l| text_width(l)).unwrap_or(0);
            lines.extend(hint_lines.into_iter().map(|l| Line::from(Span::styled(l, t.dim))));
            f.render_widget(Paragraph::new(lines), inner);
            let cy = (first_text + text_lines.len() - 1).min(inner.height.saturating_sub(1) as usize);
            f.set_cursor_position(Position { x: inner.x + (2 + last).min(w.saturating_sub(1)) as u16, y: inner.y + cy as u16 });
        }
    }
}

#[cfg(test)]
mod render_tests {
    use super::super::state::*;
    use super::super::theme::ThemeName;
    use crate::db::IndexMode;
    use ratatui::text::Line;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::sync::atomic::{AtomicBool, AtomicU64};
    use std::sync::Arc;

    fn app(theme: ThemeName, ascii: bool, mode: IndexMode) -> App {
        let ctx = Ctx {
            title: "slim_demo".into(),
            mode,
            index: None,
            user_key: "lake:t".into(),
            editor: None,
            info: None,
            synthetic: false,
            project_root: None,
        };
        let ch = Channels {
            query: None,
            search: None,
            jobs: None,
            events: None,
            search_latest: Arc::new(AtomicU64::new(0)),
            job_cancel: Arc::new(AtomicBool::new(false)),
        };
        // These screens are drawn with the search pane on the left.
        let mut a = App::new(ctx, theme, ascii, ch);
        a.left_mode = LeftMode::Search;
        a.search.editing = true;
        a
    }

    fn screen(t: &Terminal<TestBackend>) -> String {
        let b = t.backend().buffer();
        let mut s = String::new();
        for y in 0..b.area.height {
            for x in 0..b.area.width {
                s.push_str(b[(x, y)].symbol());
            }
            s.push('\n');
        }
        s
    }

    #[test]
    fn renders_at_many_sizes_themes_and_overlays_without_panicking() {
        for (w, h) in [(100u16, 30u16), (80, 24), (60, 20), (40, 12), (30, 8), (20, 5), (200, 60)] {
            for theme in [ThemeName::LeanDark, ThemeName::LeanLight, ThemeName::Dark, ThemeName::Light, ThemeName::Contrast, ThemeName::Mono] {
                for ascii in [false, true] {
                    for mode in [IndexMode::Semantic, IndexMode::Source] {
                        let mut a = app(theme, ascii, mode);
                        a.ctx.index = Some(crate::app::worker::IndexCtx {
                            index: "x".into(),
                            source_root: None,
                            mode,
                            user_key: "k".into(),
                            encoding: crate::text::PosEncoding::Utf8, map_path: None,
                        });
                        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
                        t.draw(|f| super::draw(f, &mut a)).unwrap();
                        a.overlay = Overlay::Help { scroll: 0 };
                        t.draw(|f| super::draw(f, &mut a)).unwrap();
                        a.overlay = Overlay::Palette { input: "trail".into(), sel: 0 };
                        t.draw(|f| super::draw(f, &mut a)).unwrap();
                        a.overlay = Overlay::ConfirmExport { plan: "plan line\n  another".into(), typed: "ye".into() };
                        t.draw(|f| super::draw(f, &mut a)).unwrap();
                        let s = screen(&t);
                        if ascii && w >= 40 {
                            assert!(!s.contains('╭') && !s.contains('│'), "unicode chrome in ASCII mode at {w}x{h}");
                        }
                    }
                }
            }
        }
    }

    /// The Lean themes bring their own background, so no cell may be left at
    /// the terminal's default colours: that would put the light theme's dark
    /// text on a dark terminal. `Clear` (overlays) resets cells, so every
    /// overlay must repaint the base style.
    #[test]
    fn lean_themes_leave_no_cell_at_terminal_defaults() {
        use ratatui::style::Color;
        for theme in [ThemeName::LeanDark, ThemeName::LeanLight] {
            for (w, h) in [(120u16, 34u16), (80, 24), (40, 12)] {
                for mode in [IndexMode::Semantic, IndexMode::Source] {
                    for (i, overlay) in [
                        Overlay::None,
                        Overlay::Help { scroll: 0 },
                        Overlay::Palette { input: "tr".into(), sel: 0 },
                        Overlay::ConfirmExport { plan: "plan".into(), typed: "y".into() },
                        Overlay::Input { purpose: InputPurpose::GotoName, prompt: "Go to:".into(), text: "Demo".into() },
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let mut a = app(theme, false, mode);
                        a.ctx.index = Some(crate::app::worker::IndexCtx { index: "x".into(), source_root: None, mode, user_key: "k".into(), encoding: crate::text::PosEncoding::Utf8, map_path: None });
                        a.left_mode = [LeftMode::Search, LeftMode::Outline, LeftMode::Bookmarks, LeftMode::Trail][i % 4];
                        a.right_tab = RightTab::ALL[i % 5];
                        a.focus = [Focus::Left, Focus::Center, Focus::Right][i % 3];
                        a.status = Some(("status".into(), i % 2 == 0));
                        a.overlay = overlay;
                        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
                        t.draw(|f| super::draw(f, &mut a)).unwrap();
                        let b = t.backend().buffer();
                        for y in 0..h {
                            for x in 0..w {
                                let c = &b[(x, y)];
                                assert!(c.bg != Color::Reset && c.fg != Color::Reset, "{theme:?} {w}x{h} overlay {i}: cell ({x},{y}) {:?} uses the terminal default", c.symbol());
                            }
                        }
                    }
                }
            }
        }
    }

    const VIEW_MAP: &str = r#"
schema = 1
[project]
title = "Ünïcödé 項目 map"
[[objective]]
decl = "Demo.«weird name».ℛ_αβγ_long_long_long_long_long_long_name"
title = "主定理 — the main theorem with a very long title that cannot fit anywhere"
purpose = "説明 with\u001b[2J an escape"
[[group]]
id = "a"
title = "Gruppe α"
purpose = "p"
origin = "generated"
landmarks = ["Demo.«weird name».ℛ_αβγ_long_long_long_long_long_long_name", "Demo.gone"]
[[group]]
id = "b"
parent = "a"
landmarks = ["Demo.x"]
[[trail]]
id = "t"
stops = [{ decl = "Demo.x" }]
"#;

    fn with_views(a: &mut App) {
        use crate::app::views::*;
        use crate::app::worker::{MapBundle, Reading};
        use crate::map::{MapLoad, RefInfo, RefStatus, Resolution};
        use crate::relations::{ChildPage, EdgeLabel};
        let m = crate::map::parse(VIEW_MAP, std::path::Path::new("lean-view.toml")).unwrap();
        let mut res = Resolution::default();
        res.refs.insert("Demo.gone".into(), RefInfo { status: RefStatus::Missing { suggestions: vec!["Demo.here".into()] }, row: None, module_sha256: None });
        a.v.map.bundle = Some(MapBundle { load: MapLoad::Loaded(Arc::new(m)), resolution: res });
        a.v.map.expanded.insert("grp:a".into());
        a.v.map.expanded.insert("grp:b".into());
        a.v.goal.goal = Some(GoalRef { name: "Demo.x".into(), display: "Demo.x".into() });
        let node = |key: u64, name: &str, depth: u16, parent: Option<usize>, backref: bool| GNode {
            key,
            name: name.into(),
            row: None,
            depth,
            parent,
            labels: if depth == 0 { vec![] } else { vec![EdgeLabel::Lean { type_: true, body: true, ctor: false }, EdgeLabel::Lexical { part: crate::relations::Part::Unknown, ident: "x".into() }] },
            note: Some("why".into()),
            expanded: true,
            loaded: true,
            loading: false,
            backref,
            repeat: !backref && depth > 1,
            children: vec![],
            page: ChildPage { hidden_external: 12, hidden_generated: 3, more: Some(50), remaining: 99_000, notes: vec![("Lean dependencies are unavailable".into(), true)], ..Default::default() },
        };
        a.v.goal.nodes = vec![node(1, "Demo.x", 0, None, false), node(2, "Demo.«weird name».ℛ_αβγ", 1, Some(0), false), node(3, "Demo.x", 2, Some(1), true), node(4, "Demo.y", 2, Some(1), false)];
        a.v.goal.nodes[0].children = vec![1];
        a.v.goal.nodes[1].children = vec![2, 3];
        let stop = |id: i64, name: &str| crate::userdata::Stop { id, trail_id: 1, name: name.into(), display: name.into(), kind: "theorem".into(), note: Some("note — long ".repeat(8)), question: Some("q?".into()), ..Default::default() };
        let mut r = Reading::default();
        r.trails.push(crate::userdata::Trail { id: 1, title: "Tràil".into(), origin: "personal".into(), stops: vec![stop(1, "Demo.x"), stop(2, "Demo.gone")], ..Default::default() });
        r.statuses.insert(2, crate::userdata::StopStatus::Unresolved("gone".into()));
        r.questions.push(crate::userdata::Question { id: 1, name: "Demo.x".into(), display: "Demo.x".into(), text: "why?".into(), ..Default::default() });
        a.v.trails.reading = r;
        a.v.trails.expanded.insert(1);
        a.v.trails.loaded = true;
    }

    /// The map, goal and trail views at wide, narrow and tiny sizes, in every
    /// theme, with Unicode names: no panic, no escape sequence on screen, no
    /// Unicode chrome in ASCII mode, and every row selectable by keyboard.
    #[test]
    fn views_render_at_many_sizes_and_survive_resizing() {
        use crate::app::views::*;
        use ratatui::crossterm::event::{Event as CEvent, KeyCode, KeyEvent, KeyModifiers};
        for theme in [ThemeName::LeanDark, ThemeName::LeanLight, ThemeName::Mono] {
            for ascii in [false, true] {
                for mode in [IndexMode::Semantic, IndexMode::Source] {
                    for view in [View::Map, View::Goal, View::Trails] {
                        let mut a = app(theme, ascii, mode);
                        a.ctx.index = Some(crate::app::worker::IndexCtx { index: "x".into(), source_root: None, mode, user_key: "k".into(), encoding: crate::text::PosEncoding::Utf8, map_path: None });
                        with_views(&mut a);
                        a.search.editing = false;
                        a.set_view(view);
                        let mut keep: Option<String> = None;
                        for (w, h) in [(140u16, 40u16), (100, 30), (80, 24), (79, 24), (60, 20), (40, 12), (30, 8), (20, 5), (120, 30)] {
                            let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
                            t.draw(|f| super::draw(f, &mut a)).unwrap();
                            let s = screen(&t);
                            assert!(!s.contains('\x1b'), "escape sequence on screen");
                            if ascii && w >= 40 {
                                for c in ['╭', '│', '▾', '▸', '◎', '◆', '↺', '↗', '⏎', '…', '≥'] {
                                    assert!(!s.contains(c), "unicode chrome {c:?} in ASCII mode ({view:?} {w}x{h})");
                                }
                            }
                            if w >= 30 && h >= 8 {
                                // One pane below the threshold, two at or above it.
                                assert_eq!(a.v.single_pane, w < SINGLE_PANE_BELOW, "{w}");
                                assert_eq!(s.contains(" Detail "), w >= SINGLE_PANE_BELOW, "{view:?} {w}x{h}");
                            }
                            // Walk the outline and the detail with the keyboard.
                            for code in [KeyCode::Down, KeyCode::Down, KeyCode::Char(' '), KeyCode::Tab, KeyCode::PageDown, KeyCode::Char('w'), KeyCode::Char('>'), KeyCode::Char('w'), KeyCode::Tab, KeyCode::Down] {
                                a.handle(crate::app::Event::Input(CEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))));
                                t.draw(|f| super::draw(f, &mut a)).unwrap();
                            }
                            // The selection is an identity: resizing does not move it.
                            if view == View::Map {
                                let k = a.view_key();
                                a.handle(crate::app::Event::Input(CEvent::Resize(w, h)));
                                t.draw(|f| super::draw(f, &mut a)).unwrap();
                                assert_eq!(a.view_key(), k);
                                keep = k;
                            }
                        }
                        if view == View::Map {
                            assert!(keep.is_some());
                        }
                        a.overlay = Overlay::Help { scroll: 0 };
                        let mut t = Terminal::new(TestBackend::new(100, 30)).unwrap();
                        t.draw(|f| super::draw(f, &mut a)).unwrap();
                        assert!(screen(&t).contains("MOVING"));
                    }
                }
            }
        }
    }

    /// Every label a view shows for a relation says which kind it is.
    #[test]
    fn views_label_evidence_in_text_and_paint_every_cell() {
        use crate::app::views::*;
        use ratatui::style::Color;
        for (view, needles) in [
            (View::Map, vec!["Main results", "Mathematical components", "Unresolved references", "[generated]", "MISSING", "Files and namespaces", "3 curated"]),
            (View::Goal, vec!["relation: Curated argument", "[T+V lex]", "hidden: 12 external, 3 generated (H)", "direction: uses", "more Lean edges", "goal: Demo.x"]),
            (View::Trails, vec!["Your trails (1)", "UNRESOLVED", "Shared trails in the project map (1)", "Questions (1 open, 0 resolved)", "0/2 read"]),
        ] {
            let mut a = app(ThemeName::LeanLight, false, IndexMode::Semantic);
            a.ctx.index = Some(crate::app::worker::IndexCtx { index: "x".into(), source_root: None, mode: IndexMode::Semantic, user_key: "k".into(), encoding: crate::text::PosEncoding::Utf8, map_path: None });
            with_views(&mut a);
            a.set_view(view);
            let mut t = Terminal::new(TestBackend::new(150, 40)).unwrap();
            t.draw(|f| super::draw(f, &mut a)).unwrap();
            let s = screen(&t);
            for n in needles {
                assert!(s.contains(n), "{view:?} screen lacks {n:?}:\n{s}");
            }
            assert!(!s.to_lowercase().contains("proof dependenc"), "no panel may be titled proof dependencies");
            let b = t.backend().buffer();
            for y in 0..40 {
                for x in 0..150 {
                    let c = &b[(x, y)];
                    // The second cell of a double-width character is not drawn.
                    if x > 0 && crate::text::width(b[(x - 1, y)].symbol()) == 2 {
                        continue;
                    }
                    assert!(c.bg != Color::Reset && c.fg != Color::Reset, "{view:?}: cell ({x},{y}) {:?} uses the terminal default", c.symbol());
                }
            }
        }
    }

    /// Popups show all of their text: prompts and typed text wrap, long
    /// command names wrap, and the selected command is scrolled into view.
    #[test]
    fn popups_wrap_instead_of_truncating() {
        let flat = |t: &Terminal<TestBackend>| screen(t).replace(['│', '|', '\n'], " ").split_whitespace().collect::<Vec<_>>().join(" ");
        for (w, h) in [(100u16, 30u16), (60, 20), (40, 14)] {
            let mut a = app(ThemeName::Mono, false, IndexMode::Semantic);
            a.ctx.index = Some(crate::app::worker::IndexCtx { index: "x".into(), source_root: None, mode: IndexMode::Semantic, user_key: "k".into(), encoding: crate::text::PosEncoding::Utf8, map_path: None });
            let prompt = "Your question about Demo.Graph.a_declaration_with_a_rather_long_qualified_name (kept in your data; T lists open questions):";
            let typed = "why does the fuel bound matter here, and what would a proof of completeness need ZZEND";
            a.overlay = Overlay::Input { purpose: InputPurpose::GotoName, prompt: prompt.into(), text: typed.into() };
            let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
            t.draw(|f| super::draw(f, &mut a)).unwrap();
            let s = flat(&t);
            assert!(s.contains("T lists open questions):"), "{w}x{h}: prompt cut:\n{}", screen(&t));
            assert!(s.contains("ZZEND"), "{w}x{h}: typed text cut:\n{}", screen(&t));
            assert!(s.contains("Ctrl-U clear"), "{w}x{h}: hint cut");
            // The palette: the longest command name is shown in full.
            a.overlay = Overlay::Palette { input: "architecture".into(), sel: 0 };
            t.draw(|f| super::draw(f, &mut a)).unwrap();
            let s = flat(&t);
            assert!(s.contains("(reveals") && s.contains("declaration)") && s.contains("landmarks"), "{w}x{h}: command name cut:\n{}", screen(&t));
            // With every command listed, the last one is reachable.
            let n = COMMANDS.len();
            a.overlay = Overlay::Palette { input: String::new(), sel: n - 1 };
            t.draw(|f| super::draw(f, &mut a)).unwrap();
            assert!(flat(&t).contains("Quit"), "{w}x{h}: the selected command is off screen:\n{}", screen(&t));
            // Help lines wrap at narrow widths instead of being clipped.
            a.overlay = Overlay::Help { scroll: 0 };
            t.draw(|f| super::draw(f, &mut a)).unwrap();
            let s = flat(&t);
            assert!(s.contains("reading large") && s.contains("projects"), "{w}x{h}: help title cut:\n{}", screen(&t));
        }
    }

    fn overview_graph() -> Arc<crate::imports::ImportGraph> {
        use crate::imports::{ImportGraph, Node};
        // Sorted by name; one module name carries an escape sequence.
        let names = ["Lib.Core", "P", "P.Alg.Group", "P.Found.Basic", "P.Found.«weird ℛ»", "P.Main.Result_with_a_very_long_name_that_cannot_fit", "P.Main.evil\u{1b}[2J"];
        let nodes = names.iter().enumerate().map(|(i, n)| Node { id: i as i64 + 1, name: n.to_string(), local: i > 0, decls: if i == 1 { 0 } else { 3 + i as u32 } }).collect();
        Arc::new(ImportGraph::build(nodes, vec![(1, 2), (1, 5), (1, 6), (2, 3), (2, 4), (4, 3), (3, 0), (5, 2), (6, 3)]))
    }

    fn overview_skim() -> Box<crate::skim::Skim> {
        use crate::db::query::{DeclRow, ModStats};
        use crate::skim::{Skim, SkimDecl, SkimItem, SkimScope};
        let row = |id: i64, n: &str, kind| DeclRow { id, name: n.into(), display: n.into(), kind, origin: crate::model::Origin::Local, flags: 0, module: Some("P.Found.Basic".into()) };
        let thm = crate::model::DeclKind::Theorem;
        let items = vec![
            SkimItem::Header { title: "P.Found.Basic".into(), sub: "P/Found/Basic.lean · 2 thm".into(), doc: Some("Basics \u{1b}[2J of ℛ".into()), warn: Some("changed since it was indexed".into()) },
            SkimItem::Decl(SkimDecl {
                row: row(1, "P.long", thm),
                line: Some(3),
                sig: "theorem long {α : Type} (h₁ : a_very_long_hypothesis_name α)\n    (h₂ : another_rather_long_hypothesis_name α) :\n    the_conclusion_of_the_theorem_is_also_long α".into(),
                sig_cut: true,
                doc: Some("Documentation lead 説明 that is long enough to be cut in a narrow pane.".into()),
                sorry: true,
                folded: 12,
            }),
            SkimItem::Decl(SkimDecl { row: row(2, "P.Star.refl", crate::model::DeclKind::Constructor), line: Some(9), sig: "| refl (a : Nat) : Star r a a -- comment".into(), sig_cut: false, doc: None, sorry: false, folded: 0 }),
            SkimItem::Note { text: "no declarations indexed in this file".into(), warn: false },
            SkimItem::Decl(SkimDecl { row: row(3, "P.short", thm), line: Some(20), sig: "theorem short : True".into(), sig_cut: false, doc: None, sorry: false, folded: 1 }),
        ];
        Box::new(Skim {
            scope: SkimScope::Dir("P.Found".into()),
            title: "files under P.Found".into(),
            items,
            decls: 3,
            files: 2,
            stats: Some(ModStats { files: 2, lines: 12_345, thm: 2, def: 1, doc: 1, sorry: 1, proof_lines: 900, ..Default::default() }),
            truncated: Some("stopped after 1500 declarations".into()),
        })
    }

    /// The import view, the skim and a scoped search at every size, in every
    /// theme: no panic, nothing hostile on screen, no Unicode chrome in ASCII
    /// mode, and (Lean themes) no cell left at the terminal's colours.
    #[test]
    fn overview_views_render_at_many_sizes() {
        use crate::app::views::*;
        use crate::nameindex::{ScopeKind, ScopeSpec};
        use ratatui::crossterm::event::{Event as CEvent, KeyCode, KeyEvent, KeyModifiers};
        use ratatui::style::Color;
        for theme in [ThemeName::LeanDark, ThemeName::LeanLight, ThemeName::Contrast, ThemeName::Mono] {
            for ascii in [false, true] {
                for mode in [IndexMode::Semantic, IndexMode::Source] {
                    let mut a = app(theme, ascii, mode);
                    a.ctx.index = Some(crate::app::worker::IndexCtx { index: "x".into(), source_root: None, mode, user_key: "k".into(), encoding: crate::text::PosEncoding::Utf8, map_path: None });
                    a.search.editing = false;
                    a.search.scope = Some(ScopeSpec { kind: ScopeKind::Namespace, prefix: "Demo.«weird ℛ».evil\u{1b}[2J".into() });
                    a.center.skim.data = Some(overview_skim());
                    a.center.skim.scope = a.center.skim.data.as_ref().map(|s| s.scope.clone());
                    a.center.skim.sel = 1;
                    a.set_view(View::Imports);
                    let gen = a.gens.imports;
                    assert!(a.on_imports(gen, Ok(overview_graph())));
                    a.v.imports.path = Some(crate::imports::PathAnswer::Path(vec!["P".into(), "P.Alg.Group".into(), "Lib.Core".into()]));
                    a.rebuild_import_rows();
                    for (w, h) in [(140u16, 40u16), (100, 30), (80, 24), (79, 24), (60, 20), (40, 12), (30, 8), (20, 5)] {
                        for skim in [false, true] {
                            a.v.view = if skim { View::Browse } else { View::Imports };
                            a.center_mode = if skim { CenterMode::Skim } else { CenterMode::Decl };
                            a.focus = if skim { Focus::Center } else { Focus::Left };
                            let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
                            for code in [KeyCode::Down, KeyCode::Down, KeyCode::Char(' '), KeyCode::Down, KeyCode::Tab, KeyCode::PageDown, KeyCode::Tab, KeyCode::Up] {
                                t.draw(|f| super::draw(f, &mut a)).unwrap();
                                let s = screen(&t);
                                assert!(!s.contains('\x1b'), "escape sequence on screen ({w}x{h}, skim {skim})");
                                if ascii && w >= 40 {
                                    for c in ['╭', '│', '▾', '▸', '⏎', '…', '–', '·', '→'] {
                                        assert!(!s.contains(c), "unicode chrome {c:?} in ASCII mode ({w}x{h}, skim {skim}):\n{s}");
                                    }
                                }
                                if matches!(theme, ThemeName::LeanDark | ThemeName::LeanLight) && w >= 30 && h >= 8 {
                                    let b = t.backend().buffer();
                                    for y in 0..h {
                                        for x in 0..w {
                                            if x > 0 && crate::text::width(b[(x - 1, y)].symbol()) == 2 {
                                                continue;
                                            }
                                            let c = &b[(x, y)];
                                            assert!(c.bg != Color::Reset && c.fg != Color::Reset, "{theme:?} {w}x{h} skim {skim}: cell ({x},{y}) {:?} uses the terminal default", c.symbol());
                                        }
                                    }
                                }
                                a.handle(crate::app::Event::Input(CEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))));
                            }
                        }
                    }
                }
            }
        }
    }

    /// What the overview screens say about themselves.
    #[test]
    fn overview_views_say_what_their_numbers_mean() {
        use crate::app::views::*;
        use crate::nameindex::{ScopeKind, ScopeSpec};
        let mut a = app(ThemeName::Mono, false, IndexMode::Source);
        a.ctx.index = Some(crate::app::worker::IndexCtx { index: "x".into(), source_root: None, mode: IndexMode::Source, user_key: "k".into(), encoding: crate::text::PosEncoding::Utf8, map_path: None });
        a.search.editing = false;
        a.set_view(View::Imports);
        let gen = a.gens.imports;
        a.on_imports(gen, Ok(overview_graph()));
        let mut t = Terminal::new(TestBackend::new(150, 44)).unwrap();
        t.draw(|f| super::draw(f, &mut a)).unwrap();
        let s = screen(&t);
        for n in ["Import layers", "Layers by import depth", "6 files, depth 0–4", "Endpoints", "Hubs", "Import-only files", "imported by 3", "build order, not mathematics", "longest chain of local imports"] {
            assert!(s.contains(n), "import view lacks {n:?}:\n{s}");
        }
        // The single root is open: directories from the bottom of the import order up.
        let pos = |n: &str| s.find(n).unwrap_or_else(|| panic!("{n} not on screen:\n{s}"));
        assert!(pos("▸ Found") < pos("▸ Alg") && pos("▸ Alg") < pos("▸ Main"));
        // An endpoint's long name keeps its end.
        assert!(s.contains("that_cannot_fit"));
        // The skim: statements, what was left out, and that it is not the whole scope.
        a.v.view = View::Browse;
        a.center_mode = CenterMode::Skim;
        a.focus = Focus::Center;
        a.center.skim.data = Some(overview_skim());
        a.center.skim.sel = 1;
        a.search.scope = Some(ScopeSpec { kind: ScopeKind::Modules, prefix: "P.Found".into() });
        t.draw(|f| super::draw(f, &mut a)).unwrap();
        let s = screen(&t);
        for n in [
            "Skim files under P.Found",
            "3 statements",
            "source text",
            "not the whole scope: stopped after 1500 declarations",
            "12.3k lines,",
            "900 of them in theorem proofs",
            "theorem long {α : Type} (h₁ : a_very_long_hypothesis_name α)",
            "statement cut",
            "sorry token │ 12 lines folded",
            "1 line folded",
            "| refl (a : Nat) : Star r a a -- comment",
            "changed since it was indexed",
            "in files P.Found │ F clears",
            " 1/3 ",
        ] {
            assert!(s.contains(n), "skim lacks {n:?}:\n{s}");
        }
        // The marker is on the selected statement only.
        assert_eq!(s.matches("▸ theorem").count(), 1);
        assert!(s.contains("▸ theorem long"));
        // With room, a statement written over three lines is one row; a
        // statement holding a comment keeps its lines.
        let (rows, ranges) = super::skim_layout(&a, 220);
        let text = |l: &Line| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>();
        let (from, to) = ranges[1];
        assert!(text(&rows[from]).contains("theorem long {α : Type} (h₁ : a_very_long_hypothesis_name α) (h₂ : another_rather_long_hypothesis_name α) : the_conclusion_of_the_theorem_is_also_long α"));
        assert_eq!(to - from, 3, "statement, the note that it was cut, and the row for what was left out");
        assert_eq!(ranges.len(), a.center.skim.data.as_ref().unwrap().items.len());
    }

    /// Selecting a row of the Mentions tab paints the occurrences of its
    /// identifier in the excerpt, and nothing else.
    #[test]
    fn selected_mention_is_highlighted_where_it_is_written() {
        use crate::app::worker::{DetailBundle, SourceStatus};
        use crate::db::query::{DeclDetail, DeclRow, Pos};
        use crate::relations::Occurrence;
        use ratatui::style::Modifier;
        let mut a = app(ThemeName::Mono, false, IndexMode::Source);
        a.ctx.index = Some(crate::app::worker::IndexCtx { index: "x".into(), source_root: None, mode: IndexMode::Source, user_key: "k".into(), encoding: crate::text::PosEncoding::Utf8, map_path: None });
        a.search.editing = false;
        let row = |id: i64, n: &str| DeclRow { id, name: n.into(), display: n.into(), kind: crate::model::DeclKind::Theorem, origin: crate::model::Origin::SourceCandidate, flags: 0, module: Some("M".into()) };
        // `α_foo` is used on lines 11 and 12; `foo_other` contains the same letters.
        let lines = ["theorem t (h : α_foo x) :", "    α_foo y ∧ foo_other := by", "  exact ⟨α_foo.intro, foo_other⟩"];
        let col = |l: usize, k: usize| lines[l].match_indices("α_foo").nth(k).unwrap().0 as u32;
        let len = "α_foo".len() as u32;
        a.current = Some(1);
        a.detail = Some(Box::new(DetailBundle {
            d: DeclDetail {
                row: row(1, "t"),
                module_id: Some(1),
                module_path: Some("M.lean".into()),
                module_status: None,
                module_sha256: None,
                type_cov: crate::model::Coverage::SourceOnly,
                value_cov: crate::model::Coverage::SourceOnly,
                pos: Pos::default(),
                type_text: None,
                sig: None,
                doc: None,
                attrs: None,
                generated: None,
                keyword: Some("theorem".into()),
                sorry_line: None,
                degrees: [0; 6],
                axioms: Vec::new(),
                lean_axioms: None,
                same_name: 0,
            },
            excerpt: lines.iter().enumerate().map(|(i, l)| (10 + i, l.to_string())).collect(),
            excerpt_truncated: false,
            decl_lines: Some((10, 12)),
            source_status: SourceStatus::Ok,
            source_sha256: Some("h".into()),
            statement_src: Some("theorem t (h : α_foo x) :\n    α_foo y ∧ foo_other".into()),
            statement_at: Some((10, 0)),
            bookmarked: false,
        }));
        let occ = Arc::new(vec![
            Occurrence { line: 10, start: col(0, 0), end: col(0, 0) + len },
            Occurrence { line: 11, start: col(1, 0), end: col(1, 0) + len },
            Occurrence { line: 12, start: col(2, 0), end: col(2, 0) + len },
        ]);
        a.right_tab = RightTab::Uses;
        a.right.rows = vec![RRow::Mention { ident: "α_foo".into(), row: row(2, "N.α_foo"), occ, also: 0 }];
        a.focus = Focus::Right;
        let marked = |a: &mut App| -> Vec<String> {
            let mut t = Terminal::new(TestBackend::new(120, 40)).unwrap();
            t.draw(|f| super::draw(f, a)).unwrap();
            let b = t.backend().buffer();
            // Reverse video inside the centre pane: runs of marked cells.
            let (x0, x1) = (a.hit.center.x + 1, a.hit.center.x + a.hit.center.width - 1);
            let mut runs = Vec::new();
            for y in a.hit.center.y + 1..a.hit.center.y + a.hit.center.height - 1 {
                let mut cur = String::new();
                for x in x0..x1 {
                    let c = &b[(x, y)];
                    if c.modifier.contains(Modifier::REVERSED) {
                        cur.push_str(c.symbol());
                    } else if !cur.is_empty() {
                        runs.push(std::mem::take(&mut cur));
                    }
                }
                if !cur.is_empty() {
                    runs.push(cur);
                }
            }
            runs
        };
        assert!(marked(&mut a).is_empty(), "nothing is highlighted before the reader moves in the list");
        a.right.marks_on = true;
        a.center.version += 1;
        // Twice in the statement header (lines 10 and 11), three times in the excerpt.
        assert_eq!(marked(&mut a), vec!["α_foo"; 5]);
        // The whole-file view marks the same places.
        a.center_mode = CenterMode::Source;
        a.center.source = Some(crate::app::worker::FullSource {
            rel_path: "M.lean".into(),
            sha256: "h".into(),
            lines: (1..=9).map(|i| format!("-- line {i} α_foo")).chain(lines.iter().map(|l| l.to_string())).collect(),
            focus_line: 10,
            highlight: Some((10, 12)),
            changed: false,
        });
        a.center.source_scroll = 0;
        assert_eq!(marked(&mut a), vec!["α_foo"; 3], "other lines that contain the same text are not occurrences in this declaration");
        // Leaving the Mentions tab clears the highlight.
        a.right.marks_on = false;
        assert!(marked(&mut a).is_empty());
    }

    #[test]
    fn hostile_text_is_sanitized_on_screen() {
        let mut a = app(ThemeName::Dark, false, IndexMode::Semantic);
        a.ctx.index = Some(crate::app::worker::IndexCtx { index: "x".into(), source_root: None, mode: IndexMode::Semantic, user_key: "k".into(), encoding: crate::text::PosEncoding::Utf8, map_path: None });
        a.search.query = "evil\x1b[2J\u{202e}".into();
        a.status = Some(("status \x07 bell".into(), false));
        let mut t = Terminal::new(TestBackend::new(100, 30)).unwrap();
        t.draw(|f| super::draw(f, &mut a)).unwrap();
        let s = screen(&t);
        assert!(!s.contains('\x1b') && !s.contains('\x07') && !s.contains('\u{202e}'));
        assert!(s.contains("evil␛[2J"));
    }
}

#[cfg(test)]
mod tests {
    use super::reflow_doc;

    #[test]
    fn doc_reflow_joins_paragraph_lines_only() {
        let doc = "First line\ncontinues here.\n\n* item one\n  wraps\n* item two\n\n```lean\nexample : True := trivial\n```\nAfter.";
        let r = reflow_doc(doc);
        assert_eq!(r[0], "First line continues here.");
        assert_eq!(r[1], "");
        assert_eq!(r[2], "* item one wraps");
        assert_eq!(r[3], "* item two");
        assert!(r.contains(&"example : True := trivial".to_string()));
        assert_eq!(r.last().unwrap(), "After.");
    }
}
