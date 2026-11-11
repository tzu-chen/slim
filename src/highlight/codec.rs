//! Compact, theme-independent storage of per-line tokens.
//!
//! ```text
//! magic[4]  version:u8
//! varint n_lines
//! header (baseline: scope table; semantic: nothing)
//! varint n_chunks, then n_chunks × u32le offsets into the data section
//! data: per chunk, per line: varint n_tokens, then the tokens
//! ```
//! A chunk covers [`CHUNK_LINES`] lines and can be decoded alone, so a view
//! reads only what it shows. Everything read back is validated: a damaged
//! blob yields an error, never an out-of-range span.

use super::textmate::{scopes, ScopeId};
use super::{BaseTok, Lines, SemTok, CHUNK_LINES};
use anyhow::{bail, Result};
use std::sync::Arc;

pub fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

pub struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(b: &'a [u8]) -> Reader<'a> {
        Reader { b, pos: 0 }
    }
    pub fn varint(&mut self) -> Result<u64> {
        let mut v = 0u64;
        let mut shift = 0;
        loop {
            let Some(&byte) = self.b.get(self.pos) else { bail!("truncated") };
            self.pos += 1;
            if shift >= 63 && byte > 1 {
                bail!("varint overflow");
            }
            v |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                return Ok(v);
            }
            shift += 7;
        }
    }
    pub fn u32(&mut self) -> Result<u32> {
        let v = self.varint()?;
        u32::try_from(v).map_err(|_| anyhow::anyhow!("value out of range"))
    }
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.b.len());
        let Some(end) = end else { bail!("truncated") };
        let s = &self.b[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    pub fn pos(&self) -> usize {
        self.pos
    }
    pub fn at_end(&self) -> bool {
        self.pos == self.b.len()
    }
}

/// A token type with a stored form.
pub trait Decode: Sized + Clone {
    const MAGIC: &'static [u8; 4];
    /// Per-blob header state (the baseline scope table).
    type Header: Send + Sync;
    fn read_header(r: &mut Reader) -> Result<Self::Header>;
    fn read_line(r: &mut Reader, h: &Self::Header, n: usize, out: &mut Vec<Self>) -> Result<()>;
}

pub struct Blob<T: Decode> {
    data: Arc<[u8]>,
    n_lines: usize,
    header: T::Header,
    /// Absolute offsets of each chunk, plus the end.
    offsets: Vec<usize>,
}

impl<T: Decode> Blob<T> {
    pub fn parse(data: Vec<u8>) -> Result<Blob<T>> {
        if data.len() < 5 || &data[..4] != T::MAGIC {
            bail!("not a highlight blob");
        }
        if data[4] != 1 {
            bail!("unsupported blob version {}", data[4]);
        }
        let mut r = Reader::new(&data);
        r.bytes(5)?;
        let n_lines = r.varint()? as usize;
        if n_lines > 1 << 28 {
            bail!("implausible line count");
        }
        let header = T::read_header(&mut r)?;
        let n_chunks = r.varint()? as usize;
        if n_chunks != n_lines.div_ceil(CHUNK_LINES) {
            bail!("chunk count does not match line count");
        }
        let table = r.bytes(n_chunks.checked_mul(4).ok_or_else(|| anyhow::anyhow!("overflow"))?)?;
        let base = r.pos();
        let mut offsets = Vec::with_capacity(n_chunks + 1);
        for c in table.chunks_exact(4) {
            let o = base + u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as usize;
            if o > data.len() || offsets.last().is_some_and(|&p| p > o) {
                bail!("bad chunk offset");
            }
            offsets.push(o);
        }
        offsets.push(data.len());
        Ok(Blob { data: Arc::from(data), n_lines, header, offsets })
    }

    pub fn n_lines(&self) -> usize {
        self.n_lines
    }
    pub fn chunk_count(&self) -> usize {
        self.offsets.len() - 1
    }
    pub fn byte_len(&self) -> usize {
        self.data.len()
    }

    pub fn chunk(&self, i: usize) -> Result<Lines<T>> {
        if i + 1 >= self.offsets.len() {
            bail!("chunk out of range");
        }
        let mut r = Reader::new(&self.data[self.offsets[i]..self.offsets[i + 1]]);
        let lines_here = (self.n_lines - i * CHUNK_LINES).min(CHUNK_LINES);
        let mut out = Lines::default();
        let mut buf = Vec::new();
        for _ in 0..lines_here {
            let n = r.varint()? as usize;
            if n > 1 << 22 {
                bail!("implausible token count");
            }
            buf.clear();
            T::read_line(&mut r, &self.header, n, &mut buf)?;
            out.push_line(buf.drain(..));
        }
        if !r.at_end() {
            bail!("trailing bytes in chunk");
        }
        Ok(out)
    }
}

fn finish(magic: &[u8; 4], n_lines: usize, header: &[u8], chunks: Vec<Vec<u8>>) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(magic);
    out.push(1);
    put_varint(&mut out, n_lines as u64);
    out.extend_from_slice(header);
    put_varint(&mut out, chunks.len() as u64);
    let mut off = 0u32;
    for c in &chunks {
        out.extend_from_slice(&off.to_le_bytes());
        off += c.len() as u32;
    }
    for c in chunks {
        out.extend_from_slice(&c);
    }
    out
}

// --- baseline ---------------------------------------------------------------

impl Decode for BaseTok {
    const MAGIC: &'static [u8; 4] = b"LNB1";
    /// Local scope index -> process scope id.
    type Header = Vec<ScopeId>;

    fn read_header(r: &mut Reader) -> Result<Vec<ScopeId>> {
        let n = r.varint()? as usize;
        if n > 1 << 20 {
            bail!("implausible scope table");
        }
        let mut ids = Vec::with_capacity(n);
        for _ in 0..n {
            let len = r.varint()? as usize;
            if len > 4096 {
                bail!("scope path too long");
            }
            let s = std::str::from_utf8(r.bytes(len)?)?;
            let Some(id) = scopes().push_all(None, s) else { bail!("empty scope path") };
            ids.push(id);
        }
        Ok(ids)
    }

    fn read_line(r: &mut Reader, h: &Vec<ScopeId>, n: usize, out: &mut Vec<Self>) -> Result<()> {
        let mut start = 0u32;
        for i in 0..n {
            let d = r.u32()?;
            if i > 0 && d == 0 {
                bail!("token starts are not increasing");
            }
            start = start.checked_add(d).ok_or_else(|| anyhow::anyhow!("offset overflow"))?;
            let Some(&id) = h.get(r.varint()? as usize) else { bail!("scope index out of range") };
            out.push((start, id));
        }
        Ok(())
    }
}

/// Encodes a complete baseline layer (`chunks` in order, [`CHUNK_LINES`]
/// lines each except the last).
pub fn encode_baseline(chunks: &[Arc<Lines<BaseTok>>], n_lines: usize) -> Vec<u8> {
    let mut local: std::collections::HashMap<ScopeId, u32> = Default::default();
    let mut table: Vec<ScopeId> = Vec::new();
    let mut encoded = Vec::with_capacity(chunks.len());
    for c in chunks {
        let mut b = Vec::new();
        for l in 0..c.len() {
            let toks = c.line(l);
            put_varint(&mut b, toks.len() as u64);
            let mut prev = 0u32;
            for &(start, id) in toks {
                put_varint(&mut b, (start - prev) as u64);
                prev = start;
                let idx = *local.entry(id).or_insert_with(|| {
                    table.push(id);
                    table.len() as u32 - 1
                });
                put_varint(&mut b, idx as u64);
            }
        }
        encoded.push(b);
    }
    let mut header = Vec::new();
    put_varint(&mut header, table.len() as u64);
    for id in table {
        let s = scopes().display(id);
        put_varint(&mut header, s.len() as u64);
        header.extend_from_slice(s.as_bytes());
    }
    finish(BaseTok::MAGIC, n_lines, &header, encoded)
}

// --- semantic ---------------------------------------------------------------

impl Decode for SemTok {
    const MAGIC: &'static [u8; 4] = b"LNS1";
    type Header = ();

    fn read_header(_: &mut Reader) -> Result<()> {
        Ok(())
    }

    fn read_line(r: &mut Reader, _: &(), n: usize, out: &mut Vec<Self>) -> Result<()> {
        let mut pos = 0u32;
        for _ in 0..n {
            let start = pos.checked_add(r.u32()?).ok_or_else(|| anyhow::anyhow!("offset overflow"))?;
            let len = r.u32()?;
            if len == 0 {
                bail!("empty semantic token");
            }
            let end = start.checked_add(len).ok_or_else(|| anyhow::anyhow!("offset overflow"))?;
            let ty = u16::try_from(r.varint()?).map_err(|_| anyhow::anyhow!("token type out of range"))?;
            let mods = r.u32()?;
            out.push(SemTok { start, end, ty, mods });
            pos = end;
        }
        Ok(())
    }
}

/// Encodes semantic tokens; within a line they must be sorted and must not
/// overlap (the decoder relies on it).
pub fn encode_semantic(chunks: &[Arc<Lines<SemTok>>], n_lines: usize) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(chunks.len());
    for c in chunks {
        let mut b = Vec::new();
        for l in 0..c.len() {
            let toks = c.line(l);
            put_varint(&mut b, toks.len() as u64);
            let mut pos = 0u32;
            for t in toks {
                put_varint(&mut b, (t.start - pos) as u64);
                put_varint(&mut b, (t.end - t.start) as u64);
                put_varint(&mut b, t.ty as u64);
                put_varint(&mut b, t.mods as u64);
                pos = t.end;
            }
        }
        encoded.push(b);
    }
    finish(SemTok::MAGIC, n_lines, &[], encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sem_chunks(n_lines: usize) -> Vec<Arc<Lines<SemTok>>> {
        let mut chunks = Vec::new();
        let mut cur = Lines::default();
        for l in 0..n_lines {
            let toks: Vec<SemTok> = (0..(l % 4) as u32).map(|k| SemTok { start: k * 10 + l as u32 % 3, end: k * 10 + 5, ty: (k % 3) as u16, mods: k }).collect();
            cur.push_line(toks);
            if cur.len() == CHUNK_LINES {
                chunks.push(Arc::new(std::mem::take(&mut cur)));
            }
        }
        if !cur.is_empty() {
            chunks.push(Arc::new(cur));
        }
        chunks
    }

    #[test]
    fn semantic_round_trip_and_bounded_chunks() {
        for n in [0usize, 1, 255, 256, 257, 1000] {
            let chunks = sem_chunks(n);
            let blob = Blob::<SemTok>::parse(encode_semantic(&chunks, n)).unwrap();
            assert_eq!(blob.n_lines(), n);
            assert_eq!(blob.chunk_count(), chunks.len());
            // Any chunk decodes on its own.
            for i in (0..chunks.len()).rev() {
                assert_eq!(blob.chunk(i).unwrap(), *chunks[i]);
            }
        }
    }

    #[test]
    fn baseline_round_trip_keeps_scope_paths() {
        let a = scopes().push_all(None, "source.lean4 comment.block.lean4").unwrap();
        let b = scopes().push_all(None, "source.lean4").unwrap();
        let mut l = Lines::default();
        l.push_line([(0, b), (4, a), (9, b)]);
        l.push_line([(0, a)]);
        l.push_line([]);
        let chunks = vec![Arc::new(l)];
        let blob = Blob::<BaseTok>::parse(encode_baseline(&chunks, 3)).unwrap();
        assert_eq!(blob.chunk(0).unwrap(), *chunks[0]);
    }

    #[test]
    fn damaged_blobs_are_rejected_not_trusted() {
        let chunks = sem_chunks(600);
        let good = encode_semantic(&chunks, 600);
        assert!(Blob::<SemTok>::parse(Vec::new()).is_err());
        assert!(Blob::<SemTok>::parse(b"XXXX\x01\x00\x00".to_vec()).is_err());
        assert!(Blob::<BaseTok>::parse(good.clone()).is_err(), "wrong kind");
        // Truncation, and every single-byte corruption: parse or chunk
        // decoding fails, or yields well-formed tokens; never a panic.
        for cut in [5, 6, 9, 20, good.len() - 1] {
            if let Ok(b) = Blob::<SemTok>::parse(good[..cut].to_vec()) {
                assert!((0..b.chunk_count()).any(|i| b.chunk(i).is_err()));
            }
        }
        for i in 0..good.len().min(400) {
            let mut bad = good.clone();
            bad[i] ^= 0xff;
            if let Ok(b) = Blob::<SemTok>::parse(bad) {
                for c in 0..b.chunk_count() {
                    if let Ok(lines) = b.chunk(c) {
                        for l in 0..lines.len() {
                            let mut pos = 0;
                            for t in lines.line(l) {
                                assert!(t.start >= pos && t.end > t.start);
                                pos = t.end;
                            }
                        }
                    }
                }
            }
        }
    }
}
