//! `WfHeap` — the live GC heap, wrapping `PageHeap<Wf64Layout>`.
//!
//! Sits in a thread-local because the Forth runtime functions
//! (`rt_vec_alloc_floats`, `rt_gc_collect`, …) are called from
//! JIT'd code via the `@extern` mechanism, which has no `&mut
//! Session` to thread through.  Same pattern the rest of WF64 uses
//! for compile-only state (LET_JITS, CODE_JITS, …).
//!
//! V1b status: lazily initialised on first allocation.  64 MB
//! reservation by default — large enough to play with multi-MB
//! vectors without thinking about it, small enough that test runs
//! don't reserve embarrassing amounts of address space.

use std::cell::RefCell;

use newgc_core::{Generation, PageHeap};
use newgc_core::page_heap::PAGE_SIZE_CELLS;

use super::layout::{HeapType, Wf64Layout, make_header, tag_pointer};

/// Allocate `n_cells` (header + payload) in `gen`, routing to the
/// small-object path or the large-object path as appropriate.
/// paged_gc's `try_alloc_boxed_in` caps at one page (8192 cells);
/// anything larger needs `try_alloc_large` which finds a run of
/// contiguous free pages.
fn try_alloc_in(
    heap: &mut PageHeap<Wf64Layout>,
    gen: Generation,
    n_cells: usize,
) -> Option<std::ptr::NonNull<u64>> {
    if n_cells <= PAGE_SIZE_CELLS {
        heap.try_alloc_boxed_in(gen, n_cells)
    } else {
        heap.try_alloc_large(n_cells, gen)
    }
}

/// Default heap reservation: 64 MB.  paged_gc reserves address
/// space lazily via VirtualAlloc; the physical pages don't get
/// committed until they're actually touched.
const DEFAULT_HEAP_BYTES: usize = 64 * 1024 * 1024;

thread_local! {
    /// The live GC heap.  Initialised on first use; cleared on
    /// session reset.
    static WF_HEAP: RefCell<Option<PageHeap<Wf64Layout>>> = const { RefCell::new(None) };

    /// Monotonically-increasing counter, incremented by one on every
    /// `collect_major` / `collect_minor` call.  Exposed to Forth via
    /// the `gc-cycle` primitive (see V2 in docs/gc_design.md): a
    /// long-running word that holds a raw heap pointer can snapshot
    /// the counter before each potentially-allocating call and
    /// refresh from `@heapptr` whenever it changes.  Reset to 0 by
    /// `reset_wf_heap` so each harness test starts from a known
    /// baseline.
    static WF_GC_CYCLES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Reset the GC heap, dropping all allocations.  Called by session
/// reset between harness tests so each test starts with a fresh
/// (empty) heap.  In production this would never be called; the
/// heap lives for the session.
pub fn reset_wf_heap() {
    WF_HEAP.with(|cell| {
        *cell.borrow_mut() = None;
    });
    WF_GC_CYCLES.with(|c| c.set(0));
}

/// Current value of the GC cycle counter.  Each successful
/// `collect_major` / `collect_minor` increments it by one.
pub fn gc_cycle_count() -> u64 {
    WF_GC_CYCLES.with(|c| c.get())
}

/// True when paged_gc's auto-GC heuristic thinks a collection
/// should run before the next allocation.  Returns `false` if the
/// heap hasn't been touched yet (no point collecting an empty
/// heap).
pub fn should_collect() -> bool {
    WF_HEAP.with(|cell| {
        cell.borrow()
            .as_ref()
            .map_or(false, |h| h.should_collect())
    })
}

/// Ensure the heap is initialised, then run `f` with a mutable
/// reference to it.  Lazy initialisation lets sessions that never
/// touch the GC avoid paying for it.
pub fn with_wf_heap<R>(f: impl FnOnce(&mut PageHeap<Wf64Layout>) -> R) -> R {
    WF_HEAP.with(|cell| {
        let mut borrow = cell.borrow_mut();
        if borrow.is_none() {
            *borrow = Some(PageHeap::<Wf64Layout>::with_reservation(DEFAULT_HEAP_BYTES));
        }
        f(borrow.as_mut().expect("just initialised"))
    })
}

/// Allocate a `FloatVec` of `n_cells` payload cells in `Generation::G0`.
///
/// Returns the tagged pointer (with `TAG_FLOATVEC` low bits) ready
/// to store into a HEAPPTR slot.  Returns `None` if allocation
/// fails.  Payload cells start as zero (paged_gc fills with
/// `Wf64Layout::FILL_WORD` on page acquisition).
pub fn alloc_floatvec(n_cells: u32) -> Option<u64> {
    with_wf_heap(|heap| {
        let total = 1 + n_cells as usize;
        let ptr = try_alloc_in(heap, Generation::G0, total)?;
        unsafe {
            *ptr.as_ptr() = make_header(HeapType::FloatVec, n_cells);
            // Payload cells already zero (FILL_WORD).
        }
        Some(tag_pointer(ptr.as_ptr() as *const u8, HeapType::FloatVec))
    })
}

/// Allocate a `RefVec` of `n_cells` payload cells (all initialised
/// to nil / 0) in `Generation::G0`.
pub fn alloc_refvec(n_cells: u32) -> Option<u64> {
    with_wf_heap(|heap| {
        let total = 1 + n_cells as usize;
        let ptr = try_alloc_in(heap, Generation::G0, total)?;
        unsafe {
            *ptr.as_ptr() = make_header(HeapType::RefVec, n_cells);
        }
        Some(tag_pointer(ptr.as_ptr() as *const u8, HeapType::RefVec))
    })
}

/// Allocate a `String` of `n_bytes` in `Generation::G0`.  Caller
/// is responsible for writing the bytes to the payload (offset 8
/// from the returned untagged address).
pub fn alloc_string(n_bytes: u32) -> Option<u64> {
    with_wf_heap(|heap| {
        let payload_cells = ((n_bytes as usize) + 7) / 8;
        let total = 1 + payload_cells;
        let ptr = try_alloc_in(heap, Generation::G0, total)?;
        unsafe {
            *ptr.as_ptr() = make_header(HeapType::String, n_bytes);
        }
        Some(tag_pointer(ptr.as_ptr() as *const u8, HeapType::String))
    })
}

/// Allocate a `MutStringBuilder` with `capacity_bytes` of usable
/// payload, length initialised to 0.  See `docs/strings_design.md`:
/// 2-cell header (type/length, capacity), payload is
/// `ceil(capacity/8)` cells.
pub fn alloc_builder(capacity_bytes: u32) -> Option<u64> {
    with_wf_heap(|heap| {
        let payload_cells = ((capacity_bytes as usize) + 7) / 8;
        let total = 2 + payload_cells;
        let ptr = try_alloc_in(heap, Generation::G0, total)?;
        unsafe {
            *ptr.as_ptr() = make_header(HeapType::MutStringBuilder, 0); // length=0
            *ptr.as_ptr().add(1) = capacity_bytes as u64;
        }
        Some(tag_pointer(ptr.as_ptr() as *const u8, HeapType::MutStringBuilder))
    })
}

/// Run a major GC, walking each `[base, next)` region in `regions`
/// as a root set.  Each cell gets `evac.visit_cell`'d precisely.
///
/// Multi-region so V2s (compile-time string literals) can register
/// a second LITERAL region alongside HEAPPTR without changing the
/// per-region scan semantics.
///
/// # Safety
/// For every `(base, next)`: both must be 8-byte aligned, `next >=
/// base`, and the cells in between must each hold either nil (0)
/// or a tagged `Wf64Layout` pointer.
pub unsafe fn collect_major(regions: &[(u64, u64)]) {
    for &(base, next) in regions {
        debug_assert!(next >= base);
        debug_assert!(base & 7 == 0, "region base must be 8-byte aligned");
        debug_assert!(next & 7 == 0, "region next must be 8-byte aligned");
    }

    with_wf_heap(|heap| {
        heap.collect_major(|evac| {
            for &(base, next) in regions {
                let n_slots = ((next - base) / 8) as usize;
                let base_ptr = base as *mut u64;
                for i in 0..n_slots {
                    unsafe { evac.visit_cell(base_ptr.add(i)); }
                }
            }
        });
    });
    WF_GC_CYCLES.with(|c| c.set(c.get().wrapping_add(1)));
}

/// Run a minor GC over the same multi-region root set.
///
/// # Safety
/// Same constraints as `collect_major`.
pub unsafe fn collect_minor(regions: &[(u64, u64)]) {
    for &(base, next) in regions {
        debug_assert!(next >= base);
    }

    with_wf_heap(|heap| {
        heap.collect_minor(|evac| {
            for &(base, next) in regions {
                let n_slots = ((next - base) / 8) as usize;
                let base_ptr = base as *mut u64;
                for i in 0..n_slots {
                    unsafe { evac.visit_cell(base_ptr.add(i)); }
                }
            }
        });
    });
    WF_GC_CYCLES.with(|c| c.set(c.get().wrapping_add(1)));
}

/// Auto-cycle: paged_gc internally chooses minor vs major based on
/// `should_collect_major`.  Wraps `PageHeap::collect_auto`.  Walks
/// the same multi-region root set as `collect_major`/`collect_minor`.
///
/// Used by the kernel-side `vec-alloc-*` / `>$` auto-trigger path
/// in lieu of the older "should_collect ? collect_minor" pair —
/// the auto variant will upgrade to a major when tenure pressure
/// crosses the threshold, which the manual minor never would.
///
/// # Safety
/// Same constraints as `collect_major`.
pub unsafe fn collect_auto(regions: &[(u64, u64)]) {
    for &(base, next) in regions {
        debug_assert!(next >= base);
    }

    with_wf_heap(|heap| {
        heap.collect_auto(|evac| {
            for &(base, next) in regions {
                let n_slots = ((next - base) / 8) as usize;
                let base_ptr = base as *mut u64;
                for i in 0..n_slots {
                    unsafe { evac.visit_cell(base_ptr.add(i)); }
                }
            }
        });
    });
    WF_GC_CYCLES.with(|c| c.set(c.get().wrapping_add(1)));
}

/// Full stop-the-world collection: force-promote all young objects
/// to Tenured then compact Tenured.  Used by the fragmentation-
/// retry path in the large-object allocators — when
/// `try_alloc_large` can't find a contiguous run of free pages, a
/// `collect_full` may consolidate enough pages that the retry
/// succeeds.  Don't call casually; this is the expensive cycle.
///
/// # Safety
/// Same constraints as `collect_major`.
pub unsafe fn collect_full(regions: &[(u64, u64)]) {
    for &(base, next) in regions {
        debug_assert!(next >= base);
    }

    with_wf_heap(|heap| {
        heap.collect_full(|evac| {
            for &(base, next) in regions {
                let n_slots = ((next - base) / 8) as usize;
                let base_ptr = base as *mut u64;
                for i in 0..n_slots {
                    unsafe { evac.visit_cell(base_ptr.add(i)); }
                }
            }
        });
    });
    WF_GC_CYCLES.with(|c| c.set(c.get().wrapping_add(1)));
}

/// Tune paged_gc's allocation-budget floor.  The trigger fires at
/// `max(budget_min, 0.5 * tenured_used)` after each collection;
/// default floor is 8 MB.  Small-object-heavy workloads can lower
/// it to fire collections more aggressively; throughput-focused
/// large-object workloads can leave the default.
///
/// No-op if the heap hasn't been initialised yet.  Persists until
/// the next `reset_wf_heap`.
pub fn set_gc_budget_min_bytes(bytes: usize) {
    WF_HEAP.with(|cell| {
        if let Some(h) = cell.borrow_mut().as_mut() {
            h.set_gc_budget_min_bytes(bytes);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::layout::{PAYLOAD_MASK, TAG_FLOATVEC, TAG_MASK, TAG_REFVEC, TAG_STRING};

    /// Tests share the thread_local heap; each test resets first.
    fn reset() { reset_wf_heap(); }

    #[test]
    fn alloc_floatvec_returns_tagged_pointer() {
        reset();
        let p = alloc_floatvec(4).expect("alloc");
        assert_eq!(p & TAG_MASK, TAG_FLOATVEC);
        assert!(p & PAYLOAD_MASK != 0, "address must be non-zero");
    }

    #[test]
    fn alloc_refvec_returns_tagged_pointer() {
        reset();
        let p = alloc_refvec(8).expect("alloc");
        assert_eq!(p & TAG_MASK, TAG_REFVEC);
    }

    #[test]
    fn alloc_string_returns_tagged_pointer() {
        reset();
        let p = alloc_string(13).expect("alloc");
        assert_eq!(p & TAG_MASK, TAG_STRING);
    }

    #[test]
    fn allocated_floatvec_payload_is_zero() {
        reset();
        let p = alloc_floatvec(4).expect("alloc");
        let base = (p & PAYLOAD_MASK) as *const u64;
        unsafe {
            for i in 1..=4 {
                assert_eq!(*base.add(i), 0, "cell {i} not zero-initialised");
            }
        }
    }

    #[test]
    fn collect_with_one_root_keeps_it_alive() {
        reset();
        // Build a fake HEAPPTR region as a Box<[u64; 4]> so its
        // address is stable across the with_wf_heap call.
        let mut region: Vec<u64> = vec![0; 4];
        let p = alloc_floatvec(2).expect("alloc");
        // Write a marker into the payload so we can check survival.
        unsafe {
            let base = (p & PAYLOAD_MASK) as *mut u64;
            *base.add(1) = 0xDEAD_BEEF;
            *base.add(2) = 0xCAFE_BABE;
        }
        region[0] = p;

        let base = region.as_mut_ptr() as u64;
        let next = base + 4 * 8; // 4 slots used
        unsafe { collect_major(&[(base, next)]); }

        // Slot may have been rewritten if the object moved.
        let p_after = region[0];
        assert_eq!(p_after & TAG_MASK, TAG_FLOATVEC);
        unsafe {
            let base_ptr = (p_after & PAYLOAD_MASK) as *const u64;
            assert_eq!(*base_ptr.add(1), 0xDEAD_BEEF);
            assert_eq!(*base_ptr.add(2), 0xCAFE_BABE);
        }
    }

    #[test]
    fn reset_clears_the_heap() {
        reset();
        // Allocate something, then reset; subsequent allocation
        // gets a fresh heap.
        let _ = alloc_floatvec(1).expect("alloc");
        reset_wf_heap();
        // After reset, the next allocation should succeed (proving
        // the heap was re-initialised).
        let p2 = alloc_floatvec(1).expect("post-reset alloc");
        assert_ne!(p2, 0);
    }
}
