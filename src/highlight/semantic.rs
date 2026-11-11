//! Decoding LSP semantic tokens into validated byte ranges.
//!
//! Four coordinate systems are kept apart: source bytes (what is stored),
//! LSP position units (UTF-16 code units unless the server negotiated
//! another encoding), Unicode scalar values, and terminal cells (only the
//! renderer knows those). A token whose range does not land exactly on
//! character boundaries of the exact text that was sent to the server is
//! dropped and counted, never rounded.

use super::{Legend, Lines, SemTok, CHUNK_LINES};
use crate::text::PosEncoding;
use anyhow::{bail, Result};
use std::sync::Arc;

/// Byte offset of column `col` (in `enc` units) within `line`, or `None`
/// if the column is past the end or inside a character.
pub fn unit_to_byte(line: &str, col: usize, enc: PosEncoding) -> Option<usize> {
    match enc {
        PosEncoding::Utf8 => (col <= line.len() && line.is_char_boundary(col)).then_some(col),
        PosEncoding::Utf16 => {
            if line.is_ascii() {
                return (col <= line.len()).then_some(col);
            }
            let mut units = 0;
            for (b, c) in line.char_indices() {
                if units == col {
                    return Some(b);
                }
                if units > col {
                    return None;
                }
                units += c.len_utf16();
            }
            (units == col).then_some(line.len())
        }
        PosEncoding::CodePoint => {
            let mut n = 0;
            for (b, _) in line.char_indices() {
                if n == col {
                    return Some(b);
                }
                n += 1;
            }
            (n == col).then_some(line.len())
        }
    }
}

/// The LSP name of a position encoding, and back.
pub fn encoding_name(enc: PosEncoding) -> &'static str {
    match enc {
        PosEncoding::Utf8 => "utf-8",
        PosEncoding::Utf16 => "utf-16",
        PosEncoding::CodePoint => "utf-32",
    }
}

pub fn parse_encoding(s: &str) -> Option<PosEncoding> {
    Some(match s {
        "utf-8" => PosEncoding::Utf8,
        "utf-16" => PosEncoding::Utf16,
        "utf-32" => PosEncoding::CodePoint,
        _ => return None,
    })
}

#[derive(Debug, Default)]
pub struct Decoded {
    pub chunks: Vec<Arc<Lines<SemTok>>>,
    pub tokens: usize,
    /// Tokens rejected by validation, with the first few reasons.
    pub dropped: usize,
    pub reasons: Vec<String>,
}

/// Decodes a full `SemanticTokens.data` array (relative 5-tuples) against
/// the text it was computed for. `lines` are the document's lines without
/// terminators, split at `\n` as Lean does.
pub fn decode_lsp(data: &[u64], legend: &Legend, enc: PosEncoding, lines: &[&str]) -> Result<Decoded> {
    if data.len() % 5 != 0 {
        bail!("semantic token data length {} is not a multiple of 5", data.len());
    }
    let mut per_line: Vec<Vec<SemTok>> = Vec::new();
    per_line.resize_with(lines.len(), Vec::new);
    let mut out = Decoded::default();
    let (mut line, mut col) = (0u64, 0u64);
    let drop = |out: &mut Decoded, why: String| {
        out.dropped += 1;
        if out.reasons.len() < 5 {
            out.reasons.push(why);
        }
    };
    for t in data.chunks_exact(5) {
        let (dl, ds, len, ty, mods) = (t[0], t[1], t[2], t[3], t[4]);
        line = line.saturating_add(dl);
        col = if dl == 0 { col.saturating_add(ds) } else { ds };
        let Some(text) = lines.get(line as usize) else {
            drop(&mut out, format!("line {} is past the end of the document", line + 1));
            continue;
        };
        if len == 0 {
            drop(&mut out, format!("empty token at {}:{}", line + 1, col));
            continue;
        }
        if ty as usize >= legend.types.len() {
            drop(&mut out, format!("token type {ty} is not in the legend"));
            continue;
        }
        if mods >> legend.modifiers.len().min(32) != 0 {
            drop(&mut out, format!("modifier bits {mods:#x} are not in the legend"));
            continue;
        }
        let range = unit_to_byte(text, col as usize, enc).zip(col.checked_add(len).and_then(|e| unit_to_byte(text, e as usize, enc)));
        let Some((s, e)) = range else {
            drop(&mut out, format!("range {}:{}+{} is not on character boundaries of the line", line + 1, col, len));
            continue;
        };
        let v = &mut per_line[line as usize];
        if v.last().is_some_and(|p: &SemTok| p.end as usize > s) {
            drop(&mut out, format!("token at {}:{} overlaps the previous one", line + 1, col));
            continue;
        }
        v.push(SemTok { start: s as u32, end: e as u32, ty: ty as u16, mods: mods as u32 });
        out.tokens += 1;
    }
    let mut cur = Lines::default();
    for v in per_line {
        cur.push_line(v);
        if cur.len() == CHUNK_LINES {
            out.chunks.push(Arc::new(std::mem::take(&mut cur)));
        }
    }
    if !cur.is_empty() {
        out.chunks.push(Arc::new(cur));
    }
    Ok(out)
}

/// How a file's line ends are written (recorded with a semantic record).
pub fn line_endings(text: &str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count();
    match (crlf, lf) {
        (0, _) => "lf",
        (c, l) if c == l => "crlf",
        _ => "mixed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legend() -> Legend {
        Legend { types: vec!["keyword".into(), "variable".into(), "property".into()], modifiers: vec!["declaration".into()] }
    }

    fn toks(d: &Decoded) -> Vec<(usize, u32, u32, u16)> {
        let mut v = Vec::new();
        let mut n = 0;
        for c in &d.chunks {
            for l in 0..c.len() {
                for t in c.line(l) {
                    v.push((n, t.start, t.end, t.ty));
                }
                n += 1;
            }
        }
        v
    }

    #[test]
    fn utf16_columns_become_exact_bytes() {
        // α: 2 bytes, 1 unit. 𝔽: 4 bytes, 2 units. é as e + combining acute:
        // 1 + 2 bytes, 2 units. Tab: 1 byte, 1 unit. 中: 3 bytes, 1 unit.
        let line = "α𝔽 e\u{301}\t中x";
        assert_eq!(unit_to_byte(line, 0, PosEncoding::Utf16), Some(0));
        assert_eq!(unit_to_byte(line, 1, PosEncoding::Utf16), Some(2));
        assert_eq!(unit_to_byte(line, 2, PosEncoding::Utf16), None, "inside the surrogate pair");
        assert_eq!(unit_to_byte(line, 3, PosEncoding::Utf16), Some(6));
        assert_eq!(unit_to_byte(line, 4, PosEncoding::Utf16), Some(7));
        assert_eq!(unit_to_byte(line, 5, PosEncoding::Utf16), Some(8), "the combining mark is its own unit");
        assert_eq!(unit_to_byte(line, 6, PosEncoding::Utf16), Some(10));
        assert_eq!(unit_to_byte(line, 7, PosEncoding::Utf16), Some(11));
        assert_eq!(unit_to_byte(line, 8, PosEncoding::Utf16), Some(14));
        assert_eq!(unit_to_byte(line, 9, PosEncoding::Utf16), Some(15));
        assert_eq!(unit_to_byte(line, 10, PosEncoding::Utf16), None, "past the end");
        // The same position in the other encodings.
        assert_eq!(unit_to_byte(line, 2, PosEncoding::CodePoint), Some(6));
        assert_eq!(unit_to_byte(line, 6, PosEncoding::Utf8), Some(6));
        assert_eq!(unit_to_byte(line, 3, PosEncoding::Utf8), None, "inside 𝔽");
    }

    #[test]
    fn relative_tokens_decode_with_the_legend() {
        let lines = ["def α𝔽 := x", "", "  y.z"];
        // line 0: `def` (0,3) keyword; `α𝔽` (4, 3 units) variable
        // line 2: `y` (2,1) variable; `z` (4,1) property with a modifier
        let data = [0, 0, 3, 0, 0, 0, 4, 3, 1, 0, 2, 2, 1, 1, 0, 0, 2, 1, 2, 1];
        let d = decode_lsp(&data, &legend(), PosEncoding::Utf16, &lines).unwrap();
        assert_eq!(d.dropped, 0);
        assert_eq!(toks(&d), vec![(0, 0, 3, 0), (0, 4, 10, 1), (2, 2, 3, 1), (2, 4, 5, 2)]);
        assert_eq!(&lines[0][4..10], "α𝔽");
    }

    #[test]
    fn crlf_lines_keep_their_offsets() {
        // Lean splits at \n; the \r stays at the end of the line, after
        // every token, so offsets within the line are unaffected.
        let text = "def a := 1\r\ndef b := a\r\n";
        assert_eq!(line_endings(text), "crlf");
        assert_eq!(line_endings("a\nb\r\n"), "mixed");
        let with_cr: Vec<&str> = text.split('\n').collect();
        let data = [1, 4, 1, 1, 0];
        let d = decode_lsp(&data, &legend(), PosEncoding::Utf16, &with_cr).unwrap();
        assert_eq!(toks(&d), vec![(1, 4, 5, 1)]);
    }

    #[test]
    fn invalid_tokens_are_dropped_and_counted() {
        let lines = ["a𝔽b", "c"];
        let l = legend();
        // inside a surrogate pair; past the line end; unknown type; unknown
        // modifier; line out of range; overlap; zero length
        for (data, why) in [
            (vec![0u64, 2, 1, 0, 0], "boundaries"),
            (vec![0, 3, 5, 0, 0], "boundaries"),
            (vec![0, 0, 1, 9, 0], "legend"),
            (vec![0, 0, 1, 0, 2], "modifier"),
            (vec![5, 0, 1, 0, 0], "past the end"),
            (vec![0, 0, 3, 0, 0, 0, 1, 2, 0, 0], "overlaps"),
            (vec![0, 0, 0, 0, 0], "empty"),
        ] {
            let d = decode_lsp(&data, &l, PosEncoding::Utf16, &lines).unwrap();
            assert_eq!(d.dropped, 1, "{why}");
            assert!(d.reasons[0].contains(why), "{:?} / {why}", d.reasons);
        }
        assert!(decode_lsp(&[0, 0, 1], &l, PosEncoding::Utf16, &lines).is_err());
        // No tokens at all is a valid result with every line present.
        let d = decode_lsp(&[], &l, PosEncoding::Utf16, &lines).unwrap();
        assert_eq!((d.tokens, d.chunks.len(), d.chunks[0].len()), (0, 1, 2));
        let d = decode_lsp(&[], &l, PosEncoding::Utf16, &[]).unwrap();
        assert!(d.chunks.is_empty());
    }
}
