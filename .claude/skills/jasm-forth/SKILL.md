---
name: jasm-forth
description: Use this skill whenever working in the WF64 codebase — writing or editing `.masm` primitives in `kernel/`, adding or debugging tests in `tests/harness.rs`, touching anything in `src/lib.rs` or `src/runtime.rs`, or doing live Forth development through `cargo run --bin wf64`. Triggers on mentions of WF64, JASM, `.masm` files, Forth primitives, `lib/core.f`, startup source loading, `forget_last`, `.s`, live REPL testing, the Wf64Session test pattern, register conventions (RAX=TOS, RBP=DSP, RBX=UP), the `stk` macro, `win64_call`, the rstack-juggle for return-stack primitives, or the legacy `port_wf32` utility. Do NOT use for unrelated Rust or Forth questions outside this project.
---

# WF64 — JASM macro assembly + Rust test harness

You are working on **WF64**, a 64-bit STC Forth for Windows x64 built with JASM and a Rust test harness. We are:

- **Writing Forth in assembly.** Each Forth primitive is a `proc(name) … endp()` block in a `.masm` file under `E:\WF64\kernel\`.
- **Using our own macro assembler.** JASM (at `E:\JASM\rust\`) parses the `.masm` source, expands macros, hands MC-flavour Intel-syntax asm to LLVM-MC for encoding, and exposes the result as a callable function pointer via MCJIT.
- **Testing in Rust.** `E:\WF64\tests\harness.rs` calls primitives directly (`session.push(v); session.call("name"); session.stack()`) and feeds full REPL pipelines through `session.eval(text)` and asserts on captured stdout.
- **Growing Forth live.** `cargo run --bin wf64` starts a live REPL that auto-loads `lib/core.f`; define words interactively, test them in the running image, use `forget_last` to roll back failed experiments, and only then persist successful words into `lib/core.f`.
- **Cheating on bootstrap, not on language.** No metacompile, no PE-image emission, no Forth-side assembler. The priority is a coherent, usable Forth implemented directly in this tree.

When in doubt: prefer the current WF64 code and tests over historical or external reference material. Legacy utilities and notes may still mention `wf32`, but they are not the project's objective.

## Quick reorientation when entering this project

Run these once to see where you are:

```powershell
cd E:\WF64
cargo test --no-run         # confirms the build is clean
cargo test 2>&1 | Select-Object -Last 5   # confirms tests pass
```

Then read `get-started.md` at the project root for the current layout, register conventions, and milestone status. See `docs/MILESTONES.md` for what's done and what's next.

## Live Forth workflow (preferred once the layer exists)

When the current slice can be expressed in source instead of MASM, prefer the real Forth workflow over continuing to hand-build everything in assembly.

```powershell
cd E:\WF64
cargo run --bin wf64
```

Inside the live REPL:

```forth
: trial-word ... ;
trial-word
.s
forget_last   \ if the newest definition is wrong
```

Rules for this stage of WF64:

- Prefer **live REPL testing first** for source-defined words. The goal is to grow Forth inside Forth as soon as the substrate allows it.
- `lib/core.f` is the first persistent source library. The interactive binary loads it automatically at startup before dropping into the REPL.
- Use `.s` while exploring stack effects live.
- Use `forget_last` to roll back only the most recent post-bootstrap definition without restarting the image.
- If a live definition works, save that exact word into `lib/core.f`.
- If a feature still requires new substrate, add only the smallest bootstrap-critical MASM or Rust support needed, then return to live/source growth immediately.
- `.f` files may now use basic Forth comments: `\` to end of line and `( ... )` inline comments.

This is the preferred loop for source-level work:

```text
1. Start `wf64` and let it load `lib/core.f`.
2. Type a new word into the live REPL.
3. Test it immediately in the same running image.
4. If wrong, `forget_last` and try again.
5. If right, copy it into `lib/core.f`.
6. Add or update a harness test to lock the behavior in.
```

## Register conventions (memorise these)

| Reg | Role | Notes |
|---|---|---|
| RAX | **TOS** — always cached in register | every primitive reads/writes TOS in RAX, never via memory |
| RBP | **DSP** — data stack pointer, points at NOS | grows downward (push = `sub rbp, 8`) |
| RBX | **UP** — user area base | callee-saved in Win64, so survives Win32 API calls |
| RSP | **return stack** = native call/ret stack | STC: every `call` pushes a return addr we sometimes have to juggle |
| R12 | **RSP-save slot** for Win64 callouts | callee-saved; **never use R11** for this (caller-saved, gets trashed) |
| RCX RDX RSI RDI R8–R11 | scratch inside primitives | don't expect them to survive a `call` |

`@assign cell = 8` everywhere. NOS lives at `[DSP]`, NNOS at `[DSP + cell]`. Push pattern: `mov [DSP - cell], TOS ; mov TOS, <new> ; sub DSP, cell` (or equivalently let `stk(in, out)` emit the `sub`).

## The shape of a primitive

```masm
; word-name  ( in-stack -- out-stack )
;     One-sentence what-it-does.
proc(asm_symbol)
    ; manipulate TOS in RAX, NOS at [DSP], etc.
    stk(in, out)         ; emits add/sub rbp, |in-out|*cell — or nothing if balanced
    next()               ; emits `ret`
endp()
```

- `proc(name)` opens a `@scope name` plus emits `.globl name` and `name:`. Local labels (`.foo`) inside the proc mangle to `name$$foo` — uniqueness is automatic.
- `endp()` closes the scope. It does NOT emit `ret` — use `next()` (or write `ret` yourself).
- `stk(in, out)` is a Rust macro (registered as `wfasm::asm::macros::stk`) that emits the data-stack-pointer adjustment for the declared stack effect. `stk(2, 1)` → `add rbp, 8` (one net drop). `stk(0, 1)` → `sub rbp, 8`. `stk(1, 1)` → nothing.
- `next()` is just `ret`. STC.

Match the current WF64 conventions and keep changes minimal, direct, and test-backed.

## Test-first porting (the default workflow)

**Adding a new primitive should never require editing Rust.** The harness reads test cases from data files at runtime and classifies each as **PASS / FAIL / NYIMP**:

- **PASS** — asm symbol exists, behaves as expected
- **FAIL** — asm symbol exists, behaves wrong → `cargo test` fails
- **NYIMP** — asm symbol isn't in the kernel yet → suite still passes, NYIMP list printed to stderr

So you write tests **before** porting. The NYIMP list becomes your live to-do list.

### Direct-primitive tests — `tests/data/direct/<word>.t`

Line-oriented DSL. One file per primitive.

```text
# 1+  ( n -- n+1 )
push 41
call one_plus
expect 42

reset
push -1
call one_plus
expect 0
```

Commands:

| Command | Effect |
|---|---|
| `push <int>` | Push a cell. Decimal, `0xHEX`, `-int`, `-0xHEX` (handles `i64::MIN`). Underscores allowed. |
| `push_pad <off>` | Push `user_base + USER_PAD + off` — a scratch address inside the user-area PAD region. |
| `poke <pad-off> <hex-bytes>` | Write raw bytes into PAD+off. E.g. `poke 0xD0 48656c6c6f` seeds "Hello". |
| `expect_bytes <pad-off> <hex>` | Read N bytes from PAD+off and assert they match. Lets string-primitive tests be one-liners. |
| `call <asm-sym>` | Invoke a primitive by its asm symbol. |
| `expect <int>...` | Assert stack equals these values, **bottom-first** (Forth notation; empty = stack must be empty). |
| `reset` | Restore session to post-bootstrap state (clears stack, restores HERE/LATEST, clears STATE/BYE_REQ). |
| `#` or `;` | Comment to end of line. |

NYIMP is auto-detected from `call <sym>` — if any symbol fails `xt_of`, the file is NYIMP without running. The summary names the file AND the missing symbols so you always know what to port.

**PAD scratch:** memory at `PAD+off` persists across tests within the shared session (reset only restores HERE/LATEST/state, not arbitrary memory). If your test reads bytes past the buffer you `poke`d, it can pick up debris from a previous test. Either explicit-poke the byte past your buffer, or use a unique offset per test.

### REPL eval tests — `tests/data/eval/<name>.in` + `.out`

`.in` is Forth source; `.out` is exact-match expected stdout.

```text
# requires: - negate abs 1+ 2/
: diff - ;
7 3 diff .
10 negate .
bye
```

The optional `# requires: <forth-words>` line lists Forth-side names; if any isn't in PRIMITIVES, the pair is NYIMP. `#`-prefixed lines are stripped before the source reaches the kernel, so they don't leak in as `?` tokens.

### Running

```powershell
cargo test --test harness                          # everything
cargo test --test harness -- --nocapture           # see PASS/FAIL/NYIMP summary lines
```

The summary names every NYIMP and the missing symbols, so you always know what to port next.

### Hand-written Rust tests still exist

The older `direct_*` / `eval_*` Rust functions in `tests/harness.rs` cover the M3/M4 baseline (stack ops, dictionary primitives, the colon compiler). Leave those — they're not on the data-files path. New work goes into `tests/data/`.

## Adding a primitive (the loop)

```text
1. Write tests/data/direct/<word>.t describing the primitive.
2. cargo test --test harness -> NYIMP, suite still green.
3. Implement the smallest plausible `proc(...) ... endp()` block.
4. Put it in the right kernel/*.masm file (arith.masm for arithmetic etc.).
5. Add (forth_name, asm_sym, flags) to PRIMITIVES in src/lib.rs.
6. cargo test --test harness -> NYIMP flips to PASS.
```

`port-wf32` is a legacy helper:

- 32-bit registers → 64-bit (`eax → rax`, `ebp → rbp`, …)
- brace memory `{ -cell ebp }` → Intel bracket `[rbp - cell]`
- space-separated operands → comma-separated
- size prefix `byte { ... }` → `byte ptr [...]`
- `cdq` → `cqo`, `dword` → `qword` (the two 32→64 traps it catches automatically)
- Stack effect `N M in/out` → `stk(N, M)` always, even when balanced

What it does NOT do: evaluate RPN immediates (`sar eax cell 8 * 1-`), substitute user-area variable names (`sp0`, `state` etc.), or fix cell-size literals like `-4`. Those land as TODO comments or stay as-is for the reviewer.

For Forth names that don't mangle nicely by default, override on the command line: `cargo run --bin port-wf32 -- '/mod=slash_mod' 'r>=r_from'`.

Default mangling for the common shapes: `>` → `to_`/`_to_`, `<` → `from_`/`_from_`, `@` → `_fetch`, `!` → `_store`, `+` → `plus`, `-` → `minus`, `*` → `times`, `/` → `slash`, `?` → `q`, `=` → `_equal`. Reserved x86 mnemonics get a trailing `_` (`dup_`, `swap_`, `and_`, `or_`, `not_`).

## Why three places, always in order

The asm symbol the porter spits out has to land in (1) the kernel file so JASM exports it, (2) `PRIMITIVES` in `src/lib.rs` so `find-name` can resolve the Forth name to the xt, and (3) a `.t` file so the harness will run it. Skip any and the symptom is silent — the word just isn't there. The NYIMP machinery catches (3) ↔ (1)/(2) mismatches at test time.

## Direct API still available for harness work

When the data DSL isn't enough (e.g., M3/M4 tests that build dictionary entries by hand), the Rust API stays:

```rust
let mut s = sess();
s.push(7);
s.call("dup_").unwrap();
assert_eq!(s.stack(), vec![7, 7]);  // stack() is top-first
let out = s.eval(": square dup * ;\n5 square .\nbye\n").unwrap();
```

`session.stack()` returns `Vec<i64>` top-first. `session.depth()` is the cell count. `session.reset()` rolls back to the post-bootstrap state.

## The return-stack juggle (the trap that bit us)

WF64 does not inline `>r`, `r>`, `r@`, etc. Every primitive is a `call` target, so every return-stack primitive has the caller's return address sitting on top of RSP when it runs. Pattern:

```masm
proc(to_r)              ; >r ( n -- )  ( r: -- n )
    pop     rcx         ; save our return addr
    push    TOS         ; push n onto rstack
    push    rcx         ; restore return addr (so ret works)
    mov     TOS, [DSP]  ; raise NOS into TOS
    stk(1, 0)
    next()
endp()
```

The `pop rcx … push rcx` brackets must wrap any RSP manipulation. Forget it and you `ret` to the value the user just pushed onto the rstack — instant crash, usually with `E06D7363` (Win32 C++ EH magic) in the dump.

When you see a crash on a `:` definition that calls `>r`/`r>`/`r@`, that's the lens to use first. There is also a tricky case where `r@` needs to skip past its own return address — `mov TOS, [rsp + cell]`, not `mov TOS, [rsp]`.

## Win64 callouts

For any call into Windows or into a Rust `extern "C"` runtime function, use `win64_call(target)`. It does the shadow-space + 16-byte alignment dance with **R12** as the RSP-save slot:

```masm
mov     r12, rsp
and     rsp, -16
sub     rsp, 32
call    &target
mov     rsp, r12
```

Never substitute R11 or any caller-saved register — Windows trashes them and your restore reads garbage. R12 is callee-saved; Windows preserves it. The kernel reserves R12–R15 for this kind of thing.

The Rust runtime functions are declared in `kernel/runtime_decls.masm` (`@extern rt_emit(1)` etc.) and resolved by `bind_externs`' host_resolver in `src/lib.rs` mapping the name to a `*mut c_void`.

## When stuck

- **The kernel crashed and you got a JASM crash dump** — RIP and stack are symbolic. Look at which `<proc+offset>` was running and what was on the stack. The dump catches `int 3` (non-fatal — execution continues) and access violations (fatal).
- **A test fails with the wrong stack contents** — `session.stack()` is top-first. Easy to invert mentally. Add a `dbg!(s.stack())` to confirm.
- **Test passes direct but fails via eval** — usually a return-stack issue (see above) or a primitive that worked at depth 0 but breaks when the compiled body has data above it on the stack.
- **`WF64_DUMP_ASM=1 cargo test …`** dumps the post-expansion asm text JASM hands to MC. Useful when a macro isn't expanding the way you expect.
- **`WF64_BOOT_INFO=1 cargo run`** prints region/HERE/LATEST addresses on boot.

## What NOT to do

- **Don't invent unnecessary substrate.** If you need a helper, prefer a colon definition or a Rust runtime fn before adding a new `proc`.
- **Don't hand-roll x86 encoding.** LLVM MC encodes every instruction. We write text.
- **Don't reach for `unsafe` blocks in the Rust harness to "fix" stack issues** — the bug is almost always in the .masm. Read the rstack-juggle section first.
- **Don't skip the test.** Every primitive ported needs a direct test in `tests/harness.rs`. The harness is fast (~0.2s for 40+ tests) — there's no excuse.

## See also

- `get-started.md` (project root) — current layout, milestone status, where to look first
- `docs/MILESTONES.md` — M1–M7 roadmap
- `E:\JASM\rust\USER-GUIDE.md` — the JASM language reference (directives, macros, scope rules, the lot)
- `E:\JASM\rust\README.md` — JASM architecture overview
- `src/lib.rs` and `tests/harness.rs` — the active behavioral contract for the system
