# WF64 — Getting started

For anyone (human or Claude) opening this project for the first time, or coming back to it after a break.

If you're working **in** the project, the [`jasm-forth` skill](.claude/skills/jasm-forth/SKILL.md) under `.claude/skills/` auto-loads when Claude opens this directory and covers the day-to-day mechanics (writing a primitive, debugging a crash, the test DSL). This file is the higher-level orientation: what we're building, where the source comes from, the methods we use, and where to look next.

---

## 1. Objective

Produce a **fully usable 64-bit STC ANS Forth on Windows x86-64** with a pragmatic bootstrap and a live, inspectable implementation. The project centers on the language itself: vocabulary, stack behavior, parsing, compiler/interpreter flow, dictionary shape, and runtime semantics.

WF64 deliberately delegates assembler work, code loading, and Win32 binding generation so effort stays on the language rather than on bootstrap machinery.

---

## 2. Source

The active source of truth is this repository.

- `kernel/*.masm` contains the primitive and runtime implementation.
- `lib/core.f` contains stable source-defined startup words.
- `tests/harness.rs` and `tests/data/**` define the behavioral contract.
- `src/lib.rs` owns session bootstrap, dictionary registration, and runtime wiring.

There are legacy tools and reference materials in the tree, but contributor-facing decisions should be made from the current WF64 code and tests.

---

## 3. Target — WF64

WF64 produces a single Rust binary (`cargo run`) that:

1. Loads the assembled kernel into MCJIT at process start.
2. `VirtualAlloc2`s a 128 MB region within ±2 GB of the kernel (so all `call rel32`s fit) and carves it into data stack / return stack / user area / dictionary heap.
3. Bootstraps the dictionary by calling JITed `(create)` / `(set-xt)` / `(set-comp)` / `(set-flags)` once per primitive.
4. Drops into the REPL (`quit`) — `>` prompt, line-buffered stdin, and startup source loading.

```
E:\WF64\
├── Cargo.toml                  depends on wfasm (path = "../JASM/rust") and newgc-core (path = "../NewGC/newgc-core")
├── README.md                   public-facing intro
├── get-started.md              this file
├── docs/MILESTONES.md          M1–M7 roadmap with status
│
├── .claude/skills/jasm-forth/
│   └── SKILL.md                day-to-day mechanics, auto-loads in this dir
│
├── src/
│   ├── main.rs                 binary entrypoint — live REPL
│   ├── lib.rs                  Wf64Session — JIT setup, memory layout, PRIMITIVES, dict bootstrap
│   ├── runtime.rs              rt_emit / rt_type / rt_read_line / rt_print_int / rt_bye
│   ├── wf32_port.rs            legacy translation helper library
│   └── bin/port_wf32.rs        legacy CLI front-end retained in-tree
│
├── kernel/                     the Forth, in .masm
│   ├── main.masm               entry point (forth_main); @includes all the rest
│   ├── macros.masm             register defs, user-area offsets, proc/endp/next/stk/win64_call/brk
│   ├── runtime_decls.masm      @extern declarations for Rust runtime fns
│   │
│   │   ─── primitive groups ───
│   ├── stack.masm              dup, swap, drop, rot, over, …, 2drop, 2dup, …, 4dup
│   ├── rstack.masm             >r, r>, r@, rdrop, 2>r, …, sp@, sp!, rp@, rp!
│   ├── memory.masm             @, !, c@, c!, 2@, 2!, b@/sb@/b!, w@/sw@/w!, L@/L!, q@/q!
│   ├── arith.masm              +, *, -, negate, abs, 1+, …, /mod, um*, m*, um/mod, sm/rem, fm/mod, */, */mod, /, mod
│   ├── logic.masm              and, or, xor, invert, lshift, rshift, arshift, on, off
│   ├── compare.masm            0=, 0<>, 0<, 0>, =, <>, <, >, <=, >=, u<, u>, min, max, within
│   ├── strings.masm            count, fill, cmove, cmove>, move, zcount, lastchar, slastchar, exchange, bounds, /string, compare, str=, skip, -skip, scan, -scan, search, count-bits, msbit, lsbit, cells, cell+, cell-, char+, chars, aligned
│   │
│   │   ─── interpreter / compiler / I/O ───
│   ├── number.masm             . / number?
│   ├── io.masm                 emit, type, cr, bye
│   ├── parse.masm              parse-name, accept
│   ├── dict.masm               find-name, (create), (set-xt), (set-comp), (set-flags)
│   ├── execute.masm            execute
│   ├── compile.masm            do_lit, compile,, literal, :, ;
│   ├── interp.masm             quit (the REPL loop)
│   └── win32/                  generated bindings (kernel32.masm + 10 others). NOT @include'd
│                                 by default — kernel only calls rt_* runtime fns today.
│
├── tests/
│   ├── harness.rs              shared-session harness + data-driven runners
│   └── data/
│       ├── direct/*.t          one per primitive — push / call / expect DSL
│       └── eval/*.in + *.out   REPL source → expected stdout pairs
│
└── .cargo/config.toml          pins RUST_TEST_THREADS=1 (session state is shared)
```

Sibling projects this depends on:

- `E:\JASM\rust\` — **the assembler.** WF64's `Cargo.toml` references it via `path = "../JASM/rust"`. Build: `cd ..\JASM\rust ; cargo build`. The USER-GUIDE there is the JASM language reference.
- `E:\JASM\rust\` — the assembler dependency. The USER-GUIDE there is the JASM language reference.

### Dictionary layout

WF64 now uses a full dictionary header with an xt-side backoffset cell. `LATEST` still points at the header base, but `find-name` returns the **name token** (`nt`), which is the counted string stored in the header.

The full layout and xt-side backoffset mechanism are documented in [docs/dict_header.md](docs/dict_header.md).

Current header shape in `kernel/macros.masm`:

| Field | Meaning |
|---|---|
| `dh_link` | previous header in the linear dictionary chain |
| `dh_ct` | compilation-token entry point (`compile,` by default, `execute` for immediate words) |
| `dh_xtptr` | executable xt for interpret-state execution |
| `dh_comp` | compile-time helper slot |
| `dh_rec` | recognizer slot, reserved for later work |
| `dh_vfa`, `dh_ofa`, `dh_stk`, `dh_tfa` | compatibility fields for vocabulary/optimiser/type metadata |
| `dh_nt` | counted string byte: length followed by name bytes |

Operational rules:

- `(create)` allocates the full header, copies the counted name, defaults `dh_ct` and `dh_comp` to `compile,`, sets `dh_xtptr` to the new body start, then links the entry into `LATEST`.
- `(set-xt)` overwrites `dh_xtptr`, which is how the Rust bootstrap points primitive headers at their JIT-resolved code addresses.
- `(set-flags)` is now a bootstrap convenience shim: non-zero marks an entry immediate by switching `dh_ct` to `execute`.
- In interpret state, the REPL executes `dh_xtptr`. In compile state, it calls `dh_ct` on `dh_xtptr`.

Current short-term simplification: `dh_comp` exists in the right place, but today it still mirrors the generic `compile,` path rather than a distinct `xt-call,` / inline-capable compiler action.

---

## 4. Method — the test-first development loop

The day-to-day workflow is:

```
1. Write tests/data/direct/<word>.t describing the primitive or behavior you want.
2. cargo test --test harness -- --nocapture
     → harness reports the new word as NYIMP. Suite stays green.
3. Implement the smallest plausible fix or new primitive.
4. Put it in the right kernel/*.masm file or in `lib/core.f`.
5. Add (forth_name, asm_sym, flags) to PRIMITIVES in src/lib.rs.
6. cargo test --test harness
     → NYIMP flips to PASS.
```

**Adding a primitive never requires editing Rust test functions.** The harness reads `.t` data files and classifies each test as **PASS / FAIL / NYIMP** (Not Yet IMPlemented — asm symbol missing). Only FAIL fails the suite.

### Legacy translation helpers

The tree still contains `wf32_port.rs` and `port_wf32.rs` as optional legacy tooling. They can still be useful for extracting rough first drafts, but they are not the project objective or source of truth.

### Tests — three kinds

| Kind | Location | When |
|---|---|---|
| Direct DSL | `tests/data/direct/*.t` | Per-primitive cell-accurate tests. Default. |
| REPL eval | `tests/data/eval/*.in` + `.out` | When testing user-visible REPL behaviour. |
| Direct Rust API | `tests/harness.rs` | Stateful / multi-session scenarios the DSL can't express. |

**Direct DSL commands:** `push <int>` · `push_pad <off>` (push `user_base + USER_PAD + off`) · `poke <pad-off> <hex>` (seed memory) · `call <asm-sym>` · `expect <int>...` (bottom-first, empty = empty stack) · `expect_bytes <pad-off> <hex>` · `reset` (session back to post-bootstrap). Comments: `#` or `;`. Integers: decimal, `0xHEX`, `-int`, `-0xHEX` (handles i64::MIN). Underscores in numbers allowed.

**REPL eval:** the `.in` is fed through `quit`; the `.out` is exact-matched against captured stdout. Optional `# requires: <forth-words>` line declares dependencies — if any are missing from PRIMITIVES, the test is NYIMP. `#`-prefixed lines are stripped before the source reaches the kernel.

### Behavioral discipline

When you encounter an oddity or bug, fix it in WF64 and document the local reasoning in the proc comment or test. The goal is a correct, coherent system, not compatibility with an external implementation.

---

## 5. Links

| Link | What |
|---|---|
| [`E:\JASM\rust\`](file:///E:/JASM/rust/) | JASM macro assembler + MCJIT wrapper |
| [`E:\JASM\rust\USER-GUIDE.md`](file:///E:/JASM/rust/USER-GUIDE.md) | JASM language reference (directives, macros, scope rules) |
| [`E:\JASM\rust\README.md`](file:///E:/JASM/rust/README.md) | JASM architecture overview |
| [`.claude/skills/jasm-forth/SKILL.md`](.claude/skills/jasm-forth/SKILL.md) | Day-to-day mechanics (auto-loads in-project) |
| [`docs/MILESTONES.md`](docs/MILESTONES.md) | M1–M7 roadmap with status |
| [forth-standard.org](https://forth-standard.org/) | ANS / Forth 2012 standard |

---

## 6. Build & run

```powershell
cd E:\WF64
cargo build
cargo test                                   # full suite — Rust unit tests + harness tests + data-driven .t/.in/.out
cargo test --test harness -- --nocapture     # show PASS / FAIL / NYIMP summary
cargo run                                    # interactive REPL — type `2 3 + . bye`
cargo run --bin port-wf32 -- '+'             # optional legacy translator helper
```

**Knobs:**

- `WF64_DUMP_ASM=1 cargo test --test harness` — dump the post-expansion asm JASM hands to LLVM-MC. Use when a macro isn't expanding the way you expect.
- `WF64_BOOT_INFO=1 cargo run` — print region base / HERE / LATEST on boot.
- `WF32_KERNEL=<path>` — override the legacy translator input path.

**Sub-second iteration discipline.** Harness tests share one `Wf64Session` (rebuilding the session was the bottleneck). Once the crate is already built, the full `cargo test` run is still sub-second in practice; rebuild time is dominated by Rust compile/link work rather than by kernel execution.

---

## 7. Register conventions — memorise these

| Reg | Role |
|---|---|
| RAX | **TOS** — top of data stack, register-cached |
| RBP | **DSP** — data stack pointer, points at NOS, grows downward |
| RBX | **UP** — user area base pointer (callee-saved in Win64; survives Win32 callouts) |
| RSP | **return stack** — also the CPU call/ret stack (STC) |
| R12 | **RSP-save slot** for Win64 callouts (callee-saved; do NOT use R11) |
| RCX, RDX, RSI, RDI, R8–R11 | scratch — don't expect them to survive a `call` |
| R13, R14 | available callee-saved scratch (used by some primitives to carry args across Win64 calls) |
| **R15** | **reserved for the locals stack pointer (LP).** No primitive may use it as scratch. `forth_main`'s prologue pushes it for the Rust caller; eventually it will be initialised from `user_LP0`. |

`@assign cell = 8`. NOS = `[DSP]`, NNOS = `[DSP + cell]`. A cell push is `mov [DSP - cell], TOS ; mov TOS, <new> ; sub DSP, cell` (or let `stk` emit the `sub`).

---

## 8. Status snapshot

(Update this section as milestones land.)

| Milestone | Goal | Status |
|---|---|---|
| M1 | Kernel boots and returns | ✅ |
| M2 | `2 3 + .` prints `5` | ✅ |
| M3 | Tokenised REPL with `accept` / `parse-name` / `find-name` / `execute` / `quit` | ✅ |
| M4 | Colon-definition compiler (`:` … `;`) | ✅ |
| M5 | Control flow (`IF`/`THEN`/`BEGIN`/`UNTIL`/`DO`/`LOOP`) | ✅ |
| M6 | File loading (`INCLUDE`) | ✅ |
| M7 | ANS Forth core test suite passes | ✅ |

**`cargo test` is green. The suite covers Rust unit tests, the harness, and the data-driven `tests/data/` runners — run it to see the current count.**

---

## 9. Where to look next

For the **how** of touching code, see [`.claude/skills/jasm-forth/SKILL.md`](.claude/skills/jasm-forth/SKILL.md). Covers in detail:

- The shape of a primitive proc
- The memory layout, register rules, and JASM workflow details
- The return-stack juggle (`>r`/`r>`/`r@` trap)
- Win64 callouts (the R11 trap)
- Common bugs and how to spot them in a crash dump
- What NOT to do

For the **why** of design choices, this file + the commit log. For the **shape of the JASM language**, the USER-GUIDE in the JASM repo.
