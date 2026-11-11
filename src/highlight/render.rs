//! From stored spans to styled display rows, for the visible lines only.
//!
//! Compositing order, lowest first (`docs/highlighting.md`):
//!
//! 1. the view's base text style;
//! 2. the baseline (TextMate) style of the token;
//! 3. the semantic token's style, property by property;
//! 4. line overlays of the view (the current declaration's background),
//!    applied by the caller, which also keeps the text legible on them.
//!
//! Source bytes, characters and terminal cells stay distinct here: spans are
//! byte ranges of the source line; display text is the sanitised line (tabs
//! expanded, control characters made visible), wrapped by cell width.

use super::service::FileHighlight;
use super::textmate::{scopes, ScopeId};
use super::theme::{SemStyle, SyntaxTheme, TokStyle};
use crate::text::{sanitize_char, wrap_ranges};
use std::collections::HashMap;
use std::sync::Arc;

/// Resolves and remembers styles for one syntax theme. Changing the theme
/// means a new `Styler`; cached tokens are untouched.
pub struct Styler {
    theme: Arc<SyntaxTheme>,
    scope: HashMap<ScopeId, TokStyle>,
    /// Keyed by the legend's address: a legend lives as long as its file data.
    sem: HashMap<(usize, u16, u32), SemStyle>,
}

impl Styler {
    pub fn new(theme: Arc<SyntaxTheme>) -> Styler {
        Styler { theme, scope: HashMap::new(), sem: HashMap::new() }
    }

    pub fn theme(&self) -> &Arc<SyntaxTheme> {
        &self.theme
    }

    pub fn scope_style(&mut self, id: ScopeId) -> TokStyle {
        if let Some(s) = self.scope.get(&id) {
            return *s;
        }
        let s = self.theme.scope_style(&scopes().path(id));
        self.scope.insert(id, s);
        s
    }

    fn sem_style(&mut self, legend: &Arc<super::Legend>, ty: u16, mods: u32) -> SemStyle {
        let key = (Arc::as_ptr(legend) as usize, ty, mods);
        if let Some(s) = self.sem.get(&key) {
            return *s;
        }
        let name = legend.types.get(ty as usize).map(|s| s.as_str()).unwrap_or("");
        let ms: Vec<&str> = legend.modifiers.iter().enumerate().filter(|(i, _)| mods >> i & 1 == 1).map(|(_, m)| m.as_str()).collect();
        let s = self.theme.semantic_style(name, &ms);
        if self.sem.len() > 4096 {
            self.sem.clear();
        }
        self.sem.insert(key, s);
        s
    }

    /// Merged logical styles of 0-based line `n`: `(start byte, style)`,
    /// each running to the next start. Empty when the line has no data yet
    /// (the caller shows it plain).
    pub fn line_spans(&mut self, hl: &FileHighlight, n: usize, use_semantic: bool, out: &mut Vec<(u32, TokStyle)>) {
        out.clear();
        let Some(base) = hl.baseline.line(n).filter(|_| n < hl.baseline_lines_done) else { return };
        for &(start, id) in base {
            let st = self.scope_style(id);
            if out.last().is_none_or(|l| l.1 != st) {
                out.push((start, st));
            }
        }
        if out.is_empty() {
            out.push((0, TokStyle::default()));
        }
        let Some(sem) = hl.semantic.as_ref().filter(|_| use_semantic) else { return };
        let Some(toks) = sem.layer.line(n) else { return };
        if toks.is_empty() {
            return;
        }
        let legend = sem.legend.clone();
        let mut merged: Vec<(u32, TokStyle)> = Vec::with_capacity(out.len() + toks.len() * 2);
        let push = |m: &mut Vec<(u32, TokStyle)>, start: u32, st: TokStyle| {
            match m.last_mut() {
                Some(l) if l.0 == start => l.1 = st,
                Some(l) if l.1 == st => {}
                _ => m.push((start, st)),
            }
        };
        let mut ti = 0;
        for (i, &(start, st)) in out.iter().enumerate() {
            let end = out.get(i + 1).map(|x| x.0).unwrap_or(u32::MAX);
            let mut pos = start;
            while pos < end {
                // Skip tokens that ended before this position.
                while ti < toks.len() && toks[ti].end <= pos {
                    ti += 1;
                }
                match toks.get(ti) {
                    Some(t) if t.start <= pos => {
                        let s = self.sem_style(&legend, t.ty, t.mods);
                        push(&mut merged, pos, s.over(st));
                        pos = t.end.min(end);
                    }
                    Some(t) if t.start < end => {
                        push(&mut merged, pos, st);
                        pos = t.start;
                    }
                    _ => {
                        push(&mut merged, pos, st);
                        pos = end;
                    }
                }
            }
        }
        *out = merged;
    }
}

/// One display row: text segments with their logical style. `None` marks
/// text that is not source (the continuation indent).
pub type Row = Vec<(String, Option<TokStyle>)>;

/// Sanitises and wraps one source line exactly like
/// `wrap_line(&sanitize(line), width, hang)`, carrying each character's
/// style. `spans` are byte ranges of `line` as from [`Styler::line_spans`];
/// with no spans every character gets the default style.
pub fn styled_rows(line: &str, spans: &[(u32, TokStyle)], width: usize, hang: usize, ascii: bool) -> Vec<Row> {
    styled_rows_marked(line, spans, &[], width, hang, ascii).into_iter().map(|r| r.into_iter().map(|(s, st, _)| (s, st)).collect()).collect()
}

/// A display row whose segments also say whether they are marked.
pub type MarkedRow = Vec<(String, Option<TokStyle>, bool)>;

/// [`styled_rows`] with marks: `marks` are byte ranges of `line` (an
/// identifier the view points at); a segment is flagged when it lies in
/// one. Marks never change the text or where it wraps.
pub fn styled_rows_marked(line: &str, spans: &[(u32, TokStyle)], marks: &[(u32, u32)], width: usize, hang: usize, ascii: bool) -> Vec<MarkedRow> {
    let mut chars: Vec<char> = Vec::with_capacity(line.len());
    let mut styles: Vec<(TokStyle, bool)> = Vec::with_capacity(line.len());
    let mut si = 0;
    let mut buf = String::new();
    for (b, c) in line.char_indices() {
        while si + 1 < spans.len() && spans[si + 1].0 as usize <= b {
            si += 1;
        }
        let st = spans.get(si).filter(|s| s.0 as usize <= b).map(|s| s.1).unwrap_or_default();
        let marked = marks.iter().any(|&(a, z)| (a as usize) <= b && b < z as usize);
        buf.clear();
        sanitize_char(c, ascii, &mut buf);
        for d in buf.chars() {
            chars.push(d);
            styles.push((st, marked));
        }
    }
    let mut rows = Vec::new();
    for (a, z, indent) in wrap_ranges(&chars, width, hang) {
        let mut row: MarkedRow = Vec::new();
        if indent > 0 {
            row.push((" ".repeat(indent), None, false));
        }
        let mut i = a;
        while i < z {
            let st = styles[i];
            let mut j = i;
            while j < z && styles[j] == st {
                j += 1;
            }
            row.push((chars[i..j].iter().collect(), Some(st.0), st.1));
            i = j;
        }
        if a == z && indent == 0 {
            row.push((String::new(), Some(TokStyle::default()), false));
        }
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::highlight::theme::{BOLD, ITALIC};
    use crate::text::{sanitize, wrap_line};

    fn st(n: u8) -> TokStyle {
        TokStyle { fg: Some([n, n, n]), font: 0 }
    }

    fn text_of(rows: &[Row]) -> Vec<String> {
        rows.iter().map(|r| r.iter().map(|s| s.0.as_str()).collect()).collect()
    }

    #[test]
    fn wrapping_matches_plain_text_and_keeps_styles_on_characters() {
        // Tabs, a wide glyph, a combining mark, a non-BMP letter, a control
        // character and a bidi override, in a line that must wrap.
        let line = "\ttheorem α𝔽 (x : 中文) : e\u{301}x = \u{202E}y -- \u{1b}[31m tail words to force wrapping here";
        for width in [12usize, 20, 33, 200] {
            for ascii in [false, true] {
                let plain = wrap_line(&sanitize(line, ascii), width, 2);
                let rows = styled_rows(line, &[], width, 2, ascii);
                assert_eq!(text_of(&rows), plain, "width {width} ascii {ascii}");
            }
        }
        // Styles follow source bytes through sanitising: the tab (1 byte)
        // becomes two cells of the first style.
        let k = line.find("α").unwrap() as u32;
        let rows = styled_rows(line, &[(0, st(1)), (1, st(2)), (k, st(3)), (k + 6, st(4))], 200, 2, false);
        let r = &rows[0];
        assert_eq!(r[0], ("  ".to_string(), Some(st(1))));
        assert_eq!(r[1], ("theorem ".to_string(), Some(st(2))));
        assert_eq!(r[2], ("α𝔽".to_string(), Some(st(3))), "2-byte and 4-byte characters stay whole");
        assert!(r[3].0.starts_with(" (x : 中文)"));
        // Nothing is lost or duplicated.
        let all: String = rows.iter().flat_map(|r| r.iter().map(|s| s.0.clone())).collect();
        assert_eq!(all, sanitize(line, false));
    }

    #[test]
    fn marks_flag_byte_ranges_without_changing_text_or_wrapping() {
        // `α` is two bytes: the mark is given in bytes and lands on characters.
        let line = "theorem α_trans (h : α_trans x) : foo";
        let a = line.find("α_trans").unwrap() as u32;
        let b = line.rfind("α_trans").unwrap() as u32;
        let len = "α_trans".len() as u32;
        for width in [10usize, 16, 80] {
            let plain = styled_rows(line, &[(0, st(1)), (8, st(2))], width, 2, false);
            let rows = styled_rows_marked(line, &[(0, st(1)), (8, st(2))], &[(a, a + len), (b, b + len)], width, 2, false);
            assert_eq!(rows.len(), plain.len(), "width {width}: same wrapping");
            let text = |r: &MarkedRow| r.iter().map(|s| s.0.as_str()).collect::<String>();
            assert_eq!(rows.iter().map(text).collect::<Vec<_>>(), text_of(&plain));
            let marked: String = rows.iter().flat_map(|r| r.iter().filter(|s| s.2).map(|s| s.0.clone())).collect();
            assert_eq!(marked, "α_transα_trans", "width {width}: exactly the two occurrences");
            assert!(rows.iter().flatten().filter(|s| s.2).all(|s| s.1 == Some(st(2))), "token styles stay under the mark");
        }
        // No marks: nothing is flagged.
        assert!(styled_rows_marked(line, &[], &[], 80, 2, false).iter().flatten().all(|s| !s.2));
    }

    #[test]
    fn continuation_indent_is_not_source_text() {
        let rows = styled_rows("    aaaa bbbb cccc dddd eeee", &[(0, st(1))], 12, 2, false);
        assert!(rows.len() > 1);
        assert_eq!(rows[1][0].1, None, "the hanging indent carries no token style");
        assert!(rows[1][0].0.chars().all(|c| c == ' '));
        // An empty line is one empty row.
        assert_eq!(styled_rows("", &[], 10, 2, false).len(), 1);
    }

    #[test]
    fn semantic_tokens_overlay_baseline_spans() {
        use crate::highlight::service::{SemFacts, SemLayer};
        use crate::highlight::{Layer, Legend, Lines, SemTok};
        let theme = Arc::new(SyntaxTheme::builtin("dark-plus").unwrap());
        let kw = scopes().push_all(None, "source.lean4 keyword.other.lean4").unwrap();
        let root = scopes().push_all(None, "source.lean4").unwrap();
        let com = scopes().push_all(None, "source.lean4 comment.line.double-dash.lean4").unwrap();
        // "have foo.bar := x -- c"
        let mut base = Lines::default();
        base.push_line([(0, kw), (4, root), (18, com)]);
        let mut sem = Lines::default();
        // `foo` variable (5..8), `bar` property (9..12), `x` variable (16..17)
        sem.push_line([SemTok { start: 5, end: 8, ty: 1, mods: 0 }, SemTok { start: 9, end: 12, ty: 2, mods: 0 }, SemTok { start: 16, end: 17, ty: 1, mods: 0 }]);
        let legend = Arc::new(Legend { types: vec!["keyword".into(), "variable".into(), "property".into(), "leanSorryLike".into()], modifiers: vec![] });
        let mut hl = FileHighlight {
            rel: "A.lean".into(),
            sha256: "h".into(),
            n_lines: 1,
            baseline: Arc::new(Layer::from_chunks(vec![Arc::new(base)], 1)),
            baseline_lines_done: 1,
            baseline_from_cache: false,
            semantic: Some(SemLayer { layer: Arc::new(Layer::from_chunks(vec![Arc::new(sem)], 1)), legend, processed_lines: None }),
            facts: SemFacts::default(),
            cache_bytes: 0,
        };
        let mut s = Styler::new(theme);
        let mut out = Vec::new();
        s.line_spans(&hl, 0, false, &mut out);
        let blue = crate::highlight::theme::parse_hex("#569cd6");
        let var = crate::highlight::theme::parse_hex("#9cdcfe");
        let green = crate::highlight::theme::parse_hex("#6a9955");
        assert_eq!(out.iter().map(|x| (x.0, x.1.fg)).collect::<Vec<_>>(), vec![(0, blue), (4, None), (18, green)]);
        s.line_spans(&hl, 0, true, &mut out);
        assert_eq!(
            out.iter().map(|x| (x.0, x.1.fg)).collect::<Vec<_>>(),
            vec![(0, blue), (4, None), (5, var), (8, None), (9, var), (12, None), (16, var), (17, None), (18, green)],
            "lexical coverage stays wherever no semantic token applies"
        );
        // A semantic type without a style (leanSorryLike) changes nothing,
        // and font styles of the baseline survive a colour-only overlay.
        let mut b2 = Lines::default();
        let bold = scopes().push_all(None, "source.lean4 comment.block.documentation.lean4 markup.bold.markdown").unwrap();
        b2.push_line([(0, bold)]);
        let mut s2 = Lines::default();
        s2.push_line([SemTok { start: 0, end: 2, ty: 3, mods: 0 }, SemTok { start: 2, end: 4, ty: 1, mods: 0 }]);
        let legend2 = hl.semantic.as_ref().unwrap().legend.clone();
        hl.baseline = Arc::new(Layer::from_chunks(vec![Arc::new(b2)], 1));
        hl.semantic = Some(SemLayer { layer: Arc::new(Layer::from_chunks(vec![Arc::new(s2)], 1)), legend: legend2, processed_lines: None });
        s.line_spans(&hl, 0, true, &mut out);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].1.font & BOLD, BOLD);
        assert_eq!((out[1].0, out[1].1.fg, out[1].1.font & BOLD), (2, var, BOLD));
        assert_eq!(out[1].1.font & ITALIC, 0);
        // Lines without data yet: no spans at all.
        hl.baseline_lines_done = 0;
        s.line_spans(&hl, 0, true, &mut out);
        assert!(out.is_empty());
    }
}
