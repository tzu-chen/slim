//! Conservative declaration-candidate extraction from one Lean file.
//!
//! This is *not* a Lean parser. It recognises the command structure that
//! typical Lean 4 projects use (imports, namespaces/sections, doc comments,
//! attributes, modifiers and declaration keywords) and records candidates
//! together with everything that makes them uncertain. Names produced by
//! macros, custom commands, `deriving` clauses or anonymous instances cannot
//! be known without elaboration and are flagged rather than guessed.

use super::lexer::{tokenize, IssueKind, LexProfile, TokKind, Token};
use crate::model::{flags, DeclKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportRef {
    pub module: String,
    pub public: bool,
    pub all: bool,
    pub meta: bool,
    pub line: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// Namespace-qualified candidate name. For anonymous declarations this
    /// is a descriptive label and `flags::ANONYMOUS` is set.
    pub name: String,
    pub kind: DeclKind,
    /// Keyword as written (`lemma`, `theorem`, `class inductive`, ...).
    pub keyword: &'static str,
    pub flags: u32,
    pub doc: Option<String>,
    pub attrs: Option<String>,
    /// Byte offset where the command starts (doc comment, attributes or
    /// modifiers included).
    pub start: u32,
    pub start_line: u32,
    /// Byte offset of the declaration keyword.
    pub kw: u32,
    pub kw_line: u32,
    /// Byte offset where the statement header ends (`:=`, `where`, `|`).
    pub header_end: u32,
    /// Byte offset just after the last code token of the declaration.
    pub end: u32,
    pub end_line: u32,
    pub name_line: u32,
    /// Byte column of the name token (or keyword when anonymous).
    pub name_col: u32,
    pub namespace: String,
    pub sorry_line: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanIssue {
    pub line: u32,
    pub message: String,
}

#[derive(Clone, Debug, Default)]
pub struct FileScan {
    pub module_doc: Option<String>,
    pub module_system: bool,
    pub prelude: bool,
    pub imports: Vec<ImportRef>,
    pub decls: Vec<Candidate>,
    pub issues: Vec<ScanIssue>,
    pub line_count: u32,
    /// False when the lexer hit an unterminated comment/string/name: text
    /// after that point was not analysed reliably.
    pub lex_ok: bool,
    /// Number of command heads that were not recognised.
    pub unknown_commands: u32,
}

#[derive(Clone, Debug)]
enum Scope {
    Namespace(Vec<String>),
    Section(Option<String>),
    Mutual,
}

const DECL_KEYWORDS: &[&str] = &[
    "theorem", "lemma", "def", "abbrev", "instance", "example", "axiom", "opaque", "inductive",
    "coinductive", "structure", "class", "alias", "irreducible_def",
];

const MODIFIERS: &[&str] =
    &["private", "protected", "noncomputable", "unsafe", "partial", "nonrec", "public", "meta", "scoped", "local"];

const OTHER_COMMANDS: &[&str] = &[
    "namespace", "section", "end", "mutual", "open", "variable", "universe", "set_option", "attribute",
    "export", "import", "module", "prelude", "macro", "macro_rules", "syntax", "elab", "elab_rules",
    "notation", "notation3", "infix", "infixl", "infixr", "prefix", "postfix", "declare_syntax_cat",
    "initialize", "builtin_initialize", "deriving", "omit", "include", "add_decl_doc",
    "register_simp_attr", "simproc", "dsimproc", "run_cmd", "run_elab", "run_meta", "assert_not_exists",
    "assert_not_imported", "library_note", "suppress_compilation", "unseal", "seal",
    "compile_inductive", "initialize_simps_projections", "gen_injective_theorems%", "register_option",
    "register_builtin_option", "builtin_simproc", "binder_predicate", "unif_hint", "recommended_spelling", "grind_pattern", "extend_docs", "open_locale",
    "scoped[", "mk_iff_of_inductive_prop", "add_aesop_rules", "declare_aesop_rule_sets",
    // Project commands that tag existing declarations (Stacks project).
    "add_to_stacks", "add_to_ega", "add_to_sga",
];

/// Tokens that, even at column 0, continue the current command.
const CONTINUATIONS: &[&str] = &[
    "where", "with", "then", "else", "do", "fun", "at", "from", "using", "by", "calc", "show", "have",
    "let", "if", "match", "deriving", "extends", "termination_by", "decreasing_by", "in",
];

fn strip_doc(raw: &str) -> String {
    let inner = raw
        .strip_prefix("/--")
        .or_else(|| raw.strip_prefix("/-!"))
        .unwrap_or(raw);
    let inner = inner.strip_suffix("-/").unwrap_or(inner);
    // Dedent by the minimal indentation of non-empty continuation lines.
    let lines: Vec<&str> = inner.lines().collect();
    let min_indent = lines
        .iter()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start_matches(' ').len())
        .min()
        .unwrap_or(0);
    let mut out = String::new();
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
            out.push_str(l.get(min_indent.min(l.len() - l.trim_start_matches(' ').len())..).unwrap_or(""));
        } else {
            out.push_str(l.trim_start());
        }
    }
    out.trim().to_string()
}

fn modifier_flag(m: &str) -> u32 {
    match m {
        "private" => flags::PRIVATE,
        "protected" => flags::PROTECTED,
        "noncomputable" => flags::NONCOMPUTABLE,
        "unsafe" => flags::UNSAFE,
        "partial" => flags::PARTIAL,
        _ => 0,
    }
}

fn decl_kind(kw: &str) -> Option<(DeclKind, &'static str)> {
    Some(match kw {
        "theorem" => (DeclKind::Theorem, "theorem"),
        "lemma" => (DeclKind::Theorem, "lemma"),
        "def" => (DeclKind::Definition, "def"),
        "irreducible_def" => (DeclKind::Definition, "irreducible_def"),
        "abbrev" => (DeclKind::Abbrev, "abbrev"),
        "instance" => (DeclKind::Instance, "instance"),
        "axiom" => (DeclKind::Axiom, "axiom"),
        "opaque" => (DeclKind::Opaque, "opaque"),
        "inductive" => (DeclKind::Inductive, "inductive"),
        "coinductive" => (DeclKind::Inductive, "coinductive"),
        "structure" => (DeclKind::Structure, "structure"),
        "class" => (DeclKind::Class, "class"),
        "alias" => (DeclKind::Unknown, "alias"),
        _ => return None,
    })
}

struct Scanner<'a> {
    src: &'a str,
    toks: Vec<Token>,
    out: FileScan,
    scopes: Vec<Scope>,
    /// Index into `out.decls` of the declaration whose extent is open.
    cur: Option<usize>,
    /// Parent inductive/structure for constructor/field collection.
    body_owner: Option<usize>,
    last_code_end: u32,
    last_code_line: u32,
}

impl<'a> Scanner<'a> {
    fn text(&self, i: usize) -> &'a str {
        self.toks[i].text(self.src)
    }

    fn namespace(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        for s in &self.scopes {
            if let Scope::Namespace(p) = s {
                parts.extend(p.iter().map(|x| x.as_str()));
            }
        }
        parts.join(".")
    }

    fn in_mutual(&self) -> bool {
        self.scopes.iter().any(|s| matches!(s, Scope::Mutual))
    }

    fn issue(&mut self, line: u32, message: String) {
        if self.out.issues.len() < 200 {
            self.out.issues.push(ScanIssue { line, message });
        }
    }

    fn close_current(&mut self) {
        if let Some(c) = self.cur.take() {
            let d = &mut self.out.decls[c];
            if self.last_code_end > d.kw {
                d.end = self.last_code_end;
                d.end_line = self.last_code_line.max(d.kw_line);
            }
            if d.header_end == 0 || d.header_end > d.end {
                d.header_end = d.end;
            }
            // Children (constructors, fields) end with the parent at the latest.
            let (pend, pline) = (d.end, d.end_line);
            for child in self.out.decls[c + 1..].iter_mut() {
                if child.end == 0 || child.end > pend {
                    child.end = pend;
                    child.end_line = pline;
                }
                if child.header_end == 0 || child.header_end > child.end {
                    child.header_end = child.end;
                }
            }
        }
        self.body_owner = None;
    }

    /// Next non-comment token index after `i`.
    fn next_code(&self, i: usize) -> Option<usize> {
        (i + 1..self.toks.len()).find(|&j| !self.toks[j].is_comment())
    }

    /// Index of the token closing the bracket opened at `i`.
    fn matching_close(&self, i: usize) -> usize {
        let mut depth = 0i32;
        for j in i..self.toks.len() {
            match self.toks[j].kind {
                TokKind::Open => depth += 1,
                TokKind::Close => {
                    depth -= 1;
                    if depth == 0 {
                        return j;
                    }
                }
                _ => {}
            }
        }
        self.toks.len() - 1
    }

    fn parse_header(&mut self) -> usize {
        let n = self.toks.len();
        let mut i = 0;
        let mut seen_code = false;
        while i < n {
            let t = self.toks[i];
            match t.kind {
                TokKind::ModDoc => {
                    if self.out.module_doc.is_none() {
                        self.out.module_doc = Some(strip_doc(t.text(self.src)));
                    }
                    i += 1;
                    continue;
                }
                k if t.is_comment() => {
                    let _ = k;
                    i += 1;
                    continue;
                }
                _ => {}
            }
            let txt = t.text(self.src);
            if !seen_code && txt == "module" && t.kind == TokKind::Ident {
                self.out.module_system = true;
                seen_code = true;
                i += 1;
                continue;
            }
            seen_code = true;
            if txt == "prelude" {
                self.out.prelude = true;
                i += 1;
                continue;
            }
            // [public|private] [meta] import [all] Mod+
            let mut j = i;
            let mut public = false;
            let mut meta = false;
            loop {
                let w = self.text(j);
                if w == "public" || w == "private" {
                    public = w == "public";
                } else if w == "meta" {
                    meta = true;
                } else {
                    break;
                }
                match self.next_code(j) {
                    Some(k) => j = k,
                    None => return n,
                }
            }
            if self.text(j) != "import" {
                return i;
            }
            let line = self.toks[j].line;
            let mut k = match self.next_code(j) {
                Some(k) => k,
                None => return n,
            };
            let mut all = false;
            if self.text(k) == "all" || self.text(k) == "runtime" {
                all = self.text(k) == "all";
                k = match self.next_code(k) {
                    Some(k) => k,
                    None => return n,
                };
            }
            let mut any = false;
            // The first module name may be on a following line
            // (`public import` / newline / `Long.Module.Name`); further names
            // are taken from the same line as the first one.
            let name_line = if k < n { self.toks[k].line } else { line };
            while k < n && self.toks[k].kind == TokKind::Ident && (self.toks[k].line == name_line || !any) {
                let m = self.text(k);
                if matches!(m, "import" | "public" | "meta" | "private") {
                    break;
                }
                self.out.imports.push(ImportRef { module: m.to_string(), public, all, meta, line: self.toks[k].line });
                any = true;
                k += 1;
                while k < n && self.toks[k].is_comment() {
                    k += 1;
                }
            }
            if !any {
                self.issue(line, "import without module name".into());
            }
            i = k;
        }
        n
    }

    fn push_candidate(&mut self, c: Candidate) -> usize {
        self.out.decls.push(c);
        self.out.decls.len() - 1
    }

    fn qualify(&self, written: &str) -> String {
        if let Some(rest) = written.strip_prefix("_root_.") {
            return rest.to_string();
        }
        let ns = self.namespace();
        if ns.is_empty() {
            written.to_string()
        } else {
            format!("{ns}.{written}")
        }
    }

    /// Parses a declaration whose keyword is at `k`. `start` is the token
    /// where the command (doc/attrs/modifiers) began.
    fn parse_decl(&mut self, k: usize, start: usize, doc: Option<String>, attrs: Option<String>, mods: u32) -> usize {
        let kw_text = self.text(k);
        let mut kw_idx = k;
        let (mut kind, mut keyword) = match decl_kind(kw_text) {
            Some(x) => x,
            None => return k + 1,
        };
        if kw_text == "class" {
            if let Some(n) = self.next_code(k) {
                if self.text(n) == "inductive" {
                    kind = DeclKind::Class;
                    keyword = "class inductive";
                    kw_idx = n;
                }
            }
        }
        let kw_tok = self.toks[k];
        let mut fl = mods;
        if self.in_mutual() {
            fl |= flags::IN_MUTUAL;
        }
        if doc.is_some() {
            fl |= flags::HAS_DOC;
        }
        match kind {
            DeclKind::Class => fl |= flags::CLASS,
            DeclKind::Structure => fl |= flags::STRUCTURE,
            DeclKind::Instance => fl |= flags::INSTANCE,
            _ => {}
        }
        // Name.
        let mut j = match self.next_code(kw_idx) {
            Some(j) => j,
            None => self.toks.len(),
        };
        if kind == DeclKind::Instance && j < self.toks.len() && self.text(j) == "(" {
            // `instance (priority := n)`
            if let Some(p) = self.next_code(j) {
                if self.text(p) == "priority" {
                    let close = self.matching_close(j);
                    j = self.next_code(close).unwrap_or(self.toks.len());
                }
            }
        }
        let (name, name_tok, anonymous) = if j < self.toks.len() && self.toks[j].kind == TokKind::Ident
            && !matches!(self.text(j), "where" | "extends" | "deriving")
        {
            (self.qualify(self.text(j)), Some(j), false)
        } else {
            (String::new(), None, true)
        };
        if anonymous && kind != DeclKind::Instance {
            // Not something we understand (e.g. `alias ⟨a, b⟩ := c`).
            if keyword == "alias" {
                self.issue(kw_tok.line, "alias with anonymous-constructor syntax not expanded".into());
            }
            return j.max(k + 1);
        }
        // Header end: first `:=`, `where`, or line-leading `|` at relative depth 0.
        let mut depth = 0i32;
        let mut h = name_tok.map(|x| x + 1).unwrap_or(j);
        let mut header_end_tok = None;
        while h < self.toks.len() {
            let t = self.toks[h];
            if t.is_comment() {
                h += 1;
                continue;
            }
            if depth == 0 && t.first_on_line && self.is_strong_head(h) && t.col == 0 {
                break;
            }
            match t.kind {
                TokKind::Open => depth += 1,
                TokKind::Close => depth -= 1,
                _ => {}
            }
            if depth <= 0 {
                let w = t.text(self.src);
                if w == ":=" || (t.kind == TokKind::Ident && w == "where")
                    || (w == "|" && (t.first_on_line || matches!(kind, DeclKind::Inductive | DeclKind::Class)))
                {
                    header_end_tok = Some(h);
                    break;
                }
            }
            h += 1;
        }
        let header_end = match header_end_tok {
            Some(h) => self.toks[h].start,
            None => 0,
        };
        let label = if anonymous {
            // Describe the instance by its (short) type text.
            let from = self.toks[j.min(self.toks.len() - 1)].start as usize;
            let to = if header_end > 0 { header_end as usize } else { (from + 80).min(self.src.len()) };
            let mut s: String = self.src[from.min(to)..to].split_whitespace().collect::<Vec<_>>().join(" ");
            if s.starts_with(':') {
                s = s[1..].trim().to_string();
            }
            if s.chars().count() > 80 {
                s = s.chars().take(79).collect::<String>() + "…";
            }
            fl |= flags::ANONYMOUS;
            let ns = self.namespace();
            if ns.is_empty() {
                format!("instance : {s}")
            } else {
                format!("{ns}.instance : {s}")
            }
        } else {
            name
        };
        let (name_line, name_col) = match name_tok {
            Some(t) => (self.toks[t].line, self.toks[t].col),
            None => (kw_tok.line, kw_tok.col),
        };
        let st = self.toks[start];
        let idx = self.push_candidate(Candidate {
            name: label,
            kind,
            keyword,
            flags: fl,
            doc,
            attrs,
            start: st.start,
            start_line: st.line,
            kw: kw_tok.start,
            kw_line: kw_tok.line,
            header_end,
            end: 0,
            end_line: kw_tok.line,
            name_line,
            name_col,
            namespace: self.namespace(),
            sorry_line: None,
        });
        self.cur = Some(idx);
        if matches!(kind, DeclKind::Inductive | DeclKind::Structure | DeclKind::Class) && !anonymous {
            self.body_owner = Some(idx);
        }
        // Continue scanning right after the name (the main loop handles the
        // body so that sorry detection and constructors see every token).
        name_tok.map(|x| x + 1).unwrap_or(j)
    }

    /// Strong heads end a declaration even when bracket depth is confused.
    fn is_strong_head(&self, i: usize) -> bool {
        let t = self.toks[i];
        match t.kind {
            TokKind::DocComment | TokKind::ModDoc => true,
            TokKind::Open => t.text(self.src) == "@[",
            TokKind::Ident => {
                let w = t.text(self.src);
                DECL_KEYWORDS.contains(&w)
                    || MODIFIERS.contains(&w) && w != "meta" && w != "local" && w != "scoped"
                    || matches!(w, "namespace" | "section" | "end" | "mutual" | "open" | "variable" | "universe" | "set_option" | "attribute")
            }
            _ => false,
        }
    }

    fn is_head_word(w: &str) -> bool {
        DECL_KEYWORDS.contains(&w) || MODIFIERS.contains(&w) || OTHER_COMMANDS.contains(&w)
    }

    fn run(mut self) -> FileScan {
        let n = self.toks.len();
        let mut i = self.parse_header();
        let mut depth: i32 = 0;
        // Pending command prefix.
        let mut p_start: Option<usize> = None;
        let mut p_doc: Option<String> = None;
        let mut p_attrs: Vec<String> = Vec::new();
        let mut p_mods: u32 = 0;
        let mut in_head = false; // inside doc/attr/modifier prefix of a command
        let mut force_head = false; // after `... in`
        let mut prev_line_blank_before: u32 = 0;
        let mut last_tok_line: u32 = 0;
        while i < n {
            let t = self.toks[i];
            let w = t.text(self.src);
            let blank_before = t.first_on_line && t.line > last_tok_line + 1;
            if t.first_on_line {
                prev_line_blank_before = if blank_before { t.line } else { 0 };
            }
            let _ = prev_line_blank_before;
            // Comments: only doc comments at command level matter.
            if t.is_comment() {
                if t.kind == TokKind::ModDoc {
                    if self.out.module_doc.is_none() {
                        self.out.module_doc = Some(strip_doc(w));
                    }
                    if t.first_on_line && t.col == 0 {
                        self.close_current();
                    }
                } else if t.kind == TokKind::DocComment
                    && (in_head || (t.first_on_line && (depth == 0 || t.col == 0)) && (t.col == 0 || self.cur.is_none()))
                {
                    if !in_head {
                        if depth != 0 && t.col == 0 {
                            self.issue(t.line, "unbalanced brackets before this doc comment; depth reset".into());
                            depth = 0;
                        }
                        self.close_current();
                        p_start = Some(i);
                    }
                    p_doc = Some(strip_doc(w));
                    in_head = true;
                }
                last_tok_line = t.line + w.matches('\n').count() as u32;
                i += 1;
                continue;
            }
            last_tok_line = t.line + if t.kind == TokKind::Str { w.matches('\n').count() as u32 } else { 0 };

            let at_cmd_pos = in_head
                || force_head
                || (t.first_on_line && depth == 0)
                || (t.first_on_line && t.col == 0 && self.is_strong_head(i));
            if at_cmd_pos {
                let head_word = (t.kind == TokKind::Ident && Self::is_head_word(w)) || (t.kind == TokKind::Open && w == "@[") || (t.kind == TokKind::Sym && w == "#" && !in_head);
                if head_word {
                    if depth != 0 {
                        self.issue(t.line, "unbalanced brackets before this command; depth reset".into());
                        depth = 0;
                    }
                    force_head = false;
                    if !in_head {
                        self.close_current();
                        p_start = Some(i);
                    }
                    if w == "@[" {
                        let close = self.matching_close(i);
                        let a = &self.src[t.end as usize..self.toks[close].start as usize];
                        p_attrs.push(a.split_whitespace().collect::<Vec<_>>().join(" "));
                        in_head = true;
                        self.last_code_end = self.toks[close].end;
                        self.last_code_line = self.toks[close].line;
                        i = close + 1;
                        continue;
                    }
                    if MODIFIERS.contains(&w) {
                        // `noncomputable section`, `public section` are sections.
                        let next = self.next_code(i);
                        let next_w = next.map(|x| self.text(x)).unwrap_or("");
                        if next_w == "section" {
                            i = next.unwrap();
                            // fall through to section handling below
                        } else if matches!(w, "local" | "scoped")
                            && !matches!(next_w, "instance" | "notation" | "infix" | "infixl" | "infixr" | "prefix" | "postfix" | "macro" | "syntax" | "attribute" | "simp" | "elab" | "macro_rules")
                        {
                            in_head = false;
                            p_start = None;
                            i += 1;
                            continue;
                        } else {
                            p_mods |= modifier_flag(w);
                            in_head = true;
                            self.last_code_end = t.end;
                            self.last_code_line = t.line;
                            i += 1;
                            continue;
                        }
                    }
                    let w = self.text(i);
                    let ti = self.toks[i];
                    if decl_kind(w).is_some() && !(w == "instance" && in_head && false) {
                        let start = p_start.unwrap_or(i);
                        let doc = p_doc.take();
                        let attrs = if p_attrs.is_empty() { None } else { Some(p_attrs.join(", ")) };
                        p_attrs.clear();
                        let mods = std::mem::take(&mut p_mods);
                        in_head = false;
                        p_start = None;
                        self.last_code_end = ti.end;
                        self.last_code_line = ti.line;
                        i = self.parse_decl(i, start, doc, attrs, mods);
                        if let Some(prev) = i.checked_sub(1) {
                            self.last_code_end = self.toks[prev].end.max(self.last_code_end);
                            self.last_code_line = self.toks[prev].line.max(self.last_code_line);
                        }
                        continue;
                    }
                    // Non-declaration commands.
                    in_head = false;
                    p_start = None;
                    p_doc = None;
                    p_attrs.clear();
                    p_mods = 0;
                    match w {
                        "example" => {}
                        "namespace" => {
                            if let Some(nx) = self.next_code(i).filter(|&x| self.toks[x].kind == TokKind::Ident) {
                                let parts = crate::model::name_components(self.text(nx)).into_iter().map(String::from).collect();
                                self.scopes.push(Scope::Namespace(parts));
                                i = nx;
                            }
                        }
                        "section" => {
                            let nx = self.next_code(i).filter(|&x| {
                                self.toks[x].kind == TokKind::Ident && self.toks[x].line == ti.line && !Self::is_head_word(self.text(x))
                            });
                            let name = nx.map(|x| self.text(x).to_string());
                            self.scopes.push(Scope::Section(name));
                            if let Some(x) = nx {
                                i = x;
                            }
                        }
                        "mutual" => self.scopes.push(Scope::Mutual),
                        "end" => {
                            let nx = self.next_code(i).filter(|&x| {
                                self.toks[x].kind == TokKind::Ident && self.toks[x].line == ti.line && !Self::is_head_word(self.text(x))
                            });
                            match nx {
                                Some(x) => {
                                    let target = self.text(x).to_string();
                                    self.close_named(&target, ti.line);
                                    i = x;
                                }
                                None => match self.scopes.last() {
                                    Some(Scope::Section(None)) | Some(Scope::Mutual) => {
                                        self.scopes.pop();
                                    }
                                    Some(_) => {
                                        self.issue(ti.line, "`end` without name closes a named scope".into());
                                        self.scopes.pop();
                                    }
                                    None => self.issue(ti.line, "`end` without open scope".into()),
                                },
                            }
                        }
                        "open" | "set_option" | "attribute" | "variable" | "omit" | "include" | "unseal" | "seal" => {
                            // `... in` makes the next token a command head.
                            let mut d = 0i32;
                            let mut x = i + 1;
                            while x < n {
                                let tx = self.toks[x];
                                if tx.is_comment() {
                                    x += 1;
                                    continue;
                                }
                                if tx.first_on_line && d == 0 {
                                    break;
                                }
                                match tx.kind {
                                    TokKind::Open => d += 1,
                                    TokKind::Close => d -= 1,
                                    _ => {}
                                }
                                if d == 0 && tx.kind == TokKind::Ident && tx.text(self.src) == "in" {
                                    force_head = true;
                                    i = x;
                                    break;
                                }
                                x += 1;
                            }
                        }
                        _ => {}
                    }
                    self.last_code_end = self.toks[i].end;
                    self.last_code_line = self.toks[i].line;
                    i += 1;
                    continue;
                } else if !in_head && t.first_on_line && t.col == 0 && depth == 0 && t.kind == TokKind::Ident
                    && blank_before && !CONTINUATIONS.contains(&w) && self.cur.is_some()
                {
                    // An unrecognised column-0 word after a blank line is most
                    // likely a custom command: end the current extent.
                    self.out.unknown_commands += 1;
                    if self.out.unknown_commands <= 5 {
                        self.issue(t.line, format!("unrecognised command `{w}`: declarations it creates are not detected"));
                    }
                    self.close_current();
                } else if in_head {
                    // Prefix not followed by a declaration keyword.
                    in_head = false;
                    p_start = None;
                    p_doc = None;
                    p_attrs.clear();
                    p_mods = 0;
                }
            }
            {
                force_head = false;
                match t.kind {
                    TokKind::Open => depth += 1,
                    TokKind::Close => {
                        depth -= 1;
                        if depth < 0 {
                            depth = 0;
                        }
                    }
                    TokKind::Ident => {
                        if let Some(c) = self.cur {
                            match w {
                                "sorry" | "admit" => {
                                    let d = &mut self.out.decls[c];
                                    d.flags |= flags::LEX_SORRY;
                                    d.sorry_line.get_or_insert(t.line);
                                }
                                "native_decide" => self.out.decls[c].flags |= flags::LEX_NATIVE,
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
                // Constructors and fields of the open inductive/structure.
                if let Some(owner) = self.body_owner {
                    let od = &self.out.decls[owner];
                    let past_header = od.header_end > 0 && t.start >= od.header_end;
                    if past_header && depth == 0 {
                        self.collect_member(i, owner);
                    } else if past_header && depth == 1 && t.first_on_line && t.kind == TokKind::Open && w == "(" {
                        // binder-style structure field `(x y : T)` handled below
                    }
                }
                self.last_code_end = t.end;
                self.last_code_line = t.line + if t.kind == TokKind::Str { w.matches('\n').count() as u32 } else { 0 };
            }
            i += 1;
        }
        self.close_current();
        if !self.scopes.is_empty() {
            let open: Vec<String> = self
                .scopes
                .iter()
                .map(|s| match s {
                    Scope::Namespace(p) => format!("namespace {}", p.join(".")),
                    Scope::Section(Some(n)) => format!("section {n}"),
                    Scope::Section(None) => "section".into(),
                    Scope::Mutual => "mutual".into(),
                })
                .collect();
            // Unclosed sections/namespaces at EOF are legal in Lean 4.
            let _ = open;
        }
        self.out
    }

    fn collect_member(&mut self, i: usize, owner: usize) {
        let t = self.toks[i];
        let w = t.text(self.src);
        let (okind, oname, ons) = {
            let o = &self.out.decls[owner];
            (o.kind, o.name.clone(), o.namespace.clone())
        };
        let _ = ons;
        let push = |s: &mut Self, nm: &str, ti: Token, kind: DeclKind, fl: u32| {
            let full = format!("{oname}.{nm}");
            s.out.decls.push(Candidate {
                name: full,
                kind,
                keyword: if kind == DeclKind::Constructor { "constructor" } else { "field" },
                flags: fl,
                doc: None,
                attrs: None,
                start: ti.start,
                start_line: ti.line,
                kw: ti.start,
                kw_line: ti.line,
                header_end: 0,
                end: 0,
                end_line: ti.line,
                name_line: ti.line,
                name_col: ti.col,
                namespace: oname.clone(),
                sorry_line: None,
            });
        };
        match okind {
            DeclKind::Inductive | DeclKind::Class if w == "|" => {
                if let Some(nx) = self.next_code(i) {
                    let tn = self.toks[nx];
                    if tn.kind == TokKind::Ident {
                        let nm = tn.text(self.src).to_string();
                        // previous constructor ends here
                        self.end_last_member(owner, t);
                        push(self, &nm, tn, DeclKind::Constructor, 0);
                    }
                }
            }
            DeclKind::Structure | DeclKind::Class if t.first_on_line && t.kind == TokKind::Ident => {
                if matches!(w, "deriving" | "extends" | "where") {
                    return;
                }
                // `mk ::` custom constructor
                if let Some(nx) = self.next_code(i) {
                    if self.text(nx) == ":" {
                        if let Some(nn) = self.next_code(nx) {
                            if self.text(nn) == ":" && self.toks[nn].start == self.toks[nx].end {
                                self.end_last_member(owner, t);
                                push(self, w, t, DeclKind::Constructor, 0);
                                return;
                            }
                        }
                    }
                }
                // `x y : T` fields: identifiers up to `:` on this line
                let mut names = vec![(w.to_string(), t)];
                let mut x = i + 1;
                let mut ok = false;
                while x < self.toks.len() && self.toks[x].line == t.line {
                    let tx = self.toks[x];
                    let wx = tx.text(self.src);
                    if tx.kind == TokKind::Ident {
                        names.push((wx.to_string(), tx));
                    } else if wx == ":" || wx == ":=" || tx.kind == TokKind::Open {
                        ok = wx == ":" || tx.kind == TokKind::Open;
                        break;
                    } else {
                        break;
                    }
                    x += 1;
                }
                if ok {
                    self.end_last_member(owner, t);
                    for (nm, tk) in names {
                        push(self, &nm, tk, DeclKind::Definition, flags::PROJECTION);
                    }
                }
            }
            _ => {}
        }
    }

    fn end_last_member(&mut self, owner: usize, at: Token) {
        if let Some(last) = self.out.decls.len().checked_sub(1) {
            if last > owner && self.out.decls[last].end == 0 {
                let le = self.last_code_end.max(self.out.decls[last].kw);
                let ll = self.last_code_line;
                let d = &mut self.out.decls[last];
                d.end = le;
                d.end_line = ll.max(d.kw_line);
                let _ = at;
            }
        }
    }

    fn close_named(&mut self, target: &str, line: u32) {
        let comps: Vec<String> = crate::model::name_components(target).into_iter().map(String::from).collect();
        match self.scopes.last() {
            Some(Scope::Section(Some(n))) if n == target => {
                self.scopes.pop();
                return;
            }
            _ => {}
        }
        // Namespaces: pop scopes whose combined parts end with `comps`.
        let mut remaining = comps.len();
        while remaining > 0 {
            match self.scopes.pop() {
                Some(Scope::Namespace(p)) => {
                    let k = p.len().min(remaining);
                    if p[p.len() - k..] != comps[remaining - k..remaining] {
                        self.issue(line, format!("`end {target}` does not match the open namespace"));
                    }
                    remaining -= k;
                }
                Some(Scope::Section(n)) => {
                    self.issue(line, format!("`end {target}` closes section {}", n.unwrap_or_default()));
                    remaining -= 1;
                }
                Some(Scope::Mutual) => {
                    self.issue(line, format!("`end {target}` closes a mutual block"));
                    remaining -= 1;
                }
                None => {
                    self.issue(line, format!("`end {target}` without open scope"));
                    break;
                }
            }
        }
    }
}

/// Scans one file's text.
pub fn scan_source(src: &str, profile: LexProfile) -> FileScan {
    let (toks, lex_issues) = tokenize(src, profile);
    let line_count = src.lines().count() as u32;
    let mut sc = Scanner {
        src,
        toks,
        out: FileScan { line_count, lex_ok: lex_issues.is_empty(), ..Default::default() },
        scopes: Vec::new(),
        cur: None,
        body_owner: None,
        last_code_end: 0,
        last_code_line: 0,
    };
    for li in &lex_issues {
        let msg = match li.kind {
            IssueKind::UnterminatedComment => "unterminated block comment: rest of file unreadable",
            IssueKind::UnterminatedString => "unterminated string literal: rest of file unreadable",
            IssueKind::UnterminatedQuotedName => "unterminated «quoted name»",
        };
        sc.issue(li.line, msg.into());
    }
    if sc.toks.is_empty() {
        return sc.out;
    }
    sc.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(fs: &FileScan) -> Vec<(String, DeclKind)> {
        fs.decls.iter().map(|d| (d.name.clone(), d.kind)).collect()
    }

    const SAMPLE: &str = r#"module
/-! # Closure

Reflexive transitive closure. -/
public import Demo.Relation.Basic
import all Demo.Other
meta import Lean

namespace Demo.Rel

/-- The reflexive transitive closure. -/
inductive ReflTransGen (r : α → α → Prop) : α → α → Prop
  | refl : ReflTransGen r a a
  | tail : ReflTransGen r a b → r b c → ReflTransGen r a c

/-- Transitivity, used everywhere. -/
@[simp, stacks 00DS] theorem ReflTransGen.trans {a b c : α}
    (h₁ : ReflTransGen r a b) (h₂ : ReflTransGen r b c) : ReflTransGen r a c := by
  induction h₂ with
  | refl => exact h₁
  | tail _ h ih => exact .tail ih h

/- theorem commented_out : False := sorry -/
private lemma helper : True := trivial

theorem incomplete (x : Nat) : x = x := by
  sorry

structure Graph where
  edges : List (Nat × Nat)
  size n : Nat := 0
deriving Repr

instance : Inhabited Graph := ⟨⟨[], 0, 0⟩⟩

section Inner
variable (n : Nat)
open Nat in
theorem _root_.top_level : True := trivial
end Inner

end Demo.Rel

def «weird name» := 1
noncomputable section
def after_section := 2
end
"#;

    #[test]
    fn sample_structure() {
        let fs = scan_source(SAMPLE, LexProfile::NEWEST);
        assert!(fs.module_system);
        assert_eq!(fs.module_doc.as_deref(), Some("# Closure\n\nReflexive transitive closure."));
        assert_eq!(fs.imports.len(), 3);
        assert_eq!(fs.imports[0], ImportRef { module: "Demo.Relation.Basic".into(), public: true, all: false, meta: false, line: 5 });
        assert!(fs.imports[1].all);
        assert!(fs.imports[2].meta);
        let n = names(&fs);
        let expect = [
            ("Demo.Rel.ReflTransGen", DeclKind::Inductive),
            ("Demo.Rel.ReflTransGen.refl", DeclKind::Constructor),
            ("Demo.Rel.ReflTransGen.tail", DeclKind::Constructor),
            ("Demo.Rel.ReflTransGen.trans", DeclKind::Theorem),
            ("Demo.Rel.helper", DeclKind::Theorem),
            ("Demo.Rel.incomplete", DeclKind::Theorem),
            ("Demo.Rel.Graph", DeclKind::Structure),
            ("Demo.Rel.Graph.edges", DeclKind::Definition),
            ("Demo.Rel.Graph.size", DeclKind::Definition),
            ("Demo.Rel.Graph.n", DeclKind::Definition),
            ("Demo.Rel.instance : Inhabited Graph", DeclKind::Instance),
            ("top_level", DeclKind::Theorem),
            ("«weird name»", DeclKind::Definition),
            ("after_section", DeclKind::Definition),
        ];
        let got: Vec<(&str, DeclKind)> = n.iter().map(|(a, b)| (a.as_str(), *b)).collect();
        assert_eq!(got, expect.to_vec(), "{:#?}", fs.issues);
        let trans = &fs.decls[3];
        assert_eq!(trans.doc.as_deref(), Some("Transitivity, used everywhere."));
        assert_eq!(trans.attrs.as_deref(), Some("simp, stacks 00DS"));
        assert_eq!(trans.kw_line, 17);
        assert_eq!(trans.start_line, 16);
        assert!(trans.end_line >= 21, "extent covers the proof: {}", trans.end_line);
        let header = &SAMPLE[trans.kw as usize..trans.header_end as usize];
        assert!(header.ends_with("ReflTransGen r a c "), "{header:?}");
        assert_eq!(fs.decls[4].flags & flags::PRIVATE, flags::PRIVATE);
        assert_eq!(fs.decls[4].keyword, "lemma");
        assert!(fs.decls[5].flags & flags::LEX_SORRY != 0);
        assert_eq!(fs.decls[5].sorry_line, Some(27));
        // The commented-out sorry did not leak into helper.
        assert!(fs.decls[4].flags & flags::LEX_SORRY == 0);
        assert!(fs.decls[10].flags & flags::ANONYMOUS != 0);
        assert!(fs.lex_ok);
        assert!(fs.issues.is_empty(), "{:?}", fs.issues);
    }

    #[test]
    fn import_name_on_following_line() {
        let src = "module\n\npublic import A.B\npublic import\n  Very.Long.Module.Name\nimport C\n\ntheorem t : True := trivial\n";
        let fs = scan_source(src, LexProfile::NEWEST);
        let m: Vec<&str> = fs.imports.iter().map(|i| i.module.as_str()).collect();
        assert_eq!(m, vec!["A.B", "Very.Long.Module.Name", "C"]);
        assert!(fs.issues.is_empty(), "{:?}", fs.issues);
        assert_eq!(fs.decls.len(), 1);
    }

    #[test]
    fn doc_on_same_line_and_indented_namespace_bodies() {
        let src = "namespace A\n  /-- d -/ theorem x : True := trivial\n  def y := 1\nend A\n";
        let fs = scan_source(src, LexProfile::NEWEST);
        let n = names(&fs);
        assert_eq!(n, vec![("A.x".into(), DeclKind::Theorem), ("A.y".into(), DeclKind::Definition)]);
        assert_eq!(fs.decls[0].doc.as_deref(), Some("d"));
    }

    #[test]
    fn unbalanced_brackets_recover_at_strong_heads() {
        let src = "def broken := (1 + \n\ntheorem next : True := trivial\n";
        let fs = scan_source(src, LexProfile::NEWEST);
        assert!(names(&fs).iter().any(|(n, _)| n == "next"));
        assert!(!fs.issues.is_empty());
    }

    #[test]
    fn unterminated_comment_is_reported() {
        let src = "theorem a : True := trivial\n/- open comment\ntheorem hidden : True := trivial\n";
        let fs = scan_source(src, LexProfile::NEWEST);
        assert!(!fs.lex_ok);
        assert_eq!(names(&fs), vec![("a".into(), DeclKind::Theorem)]);
    }

    #[test]
    fn mutual_and_class_inductive() {
        let src = "mutual\ninductive Even : Nat → Prop\n  | zero : Even 0\ninductive Odd : Nat → Prop\n  | one : Odd 1\nend\nclass inductive Good : Prop\n  | intro\n";
        let fs = scan_source(src, LexProfile::NEWEST);
        let n = names(&fs);
        assert!(n.contains(&("Even.zero".into(), DeclKind::Constructor)));
        assert!(n.contains(&("Odd.one".into(), DeclKind::Constructor)));
        assert!(n.contains(&("Good".into(), DeclKind::Class)));
        assert!(n.contains(&("Good.intro".into(), DeclKind::Constructor)));
        assert!(fs.decls[0].flags & flags::IN_MUTUAL != 0);
    }

    #[test]
    fn set_option_in_same_line() {
        let src = "set_option maxHeartbeats 400 in theorem slow : True := trivial\nopen Foo in\ndef f := 1\n";
        let fs = scan_source(src, LexProfile::NEWEST);
        assert_eq!(names(&fs), vec![("slow".into(), DeclKind::Theorem), ("f".into(), DeclKind::Definition)]);
    }

    #[test]
    fn pattern_matching_def_header() {
        let src = "def f : Nat → Nat\n  | 0 => 1\n  | n + 1 => f n\ntermination_by n => n\n";
        let fs = scan_source(src, LexProfile::NEWEST);
        let d = &fs.decls[0];
        let header = &src[d.kw as usize..d.header_end as usize];
        assert_eq!(header.trim_end(), "def f : Nat → Nat");
        assert_eq!(d.end_line, 4, "termination_by continues the declaration");
    }

    #[test]
    fn unknown_command_after_blank_line_ends_extent() {
        let src = "theorem a : True := by\n  trivial\n\nmy_custom_cmd foo\n";
        let fs = scan_source(src, LexProfile::NEWEST);
        assert_eq!(fs.decls[0].end_line, 2);
        assert_eq!(fs.unknown_commands, 1);
    }
}
