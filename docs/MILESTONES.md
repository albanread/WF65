# WF64 Milestones

Small, observable, sequential. Each finishes with a runnable demo —
no milestone is "halfway done." Order is load-bearing; later
milestones depend on earlier ones.

## M1 — Boot ✅

**Goal:** Kernel loads, `forth_main` runs, returns to driver, process
exits cleanly.

**Source:** empty `proc(forth_main) xor rax, rax  next() endp()`.

**Driver:** install SEH, assemble kernel, JIT, lookup `forth_main`,
call, print exit code.

**Done when:** `$ cargo run` prints `wf64: forth_main returned 0x0`.

## M2 — `2 3 + .` from a hard-coded program

**Goal:** A primitive set just big enough to literally execute
`2 3 + .` end to end and print `5 ` to stdout.

**New kernel content:**
- `kernel/runtime_decls.masm` — `@extern rt_emit(1)` for the host
  runtime function that writes a byte to stdout
- `kernel/io.masm` — `proc(emit)` that calls `rt_emit` with Win64 ABI
- `kernel/arith.masm` — `proc(plus)` (`+`): pop NOS, add to TOS, drop
- `kernel/number.masm` — `proc(dot)` (`.`): print TOS as signed decimal
  followed by a space, then drop
- `kernel/main.masm` — `forth_main` body: `pushd(2); pushd(3); call plus; call dot; next()`

**New driver code:**
- `src/runtime.rs` — `extern "C" fn rt_emit(ch: u64) -> u64` writes
  `ch as u8` to stdout
- In `main.rs`: register `rt_emit` via `Jit::define_extern_fn`
- Update `PRIMITIVES` (in `src/lib.rs`) to include all the new primitive names

**Done when:** `$ cargo run` prints `5 ` and exits 0.

Stack adjustment (`stk` Rust macro from JASM) gets used here for the
first time — `+` is `(a b -- a+b)`, net pop of one cell.

## M3 — Tokenized REPL

**Goal:** `forth_main` reads a line, splits on whitespace, looks each
token up in a dictionary, calls the matching primitive. Echoes `ok`
after each successful line.

**New kernel content:**
- `kernel/stack.masm` — `dup`, `drop`, `swap`, `over`, `rot`
- `kernel/parse.masm` — `parse-name`: scan the source buffer, return
  (addr, len) of the next whitespace-delimited token
- `kernel/dict.masm` — `find-name`: walk a static array of
  (name-ptr, name-len, xt) tuples baked into the kernel image,
  return xt or 0
- `kernel/interp.masm` — `interpret` (one token), `quit` (line loop)
- `kernel/main.masm` — `forth_main` calls `quit`

**New runtime support:**
- `rt_accept(buf, max)` — reads a line from stdin via `ReadConsoleA`
- User area allocation: small buffer for input line, scratch for
  parsed token

**Dictionary in M3 is static:** a hand-written array in
`kernel/dict.masm` listing every primitive's name and xt. M4 makes it
dynamic so the user can add definitions.

**Done when:**

```
$ cargo run
> 2 3 + . ok
5
> 10 20 swap . . ok
10 20
> bye
$
```

## M4 — Colon-definition compiler

**Goal:** `: SQUARE DUP * ; 5 SQUARE .` prints `25 `. New words live
in JITed memory we allocated ourselves; calling them is the same as
calling any kernel primitive.

**New kernel content:**
- `kernel/compiler.masm` — `:`, `;`, `create`, `,` (comma),
  `compile,` (compiles a CALL to xt), `here`, `allot`
- `kernel/state.masm` — `state` (interpret vs compile), `[`, `]`
- Updated dictionary structure: linked list of header records, with
  pointer to next-entry, name length+bytes, flags, and xt

**New runtime support:**
- `rt_alloc_exec(bytes)` — VirtualAlloc a chunk of
  `PAGE_EXECUTE_READWRITE` memory. The dictionary heap is one big
  chunk; HERE allocates within it.

**Mechanism:** colon definitions are just RWX bytes. `compile,` emits
5 bytes per word: `E8 xx xx xx xx` (relative call). `;` emits one
byte: `C3` (ret). The resulting block of bytes is a valid x86-64
function — callable by exactly the same `call <xt>` that calls a
kernel primitive. No interpreter loop, no threading model switch.

**Done when:** `: SQUARE DUP * ; 5 SQUARE .` → `25`.

## M5 — Control flow ✅

**Goal:** `IF`/`THEN`/`ELSE`, `BEGIN`/`UNTIL`, `BEGIN`/`WHILE`/`REPEAT`,
`DO`/`LOOP`/`+LOOP` all work in colon definitions.

**Mechanism:** these are immediate words that emit `?bra`/`bra` (relative
jump primitives) with forward-reference patches resolved on the matching
`THEN`/`REPEAT`/`LOOP`. The primitives `bra`,
`?bra`, `-?bra` already need to exist (used internally by `quit` for
loop dispatch).

**Done when:** a Forth loop runs:

```
$ cargo run
> : COUNTDOWN  BEGIN  DUP .  1-  DUP 0= UNTIL  DROP ; ok
> 5 COUNTDOWN ok
5 4 3 2 1
```

## M6 — File loading ✅

**Goal:** `INCLUDE` loads a Forth source file through the normal source
pipeline, preserving nested source state correctly.

**Implemented shape:**
- `included` / `include` are source-defined in `lib/core.f`
- Rust runtime helpers `rt_slurp_file`, `rt_slurp_len`, `rt_slurp_pop`
  read the file into a nested stack of owned buffers
- evaluation is still routed through the existing `evaluate` path, with
  explicit save/restore of source context

**Host surface used:** Rust stdlib file reads via the runtime helpers;
no separate kernel-side file primitive layer was needed for M6.

**Done when:** a `.fs` file with arbitrary Forth source loads and
runs.

## M7 — ANS Forth core test suite ✅

**Goal:** [forth-standard.org's core test suite](https://forth-standard.org/standard/testsuite)
runs and passes.

This shakes out the long tail of primitives that the earlier
milestones didn't force into existence — exception handling
(`CATCH`/`THROW`), the full number-input vocabulary, double-cell
arithmetic, etc.

**Done when:** anstests64 emits no `WRONG NUMBER OF RESULTS` or
`INCORRECT RESULT` lines.

**Completed:** `cargo test` is green, including `m7_ans_core_tests_pass`.
Key fixes made:
- `$` hex prefix added to `number_q` (number.masm)
- Hayes tester rewritten to use `BEGIN`/`WHILE`/`REPEAT` (no DO/LOOP)
- Various test corrections: `/mod` symmetric semantics, `fm/mod`/`sm/rem`
  require double-cell input, `[']` vs `'` in interpreted vs compiled context,
  `pick` index semantics, `?do...loop` vs `?do...then`

## After M7

Optional shoulders to climb:

- **Self-test as part of `cargo test`** — Rust integration test
  harness that boots WF64, feeds it a script, asserts output.
- **Embed mode** — bake `kernel/*.masm` into the binary via
  `include_str!`, ship a single `wf64.exe` with no external source
  dependency.
- **REPL niceties** — readline-style editing, history, completion.
- **Forth source library** — `tasks.fs`, `editor.fs`, `assembler.fs`
  (Forth-level assembler that emits via the colon-def compiler),
  `recognizers.fs`.
- **AOT** — extend JASM to emit `.o` files via LLVM MC, link via
  Rust's MSVC linker, produce a standalone `wf64-kernel.exe` with no
  Rust runtime. Optional; the embed mode already gives a single .exe.
