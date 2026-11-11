//! The baseline provider: whole-file driving of the TextMate tokenizer.
//!
//! Tokenization is strictly sequential (a line's state depends on every
//! line before it), so a file is tokenized from the top in order; the
//! builder keeps the state after the last line it has seen, which lets a
//! caller stop after the visible lines and continue later.

use super::textmate::{Grammar, LineState, ScopeId, Tokenizer};

pub struct BaselineBuilder {
    tok: Tokenizer,
    state: LineState,
    lines_done: usize,
}

/// A file's tokenizer state after some of its lines.
#[derive(Clone, Debug, Default)]
pub struct Suspended {
    state: LineState,
    lines_done: usize,
}

impl Suspended {
    pub fn lines_done(&self) -> usize {
        self.lines_done
    }
}

impl Default for BaselineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl BaselineBuilder {
    pub fn new() -> BaselineBuilder {
        let tok = Tokenizer::new(Grammar::lean4());
        let state = tok.initial_state();
        BaselineBuilder { tok, state, lines_done: 0 }
    }

    /// Starts a new file, keeping compiled patterns.
    pub fn reset(&mut self) {
        self.state = self.tok.initial_state();
        self.lines_done = 0;
    }

    pub fn lines_done(&self) -> usize {
        self.lines_done
    }

    /// Takes the position in the current file so another file can be
    /// tokenized meanwhile; [`BaselineBuilder::resume`] continues it.
    pub fn suspend(&mut self) -> Suspended {
        let s = Suspended { state: std::mem::take(&mut self.state), lines_done: self.lines_done };
        self.reset();
        s
    }

    pub fn resume(&mut self, s: Suspended) {
        self.state = s.state;
        self.lines_done = s.lines_done;
    }

    /// Tokenizes the next line (without terminator). Offsets in `out` are
    /// bytes within `line`. A byte-order mark on the first line is not part
    /// of the Lean source text: it gets the root scope.
    pub fn feed(&mut self, line: &str, out: &mut Vec<(u32, ScopeId)>) {
        let first = self.lines_done == 0;
        self.lines_done += 1;
        match line.strip_prefix('\u{FEFF}').filter(|_| first) {
            Some(rest) => {
                self.tok.tokenize_line(rest, &mut self.state, true, out);
                for t in out.iter_mut() {
                    t.0 += 3;
                }
                // The mark takes the scope of whatever follows it.
                if let Some(t) = out.first_mut() {
                    t.0 = 0;
                }
            }
            None => self.tok.tokenize_line(line, &mut self.state, first, out),
        }
    }
}

/// Splits text into display lines the way [`crate::sourceview`] does: at
/// `\n`, dropping one trailing `\r`.
pub fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l))
}
