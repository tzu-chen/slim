//! What stands between the declarations of a file: `open`, `variable` and
//! `universe` commands with the scope they hold in, and module-doc blocks
//! (`/-! … -/`).
//!
//! Only the text *between* declarations is lexed (their extents are known
//! from the index), so the cost follows the size of the gaps, not of the
//! file. Like the scanner this reads text: nothing is elaborated, and a
//! command produced by a macro is not seen.

use super::lexer::{LexProfile, Lexer, TokKind, Token};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmdKind {
    Open,
    Variable,
    Universe,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    /// In force from its end up to `until` (the end of its section or
    /// namespace; for `… in`, just past the next declaration's start).
    Cmd { kind: CmdKind, until: usize },
    /// A module-doc block.
    Prose,
    /// The `end` of a scope that holds `open` or `variable` commands.
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub kind: EventKind,
    pub start: usize,
    pub end: usize,
    /// 1-based line of `start`.
    pub line: u32,
}

#[derive(Clone, Debug, Default)]
pub struct FileScope {
    /// In file order.
    pub events: Vec<Event>,
}

impl FileScope {
    /// The commands in force for a declaration that starts at byte `at`.
    pub fn in_force(&self, at: usize) -> impl Iterator<Item = (&Event, CmdKind)> {
        self.events.iter().filter_map(move |e| match e.kind {
            EventKind::Cmd { kind, until } if e.end <= at && at < until => Some((e, kind)),
            _ => None,
        })
    }
}

enum Frame {
    Namespace(Vec<String>),
    Section(Option<String>),
    Mutual,
}

struct Walk<'a> {
    text: &'a str,
    line_starts: &'a [usize],
    out: Vec<Event>,
    /// Scope stack: the frame and the commands declared in it.
    frames: Vec<(Frame, Vec<usize>)>,
    /// The tokens of the gap being read (kept between gaps).
    toks: Vec<Token>,
    /// Where the lines that may hold a command start, in order.
    lines: &'a [usize],
}

impl Walk<'_> {
    fn line_of(&self, b: usize) -> u32 {
        (match self.line_starts.binary_search(&b) {
            Ok(i) => i + 1,
            Err(i) => i,
        }) as u32
    }

    /// Closes one frame at byte `at`; true when it held commands.
    fn pop(&mut self, at: usize) -> Option<(Frame, bool)> {
        let (f, cmds) = self.frames.pop()?;
        for &k in &cmds {
            if let EventKind::Cmd { until, .. } = &mut self.out[k].kind {
                *until = (*until).min(at);
            }
        }
        Some((f, !cmds.is_empty()))
    }

    /// `end` or `end Name`, as the scanner closes scopes.
    fn close(&mut self, target: Option<&str>, at: usize) -> bool {
        let mut held = false;
        let Some(target) = target else {
            return self.pop(at).is_some_and(|(_, h)| h);
        };
        if matches!(self.frames.last(), Some((Frame::Section(Some(n)), _)) if n == target) {
            return self.pop(at).is_some_and(|(_, h)| h);
        }
        let mut remaining = crate::model::name_components(target).len();
        while remaining > 0 {
            match self.pop(at) {
                Some((Frame::Namespace(p), h)) => {
                    held |= h;
                    remaining -= p.len().min(remaining);
                }
                Some((_, h)) => {
                    held |= h;
                    remaining -= 1;
                }
                None => break,
            }
        }
        held
    }

    fn gap(&mut self, a: usize, b: usize) {
        let text = self.text;
        if a >= b || b > text.len() || !text.is_char_boundary(a) || !text.is_char_boundary(b) {
            return;
        }
        // Most gaps hold no command: a blank line, a comment, an `example`,
        // and sometimes megabytes of data a declaration's extent missed.
        let k = self.lines.partition_point(|&x| x < a);
        if self.lines.get(k).is_none_or(|&x| x >= b) {
            return;
        }
        let src = &text[a..b];
        let mut lx = Lexer::new(src, LexProfile::NEWEST);
        let mut toks = std::mem::take(&mut self.toks);
        toks.clear();
        while let Some(t) = lx.next_token() {
            toks.push(t);
        }
        // Text after the previous declaration on its own last line is not
        // the start of a line.
        let mid_line = a > 0 && text.as_bytes()[a - 1] != b'\n';
        let word = |t: &Token| t.text(src);
        let next_code = |i: usize| (i + 1..toks.len()).find(|&j| !toks[j].is_comment());
        // The command being read (its event), and its bracket depth.
        let mut cur: Option<usize> = None;
        // `… in` commands waiting for the declaration they are for.
        let mut ins: Vec<usize> = Vec::new();
        let mut depth = 0i32;
        // Inside a command this module does not read (an `example`, a macro).
        let mut other = false;
        let mut i = 0;
        while i < toks.len() {
            let t = toks[i];
            let first = t.first_on_line && !(mid_line && t.line == 1);
            if t.kind == TokKind::ModDoc {
                cur = None;
                other = false;
                self.out.push(Event { kind: EventKind::Prose, start: a + t.start as usize, end: a + t.end as usize, line: self.line_of(a + t.start as usize) });
                i += 1;
                continue;
            }
            if t.is_comment() {
                if t.kind == TokKind::DocComment && first {
                    cur = None;
                }
                i += 1;
                continue;
            }
            let w = word(&t);
            let head = first && (depth <= 0 || t.col == 0) && (t.col == 0 || !other);
            if head && (t.kind == TokKind::Ident && super::scan::is_command_word(w) || t.kind == TokKind::Open && w == "@[" || w == "#") {
                cur = None;
                depth = 0;
                other = false;
                // `noncomputable section`, `public section`.
                let mut k = i;
                while matches!(word(&toks[k]), "noncomputable" | "public" | "private" | "meta" | "unsafe" | "partial" | "protected" | "nonrec") {
                    match next_code(k) {
                        Some(n) => k = n,
                        None => break,
                    }
                }
                let tk = toks[k];
                // `open Foo in example …`: not for the next declaration.
                if !matches!(word(&tk), "open" | "variable" | "universe" | "set_option" | "attribute" | "omit" | "include" | "unseal" | "seal") {
                    for c in ins.drain(..) {
                        if let EventKind::Cmd { until, .. } = &mut self.out[c].kind {
                            *until = 0;
                        }
                    }
                }
                let same_line_name = |k: usize| next_code(k).filter(|&x| toks[x].kind == TokKind::Ident && toks[x].line == toks[k].line && !super::scan::is_command_word(word(&toks[x])));
                match word(&tk) {
                    "namespace" => {
                        if let Some(x) = next_code(k).filter(|&x| toks[x].kind == TokKind::Ident) {
                            let parts = crate::model::name_components(word(&toks[x])).into_iter().map(String::from).collect();
                            self.frames.push((Frame::Namespace(parts), Vec::new()));
                            i = x;
                        }
                    }
                    "section" => {
                        let x = same_line_name(k);
                        self.frames.push((Frame::Section(x.map(|x| word(&toks[x]).to_string())), Vec::new()));
                        i = x.unwrap_or(k);
                    }
                    "mutual" => self.frames.push((Frame::Mutual, Vec::new())),
                    "end" => {
                        let x = same_line_name(k);
                        let last = x.unwrap_or(k);
                        let (s, e) = (a + tk.start as usize, a + toks[last].end as usize);
                        if self.close(x.map(|x| word(&toks[x])), s) {
                            self.out.push(Event { kind: EventKind::End, start: s, end: e, line: self.line_of(s) });
                        }
                        i = last;
                    }
                    kw @ ("open" | "variable" | "universe") if k == i => {
                        let kind = match kw {
                            "open" => CmdKind::Open,
                            "variable" => CmdKind::Variable,
                            _ => CmdKind::Universe,
                        };
                        let s = a + t.start as usize;
                        self.out.push(Event { kind: EventKind::Cmd { kind, until: usize::MAX }, start: s, end: a + t.end as usize, line: self.line_of(s) });
                        let k = self.out.len() - 1;
                        cur = Some(k);
                        // It ends with the scope it is written in.
                        if let Some((_, cmds)) = self.frames.last_mut() {
                            cmds.push(k);
                        }
                    }
                    _ => other = true,
                }
                i += 1;
                continue;
            }
            // Continuation lines are indented.
            if first && t.col == 0 {
                cur = None;
            }
            if let Some(c) = cur {
                match t.kind {
                    TokKind::Open => depth += 1,
                    TokKind::Close => depth -= 1,
                    _ => {}
                }
                if depth <= 0 && t.kind == TokKind::Ident && w == "in" {
                    // `open Foo in`: for the next declaration only.
                    if let EventKind::Cmd { until, .. } = &mut self.out[c].kind {
                        *until = b + 1;
                    }
                    ins.push(c);
                    cur = None;
                    depth = 0;
                } else {
                    self.out[c].end = a + t.end as usize;
                }
            }
            i += 1;
        }
        self.toks = toks;
    }
}

/// Where the lines start that may hold something this module reads: a
/// module doc, or one of the commands at the start of a line.
fn command_lines(text: &str, line_starts: &[usize]) -> Vec<usize> {
    const WORDS: [&str; 7] = ["open", "variable", "universe", "namespace", "section", "end", "mutual"];
    let word_end = |r: &str| !r.starts_with(|c: char| c.is_alphanumeric() || c == '_' || c == '.' || c == '\'');
    let mut out = Vec::new();
    for &a in line_starts {
        let rest = text[a..].trim_start_matches([' ', '\t']);
        let hit = match rest.as_bytes().first() {
            Some(b'/') => rest.starts_with("/-!"),
            Some(b'o' | b'v' | b'u' | b'n' | b's' | b'e' | b'm' | b'p') => {
                WORDS.iter().any(|w| rest.strip_prefix(w).is_some_and(word_end))
                    || ["noncomputable", "public", "private", "meta"].iter().any(|m| rest.strip_prefix(m).is_some_and(|r| r.trim_start_matches(' ').starts_with("section")))
            }
            _ => false,
        };
        if hit {
            out.push(a);
        }
    }
    out
}

/// Reads the gaps between `extents` (byte ranges of declarations, in file
/// order; nested ranges are fine). `line_starts` are the byte offsets of the
/// lines of `text`.
pub fn file_scope(text: &str, line_starts: &[usize], extents: &[(usize, usize)]) -> FileScope {
    let lines = &command_lines(text, line_starts);
    let mut w = Walk { text, line_starts, out: Vec::new(), frames: Vec::new(), toks: Vec::new(), lines };
    let mut pos = 0usize;
    for &(s, e) in extents {
        if s > pos {
            w.gap(pos, s);
        }
        pos = pos.max(e);
    }
    if pos < text.len() {
        w.gap(pos, text.len());
    }
    FileScope { events: w.out }
}

/// A command's text on one line.
pub fn one_line(text: &str, e: &Event) -> String {
    text.get(e.start..e.end).unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The namespaces an `open` command opens (`open scoped` opens no names).
pub fn opened(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut lx = Lexer::new(cmd, LexProfile::NEWEST);
    let mut first = true;
    while let Some(t) = lx.next_token() {
        let w = t.text(cmd);
        if t.is_comment() {
            continue;
        }
        if first {
            first = false;
            if w == "open" {
                continue;
            }
        }
        if t.kind != TokKind::Ident || matches!(w, "scoped" | "hiding" | "renaming" | "in") {
            break;
        }
        out.push(w.to_string());
    }
    out
}

/// One bracketed binder group of a `variable` command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub text: String,
    /// The names it binds (none for an anonymous instance `[Foo α]`).
    pub names: Vec<String>,
    /// Identifiers in its type (dotted ones by their first component too).
    pub uses: Vec<String>,
}

/// The binder groups of a `variable` command; empty when it has another shape.
pub fn groups(cmd: &str) -> Vec<Group> {
    let mut lx = Lexer::new(cmd, LexProfile::NEWEST);
    let mut toks = Vec::new();
    while let Some(t) = lx.next_token() {
        if !t.is_comment() {
            toks.push(t);
        }
    }
    let mut out = Vec::new();
    let mut i = 1; // after `variable`
    while i < toks.len() {
        if toks[i].kind != TokKind::Open {
            return Vec::new();
        }
        let mut depth = 0i32;
        let mut j = i;
        while j < toks.len() {
            match toks[j].kind {
                TokKind::Open => depth += 1,
                TokKind::Close => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                break;
            }
            j += 1;
        }
        if j >= toks.len() {
            return Vec::new();
        }
        let inner = &toks[i + 1..j];
        let colon = inner.iter().position(|t| t.text(cmd) == ":");
        let ids = |ts: &[Token]| -> Vec<String> { ts.iter().filter(|t| t.kind == TokKind::Ident).map(|t| t.text(cmd).to_string()).collect() };
        let (names, ty) = match colon {
            Some(c) => (ids(&inner[..c]), &inner[c + 1..]),
            // `[Foo α]` binds no name; `(x)` and `{α}` bind theirs.
            None if toks[i].text(cmd) == "[" => (Vec::new(), inner),
            None => (ids(inner), &inner[..0]),
        };
        let mut uses = Vec::new();
        for u in ids(ty) {
            if let Some((h, _)) = u.split_once('.') {
                uses.push(h.to_string());
            }
            uses.push(u);
        }
        out.push(Group { text: cmd[toks[i].start as usize..toks[j].end as usize].to_string(), names, uses });
        i = j + 1;
    }
    out
}

/// Which groups a declaration takes: those binding a name it writes, the
/// groups those mention, and anonymous instances about any of them.
pub fn taken(groups: &[Group], used: &std::collections::HashSet<&str>) -> Vec<bool> {
    let mut on = vec![false; groups.len()];
    let mut names: std::collections::HashSet<&str> = std::collections::HashSet::new();
    loop {
        let mut changed = false;
        for (k, g) in groups.iter().enumerate() {
            if on[k] {
                continue;
            }
            let hit = if g.names.is_empty() {
                g.uses.iter().any(|u| names.contains(u.as_str()))
            } else {
                g.names.iter().any(|n| used.contains(n.as_str()) || names.contains(n.as_str()))
            };
            if hit {
                on[k] = true;
                changed = true;
                names.extend(g.names.iter().map(|s| s.as_str()));
                // What its type mentions is taken along with it.
                for u in &g.uses {
                    if groups.iter().any(|x| x.names.iter().any(|n| n == u)) {
                        names.insert(u.as_str());
                    }
                }
            }
        }
        if !changed {
            return on;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "import A\n/-! # Title\n\nLead. -/\nnamespace N\nopen X Y.Z\nopen scoped T\nvariable {g : G} {C : F g.core}\n  [Inst g]\n\ntheorem a : True := trivial\n\nsection S\nvariable (n : Nat)\n/-! ## Part two -/\nopen Q in\ntheorem b : n = n := rfl\ntheorem c : True := trivial\nend S\n\ntheorem d : True := trivial\nend N\n";

    fn scope() -> (FileScope, Vec<(usize, usize)>) {
        let ex: Vec<(usize, usize)> = ["theorem a", "theorem b", "theorem c", "theorem d"]
            .iter()
            .map(|k| {
                let s = SRC.find(k).unwrap();
                (s, s + SRC[s..].find('\n').unwrap())
            })
            .collect();
        (file_scope(SRC, &crate::text::line_starts(SRC), &ex), ex)
    }

    #[test]
    fn commands_hold_in_their_scope_and_prose_is_kept() {
        let (fs, ex) = scope();
        let at = |k: usize| -> Vec<String> { fs.in_force(ex[k].0).map(|(e, _)| one_line(SRC, e)).collect() };
        let base = vec!["open X Y.Z", "open scoped T", "variable {g : G} {C : F g.core} [Inst g]"];
        assert_eq!(at(0), base);
        // `open Q in` is for `b` only; the section's variable ends with it.
        assert_eq!(at(1), [base.clone(), vec!["variable (n : Nat)", "open Q"]].concat());
        assert_eq!(at(2), [base.clone(), vec!["variable (n : Nat)"]].concat());
        assert_eq!(at(3), base);
        let prose: Vec<u32> = fs.events.iter().filter(|e| e.kind == EventKind::Prose).map(|e| e.line).collect();
        assert_eq!(prose, vec![2, 15]);
        // Only the scope that holds a command says where it ends.
        let ends: Vec<String> = fs.events.iter().filter(|e| e.kind == EventKind::End).map(|e| one_line(SRC, e)).collect();
        assert_eq!(ends, vec!["end S", "end N"]);
    }

    #[test]
    fn open_and_variable_commands_are_read() {
        assert_eq!(opened("open X Y.Z"), vec!["X", "Y.Z"]);
        assert_eq!(opened("open A hiding b"), vec!["A"]);
        assert!(opened("open scoped Topology").is_empty());
        let g = groups("variable {g : G} {C : F g.core} [Inst g] (x y : C.book)");
        assert_eq!(g.iter().map(|x| x.names.join(",")).collect::<Vec<_>>(), vec!["g", "C", "", "x,y"]);
        // `C` is written; `g` comes with its type, the instance with `g`.
        let used = ["C"].into_iter().collect();
        assert_eq!(taken(&g, &used), vec![true, true, true, false]);
    }
}
