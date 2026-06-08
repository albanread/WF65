# WF64 — Vocabularies & word lists

How WF64 stores word lists, how the search order works, and the one sharp
edge you must respect when you create your own word list: **reset-safety**.

This is the practical companion to [dictionary_overlay.md](dictionary_overlay.md)
(which is the design rationale). If you only read one thing, read §4.

---

## 1. Two structures, not one

Every definition lives in **two** independent structures:

1. **Creation-order chain** — a single linear list threaded through each
   header's `dh_link`, rooted at `LATEST`. This is the permanent record. It
   backs `LATEST`/`LATESTXT`, `forget_last`, reset-to-boot rollback, and Rust's
   debug scan. It is *not* used for name lookup.

2. **Searchable overlay** — word-list objects and per-name *overlay nodes*,
   organised by word list and consulted by `find-name`, the interpreter, and
   the search-order words. This *is* name lookup.

The overlay stores a back-pointer to each header; it never duplicates header
fields. Headers remain the source of truth for `xt`/`ct`/`>name`/`dh_comp`/…

## 2. Where word lists live — their own memory section

The reserved dictionary region is **dual-ended**:

```
  low end  ──►  dictionary heap: headers, code bodies, data space   (grows up,   HERE)
  high end ◄──  index arena: word-list objects + overlay nodes       (grows down, INDEX_HERE)
```

So word-list objects and their overlay nodes are **not** in the `HERE`-managed
data space — they have their own arena at the top of the region, growing
downward (`user_INDEX_HERE` / `user_INDEX_LATEST`). This keeps `HERE` semantics
clean and search metadata out of user data space.

A **wid** (word-list id) is the address of a word-list object in that arena.
Each word-list object is a fixed bucket array (`wl_bucket_count = 512` cells)
plus metadata. Names are hash-threaded per word list: each overlay node holds an
ASCII-folded hash, the folded first char, the length, a header pointer, and a
next-in-bucket pointer. Lookup is O(chain length in one bucket).

Name policy: lengths 1–32, ASCII case-folded for lookup (so `DUP` finds `dup`).

## 3. Search order & the standard words

The user area holds the current compilation word list (`CURRENT`), the built-in
`FORTH` wid, the active order count, and the order array (`user_CONTEXT`).

| Word | Effect |
|---|---|
| `wordlist ( -- wid )` | create a new, empty word list in the index arena |
| `get-current ( -- wid )` / `set-current ( wid -- )` | the word list new definitions go into |
| `get-order ( -- widn..wid1 n )` / `set-order ( widn..wid1 n -- )` | read / replace the whole search order (`wid1` is searched first) |
| `also ( -- )` | duplicate the top-of-order wid (widen order by one) |
| `previous ( -- )` | drop the top-of-order wid |
| `definitions ( -- )` | make `CURRENT` = the first wid in the order |
| `only ( -- )` / `forth ( -- )` | reset to a minimal order / put `FORTH` on top |
| `search-wordlist ( c-addr u wid -- 0 \| xt 1 \| xt -1 )` | look a name up in ONE wid (ignores the order) |

Semantics: new definitions go into `CURRENT`; `find-name` searches the order
`wid1 … widn`; `search-wordlist` searches exactly one wid.

Splicing a word list to the front for the duration of some compilation, then
removing it, is a common pattern (it is how `lib/oop.f` scopes instance-variable
names to a class body):

```forth
: (push-front-order) ( wid -- )   >r get-order r> swap 1+ set-order ;
: (drop-front-order) ( -- )       get-order swap drop 1- set-order ;

myvocab (push-front-order)   \ myvocab is now searched first
\ … compile things that should see myvocab's names …
(drop-front-order)            \ back to the previous order
```

## 4. Reset-safety — the sharp edge

> If you create your own word list and add entries to it after boot, you MUST
> arrange for `reset()` to restore its buckets, or the next session-reset will
> leave **dangling bucket heads** and the following lookup can hang or crash.

Why: `Wf64Session::reset()` (test harness, and any embedding that resets)
rewinds the dictionary to its post-boot state. Concretely it:

- rewinds `HERE` to `boot_here` and `INDEX_HERE`/`INDEX_LATEST` to their boot
  values — so **every overlay node created since boot is freed** (the arena
  pointer moves back; the node memory will be reused), and
- restores the bucket arrays of **only** `FORTH`, `TOOLS`, and `PRIVATE` from
  boot snapshots.

A word list you created at boot is itself below the fence (it survives), but any
entries a test added live in rewound arena memory. After reset its buckets still
point at those (now-reused) addresses → the next `(create)` into it links a new
node whose `next` is a stale/aliased node → a self-referential bucket chain →
`find-name` loops forever or faults.

This is exactly why the three built-in word lists are snapshotted/restored, and
why the search test suite was originally flaky before that was added.

### The pattern for a reset-safe boot word list

`lib/oop.f`'s per-class ivar word list does this; copy it:

1. **Create it at boot, before the boot snapshot.** Put the creating source in a
   file loaded during `with_kernel()` (e.g. alongside `core.f`/`oop.f`), so the
   wid exists when the snapshot is taken.
2. **Publish its wid to a known user-area cell** so Rust can find it without
   knowing Forth variable layout:
   ```forth
   wordlist constant myvocab-wl
   myvocab-wl  base $1808 +  !      \ user_OOP_IVARS_WID — pick a free cell
   ```
   (`base` returns the user-area base `UP`; `BASE` the radix variable sits at
   offset 0, so `base N +` is `UP+N`. Free cells live between `user_SELF`
   region and `user_HEAPPTR_BASE = 0x2000`.)
3. **Snapshot its buckets at boot** (in `Wf64Session::with_kernel`, next to the
   FORTH/TOOLS/PRIVATE snapshots): read the wid from the cell, copy its 512-cell
   bucket array into a `boot_*_buckets: Vec<u64>` field.
4. **Restore them in `reset()`** (next to the other bucket restores): if the wid
   is non-zero, `copy_nonoverlapping` the snapshot back over its buckets.

If you create a word list only transiently *within* a single session and never
reset, none of this applies — `HERE`/`INDEX` are monotonic within a session, so
entries don't dangle until a reset rewinds the arena.

### Alternative: don't use a word list at all

If you need a name→value map that must be reset-stable but doesn't need to be in
the search order (e.g. a selector table), a **flat buffer below `boot_here`** is
simpler than a word list: it is never rewound and needs no Rust plumbing.
`lib/oop.f`'s selector table (`sel-names` / `#sel`) is built this way.

---

## 5. Forget & rollback

`forget_last` walks the creation-order chain from `LATEST`, and — because it only
ever removes the newest definition — also unlinks that one header's overlay node
from its bucket. `marker` / `forget` (in `core.f`) build on `forget_last`.

Wholesale `reset()` is the heavier hammer described in §4; it does not walk the
chain node-by-node, it rewinds the arenas and restores the snapshotted buckets.

## 6. See also

- [dictionary_overlay.md](dictionary_overlay.md) — the overlay design rationale.
- `kernel/macros.masm` — `wl_bucket_count`, `user_INDEX_HERE`, `user_CONTEXT`,
  `user_FORTH_WID`/`TOOLS_WID`/`PRIVATE_WID`, and the OOP user cells.
- `src/lib.rs` — `reset()` and the boot bucket snapshots (the reference impl of
  §4).
- `lib/oop.f` — a working reset-safe boot word list (`ivars-wl`) and a flat
  reset-stable table (selectors).
- `lib/core.f` — `also` / `previous` / `only` / `order` / `words` and `marker`.
