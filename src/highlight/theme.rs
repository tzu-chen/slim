//! Syntax themes: VS Code colour-theme files resolved the way VS Code does.
//!
//! * TextMate token colours: a port of `vscode-textmate`'s theme trie
//!   (`parseTheme`, `ThemeTrieElement`, parent-scope matching, specificity
//!   order, attribute inheritance along the scope stack).
//! * Semantic tokens: a port of VS Code's `ColorThemeData.getTokenStyle`
//!   (checked against VS Code 1.142.0): the theme's `semanticTokenColors`
//!   first, then the default rules, which probe TextMate scopes (built-in
//!   map plus the Lean extension's `semanticTokenScopes`). A semantic style
//!   overrides the TextMate style per property and leaves the rest.
//!
//! The output is a logical style (colour and font-style bits), independent
//! of the terminal; `app::render` turns it into terminal cells.

use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

pub const DARK_PLUS: &str = include_str!("../../assets/themes/dark_plus.json");
pub const DARK_VS: &str = include_str!("../../assets/themes/dark_vs.json");
pub const LIGHT_PLUS: &str = include_str!("../../assets/themes/light_plus.json");
pub const LIGHT_VS: &str = include_str!("../../assets/themes/light_vs.json");
pub const HC_BLACK: &str = include_str!("../../assets/themes/hc_black.json");
pub const HC_LIGHT: &str = include_str!("../../assets/themes/hc_light.json");
/// VS Code release the bundled themes and the resolution rules come from.
pub const THEME_SOURCE: &str = "VS Code 1.142.0 theme-defaults";

pub const ITALIC: u8 = 1;
pub const BOLD: u8 = 2;
pub const UNDERLINE: u8 = 4;
pub const STRIKE: u8 = 8;

pub type Rgb = [u8; 3];

/// Resolved style of a token: `fg == None` means the default text colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TokStyle {
    pub fg: Option<Rgb>,
    pub font: u8,
}

/// What a semantic token changes; unset properties keep the syntax style.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SemStyle {
    pub fg: Option<Rgb>,
    pub italic: Option<bool>,
    pub bold: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
}

impl SemStyle {
    pub fn is_empty(&self) -> bool {
        *self == SemStyle::default()
    }
    pub fn over(&self, base: TokStyle) -> TokStyle {
        let mut out = base;
        if let Some(c) = self.fg {
            out.fg = Some(c);
        }
        for (v, bit) in [(self.italic, ITALIC), (self.bold, BOLD), (self.underline, UNDERLINE), (self.strike, STRIKE)] {
            match v {
                Some(true) => out.font |= bit,
                Some(false) => out.font &= !bit,
                None => {}
            }
        }
        out
    }
}

pub fn parse_hex(s: &str) -> Option<Rgb> {
    let h = s.strip_prefix('#')?;
    if !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = |i: usize, n: usize| u8::from_str_radix(&h[i..i + n], 16).ok();
    match h.len() {
        // #rgb, #rgba (alpha is ignored: terminals have no translucent text)
        3 | 4 => Some([v(0, 1)? * 17, v(1, 1)? * 17, v(2, 1)? * 17]),
        6 | 8 => Some([v(0, 2)?, v(2, 2)?, v(4, 2)?]),
        _ => None,
    }
}

fn font_bits(s: &str) -> u8 {
    let mut f = 0;
    for seg in s.split(' ') {
        f |= match seg {
            "italic" => ITALIC,
            "bold" => BOLD,
            "underline" => UNDERLINE,
            "strikethrough" => STRIKE,
            _ => 0,
        };
    }
    f
}

// ---------------------------------------------------------------------------
// TextMate theme trie (vscode-textmate `Theme`)
// ---------------------------------------------------------------------------

/// `font == -1`: not set. `fg == None`: not set.
#[derive(Clone, Debug)]
struct TrieRule {
    depth: usize,
    parents: Option<Vec<String>>,
    font: i8,
    fg: Option<Rgb>,
}

impl TrieRule {
    fn overwrite(&mut self, depth: usize, font: i8, fg: Option<Rgb>) {
        if depth >= self.depth {
            self.depth = depth;
        }
        if font != -1 {
            self.font = font;
        }
        if fg.is_some() {
            self.fg = fg;
        }
    }
}

#[derive(Clone, Debug)]
struct Trie {
    main: TrieRule,
    with_parents: Vec<TrieRule>,
    children: HashMap<String, Trie>,
}

fn str_arr_cmp(a: &Option<Vec<String>>, b: &Option<Vec<String>>) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;
    match (a, b) {
        (None, None) => Equal,
        (None, _) => Less,
        (_, None) => Greater,
        (Some(x), Some(y)) => {
            if x.len() == y.len() {
                x.iter().zip(y).map(|(p, q)| p.cmp(q)).find(|o| *o != Equal).unwrap_or(Equal)
            } else {
                x.len().cmp(&y.len())
            }
        }
    }
}

impl Trie {
    fn insert(&mut self, depth: usize, scope: &str, parents: &Option<Vec<String>>, font: i8, fg: Option<Rgb>) {
        if scope.is_empty() {
            match parents {
                None => self.main.overwrite(depth, font, fg),
                Some(_) => {
                    if let Some(r) = self.with_parents.iter_mut().find(|r| str_arr_cmp(&r.parents, parents).is_eq()) {
                        r.overwrite(depth, font, fg);
                        return;
                    }
                    self.with_parents.push(TrieRule {
                        depth,
                        parents: parents.clone(),
                        font: if font == -1 { self.main.font } else { font },
                        fg: fg.or(self.main.fg),
                    });
                }
            }
            return;
        }
        let (head, tail) = scope.split_once('.').unwrap_or((scope, ""));
        if !self.children.contains_key(head) {
            let child = Trie { main: self.main.clone(), with_parents: self.with_parents.clone(), children: HashMap::new() };
            self.children.insert(head.to_string(), child);
        }
        self.children.get_mut(head).unwrap().insert(depth + 1, tail, parents, font, fg);
    }

    /// Candidate rules for a scope name, most specific first.
    fn candidates(&self, scope: &str) -> Vec<&TrieRule> {
        if !scope.is_empty() {
            let (head, tail) = scope.split_once('.').unwrap_or((scope, ""));
            if let Some(c) = self.children.get(head) {
                return c.candidates(tail);
            }
        }
        let mut rules: Vec<&TrieRule> = self.with_parents.iter().chain(std::iter::once(&self.main)).collect();
        rules.sort_by(|a, b| cmp_specificity(a, b));
        rules
    }
}

fn cmp_specificity(a: &TrieRule, b: &TrieRule) -> std::cmp::Ordering {
    if a.depth != b.depth {
        return b.depth.cmp(&a.depth);
    }
    let empty = Vec::new();
    let (ap, bp) = (a.parents.as_ref().unwrap_or(&empty), b.parents.as_ref().unwrap_or(&empty));
    let (mut i, mut j) = (0, 0);
    loop {
        // Parent scopes with the child combinator are more specific.
        if ap.get(i).is_some_and(|s| s == ">") {
            i += 1;
        }
        if bp.get(j).is_some_and(|s| s == ">") {
            j += 1;
        }
        if i >= ap.len() || j >= bp.len() {
            break;
        }
        let d = bp[j].len().cmp(&ap[i].len());
        if d.is_ne() {
            return d;
        }
        i += 1;
        j += 1;
    }
    bp.len().cmp(&ap.len())
}

fn matches_scope(name: &str, pattern: &str) -> bool {
    name == pattern || (name.len() > pattern.len() && name.starts_with(pattern) && name.as_bytes()[pattern.len()] == b'.')
}

/// `path` is the scope path above the scope being matched, root first.
fn parents_match(path: &[Arc<str>], parents: &Option<Vec<String>>) -> bool {
    let Some(parents) = parents else { return true };
    let mut pos = path.len(); // index one past the current candidate
    let mut i = 0;
    while i < parents.len() {
        let mut pat = &parents[i];
        let mut must = false;
        if pat == ">" {
            if i == parents.len() - 1 {
                return false;
            }
            i += 1;
            pat = &parents[i];
            must = true;
        }
        loop {
            if pos == 0 {
                return false;
            }
            if matches_scope(&path[pos - 1], pat) {
                break;
            }
            if must {
                return false;
            }
            pos -= 1;
        }
        pos -= 1;
        i += 1;
    }
    true
}

// ---------------------------------------------------------------------------
// Semantic token rules
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Selector {
    ty: String,
    modifiers: Vec<String>,
    language: Option<String>,
}

/// `type.mod1.mod2:language`; `*` matches any type.
fn parse_selector(s: &str) -> Option<Selector> {
    let (body, language) = match s.split_once(':') {
        Some((b, l)) => (b, Some(l.to_string())),
        None => (s, None),
    };
    let mut parts = body.split('.');
    let ty = parts.next().filter(|t| !t.is_empty())?.to_string();
    Some(Selector { ty, modifiers: parts.map(String::from).collect(), language })
}

/// VS Code's built-in super types.
fn type_hierarchy(ty: &str) -> Vec<&str> {
    match ty {
        "member" => vec!["member", "method"],
        t => vec![t],
    }
}

impl Selector {
    fn score(&self, ty: &str, modifiers: &[&str], language: &str) -> i32 {
        let mut score = 0;
        if let Some(l) = &self.language {
            if l != language {
                return -1;
            }
            score += 10;
        }
        if self.ty != "*" {
            match type_hierarchy(ty).iter().position(|t| *t == self.ty) {
                Some(level) => score += 100 - level as i32,
                None => return -1,
            }
        }
        if self.modifiers.iter().any(|m| !modifiers.contains(&m.as_str())) {
            return -1;
        }
        score + self.modifiers.len() as i32 * 100
    }
}

/// VS Code's default rules: semantic selector → TextMate scopes to probe,
/// in registration order. The last entry is the Lean extension's
/// `semanticTokenScopes` contribution (vscode-lean4 0.0.240).
const DEFAULT_PROBES: &[(&str, &[&str])] = &[
    ("comment", &["comment"]),
    ("string", &["string"]),
    ("keyword", &["keyword.control"]),
    ("number", &["constant.numeric"]),
    ("regexp", &["constant.regexp"]),
    ("operator", &["keyword.operator"]),
    ("namespace", &["entity.name.namespace"]),
    ("type", &["entity.name.type", "support.type"]),
    ("struct", &["entity.name.type.struct"]),
    ("class", &["entity.name.type.class", "support.class"]),
    ("interface", &["entity.name.type.interface"]),
    ("enum", &["entity.name.type.enum"]),
    ("typeParameter", &["entity.name.type.parameter"]),
    ("function", &["entity.name.function", "support.function"]),
    ("method", &["entity.name.function.member", "support.function"]),
    ("macro", &["entity.name.function.preprocessor"]),
    ("variable", &["variable.other.readwrite", "entity.name.variable"]),
    ("parameter", &["variable.parameter"]),
    ("property", &["variable.other.property"]),
    ("enumMember", &["variable.other.enummember"]),
    ("event", &["variable.other.event"]),
    ("decorator", &["entity.name.decorator", "entity.name.function"]),
    ("variable.readonly", &["variable.other.constant"]),
    ("property.readonly", &["variable.other.constant.property"]),
    ("type.defaultLibrary", &["support.type"]),
    ("class.defaultLibrary", &["support.class"]),
    ("interface.defaultLibrary", &["support.class"]),
    ("variable.defaultLibrary", &["support.variable", "support.other.variable"]),
    ("variable.defaultLibrary.readonly", &["support.constant"]),
    ("property.defaultLibrary", &["support.variable.property"]),
    ("property.defaultLibrary.readonly", &["support.constant.property"]),
    ("function.defaultLibrary", &["support.function"]),
    ("member.defaultLibrary", &["support.function"]),
    ("keyword", &["keyword.other"]),
];

/// A raw `tokenColors` entry, kept for scope probing.
#[derive(Clone, Debug)]
struct RawRule {
    /// Each selector as its space-separated identifiers.
    selectors: Vec<Vec<String>>,
    fg: Option<Rgb>,
    font: Option<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeKind {
    Dark,
    Light,
    HcDark,
    HcLight,
}

pub struct SyntaxTheme {
    pub name: String,
    pub kind: ThemeKind,
    /// `editor.foreground` / `editor.background` of the theme file, if any.
    pub editor_fg: Option<Rgb>,
    pub editor_bg: Option<Rgb>,
    /// Whether the theme enables semantic highlighting (VS Code's
    /// `semanticHighlighting`); slim's own setting can override it.
    pub semantic_highlighting: bool,
    trie: Trie,
    default_font: u8,
    raw: Vec<RawRule>,
    semantic: Vec<(Selector, SemStyle)>,
    defaults: Vec<(Selector, Vec<String>)>,
}

/// Strips `//` and `/* */` comments and trailing commas (VS Code themes are
/// JSON with comments).
pub fn strip_jsonc(s: &str) -> String {
    let b: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut in_str = false;
    while i < b.len() {
        let c = b[i];
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < b.len() {
                out.push(b[i + 1]);
                i += 1;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            out.push(c);
        } else if c == '/' && b.get(i + 1) == Some(&'/') {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
            continue;
        } else if c == '/' && b.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        } else if c == ',' {
            let mut j = i + 1;
            while j < b.len() && b[j].is_whitespace() {
                j += 1;
            }
            if !matches!(b.get(j), Some('}') | Some(']')) {
                out.push(c);
            }
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

fn parse_json(text: &str) -> Result<Value> {
    serde_json::from_str(text).or_else(|_| serde_json::from_str(&strip_jsonc(text))).context("theme is not valid JSON")
}

impl SyntaxTheme {
    /// A bundled theme: `dark-plus`, `light-plus`, `hc-black`, `hc-light`.
    pub fn builtin(name: &str) -> Option<SyntaxTheme> {
        let (label, kind, layers): (&str, ThemeKind, &[&str]) = match name {
            "dark-plus" | "dark+" | "dark-modern" | "dark" => ("Dark+ (VS Code default dark)", ThemeKind::Dark, &[DARK_VS, DARK_PLUS]),
            "light-plus" | "light+" | "light-modern" | "light" => ("Light+ (VS Code default light)", ThemeKind::Light, &[LIGHT_VS, LIGHT_PLUS]),
            "hc-black" | "hc-dark" | "contrast" => ("Dark High Contrast (VS Code)", ThemeKind::HcDark, &[HC_BLACK]),
            "hc-light" => ("Light High Contrast (VS Code)", ThemeKind::HcLight, &[HC_LIGHT]),
            _ => return None,
        };
        let layers: Vec<Value> = layers.iter().map(|l| parse_json(l).expect("bundled theme parses")).collect();
        Some(SyntaxTheme::from_layers(label, kind, &layers))
    }

    pub const BUILTIN_NAMES: &'static [&'static str] = &["dark-plus", "light-plus", "hc-black", "hc-light"];

    /// A VS Code colour theme file (`.json`, comments allowed); `include`
    /// is followed relative to the file.
    pub fn from_file(path: &Path) -> Result<SyntaxTheme> {
        let mut layers = Vec::new();
        let mut cur = path.to_path_buf();
        for _ in 0..8 {
            let text = std::fs::read_to_string(&cur).with_context(|| format!("reading theme {}", cur.display()))?;
            if text.len() > 8 << 20 {
                bail!("theme {} is too large", cur.display());
            }
            let j = parse_json(&text).with_context(|| format!("theme {}", cur.display()))?;
            let inc = j.get("include").and_then(|v| v.as_str()).map(String::from);
            layers.push(j);
            match inc {
                Some(i) => cur = cur.parent().unwrap_or(Path::new(".")).join(i),
                None => break,
            }
        }
        layers.reverse();
        let top = layers.last().unwrap();
        if layers.iter().all(|l| l.get("tokenColors").and_then(|t| t.as_array()).is_none()) {
            bail!("theme {} has no tokenColors array (TextMate .tmTheme files are not supported)", path.display());
        }
        let kind = match top.get("type").and_then(|v| v.as_str()) {
            Some("light") => ThemeKind::Light,
            Some("hc") | Some("hcDark") => ThemeKind::HcDark,
            Some("hcLight") => ThemeKind::HcLight,
            Some(_) => ThemeKind::Dark,
            None => {
                // No declared type: decide from the editor background.
                let bg = layers.iter().rev().find_map(|l| l.pointer("/colors/editor.background").and_then(|v| v.as_str()).and_then(parse_hex));
                match bg {
                    Some([r, g, b]) if (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000 > 140 => ThemeKind::Light,
                    _ => ThemeKind::Dark,
                }
            }
        };
        let name = top.get("name").and_then(|v| v.as_str()).map(String::from).unwrap_or_else(|| path.display().to_string());
        Ok(SyntaxTheme::from_layers(&name, kind, &layers))
    }

    /// `layers`: included files first, so later layers override.
    fn from_layers(name: &str, kind: ThemeKind, layers: &[Value]) -> SyntaxTheme {
        struct Parsed {
            scope: String,
            parents: Option<Vec<String>>,
            index: usize,
            font: i8,
            fg: Option<Rgb>,
        }
        let mut parsed: Vec<Parsed> = Vec::new();
        let mut raw = Vec::new();
        let mut semantic = Vec::new();
        let mut semantic_highlighting = false;
        let (mut editor_fg, mut editor_bg) = (None, None);
        let mut index = 0;
        for layer in layers {
            if let Some(b) = layer.get("semanticHighlighting").and_then(|v| v.as_bool()) {
                semantic_highlighting = b;
            }
            if let Some(c) = layer.pointer("/colors/editor.foreground").and_then(|v| v.as_str()).and_then(parse_hex) {
                editor_fg = Some(c);
            }
            if let Some(c) = layer.pointer("/colors/editor.background").and_then(|v| v.as_str()).and_then(parse_hex) {
                editor_bg = Some(c);
            }
            for entry in layer.get("tokenColors").and_then(|t| t.as_array()).into_iter().flatten() {
                index += 1;
                let Some(settings) = entry.get("settings").filter(|s| s.is_object()) else { continue };
                let scopes: Vec<String> = match entry.get("scope") {
                    Some(Value::String(s)) => s.trim_matches(',').split(',').map(String::from).collect(),
                    Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
                    _ => vec![String::new()],
                };
                let font_str = settings.get("fontStyle").and_then(|v| v.as_str());
                let font: i8 = font_str.map(|s| font_bits(s) as i8).unwrap_or(-1);
                let fg = settings.get("foreground").and_then(|v| v.as_str()).and_then(parse_hex);
                let mut selectors = Vec::new();
                for sc in &scopes {
                    // Array entries may themselves hold comma-separated selectors
                    // for probing; the TextMate trie takes them as written.
                    let trimmed = sc.trim();
                    let segs: Vec<&str> = trimmed.split(' ').collect();
                    let scope = segs[segs.len() - 1].to_string();
                    let parents = (segs.len() > 1).then(|| segs[..segs.len() - 1].iter().rev().map(|s| s.to_string()).collect());
                    parsed.push(Parsed { scope, parents, index, font, fg });
                    for sel in sc.split(',') {
                        let ids: Vec<String> = sel.split_whitespace().map(String::from).collect();
                        if !ids.is_empty() {
                            selectors.push(ids);
                        }
                    }
                }
                if entry.get("scope").is_some() {
                    raw.push(RawRule { selectors, fg, font: font_str.map(font_bits) });
                }
            }
            for (sel, v) in layer.get("semanticTokenColors").and_then(|t| t.as_object()).into_iter().flatten() {
                let Some(selector) = parse_selector(sel) else { continue };
                let style = match v {
                    Value::String(c) => SemStyle { fg: parse_hex(c), ..Default::default() },
                    Value::Object(o) => {
                        let mut st = SemStyle { fg: o.get("foreground").and_then(|v| v.as_str()).and_then(parse_hex), ..Default::default() };
                        if let Some(fs) = o.get("fontStyle").and_then(|v| v.as_str()) {
                            let f = font_bits(fs);
                            st.italic = Some(f & ITALIC != 0);
                            st.bold = Some(f & BOLD != 0);
                            st.underline = Some(f & UNDERLINE != 0);
                            st.strike = Some(f & STRIKE != 0);
                        }
                        let b = |k: &str| o.get(k).and_then(|v| v.as_bool());
                        st.italic = b("italic").or(st.italic);
                        st.bold = b("bold").or(st.bold);
                        st.underline = b("underline").or(st.underline);
                        st.strike = b("strikethrough").or(st.strike);
                        st
                    }
                    _ => continue,
                };
                if !style.is_empty() {
                    semantic.push((selector, style));
                }
            }
        }
        parsed.sort_by(|a, b| a.scope.cmp(&b.scope).then_with(|| str_arr_cmp(&a.parents, &b.parents)).then(a.index.cmp(&b.index)));
        let mut default_font = 0u8;
        let mut rest = parsed.as_slice();
        while let Some(first) = rest.first().filter(|p| p.scope.is_empty()) {
            if first.font != -1 {
                default_font = first.font as u8;
            }
            if first.fg.is_some() {
                editor_fg = first.fg;
            }
            rest = &rest[1..];
        }
        let mut trie = Trie { main: TrieRule { depth: 0, parents: None, font: -1, fg: None }, with_parents: Vec::new(), children: HashMap::new() };
        for p in rest {
            trie.insert(0, &p.scope, &p.parents, p.font, p.fg);
        }
        let defaults = DEFAULT_PROBES
            .iter()
            .filter_map(|(sel, probes)| Some((parse_selector(sel)?, probes.iter().map(|s| s.to_string()).collect())))
            .collect();
        SyntaxTheme { name: name.to_string(), kind, editor_fg, editor_bg, semantic_highlighting, trie, default_font, raw, semantic, defaults }
    }

    /// TextMate style of a scope stack (root first), as VS Code computes
    /// token metadata: every scope on the stack may override what it
    /// inherits.
    pub fn scope_style(&self, path: &[Arc<str>]) -> TokStyle {
        let mut st = TokStyle { fg: None, font: self.default_font };
        for i in 0..path.len() {
            let cands = self.trie.candidates(&path[i]);
            if let Some(r) = cands.into_iter().find(|r| parents_match(&path[..i], &r.parents)) {
                if r.font != -1 {
                    st.font = r.font as u8;
                }
                if r.fg.is_some() {
                    st.fg = r.fg;
                }
            }
        }
        st
    }

    /// First probe scope for which some `tokenColors` rule defines a
    /// foreground or a font style (`resolveScopes`).
    fn probe(&self, probes: &[String]) -> Option<SemStyle> {
        for probe in probes {
            let (mut fg, mut font) = (None, None);
            let (mut fg_score, mut font_score) = (-1i64, -1i64);
            for rule in &self.raw {
                // A selector matches when each of its identifiers matches the
                // probed scope; the last identifier gives the score.
                let score = rule
                    .selectors
                    .iter()
                    .filter(|ids| ids.len() == 1 && matches_scope(probe, &ids[0]))
                    .map(|ids| 65536 + ids[0].len() as i64)
                    .max()
                    .unwrap_or(-1);
                if score < 0 {
                    continue;
                }
                if score >= fg_score && rule.fg.is_some() {
                    fg = rule.fg;
                    fg_score = score;
                }
                if score >= font_score && rule.font.is_some() {
                    font = rule.font;
                    font_score = score;
                }
            }
            if fg.is_some() || font.is_some() {
                let mut st = SemStyle { fg, ..Default::default() };
                if let Some(f) = font {
                    st.italic = Some(f & ITALIC != 0);
                    st.bold = Some(f & BOLD != 0);
                    st.underline = Some(f & UNDERLINE != 0);
                    st.strike = Some(f & STRIKE != 0);
                }
                return Some(st);
            }
        }
        None
    }

    /// Style of a semantic token type with modifiers (language `lean4`).
    pub fn semantic_style(&self, ty: &str, modifiers: &[&str]) -> SemStyle {
        const LANG: &str = "lean4";
        let mut out = SemStyle::default();
        // foreground, bold, underline, strike, italic
        let mut score = [-1i32; 5];
        let apply = |m: i32, st: &SemStyle, out: &mut SemStyle, score: &mut [i32; 5]| {
            if st.fg.is_some() && score[0] <= m {
                score[0] = m;
                out.fg = st.fg;
            }
            for (i, (src, dst)) in [(st.bold, &mut out.bold), (st.underline, &mut out.underline), (st.strike, &mut out.strike), (st.italic, &mut out.italic)]
                .into_iter()
                .enumerate()
            {
                if src.is_some() && score[i + 1] <= m {
                    score[i + 1] = m;
                    *dst = src;
                }
            }
        };
        for (sel, st) in &self.semantic {
            let m = sel.score(ty, modifiers, LANG);
            if m >= 0 {
                apply(m, st, &mut out, &mut score);
            }
        }
        // Properties set by the theme's own rules are never replaced by defaults.
        let mut undefined = false;
        for s in score.iter_mut() {
            if *s == -1 {
                undefined = true;
            } else {
                *s = i32::MAX;
            }
        }
        if undefined {
            for (sel, probes) in &self.defaults {
                let m = sel.score(ty, modifiers, LANG);
                if m >= 0 {
                    if let Some(st) = self.probe(probes) {
                        apply(m, &st, &mut out, &mut score);
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(s: &str) -> Vec<Arc<str>> {
        s.split(' ').map(Arc::from).collect()
    }

    #[test]
    fn dark_plus_textmate_colours() {
        let t = SyntaxTheme::builtin("dark-plus").unwrap();
        let fg = |s: &str| t.scope_style(&path(s)).fg;
        assert_eq!(fg("source.lean4"), None);
        assert_eq!(fg("source.lean4 keyword.other.lean4"), parse_hex("#569cd6"));
        assert_eq!(fg("source.lean4 comment.block.lean4"), parse_hex("#6a9955"));
        assert_eq!(fg("source.lean4 string.quoted.double.lean4"), parse_hex("#ce9178"));
        assert_eq!(fg("source.lean4 meta.definitioncommand.lean4 entity.name.function.lean4"), parse_hex("#dcdcaa"));
        assert_eq!(fg("source.lean4 constant.numeric.lean4"), parse_hex("#b5cea8"));
        assert_eq!(fg("source.lean4 invalid.illegal.lean4"), parse_hex("#f44747"));
        // Markdown inside a documentation comment: the heading rule gives bold.
        let h = t.scope_style(&path("source.lean4 comment.block.documentation.lean4 markup.heading.markdown heading.1.markdown"));
        assert_eq!(h.font & BOLD, BOLD);
        assert_eq!(h.fg, parse_hex("#569cd6"));
    }

    #[test]
    fn semantic_tokens_follow_vscode_resolution() {
        let t = SyntaxTheme::builtin("dark-plus").unwrap();
        assert!(t.semantic_highlighting);
        // The Lean extension maps `keyword` to `keyword.other`, registered
        // after the built-in `keyword.control`: the later equal score wins.
        assert_eq!(t.semantic_style("keyword", &[]).fg, parse_hex("#569cd6"));
        assert_eq!(t.semantic_style("variable", &[]).fg, parse_hex("#9cdcfe"));
        assert_eq!(t.semantic_style("property", &[]).fg, parse_hex("#9cdcfe"));
        assert_eq!(t.semantic_style("function", &[]).fg, parse_hex("#dcdcaa"));
        // No rule and no probe scope: the syntax colour stays.
        assert!(t.semantic_style("leanSorryLike", &[]).is_empty());
        assert_eq!(t.semantic_style("variable", &["readonly"]).fg, parse_hex("#4fc1ff"));
        let l = SyntaxTheme::builtin("light-plus").unwrap();
        assert_eq!(l.semantic_style("variable", &[]).fg, parse_hex("#001080"));
        assert_eq!(l.semantic_style("keyword", &[]).fg, parse_hex("#0000ff"));
        assert_eq!(l.semantic_style("function", &[]).fg, parse_hex("#795e26"));
    }

    #[test]
    fn semantic_overrides_only_what_it_defines() {
        let base = TokStyle { fg: Some([1, 2, 3]), font: BOLD };
        let s = SemStyle { fg: Some([9, 9, 9]), italic: Some(true), ..Default::default() };
        assert_eq!(s.over(base), TokStyle { fg: Some([9, 9, 9]), font: BOLD | ITALIC });
        assert_eq!(SemStyle { bold: Some(false), ..Default::default() }.over(base), TokStyle { fg: Some([1, 2, 3]), font: 0 });
    }

    #[test]
    fn user_theme_with_comments_parents_and_semantic_rules() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("base.json"),
            r##"{ "tokenColors": [ { "scope": "comment", "settings": { "foreground": "#111111", "fontStyle": "italic" } } ] }"##,
        )
        .unwrap();
        let p = d.path().join("t.json");
        std::fs::write(
            &p,
            r##"{
              // a comment
              "name": "T", "type": "light", "include": "./base.json",
              "colors": { "editor.foreground": "#222222", },
              "tokenColors": [
                { "scope": ["keyword", "comment markup.bold"], "settings": { "foreground": "#333" } },
                { "scope": "source.lean4 > keyword.other", "settings": { "fontStyle": "underline" } },
              ],
              "semanticTokenColors": { "variable:lean4": { "foreground": "#444444", "bold": true }, "variable.declaration": "#555555", "*.declaration": "#666666" }
            }"##,
        )
        .unwrap();
        let t = SyntaxTheme::from_file(&p).unwrap();
        assert_eq!(t.kind, ThemeKind::Light);
        assert_eq!(t.editor_fg, Some([0x22; 3]));
        let c = t.scope_style(&path("source.lean4 comment.block.lean4"));
        assert_eq!((c.fg, c.font), (Some([0x11; 3]), ITALIC));
        // Parent selector: bold markup inside a comment; the italic is inherited.
        let b = t.scope_style(&path("source.lean4 comment.block.lean4 markup.bold.markdown"));
        assert_eq!((b.fg, b.font), (Some([0x33; 3]), ITALIC));
        assert_eq!(t.scope_style(&path("source.lean4 markup.bold.markdown")).fg, None);
        // Child combinator.
        let k = t.scope_style(&path("source.lean4 keyword.other.lean4"));
        assert_eq!((k.fg, k.font), (Some([0x33; 3]), UNDERLINE));
        let k2 = t.scope_style(&path("source.lean4 meta.x keyword.other.lean4"));
        assert_eq!(k2.font, 0);
        let v = t.semantic_style("variable", &[]);
        assert_eq!((v.fg, v.bold), (Some([0x44; 3]), Some(true)));
        // Higher score (type and modifier) wins for the colour; the language
        // rule (110) still beats the wildcard (100). Bold stays.
        let vd = t.semantic_style("variable", &["declaration"]);
        assert_eq!((vd.fg, vd.bold), (Some([0x55; 3]), Some(true)));
        assert!(SyntaxTheme::from_file(&d.path().join("missing.json")).is_err());
    }
}
