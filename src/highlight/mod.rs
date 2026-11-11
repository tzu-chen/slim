//! Syntax and semantic highlighting.
//!
//! Layers, each independent of the next (`docs/highlighting.md`):
//!
//! ```text
//! baseline  (textmate, baseline)  source bytes -> TextMate scope stacks
//! semantic  (lsp, semantic, indexer)  Lean's own semantic tokens, extracted
//!                                 once by an explicit job and cached
//! store     (codec, store)        compact per-file spans in SQLite
//! freshness (env, freshness)      separate facts: source alignment,
//!                                 environment, coverage, build relation
//! styles    (theme)               scopes / token types -> logical styles
//! service                         background worker used by the browser
//! ```
//!
//! Browsing only ever reads the cache and tokenizes text; the semantic
//! indexer is the one part that starts Lean, and only when asked.

pub mod baseline;
pub mod cli;
pub mod codec;
pub mod env;
pub mod indexer;
pub mod lsp;
pub mod semantic;
pub mod service;
pub mod store;
pub mod textmate;
pub mod theme;

use std::sync::Arc;
use std::sync::OnceLock;
use textmate::ScopeId;

/// Lines per storage chunk: the unit of decoding and of tokenization progress.
pub const CHUNK_LINES: usize = 256;
/// Bumped when extraction or validation changes what a semantic record means.
pub const EXTRACTOR_VERSION: &str = "slim-semhl-1";

/// A baseline token: start byte within the line and its scope stack. It
/// ends where the next token of the line starts.
pub type BaseTok = (u32, ScopeId);

/// A semantic token within one line (byte offsets, end exclusive).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SemTok {
    pub start: u32,
    pub end: u32,
    /// Index into the legend's token types.
    pub ty: u16,
    /// Bit set over the legend's modifiers.
    pub mods: u32,
}

/// The server's token legend, stored with the tokens it decodes.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Legend {
    pub types: Vec<String>,
    pub modifiers: Vec<String>,
}

/// Tokens of consecutive lines in one allocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lines<T> {
    ends: Vec<u32>,
    toks: Vec<T>,
}

impl<T> Default for Lines<T> {
    fn default() -> Self {
        Lines { ends: Vec::new(), toks: Vec::new() }
    }
}

impl<T> Lines<T> {
    pub fn push_line(&mut self, toks: impl IntoIterator<Item = T>) {
        self.toks.extend(toks);
        self.ends.push(self.toks.len() as u32);
    }
    pub fn len(&self) -> usize {
        self.ends.len()
    }
    pub fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }
    pub fn line(&self, i: usize) -> &[T] {
        let Some(&e) = self.ends.get(i) else { return &[] };
        let s = if i == 0 { 0 } else { self.ends[i - 1] };
        &self.toks[s as usize..e as usize]
    }
    pub fn token_count(&self) -> usize {
        self.toks.len()
    }
    pub fn heap_bytes(&self) -> usize {
        self.ends.capacity() * 4 + self.toks.capacity() * std::mem::size_of::<T>()
    }
}

/// One layer of a file's highlight data, in chunks of [`CHUNK_LINES`]
/// lines. Chunks are either already decoded (fresh tokenization) or
/// decoded on first use from the stored blob, so a view only pays for the
/// lines it shows.
pub struct Layer<T: codec::Decode> {
    chunks: Vec<OnceLock<Option<Arc<Lines<T>>>>>,
    blob: Option<Arc<codec::Blob<T>>>,
    n_lines: usize,
}

impl<T: codec::Decode> Clone for Layer<T> {
    fn clone(&self) -> Self {
        Layer { chunks: self.chunks.clone(), blob: self.blob.clone(), n_lines: self.n_lines }
    }
}

impl<T: codec::Decode> Layer<T> {
    /// A layer backed by a stored blob.
    pub fn from_blob(blob: codec::Blob<T>) -> Layer<T> {
        let n_lines = blob.n_lines();
        let n = blob.chunk_count();
        Layer { chunks: (0..n).map(|_| OnceLock::new()).collect(), blob: Some(Arc::new(blob)), n_lines }
    }
    /// An empty layer for `n_lines` lines, to be filled chunk by chunk.
    pub fn pending(n_lines: usize) -> Layer<T> {
        Layer { chunks: (0..n_lines.div_ceil(CHUNK_LINES)).map(|_| OnceLock::new()).collect(), blob: None, n_lines }
    }
    pub fn from_chunks(chunks: Vec<Arc<Lines<T>>>, n_lines: usize) -> Layer<T> {
        Layer { chunks: chunks.into_iter().map(|c| OnceLock::from(Some(c))).collect(), blob: None, n_lines }
    }
    pub fn set_chunk(&mut self, i: usize, c: Arc<Lines<T>>) {
        if let Some(slot) = self.chunks.get_mut(i) {
            *slot = OnceLock::from(Some(c));
        }
    }
    pub fn n_lines(&self) -> usize {
        self.n_lines
    }
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }
    /// The chunk, decoding it if needed. `None`: not tokenized yet, or the
    /// stored data is damaged (the caller shows plain text).
    pub fn chunk(&self, i: usize) -> Option<&Arc<Lines<T>>> {
        let slot = self.chunks.get(i)?;
        match (&self.blob, slot.get()) {
            (_, Some(c)) => c.as_ref(),
            (Some(b), None) => slot.get_or_init(|| b.chunk(i).ok().map(Arc::new)).as_ref(),
            (None, None) => None,
        }
    }
    /// Tokens of 0-based line `n`.
    pub fn line(&self, n: usize) -> Option<&[T]> {
        if n >= self.n_lines {
            return None;
        }
        Some(self.chunk(n / CHUNK_LINES)?.line(n % CHUNK_LINES))
    }
    /// Collected chunks, if every one is present (for encoding).
    pub fn all_chunks(&self) -> Option<Vec<Arc<Lines<T>>>> {
        (0..self.chunks.len()).map(|i| self.chunk(i).cloned()).collect()
    }
    /// Approximate memory held: decoded chunks plus the blob.
    pub fn heap_bytes(&self) -> usize {
        let decoded: usize = self.chunks.iter().filter_map(|c| c.get().and_then(|c| c.as_ref())).map(|c| c.heap_bytes() + 48).sum();
        decoded + self.blob.as_ref().map(|b| b.byte_len()).unwrap_or(0) + self.chunks.len() * 16
    }
}

/// How much of a file a semantic record covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coverage {
    /// The server reported the whole document processed before the tokens
    /// were taken.
    Complete,
    /// Tokens exist for a processed prefix only (time limit, dropped
    /// tokens, or no trustworthy completion signal).
    Partial,
    /// No usable tokens (imports unavailable, server failure, …).
    Failed,
    /// Not attempted by decision (too large, not UTF-8, …).
    Skipped,
}

impl Coverage {
    pub fn as_str(self) -> &'static str {
        match self {
            Coverage::Complete => "complete",
            Coverage::Partial => "partial",
            Coverage::Failed => "failed",
            Coverage::Skipped => "skipped",
        }
    }
    pub fn parse(s: &str) -> Option<Coverage> {
        Some(match s {
            "complete" => Coverage::Complete,
            "partial" => Coverage::Partial,
            "failed" => Coverage::Failed,
            "skipped" => Coverage::Skipped,
            _ => return None,
        })
    }
}

/// What the reader asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Off,
    /// Baseline syntax colouring only.
    Syntax,
    /// Baseline plus cached semantic tokens when they are usable.
    Semantic,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        Some(match s {
            "off" | "none" | "plain" => Mode::Off,
            "syntax" | "baseline" => Mode::Syntax,
            "semantic" | "full" | "on" => Mode::Semantic,
            _ => return None,
        })
    }
    pub fn label(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Syntax => "syntax",
            Mode::Semantic => "syntax + semantic",
        }
    }
    pub fn next(self) -> Mode {
        match self {
            Mode::Semantic => Mode::Syntax,
            Mode::Syntax => Mode::Off,
            Mode::Off => Mode::Semantic,
        }
    }
}
pub mod render;
pub mod bench;
