//! Garbage collection — Wf64Layout and the HEAPPTR runtime.
//!
//! Designed in `docs/gc_design.md`.  This module is being grown
//! through milestones V1a (Wf64Layout against paged_gc's own tests,
//! Rust-only), V1b (kernel hooks), V1c (HEAPPTR defining word and
//! forget semantics).  See the design doc for current status.
//!
//! V1a status: layout binding implemented, unit tests cover the
//! HeapLayout trait surface.  Next step: run paged_gc's existing
//! test infrastructure against `PageHeap<Wf64Layout>`, then add
//! WF64-flavoured synthetic stress tests including the HEAPPTR-
//! region-walk simulation that is the V1a → V1b checkpoint.

pub mod layout;
pub mod heap;

pub use heap::{
    reset_wf_heap, with_wf_heap,
    alloc_floatvec, alloc_refvec, alloc_string, alloc_builder,
    collect_major, collect_minor, collect_auto, collect_full,
    gc_cycle_count, should_collect,
    set_gc_budget_min_bytes,
};

pub use layout::{
    HeapType, Wf64Layout,
    TAG_BITS, TAG_MASK, PAYLOAD_MASK,
    TAG_FIXNUM, TAG_CONS, TAG_FLOATVEC, TAG_REFVEC, TAG_STRING, TAG_BUILDER,
    TAG_IMMEDIATE, TAG_FORWARD,
    TYPE_SHIFT, TYPE_BITS, TYPE_MASK,
    LEN_SHIFT, LEN_BITS, LEN_MASK,
    GC_SHIFT, GC_BITS, GC_MASK,
    MAX_LENGTH,
    make_header, tag_pointer, make_forward_marker, decode_header,
};
