# GC integration design — paged_gc as a Forth heap

## Goal

Give WF64 a garbage-collected heap for large objects (vectors, lists,
strings, future SIMD payloads) without changing Forth's value
semantics. Existing arithmetic, the data stack, the dictionary,
`VARIABLE` — all unchanged. The GC's responsibility is a parallel,
explicitly-rooted region of long-lived storage.

## The model in one screen

There are two distinct kinds of address the user sees:

- **Handle** — the address of a `HEAPPTR` slot in user space.  A
  regular Forth address.  Stable for the program's lifetime: the slot
  doesn't move.  Safe to leave on the data stack indefinitely, pass
  to colon definitions, store in `VARIABLE`s.  The GC never touches
  the handle's address; only its *contents* can change (when GC
  rewrites the slot to point at the moved object).

- **GC pointer** — the address of the actual heap object.  May get
  moved by the GC during a cycle (the slot pointing at it gets
  rewritten transparently).  Lives **only** inside HEAPPTR slots and
  transiently inside the body of one access primitive.  **Never
  appears on the user-facing data stack.**

The user works exclusively with handles. Every access word
(`vec-f@`, `vec-f!`, `vec-alloc-floats!`, …) takes a handle as input,
dereferences it internally to get the GC pointer, does its work, and
returns. The GC pointer's lifetime is bounded by the body of one
primitive, never spanning a possible collection.

The `HEAPPTR` region is the sole root set. The GC scans only that
region. Nothing else — not the data stack, not the return stack, not
`VARIABLE` cells, not the dictionary heap — is examined for roots.
Nothing else *needs* to be: by construction, no GC pointer ever
escapes to those places.

## Why this works

The reason the earlier "tag every Forth value" path was a mistake:
it conflated *where roots live* with *what type a value is*. Forth's
data stack is a transient bus, not a root storage area. The cells
on it at any moment are whatever the most recent words pushed —
mostly raw integers, addresses, intermediate computations. Treating
them as potential roots forces tagging, forces compaction
compromises, forces a kernel rewrite.

The right framing: **there is a fundamentally different kind of
variable for GC-managed references.** `VARIABLE` holds Forth values;
`HEAPPTR` holds GC pointers. The two namespaces don't share storage,
don't share access words, don't share scanning rules. The
HEAPPTR region is small (4 KB = 512 slots in the default
configuration), homogeneous in content (every cell is either nil or
a tagged GC pointer), and the GC walks it in microseconds.

## The Forth API

```forth
HEAPPTR name              \ defining word: reserves a slot, creates
                          \ `name` that pushes the handle (slot addr)

vec-alloc-floats! ( n handle -- )
                          \ allocate n cells of opaque f64 payload
                          \ + header, store the tagged pointer in
                          \ the slot at [handle].  Atomic from the
                          \ user's perspective — the raw GC pointer
                          \ never appears on the data stack.

vec-alloc-refs!   ( n handle -- )
                          \ allocate n cells of pointer-typed payload
                          \ (each cell holds a GC pointer or nil)
                          \ + header, store in [handle].

vec-f@   ( handle index -- ) ( F: -- val )
vec-f!   ( handle index -- ) ( F: val -- )
                          \ access an opaque-f64 vector's cells

vec-len  ( handle -- n )
                          \ length in cells (excluding header)

!heapptr ( gc-ptr handle -- )
@heapptr ( handle -- gc-ptr )
                          \ raw escape hatches.  !heapptr tags on
                          \ store, @heapptr strips on load.
                          \
                          \ The address @heapptr returns has STRICTLY
                          \ BOUNDED LIFETIME.  Treat it like a
                          \ pointer returned by alloca: valid until
                          \ the next allocation or `(gc)`, then
                          \ undefined.  If you need to hold a
                          \ reference across either of those events,
                          \ store it back via !heapptr — that's what
                          \ HEAPPTRs are for.

(gc)     ( -- )           \ run a major collection.  Explicit only
                          \ in V1; data stack must be clean of
                          \ raw gc-ptrs (which never happens if you
                          \ use the wrapped vec-* words).

gc-stats ( -- )           \ dump heap stats to stdout
```

## A worked example — 2 MB float vector

```forth
HEAPPTR samples                             \ slot 0 (or wherever)

262144 constant N-SAMPLES                   \ 256K f64 × 8 = 2 MB

262144 samples vec-alloc-floats!            \ alloc + park
\ ▲ data stack is empty here.  samples now references the block.

: fill-sines  ( handle -- )
    N-SAMPLES 0 DO
        i s>f 1000e f/ fsin
        dup i vec-f!
    LOOP
    drop ;

samples fill-sines

samples 1000 vec-f@ f.                      \ → 0.841471

: sum-vec  ( handle -- ) ( F: -- total )
    0e
    N-SAMPLES 0 DO  dup i vec-f@ f+  LOOP
    drop ;

samples sum-vec f.                          \ → ~-132.34

(gc)                                        \ data stack clean

samples 1000 vec-f@ f.                      \ still 0.841471
```

The user never sees a tagged value, never holds a raw GC pointer,
never has to think about when GC fires. The discipline is "use the
`vec-*` words and you're fine" — which is the same kind of
discipline already needed for `>r` / `r>` matching.

## Where the storage actually lives

- **Kernel + dictionary** — at `dict_base`, near-2 GB region from the
  VirtualAlloc2 placement we already do. Unchanged.

- **User area** — at `dict_base + 0x80000`. Existing layout unchanged.
  Adds two new fields:

  | offset | size | name |
  |---|---|---|
  | `0x1800` | 8 B | `user_HEAPPTR_BASE` (constant: addr of slot 0) |
  | `0x1808` | 8 B | `user_HEAPPTR_NEXT` (bump pointer for new HEAPPTRs) |
  | `0x1810` | 4 KB | slots — 512 HEAPPTR slots in declaration order |

  Growth strategy: when `HEAPPTR foo` is declared, store the current
  `next` value as `foo`'s handle and bump `next` by 8.  At 512 slots
  we run out; phase 2 may grow via a side region.

- **GC heap** — a separate VirtualAlloc region, owned by
  `paged_gc::PageHeap<Wf64Layout>`. Far from the dictionary heap so
  large allocations don't fight the kernel's near-2 GB reservation.
  Sized at start; grows or fails as configured.

## What `Wf64Layout` looks like

We model `Wf64Layout` directly on `LispLayout`'s shape — same 3-bit
tag scheme, same header word format, same scan rules. The
differences are which types we encode in the 5-bit `HeapType` field
and what their `pointer_cells_start..end` shape is.

```text
Tag (3 bits, low bits of every cell):
  000 Fixnum     → Immediate  (only appears as the nil/0 fill word)
  001 Cons       → PointerCons   (reserved; phase 2)
  010 FloatVec   → PointerHeader  (raw f64 array)
  011 RefVec     → PointerHeader  (vector of GC pointers)
  100 String     → PointerHeader  (raw bytes)
  101            → (reserved)
  110 Immediate  → Immediate  (reserved for future non-pointer non-fixnum types)
  111 Forward    → Forwarded  (GC internal)

Header word (1 cell per object, offset 0):
  bits 0..4   type (5 bits, indexes a small HeapType enum)
  bits 5..28  length in cells, excluding header (24 bits, max 16M cells)
  bits 29..36 GC bits (mark, age, pin slot — managed by paged_gc)
```

### Nil and the Fixnum tag — deliberate deviation from Lisp

A reader coming from `LispLayout` will expect `Fixnum` to be the
tag for general integers, with classify-as-immediate as the
arithmetic shortcut. **WF64 doesn't have tagged integers.** The
Fixnum tag exists solely so that the value `0` — written into every
freshly-allocated cell as the fill word, and into every HEAPPTR
slot when nothing has been stored there — classifies cleanly as
`Immediate` and the GC skips it. No general integer ever has the
Fixnum tag because no Forth value is ever stored in a place the GC
scans except via `!heapptr`, and `!heapptr` always tags as a
pointer.

The contract:

| cell value | `Wf64Layout::classify` returns |
|---|---|
| `0` | `Immediate` (via Fixnum branch) — this is nil |
| `addr | 0b010` | `PointerHeader(addr)` — FloatVec pointer |
| `addr | 0b011` | `PointerHeader(addr)` — RefVec pointer |
| `addr | 0b100` | `PointerHeader(addr)` — String pointer |
| `addr | 0b111` | `Forwarded(addr)` — GC-internal |
| anything else | `Immediate` (won't actually appear in scanned regions) |

V1a's synthetic stress tests include explicit cases for the nil
classification — a HEAPPTR region full of zeros runs a GC cycle
without crashing, with stats showing zero objects marked.

Inside the GC heap, pointer-bearing cells are tagged. The HEAPPTR
region's slots are also tagged (`!heapptr` ORs in the header tag
before writing). `@heapptr` masks the tag off before returning,
restoring a plain address. So the Forth-visible "GC pointer" inside
a primitive is always untagged; the tag exists for GC bookkeeping
only.

`paged_gc`'s scanner reads cells via `Wf64Layout::classify(raw)`.
For HEAPPTR slots — every cell is either `0` (nil → classified
as `Immediate` via the Fixnum branch) or a tagged pointer. Both
branches are handled. No false positives, no false negatives.

For inside-heap-object cells — depends on the object's type. A
`FloatVec`'s payload is opaque (raw f64 bits), so its `HeapType`
declares `pointer_cells_start == pointer_cells_end == 0`. The GC
scans only the header. A `RefVec`'s payload cells are all
pointer-typed, so `pointer_cells_start = 1, pointer_cells_end =
1 + length`. Cells of unknown shape (a future "binary blob" type)
also declare opaque.

## Root scanning

When `(gc)` runs, the runtime function gathers a single range:

```rust
let lo = up + USER_HEAPPTR_REGION_BASE;
let hi = read_u64(up + USER_HEAPPTR_NEXT);   // bump pointer
heap.collect_major(|evac| {
    let mut p = lo as *mut u64;
    let end = hi as *mut u64;
    while p < end {
        unsafe { evac.visit_cell(p); }
        p = unsafe { p.add(1) };
    }
});
```

`evac.visit_cell` is `paged_gc`'s precise-root API: it reads the cell,
classifies it via `Wf64Layout::classify`, marks the target if it's a
pointer, and (in the rewrite pass) updates the cell if the target
moved.

That's the entire root walk. No conservative pinning, no data-stack
scan, no return-stack scan. Compaction is unrestricted because no
unmoveable references exist.

## Safepoint discipline (V1)

`(gc)` is the only GC entry point. The user's responsibility:

1. Don't call `(gc)` while a raw GC pointer (from `@heapptr` or an
   internal access path) is live on the data stack. In practice this
   means: don't reach for `@heapptr` directly unless you know what
   you're doing; use the wrapped `vec-*` words.

2. The wrapped words are atomic from the GC's view — they
   dereference, operate, return. No partial state spans a possible
   GC. So as long as the user doesn't manually call `(gc)`
   mid-sequence in a custom word that's holding a raw pointer, the
   model is safe.

V2 adds automatic GC inside `vec-alloc-*` words: if `should_collect()`
fires, the allocator runs a cycle *before* allocating, so the new
allocation can't be invalidated mid-flight. The HEAPPTR rooting
makes this work — every existing reference is reachable through a
handle, so the cycle is safe.

## Generational correctness

`paged_gc` is generational. Old → young pointer writes need card-mark
write barriers. `paged_gc` exposes `mark_card_at(slot_addr)` as the
runtime hook (sub-phase 9's runtime side is already done; what
sub-phase 9 adds is compiler-emitted automatic barriers, which we
don't need — we hand-call from inside our access primitives).

`vec-f!` writing into an opaque payload cell — no barrier needed
(the cell holds raw f64 bits, not a pointer).

`!heapptr` writing into a HEAPPTR slot — this IS a potential old→
young write. The runtime function calls `mark_card_at(slot_addr)`
unconditionally before storing. Cost: one byte-store per `!heapptr`.

A future `vec-ref!` for `RefVec` cells — same drill, barrier on
every store.

## Implementation phases

### V1a — `Wf64Layout` against paged_gc's own tests

Pure Rust. No kernel touches. Implement `HeapLayout` for
`Wf64Layout`. Add the crate as a path dependency. Run paged_gc's
311-test suite against `PageHeap<Wf64Layout>`. Add WF64-flavoured
synthetic stress tests modelled on the existing `LispLayout` ones —
30–50 tests covering the same scenarios with our tag scheme and
`HeapType` variants.

Done when all of paged_gc's tests pass under our layout and the new
synthetic tests we author also pass. This validates the binding
before any kernel work begins.

**The V1a → V1b checkpoint.** Before any MASM is written, one of the
new synthetic tests builds a Rust-side simulation of the HEAPPTR
region: a `Vec<u64>` of mixed nil and tagged-pointer cells, plus a
"NEXT" length value, fed to `heap.collect_major(|evac| { for cell
in region[..next] { evac.visit_cell(cell.as_mut_ptr()); } })`. The
test then verifies survivors are present and that rewrites updated
the simulated slots correctly across the cycle. This catches bugs in
the interaction between `Wf64Layout::classify`,
`rewrite_pointer_addr`, and the closure-based safepoint API before
they show up as silent corruption through MASM. V1b doesn't start
until this test passes.

### V1b — Kernel hooks and HEAPPTR region

`Wf64Session` owns the `WfHeap`. User area gains the HEAPPTR region
fields. New runtime functions: `rt_vec_alloc_floats(n, slot_addr)`,
`rt_vec_alloc_refs(n, slot_addr)`, `rt_gc_collect(up)`. New kernel
primitives in MASM: `HEAPPTR`, `!heapptr`, `@heapptr`, `vec-f@`,
`vec-f!`, `vec-alloc-floats!`, `vec-alloc-refs!`, `vec-len`, `(gc)`,
`gc-stats`.

Done when the 2 MB worked example above runs end to end, `(gc)` is
visibly reclaiming dropped vectors via `gc-stats`, and at least one
test pushes ~100 vectors through the heap with explicit collections
between, verifying object survival via the rooted ones.

### V1c — VARIABLE-style ergonomics

The defining-word machinery for `HEAPPTR`. Better error reporting
for nil dereferences and slot-region overflow.

**Forget semantics — explicit.** The interaction between Forth's
`forget` and the HEAPPTR region is the one place the model is
subtle, and it's worth pinning down precisely:

```forth
HEAPPTR a
HEAPPTR b
1024 a vec-alloc-floats!
2048 b vec-alloc-floats!
forget a              \ what happens?
(gc)                  \ what does the root walk see?
```

After `forget a`, the dictionary entries for both `a` and `b` are
gone (Forth's `forget` rewinds the dict chain past everything
defined since `a`). But the HEAPPTR slots themselves are still
sitting in user space with valid tagged pointers in them. If we did
nothing else, the next `(gc)` would walk those slots, mark the two
2K-cell vectors as live, and we'd leak them forever — no Forth code
can reach them by name, but the GC can still see them as roots.

The fix has two parts, both required:

1. **Rewind `HEAPPTR_NEXT`.** When a HEAPPTR-defining word gets
   forgotten, `HEAPPTR_NEXT` rolls back past its slot. The GC
   walks only `[BASE, NEXT)`, so after the rewind the abandoned
   slots are no longer in the scanned region.

2. **Zero the abandoned slots.** Before `HEAPPTR_NEXT` rewinds,
   write `0` into every slot that's about to fall outside the
   scanned region. This matters because `HEAPPTR_NEXT` will
   eventually advance again — if the user re-declares
   `HEAPPTR c` after the `forget`, the new slot is whatever
   was sitting at the rewound address. Without zeroing, a fresh
   `c` would appear to start life pointing at the now-dead
   vector from before the `forget`. Stale by construction.

Implementation hook: WF64's existing `marker` / `forget` machinery
already has a callback for "this header is about to be unlinked."
We add a check: if the header's word was a HEAPPTR, zero its slot
and decrement `HEAPPTR_NEXT` by 8. The two together are five
instructions; the consistency invariant they protect is "every cell
in `[BASE, NEXT)` is reachable via some still-live HEAPPTR word."

Net effect of the example above: after `forget a`, both slots are
zeroed and `NEXT` rewinds past both. `(gc)` walks an empty region.
Both 2K-cell vectors become unreachable and get reclaimed. Correct.

### V2 — auto-GC and write barriers

`vec-alloc-*` words check `should_collect()` and run a cycle if so.
`!heapptr` calls `mark_card_at`. Generational correctness verified
by stress tests that allocate young, promote to old via repeated
collections, mutate old to point at young.

**Cycle counter — a feature, not derivable.** Once auto-GC fires
inside `vec-alloc-*`, any `@heapptr` result a user is holding may
silently become stale across an allocation that used to be safe in
V1. We expose `gc-cycle ( -- n )`: a monotonically-increasing
counter incremented on every collection. A long-running word that
genuinely needs to keep a raw pointer can snapshot the counter
before, compare after each allocation, and refresh from
`@heapptr` if it changed. This is rarely needed in practice — the
wrapped `vec-*` words don't expose the problem — but it's a
feature the implementation has to provide, not something users
can derive on their own. Spec it in V2, even if most code never
touches it.

### V2s — Managed strings (parallel to V3)

Adds immutable `String` and `MutStringBuilder` heap types on top
of V2's GC. Replaces the four overlapping unsafe Forth string
representations with one safe-by-default library. Sits between V2
and V3 — could land before or after V3 depending on which workload
hits first (V2s helps every program that does I/O; V3 helps
numerical kernels).

Full design lives in `strings_design.md`. The short version: handle-
first API, `$` suffix convention, immutable strings + builders for
incremental construction, UTF-8 native with codepoint-aware access,
`$>addr` as the alloca-style legacy-interop escape with the same
lifetime contract as `@heapptr`.

### V3 — Vector primitives (no DSL)

`MAKE-VEC4 ( s0 s1 s2 s3 handle -- )`, `V.` for printing, basic
field access. Bulk operations: `vec-fill`, `vec-copy`, `vec-map`
implemented in Forth using `vec-f@`/`vec-f!`. Sets the stage for
the SIMD DSL.

### V4 — VEC DSL with element-wise ops

`VEC (inputs; scalars) -> (outputs) = body END`. Mirrors LET's
grammar. f64x4 by default. Vector stack in **ymm6-ymm15** — all ten
callee-saved YMM registers, parity with LET's xmm6-xmm15 budget.
This gives the same realistic ceiling (7–8 user-named values once
libm calls steal slots) and one less thing to remember about the
two DSLs. `axpy`, scalar-times-vector, vector-add.

### V5 — VEC reductions, masks, select, LET unification

`reduce_add`, `reduce_min`, lane-comparison masks,
`select(mask, then, else)` via `vblendvpd`. Scalar WHEREs inside
VEC bodies (compute `1/sqrt(len_sq)` once, broadcast, use in lanes).
Four-pixel Mandelbrot rewriting the LET-guide example.

## Open questions answered (from earlier rounds)

- **`HeapLayout::rewrite_pointer_addr`** preserves the original 3-bit
  tag and ORs in the new payload. Two lines in LispLayout.

- **LispLayout's header** carries type (5 bits), length (24 bits),
  and GC bits (8 bits) in one cell. Per-object size is in the
  header; per-type cell-shape lives in a static enum.

- **Alignment hints** — not exposed by paged_gc. Use `vmovupd`
  (unaligned, free on Sandy Bridge+).

- **Safepoint API** — closure-based: `heap.collect_major(|evac| { …
  })`. Inside, walk roots and call `evac.visit_cell(addr)`.

## What's deliberately not happening

- **Tagging Forth values.** `5` stays `5`. Arithmetic primitives
  stay arithmetic primitives. Number I/O stays as is. core.f stays
  as is.

- **GC-scanning the data stack, return stack, or `VARIABLE` region.**
  None of those hold GC pointers by construction. If a user puts
  one there manually via `@heapptr some-var !`, they have opted in
  to use-after-free; we document, don't enforce.

- **Compacting the dictionary heap.** Compiled code has rel32
  references; moving it breaks the code. The dict heap grows
  monotonically and can be `forget`-rewound, but isn't GC'd.

- **Multi-threaded mutator support.** WF64 is single-threaded; no
  reason to fight paged_gc's evolving threading model.

- **Auto-differentiation** of GC-stored data. Different project.

## The two doc fixes from this conversation

While we're updating documentation: `docs/dsl_user_guide.md`
already has the corrections discussed — the "10 named values"
claim now reads "realistic ceiling 7-8 in libm-heavy bodies because
each lifted call consumes a slot," and the "multi-line CODE: in
interactive REPL" caveat is now in the 90-second intro of the
CODE: section rather than buried in the errors block.

## Where this goes if it works

If V1a–V1c land cleanly, WF64 has GC-managed storage for arbitrary-
sized objects with a usage pattern that doesn't disturb Forth's
existing character. V2 adds the ergonomics that make it pleasant to
use without manual `(gc)` calls. V3–V5 give us SIMD numerics that
operate on data that actually fits the problem (a 16 MB simulation
domain, a 4 MP image, a 100K-point trajectory) rather than chunks
sized for whatever fits on the data stack.

That sequence — heap, then ergonomics, then SIMD — matches how the
project has grown so far: substrate first, polish next, performance
last. Each step is testable in isolation against the layer below.
