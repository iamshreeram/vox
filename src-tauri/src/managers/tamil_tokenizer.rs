//! Pure-Rust SentencePiece BPE tokenizer for the Tamil TTS pack (`tokenizer.model`).
//!
//! The ModelProto is read with a small protobuf reader (unknown fields are skipped by wire
//! type). Encoding follows the SentencePiece steps the pack uses: precompiled charsmap
//! normalization (longest match), whitespace collapsing, dummy prefix, space escaping, word
//! splitting at the space mark, and BPE merges ordered by piece score (highest first, leftmost
//! on ties). Consecutive unknown symbols inside a word fuse into one unk id.
//!
//! Semantics follow SentencePiece (Apache-2.0, Google). The charsmap trie walk mirrors the
//! layout used by the `spm_precompiled` crate, but takes the longest match as SentencePiece does.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::path::Path;

/// SentencePiece space mark (LOWER ONE EIGHTH BLOCK).
const SPACE_MARK: char = '\u{2581}';
/// Text rendered for an unknown id (U+2047 DOUBLE QUESTION MARK, surrounded by spaces).
const UNK_TEXT: &str = " \u{2047} ";
const MODEL_TYPE_BPE: u64 = 2;

/// Kind of a vocabulary piece (SentencePiece `SentencePiece.Type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PieceKind {
    /// Regular piece (type 1).
    Normal,
    /// Unknown piece (type 2).
    Unknown,
    /// Control piece such as `<s>` (type 3). Produces no text when decoded.
    Control,
    /// User-defined piece (type 4).
    UserDefined,
    /// Unused piece (type 5).
    Unused,
    /// Byte piece (type 6).
    Byte,
}

/// One vocabulary entry.
#[derive(Clone, Debug)]
pub(crate) struct Piece {
    pub(crate) text: String,
    pub(crate) score: f32,
    pub(crate) kind: PieceKind,
}

/// SentencePiece BPE tokenizer loaded from a `tokenizer.model` file.
#[derive(Debug)]
pub struct TamilTokenizer {
    pieces: Vec<Piece>,
    /// Text to (id, score) for pieces that may be produced by encoding.
    index: HashMap<String, (u32, f32)>,
    unk_id: u32,
    charsmap: Option<CharsMap>,
    add_dummy_prefix: bool,
    remove_extra_whitespaces: bool,
    escape_whitespaces: bool,
}

impl TamilTokenizer {
    /// Loads a SentencePiece BPE `tokenizer.model` from disk.
    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes =
            std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Self::from_bytes(&bytes)
    }

    /// Parses a serialized SentencePiece ModelProto of type BPE.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.is_empty() {
            return Err("tokenizer model is empty".to_string());
        }
        let mut reader = Reader::new(bytes);
        let mut pieces = Vec::new();
        let mut trainer = TrainerSpec::default();
        let mut normalizer = NormalizerSpec::default();
        while !reader.done() {
            let (field, wire) = reader.field()?;
            match (field, wire) {
                (1, 2) => pieces.push(parse_piece(reader.len_delimited()?)?),
                (2, 2) => trainer = parse_trainer(reader.len_delimited()?)?,
                (3, 2) => normalizer = parse_normalizer(reader.len_delimited()?)?,
                _ => reader.skip(wire)?,
            }
        }
        if pieces.is_empty() {
            return Err("tokenizer model has no pieces".to_string());
        }
        if trainer.model_type != MODEL_TYPE_BPE {
            return Err(format!(
                "tokenizer model_type {} is not BPE (2)",
                trainer.model_type
            ));
        }
        if trainer.byte_fallback {
            return Err("byte fallback is not supported".to_string());
        }
        let unk_id = usize::try_from(trainer.unk_id)
            .ok()
            .filter(|id| *id < pieces.len())
            .ok_or_else(|| format!("unk_id {} is outside the vocabulary", trainer.unk_id))?;
        let charsmap = CharsMap::parse(&normalizer.charsmap)?;

        let mut index = HashMap::new();
        for (id, piece) in pieces.iter().enumerate() {
            if matches!(piece.kind, PieceKind::Normal | PieceKind::UserDefined) {
                let id = u32::try_from(id).map_err(|_| "too many pieces".to_string())?;
                index.entry(piece.text.clone()).or_insert((id, piece.score));
            }
        }

        Ok(Self {
            pieces,
            index,
            unk_id: u32::try_from(unk_id).map_err(|_| "unk_id out of range".to_string())?,
            charsmap,
            add_dummy_prefix: normalizer.add_dummy_prefix,
            remove_extra_whitespaces: normalizer.remove_extra_whitespaces,
            escape_whitespaces: normalizer.escape_whitespaces,
        })
    }

    /// Number of vocabulary pieces (ids run from 0 to `vocab_size() - 1`).
    pub fn vocab_size(&self) -> usize {
        self.pieces.len()
    }

    /// Encodes text to ids. No BOS or EOS is added. Unknown symbols map to id 0.
    pub fn encode(&self, text: &str) -> Vec<u32> {
        let mut normalized = self.normalize(text);
        if normalized.is_empty() {
            return Vec::new();
        }
        if self.add_dummy_prefix {
            normalized.insert(0, ' ');
        }
        if self.escape_whitespaces {
            normalized = normalized.replace(' ', &SPACE_MARK.to_string());
        }
        let mark = if self.escape_whitespaces {
            SPACE_MARK
        } else {
            ' '
        };
        let mut ids = Vec::new();
        let mut word = String::new();
        for ch in normalized.chars() {
            if ch == mark && !word.is_empty() {
                ids.extend(self.encode_word(&word));
                word.clear();
            }
            word.push(ch);
        }
        if !word.is_empty() {
            ids.extend(self.encode_word(&word));
        }
        ids
    }

    /// Decodes ids to text. Control pieces produce nothing, unknown ids (including ids outside
    /// the vocabulary) produce the replacement marker, and the leading space mark is dropped.
    pub fn decode(&self, ids: &[u32]) -> String {
        let mut out = String::new();
        let mut at_start = self.add_dummy_prefix;
        for &id in ids {
            let Some(piece) = self.piece(id) else {
                out.push_str(UNK_TEXT);
                at_start = false;
                continue;
            };
            if piece.kind == PieceKind::Control {
                continue;
            }
            if id == self.unk_id || piece.kind == PieceKind::Unknown {
                out.push_str(UNK_TEXT);
            } else {
                let body = if at_start {
                    piece
                        .text
                        .strip_prefix(SPACE_MARK)
                        .unwrap_or(piece.text.as_str())
                } else {
                    piece.text.as_str()
                };
                out.push_str(&body.replace(SPACE_MARK, " "));
            }
            at_start = false;
        }
        out
    }

    /// Returns the vocabulary entry for `id`, if it exists.
    pub(crate) fn piece(&self, id: u32) -> Option<&Piece> {
        self.pieces.get(usize::try_from(id).ok()?)
    }

    /// Applies the charsmap, then collapses and trims spaces when configured.
    fn normalize(&self, text: &str) -> String {
        let collapse = self.remove_extra_whitespaces;
        let mut out = String::with_capacity(text.len());
        let mut pos = 0;
        while pos < text.len() {
            let Some(rest) = text.get(pos..) else { break };
            let matched = self
                .charsmap
                .as_ref()
                .and_then(|map| map.longest_match(rest));
            match matched {
                Some((len, replacement)) if text.is_char_boundary(pos + len) => {
                    for ch in replacement.chars() {
                        push_normalized(&mut out, ch, collapse);
                    }
                    pos += len;
                }
                _ => {
                    let Some(ch) = rest.chars().next() else { break };
                    push_normalized(&mut out, ch, collapse);
                    pos += ch.len_utf8();
                }
            }
        }
        if collapse {
            while out.ends_with(' ') {
                out.pop();
            }
        }
        out
    }

    /// Runs BPE on one word (a space-mark-initial run) and returns its ids.
    fn encode_word(&self, word: &str) -> Vec<u32> {
        let chars: Vec<char> = word.chars().collect();
        let count = chars.len();
        let mut syms: Vec<Symbol> = chars
            .iter()
            .enumerate()
            .map(|(i, ch)| Symbol {
                text: ch.to_string(),
                prev: i.checked_sub(1),
                next: (i + 1 < count).then_some(i + 1),
                dead: false,
            })
            .collect();

        let mut heap = BinaryHeap::new();
        for left in 0..count.saturating_sub(1) {
            self.push_merge(&syms, &mut heap, left, left + 1);
        }
        while let Some(merge) = heap.pop() {
            let (left, right) = (merge.left, merge.right);
            let Some(right_sym) = syms.get(right) else {
                continue;
            };
            let Some(left_sym) = syms.get(left) else {
                continue;
            };
            let stale = left_sym.dead
                || right_sym.dead
                || left_sym.next != Some(right)
                || format!("{}{}", left_sym.text, right_sym.text) != merge.merged;
            if stale {
                continue;
            }
            let after = right_sym.next;
            if let Some(sym) = syms.get_mut(left) {
                sym.text = merge.merged;
                sym.next = after;
            }
            if let Some(sym) = syms.get_mut(right) {
                sym.dead = true;
            }
            if let Some(after) = after.and_then(|i| syms.get_mut(i)) {
                after.prev = Some(left);
            }
            let before = syms.get(left).and_then(|sym| sym.prev);
            let next = syms.get(left).and_then(|sym| sym.next);
            if let Some(before) = before {
                self.push_merge(&syms, &mut heap, before, left);
            }
            if let Some(next) = next {
                self.push_merge(&syms, &mut heap, left, next);
            }
        }

        let mut ids = Vec::new();
        let mut in_unknown_run = false;
        let mut cursor = Some(0usize);
        while let Some(index) = cursor {
            let Some(sym) = syms.get(index) else { break };
            if let Some(&(id, _)) = self.index.get(&sym.text) {
                ids.push(id);
                in_unknown_run = false;
            } else {
                if !in_unknown_run {
                    ids.push(self.unk_id);
                }
                in_unknown_run = true;
            }
            cursor = sym.next;
        }
        ids
    }

    /// Queues the merge of `left` and `right` if their concatenation is a vocabulary piece.
    fn push_merge(&self, syms: &[Symbol], heap: &mut BinaryHeap<Merge>, left: usize, right: usize) {
        let (Some(left_sym), Some(right_sym)) = (syms.get(left), syms.get(right)) else {
            return;
        };
        let merged = format!("{}{}", left_sym.text, right_sym.text);
        if let Some(&(_, score)) = self.index.get(&merged) {
            heap.push(Merge {
                score,
                left,
                right,
                merged,
            });
        }
    }
}

/// Appends one normalized character, dropping spaces that would repeat or lead.
fn push_normalized(out: &mut String, ch: char, collapse: bool) {
    if collapse && ch == ' ' && (out.is_empty() || out.ends_with(' ')) {
        return;
    }
    out.push(ch);
}

/// One BPE symbol in the linked list of a word.
#[derive(Debug)]
struct Symbol {
    text: String,
    prev: Option<usize>,
    next: Option<usize>,
    dead: bool,
}

/// A candidate merge. The heap pops the highest score first, leftmost on ties.
#[derive(Debug)]
struct Merge {
    score: f32,
    left: usize,
    right: usize,
    merged: String,
}

impl PartialEq for Merge {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Merge {}

impl PartialOrd for Merge {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Merge {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .total_cmp(&other.score)
            .then_with(|| other.left.cmp(&self.left))
    }
}

/// Precompiled SentencePiece charsmap: a double-array trie plus NUL-terminated replacements.
#[derive(Debug)]
struct CharsMap {
    trie: Vec<u32>,
    normalized: Vec<u8>,
}

impl CharsMap {
    /// Parses the `precompiled_charsmap` bytes. Empty input means no charsmap.
    fn parse(data: &[u8]) -> Result<Option<Self>, String> {
        if data.is_empty() {
            return Ok(None);
        }
        let head: [u8; 4] = data
            .get(0..4)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or("precompiled charsmap is truncated")?;
        let trie_len = u32::from_le_bytes(head) as usize;
        if !trie_len.is_multiple_of(4) {
            return Err("precompiled charsmap trie length is not a multiple of 4".to_string());
        }
        let trie_end = 4usize
            .checked_add(trie_len)
            .ok_or("precompiled charsmap trie length overflows")?;
        let trie_bytes = data
            .get(4..trie_end)
            .ok_or("precompiled charsmap trie is truncated")?;
        if trie_bytes.is_empty() {
            return Err("precompiled charsmap trie is empty".to_string());
        }
        let (units, _) = trie_bytes.as_chunks::<4>();
        let trie = units.iter().map(|c| u32::from_le_bytes(*c)).collect();
        let normalized = data
            .get(trie_end..)
            .ok_or("precompiled charsmap replacements are truncated")?;
        std::str::from_utf8(normalized)
            .map_err(|_| "precompiled charsmap replacements are not UTF-8".to_string())?;
        Ok(Some(Self {
            trie,
            normalized: normalized.to_vec(),
        }))
    }

    /// Longest charsmap key that prefixes `text`. Returns (bytes consumed, replacement).
    fn longest_match(&self, text: &str) -> Option<(usize, &str)> {
        let root = *self.trie.first()?;
        let mut node = unit_offset(root);
        let mut best: Option<(usize, usize)> = None;
        for (depth, &byte) in text.as_bytes().iter().enumerate() {
            if byte == 0 {
                break;
            }
            node ^= usize::from(byte);
            let Some(&unit) = self.trie.get(node) else {
                break;
            };
            if unit_label(unit) != usize::from(byte) {
                break;
            }
            node ^= unit_offset(unit);
            if unit_has_leaf(unit) {
                let Some(&leaf) = self.trie.get(node) else {
                    break;
                };
                best = Some((depth + 1, (leaf & 0x7fff_ffff) as usize));
            }
        }
        let (len, start) = best?;
        let tail = self.normalized.get(start..)?;
        let end = start + tail.iter().position(|&b| b == 0)?;
        let replacement = std::str::from_utf8(self.normalized.get(start..end)?).ok()?;
        Some((len, replacement))
    }
}

fn unit_has_leaf(unit: u32) -> bool {
    (unit >> 8) & 1 == 1
}

fn unit_label(unit: u32) -> usize {
    (unit & 0x8000_00ff) as usize
}

fn unit_offset(unit: u32) -> usize {
    ((unit >> 10) << ((unit & (1 << 9)) >> 6)) as usize
}

#[derive(Debug)]
struct TrainerSpec {
    model_type: u64,
    unk_id: i64,
    byte_fallback: bool,
}

impl Default for TrainerSpec {
    fn default() -> Self {
        // Proto default for model_type is UNIGRAM (1), so a model without a trainer is rejected.
        Self {
            model_type: 1,
            unk_id: 0,
            byte_fallback: false,
        }
    }
}

#[derive(Debug)]
struct NormalizerSpec {
    charsmap: Vec<u8>,
    add_dummy_prefix: bool,
    remove_extra_whitespaces: bool,
    escape_whitespaces: bool,
}

impl Default for NormalizerSpec {
    fn default() -> Self {
        Self {
            charsmap: Vec::new(),
            add_dummy_prefix: true,
            remove_extra_whitespaces: true,
            escape_whitespaces: true,
        }
    }
}

/// Minimal protobuf wire reader (varint, fixed32/64, length-delimited).
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn done(&self) -> bool {
        self.pos >= self.data.len()
    }

    fn byte(&mut self) -> Result<u8, String> {
        let byte = *self
            .data
            .get(self.pos)
            .ok_or_else(|| "tokenizer model is truncated".to_string())?;
        self.pos += 1;
        Ok(byte)
    }

    fn varint(&mut self) -> Result<u64, String> {
        let mut value = 0u64;
        let mut shift = 0u32;
        loop {
            let byte = self.byte()?;
            if shift < 64 {
                value |= u64::from(byte & 0x7f) << shift;
            }
            if byte & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
            if shift >= 70 {
                return Err("tokenizer model has an overlong varint".to_string());
            }
        }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], String> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or_else(|| "tokenizer model length overflows".to_string())?;
        let slice = self
            .data
            .get(self.pos..end)
            .ok_or_else(|| "tokenizer model is truncated".to_string())?;
        self.pos = end;
        Ok(slice)
    }

    fn fixed32(&mut self) -> Result<[u8; 4], String> {
        let slice = self.take(4)?;
        <[u8; 4]>::try_from(slice).map_err(|_| "tokenizer model is truncated".to_string())
    }

    fn len_delimited(&mut self) -> Result<&'a [u8], String> {
        let len = usize::try_from(self.varint()?)
            .map_err(|_| "tokenizer model length overflows".to_string())?;
        self.take(len)
    }

    fn field(&mut self) -> Result<(u64, u8), String> {
        let tag = self.varint()?;
        let field = tag >> 3;
        if field == 0 {
            return Err("tokenizer model has field number 0".to_string());
        }
        Ok((field, (tag & 7) as u8))
    }

    fn skip(&mut self, wire: u8) -> Result<(), String> {
        match wire {
            0 => {
                self.varint()?;
            }
            1 => {
                self.take(8)?;
            }
            2 => {
                self.len_delimited()?;
            }
            5 => {
                self.take(4)?;
            }
            other => return Err(format!("unsupported protobuf wire type {other}")),
        }
        Ok(())
    }
}

fn parse_piece(data: &[u8]) -> Result<Piece, String> {
    let mut reader = Reader::new(data);
    let mut text: Option<String> = None;
    let mut score = 0f32;
    // Proto default for the piece type is NORMAL (1).
    let mut kind = 1u64;
    while !reader.done() {
        let (field, wire) = reader.field()?;
        match (field, wire) {
            (1, 2) => {
                let bytes = reader.len_delimited()?;
                let parsed = std::str::from_utf8(bytes)
                    .map_err(|_| "piece text is not UTF-8".to_string())?;
                text = Some(parsed.to_string());
            }
            (2, 5) => score = f32::from_le_bytes(reader.fixed32()?),
            (3, 0) => kind = reader.varint()?,
            _ => reader.skip(wire)?,
        }
    }
    let text = text.ok_or_else(|| "piece has no text".to_string())?;
    Ok(Piece {
        text,
        score,
        kind: piece_kind(kind)?,
    })
}

fn piece_kind(value: u64) -> Result<PieceKind, String> {
    Ok(match value {
        1 => PieceKind::Normal,
        2 => PieceKind::Unknown,
        3 => PieceKind::Control,
        4 => PieceKind::UserDefined,
        5 => PieceKind::Unused,
        6 => PieceKind::Byte,
        other => return Err(format!("unknown piece type {other}")),
    })
}

fn parse_trainer(data: &[u8]) -> Result<TrainerSpec, String> {
    let mut reader = Reader::new(data);
    let mut spec = TrainerSpec::default();
    while !reader.done() {
        let (field, wire) = reader.field()?;
        match (field, wire) {
            (3, 0) => spec.model_type = reader.varint()?,
            // int32 is sign-extended to 64 bits on the wire.
            (40, 0) => spec.unk_id = reader.varint()? as i64,
            (35, 0) => spec.byte_fallback = reader.varint()? != 0,
            _ => reader.skip(wire)?,
        }
    }
    Ok(spec)
}

fn parse_normalizer(data: &[u8]) -> Result<NormalizerSpec, String> {
    let mut reader = Reader::new(data);
    let mut spec = NormalizerSpec::default();
    while !reader.done() {
        let (field, wire) = reader.field()?;
        match (field, wire) {
            (2, 2) => spec.charsmap = reader.len_delimited()?.to_vec(),
            (3, 0) => spec.add_dummy_prefix = reader.varint()? != 0,
            (4, 0) => spec.remove_extra_whitespaces = reader.varint()? != 0,
            (5, 0) => spec.escape_whitespaces = reader.varint()? != 0,
            _ => reader.skip(wire)?,
        }
    }
    Ok(spec)
}
