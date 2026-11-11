//! A conservative lexer for Lean 4 source text.
//!
//! Lean's syntax is extensible, so no lexer can be exact for arbitrary
//! projects. This one handles the parts that matter for not misreading a
//! file: nested block comments, doc and module-doc comments, string literals
//! (escapes, raw strings `r#"..."#`, and `s!`/`m!`/`f!`/`throwError`
//! interpolation with nested code), character literals, `«quoted»` name
//! parts, name literals, Unicode identifiers per Lean's own character classes
//! (which changed between toolchain versions, see [`LexProfile`]) and bracket
//! structure. Everything else becomes a one-character symbol token.

use memchr::{memchr, memchr2, memchr3};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokKind {
    Ident,
    Number,
    Str,
    Char,
    NameLit,
    LineComment,
    BlockComment,
    DocComment,
    ModDoc,
    /// `(`, `[`, `{`, `⦃`, `⟨`, `⟦`, `‹` and the attribute opener `@[`.
    Open,
    /// `)`, `]`, `}`, `⦄`, `⟩`, `⟧`, `›`.
    Close,
    /// `:=` or any other single character.
    Sym,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokKind,
    pub start: u32,
    pub end: u32,
    /// 1-based line of the first byte.
    pub line: u32,
    /// Byte column of the first byte.
    pub col: u32,
    /// First token on its line.
    pub first_on_line: bool,
}

impl Token {
    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.start as usize..self.end as usize]
    }
    pub fn is_comment(&self) -> bool {
        matches!(
            self.kind,
            TokKind::LineComment | TokKind::BlockComment | TokKind::DocComment | TokKind::ModDoc
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueKind {
    UnterminatedComment,
    UnterminatedString,
    UnterminatedQuotedName,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LexIssue {
    pub line: u32,
    pub kind: IssueKind,
}

/// Identifier character classes differ between Lean releases. Checked in
/// the toolchain sources (`Init/Meta*.lean`, `isLetterLike`/`isSubScriptAlnum`):
/// v4.16.0 has neither extension; v4.21.0 adds `ⱼ` (U+2C7C) as a subscript;
/// v4.28.0 and later also accept Latin-1 Supplement and Latin Extended-A
/// letters. Versions between those checked points use the newer behaviour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LexProfile {
    pub latin_letters: bool,
    pub subscript_j: bool,
}

impl LexProfile {
    pub const NEWEST: LexProfile = LexProfile { latin_letters: true, subscript_j: true };

    /// Chooses a profile from a `lean-toolchain` string such as
    /// `leanprover/lean4:v4.21.0`. Unknown strings get the newest rules.
    pub fn for_toolchain(toolchain: &str) -> LexProfile {
        match parse_lean_version(toolchain) {
            Some((4, minor, _)) if minor < 17 => LexProfile { latin_letters: false, subscript_j: false },
            Some((4, minor, _)) if minor < 22 => LexProfile { latin_letters: false, subscript_j: true },
            _ => LexProfile::NEWEST,
        }
    }
}

/// Parses `leanprover/lean4:v4.35.0-rc3` (or `v4.35.0`, `4.35.0`) into
/// `(major, minor, patch)`.
pub fn parse_lean_version(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.trim();
    let s = s.rsplit(':').next().unwrap_or(s);
    let s = s.trim_start_matches('v');
    let core = s.split('-').next()?;
    let mut it = core.split('.');
    let a = it.next()?.parse().ok()?;
    let b = it.next()?.parse().ok()?;
    let c = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    Some((a, b, c))
}

fn is_letter_like(c: char, p: LexProfile) -> bool {
    let v = c as u32;
    (0x3b1..=0x3c9).contains(&v) && v != 0x3bb
        || (0x391..=0x3a9).contains(&v) && v != 0x3a0 && v != 0x3a3
        || (0x3ca..=0x3fb).contains(&v)
        || (0x1f00..=0x1ffe).contains(&v)
        || (0x2100..=0x214f).contains(&v)
        || (0x1d49c..=0x1d59f).contains(&v)
        || p.latin_letters && ((0xc0..=0xff).contains(&v) && v != 0xd7 && v != 0xf7)
        || p.latin_letters && (0x100..=0x17f).contains(&v)
}

fn is_subscript_alnum(c: char, p: LexProfile) -> bool {
    let v = c as u32;
    (0x2080..=0x2089).contains(&v)
        || (0x2090..=0x209c).contains(&v)
        || (0x1d62..=0x1d6a).contains(&v)
        || p.subscript_j && v == 0x2c7c
}

pub fn is_id_first(c: char, p: LexProfile) -> bool {
    c.is_ascii_alphabetic() || c == '_' || (!c.is_ascii() && is_letter_like(c, p))
}

pub fn is_id_rest(c: char, p: LexProfile) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c, '_' | '\'' | '!' | '?')
        || (!c.is_ascii() && (is_letter_like(c, p) || is_subscript_alnum(c, p)))
}

fn is_open(c: char) -> bool {
    matches!(c, '(' | '[' | '{' | '⦃' | '⟨' | '⟦' | '‹')
}

fn is_close(c: char) -> bool {
    matches!(c, ')' | ']' | '}' | '⦄' | '⟩' | '⟧' | '›')
}

pub struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    line: u32,
    line_begin: usize,
    last_token_line: u32,
    profile: LexProfile,
    prev: Option<Token>,
    pub issues: Vec<LexIssue>,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str, profile: LexProfile) -> Lexer<'a> {
        let mut lx = Lexer {
            src,
            bytes: src.as_bytes(),
            pos: 0,
            line: 1,
            line_begin: 0,
            last_token_line: 0,
            profile,
            prev: None,
            issues: Vec::new(),
        };
        if src.starts_with('\u{feff}') {
            lx.pos = 3;
            lx.line_begin = 3;
        }
        lx
    }

    pub fn line(&self) -> u32 {
        self.line
    }

    fn peek_char(&self, at: usize) -> Option<char> {
        self.src.get(at..)?.chars().next()
    }

    /// Advances over `[from, to)`, counting newlines.
    fn advance_to(&mut self, to: usize) {
        let to = to.min(self.bytes.len());
        let mut i = self.pos;
        while let Some(off) = memchr(b'\n', &self.bytes[i..to]) {
            self.line += 1;
            i += off + 1;
            self.line_begin = i;
        }
        self.pos = to;
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b' ' | b'\t' | b'\r' => self.pos += 1,
                b'\n' => {
                    self.pos += 1;
                    self.line += 1;
                    self.line_begin = self.pos;
                }
                _ => break,
            }
        }
    }

    /// Scans a nested block comment starting at `self.pos` (which points at
    /// `/-`). Returns the end offset.
    fn block_comment_end(&mut self, body: usize, start_line: u32) -> usize {
        let b = self.bytes;
        let mut i = body;
        let mut depth = 1usize;
        while i < b.len() {
            match memchr2(b'-', b'/', &b[i..]) {
                None => break,
                Some(off) => {
                    let j = i + off;
                    if b[j] == b'-' && j + 1 < b.len() && b[j + 1] == b'/' {
                        depth -= 1;
                        i = j + 2;
                        if depth == 0 {
                            return i;
                        }
                    } else if b[j] == b'/' && j + 1 < b.len() && b[j + 1] == b'-' {
                        depth += 1;
                        i = j + 2;
                    } else {
                        i = j + 1;
                    }
                }
            }
        }
        self.issues.push(LexIssue { line: start_line, kind: IssueKind::UnterminatedComment });
        b.len()
    }

    /// End offset of a plain string literal starting at `start` (the `"`).
    fn string_end(&mut self, start: usize, interpolated: bool, start_line: u32) -> usize {
        let b = self.bytes;
        let mut i = start + 1;
        loop {
            let found = if interpolated { memchr3(b'"', b'\\', b'{', &b[i..]) } else { memchr2(b'"', b'\\', &b[i..]) };
            match found {
                None => {
                    self.issues.push(LexIssue { line: start_line, kind: IssueKind::UnterminatedString });
                    return b.len();
                }
                Some(off) => {
                    let j = i + off;
                    match b[j] {
                        b'"' => return j + 1,
                        b'\\' => i = (j + 2).min(b.len()),
                        _ => {
                            // `{` in an interpolated string: skip balanced code.
                            i = self.code_until_close_brace(j + 1);
                        }
                    }
                }
            }
        }
    }

    /// Inside string interpolation: lexes code until the `}` matching an
    /// already-consumed `{`, returning the offset after it.
    fn code_until_close_brace(&mut self, from: usize) -> usize {
        let saved_pos = self.pos;
        let saved_line = (self.line, self.line_begin, self.last_token_line);
        let saved_prev = self.prev;
        self.pos = from;
        let mut depth = 0i32;
        let mut end = self.bytes.len();
        while let Some(t) = self.next_token() {
            let txt = t.text(self.src);
            if t.kind == TokKind::Open && txt == "{" {
                depth += 1;
            } else if t.kind == TokKind::Close && txt == "}" {
                if depth == 0 {
                    end = t.end as usize;
                    break;
                }
                depth -= 1;
            }
        }
        self.pos = saved_pos;
        self.line = saved_line.0;
        self.line_begin = saved_line.1;
        self.last_token_line = saved_line.2;
        self.prev = saved_prev;
        end
    }

    fn raw_string_end(&mut self, start: usize, start_line: u32) -> Option<usize> {
        // r#*" ... "#*
        let b = self.bytes;
        let mut i = start + 1;
        let mut hashes = 0;
        while i < b.len() && b[i] == b'#' {
            hashes += 1;
            i += 1;
        }
        if i >= b.len() || b[i] != b'"' {
            return None;
        }
        i += 1;
        loop {
            match memchr(b'"', &b[i..]) {
                None => {
                    self.issues.push(LexIssue { line: start_line, kind: IssueKind::UnterminatedString });
                    return Some(b.len());
                }
                Some(off) => {
                    let j = i + off + 1;
                    if b[j..].iter().take(hashes).filter(|&&c| c == b'#').count() == hashes
                        && j + hashes <= b.len()
                    {
                        return Some(j + hashes);
                    }
                    i = j;
                }
            }
        }
    }

    fn char_literal_end(&self, start: usize) -> Option<usize> {
        // start points at '\''
        let mut i = start + 1;
        let c = self.peek_char(i)?;
        if c == '\\' {
            i += 1;
            let e = self.peek_char(i)?;
            match e {
                'x' => i += 3,
                'u' => i += 5,
                _ => i += e.len_utf8(),
            }
        } else if c == '\'' || c == '\n' {
            return None;
        } else {
            i += c.len_utf8();
        }
        if self.bytes.get(i) == Some(&b'\'') {
            Some(i + 1)
        } else {
            None
        }
    }

    /// Scans an identifier (possibly dotted, with `«»` parts) at `start`.
    fn ident_end(&mut self, start: usize) -> usize {
        let mut i = start;
        loop {
            // one component
            match self.peek_char(i) {
                Some('«') => match self.src[i..].find('»') {
                    Some(off) if !self.src[i..i + off].contains('\n') => i += off + '»'.len_utf8(),
                    _ => {
                        self.issues.push(LexIssue { line: self.line, kind: IssueKind::UnterminatedQuotedName });
                        return i + '«'.len_utf8();
                    }
                },
                Some(c) if is_id_first(c, self.profile) => {
                    i += c.len_utf8();
                    while let Some(c) = self.peek_char(i) {
                        if is_id_rest(c, self.profile) {
                            i += c.len_utf8();
                        } else {
                            break;
                        }
                    }
                }
                _ => return i,
            }
            // continue with `.` + component?
            if self.bytes.get(i) == Some(&b'.') {
                match self.peek_char(i + 1) {
                    Some(c) if c == '«' || is_id_first(c, self.profile) => {
                        i += 1;
                        continue;
                    }
                    _ => return i,
                }
            }
            return i;
        }
    }

    fn prev_enables_interpolation(&self, at: usize) -> bool {
        match self.prev {
            Some(p) if p.kind == TokKind::Ident && p.end as usize == at => {
                let t = p.text(self.src);
                t.ends_with('!') || matches!(t, "throwError" | "throwErrorAt" | "dbg_trace")
            }
            Some(p) if p.kind == TokKind::Ident => {
                matches!(p.text(self.src), "throwError" | "throwErrorAt" | "dbg_trace")
            }
            _ => false,
        }
    }

    pub fn next_token(&mut self) -> Option<Token> {
        self.skip_ws();
        if self.pos >= self.bytes.len() {
            return None;
        }
        let start = self.pos;
        let line = self.line;
        let col = (start - self.line_begin) as u32;
        let first_on_line = self.last_token_line != line;
        let b = self.bytes;
        let c0 = b[start];
        let c1 = b.get(start + 1).copied();
        let (kind, end) = if c0 == b'-' && c1 == Some(b'-') {
            let e = memchr(b'\n', &b[start..]).map(|o| start + o).unwrap_or(b.len());
            (TokKind::LineComment, e)
        } else if c0 == b'/' && c1 == Some(b'-') {
            // As in Lean's tokenizer, `/--` and `/-!` always start doc
            // comments whose body is scanned after the three-character opener.
            let (k, body) = match b.get(start + 2) {
                Some(b'-') => (TokKind::DocComment, start + 3),
                Some(b'!') => (TokKind::ModDoc, start + 3),
                _ => (TokKind::BlockComment, start + 2),
            };
            let e = self.block_comment_end(body, line);
            (k, e)
        } else if c0 == b'"' {
            let interp = self.prev_enables_interpolation(start);
            let e = self.string_end(start, interp, line);
            (TokKind::Str, e)
        } else if c0 == b'r' && matches!(c1, Some(b'"') | Some(b'#')) {
            match self.raw_string_end(start, line) {
                Some(e) => (TokKind::Str, e),
                None => (TokKind::Ident, self.ident_end(start)),
            }
        } else if c0 == b'\'' {
            match self.char_literal_end(start) {
                Some(e) => (TokKind::Char, e),
                None => (TokKind::Sym, start + 1),
            }
        } else if c0 == b'`' {
            // ``Name, `Name are name literals; `( and `[ open quotations.
            let mut i = start + 1;
            if b.get(i) == Some(&b'`') {
                i += 1;
            }
            match self.peek_char(i) {
                Some(c) if c == '«' || is_id_first(c, self.profile) => (TokKind::NameLit, self.ident_end(i)),
                _ => (TokKind::Sym, start + 1),
            }
        } else if c0.is_ascii_digit() {
            let mut i = start + 1;
            while i < b.len() {
                let c = b[i];
                if c.is_ascii_alphanumeric() || c == b'_' || (c == b'.' && b.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
                    i += 1;
                } else {
                    break;
                }
            }
            (TokKind::Number, i)
        } else if c0 == b':' && c1 == Some(b'=') {
            (TokKind::Sym, start + 2)
        } else if c0 == b'@' && c1 == Some(b'[') {
            (TokKind::Open, start + 2)
        } else {
            let c = self.peek_char(start).unwrap_or('\u{fffd}');
            if c == '«' || is_id_first(c, self.profile) {
                (TokKind::Ident, self.ident_end(start))
            } else if is_open(c) {
                (TokKind::Open, start + c.len_utf8())
            } else if is_close(c) {
                (TokKind::Close, start + c.len_utf8())
            } else {
                (TokKind::Sym, start + c.len_utf8())
            }
        };
        let end = end.max(start + 1).min(b.len());
        // keep char boundaries
        let mut end = end;
        while end < b.len() && !self.src.is_char_boundary(end) {
            end += 1;
        }
        self.advance_to(end);
        self.last_token_line = self.line;
        // A multi-line token ends on a later line; tokens after it on that
        // line are not first on their line.
        let tok = Token { kind, start: start as u32, end: end as u32, line, col, first_on_line };
        if !tok.is_comment() {
            self.prev = Some(tok);
        }
        Some(tok)
    }
}

impl Iterator for Lexer<'_> {
    type Item = Token;
    fn next(&mut self) -> Option<Token> {
        self.next_token()
    }
}

/// Convenience: all tokens of `src`.
pub fn tokenize(src: &str, profile: LexProfile) -> (Vec<Token>, Vec<LexIssue>) {
    let mut lx = Lexer::new(src, profile);
    let mut v = Vec::new();
    while let Some(t) = lx.next_token() {
        v.push(t);
    }
    (v, lx.issues)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<(TokKind, String)> {
        let (toks, _) = tokenize(src, LexProfile::NEWEST);
        toks.iter().map(|t| (t.kind, t.text(src).to_string())).collect()
    }

    fn idents(src: &str) -> Vec<String> {
        kinds(src).into_iter().filter(|(k, _)| *k == TokKind::Ident).map(|(_, s)| s).collect()
    }

    #[test]
    fn nested_comments_hide_declarations() {
        let src = "/- outer /- inner theorem fake -/ still comment def fake2 -/ theorem real : True := trivial";
        assert_eq!(idents(src), vec!["theorem", "real", "True", "trivial"]);
        let (_, issues) = tokenize("/- /- -/ unterminated", LexProfile::NEWEST);
        assert_eq!(issues[0].kind, IssueKind::UnterminatedComment);
    }

    #[test]
    fn doc_and_module_doc() {
        let k = kinds("/-! module doc -/\n/-- decl doc -/\ndef x := 1\n/--/ odd -/");
        assert_eq!(k[0].0, TokKind::ModDoc);
        assert_eq!(k[1].0, TokKind::DocComment);
        assert_eq!(k.last().unwrap(), &(TokKind::DocComment, "/--/ odd -/".to_string()));
        let k = kinds("/- a -/ /-/ b -/");
        assert_eq!(k.len(), 2);
        assert!(k.iter().all(|(t, _)| *t == TokKind::BlockComment));
    }

    #[test]
    fn strings_hide_keywords() {
        let src = r##"def s := "theorem in \" string -/ /-" def t := r#"raw " theorem"# def u := 'x' def v := '\'' "##;
        let ids = idents(src);
        assert_eq!(ids, vec!["def", "s", "def", "t", "def", "u", "def", "v"]);
    }

    #[test]
    fn interpolated_strings_with_nested_code() {
        let src = r#"def m := s!"a {f "}" (g "x")} b" theorem after : True := trivial"#;
        let ids = idents(src);
        assert_eq!(ids, vec!["def", "m", "s!", "theorem", "after", "True", "trivial"]);
        let src2 = r#"throwError "bad {x} here" def k := 1"#;
        assert_eq!(idents(src2), vec!["throwError", "def", "k"]);
    }

    #[test]
    fn identifiers_unicode_and_quoted() {
        let ids = idents("theorem reach_αβ₁ : ℕ := Demo.«weird name».x' + h.1 + f!");
        assert_eq!(ids, vec!["theorem", "reach_αβ₁", "ℕ", "Demo.«weird name».x'", "h", "f!"]);
        // λ is not an identifier character; Π and Σ are not either.
        let k = kinds("λx => x");
        assert_eq!(k[0], (TokKind::Sym, "λ".into()));
        assert_eq!(k[1], (TokKind::Ident, "x".into()));
    }

    #[test]
    fn version_profiles() {
        let old = LexProfile::for_toolchain("leanprover/lean4:v4.16.0");
        let mid = LexProfile::for_toolchain("leanprover/lean4:v4.21.0");
        let new = LexProfile::for_toolchain("leanprover/lean4:v4.35.0-rc3");
        assert!(!old.latin_letters && !old.subscript_j);
        assert!(!mid.latin_letters && mid.subscript_j);
        assert!(new.latin_letters && new.subscript_j);
        let src = "def café := 1";
        let (t_old, _) = tokenize(src, old);
        let (t_new, _) = tokenize(src, new);
        assert_eq!(t_old[1].text(src), "caf");
        assert_eq!(t_new[1].text(src), "café");
        assert_eq!(parse_lean_version("leanprover/lean4:v4.35.0-rc3"), Some((4, 35, 0)));
        assert_eq!(parse_lean_version("v4.9.1"), Some((4, 9, 1)));
        assert_eq!(parse_lean_version("nightly"), None);
    }

    #[test]
    fn char_literals_vs_primes() {
        let k = kinds("f' 'a' '\\n' x'' '\\x41'");
        assert_eq!(k[0], (TokKind::Ident, "f'".into()));
        assert_eq!(k[1].0, TokKind::Char);
        assert_eq!(k[2].0, TokKind::Char);
        assert_eq!(k[3], (TokKind::Ident, "x''".into()));
        assert_eq!(k[4].0, TokKind::Char);
    }

    #[test]
    fn positions_and_lines() {
        let src = "namespace A\n  /- c\n  c -/ theorem x : True := trivial\n";
        let (toks, _) = tokenize(src, LexProfile::NEWEST);
        let thm = toks.iter().find(|t| t.text(src) == "theorem").unwrap();
        assert_eq!(thm.line, 3);
        assert!(!thm.first_on_line);
        let ns = &toks[0];
        assert!(ns.first_on_line && ns.col == 0 && ns.line == 1);
        let comment = toks.iter().find(|t| t.kind == TokKind::BlockComment).unwrap();
        assert!(comment.first_on_line && comment.col == 2);
    }

    #[test]
    fn name_literals_and_quotations() {
        let k = kinds("``Nat.succ `foo `(term| x) :=");
        assert_eq!(k[0], (TokKind::NameLit, "``Nat.succ".into()));
        assert_eq!(k[1], (TokKind::NameLit, "`foo".into()));
        assert_eq!(k[2], (TokKind::Sym, "`".into()));
        assert_eq!(k[3], (TokKind::Open, "(".into()));
        assert_eq!(k.last().unwrap(), &(TokKind::Sym, ":=".into()));
    }

    #[test]
    fn bom_and_crlf() {
        let src = "\u{feff}def x := 1\r\ndef y := 2\r\n";
        let (toks, _) = tokenize(src, LexProfile::NEWEST);
        assert_eq!(toks[0].text(src), "def");
        assert_eq!(toks[0].col, 0);
        let y = toks.iter().find(|t| t.text(src) == "y").unwrap();
        assert_eq!(y.line, 2);
    }
}
