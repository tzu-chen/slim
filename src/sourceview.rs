//! Reading source text for display: containment within the approved root,
//! bounded reads, a small cache, and staleness detection by content hash.

use crate::paths::{resolve_contained, sha256_hex};
use crate::text::{decode_lossy, line_starts, PosEncoding};
use anyhow::{bail, Result};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const MAX_FILE_BYTES: u64 = 64 << 20;

#[derive(Clone, Debug)]
pub struct SourceFileText {
    pub path: PathBuf,
    pub text: Arc<str>,
    pub line_starts: Arc<Vec<usize>>,
    pub sha256: String,
    pub valid_utf8: bool,
    size: u64,
    mtime: Option<std::time::SystemTime>,
}

impl SourceFileText {
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }
    /// Line `n` (1-based) without its terminator.
    pub fn line(&self, n: usize) -> &str {
        if n == 0 || n > self.line_starts.len() {
            return "";
        }
        let a = self.line_starts[n - 1];
        let b = self.line_starts.get(n).copied().unwrap_or(self.text.len());
        let s = &self.text[a..b];
        s.strip_suffix('\n').map(|x| x.strip_suffix('\r').unwrap_or(x)).unwrap_or(s)
    }
    /// 1-based line containing byte offset `b`.
    pub fn line_of_byte(&self, b: usize) -> usize {
        match self.line_starts.binary_search(&b) {
            Ok(i) => i + 1,
            Err(i) => i,
        }
    }
    /// Byte offset of a (line, column) position in the given encoding.
    pub fn byte_of(&self, line: usize, col: usize, enc: PosEncoding) -> usize {
        if line == 0 || line > self.line_starts.len() {
            return self.text.len();
        }
        let l = self.line(line);
        self.line_starts[line - 1] + crate::text::col_to_byte(l, col, enc)
    }
}

/// Small LRU of recently read files (keyed by canonical path, validated by
/// size and mtime before reuse).
pub struct SourceCache {
    entries: VecDeque<SourceFileText>,
    cap: usize,
}

impl Default for SourceCache {
    fn default() -> Self {
        SourceCache { entries: VecDeque::new(), cap: 8 }
    }
}

impl SourceCache {
    pub fn get(&mut self, root: &Path, rel: &str) -> Result<SourceFileText> {
        let path = resolve_contained(root, rel)?;
        let md = std::fs::metadata(&path)?;
        if md.len() > MAX_FILE_BYTES {
            bail!("{rel}: {} bytes exceeds the display limit", md.len());
        }
        let mtime = md.modified().ok();
        if let Some(pos) = self.entries.iter().position(|e| e.path == path && e.size == md.len() && e.mtime == mtime) {
            let e = self.entries.remove(pos).unwrap();
            self.entries.push_front(e.clone());
            return Ok(e);
        }
        let bytes = std::fs::read(&path)?;
        let sha256 = sha256_hex(&bytes);
        let (text, valid) = decode_lossy(&bytes);
        let text: Arc<str> = Arc::from(text.as_ref());
        let ls = Arc::new(line_starts(&text));
        let e = SourceFileText { path, text, line_starts: ls, sha256, valid_utf8: valid, size: md.len(), mtime };
        self.entries.push_front(e.clone());
        while self.entries.len() > self.cap {
            self.entries.pop_back();
        }
        Ok(e)
    }
}

/// Lines `[from, to]` (1-based, inclusive) with their numbers, bounded.
pub fn excerpt(f: &SourceFileText, from: usize, to: usize, max_lines: usize) -> (Vec<(usize, String)>, bool) {
    let from = from.max(1);
    let to = to.min(f.line_count()).max(from);
    let mut out = Vec::new();
    let mut truncated = false;
    for n in from..=to {
        if out.len() >= max_lines {
            truncated = true;
            break;
        }
        out.push((n, f.line(n).to_string()));
    }
    (out, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lines_and_positions() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("A.lean"), "line one\r\nα𝔽 two\nthree").unwrap();
        let mut c = SourceCache::default();
        let f = c.get(d.path(), "A.lean").unwrap();
        assert_eq!(f.line_count(), 3);
        assert_eq!(f.line(1), "line one");
        assert_eq!(f.line(2), "α𝔽 two");
        assert_eq!(f.line(3), "three");
        assert_eq!(f.line_of_byte(0), 1);
        assert_eq!(f.line_of_byte(10), 2);
        // code point column 2 of line 2 is the space after 𝔽
        let b = f.byte_of(2, 2, PosEncoding::CodePoint);
        assert_eq!(&f.text[b..b + 1], " ");
        let b16 = f.byte_of(2, 3, PosEncoding::Utf16);
        assert_eq!(b, b16);
        let (ex, trunc) = excerpt(&f, 2, 99, 1);
        assert_eq!(ex, vec![(2, "α𝔽 two".to_string())]);
        assert!(trunc);
        assert!(c.get(d.path(), "../A.lean").is_err());
    }
}
