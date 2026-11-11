//! A TextMate grammar interpreter: the baseline (syntax) highlighter.
//!
//! This is a port of the tokenizer of `vscode-textmate` 9.3 (the engine VS
//! Code uses), running the Lean extension's own JSON grammars unchanged on
//! Oniguruma, the same regular-expression library VS Code uses. It keeps
//! whole scope stacks, so any TextMate theme can be resolved afterwards.
//! Offsets are UTF-8 bytes within a line.
//!
//! Deliberate limits (see `docs/highlighting.md`): only the grammars bundled
//! in `assets/grammars` are known, so an `include` of another language
//! (fenced code blocks of other languages inside comments) matches nothing,
//! exactly as in VS Code when that language's extension is not installed;
//! injections are not implemented (the Lean grammars have none); lines
//! longer than [`MAX_LINE_UNITS`] UTF-16 units are not tokenized, like VS
//! Code's `editor.maxTokenizationLineLength`.

use onig::{MatchParam, Regex, RegexOptions, Region, SearchOptions, Syntax};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

pub const GRAMMAR_LEAN4: &str = include_str!("../../assets/grammars/lean4.json");
pub const GRAMMAR_LEAN4_MARKDOWN: &str = include_str!("../../assets/grammars/lean4-markdown.json");
/// Lean extension the grammars were taken from.
pub const GRAMMAR_SOURCE: &str = "vscode-lean4 0.0.240";
/// Bumped when the tokenizer's output can change for the same grammar.
pub const ENGINE_VERSION: u32 = 1;
/// VS Code's default `editor.maxTokenizationLineLength`.
pub const MAX_LINE_UNITS: usize = 20_000;

/// Identity of the baseline provider: engine version and grammar contents.
/// Cached baseline spans are only valid for the same identity.
pub fn baseline_identity() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        let mut all = Vec::new();
        all.extend_from_slice(GRAMMAR_LEAN4.as_bytes());
        all.extend_from_slice(GRAMMAR_LEAN4_MARKDOWN.as_bytes());
        format!("textmate-e{ENGINE_VERSION}/{GRAMMAR_SOURCE}/{}", &crate::paths::sha256_hex(&all)[..16])
    })
}

// ---------------------------------------------------------------------------
// Scope stacks, interned process-wide
// ---------------------------------------------------------------------------

/// An interned scope stack (`source.lean4 comment.block.lean4 …`).
pub type ScopeId = u32;

#[derive(Default)]
struct ScopeInner {
    names: Vec<Arc<str>>,
    name_ids: HashMap<Arc<str>, u32>,
    /// (parent stack or `u32::MAX`, scope name id)
    stacks: Vec<(u32, u32)>,
    stack_ids: HashMap<(u32, u32), u32>,
}

/// Process-wide table of scope stacks. Ids are only meaningful inside one
/// process; persisted data stores the scope names.
#[derive(Default)]
pub struct ScopeTable {
    inner: RwLock<ScopeInner>,
}

pub fn scopes() -> &'static ScopeTable {
    static T: OnceLock<ScopeTable> = OnceLock::new();
    T.get_or_init(ScopeTable::default)
}

impl ScopeTable {
    pub fn push(&self, parent: Option<ScopeId>, name: &str) -> ScopeId {
        let p = parent.unwrap_or(u32::MAX);
        {
            let r = self.inner.read().unwrap();
            if let Some(&n) = r.name_ids.get(name) {
                if let Some(&id) = r.stack_ids.get(&(p, n)) {
                    return id;
                }
            }
        }
        let mut w = self.inner.write().unwrap();
        let n = match w.name_ids.get(name) {
            Some(&n) => n,
            None => {
                let a: Arc<str> = Arc::from(name);
                let n = w.names.len() as u32;
                w.names.push(a.clone());
                w.name_ids.insert(a, n);
                n
            }
        };
        if let Some(&id) = w.stack_ids.get(&(p, n)) {
            return id;
        }
        let id = w.stacks.len() as u32;
        w.stacks.push((p, n));
        w.stack_ids.insert((p, n), id);
        id
    }

    /// Pushes a space-separated list of scope names.
    pub fn push_all(&self, parent: Option<ScopeId>, names: &str) -> Option<ScopeId> {
        let mut cur = parent;
        for n in names.split(' ').filter(|s| !s.is_empty()) {
            cur = Some(self.push(cur, n));
        }
        cur
    }

    /// Scope names from the root to the leaf.
    pub fn path(&self, id: ScopeId) -> Vec<Arc<str>> {
        let r = self.inner.read().unwrap();
        let mut out = Vec::new();
        let mut cur = id;
        while cur != u32::MAX {
            let Some(&(p, n)) = r.stacks.get(cur as usize) else { break };
            out.push(r.names[n as usize].clone());
            cur = p;
        }
        out.reverse();
        out
    }

    pub fn display(&self, id: ScopeId) -> String {
        self.path(id).iter().map(|s| &**s).collect::<Vec<_>>().join(" ")
    }
}

// ---------------------------------------------------------------------------
// Regular expressions
// ---------------------------------------------------------------------------

struct Re {
    id: usize,
    regex: Option<Regex>,
}

static RE_IDS: AtomicUsize = AtomicUsize::new(1);

fn compile_re(source: &str) -> Arc<Re> {
    // Shared by every tokenizer: one compilation per distinct source.
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Re>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(r) = cache.lock().unwrap().get(source) {
        return r.clone();
    }
    // The options VS Code's Oniguruma binding uses.
    let regex = Regex::with_options(source, RegexOptions::REGEX_OPTION_CAPTURE_GROUP, Syntax::default()).ok();
    let re = Arc::new(Re { id: RE_IDS.fetch_add(1, Ordering::Relaxed), regex });
    let mut c = cache.lock().unwrap();
    if c.len() > 4096 {
        c.clear(); // only end patterns with back-references are unbounded
    }
    c.insert(source.to_string(), re.clone());
    re
}

/// A pattern source with `\A`/`\G` anchor variants (vscode-textmate's
/// `RegExpSource`).
#[derive(Clone)]
struct ReSrc {
    source: Arc<str>,
    has_anchor: bool,
    has_backrefs: bool,
    /// a0g0, a0g1, a1g0, a1g1
    variants: Option<Arc<[Arc<str>; 4]>>,
}

fn has_backref(s: &str) -> bool {
    let b = s.as_bytes();
    (0..b.len().saturating_sub(1)).any(|i| b[i] == b'\\' && b[i + 1].is_ascii_digit())
}

impl ReSrc {
    fn new(src: &str) -> ReSrc {
        let chars: Vec<char> = src.chars().collect();
        let mut out = String::with_capacity(src.len());
        let mut has_anchor = false;
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '\\' && i + 1 < chars.len() {
                match chars[i + 1] {
                    'z' => out.push_str("$(?!\\n)(?<!\\n)"),
                    c => {
                        if c == 'A' || c == 'G' {
                            has_anchor = true;
                        }
                        out.push('\\');
                        out.push(c);
                    }
                }
                i += 2;
            } else {
                out.push(chars[i]);
                i += 1;
            }
        }
        let mut r = ReSrc { has_backrefs: has_backref(&out), source: Arc::from(out), has_anchor, variants: None };
        r.build_variants();
        r
    }

    fn build_variants(&mut self) {
        if !self.has_anchor {
            self.variants = None;
            return;
        }
        let chars: Vec<char> = self.source.chars().collect();
        let mut v: [String; 4] = Default::default();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            for s in v.iter_mut() {
                s.push(c);
            }
            if c == '\\' && i + 1 < chars.len() {
                let n = chars[i + 1];
                let never = '\u{FFFF}';
                let (a, g) = (n == 'A', n == 'G');
                v[0].push(if a || g { never } else { n });
                v[1].push(if a { never } else { n });
                v[2].push(if g { never } else { n });
                v[3].push(n);
                i += 1;
            }
            i += 1;
        }
        self.variants = Some(Arc::new(v.map(Arc::from)));
    }

    fn with_source(&self, source: &str) -> ReSrc {
        let mut r = ReSrc { source: Arc::from(source), has_anchor: self.has_anchor, has_backrefs: self.has_backrefs, variants: None };
        r.build_variants();
        r
    }

    fn resolved(&self, allow_a: bool, allow_g: bool) -> &Arc<str> {
        match &self.variants {
            Some(v) => &v[(allow_a as usize) * 2 + allow_g as usize],
            None => &self.source,
        }
    }

    /// `\1` … replaced by the (escaped) text of the begin captures.
    fn resolve_backrefs(&self, text: &str, caps: &[(usize, usize)]) -> String {
        let b = self.source.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'\\' && i + 1 < b.len() && b[i + 1].is_ascii_digit() {
                let mut j = i + 1;
                let mut n = 0usize;
                while j < b.len() && b[j].is_ascii_digit() {
                    n = n.saturating_mul(10).saturating_add((b[j] - b'0') as usize);
                    j += 1;
                }
                let cap = caps.get(n).map(|&(s, e)| &text[s..e]).unwrap_or("");
                for c in cap.chars() {
                    if "-\\{}*+?|^$.,[]()#".contains(c) || c.is_whitespace() {
                        out.push(b'\\');
                    }
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
                i = j;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------

type RuleId = u32;
const END_RULE: i64 = -1;
const WHILE_RULE: i64 = -2;

enum Kind {
    Compiling,
    Match { re: ReSrc, captures: Vec<Option<RuleId>> },
    IncludeOnly { patterns: Vec<RuleId>, has_missing: bool },
    BeginEnd {
        begin: ReSrc,
        begin_captures: Vec<Option<RuleId>>,
        end: ReSrc,
        end_captures: Vec<Option<RuleId>>,
        apply_end_last: bool,
        patterns: Vec<RuleId>,
        has_missing: bool,
    },
    BeginWhile {
        begin: ReSrc,
        begin_captures: Vec<Option<RuleId>>,
        while_: ReSrc,
        while_captures: Vec<Option<RuleId>>,
        patterns: Vec<RuleId>,
        has_missing: bool,
    },
    Capture { retokenize: Option<RuleId> },
}

struct Rule {
    name: Option<String>,
    name_capturing: bool,
    content_name: Option<String>,
    content_capturing: bool,
    kind: Kind,
}

/// `$1`, `${2:/downcase}` in scope names.
fn name_has_captures(s: &Option<String>) -> bool {
    let Some(s) = s else { return false };
    let b = s.as_bytes();
    (0..b.len().saturating_sub(1)).any(|i| b[i] == b'$' && (b[i + 1].is_ascii_digit() || b[i + 1] == b'{'))
}

fn replace_captures(name: &str, text: &str, caps: &[(usize, usize)]) -> String {
    let b = name.as_bytes();
    let mut out = String::with_capacity(name.len());
    let mut i = 0;
    let cap_text = |n: usize| -> Option<String> {
        let &(s, e) = caps.get(n)?;
        let mut t = &text[s..e];
        // Leading dots would make the scope selector invalid.
        while let Some(r) = t.strip_prefix('.') {
            t = r;
        }
        Some(t.to_string())
    };
    while i < b.len() {
        if b[i] == b'$' && i + 1 < b.len() && b[i + 1].is_ascii_digit() {
            let mut j = i + 1;
            let mut n = 0usize;
            while j < b.len() && b[j].is_ascii_digit() {
                n = n.saturating_mul(10).saturating_add((b[j] - b'0') as usize);
                j += 1;
            }
            match cap_text(n) {
                Some(t) => out.push_str(&t),
                None => out.push_str(&name[i..j]),
            }
            i = j;
            continue;
        }
        if b[i] == b'$' && i + 1 < b.len() && b[i + 1] == b'{' {
            // ${N:/downcase} or ${N:/upcase}
            if let Some(close) = name[i..].find('}') {
                let body = &name[i + 2..i + close];
                if let Some((num, cmd)) = body.split_once(":/") {
                    if let (Ok(n), true) = (num.parse::<usize>(), cmd == "downcase" || cmd == "upcase") {
                        match cap_text(n) {
                            Some(t) => out.push_str(&if cmd == "downcase" { t.to_lowercase() } else { t.to_uppercase() }),
                            None => out.push_str(&name[i..i + close + 1]),
                        }
                        i += close + 1;
                        continue;
                    }
                }
            }
        }
        let ch = name[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

impl Rule {
    fn get_name(&self, text: &str, caps: &[(usize, usize)]) -> Option<std::borrow::Cow<'_, str>> {
        let n = self.name.as_deref()?;
        Some(if self.name_capturing { replace_captures(n, text, caps).into() } else { n.into() })
    }
    fn get_content_name(&self, text: &str, caps: &[(usize, usize)]) -> Option<std::borrow::Cow<'_, str>> {
        let n = self.content_name.as_deref()?;
        Some(if self.content_capturing { replace_captures(n, text, caps).into() } else { n.into() })
    }
}

// ---------------------------------------------------------------------------
// Grammar compilation
// ---------------------------------------------------------------------------

type Repo = HashMap<String, &'static Value>;

struct RawGrammar {
    json: &'static Value,
    self_rule: &'static Value,
}

pub struct Grammar {
    rules: Vec<Rule>,
    root: RuleId,
    root_scope: String,
}

struct Compiler {
    rules: Vec<Rule>,
    memo: HashMap<usize, RuleId>,
    raw: HashMap<&'static str, RawGrammar>,
    included: HashMap<String, Arc<Repo>>,
}

fn leak_grammar(text: &str) -> Option<(&'static str, RawGrammar)> {
    let json: &'static Value = Box::leak(Box::new(serde_json::from_str::<Value>(text).ok()?));
    let scope: &'static str = json.get("scopeName")?.as_str()?;
    let self_rule: &'static Value =
        Box::leak(Box::new(serde_json::json!({ "patterns": json.get("patterns").cloned().unwrap_or(Value::Null), "name": scope })));
    Some((scope, RawGrammar { json, self_rule }))
}

impl Compiler {
    fn repo_of(&mut self, scope: &str, base: Option<&'static Value>) -> Option<Arc<Repo>> {
        if let Some(r) = self.included.get(scope) {
            return Some(r.clone());
        }
        let g = self.raw.get(scope)?;
        let mut repo = Repo::new();
        if let Some(o) = g.json.get("repository").and_then(|r| r.as_object()) {
            for (k, v) in o {
                repo.insert(k.clone(), v);
            }
        }
        repo.insert("$self".into(), g.self_rule);
        repo.insert("$base".into(), base.unwrap_or(g.self_rule));
        let repo = Arc::new(repo);
        self.included.insert(scope.to_string(), repo.clone());
        Some(repo)
    }

    fn rule_id(&mut self, desc: &'static Value, repo: &Arc<Repo>) -> RuleId {
        let key = desc as *const Value as usize;
        if let Some(&id) = self.memo.get(&key) {
            return id;
        }
        let id = self.rules.len() as RuleId;
        let name = desc.get("name").and_then(|v| v.as_str()).map(String::from);
        let content_name = desc.get("contentName").and_then(|v| v.as_str()).map(String::from);
        self.rules.push(Rule {
            name_capturing: name_has_captures(&name),
            content_capturing: name_has_captures(&content_name),
            name,
            content_name,
            kind: Kind::Compiling,
        });
        self.memo.insert(key, id);
        let s = |k: &str| desc.get(k).and_then(|v| v.as_str());
        let kind = if let Some(m) = s("match") {
            Kind::Match { re: ReSrc::new(m), captures: self.captures(desc.get("captures"), repo) }
        } else if s("begin").is_none() {
            let repo2 = match desc.get("repository").and_then(|r| r.as_object()) {
                Some(local) => {
                    let mut m = (**repo).clone();
                    for (k, v) in local {
                        m.insert(k.clone(), v);
                    }
                    Arc::new(m)
                }
                None => repo.clone(),
            };
            let (patterns, has_missing) = match (desc.get("patterns").and_then(|p| p.as_array()), s("include")) {
                (Some(p), _) => self.patterns(Some(p), &repo2),
                (None, Some(inc)) => {
                    let id = self.include(inc, &repo2);
                    let mut v = Vec::new();
                    if let Some(id) = id.filter(|&id| !self.skippable(id)) {
                        v.push(id);
                    }
                    let missing = v.is_empty();
                    (v, missing)
                }
                (None, None) => (Vec::new(), false),
            };
            Kind::IncludeOnly { patterns, has_missing }
        } else if let Some(w) = s("while") {
            let begin_captures = self.captures(desc.get("beginCaptures").or(desc.get("captures")), repo);
            let while_captures = self.captures(desc.get("whileCaptures").or(desc.get("captures")), repo);
            let (patterns, has_missing) = self.patterns(desc.get("patterns").and_then(|p| p.as_array()), repo);
            Kind::BeginWhile { begin: ReSrc::new(s("begin").unwrap()), begin_captures, while_: ReSrc::new(w), while_captures, patterns, has_missing }
        } else {
            let begin_captures = self.captures(desc.get("beginCaptures").or(desc.get("captures")), repo);
            let end_captures = self.captures(desc.get("endCaptures").or(desc.get("captures")), repo);
            let (patterns, has_missing) = self.patterns(desc.get("patterns").and_then(|p| p.as_array()), repo);
            Kind::BeginEnd {
                begin: ReSrc::new(s("begin").unwrap()),
                begin_captures,
                end: ReSrc::new(s("end").filter(|e| !e.is_empty()).unwrap_or("\u{FFFF}")),
                end_captures,
                apply_end_last: desc.get("applyEndPatternLast").is_some_and(|v| v.as_bool() == Some(true) || v.as_i64() == Some(1)),
                patterns,
                has_missing,
            }
        };
        self.rules[id as usize].kind = kind;
        id
    }

    fn captures(&mut self, caps: Option<&'static Value>, repo: &Arc<Repo>) -> Vec<Option<RuleId>> {
        let Some(obj) = caps.and_then(|c| c.as_object()) else { return Vec::new() };
        let max = obj.keys().filter_map(|k| k.parse::<usize>().ok()).max().unwrap_or(0);
        let mut out = vec![None; max + 1];
        for (k, v) in obj {
            let Ok(i) = k.parse::<usize>() else { continue };
            let retokenize = v.get("patterns").is_some().then(|| self.rule_id(v, repo));
            let name = v.get("name").and_then(|v| v.as_str()).map(String::from);
            let content_name = v.get("contentName").and_then(|v| v.as_str()).map(String::from);
            let id = self.rules.len() as RuleId;
            self.rules.push(Rule {
                name_capturing: name_has_captures(&name),
                content_capturing: name_has_captures(&content_name),
                name,
                content_name,
                kind: Kind::Capture { retokenize },
            });
            out[i] = Some(id);
        }
        out
    }

    fn include(&mut self, inc: &str, repo: &Arc<Repo>) -> Option<RuleId> {
        if inc == "$base" || inc == "$self" {
            let d = *repo.get(inc)?;
            return Some(self.rule_id(d, repo));
        }
        match inc.find('#') {
            Some(0) => {
                let d = *repo.get(&inc[1..])?;
                Some(self.rule_id(d, repo))
            }
            None => {
                let ext = self.repo_of(inc, repo.get("$base").copied())?;
                let d = *ext.get("$self")?;
                Some(self.rule_id(d, &ext))
            }
            Some(i) => {
                let ext = self.repo_of(&inc[..i], repo.get("$base").copied())?;
                let d = *ext.get(&inc[i + 1..])?;
                Some(self.rule_id(d, &ext))
            }
        }
    }

    /// A container whose own patterns all failed to resolve is dropped.
    fn skippable(&self, id: RuleId) -> bool {
        match &self.rules[id as usize].kind {
            Kind::IncludeOnly { patterns, has_missing }
            | Kind::BeginEnd { patterns, has_missing, .. }
            | Kind::BeginWhile { patterns, has_missing, .. } => *has_missing && patterns.is_empty(),
            _ => false,
        }
    }

    fn patterns(&mut self, pats: Option<&'static Vec<Value>>, repo: &Arc<Repo>) -> (Vec<RuleId>, bool) {
        let Some(pats) = pats else { return (Vec::new(), false) };
        let mut out = Vec::new();
        for p in pats {
            let id = match p.get("include").and_then(|v| v.as_str()) {
                Some(inc) => self.include(inc, repo),
                None => Some(self.rule_id(p, repo)),
            };
            if let Some(id) = id {
                if !self.skippable(id) {
                    out.push(id);
                }
            }
        }
        let missing = out.len() != pats.len();
        (out, missing)
    }
}

impl Grammar {
    /// The Lean 4 grammar of the VS Code extension, with its Markdown
    /// grammar for comments.
    pub fn lean4() -> &'static Grammar {
        static G: OnceLock<Grammar> = OnceLock::new();
        G.get_or_init(|| Grammar::compile(&[GRAMMAR_LEAN4, GRAMMAR_LEAN4_MARKDOWN], "source.lean4").expect("bundled grammar compiles"))
    }

    pub fn compile(sources: &[&str], root_scope: &str) -> Option<Grammar> {
        let mut c = Compiler { rules: Vec::new(), memo: HashMap::new(), raw: HashMap::new(), included: HashMap::new() };
        for s in sources {
            let (scope, raw) = leak_grammar(s)?;
            c.raw.insert(scope, raw);
        }
        let repo = c.repo_of(root_scope, None)?;
        let root = c.rule_id(*repo.get("$self")?, &repo);
        Some(Grammar { rules: c.rules, root, root_scope: root_scope.to_string() })
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    fn collect(&self, id: RuleId, out: &mut Vec<(ReSrc, i64)>, depth: usize) {
        if depth > 64 {
            return;
        }
        match &self.rules[id as usize].kind {
            Kind::Match { re, .. } => out.push((re.clone(), id as i64)),
            Kind::IncludeOnly { patterns, .. } => {
                for &p in patterns {
                    self.collect(p, out, depth + 1);
                }
            }
            Kind::BeginEnd { begin, .. } | Kind::BeginWhile { begin, .. } => out.push((begin.clone(), id as i64)),
            Kind::Capture { .. } | Kind::Compiling => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

/// One element of the rule stack carried from line to line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    rule: RuleId,
    enter_pos: i64,
    anchor_pos: i64,
    begin_captured_eol: bool,
    end_rule: Option<Arc<str>>,
    name_scopes: ScopeId,
    content_scopes: ScopeId,
}

/// The tokenizer state at the start of a line.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct LineState {
    stack: Vec<Frame>,
}

impl LineState {
    pub fn depth(&self) -> usize {
        self.stack.len()
    }
}

type Scanner = Arc<Vec<(Arc<Re>, i64, bool)>>;

struct Cached {
    epoch: u64,
    from: usize,
    caps: Option<Arc<[(usize, usize)]>>,
}

struct LineTokens<'a> {
    out: &'a mut Vec<(u32, ScopeId)>,
    last_end: i64,
}

impl LineTokens<'_> {
    fn produce(&mut self, scopes: ScopeId, end: usize) {
        if self.last_end >= end as i64 {
            return;
        }
        self.out.push((self.last_end.max(0) as u32, scopes));
        self.last_end = end as i64;
    }
}

pub struct Tokenizer {
    g: &'static Grammar,
    scanners: HashMap<(RuleId, bool, Option<Arc<str>>, bool, bool), Scanner>,
    cache: HashMap<usize, Cached>,
    epoch: u64,
    next_epoch: u64,
    region: Region,
    buf: String,
}

impl Tokenizer {
    pub fn new(g: &'static Grammar) -> Tokenizer {
        Tokenizer { g, scanners: HashMap::new(), cache: HashMap::new(), epoch: 0, next_epoch: 0, region: Region::new(), buf: String::new() }
    }

    pub fn initial_state(&self) -> LineState {
        let root = scopes().push(None, &self.g.root_scope);
        LineState {
            stack: vec![Frame { rule: self.g.root, enter_pos: -1, anchor_pos: -1, begin_captured_eol: false, end_rule: None, name_scopes: root, content_scopes: root }],
        }
    }

    fn scanner(&mut self, rule: RuleId, while_: bool, end_rule: &Option<Arc<str>>, allow_a: bool, allow_g: bool) -> Scanner {
        let g = self.g;
        // Only rules whose end/while pattern has back-references depend on
        // the resolved end source.
        let dynamic = match &g.rules[rule as usize].kind {
            Kind::BeginEnd { end, .. } => !while_ && end.has_backrefs,
            Kind::BeginWhile { while_: w, .. } => while_ && w.has_backrefs,
            _ => false,
        };
        let key = (rule, while_, if dynamic { end_rule.clone() } else { None }, allow_a, allow_g);
        if let Some(s) = self.scanners.get(&key) {
            return s.clone();
        }
        let mut pats: Vec<(ReSrc, i64)> = Vec::new();
        match &g.rules[rule as usize].kind {
            Kind::BeginWhile { while_: w, .. } if while_ => {
                let src = if w.has_backrefs { w.with_source(end_rule.as_deref().unwrap_or("\u{FFFF}")) } else { w.clone() };
                pats.push((src, WHILE_RULE));
            }
            Kind::BeginEnd { end, apply_end_last, patterns, .. } => {
                for &p in patterns {
                    g.collect(p, &mut pats, 0);
                }
                let src = if end.has_backrefs { end.with_source(end_rule.as_deref().unwrap_or(&end.source)) } else { end.clone() };
                if *apply_end_last {
                    pats.push((src, END_RULE));
                } else {
                    pats.insert(0, (src, END_RULE));
                }
            }
            Kind::BeginWhile { patterns, .. } | Kind::IncludeOnly { patterns, .. } => {
                for &p in patterns {
                    g.collect(p, &mut pats, 0);
                }
            }
            Kind::Match { re, .. } => pats.push((re.clone(), rule as i64)),
            Kind::Capture { .. } | Kind::Compiling => {}
        }
        let s: Scanner = Arc::new(
            pats.iter()
                .map(|(src, tag)| {
                    let resolved = src.resolved(allow_a, allow_g);
                    // A pattern that still contains `\G` depends on where the
                    // search starts, so its result cannot be reused.
                    let cacheable = !(src.has_anchor && allow_g && src.source.contains("\\G"));
                    (compile_re(resolved), *tag, cacheable)
                })
                .collect(),
        );
        if self.scanners.len() > 20_000 {
            self.scanners.clear();
        }
        self.scanners.insert(key, s.clone());
        s
    }

    /// The leftmost match at or after `pos`; among equal starts, the first
    /// pattern wins (Oniguruma scanner semantics).
    fn find_next(&mut self, scanner: &Scanner, text: &str, pos: usize) -> Option<(i64, Arc<[(usize, usize)]>)> {
        let mut best: Option<(i64, Arc<[(usize, usize)]>)> = None;
        for (re, tag, cacheable) in scanner.iter() {
            let Some(regex) = &re.regex else { continue };
            let mut found: Option<Arc<[(usize, usize)]>> = None;
            let mut hit = false;
            if *cacheable {
                if let Some(c) = self.cache.get(&re.id) {
                    if c.epoch == self.epoch && c.from <= pos {
                        match &c.caps {
                            None => hit = true,
                            Some(caps) if caps[0].0 >= pos => {
                                found = Some(caps.clone());
                                hit = true;
                            }
                            _ => {}
                        }
                    }
                }
            }
            if !hit {
                self.region.clear();
                let r = regex.search_with_param(text, pos, text.len(), SearchOptions::SEARCH_OPTION_NONE, Some(&mut self.region), MatchParam::default());
                if let Ok(Some(_)) = r {
                    let caps: Vec<(usize, usize)> = (0..self.region.len()).map(|i| self.region.pos(i).unwrap_or((0, 0))).collect();
                    found = Some(Arc::from(caps));
                }
                if *cacheable {
                    self.cache.insert(re.id, Cached { epoch: self.epoch, from: pos, caps: found.clone() });
                }
            }
            if let Some(caps) = found {
                let start = caps[0].0;
                if best.as_ref().is_none_or(|(_, b)| start < b[0].0) {
                    best = Some((*tag, caps));
                }
                if start == pos {
                    break;
                }
            }
        }
        best
    }

    fn push_scopes(base: ScopeId, name: Option<std::borrow::Cow<'_, str>>) -> ScopeId {
        match name {
            Some(n) => scopes().push_all(Some(base), &n).unwrap_or(base),
            None => base,
        }
    }

    fn handle_captures(&mut self, text: &str, is_first: bool, stack: &[Frame], toks: &mut LineTokens, captures: &[Option<RuleId>], caps: &[(usize, usize)]) {
        if captures.is_empty() {
            return;
        }
        let g = self.g;
        let top = stack.last().unwrap();
        let n = captures.len().min(caps.len());
        let mut local: Vec<(ScopeId, usize)> = Vec::new();
        let max_end = caps[0].1;
        for i in 0..n {
            let Some(cr) = captures[i] else { continue };
            let (s, e) = caps[i];
            if e == s {
                continue;
            }
            if s > max_end {
                break;
            }
            while let Some(&(sc, endp)) = local.last() {
                if endp > s {
                    break;
                }
                toks.produce(sc, endp);
                local.pop();
            }
            match local.last() {
                Some(&(sc, _)) => toks.produce(sc, s),
                None => toks.produce(top.content_scopes, s),
            }
            let rule = &g.rules[cr as usize];
            if let Kind::Capture { retokenize: Some(rt) } = rule.kind {
                let ns = Self::push_scopes(top.content_scopes, rule.get_name(text, caps));
                let cs = Self::push_scopes(ns, rule.get_content_name(text, caps));
                let mut st = stack.to_vec();
                st.push(Frame { rule: rt, enter_pos: s as i64, anchor_pos: -1, begin_captured_eol: false, end_rule: None, name_scopes: ns, content_scopes: cs });
                let outer = self.epoch;
                self.next_epoch += 1;
                self.epoch = self.next_epoch;
                self.tokenize_string(&text[..e], is_first && s == 0, s, &mut st, toks, false);
                self.epoch = outer;
                continue;
            }
            if let Some(name) = rule.get_name(text, caps) {
                let base = local.last().map(|x| x.0).unwrap_or(top.content_scopes);
                local.push((Self::push_scopes(base, Some(name)), e));
            }
        }
        while let Some((sc, endp)) = local.pop() {
            toks.produce(sc, endp);
        }
    }

    fn check_while(&mut self, text: &str, mut is_first: bool, mut pos: usize, stack: &mut Vec<Frame>, toks: &mut LineTokens) -> (usize, i64, bool) {
        let g = self.g;
        let mut anchor: i64 = if stack.last().is_some_and(|f| f.begin_captured_eol) { 0 } else { -1 };
        let whiles: Vec<usize> = (0..stack.len()).filter(|&i| matches!(g.rules[stack[i].rule as usize].kind, Kind::BeginWhile { .. })).collect();
        for i in whiles {
            let (rule, end_rule, content) = (stack[i].rule, stack[i].end_rule.clone(), stack[i].content_scopes);
            let sc = self.scanner(rule, true, &end_rule, is_first, pos as i64 == anchor);
            match self.find_next(&sc, text, pos) {
                Some((WHILE_RULE, caps)) => {
                    if !caps.is_empty() {
                        toks.produce(content, caps[0].0);
                        if let Kind::BeginWhile { while_captures, .. } = &g.rules[rule as usize].kind {
                            let prefix = stack[..=i].to_vec();
                            self.handle_captures(text, is_first, &prefix, toks, while_captures, &caps);
                        }
                        toks.produce(content, caps[0].1);
                        anchor = caps[0].1 as i64;
                        if caps[0].1 > pos {
                            pos = caps[0].1;
                            is_first = false;
                        }
                    }
                }
                _ => {
                    stack.truncate(i);
                    break;
                }
            }
        }
        (pos, anchor, is_first)
    }

    fn tokenize_string(&mut self, text: &str, mut is_first: bool, mut pos: usize, stack: &mut Vec<Frame>, toks: &mut LineTokens, check_while: bool) {
        let g = self.g;
        let len = text.len();
        let mut anchor: i64 = -1;
        if check_while {
            (pos, anchor, is_first) = self.check_while(text, is_first, pos, stack, toks);
        }
        loop {
            let Some(top) = stack.last() else { return };
            let (top_rule, top_end, top_content) = (top.rule, top.end_rule.clone(), top.content_scopes);
            let sc = self.scanner(top_rule, false, &top_end, is_first, pos as i64 == anchor);
            let Some((tag, caps)) = self.find_next(&sc, text, pos) else {
                toks.produce(top_content, len);
                return;
            };
            let has_advanced = caps[0].1 > pos;
            if tag == END_RULE {
                toks.produce(top_content, caps[0].0);
                let t = stack.last_mut().unwrap();
                t.content_scopes = t.name_scopes;
                if let Kind::BeginEnd { end_captures, .. } = &g.rules[top_rule as usize].kind {
                    let snapshot = stack.clone();
                    self.handle_captures(text, is_first, &snapshot, toks, end_captures, &caps);
                }
                toks.produce(stack.last().unwrap().content_scopes, caps[0].1);
                let popped = stack.pop().unwrap();
                anchor = popped.anchor_pos;
                if !has_advanced && popped.enter_pos == pos as i64 {
                    // Pushed and popped without advancing: stop here.
                    let sc = popped.content_scopes;
                    stack.push(popped);
                    toks.produce(sc, len);
                    return;
                }
            } else {
                let rid = tag as RuleId;
                let rule = &g.rules[rid as usize];
                toks.produce(top_content, caps[0].0);
                let old_len = stack.len();
                let name_scopes = Self::push_scopes(top_content, rule.get_name(text, &caps));
                stack.push(Frame {
                    rule: rid,
                    enter_pos: pos as i64,
                    anchor_pos: anchor,
                    begin_captured_eol: caps[0].1 == len,
                    end_rule: None,
                    name_scopes,
                    content_scopes: name_scopes,
                });
                let same_rule_below = |stack: &Vec<Frame>| {
                    let new = stack.last().unwrap();
                    stack[..old_len].iter().rev().take_while(|f| f.enter_pos == new.enter_pos).any(|f| f.rule == new.rule)
                };
                match &rule.kind {
                    Kind::BeginEnd { begin_captures, end, .. } => {
                        let snapshot = stack.clone();
                        self.handle_captures(text, is_first, &snapshot, toks, begin_captures, &caps);
                        toks.produce(name_scopes, caps[0].1);
                        anchor = caps[0].1 as i64;
                        let content = Self::push_scopes(name_scopes, rule.get_content_name(text, &caps));
                        let t = stack.last_mut().unwrap();
                        t.content_scopes = content;
                        if end.has_backrefs {
                            t.end_rule = Some(Arc::from(end.resolve_backrefs(text, &caps)));
                        }
                        if !has_advanced && same_rule_below(stack) {
                            stack.pop();
                            toks.produce(stack.last().unwrap().content_scopes, len);
                            return;
                        }
                    }
                    Kind::BeginWhile { begin_captures, while_, .. } => {
                        let snapshot = stack.clone();
                        self.handle_captures(text, is_first, &snapshot, toks, begin_captures, &caps);
                        toks.produce(name_scopes, caps[0].1);
                        anchor = caps[0].1 as i64;
                        let content = Self::push_scopes(name_scopes, rule.get_content_name(text, &caps));
                        let t = stack.last_mut().unwrap();
                        t.content_scopes = content;
                        if while_.has_backrefs {
                            t.end_rule = Some(Arc::from(while_.resolve_backrefs(text, &caps)));
                        }
                        if !has_advanced && same_rule_below(stack) {
                            stack.pop();
                            toks.produce(stack.last().unwrap().content_scopes, len);
                            return;
                        }
                    }
                    Kind::Match { captures, .. } => {
                        let snapshot = stack.clone();
                        self.handle_captures(text, is_first, &snapshot, toks, captures, &caps);
                        toks.produce(name_scopes, caps[0].1);
                        stack.pop();
                        if !has_advanced {
                            // Not advancing, nor pushing or popping: stop.
                            if stack.len() > 1 {
                                stack.pop();
                            }
                            toks.produce(stack.last().unwrap().content_scopes, len);
                            return;
                        }
                    }
                    _ => {
                        stack.pop();
                        toks.produce(top_content, len);
                        return;
                    }
                }
            }
            if caps[0].1 > pos {
                pos = caps[0].1;
                is_first = false;
            }
        }
    }

    /// Tokenizes one line (without its terminator). `out` receives
    /// `(start byte, scope stack)` in order; a token ends where the next one
    /// starts. Adjacent tokens with equal scopes are merged.
    pub fn tokenize_line(&mut self, line: &str, state: &mut LineState, first_line: bool, out: &mut Vec<(u32, ScopeId)>) {
        out.clear();
        if state.stack.is_empty() {
            *state = self.initial_state();
        }
        // UTF-16 length, as VS Code counts it.
        let too_long = line.len() > MAX_LINE_UNITS && line.encode_utf16().count() > MAX_LINE_UNITS;
        if too_long {
            out.push((0, state.stack.last().unwrap().content_scopes));
            return;
        }
        for f in state.stack.iter_mut() {
            f.enter_pos = -1;
            f.anchor_pos = -1;
        }
        let mut buf = std::mem::take(&mut self.buf);
        buf.clear();
        buf.push_str(line);
        buf.push('\n');
        self.next_epoch += 1;
        self.epoch = self.next_epoch;
        let mut raw: Vec<(u32, ScopeId)> = Vec::new();
        {
            let mut toks = LineTokens { out: &mut raw, last_end: 0 };
            self.tokenize_string(&buf, first_line, 0, &mut state.stack, &mut toks, true);
        }
        self.buf = buf;
        if state.stack.is_empty() {
            *state = self.initial_state();
        }
        // Drop the token produced for the appended newline.
        if raw.last().is_some_and(|t| t.0 as usize == line.len()) && !(raw.len() == 1 && line.is_empty()) {
            raw.pop();
        }
        if raw.is_empty() {
            raw.push((0, state.stack.last().unwrap().content_scopes));
        }
        for t in raw {
            if out.last().is_none_or(|l: &(u32, ScopeId)| l.1 != t.1) {
                out.push(t);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokenize(text: &str) -> Vec<Vec<(u32, String)>> {
        let mut t = Tokenizer::new(Grammar::lean4());
        let mut st = t.initial_state();
        let mut out = Vec::new();
        let mut toks = Vec::new();
        for (i, l) in text.split('\n').enumerate() {
            t.tokenize_line(l, &mut st, i == 0, &mut toks);
            out.push(toks.iter().map(|&(s, id)| (s, scopes().display(id))).collect());
        }
        out
    }

    #[test]
    fn keywords_names_and_comments() {
        let t = tokenize("theorem foo : True := by\n  trivial -- done\n/- a /- nested -/ b -/ def");
        assert_eq!(t[0][0], (0, "source.lean4 meta.definitioncommand.lean4 keyword.other.definitioncommand.lean4".into()));
        assert_eq!(t[0][2], (8, "source.lean4 meta.definitioncommand.lean4 entity.name.function.lean4".into()));
        assert!(t[0].iter().any(|x| x.1 == "source.lean4 keyword.other.lean4"), "by is a keyword");
        assert!(t[1].last().unwrap().1.contains("comment.line.double-dash.lean4"));
        // The nested block comment ends at the outer `-/`, then `def` is code.
        let last = t[2].last().unwrap();
        assert!(last.1.contains("keyword"), "{:?}", t[2]);
        assert!(t[2][0].1.contains("comment.block.lean4"));
    }

    #[test]
    fn multiline_state_is_carried() {
        let t = tokenize("/-- doc\nstill *doc*\n-/\ndef x := \"a\\n\"");
        assert!(t[1][0].1.contains("comment.block.documentation.lean4"));
        assert!(t[1].iter().any(|x| x.1.contains("markup.italic.markdown")), "{:?}", t[1]);
        assert!(t[3].iter().any(|x| x.1.ends_with("constant.character.escape.lean4")));
    }

    #[test]
    fn offsets_are_utf8_bytes() {
        let t = tokenize("def α𝔽 : Prop := sorry");
        // "def " is 4 bytes, α is 2, 𝔽 is 4.
        assert_eq!(t[0][2].0, 4);
        assert_eq!(t[0][3].0, 10);
        assert!(t[0].iter().any(|x| x.1.ends_with("invalid.illegal.lean4")));
    }

    #[test]
    fn very_long_lines_are_left_plain() {
        let long = format!("def x := {}", "1 + ".repeat(6000));
        let t = tokenize(&long);
        assert_eq!(t[0].len(), 1);
        assert_eq!(t[0][0].1, "source.lean4");
    }

    #[test]
    fn empty_input() {
        let t = tokenize("");
        assert_eq!(t, vec![vec![(0, "source.lean4".to_string())]]);
    }
}
