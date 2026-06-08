# Managed strings — design (V2s)

A safe-by-default string library built on the GC heap (see
`gc_design.md`). Drops in alongside the existing legacy string forms
(`S"`, `TYPE`, `COUNT`, `PAD`, …) which continue to work unchanged
for one-shot scripts and trivial output. The new library covers
every case where Forth's traditional strings are dangerous: long
strings, strings that need to outlive their creation context,
strings that get manipulated rather than just printed.

## Why this is needed — Forth's four broken string forms

A modern Forth program has four overlapping ways to represent a
string and none of them are safe:

1. **Counted strings** (`c-addr` with length byte at the address).
   255-byte limit, count byte sits in front so passing the data
   alone loses the length, `c!` can corrupt the count silently.

2. **Address-length pairs** (`c-addr u`). Arbitrary length, but
   the two cells can get separated by `SWAP DROP`, passed in
   reverse, or referenced after the buffer has been overwritten.
   The classic `S" hello" S" world"` footgun: both produce pairs
   pointing into the same transient buffer.

3. **Dictionary-baked literals.** `S"` in a colon-def compiles the
   bytes inline. Stable but live in executable memory; `c!` on
   them is at best UB, at worst a SIGSEGV.

4. **PAD and transient buffers.** Number formatting, parsing,
   conversion — all share scratch regions that any other word can
   clobber.

The combined effect: every Forth string operation requires the
programmer to track lifetime and ownership in their head. That's a
lot of overhead in a language that already wants you to track stack
depth in your head.

## The model

**One value type, immutable: `String`.** Bytes are UTF-8. Length is
in bytes, encoded in the GC header. Once a String exists, its
contents never change.

**One mutable type for building: `MutStringBuilder`** (`sb` in
identifiers). Used for incremental construction over a short stretch
of code; converted to a `String` at the natural boundary via
`sb>string`. Two distinct types so aliasing-with-mutation never
happens by accident.

**Handle-first API, just like the rest of the heap design.** The
user works exclusively with HEAPPTR handles. Raw tagged pointers
appear only inside the body of one primitive — never on the
user-facing data stack between operations.

**`$` suffix convention** for words and handle names. Reads
naturally postfix: `result$ !$`, `name$ @$ sb-append$`. The legacy
forms (`S"`, `TYPE`, …) keep their original names; the `$` suffix
makes "this is the managed-string version" visible at a glance.

## Object shapes

### `String`

- Header: standard 1 cell. Type=4 (`String`), length-in-bytes in the
  24-bit length field (max 16 MB/string, fine — bigger payloads
  should be RefVec-of-Strings split by lines).
- Payload: raw UTF-8 bytes. Not null-terminated. Length is
  authoritative.
- GC scan: opaque. `pointer_cells_start == pointer_cells_end == 0`;
  the GC marks the header and skips the bytes.

### `MutStringBuilder`

- Header: **2 cells.** Type=5 (`MutStringBuilder`), length-in-bytes
  in the first header word's 24-bit field. The second header word
  is the capacity-in-bytes.
- Payload: raw UTF-8 bytes, length-many used, capacity-many
  allocated.
- GC scan: opaque, same as `String`. The 2-cell header is a
  `HeapType`-specific layout returned by
  `Wf64Layout::header_layout` — total_cells reflects the 2-cell
  header. The GC walker advances past the header by total_cells,
  not by a hardcoded 1.

`sb>string` produces a fresh `String` of exactly the current
length, then resets the builder's length to 0 (capacity retained).
The new String is a separate allocation; the builder can be reused
or dropped.

## Compile-time literals — `S$"`

Compile-time string literals live in a **separate LITERAL region**
in user space, parallel to the HEAPPTR region:

```text
user_HEAPPTR_REGION   — user-declared HEAPPTR slots,    4 KB,  bump-allocated
user_LITERAL_REGION   — compile-time literal-string slots, 64 KB, bump-allocated
```

`S$"` at compile time:
1. Allocates the next LITERAL slot.
2. Allocates a `String` GC object containing the bytes.
3. Stores the tagged pointer into the slot.
4. Emits compiled code that, at runtime, pushes `[slot_addr]` —
   the live tagged String pointer (updated by the GC if the
   underlying String moves).

The GC's root walk hits both the HEAPPTR region and the LITERAL
region. Same primitive (one range pair walked precisely per
collection), no shared budget. 64 KB / 8 = 8K literals — enough
for any realistic program.

**`forget` interaction.** V2s leaves literals in place after
`forget`: monotonically retained within a session. This means
experimental REPL sessions can slowly leak literal slots, but a
fresh session is clean. V2s.1 may add per-forget reclamation by
walking the abandoned dict region for embedded literal-load
patterns and zeroing the referenced slots, but it's not essential
for shipping V2s.

## The Forth surface

```forth
\ ── Construction ─────────────────────────────────────────────────────
S$" literal"            ( -- str )     \ literal, allocates a LITERAL slot
empty$                  ( -- str )     \ canonical shared empty string
n char$                 ( ch -- str )  \ single-codepoint string

\ ── Access ───────────────────────────────────────────────────────────
$len      ( $h -- bytes )              \ byte length
$clen     ( $h -- chars )              \ codepoint count, O(n)
$b@       ( $h byte-idx -- byte )      \ byte access; throws on OOB
$c@       ( $h char-idx -- codepoint ) \ codepoint access; O(n)

\ ── Comparison ───────────────────────────────────────────────────────
$=        ( $a $b -- f )               \ byte-exact equality
$ci=      ( $a $b -- f )               \ Unicode case-insensitive
$cmp      ( $a $b -- n )               \ lex compare, -1/0/1
$hash     ( $h -- u )                  \ fast non-cryptographic hash

\ ── Pure operations (return new String, leave operands intact) ──────
$+        ( $a $b -- $c )              \ concatenation
$slice    ( $h start end -- $s )       \ byte range, exclusive end
$substr   ( $h start len -- $s )       \ sugar over $slice
$repeat   ( $h n -- $s )
$trim     ( $h -- $s )
$ltrim    ( $h -- $s )
$rtrim    ( $h -- $s )
$upper    ( $h -- $s )                 \ Unicode case folding
$lower    ( $h -- $s )                 \ (may change byte length)
$replace  ( $needle $repl $haystack -- $s )

\ ── Searching ────────────────────────────────────────────────────────
$find     ( $needle $haystack -- idx | -1 )   \ first match, byte idx
$rfind    ( $needle $haystack -- idx | -1 )
$starts?  ( $prefix $h -- f )
$ends?    ( $suffix $h -- f )
$contains? ( $needle $h -- f )

\ ── Conversion ───────────────────────────────────────────────────────
$>n       ( $h -- n true | false )     \ parse as integer
$>f       ( $h -- ) ( F: -- r true | false )
n>$       ( n -- $s )                  \ format integer
n>$base   ( n base -- $s )
f>$       ( -- $s ) ( F: r -- )
$>addr    ( $h -- c-addr u )           \ legacy interop, see lifetime contract

\ ── Iteration ────────────────────────────────────────────────────────
$each-char  ( $h xt -- )               \ xt called with each codepoint
$each-byte  ( $h xt -- )
$split    ( $sep $h -- $h ... n )      \ stack-shaped; for small splits
$split-vec ( $sep $h -- refvec )       \ RefVec-shaped; for large splits

\ ── Builders ─────────────────────────────────────────────────────────
sb-new        ( capacity -- sb )       \ allocate a builder
sb-append$    ( $s sb -- )             \ append a String's contents
sb-append-c   ( codepoint sb -- )      \ append a single character
sb-append-n   ( n sb -- )              \ append decimal integer
sb-append-f   ( sb -- ) ( F: r -- )    \ append formatted float
sb-len        ( sb -- bytes )
sb-clear      ( sb -- )                \ reset length to 0; keep capacity
sb>string     ( sb -- $s )             \ finalize to immutable String

\ ── HEAPPTR-typed storage ────────────────────────────────────────────
!$        ( $ptr handle -- )           \ store, with type check
@$        ( handle -- $ptr )           \ fetch, with type check
```

## The user-facing safety contract

### `$>addr` lifetime — identical to `@heapptr`

`$>addr` returns a `c-addr u` pair pointing into the GC heap. The
pointer is valid **until the next allocation or `(gc)`** — same
contract as `@heapptr`. Treat it like a pointer returned by alloca.
The intended use is one-shot interop with legacy words:

```forth
my-message$ @$ $>addr type cr        \ fine: type reads once, returns
```

Not the intended use:

```forth
my-message$ @$ $>addr  some-word-that-allocates  rot rot type
\ DANGER: the c-addr may be stale after some-word-that-allocates fires
```

For cases that need a stable byte pointer over multiple operations,
copy into a builder, or pin via a future `$pin ... $unpin` pair if
that need ever materializes.

### `$find` results are codepoint-safe

`$find / $rfind / $starts? / $ends?` all return byte indices that
are guaranteed to fall on UTF-8 codepoint boundaries — needles are
byte sequences and matches start where the needle starts, which is
always a codepoint boundary. **Indices from these words can be fed
straight into `$slice` without revalidation.**

User-supplied indices (from arithmetic, parsed input, etc.) get
validated by `$slice` and throw if mid-codepoint. Loud failure beats
silent garbage.

### UTF-8 validity is not enforced at construction

`$find`-and-friends operate on bytes; they don't care about
validity. `$@c` / `$@ch` / `$upper` / `$lower` / `$clen` validate
at use and throw on malformed UTF-8. There's a `$valid?` predicate
for explicit checks and `$validate` for "give me an error if this
isn't clean UTF-8."

The rationale: validating at construction would force every
incoming-bytes-from-anywhere path (file reads, network, user
input) to either pre-validate or pay double. Validation-on-use is
pay-for-what-you-use.

### Case folding may change byte length

`$lower` and `$upper` use Unicode case folding, not just ASCII. The
German ß lowercases to itself but uppercases to "SS"; Turkish
dotted/dotless i pairs cross-case; etc. The returned string is a
**fresh allocation with whatever length the folding produced**. This
is harmless under immutable-String semantics — you wouldn't expect
in-place mutation anyway.

## Number formatting via builders

The pictured-numeric-output words (`<# #S #> HOLD SIGN`) are good
Forth, but operate on a global PAD. Builder-based replacements:

```forth
\ Old:  <# 1234 s>d #s #>  produces  c-addr u  pointing into PAD
\ New:  sb-new <# 1234 s>d #s #>  sb>string  produces  $s

42 n>$  result$ !$               \ simple cases

\ Custom formatting:
16 sb-new                        ( -- sb )
123 over sb-append-n             ( -- sb )
S$" px" over sb-append$          ( -- sb )
sb>string  result$ !$            \ result$ holds "123px"
```

The pictured-numeric words can still exist (`<#` opens a builder,
`#>` closes and produces a String), preserving the Forth idiom
while removing the trap.

## Legacy interop

The legacy string words still work unchanged. `S" hello" TYPE` is
fine for one-shot output. The new library is parallel-and-safe, not
a replacement.

Bridges in both directions:

- **Managed → legacy**: `$>addr` produces an `c-addr u` pair for
  passing to legacy readers. Lifetime as documented above.
- **Legacy → managed**: `addr u >$` copies bytes from arbitrary
  memory into a fresh `String`. The destination is independent of
  the source after the call — even if the source buffer gets
  reused (PAD-style), the managed copy is safe.

## Object shape — concrete layout

For the implementation files:

```rust
// In Wf64Layout's HeapType enum
pub enum HeapType {
    // ...
    String      = 4,    // 1-cell header, opaque payload bytes
    MutStringBuilder = 5, // 2-cell header (length + capacity), opaque payload bytes
    // ...
}

// In Wf64Layout::header_layout
unsafe fn header_layout(header_cell: *const u64) -> ObjectLayout {
    let raw = *header_cell;
    let type_tag = (raw & TYPE_MASK) as u8;
    let length_bytes = ((raw >> LEN_SHIFT) & LEN_MASK) as usize;
    match HeapType::from_bits(type_tag) {
        Some(HeapType::String) => ObjectLayout {
            total_cells: 1 + (length_bytes + 7) / 8,  // header + padded payload
            pointer_cells_start: 0,
            pointer_cells_end: 0,
        },
        Some(HeapType::MutStringBuilder) => {
            let capacity_bytes = unsafe { *header_cell.add(1) } as usize;
            ObjectLayout {
                total_cells: 2 + (capacity_bytes + 7) / 8,  // 2-cell header
                pointer_cells_start: 0,
                pointer_cells_end: 0,
            }
        }
        // ...
    }
}
```

Note `total_cells` for a builder uses **capacity**, not length —
the GC needs to know the full allocated extent, not the currently-
used portion.

## What's deliberately out of scope for V2s

- **Interning**. Defer to V2s.1 or later. Premature interning is
  real; you want workload data to know if your programs are
  comparison-heavy enough for it to matter. The implementation is
  a small open-addressed hash table keyed by string hash, checked
  at every `S$"` allocation.

- **Regex / pattern matching**. Maybe its own DSL someday (a
  `MATCH:` block in the DSL family). Not V2s.

- **Locale-aware comparison / collation**. Unicode-canonical-form
  normalization. These are hard problems with no good universal
  answer. Out of scope; a power user can implement what they need
  on top of the byte primitives.

- **`forget`-driven literal reclamation**. V2s.1 polish if anyone
  notices a leak; not essential.

- **String pinning** (long-lived `$>addr` results). Add if and when
  a real workload demands it. The alloca-style discipline handles
  the common cases.

## Milestone placement

**V2s — between V2 (auto-GC + write barriers) and V3 (heap vec4).**

The dependency chain is clean:

- V2s needs V2's auto-GC because `S$"` allocates eagerly and a
  literal-heavy file load would otherwise need manual `(gc)`
  pacing. With V2's auto-GC inside `vec-alloc-*`, `S$"` inherits
  the same safety without extra design.

- V2s benefits from V3-style RefVec for `$split-vec`, but
  doesn't require it — `$split-vec` can wait until RefVec lands
  in V3, while `$split` (stack-shaped) handles the common case
  from day one.

**Arguably V2s is more useful than V3** in terms of programs-
unlocked-per-line-of-code. Vectors-for-SIMD only help numerical
code; safe strings help every program that does I/O. If we
wanted to reshuffle, V2s before V3 makes a more usable system
sooner. Either order works technically.

## What the test plan looks like

Roughly 60–80 tests across these axes:

- **Object integrity**: allocate, length, byte access, round-trip
  through GC.
- **Operations**: each operation produces a fresh String, leaves
  inputs intact, handles edge cases (empty, single codepoint,
  multi-byte codepoints, malformed UTF-8 at non-validating
  ops).
- **Compile-time literals**: `S$"` allocates exactly once per
  literal, the LITERAL region grows correctly, two identical
  literals are two distinct objects (interning is deferred).
- **Builders**: append patterns (small/large, single/many
  appends), capacity growth, `sb>string` produces correct
  output, builder reuse after `sb-clear`.
- **Number formatting**: `n>$` / `f>$` round-trip correctly for
  edge values (negative, zero, INT64_MIN, infinity, NaN).
- **Legacy interop**: `$>addr` produces correct bytes for `TYPE`;
  `addr u >$` correctly copies bytes from a transient buffer.
- **GC interaction**: collection during builder growth; String
  survival across cycles; large allocation (1 MB+) triggers
  appropriate collection behavior.

The text-processing demo from the original framing — load file,
tokenize by whitespace, count unique words, print top 10 by
frequency — serves as the integration test. Done without writing
a single `c@` or `c!`.

## Programmer's mental model in one paragraph

You hold handles to immutable strings. To build a string, you use a
builder, append into it, and convert to immutable when done. You
search and manipulate via pure operations that return new strings;
the GC cleans up what nobody references. To talk to a legacy
addr/len-taking word, `$>addr` gives you a pointer that's alive
for one operation. There's no PAD, no counted strings, no
addresses you have to worry about losing the length of, no
mutations behind your back. The library has about 40 words; you
learn 8 of them (`S$"`, `!$`, `@$`, `$+`, `$len`, `$slice`,
`$find`, `$>addr`) and you've covered 90% of real use cases.

That's the deliverable.
