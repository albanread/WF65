//! `Wf64Layout` — paged_gc `HeapLayout` impl for WF64.
//!
//! Tag scheme (3 bits, low bits of every scanned cell):
//!
//! ```text
//! 000  Fixnum       → Immediate   (only appears as nil/0 — see below)
//! 001  Cons         → PointerCons (reserved; not used in V1a)
//! 010  FloatVecPtr  → PointerHeader  — raw f64 payload
//! 011  RefVecPtr    → PointerHeader  — payload cells are GC pointers
//! 100  StringPtr    → PointerHeader  — raw UTF-8 bytes
//! 101  BuilderPtr   → PointerHeader  — MutStringBuilder, 2-cell header
//! 110  Immediate    → Immediate   (reserved for future non-pointer values)
//! 111  Forward      → Forwarded   (GC-internal, written by evacuator)
//! ```
//!
//! **Fixnum tag (000) is for nil only.**  WF64's Forth values are not
//! tagged — `5` stays `5`, arithmetic primitives stay arithmetic
//! primitives.  The Fixnum tag exists solely so the value `0` (the
//! fill word for fresh cells and the "empty" value of a HEAPPTR slot)
//! classifies cleanly as `Immediate` and the GC skips it.  No general
//! integer is ever stored where the GC scans — only HEAPPTR slots and
//! GC heap pointer cells, both of which hold either `0` or a tagged
//! pointer.
//!
//! Header word layout (1 cell per header-bearing object):
//!
//! ```text
//!   bits  0..5   type    (5 bits, indexes the HeapType enum, 32 slots)
//!   bits  5..29  length  (24 bits — see below)
//!   bits 29..37  GC bits (mark, age, etc. — managed by paged_gc)
//!   bits 37..64  reserved
//! ```
//!
//! Length semantics depend on type:
//!   - `FloatVec`, `RefVec`: length is **payload cells** (excluding
//!     header).  Max 16M cells = 128 MB per vector.
//!   - `String`: length is **bytes**.  Max 16 MB per string.  Payload
//!     is `ceil(length/8)` cells of packed UTF-8.
//!
//! See `docs/gc_design.md` for the full design.  See
//! `docs/strings_design.md` for `MutStringBuilder` (V2s, not yet
//! implemented — it uses a 2-cell header).

use newgc_core::traits::{HeapLayout, ObjectLayout, PointerKind, WordKind};

// ─── Tag scheme ──────────────────────────────────────────────────────

/// Number of low bits reserved for the tag.
pub const TAG_BITS: u32 = 3;
/// Mask for the tag bits.
pub const TAG_MASK: u64 = 0b111;
/// Mask for the payload (= the aligned address, for pointer tags).
pub const PAYLOAD_MASK: u64 = !TAG_MASK;

pub const TAG_FIXNUM:    u64 = 0b000;
pub const TAG_CONS:      u64 = 0b001;
pub const TAG_FLOATVEC:  u64 = 0b010;
pub const TAG_REFVEC:    u64 = 0b011;
pub const TAG_STRING:    u64 = 0b100;
pub const TAG_BUILDER:   u64 = 0b101;
pub const TAG_IMMEDIATE: u64 = 0b110;
pub const TAG_FORWARD:   u64 = 0b111;

// ─── Header word layout ──────────────────────────────────────────────

pub const TYPE_SHIFT: u32 = 0;
pub const TYPE_BITS:  u32 = 5;
pub const TYPE_MASK:  u64 = (1 << TYPE_BITS) - 1;

pub const LEN_SHIFT: u32 = TYPE_SHIFT + TYPE_BITS;     // = 5
pub const LEN_BITS:  u32 = 24;
pub const LEN_MASK:  u64 = (1 << LEN_BITS) - 1;        // 0xFFFFFF

pub const GC_SHIFT: u32 = LEN_SHIFT + LEN_BITS;        // = 29
pub const GC_BITS:  u32 = 8;
pub const GC_MASK:  u64 = (1 << GC_BITS) - 1;

/// Maximum length value encodable in the 24-bit field.
/// For FloatVec / RefVec: max payload cells.
/// For String: max bytes.
pub const MAX_LENGTH: u32 = (1 << LEN_BITS) - 1;       // 16_777_215

// ─── HeapType enum ───────────────────────────────────────────────────

/// Object-type discriminator inside the header word's 5-bit type field.
///
/// Values are chosen to match their pointer-tag values (so a FloatVec
/// pointer has low bits `010` and its header's type field is `2`).
/// This is convention for debugging clarity; nothing in the GC machinery
/// requires the two namespaces to align.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum HeapType {
    /// Raw f64 array.  Payload cells are opaque.
    FloatVec = 2,
    /// Vector of GC pointers.  Every payload cell is a Word the GC
    /// must classify and follow.
    RefVec = 3,
    /// Raw UTF-8 bytes.  Payload is `ceil(byte_length/8)` cells.
    /// Length in the header is BYTES, not cells.
    String = 4,
    /// Mutable string builder (V2s).  Header is **2 cells**: cell 0
    /// is the standard type/length/GC word (length = currently-used
    /// bytes), cell 1 is the capacity in bytes.  Payload is
    /// `ceil(capacity/8)` cells; only the first `length` bytes are
    /// meaningful, the rest is uninitialised.  GC walks
    /// `total_cells = 2 + ceil(capacity/8)` — uses capacity, not
    /// length, so the GC knows the full allocated extent regardless
    /// of how much the user has appended.
    MutStringBuilder = 5,
}

impl HeapType {
    pub fn from_bits(bits: u8) -> Option<HeapType> {
        match bits {
            2 => Some(HeapType::FloatVec),
            3 => Some(HeapType::RefVec),
            4 => Some(HeapType::String),
            5 => Some(HeapType::MutStringBuilder),
            _ => None,
        }
    }

    /// The tag bits for a pointer to this type of object.
    pub fn pointer_tag(self) -> u64 {
        match self {
            HeapType::FloatVec         => TAG_FLOATVEC,
            HeapType::RefVec           => TAG_REFVEC,
            HeapType::String           => TAG_STRING,
            HeapType::MutStringBuilder => TAG_BUILDER,
        }
    }
}

// ─── Helper constructors ─────────────────────────────────────────────

/// Build a header word for a given type and length.
///
/// For `FloatVec` / `RefVec`: `length` is payload cell count.
/// For `String`: `length` is byte count.
pub fn make_header(ty: HeapType, length: u32) -> u64 {
    debug_assert!(length <= MAX_LENGTH, "length {length} exceeds 24-bit field");
    ((ty as u64) << TYPE_SHIFT) | ((length as u64 & LEN_MASK) << LEN_SHIFT)
}

/// Tag an 8-byte-aligned address as a pointer of the given type.
pub fn tag_pointer(addr: *const u8, ty: HeapType) -> u64 {
    debug_assert!((addr as u64) & TAG_MASK == 0,
        "pointer {addr:p} is not 8-byte aligned");
    (addr as u64) | ty.pointer_tag()
}

/// Build a forwarding marker pointing at `new_addr`.
pub fn make_forward_marker(new_addr: *const u8) -> u64 {
    debug_assert!((new_addr as u64) & TAG_MASK == 0,
        "forwarding target {new_addr:p} is not 8-byte aligned");
    (new_addr as u64) | TAG_FORWARD
}

/// Decode the header at `header_cell` to get (type, length).
///
/// # Safety
/// `header_cell` must point at a valid header — typically guaranteed
/// by paged_gc having verified the start-bit bitmap.
pub unsafe fn decode_header(header_cell: *const u64) -> (Option<HeapType>, u32) {
    let raw = unsafe { *header_cell };
    let type_bits = ((raw >> TYPE_SHIFT) & TYPE_MASK) as u8;
    let length = ((raw >> LEN_SHIFT) & LEN_MASK) as u32;
    (HeapType::from_bits(type_bits), length)
}

// ─── The layout type ─────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, Default)]
pub struct Wf64Layout;

impl HeapLayout for Wf64Layout {
    /// Newly-allocated cells get 0 — WF64's nil.  Classifies as
    /// `Immediate` via the Fixnum branch; the GC skips it.
    const FILL_WORD: u64 = 0;

    #[inline(always)]
    fn classify(raw: u64) -> WordKind {
        let addr = (raw & PAYLOAD_MASK) as *const u8;
        match raw & TAG_MASK {
            TAG_FIXNUM | TAG_IMMEDIATE => WordKind::Immediate,
            TAG_CONS => WordKind::PointerCons(addr),
            TAG_FLOATVEC | TAG_REFVEC | TAG_STRING | TAG_BUILDER =>
                WordKind::PointerHeader(addr),
            TAG_FORWARD => WordKind::Forwarded(addr),
            // Everything else is treated as immediate so a stray
            // write here doesn't trick the GC into following a
            // non-pointer.  Safe default for the "I don't recognise
            // this" case.
            _ => WordKind::Immediate,
        }
    }

    #[inline(always)]
    fn make_forward(new_addr: *const u8) -> u64 {
        make_forward_marker(new_addr)
    }

    #[inline(always)]
    fn make_pointer(addr: *const u8, kind: PointerKind) -> u64 {
        debug_assert!((addr as u64) & TAG_MASK == 0);
        match kind {
            PointerKind::Cons => (addr as u64) | TAG_CONS,
            // Canonical header tag for the test-only `make_pointer`
            // path: FloatVec.  The production path uses
            // `rewrite_pointer_addr` which preserves the original
            // fine-grained tag (FloatVec / RefVec / String).
            PointerKind::Header => (addr as u64) | TAG_FLOATVEC,
        }
    }

    #[inline(always)]
    fn rewrite_pointer_addr(old_raw: u64, new_addr: *const u8) -> u64 {
        // Preserve the original 3-bit tag (which encodes the fine-
        // grained heap type) and substitute the new address.
        // The new address is guaranteed 8-byte aligned by paged_gc,
        // so its low 3 bits are already zero.
        debug_assert!((new_addr as u64) & TAG_MASK == 0);
        let tag_bits = old_raw & TAG_MASK;
        (new_addr as u64) | tag_bits
    }

    #[inline(always)]
    unsafe fn header_layout(header_cell: *const u64) -> ObjectLayout {
        let raw = unsafe { *header_cell };
        let type_bits = ((raw >> TYPE_SHIFT) & TYPE_MASK) as u8;
        let length = ((raw >> LEN_SHIFT) & LEN_MASK) as usize;
        match HeapType::from_bits(type_bits) {
            Some(HeapType::FloatVec) => ObjectLayout {
                total_cells: 1 + length,        // length is payload cells
                pointer_cells_start: 0,
                pointer_cells_end: 0,           // payload is opaque f64 bits
            },
            Some(HeapType::RefVec) => ObjectLayout {
                total_cells: 1 + length,        // length is payload cells
                pointer_cells_start: 1,         // skip header
                pointer_cells_end: 1 + length,  // all payload is pointer cells
            },
            Some(HeapType::String) => {
                // length is bytes; payload is ceil(length/8) cells.
                let payload_cells = (length + 7) / 8;
                ObjectLayout {
                    total_cells: 1 + payload_cells,
                    pointer_cells_start: 0,
                    pointer_cells_end: 0,        // opaque UTF-8 bytes
                }
            }
            Some(HeapType::MutStringBuilder) => {
                // 2-cell header: standard word, then capacity-in-bytes
                // as a raw u64.  Payload extent is governed by
                // capacity (not the current length).
                let capacity_bytes = unsafe { *header_cell.add(1) } as usize;
                let payload_cells = (capacity_bytes + 7) / 8;
                ObjectLayout {
                    total_cells: 2 + payload_cells,
                    pointer_cells_start: 0,
                    pointer_cells_end: 0,        // opaque UTF-8 bytes
                }
            }
            None => {
                // Unknown type bits — this is a corruption case.
                // Return a single-cell opaque layout so we don't
                // crash; the next mark/sweep cycle will eventually
                // catch the inconsistency via the start-bit bitmap.
                ObjectLayout {
                    total_cells: 1,
                    pointer_cells_start: 0,
                    pointer_cells_end: 0,
                }
            }
        }
    }
}

// ─── Unit tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Use a known 8-byte-aligned address for round-trip tests.
    // We don't actually dereference it — just check tag operations.
    fn fake_addr(offset: u64) -> *const u8 {
        // Multiples of 8 starting from a non-zero base so the result
        // is never confused with nil (raw 0).
        ((0x1000_0000 + (offset * 8)) as usize) as *const u8
    }

    // ── classify ──────────────────────────────────────────────────────

    #[test]
    fn classify_nil_is_immediate() {
        assert!(matches!(Wf64Layout::classify(0), WordKind::Immediate));
    }

    #[test]
    fn classify_fill_word_is_immediate() {
        // The contract: FILL_WORD must classify as Immediate.
        assert!(matches!(
            Wf64Layout::classify(Wf64Layout::FILL_WORD),
            WordKind::Immediate
        ));
    }

    #[test]
    fn classify_floatvec_pointer() {
        let p = fake_addr(1);
        let raw = tag_pointer(p, HeapType::FloatVec);
        match Wf64Layout::classify(raw) {
            WordKind::PointerHeader(a) => assert_eq!(a, p),
            other => panic!("expected PointerHeader, got {other:?}"),
        }
    }

    #[test]
    fn classify_refvec_pointer() {
        let p = fake_addr(2);
        let raw = tag_pointer(p, HeapType::RefVec);
        match Wf64Layout::classify(raw) {
            WordKind::PointerHeader(a) => assert_eq!(a, p),
            other => panic!("expected PointerHeader, got {other:?}"),
        }
    }

    #[test]
    fn classify_string_pointer() {
        let p = fake_addr(3);
        let raw = tag_pointer(p, HeapType::String);
        match Wf64Layout::classify(raw) {
            WordKind::PointerHeader(a) => assert_eq!(a, p),
            other => panic!("expected PointerHeader, got {other:?}"),
        }
    }

    #[test]
    fn classify_cons_pointer() {
        let p = fake_addr(4);
        let raw = (p as u64) | TAG_CONS;
        match Wf64Layout::classify(raw) {
            WordKind::PointerCons(a) => assert_eq!(a, p),
            other => panic!("expected PointerCons, got {other:?}"),
        }
    }

    #[test]
    fn classify_forward_marker() {
        let p = fake_addr(5);
        let raw = make_forward_marker(p);
        match Wf64Layout::classify(raw) {
            WordKind::Forwarded(a) => assert_eq!(a, p),
            other => panic!("expected Forwarded, got {other:?}"),
        }
    }

    #[test]
    fn classify_builder_tag_is_pointer_header() {
        // Tag 0b101 (5) is TAG_BUILDER — must classify as a pointer
        // so the GC follows it into the 2-cell-header MutStringBuilder.
        // (Was Immediate while 101 was reserved; flipped in V2s C1.)
        match Wf64Layout::classify(0x1000_0000_0000_0005) {
            WordKind::PointerHeader(_) => (),
            other => panic!("TAG_BUILDER should classify as PointerHeader, got {other:?}"),
        }
    }

    #[test]
    fn classify_immediate_tag_six() {
        // Tag 0b110 is reserved for future immediates (e.g. true,
        // false, char).  Must classify as Immediate today.
        assert!(matches!(
            Wf64Layout::classify(0x1000_0000_0000_0006),
            WordKind::Immediate
        ));
    }

    // ── make_forward / make_pointer round-trips ──────────────────────

    #[test]
    fn make_forward_classifies_correctly_at_multiple_alignments() {
        // Sweep multiple 8-byte-aligned offsets to catch tag-encoding
        // bugs that only show up at specific bit patterns.
        for i in 0..8 {
            let p = fake_addr(i + 100);
            let raw = Wf64Layout::make_forward(p);
            match Wf64Layout::classify(raw) {
                WordKind::Forwarded(a) => assert_eq!(a, p,
                    "forward roundtrip failed at offset {i}"),
                other => panic!("offset {i}: expected Forwarded, got {other:?}"),
            }
        }
    }

    #[test]
    fn make_pointer_roundtrips_both_kinds() {
        let p = fake_addr(7);
        for kind in [PointerKind::Cons, PointerKind::Header] {
            let raw = Wf64Layout::make_pointer(p, kind);
            let classified = Wf64Layout::classify(raw);
            match (kind, classified) {
                (PointerKind::Cons, WordKind::PointerCons(a)) => {
                    assert_eq!(a, p, "Cons roundtrip lost address")
                }
                (PointerKind::Header, WordKind::PointerHeader(a)) => {
                    assert_eq!(a, p, "Header roundtrip lost address")
                }
                (k, c) => panic!("kind {k:?} classified as {c:?}"),
            }
        }
    }

    // ── rewrite_pointer_addr ─────────────────────────────────────────

    #[test]
    fn rewrite_preserves_each_pointer_tag() {
        let old_addr = fake_addr(8);
        let new_addr = fake_addr(9);
        for tag in [TAG_CONS, TAG_FLOATVEC, TAG_REFVEC, TAG_STRING, TAG_BUILDER] {
            let old_raw = (old_addr as u64) | tag;
            let new_raw = Wf64Layout::rewrite_pointer_addr(old_raw, new_addr);
            assert_eq!(new_raw & TAG_MASK, tag,
                "rewrite lost tag {tag:#b}");
            assert_eq!(new_raw & PAYLOAD_MASK, new_addr as u64,
                "rewrite set wrong address for tag {tag:#b}");
        }
    }

    // ── header_layout ────────────────────────────────────────────────

    #[test]
    fn header_layout_floatvec_5_cells() {
        let header = make_header(HeapType::FloatVec, 5);
        let cell = &header as *const u64;
        let layout = unsafe { Wf64Layout::header_layout(cell) };
        assert_eq!(layout.total_cells, 6, "1 header + 5 payload");
        assert_eq!(layout.pointer_cells_start, 0);
        assert_eq!(layout.pointer_cells_end, 0,
            "FloatVec payload is opaque f64 bits");
    }

    #[test]
    fn header_layout_floatvec_zero_length() {
        let header = make_header(HeapType::FloatVec, 0);
        let cell = &header as *const u64;
        let layout = unsafe { Wf64Layout::header_layout(cell) };
        assert_eq!(layout.total_cells, 1, "header-only object");
        assert_eq!(layout.pointer_cell_count(), 0);
    }

    #[test]
    fn header_layout_refvec_3_cells() {
        let header = make_header(HeapType::RefVec, 3);
        let cell = &header as *const u64;
        let layout = unsafe { Wf64Layout::header_layout(cell) };
        assert_eq!(layout.total_cells, 4, "1 header + 3 payload");
        assert_eq!(layout.pointer_cells_start, 1,
            "RefVec scans from offset 1 (skip header)");
        assert_eq!(layout.pointer_cells_end, 4,
            "RefVec scans all 3 payload cells");
    }

    #[test]
    fn header_layout_string_13_bytes() {
        // 13 bytes packs into 2 cells (ceil(13/8) = 2).  Total = 3.
        let header = make_header(HeapType::String, 13);
        let cell = &header as *const u64;
        let layout = unsafe { Wf64Layout::header_layout(cell) };
        assert_eq!(layout.total_cells, 3, "1 header + 2 payload (for 13 bytes)");
        assert_eq!(layout.pointer_cells_start, 0);
        assert_eq!(layout.pointer_cells_end, 0,
            "String payload is opaque UTF-8 bytes");
    }

    #[test]
    fn header_layout_string_exact_8_bytes() {
        // 8 bytes packs into exactly 1 cell.
        let header = make_header(HeapType::String, 8);
        let cell = &header as *const u64;
        let layout = unsafe { Wf64Layout::header_layout(cell) };
        assert_eq!(layout.total_cells, 2, "1 header + 1 payload (for 8 bytes)");
    }

    #[test]
    fn header_layout_string_zero_bytes() {
        // 0 bytes packs into 0 cells.  Just the header.
        let header = make_header(HeapType::String, 0);
        let cell = &header as *const u64;
        let layout = unsafe { Wf64Layout::header_layout(cell) };
        assert_eq!(layout.total_cells, 1, "empty string is header-only");
    }

    #[test]
    fn header_layout_unknown_type_safe_fallback() {
        // Unknown type bits (e.g. value 31 — never used).  We return
        // a 1-cell opaque layout so a corrupt heap doesn't crash the
        // scanner — the start-bit machinery should catch the actual
        // inconsistency.
        let header: u64 = 31; // type = 31, length = 0
        let cell = &header as *const u64;
        let layout = unsafe { Wf64Layout::header_layout(cell) };
        assert_eq!(layout.total_cells, 1);
        assert_eq!(layout.pointer_cell_count(), 0);
    }

    // ── header construction round-trips ──────────────────────────────

    #[test]
    fn decode_header_roundtrip() {
        for ty in [HeapType::FloatVec, HeapType::RefVec, HeapType::String] {
            for length in [0u32, 1, 7, 8, 100, 65535, MAX_LENGTH] {
                let raw = make_header(ty, length);
                let cell = &raw as *const u64;
                let (decoded_ty, decoded_len) = unsafe { decode_header(cell) };
                assert_eq!(decoded_ty, Some(ty),
                    "type decode failed for {ty:?} len={length}");
                assert_eq!(decoded_len, length,
                    "length decode failed for {ty:?} len={length}");
            }
        }
    }

    #[test]
    fn header_length_max_value() {
        // The 24-bit length field's maximum value must round-trip.
        let raw = make_header(HeapType::FloatVec, MAX_LENGTH);
        let cell = &raw as *const u64;
        let (_ty, len) = unsafe { decode_header(cell) };
        assert_eq!(len, MAX_LENGTH);
    }

    // ── tag scheme sanity ────────────────────────────────────────────

    #[test]
    fn pointer_tag_matches_heap_type() {
        // By convention the pointer tag and the header type field share
        // bit patterns for the four V1a/V2s heap types.  Verify so a
        // future refactor doesn't quietly break the invariant.
        assert_eq!(HeapType::FloatVec.pointer_tag(),         TAG_FLOATVEC);
        assert_eq!(HeapType::RefVec.pointer_tag(),           TAG_REFVEC);
        assert_eq!(HeapType::String.pointer_tag(),           TAG_STRING);
        assert_eq!(HeapType::MutStringBuilder.pointer_tag(), TAG_BUILDER);
        assert_eq!(HeapType::FloatVec         as u64, TAG_FLOATVEC);
        assert_eq!(HeapType::RefVec           as u64, TAG_REFVEC);
        assert_eq!(HeapType::String           as u64, TAG_STRING);
        assert_eq!(HeapType::MutStringBuilder as u64, TAG_BUILDER);
    }

    #[test]
    fn header_layout_builder_two_cell() {
        // 2-cell header: word 0 = type/length, word 1 = capacity.
        // total_cells uses capacity, not length.
        let header_word = make_header(HeapType::MutStringBuilder, 5); // length=5
        let layout_buf: [u64; 2] = [header_word, 17];                 // capacity=17
        let cell = &layout_buf[0] as *const u64;
        let layout = unsafe { Wf64Layout::header_layout(cell) };
        // capacity 17 bytes → ceil(17/8) = 3 payload cells.
        // total = 2 (header) + 3 = 5.
        assert_eq!(layout.total_cells, 5);
        assert_eq!(layout.pointer_cell_count(), 0);
    }

    #[test]
    fn all_tag_values_distinct() {
        let tags = [
            TAG_FIXNUM, TAG_CONS, TAG_FLOATVEC, TAG_REFVEC,
            TAG_STRING, TAG_BUILDER, TAG_IMMEDIATE, TAG_FORWARD,
        ];
        for (i, &a) in tags.iter().enumerate() {
            for &b in &tags[i + 1..] {
                assert_ne!(a, b, "duplicate tag value {a:#b}");
            }
        }
    }
}
