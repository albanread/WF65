# WF65 memory map

How WF65 lays out memory at runtime: the register file, the one big
JIT/dictionary region, the user area, and the separate side regions — including
the **RW / no-execute data region** that holds variable and `CREATE` bodies
(the W^X split).

All constants below are from `src/lib.rs` (region offsets) and
`kernel/macros.masm` (user-cell offsets); the two are kept in lockstep.

---

## Registers

WF65 is a subroutine-threaded code (STC) Forth: every word is entered with a
native `call` and returns with `ret`, so the **return stack is the native
RSP**.

| Reg | Role | Notes |
|-----|------|-------|
| RAX | **TOS** — top of data stack, always in a register | never spilled to memory between words |
| RBP | **DSP** — data stack pointer, points at NOS | grows **down** (push = `sub rbp,8`) |
| RBX | **UP** — user-area base pointer | callee-saved in Win64, survives Win32 calls |
| RSP | **return stack** = native call/ret stack | grows down; STC return addresses live here |
| R15 | **LP** — locals stack pointer | grows down in the locals region |
| R12–R15 | reserved for Win64 call-outs | R12 = RSP-save slot; never use R11 for that |
| RCX RDX RSI RDI R8–R11 | scratch inside primitives | not preserved across a `call` |

`cell = 8`. NOS is at `[RBP]`, NNOS at `[RBP+8]`.

---

## The main region (128 MB, `PAGE_EXECUTE_READWRITE`)

One `VirtualAlloc2` reservation placed in **near memory** (within ±1.7 GB of the
JIT-compiled kernel) so the dictionary and kernel can reach each other with
`rel32` calls/jumps. `region_base` is the allocation base; everything here is an
offset from it.

```
region_base + 0x00000  ┌───────────────────────────────┐
                       │ data stack   (grows DOWN from  │  RBP / DSP
                       │              0x40000)          │
region_base + 0x40000  ├───────────────────────────────┤  OFFSET_DSP_TOP
                       │ return stack (grows DOWN from  │  RSP
                       │              0x80000)          │
region_base + 0x80000  ├───────────────────────────────┤  OFFSET_RSP_TOP / OFFSET_USER_BASE
                       │ USER AREA  (RBX / UP)          │  fixed cells + sub-regions
                       │   see "User area" below        │
region_base + 0xC0000  ├───────────────────────────────┤  OFFSET_DICT_BASE
                       │ DICTIONARY + CODE             ↑│  user_HERE bumps UP
                       │   headers, colon-def code,     │  (PAGE_EXECUTE_READWRITE)
                       │   CREATE stubs, CODE:/LET? no  │
                       │   (those go to the JIT arena)  │
                       │              ...               │
region_base + 128MB    ├───────────────────────────────┤
        − 64 KB        │ SEH unwind metadata           │  DEBUG_META_SIZE; RtlAddFunctionTable
region_base + 128MB    └───────────────────────────────┘
```

| Symbol | Value | Meaning |
|--------|-------|---------|
| `REGION_SIZE` | 128 MB | total reservation, `PAGE_EXECUTE_READWRITE` |
| `OFFSET_DSP_TOP` | `0x40000` | data stack top (empty); grows down |
| `OFFSET_RSP_TOP` | `0x80000` | return stack top; grows down |
| `OFFSET_USER_BASE` | `0x80000` | user-area base = RBX/UP |
| `OFFSET_DICT_BASE` | `0xC0000` | dictionary/code base; `user_HERE` grows up |
| `DEBUG_META_SIZE` | 64 KB | runtime-word SEH unwind info at the top |

The dictionary holds **code and headers only**. Word *data* (variable bodies,
`CREATE`/`buffer:` storage) does **not** live here — see the data region below.

---

## User area (`RBX` / UP, at `region_base + 0x80000`)

A flat block of fixed cells plus a few embedded sub-regions. Key cells (offsets
from UP; mirrored in `kernel/macros.masm` and `src/lib.rs`):

| Offset | Cell | Meaning |
|--------|------|---------|
| `0x00` | `base` (radix) | `BASE`; the Forth word `base` returns **UP** itself |
| `0x10` | `user_LATEST` | newest dictionary header |
| `0x18` | `user_HERE` | **code**-space bump pointer (dict/code) |
| `0x20` | `user_DICT_END` | top of the code region |
| `0x70` | `user_RSP_CURRENT` | saved return-stack pointer |
| `0x1300` | `USER_FP_STACK` | floating-point stack |
| `0x1820` | **`user_VAR_HERE`** | **data**-space bump pointer (variables/CREATE bodies) |
| `0x1828` | **`user_VAR_LIMIT`** | end of the data region (overflow guard) |
| `0x2000` | HEAPPTR region (4 KB) | GC root slots — 512 × 8 B |
| `0x3000` | LITERAL region (64 KB) | compile-time literal slots |

Note the two distinct bump pointers: **`user_HERE`** advances *code* (colon
definitions, headers, CREATE stubs) in the executable region; **`user_VAR_HERE`**
advances *data* in the separate no-execute region. `here` / `,` / `c,` / `allot`
/ `align` / `falign` / `buffer:` all operate on `user_VAR_HERE`; the colon
compiler and `CREATE` stub emitter operate on `user_HERE`.

---

## Side regions (separate `VirtualAlloc`s)

These are reached by absolute address (not `rel32`), so they can live anywhere
in the address space.

### Data space — variables & CREATE bodies (16 MB, `PAGE_READWRITE`, **no execute**)

`alloc_var_region()` → `var_base`, `user_VAR_HERE` bumps up, `user_VAR_LIMIT =
var_base + 16 MB`.

This is the **W^X split**. Every `CREATE`-family word (`variable`, `constant`,
`2variable`, `buffer:`, `value`, `defer`, `fvariable`, …) puts its body **here**,
not in the executable dictionary. The word's executable stub stays in the code
region and bakes the body's absolute address into its `mov rax, imm64` (at
`xt+6`); `>body`, `defer@/!`, `to`/`is`, and the `hotvariable` inliner all read
the body from that baked `imm64`.

Why a separate region — two reasons, one defect:

1. **W^X.** Previously the whole dictionary was `PAGE_EXECUTE_READWRITE`: all
   data was executable and all code was writable. Now data is non-executable and
   the data pointer can never write into code.
2. **No self-modifying-code machine clears.** A variable body used to sit at
   `xt+24`, sharing a 64-byte cache line with its executable `CREATE` stub at
   `xt+0..18`. A `v ! … call v` loop stored into a line holding in-flight
   instructions → the CPU issued an SMC machine clear (~265 cyc) on **every**
   reference. Moving bodies off executable pages eliminates this for *all*
   variables (a cold-variable loop dropped ~34000 → ~238 cyc/iter, ~143×).

`allot` bounds-checks against `user_VAR_LIMIT` and throws `-8` on overflow (the
single data-growth choke point). `forget` reclaims `user_VAR_HERE` for CREATE
words (strict LIFO: the body `imm64` at `xt+6` is the `VAR_HERE` captured at
creation). `marker` snapshots/restores it alongside HERE/LATEST. `reset()` rolls
it back to the post-bootstrap snapshot. `unused` reports `VAR_LIMIT − VAR_HERE`.

### Locals stack (1 MB, `PAGE_READWRITE`)

`alloc_locals_region()`; R15 (LP) grows down from the top. Holds `{: … :}` /
`LET` locals for the executing colon definition.

### Near JIT code arena (32 MB, `PAGE_EXECUTE_READWRITE`)

Placed within ±128 MB of `region_base` so runtime `CODE:` / `LET` machine code is
`rel32`-reachable from both the kernel and the dictionary — runtime-assembled
words become first-class, no far-jump trampoline.

---

## Summary

| Region | Size | Protection | Pointer | Grows |
|--------|------|------------|---------|-------|
| data stack | (in 128 MB) | RWX | RBP | down from `+0x40000` |
| return stack | (in 128 MB) | RWX | RSP | down from `+0x80000` |
| user area | (in 128 MB) | RWX | RBX (fixed) | — |
| dictionary / code | ~127 MB | **RWX** | `user_HERE` | up from `+0xC0000` |
| **data (vars/CREATE)** | **16 MB** | **RW, no-exec** | **`user_VAR_HERE`** | **up** |
| locals | 1 MB | RW | R15 | down |
| JIT arena | 32 MB | RWX | — | bump |
| HEAPPTR (GC roots) | 4 KB | RWX | — | bump |
| LITERAL | 64 KB | RWX | — | bump |

The code/dictionary region stays `PAGE_EXECUTE_READWRITE` because the colon
compiler and `does>` patch code at runtime; hardening it to read-execute would
require per-write `VirtualProtect` and is out of scope. The variable data region
is the part that is now strictly non-executable.

See also: [dictionary_overlay.md](dictionary_overlay.md) (wordlist overlay
structure) and the dictionary-header layout in `kernel/macros.masm`.
