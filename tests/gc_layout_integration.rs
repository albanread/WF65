//! V1a integration tests — drive `PageHeap<Wf64Layout>` through real
//! allocate / mutate / collect cycles.
//!
//! The centrepiece is `heapptr_region_walk_simulation` — the
//! V1a → V1b checkpoint per `docs/gc_design.md`.  Until that test
//! passes, no kernel work begins.
//!
//! Modelled on `E:\NewGC\crates\newgc-core\tests\synthetic.rs` but
//! using Wf64Layout's 3-bit tag scheme and HeapType variants.

use newgc_core::{Generation, HeapLayout, PageHeap};
use wf64::gc::{
    HeapType, Wf64Layout,
    PAYLOAD_MASK, TAG_MASK, TAG_FLOATVEC, TAG_REFVEC, TAG_STRING, TAG_FORWARD,
    make_header, tag_pointer,
};

// ─── Heap shorthand ──────────────────────────────────────────────────

type WfHeap = PageHeap<Wf64Layout>;

/// 8-page heap = 512 KB.  Enough to exercise allocation paths,
/// small enough that near-OOM is easy to provoke.
fn small_heap() -> WfHeap {
    WfHeap::with_reservation(8 * 64 * 1024)
}

/// 32-page heap = 2 MB.  For tests that build larger graphs.
fn medium_heap() -> WfHeap {
    WfHeap::with_reservation(32 * 64 * 1024)
}

// ─── Object constructors ─────────────────────────────────────────────

/// Allocate a `FloatVec` with `n` opaque f64 cells in `gen`, fill it
/// with caller-supplied bits, return the tagged pointer.
fn alloc_floatvec(h: &mut WfHeap, gen: Generation, fill_bits: &[u64]) -> u64 {
    let n = fill_bits.len();
    assert!(n <= u32::MAX as usize);
    let p = h
        .try_alloc_boxed_in(gen, 1 + n)
        .expect("floatvec alloc");
    unsafe {
        *p.as_ptr() = make_header(HeapType::FloatVec, n as u32);
        for (i, &bits) in fill_bits.iter().enumerate() {
            *p.as_ptr().add(1 + i) = bits;
        }
    }
    tag_pointer(p.as_ptr() as *const u8, HeapType::FloatVec)
}

/// Allocate a `RefVec` of `n` pointer cells, all initialised to nil
/// (0). Caller can patch slots via `set_refvec_slot` after.
fn alloc_refvec(h: &mut WfHeap, gen: Generation, n: u32) -> u64 {
    let p = h
        .try_alloc_boxed_in(gen, 1 + n as usize)
        .expect("refvec alloc");
    unsafe {
        *p.as_ptr() = make_header(HeapType::RefVec, n);
        // paged_gc fills cells with FILL_WORD (0) on page acquisition,
        // so payload cells are already nil — but be explicit.
        for i in 1..=n as usize {
            *p.as_ptr().add(i) = 0;
        }
    }
    tag_pointer(p.as_ptr() as *const u8, HeapType::RefVec)
}

/// Allocate a `String` of `n` bytes in `gen`, copy `bytes` in,
/// return the tagged pointer. `n = bytes.len()` for clarity.
fn alloc_string(h: &mut WfHeap, gen: Generation, bytes: &[u8]) -> u64 {
    let n = bytes.len();
    assert!(n <= u32::MAX as usize);
    let payload_cells = (n + 7) / 8;
    let p = h
        .try_alloc_boxed_in(gen, 1 + payload_cells)
        .expect("string alloc");
    unsafe {
        *p.as_ptr() = make_header(HeapType::String, n as u32);
        let payload_ptr = (p.as_ptr() as *mut u8).add(8);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), payload_ptr, n);
    }
    tag_pointer(p.as_ptr() as *const u8, HeapType::String)
}

// ─── Object access ───────────────────────────────────────────────────

/// Read payload cell `idx` (1-based: cell 1 is the first payload
/// cell, NOT the header) of an object pointed to by `tagged_ptr`.
unsafe fn cell_at(tagged_ptr: u64, idx: usize) -> u64 {
    let base = (tagged_ptr & PAYLOAD_MASK) as *const u64;
    unsafe { *base.add(idx) }
}

/// Write payload cell `idx` of a RefVec.
unsafe fn set_refvec_slot(tagged_ptr: u64, idx: usize, value: u64) {
    let base = (tagged_ptr & PAYLOAD_MASK) as *mut u64;
    unsafe { *base.add(idx) = value }
}

/// Read raw bytes from a String object.  Length comes from the
/// header.
unsafe fn string_bytes(tagged_ptr: u64) -> Vec<u8> {
    let base = (tagged_ptr & PAYLOAD_MASK) as *const u64;
    let hdr = unsafe { *base };
    let len = ((hdr >> wf64::gc::LEN_SHIFT) & wf64::gc::LEN_MASK) as usize;
    let payload_ptr = unsafe { (base as *const u8).add(8) };
    let mut v = Vec::with_capacity(len);
    for i in 0..len {
        v.push(unsafe { *payload_ptr.add(i) });
    }
    v
}

// ─── Simulated HEAPPTR region ────────────────────────────────────────

/// The V1a → V1b checkpoint shape: a region of cells that the GC
/// scans precisely via `evac.visit_cell`.
///
/// This mirrors what the kernel will eventually do once V1b lands:
/// user_HEAPPTR_REGION + user_HEAPPTR_NEXT define a range of cells
/// that the runtime walks at every collection.  Here we build the
/// same range in a Rust `Vec<u64>` and verify the GC handles it
/// correctly — survivors mark, forwarded slots get rewritten,
/// unreachable objects disappear.
struct HeapPtrRegion {
    slots: Vec<u64>,
}

impl HeapPtrRegion {
    fn new(capacity: usize) -> Self {
        HeapPtrRegion { slots: vec![0; capacity] }
    }

    /// Store a tagged pointer in slot `i`.  Returns the slot's
    /// stable address (the "handle" in WF64 parlance).
    fn store(&mut self, i: usize, tagged_ptr: u64) -> *mut u64 {
        self.slots[i] = tagged_ptr;
        unsafe { self.slots.as_mut_ptr().add(i) }
    }

    /// Run the precise root walk: visit every cell in the region.
    /// This is exactly what the kernel will do in V1b.
    fn visit_all(&mut self, heap: &mut WfHeap, kind: CollectKind) {
        let base = self.slots.as_mut_ptr();
        let n = self.slots.len();
        let walk = |evac: &mut newgc_core::page_heap::evac::PageEvacuator<'_, Wf64Layout>| {
            for i in 0..n {
                unsafe { evac.visit_cell(base.add(i)); }
            }
        };
        match kind {
            CollectKind::Minor => { heap.collect_minor(walk); }
            CollectKind::Major => { heap.collect_major(walk); }
        }
    }
}

#[derive(Copy, Clone)]
enum CollectKind { Minor, Major }

// ─── Tests — basic allocation & classify path ────────────────────────

#[test]
fn allocate_floatvec_then_walk_sees_it() {
    let mut h = small_heap();
    let mut region = HeapPtrRegion::new(8);

    let v = alloc_floatvec(&mut h, Generation::G0, &[
        f64::to_bits(1.0),
        f64::to_bits(2.0),
        f64::to_bits(3.0),
    ]);
    region.store(0, v);

    // The tagged pointer's tag must be FloatVec.
    assert_eq!(v & TAG_MASK, TAG_FLOATVEC);

    // Run a major GC. Object is rooted via region slot 0; must survive.
    region.visit_all(&mut h, CollectKind::Major);

    // After GC the slot may have been rewritten if the object moved.
    // Re-fetch and verify payload still reads the same bytes.
    let v_after = region.slots[0];
    assert_eq!(v_after & TAG_MASK, TAG_FLOATVEC,
        "tag preserved across rewrite");
    unsafe {
        assert_eq!(cell_at(v_after, 1), f64::to_bits(1.0));
        assert_eq!(cell_at(v_after, 2), f64::to_bits(2.0));
        assert_eq!(cell_at(v_after, 3), f64::to_bits(3.0));
    }
}

#[test]
fn allocate_refvec_then_walk_follows_payload() {
    let mut h = small_heap();
    let mut region = HeapPtrRegion::new(8);

    // Build a RefVec of length 2, point its cells at two FloatVecs.
    let inner1 = alloc_floatvec(&mut h, Generation::G0, &[f64::to_bits(10.0)]);
    let inner2 = alloc_floatvec(&mut h, Generation::G0, &[f64::to_bits(20.0)]);
    let outer = alloc_refvec(&mut h, Generation::G0, 2);
    unsafe {
        set_refvec_slot(outer, 1, inner1);
        set_refvec_slot(outer, 2, inner2);
    }
    region.store(0, outer);

    // The outer RefVec is the only direct root.  inner1/inner2 are
    // reachable only via the RefVec's payload — they must survive
    // because the GC follows pointer-typed payload cells.
    region.visit_all(&mut h, CollectKind::Major);

    let outer_after = region.slots[0];
    assert_eq!(outer_after & TAG_MASK, TAG_REFVEC);

    unsafe {
        let inner1_after = cell_at(outer_after, 1);
        let inner2_after = cell_at(outer_after, 2);
        assert_eq!(inner1_after & TAG_MASK, TAG_FLOATVEC);
        assert_eq!(inner2_after & TAG_MASK, TAG_FLOATVEC);
        // Read each inner vec's data through the (possibly rewritten)
        // pointer in the RefVec.
        assert_eq!(cell_at(inner1_after, 1), f64::to_bits(10.0));
        assert_eq!(cell_at(inner2_after, 1), f64::to_bits(20.0));
    }
}

#[test]
fn allocate_string_survives_collection() {
    let mut h = small_heap();
    let mut region = HeapPtrRegion::new(4);

    let msg = b"Hello, WF64 GC!";
    let s = alloc_string(&mut h, Generation::G0, msg);
    region.store(0, s);

    assert_eq!(s & TAG_MASK, TAG_STRING);

    region.visit_all(&mut h, CollectKind::Major);

    let s_after = region.slots[0];
    assert_eq!(s_after & TAG_MASK, TAG_STRING);
    let bytes_after = unsafe { string_bytes(s_after) };
    assert_eq!(bytes_after, msg);
}

#[test]
fn empty_region_collects_cleanly() {
    // Region of all-nil slots — the GC walks but finds nothing to
    // root.  Verifies the nil/0 classification path doesn't trip.
    let mut h = small_heap();
    let mut region = HeapPtrRegion::new(16);

    // Allocate some objects but DON'T root them.  They should be
    // reclaimed.
    let _orphan = alloc_floatvec(&mut h, Generation::G0, &[f64::to_bits(42.0)]);

    region.visit_all(&mut h, CollectKind::Major);

    // Region intact (all nil).
    for slot in &region.slots {
        assert_eq!(*slot, 0);
    }
}

#[test]
fn unrooted_objects_get_reclaimed() {
    let mut h = small_heap();

    // Allocate a bunch of FloatVecs without rooting them.
    for i in 0..100 {
        let _ = alloc_floatvec(&mut h, Generation::G0, &[f64::to_bits(i as f64)]);
    }
    let pages_before = h.count_pages_in_gen(Generation::G0);
    assert!(pages_before > 0, "should have allocated SOMETHING");

    // Empty root walk → everything is unreachable.
    let mut region = HeapPtrRegion::new(0);
    region.visit_all(&mut h, CollectKind::Major);

    // G0 should be empty after collection.
    let pages_after = h.count_pages_in_gen(Generation::G0);
    assert_eq!(pages_after, 0, "unrooted objects should have been reclaimed");
}

#[test]
fn rooted_objects_persist_across_repeated_collections() {
    let mut h = small_heap();
    let mut region = HeapPtrRegion::new(4);

    let v = alloc_floatvec(&mut h, Generation::G0, &[
        f64::to_bits(1.5),
        f64::to_bits(2.5),
        f64::to_bits(3.5),
        f64::to_bits(4.5),
    ]);
    region.store(0, v);

    // Run multiple cycles. Object should keep surviving and its
    // payload should remain readable through whatever rewriting
    // the GC does.
    for cycle in 0..5 {
        region.visit_all(&mut h, CollectKind::Major);
        let v_after = region.slots[0];
        assert_eq!(v_after & TAG_MASK, TAG_FLOATVEC,
            "cycle {cycle}: tag must remain FloatVec");
        unsafe {
            assert_eq!(cell_at(v_after, 1), f64::to_bits(1.5),
                "cycle {cycle}: payload[0] corrupted");
            assert_eq!(cell_at(v_after, 4), f64::to_bits(4.5),
                "cycle {cycle}: payload[3] corrupted");
        }
    }
}

// ─── The V1a → V1b checkpoint test ───────────────────────────────────

#[test]
fn heapptr_region_walk_simulation() {
    // This is the test the design doc names as the checkpoint before
    // any kernel work begins.  It validates the interaction between
    // Wf64Layout::classify, Wf64Layout::rewrite_pointer_addr, and
    // paged_gc's closure-based safepoint API — using the exact
    // shape of root walk the kernel will eventually do.
    //
    // Scenario: a region of 32 HEAPPTR slots, with:
    //   - some slots nil (= 0) — must classify as Immediate, GC skips
    //   - some slots holding FloatVec pointers — must follow
    //   - some slots holding RefVec pointers whose payload references
    //     other slots' objects — verify multi-step reachability
    //   - some slots holding String pointers — must follow but not
    //     scan payload
    //   - some objects allocated but NOT rooted — must be reclaimed
    //
    // After a major collection:
    //   - every rooted object is still reachable from its slot
    //   - payload bytes are intact
    //   - unrooted objects are gone (page count tells us)
    //   - all classify paths exercised cleanly

    let mut h = medium_heap();
    let mut region = HeapPtrRegion::new(32);

    // Slot 0: a FloatVec of 4 doubles.
    let fv = alloc_floatvec(&mut h, Generation::G0, &[
        f64::to_bits(0.1), f64::to_bits(0.2),
        f64::to_bits(0.3), f64::to_bits(0.4),
    ]);
    region.store(0, fv);

    // Slot 1: nil.  Stays nil.
    region.slots[1] = 0;

    // Slot 2: a RefVec of 3 cells.  Cells point to inner FloatVecs
    // that have NO other reference — they must survive via the
    // RefVec, validating the inside-object pointer scan.
    let rv = alloc_refvec(&mut h, Generation::G0, 3);
    let inner_a = alloc_floatvec(&mut h, Generation::G0, &[f64::to_bits(100.0)]);
    let inner_b = alloc_floatvec(&mut h, Generation::G0, &[f64::to_bits(200.0)]);
    let inner_c = alloc_floatvec(&mut h, Generation::G0, &[f64::to_bits(300.0)]);
    unsafe {
        set_refvec_slot(rv, 1, inner_a);
        set_refvec_slot(rv, 2, inner_b);
        set_refvec_slot(rv, 3, inner_c);
    }
    region.store(2, rv);

    // Slot 3: a String.
    let s1 = alloc_string(&mut h, Generation::G0, b"alpha");
    region.store(3, s1);

    // Slots 4..10: more nils.  Sweep the classify Immediate path.
    for i in 4..10 {
        region.slots[i] = 0;
    }

    // Slot 10: another FloatVec.
    let fv2 = alloc_floatvec(&mut h, Generation::G0, &[
        f64::to_bits(-1.0), f64::to_bits(-2.0),
    ]);
    region.store(10, fv2);

    // Slot 11: another String, longer this time (forces multi-cell
    // payload, exercises the byte→cell length conversion).
    let s2 = alloc_string(
        &mut h, Generation::G0,
        b"The quick brown fox jumps over the lazy dog.",
    );
    region.store(11, s2);

    // Now allocate a LOT of ORPHANS so they spill across multiple
    // pages — that way the post-collect page count is observably
    // smaller and we can sanity-check reclamation.  Each FloatVec
    // here is ~16 cells; 1000 of them = 16K cells = 2 pages worth.
    for i in 0..1000 {
        let _ = alloc_floatvec(&mut h, Generation::G0, &[
            f64::to_bits(i as f64),
            f64::to_bits((i + 1) as f64),
            f64::to_bits((i + 2) as f64),
            f64::to_bits((i + 3) as f64),
            f64::to_bits((i + 4) as f64),
            f64::to_bits((i + 5) as f64),
            f64::to_bits((i + 6) as f64),
            f64::to_bits((i + 7) as f64),
            f64::to_bits((i + 8) as f64),
            f64::to_bits((i + 9) as f64),
            f64::to_bits((i + 10) as f64),
            f64::to_bits((i + 11) as f64),
            f64::to_bits((i + 12) as f64),
            f64::to_bits((i + 13) as f64),
            f64::to_bits((i + 14) as f64),
        ]);
    }

    let pages_pre = h.count_pages_in_gen(Generation::G0);
    eprintln!("[checkpoint] G0 pages before collect: {pages_pre}");
    assert!(pages_pre > 1, "test setup: orphans should fill multiple pages");

    // The big moment: run the precise root walk and a major GC.
    region.visit_all(&mut h, CollectKind::Major);

    // Verify each rooted object's payload is intact through the
    // (possibly rewritten) slot pointer.

    let fv_after = region.slots[0];
    assert_eq!(fv_after & TAG_MASK, TAG_FLOATVEC,
        "slot 0 must still be a FloatVec");
    unsafe {
        assert_eq!(cell_at(fv_after, 1), f64::to_bits(0.1));
        assert_eq!(cell_at(fv_after, 4), f64::to_bits(0.4));
    }

    assert_eq!(region.slots[1], 0, "nil slot must stay nil");

    let rv_after = region.slots[2];
    assert_eq!(rv_after & TAG_MASK, TAG_REFVEC,
        "slot 2 must still be a RefVec");
    unsafe {
        let ia = cell_at(rv_after, 1);
        let ib = cell_at(rv_after, 2);
        let ic = cell_at(rv_after, 3);
        assert_eq!(ia & TAG_MASK, TAG_FLOATVEC);
        assert_eq!(ib & TAG_MASK, TAG_FLOATVEC);
        assert_eq!(ic & TAG_MASK, TAG_FLOATVEC);
        // Read the inner objects' payloads through the (rewritten)
        // pointers in the RefVec.
        assert_eq!(cell_at(ia, 1), f64::to_bits(100.0),
            "inner_a payload preserved through RefVec chain");
        assert_eq!(cell_at(ib, 1), f64::to_bits(200.0));
        assert_eq!(cell_at(ic, 1), f64::to_bits(300.0));
    }

    let s1_after = region.slots[3];
    assert_eq!(s1_after & TAG_MASK, TAG_STRING);
    assert_eq!(unsafe { string_bytes(s1_after) }, b"alpha");

    let fv2_after = region.slots[10];
    assert_eq!(fv2_after & TAG_MASK, TAG_FLOATVEC);
    unsafe {
        assert_eq!(cell_at(fv2_after, 1), f64::to_bits(-1.0));
        assert_eq!(cell_at(fv2_after, 2), f64::to_bits(-2.0));
    }

    let s2_after = region.slots[11];
    assert_eq!(s2_after & TAG_MASK, TAG_STRING);
    assert_eq!(
        unsafe { string_bytes(s2_after) },
        b"The quick brown fox jumps over the lazy dog.",
    );

    // Unrooted-object page-count check: G0 should be smaller than
    // before, ideally much smaller.  We don't assert "= 0" because
    // the rooted objects may have moved within G0 or been promoted
    // (major collect can promote survivors to G1/Tenured).
    let pages_post_g0 = h.count_pages_in_gen(Generation::G0);
    eprintln!("[checkpoint] G0 pages after collect: {pages_post_g0}");
    assert!(pages_post_g0 < pages_pre,
        "G0 should shrink after orphans collected: pre={pages_pre}, post={pages_post_g0}");
}

// ─── Sanity: forwarding-marker mechanics ─────────────────────────────

#[test]
fn forwarding_markers_classify_at_multiple_alignments() {
    // Sweep multiple 8-byte-aligned addresses and confirm forward
    // marker round-trips cleanly.  This is the per-LispLayout test
    // pattern that catches tag-encoding bugs at specific bit
    // patterns.
    use newgc_core::traits::WordKind;
    for offset in 0..8 {
        let addr = (0x4000_0000 + offset * 8) as *const u8;
        let fw = Wf64Layout::make_forward(addr);
        match Wf64Layout::classify(fw) {
            WordKind::Forwarded(a) => assert_eq!(a, addr,
                "forward roundtrip failed at offset {offset}"),
            other => panic!("offset {offset}: expected Forwarded, got {other:?}"),
        }
        assert_eq!(fw & TAG_MASK, TAG_FORWARD);
    }
}

// ─── Smoke: minor collection works the same shape ────────────────────

#[test]
fn minor_collection_promotes_survivors() {
    let mut h = small_heap();
    let mut region = HeapPtrRegion::new(4);

    let v = alloc_floatvec(&mut h, Generation::G0, &[
        f64::to_bits(7.0), f64::to_bits(8.0),
    ]);
    region.store(0, v);

    // After several minor cycles, the object should still be
    // reachable.  paged_gc's promotion threshold may move it from
    // G0 to G1 to Tenured.  We don't care which — only that the
    // slot still points at a live FloatVec with the right contents.
    for _ in 0..6 {
        region.visit_all(&mut h, CollectKind::Minor);
    }

    let v_after = region.slots[0];
    assert_eq!(v_after & TAG_MASK, TAG_FLOATVEC);
    unsafe {
        assert_eq!(cell_at(v_after, 1), f64::to_bits(7.0));
        assert_eq!(cell_at(v_after, 2), f64::to_bits(8.0));
    }
}
