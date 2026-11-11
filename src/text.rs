//! Text handling: sanitising untrusted text before it reaches the terminal,
//! terminal-cell-aware wrapping, and conversions between the position
//! encodings used by different producers (UTF-8 bytes, Unicode scalar values
//! as used by `Lean.Position`, and UTF-16 code units as used by LSP).

use std::borrow::Cow;
use unicode_width::UnicodeWidthChar;

/// Width of a tab stop when expanding tabs for display.
pub const TAB_WIDTH: usize = 2;

fn is_bidi_control(c: char) -> bool {
    matches!(c as u32, 0x202A..=0x202E | 0x2066..=0x2069 | 0x200E | 0x200F | 0x061C)
}

/// Replaces characters that could alter terminal state or mislead the
/// reader. Raw text is never modified in storage; this is display-only.
///
/// * C0 controls (except tab) become Unicode control pictures (`␛` for ESC);
/// * DEL and C1 controls become `�`;
/// * bidirectional overrides become a visible `⟦U+202E⟧` marker (or ASCII);
/// * tabs expand to spaces.
pub fn sanitize(s: &str, ascii: bool) -> Cow<'_, str> {
    if s.chars().all(|c| !(c.is_control() || is_bidi_control(c))) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        sanitize_char(c, ascii, &mut out);
    }
    Cow::Owned(out)
}

/// Appends the display form of one character (see [`sanitize`]).
pub fn sanitize_char(c: char, ascii: bool, out: &mut String) {
    let v = c as u32;
    if c == '\t' {
        out.push_str(&" ".repeat(TAB_WIDTH));
    } else if c == '\n' {
        out.push('\n');
    } else if v < 0x20 {
        if ascii {
            out.push('^');
            out.push(char::from_u32(v + 0x40).unwrap_or('?'));
        } else {
            out.push(char::from_u32(0x2400 + v).unwrap_or('\u{FFFD}'));
        }
    } else if v == 0x7f || (0x80..=0x9f).contains(&v) {
        out.push(if ascii { '?' } else { '\u{FFFD}' });
    } else if is_bidi_control(c) {
        if ascii {
            out.push_str(&format!("<U+{:04X}>", v));
        } else {
            out.push_str(&format!("⟦U+{:04X}⟧", v));
        }
    } else if c.is_control() {
        out.push(if ascii { '?' } else { '\u{FFFD}' });
    } else {
        out.push(c);
    }
}

/// Terminal cell width of a (sanitised) string.
pub fn width(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// Truncates to at most `max` cells, appending `…` (or `~`) when cut.
pub fn truncate(s: &str, max: usize, ascii: bool) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let ell = if ascii { '~' } else { '…' };
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push(ell);
    out
}

/// Truncates to at most `max` cells keeping the *end* (`…tail`), which is
/// the informative part of qualified names.
pub fn truncate_left(s: &str, max: usize, ascii: bool) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let ell = if ascii { '~' } else { '…' };
    let mut out: Vec<char> = Vec::new();
    let mut w = 0;
    for c in s.chars().rev() {
        let cw = c.width().unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push(ell);
    out.into_iter().rev().collect()
}

/// Wraps one logical line (no `\n`) into rows of at most `width` cells.
/// Breaks at spaces when possible, otherwise hard-breaks between characters
/// (never inside a zero-width sequence). Continuation rows are indented by
/// the line's own indentation plus `hang` cells, capped at half the width.
/// Nothing is ever dropped: the concatenation of rows (modulo the inserted
/// indentation and the break spaces) is the input.
pub fn wrap_line(line: &str, width: usize, hang: usize) -> Vec<String> {
    let width = width.max(4);
    if self::width(line) <= width {
        return vec![line.to_string()];
    }
    let chars: Vec<char> = line.chars().collect();
    wrap_ranges(&chars, width, hang)
        .into_iter()
        .map(|(a, z, indent)| {
            let mut row = " ".repeat(indent);
            row.extend(chars[a..z].iter());
            row
        })
        .collect()
}

/// The rows [`wrap_line`] makes, as `(start, end, indent)` over `chars`:
/// row text is `indent` spaces followed by `chars[start..end]`. Lets a
/// caller that carries per-character attributes wrap exactly like plain
/// text.
pub fn wrap_ranges(chars: &[char], width: usize, hang: usize) -> Vec<(usize, usize, usize)> {
    let width = width.max(4);
    let total: usize = chars.iter().map(|c| c.width().unwrap_or(0)).sum();
    if total <= width {
        return vec![(0, chars.len(), 0)];
    }
    let indent_cells = chars.iter().take_while(|c| **c == ' ').count();
    let cont_indent = (indent_cells + hang).min(width / 2);
    let mut rows = Vec::new();
    let mut i = 0;
    let mut first = true;
    while i < chars.len() {
        let prefix = if first { 0 } else { cont_indent };
        let avail = width - prefix;
        // Find how many chars fit.
        let mut w = 0;
        let mut j = i;
        let mut last_space: Option<usize> = None;
        while j < chars.len() {
            let cw = chars[j].width().unwrap_or(0);
            if w + cw > avail {
                break;
            }
            if chars[j] == ' ' && j > i {
                last_space = Some(j);
            }
            w += cw;
            j += 1;
        }
        // Keep zero-width combining marks with their base character.
        while j < chars.len() && j > i && chars[j].width() == Some(0) {
            j += 1;
        }
        let end = if j >= chars.len() {
            chars.len()
        } else if let Some(sp) = last_space.filter(|&sp| sp > i + avail / 3) {
            sp
        } else if j == i {
            i + 1 // a single character wider than the row
        } else {
            j
        };
        rows.push((i, end, prefix));
        i = end;
        // Skip the break space.
        if i < chars.len() && chars[i] == ' ' && end != j {
            i += 1;
        }
        first = false;
    }
    rows
}

/// Wraps multi-line text; each source line wraps independently.
pub fn wrap_text(text: &str, width: usize, hang: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        out.extend(wrap_line(line, width, hang));
    }
    out
}

// ---------------------------------------------------------------------------
// Position encodings
// ---------------------------------------------------------------------------

/// Byte offset of the start of each line (0-based line numbers).
pub fn line_starts(text: &str) -> Vec<usize> {
    let mut v = vec![0];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            v.push(i + 1);
        }
    }
    v
}

/// Converts a column measured in Unicode scalar values (Lean's
/// `Position.column`) to a byte offset within `line`. Columns past the end
/// clamp to the line length.
pub fn codepoint_col_to_byte(line: &str, col: usize) -> usize {
    line.char_indices().nth(col).map(|(b, _)| b).unwrap_or(line.len())
}

/// Converts a byte offset within `line` to a column in Unicode scalar values.
/// Offsets inside a multi-byte character round down.
pub fn byte_to_codepoint_col(line: &str, byte: usize) -> usize {
    let byte = byte.min(line.len());
    line.char_indices()
        .take_while(|(b, c)| b + c.len_utf8() <= byte)
        .count()
}

/// Converts an LSP-style UTF-16 column to a byte offset within `line`.
/// A column pointing into the middle of a surrogate pair rounds down.
pub fn utf16_col_to_byte(line: &str, col: usize) -> usize {
    let mut units = 0;
    for (b, c) in line.char_indices() {
        let n = c.len_utf16();
        if units + n > col {
            return b;
        }
        units += n;
    }
    line.len()
}

/// Converts a byte offset within `line` to a UTF-16 column.
pub fn byte_to_utf16_col(line: &str, byte: usize) -> usize {
    let byte = byte.min(line.len());
    line.char_indices()
        .take_while(|(b, c)| b + c.len_utf8() <= byte)
        .map(|(_, c)| c.len_utf16())
        .sum()
}

/// Position encodings a producer may use. LSP clients and servers negotiate
/// one of these (`positionEncoding`); Lean's `.olean` declaration ranges use
/// code points plus a precomputed UTF-16 column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PosEncoding {
    Utf8,
    Utf16,
    CodePoint,
}

/// Converts a column in the given encoding to a byte offset in `line`.
pub fn col_to_byte(line: &str, col: usize, enc: PosEncoding) -> usize {
    match enc {
        PosEncoding::Utf8 => {
            let mut b = col.min(line.len());
            while b > 0 && !line.is_char_boundary(b) {
                b -= 1;
            }
            b
        }
        PosEncoding::Utf16 => utf16_col_to_byte(line, col),
        PosEncoding::CodePoint => codepoint_col_to_byte(line, col),
    }
}

/// Converts a byte offset in `line` to a column in the given encoding.
pub fn byte_to_col(line: &str, byte: usize, enc: PosEncoding) -> usize {
    match enc {
        PosEncoding::Utf8 => byte.min(line.len()),
        PosEncoding::Utf16 => byte_to_utf16_col(line, byte),
        PosEncoding::CodePoint => byte_to_codepoint_col(line, byte),
    }
}

/// Decodes bytes as UTF-8, replacing invalid sequences; reports whether the
/// input was valid.
pub fn decode_lossy(bytes: &[u8]) -> (Cow<'_, str>, bool) {
    match std::str::from_utf8(bytes) {
        Ok(s) => (Cow::Borrowed(s), true),
        Err(_) => (String::from_utf8_lossy(bytes), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_escape_sequences() {
        let s = "ok\x1b[31mred\x07bell\u{202E}evil\u{9b}";
        let out = sanitize(s, false);
        assert!(!out.contains('\x1b'));
        assert!(!out.contains('\x07'));
        assert!(!out.contains('\u{202E}'));
        assert!(!out.contains('\u{9b}'));
        assert!(out.contains("␛[31mred"));
        assert!(out.contains("⟦U+202E⟧"));
        let a = sanitize(s, true);
        assert!(a.contains("^[[31m"));
        assert!(a.contains("<U+202E>"));
        // Mathematical text is untouched.
        let m = "∀ (α : Type u) {R : α → α → Prop}, ReflTransGen R a b → ℕ";
        assert!(matches!(sanitize(m, false), Cow::Borrowed(_)));
    }

    #[test]
    fn widths_and_truncation() {
        assert_eq!(width("abc"), 3);
        assert_eq!(width("αβγ"), 3);
        assert_eq!(width("全角"), 4);
        assert_eq!(width("e\u{301}"), 1); // combining acute accent
        assert_eq!(truncate("abcdef", 4, false), "abc…");
        assert_eq!(truncate("abcdef", 4, true), "abc~");
        assert_eq!(width(&truncate("全角全角", 5, false)), 5);
    }

    #[test]
    fn wrapping_never_drops_text() {
        let line = "theorem Demo.Graph.search_sound {g : Graph} {fuel : ℕ} {a b : ℕ} (h : search g fuel [a] b = true) : g.Reach a b";
        for w in [10, 17, 23, 40, 80] {
            let rows = wrap_line(line, w, 4);
            for r in &rows {
                assert!(width(r) <= w.max(4), "row too wide at {w}: {r:?}");
            }
            let joined: String = rows.iter().map(|r| r.trim()).collect::<Vec<_>>().join(" ");
            let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join("");
            assert_eq!(norm(&joined), norm(line), "width {w}");
        }
        // Wide characters never split and every row fits.
        let wide = "全角全角全角全角全角";
        let rows = wrap_line(wide, 5, 0);
        assert!(rows.iter().all(|r| width(r) <= 5));
        assert_eq!(rows.concat().replace(' ', ""), wide);
        // Combining marks stay attached to their base character.
        let comb = "aaaae\u{301}bbbb";
        let rows = wrap_line(comb, 5, 0);
        assert!(rows.iter().any(|r| r.contains("e\u{301}")));
    }

    #[test]
    fn position_conversions() {
        // 𝔽 is U+1D53D: 4 UTF-8 bytes, 2 UTF-16 units, 1 code point.
        let line = "a𝔽→b";
        assert_eq!(line.len(), 1 + 4 + 3 + 1);
        // byte offsets of each char: a=0, 𝔽=1, →=5, b=8
        assert_eq!(codepoint_col_to_byte(line, 0), 0);
        assert_eq!(codepoint_col_to_byte(line, 1), 1);
        assert_eq!(codepoint_col_to_byte(line, 2), 5);
        assert_eq!(codepoint_col_to_byte(line, 3), 8);
        assert_eq!(codepoint_col_to_byte(line, 99), line.len());
        assert_eq!(utf16_col_to_byte(line, 1), 1);
        assert_eq!(utf16_col_to_byte(line, 2), 1); // inside surrogate pair: round down
        assert_eq!(utf16_col_to_byte(line, 3), 5);
        assert_eq!(utf16_col_to_byte(line, 4), 8);
        assert_eq!(byte_to_utf16_col(line, 8), 4);
        assert_eq!(byte_to_codepoint_col(line, 8), 3);
        assert_eq!(byte_to_codepoint_col(line, 3), 1); // inside 𝔽: rounds down
        assert_eq!(byte_to_utf16_col(line, 3), 1);
        for enc in [PosEncoding::Utf8, PosEncoding::Utf16, PosEncoding::CodePoint] {
            for (b, _) in line.char_indices() {
                let c = byte_to_col(line, b, enc);
                assert_eq!(col_to_byte(line, c, enc), b, "{enc:?} at byte {b}");
            }
        }
        assert_eq!(col_to_byte(line, 2, PosEncoding::Utf8), 1); // mid-char byte rounds down
    }

    #[test]
    fn line_start_table() {
        let t = "a\nbc\n\nd";
        assert_eq!(line_starts(t), vec![0, 2, 5, 6]);
    }
}
