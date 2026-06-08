# What WF64's Forth side needs from paged_gc

A working note from the Forth team to the GC team.  Originally
written part-way through landing V2 of `docs/gc_design.md`; updated
after the GC team's review.  This second pass marks every item as
**Done**, **Partially addressed**, **Decide**, or **Open**, with
the code change WF64 made (if any) in response.

Status of WF64's integration: V1a–V1c, V2 stages A+B, V2s stages
A+B landed.  Heap is a `PageHeap<Wf64Layout>` in a `thread_local`,
lazily initialised at 64 MB reservation.  Root set: two contiguous
user-area regions (HEAPPTR for runtime handles, LITERAL for
compile-time string slots), both walked precisely by
`evac.visit_cell` on every collection.

---

## 1. Static-region card table — **Open (primitive exists, ergonomics gap)**

**Original ask:** `register_static_root_region(base, size_cells)`
convenience or a worked example for the existing
`collect_minor_with_static` API.

**Review:** Primitive is at `coordinator_api.rs:190` with the right
signature.  No `register_static_root_region` helper, and the
binding has to wire up `CardTable` manually — sizing, ownership,
reset timing all left as an exercise.

**Status:** Still blocked on either the wrapper or a worked
example.  Until then, WF64 continues to rescan all of
`[HEAPPTR_BASE, NEXT)` and `[LITERAL_BASE, NEXT)` on every minor.
Acceptable at current region sizes (4 KB + 64 KB).  Will become a
real cost in long-running sessions with 10K+ live handles.

**WF64 next move:** None until V2c.  Hold for GC-team progress.

---

## 2. User write-barrier `mark_card_at` — **Done**

**Original ask:** Confirm `mark_card_at` is the right hook for
binding-driven writes; confirm whether to gate on generation.

**Review:** Docstring at `space.rs:786` is explicit: "Cheap: one
byte store via `AtomicU8::store(Relaxed)`.  Safe to call
unconditionally — false positives just keep a card dirty for one
extra cycle."  Out-of-reservation addresses are a no-op.

**Status:** Answered.  Call unconditionally on every `vec-ref!`-
shaped write when V3 lands.  No `generation_of` gate needed.

**WF64 code change:** None today.  Note in the V3 design that
`vec-ref!` will call `rt_mark_card_at(slot_addr)` after the store.

---

## 3. Pin support for `$>addr` — **Decide (option a/b deferred)**

**Original ask:** Scoped `heap.pin(addr, |p| { ... })` API, or
confirmation that the alloca-style discipline is the contract.

**Review:** No `heap.pin` API exists.  The conservative-pin feature
(`#[cfg(feature = "conservative-pin")]`) does fire on every
`collect_minor_with_static` call from the supplied stack ranges.
If the raw `$>addr` result lives on the Forth return stack during
the Win32 call, it may already be protected — *if* WF64 wires the
return stack into `pin_stack_ranges`, which it doesn't today.

**Status:** Decide.  WF64 currently treats `$>addr` as a sharp
tool — option (b).  The conservative-pin route is worth a closer
look once we have a real workload that breaks.  Not a blocker for
shipping V2s.

**WF64 next move:** Add a "lifetime contract" note in the V2s user
guide (when it gets written) and move on.

---

## 4. Trigger configuration setter — **Done**

**Original ask:** Setter for `auto_gc_trigger_bytes` /
`gc_budget_min_bytes`.

**Review:** `set_gc_budget_min_bytes(bytes)` exists at
`space.rs:672` and recomputes the trigger threshold immediately.
That covers the practical case: setting the budget floor lower
makes collections fire more aggressively.
`set_tenured_full_threshold_bps` covers the major-trigger side.

**WF64 code change:** Re-exported as `gc::set_gc_budget_min_bytes`
in `src/gc/mod.rs`.  Not called anywhere yet — default 8 MB floor
is fine for current vector-heavy tests.  Will revisit if a
small-object-heavy workload hits.

---

## 5. Heap stats accessor — **Done (one nice-to-have remains)**

**Original ask:** `heap.stats()` exposing per-generation
occupancy, allocation counters, and cycle counts.

**Review:** `stats()` at `space.rs:688` returns `GcStats` with
~20 fields including `g0/g1/tenured_pages`, `*_used_bytes`,
`bytes_alloc_since_gc`, `auto_gc_trigger_bytes`,
`last_mark_live_bytes`, `last_zero_live_pages_released`,
`minors_since_g0_promote`, `g0_promotes_since_g1_promote`.
**Missing:** cumulative `minor_cycles` / `major_cycles` totals.
For "did a minor or a major run?" use
`CollectResult.promoted_g0`.

**WF64 code change:** None yet.  `WF_GC_CYCLES` (a single counter)
is enough for the current tests.  Splitting it into separate
minor/major counters can wait until V2c assertions need it.

---

## 6. Heap clear — **Open, low priority**

**Original ask:** `heap.clear()` to avoid drop + re-VirtualAlloc.

**Review:** No `heap.clear()` exists; drop + re-init is the only
path.

**Status:** Open, low priority.  `reset_wf_heap` continues to
drop the whole `PageHeap`.  Cheap enough at session-reset
cadence (one per harness test).

---

## 7. Major-GC trigger pattern — **Done**

**Original ask:** When to call `should_collect_major()`; should
auto-trigger ever skip minor and go straight to major.

**Review:** Use `collect_auto`, which internally calls
`should_collect_major()` and upgrades minor → major when tenure
pressure crosses the threshold.  Always check, never minor-first
when major is needed; cost of a major over an empty G0/G1 is just
the tenured-scan overhead.

**WF64 code change:** Refactored the auto-trigger path
(`vec-alloc-floats!`, `vec-alloc-refs!`, `>$`).  Replaced the
two-call "`rt_gc_should_collect` then maybe `rt_gc_collect_minor`"
pattern with a single `rt_gc_auto_step(UP)` extern that gates on
`should_collect()` and dispatches to `collect_auto` when true.
`collect_auto` does the minor-vs-major decision internally.

`heap.collect_auto` re-exported as `gc::collect_auto` in
`src/gc/heap.rs`.  Suite still green after the refactor —
behavioural equivalence preserved.

---

## 8. Cycle counter alignment — **Done (keep ours)**

**Review:** Only public cycle accessor is
`minors_since_g0_promote()`, a within-cohort counter that resets
on each G0 promotion.  No cumulative counter.

**WF64 code change:** None.  `WF_GC_CYCLES` stays.  Document in
the runtime that it's a WF64-side artifact, not a paged_gc shadow.

---

## 9. Allocator failure semantics — **Done (added fragmentation retry)**

**Review:** `try_alloc_boxed_in` returns `None` when n_cells == 0,
n_cells > 8192, or free pages exhausted.  `try_alloc_large` can
**also** fail from fragmentation: 10 scattered free pages won't
satisfy a 4-page request even though the total is enough.
Recommended pattern: alloc → if None and n_pages > 1 →
`collect_full` → retry → throw if still None.

**WF64 code change:**
- New `gc::collect_full(regions)` wrapper around
  `heap.collect_full`.
- New `rt_gc_collect_full(up)` extern.
- `rt_vec_alloc_floats`, `rt_vec_alloc_refs`, `rt_string_from_bytes`
  now take `up` and implement the retry: on alloc failure for a
  payload exceeding one page, run `collect_full(regions)` then
  retry once before surfacing `u64::MAX`.  Kernel callers pass
  UP in `rcx`.

No test specifically exercises the fragmentation path (it's hard
to induce in a 64 MB heap with the test sizes WF64 uses), but the
code path is now in place for production workloads.

---

## 10. Move-event hook — **Open, low priority**

**Review:** No `FnMut` callback in the evacuator.  Low priority
as the original doc said; would be nice when SIMD DSLs (V4/V5)
produce harder-to-trace allocation patterns.

---

## Updated priority summary

Items the GC team can close as resolved (WF64 has made the
matching code changes): **#2, #4, #7, #8, #9**.

Items still genuinely open:

- **#1** — static-region card table convenience or worked
  example.  Real blocker for V2c.
- **#3** — pin decision (option a/b).  Not a V2s blocker;
  WF64 currently treats `$>addr` as a sharp tool per option (b).
- **#5** — `minor_cycles` / `major_cycles` cumulative counters
  on `GcStats`.  Nice-to-have; WF64 splits its own counter when
  V2c assertions need it.
- **#6, #10** — `heap.clear`, move-event hook.  Low priority,
  unchanged.

The two V2c blockers from the original doc remain:

1. **#1** (worked example or convenience wrapper)
2. **Documentation of the `collect_auto` pattern in WF64 source**
   — landed in this round; see comments in
   `kernel/gc.masm:vec_alloc_floats_store` and the new
   `rt_gc_auto_step` docstring.

— WF64 Forth side, post-review
