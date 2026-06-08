# WF64 Dictionary Overlay Plan

This document describes the planned and initial implementation of WF64's searchable dictionary overlay.

## Goals

WF64 keeps the existing WF32-compatible per-word header shape for compilation, `xt`/`ct` recovery, `>name`, and defining-word behavior, while moving name lookup and wordlist organization into a separate searchable index.

The design goals are:

1. Preserve the current header ABI and existing compilation flows.
2. Replace the single linear `LATEST` scan with a wordlist-aware searchable structure.
3. Support the Search-Order substrate cleanly: `FORTH-WORDLIST`, `WORDLIST`, `GET-CURRENT`, `SET-CURRENT`, `GET-ORDER`, `SET-ORDER`, `DEFINITIONS`, and `SEARCH-WORDLIST`.
4. Make primitive bootstrap publication use the same kernel-side dictionary/index path as ordinary definitions.
5. Keep Rust free to maintain its own debug/introspection list without owning the dictionary policy.

## Non-goals

This overlay does not replace the current header format. It does not move xt/ct metadata out of the header, and it does not redefine the compilation model.

It also does not require the searchable dictionary to live in the normal `HERE`-managed data-space stream.

## Two structures, not one

WF64 keeps two distinct structures:

1. A private global creation-order chain via `dh_link` and `LATEST`.
2. A searchable dictionary overlay organized by word list and search order.

The creation-order chain remains permanent because it is the cheapest correct basis for:

1. `LATEST` / `LATESTXT`
2. `forget_last`
3. reset-to-boot rollback
4. runtime debug scanning in Rust
5. deterministic "newest definition" behavior independent of search order

The searchable dictionary overlay is the public lookup structure used by `find-name`, the interpreter, and search-order words.

## Searchable overlay

The overlay lives in a dedicated index arena allocated from the top of the existing dictionary region and grown downward. The ordinary dictionary heap still grows upward from `HERE`.

That produces a dual-ended layout inside the same reserved region:

1. low end: dictionary heap, headers, bodies, code, Forth data space
2. high end: wordlist objects and dictionary index nodes

This keeps `HERE` semantics simple and keeps search metadata out of user-visible contiguous data-space allocations.

## Header compatibility

WF32-style headers remain the source of truth for per-word metadata:

1. `dh_ct`
2. `dh_xtptr`
3. `dh_comp`
4. `dh_tfa`
5. `dh_nt`
6. `dh_link`

The overlay stores pointers back to headers rather than replacing header fields.

## Word lists

A word list identifier (`wid`) is the address of a wordlist object in the index arena.

Each wordlist object contains:

1. a fixed bucket array
2. optional metadata fields

The first implementation uses one built-in `FORTH` word list created by a kernel init step before primitive bootstrap publication begins.

## Hashing

Name lookup is hash-threaded per word list.

Each definition is published into the current word list using:

1. an ASCII-folded 32-bit hash
2. the folded first character
3. the name length
4. a pointer to the owning header
5. a next pointer for the bucket thread

Buckets are fixed-size per word list. The initial design uses a moderate fixed count rather than dynamic rehashing.

This is intentionally simple, predictable, and fast enough for the target scale.

## Name policy

WF64's implementation policy is:

1. names of length 1 through 32 are supported
2. names longer than 32 are rejected at creation time
3. ASCII case folding is used for dictionary lookup

This keeps standard words findable in uppercase while allowing existing lowercase usage.

## Search order

The user area stores:

1. the current compilation word list (`CURRENT`)
2. the built-in `FORTH` word list (`FORTH-WID`)
3. the number of active word lists in the search order
4. the search-order array itself

The semantics are:

1. new definitions are inserted into `CURRENT`
2. `find-name` searches the active search order from first to last
3. `SEARCH-WORDLIST` searches only the specified wid
4. `DEFINITIONS` makes `CURRENT` equal to the first wid in the search order

## Primitive publication

Primitive bootstrap publication moves out of Rust policy code and into the kernel.

Rust still resolves JIT symbol addresses, but it no longer decides how names are linked into the searchable dictionary. Instead, Rust passes name/xt/helper/flags to a kernel publisher that:

1. creates the ordinary header
2. sets xt/comp/immediacy
3. publishes the definition into the current word list overlay

This unifies primitive publication with ordinary definitions.

## Forget and rollback

`forget_last` continues to use the global creation-order chain rooted at `LATEST`.

When forgetting the newest definition, the kernel also removes that header's overlay node from its owning word list bucket. Because only the newest definition is forgotten, a targeted bucket unlink is sufficient.

## Implementation phases

1. Add overlay user-area state and index-arena allocator.
2. Add wordlist objects and overlay nodes.
3. Initialize the built-in `FORTH` word list before primitive bootstrap.
4. Make `create` publish into the overlay.
5. Make `find-name` search the overlay instead of the global `LATEST` chain.
6. Add the Search-Order substrate words.
7. Replace the Rust bootstrap loop's use of `create`/`set-xt`/`set-comp`/`set-flags` with a kernel primitive publisher.
8. Add source-defined convenience words such as `DEFINITIONS` wrappers and later `VOCABULARY`.

## Current recommendation

Keep the header ABI stable. Treat the searchable dictionary as an overlay over headers, not as the headers themselves.

That gives WF64:

1. a safer migration path
2. clean search-order semantics
3. a scalable lookup structure
4. no disruption to existing compiler/header flows
5. a clear division of responsibility between Rust host services and kernel dictionary policy
