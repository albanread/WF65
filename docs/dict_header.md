# WF64 Dictionary Header

This document describes the current WF64 dictionary/header layout and the xt/header mechanism it implements.

## Overview

WF64 now has two linked pieces of per-word metadata:

1. The header side in dictionary memory.
2. The xt-side backoffset cell immediately before the executable entry.

That means a word can be reached in both directions:

- header/NT -> xt via `dh_xtptr`
- xt -> header/NT via the cell at `xt - cell`

This gives constant-time recovery of header-side metadata from an xt.

WF64 now also uses `dh_tfa` as a real type byte for the first time:

- `0x82` = colon definition (`tcol`)
- `0x91` = CREATE-family definition (`tcre`)

## Header layout

Cell size is 8 bytes.

| Field | Offset | Size | Meaning |
|---|---:|---:|---|
| `dh_link` | `0x00` | 8 | previous header in the dictionary chain |
| `dh_ct` | `0x08` | 8 | compilation-token entry point |
| `dh_xtptr` | `0x10` | 8 | executable xt |
| `dh_comp` | `0x18` | 8 | compile helper slot |
| `dh_rec` | `0x20` | 8 | recognizer slot |
| `dh_vfa` | `0x28` | 2 | view field |
| `dh_ofa` | `0x2A` | 2 | optimizer/body-related field |
| `dh_stk` | `0x2C` | 2 | stack-effect field |
| `dh_tfa` | `0x2E` | 1 | type flag |
| `dh_nt` | `0x2F` | 1 + bytes | counted name token |

`dh_name` is the first character byte, immediately after the counted-length byte at `dh_nt`.

## Memory picture

For a runtime-created colon definition, the layout in dictionary memory is:

```text
header base
    |
    v
+-------------------------------+
| dh_link      (8)              |
+-------------------------------+
| dh_ct        (8)              |
+-------------------------------+
| dh_xtptr     (8) ------------+-----------------------+
+-------------------------------+                       |
| dh_comp      (8)              |                       |
+-------------------------------+                       |
| dh_rec       (8)              |                       |
+-------------------------------+                       |
| dh_vfa       (2)              |                       |
+-------------------------------+                       |
| dh_ofa       (2)              |                       |
+-------------------------------+                       |
| dh_stk       (2)              |                       |
+-------------------------------+                       |
| dh_tfa       (1)              |                       |
+-------------------------------+                       |
| dh_nt len    (1)              |                       |
| dh_name bytes                  |                       |
| padding to cell alignment      |                       |
+-------------------------------+                       |
| xt-side backoffset cell (8)    | <------------------+ |
+-------------------------------+                    | |
| first code byte of word        |  xt --------------+ |
| compiled body / machine code    |                      |
+-------------------------------+                      |
                                                       |
ct = xt + *(xt - cell) <-------------------------------+
```

The xt-side cell stores:

```text
ct_backoffset = (header + dh_ct) - xt
```

So the owning header is recoverable from an xt without a dictionary scan.

## Primitive words

Primitive words live in the JIT text allocation rather than the dictionary heap, but they now use the same xt-side convention.

Each `proc(...)` reserves one cell immediately before the exported code label. During dictionary bootstrap:

1. `(create)` creates the live dictionary header.
2. `(set-xt)` points `dh_xtptr` at the primitive's JIT entry.
3. Rust computes `ct_backoffset = (header + dh_ct) - xt`.
4. Rust temporarily makes the containing JIT page writable with `VirtualProtect`.
5. Rust writes the backoffset into `xt - 8`.
6. Rust restores the original page protection.

This avoids any extra jump or stub. The canonical xt remains the real primitive entry address.

## Runtime-created words

`(create)` now reserves one cell immediately before the body xt and writes the same `ct_backoffset` there when the header is created. So colon definitions and future runtime-defined words already have the same constant-time xt -> header path.

That means `>name` can be constant-time for both:

- primitives
- colon definitions
- any other runtime-created words that use `(create)`

### CREATE-family words

WF64 now has a minimal public `create` word.

Unlike `:` words, a CREATE-family word needs its xt to execute custom per-word code: running the created word must push its body address. So `create` uses the header from `(create)` and then writes a small per-word stub at `dh_xtptr`.

Current stub shape:

```text
xt:
    mov [rbp-8], rax      ; spill old TOS
    mov rax, body_addr    ; new TOS = body address
    sub rbp, 8            ; grow data stack by one cell
    ret
    padding to 24 bytes

body:
    first byte of the created word's data area
```

So for CREATE-family words today:

- `dh_tfa = 0x91`
- `body = xt + 24`
- executing the word pushes `body`

This is the first real success case for `>body` in WF64.

WF64 also now has minimal `catch` / `throw` support:

- `catch` installs a handler frame and returns `0` on success or the throw code on failure
- `throw` unwinds to the current handler when non-zero
- `abort` throws `-1`, and `?throw` implements the current `( f n -- )` conditional throw shape
- named throw-code words now include `throw_abort`, `throw_abortq`, `throw_componly`, `throw_namereqd`, and `throw_mismatch`
- `(comp-only)` now throws `throw_componly`
- uncaught throws unwind back to `forth_main` and are reported to the Rust host as an error

## Current semantics

Today the important fields behave like this:

- `dh_ct` defaults to `compile_comma`
- `dh_xtptr` is the executable entry
- `dh_comp` still mirrors the generic `compile,` path rather than a distinct `xt-call,` action
- `set_flags` marks an entry immediate by switching `dh_ct` to `execute`
- `dh_tfa` is no longer used as an immediate scratch byte; it now carries actual type information

So the current interpreter/compiler split is:

- interpret state: execute `dh_xtptr`
- compile state: call `dh_ct` on `dh_xtptr`

### `>body`

WF64 now supports both the CREATE-family success case and the rejection path for `>body`.

- For `tcre` words, `>body` returns the created word's body address.
- For other word kinds, `>body` now `THROW`s `-31`.

That means `>body` now has the right behavior shape for both supported and unsupported word kinds, even though the broader THROW message/reporting layer is still minimal.

## Note

The WF64-specific part is only how primitive xt-side cells are populated after MCJIT finalization. The observable dictionary shape is that xts are self-describing again.

## Relevant files

- `kernel/macros.masm`
- `kernel/dict.masm`
- `kernel/compile.masm`
- `src/lib.rs`
- `tests/harness.rs`
