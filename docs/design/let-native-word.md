# Design / work order: make `LET` a native Forth word (drop the trampoline)

**Status:** proposed. Hand-off spec for implementation.
**Owner:** (assign)
**Estimated size:** medium. Touches `src/let_lang/codegen.rs`, `src/runtime.rs`
(`rt_let_compile`), one JASM helper, and `kernel/main.masm`. No change to the
LET *language* (parser, grammar, semantics) — only its backend/ABI.

---

## 0. TL;DR

Today a `LET … END` form compiles to a stand-alone Win64 function in a separate
JIT module, and the Forth word's body is a **trampoline** that marshals the
Forth FP stack into that function's `(rcx, rdx)` ABI, `call`s it, and marshals
back. That trampoline is an ABI seam that has produced a string of
register-corruption bugs (it has clobbered RAX, R12, and — via the callee —
xmm6-15). A single call hides the damage; calling a LET word in a Forth **loop**
corrupts state every iteration and crashes.

Replace the seam: have the codegen emit a body that uses the **Forth ABI
directly** (reads/writes the Forth FP stack, ends in `ret`), and **copy the
assembled machine code into the Forth dictionary** so the word's xt points at it
directly — exactly like a hand-written kernel primitive (`fractal-iter`), just
MC-assembled instead of typed into a `.masm`. The trampoline disappears, and the
whole class of register-preservation bugs disappears with it. Bonus: a
pure-arithmetic LET becomes inline, zero-call-overhead code at hand-MASM speed.

---

## 1. Background: the WF64 Forth ABI (what any native word must honor)

From `kernel/macros.masm`:

| Reg  | Role                                             |
|------|--------------------------------------------------|
| RAX  | TOS — top of the **data** stack (cached in reg)  |
| RBP  | DSP — data stack pointer (points at NOS)         |
| RBX  | UP  — user-area / per-task base pointer          |
| RSP  | RP  — Forth return stack                          |
| R15  | LP  — locals stack pointer                        |
| RCX/RDX/R8-R11 | scratch (caller-saved); free to clobber|
| R12-R14 | avoid; callee-saved per Win64                 |
| xmm0-5  | scratch (caller-saved)                        |
| xmm6-15 | callee-saved per Win64                        |

A Forth word is entered by a `call` and must end by `ret` (the `next()` macro).
It may freely use RCX/RDX/R8-R11 and xmm0-5. It must leave RBP/RBX/RSP/R15 (and,
per the boundary contract in §3, xmm6-15) as it found them, and must only change
RAX/the data stack and the FP stack as its stack effect declares.

### The Forth FP (float) stack

A separate stack in the user area, pointer at `[UP + user_FSP]`
(`user_FSP = 0x1218`, see `kernel/macros.masm` / `RT_USER_FSP` in `runtime.rs`).
It **grows downward**: TOS is the **lowest** address.

- Pushing a double: `sub [UP+user_FSP], 8; movsd [new_fsp], xmm`.
- Popping: `add [UP+user_FSP], 8`.
- With N items on the FP stack, the top (last-pushed) is at `[FSP+0]`, the next
  at `[FSP+8]`, … the deepest at `[FSP+(N-1)*8]`.

Forth's calling convention for FP args/results: **the last-declared input is TOS
at call time; the last-declared output is TOS on return.** `fractal-iter`
(`kernel/igui_gfx.masm`) is the reference example of a primitive that pops doubles
off `[UP+user_FSP]`, computes in xmm, and adjusts FSP.

### HERE / the dictionary

`HERE` lives at `[UP + user_HERE]` (`RT_USER_HERE = 0x18`). The dictionary is
**executable** code space — the current LET trampoline and `CODE:` JMP stubs are
emitted there and run. New word bodies can be `memcpy`'d to HERE and executed.

---

## 2. How LET works today (code map)

- **`src/let_lang/parser.rs`** — lexer + parser for `LET (ins) -> (outs) =
  exprs WHERE … END`. (No change needed.)
- **`src/let_lang/codegen.rs`** — `lower(form, fn_name, libm_table) -> asm_text`.
  Emits Intel-syntax asm for a **Win64 function** `fn(rcx = *const f64 inputs,
  rdx = *mut f64 outputs)`:
  - Prologue `push r12; mov r12, rdx` (stash outputs ptr in callee-saved r12).
  - Loads input *i* from `[rcx + (n_in-1-i)*8]` into a **named xmm** (`xmm6+`;
    `FIRST_NAMED_REG`). Named values (inputs + WHERE bindings) live in xmm6-15
    so libm calls (which clobber xmm0-5) don't trash them.
  - Computes WHERE bindings (topo-sorted) and results using xmm0-5 as scratch.
  - Stores result *i* to `[r12 + (n_out-1-i)*8]`.
  - Epilogue `pop r12; ret`.
  - Appends a RIP-relative **const pool** and three 16-byte-aligned SSE mask
    constants (`$$sign_mask`, `$$abs_mask`, `$$one_bits`) referenced as
    `xmmword ptr [rip + …]` by `andpd`/`xorpd` (these instructions **require
    16-byte alignment** of the memory operand).
- **`src/runtime.rs` → `try_compile_let` / `rt_let_compile`** (~line 2408):
  reads the source up to `END`, calls `let_lang::compile`, JITs it into a fresh
  module via `Jit` (`add_asm` / `declare_fn` / `lookup_addr`), keeps the module
  alive in the `LET_JITS` thread-local, then calls `emit_let_trampoline`.
- **`src/runtime.rs` → `emit_let_trampoline`** (~line 2495): emits, **at HERE**,
  the Forth word body that bridges Forth↔the JIT'd function:
  ```
  mov rcx, [rbx + user_FSP]      ; rcx = FSP = inputs ptr
  lea rdx, [rcx + delta]         ; rdx = outputs ptr   (delta = (n_in-n_out)*8)
  mov r12, rsp ; and rsp,-16 ; sub rsp,32             ; align for Win64 call
  mov rax, fn_addr ; call rax    ; <-- the seam
  mov rsp, r12
  add [rbx + user_FSP], delta    ; pop n_in, push n_out
  ; ret (next)
  ```
- **JASM `Jit`** (`E:\JASM\rust\src\jit.rs`): public API is `new`, `add_asm`,
  `declare_fn`, `lookup_addr(name) -> u64`. **It returns addresses, not bytes.**

`CODE:` (`rt_code_compile_body`, runtime.rs ~2665) uses the same pattern: JIT to
a far module, then a 12-byte `JMP` trampoline at HERE. Neither LET nor CODE:
currently copies the assembled bytes into the dictionary — that capability is
what this work adds (and CODE: could later reuse it).

---

## 3. The three bugs in the trampoline approach (why we're replacing it)

The trampoline is the host's Win64 world calling into a Win64 function, wedged
inside Forth's world, and it has leaked host/Forth state three ways:

1. **RAX (TOS) clobbered.** `mov rax, fn_addr; call rax` destroys the cached
   data-stack TOS; never restored. *(Patched: `push rax`/`pop rax` added.)*
2. **R12 clobbered.** `mov r12, rsp` trashes a callee-saved reg; never restored.
   *(Patched: `push r12`/`pop r12` added.)*
3. **xmm6-15 clobbered — NOT yet handled.** The compiled function parks named
   values in xmm6-15 and never restores them. `kernel/main.masm` (the
   `forth_main` Forth↔host boundary) explicitly states *"All callee-saved GPRs
   are preserved; xmm6-15 are untouched"* — i.e. it does **not** save/restore
   xmm6-15; it assumes Forth never touches them. A LET word breaks that
   assumption, so after an eval the host's (GUI thread / Direct2D) callee-saved
   xmm6-15 are corrupt. This is the most likely cause of the
   *renders-then-crashes* behaviour in the GUI (D2D uses xmm6-15 heavily;
   headless single-call tests don't, which is why they pass).

Bugs 1 and 2 were point-patches. Bug 3 is the same disease: an ABI seam leaking
register state. Rather than keep patching, remove the seam.

---

## 4. The redesign: emit a Forth-ABI body and splice it into the dictionary

### 4.1 New codegen ABI (`codegen.rs`)

Emit the word body to use the Forth FP stack **directly**, no `(rcx,rdx)` args,
no `push r12`, no Win64 prologue (for the no-libm case):

```
    .intel_syntax noprefix
    .text
    .p2align 4                      ; 16-align the function start (see §4.3)
    .globl <fn>
<fn>:
    mov   rcx, [rbx + user_FSP]     ; rcx = FSP (rcx is Forth scratch — free)
    ; load inputs (same offsets as before): input i at [rcx + (n_in-1-i)*8]
    movsd xmm<r>, qword ptr [rcx + (n_in-1-i)*8]
    ...
    ; compute WHERE bindings + results in xmm (named -> xmm6+, scratch xmm0-5)
    ...
    ; store outputs back INTO the FP-stack region; output i at
    ;   [rcx + delta + (n_out-1-i)*8]  ==  [rcx + (n_in-1-i)*8]
    ; (outputs overwrite the now-dead input slots; safe because all inputs are
    ;  already loaded into xmm before the first store)
    movsd qword ptr [rcx + (n_in-1-i)*8], xmm<result_i>
    ...
    add   qword ptr [rbx + user_FSP], <delta>   ; delta = (n_in - n_out)*8
    ret
    ; <const pool + 16-byte-aligned masks, RIP-relative, exactly as today>
<fn>_end:                            ; trailing label — see §4.3 for length
```

Notes:
- `delta = (n_in - n_out)*8`. For mbrot (4 in, 3 out) delta = 8; outputs land at
  `[rcx+24]`, `[rcx+16]`, `[rcx+8]`; FSP += 8 makes `[rcx+8]` the new TOS. Worked
  example matches the current trampoline's semantics exactly.
- Registers used: **rcx only** (Forth scratch) + xmm. RAX/RBP/RBX/RSP/R15 are
  untouched. So bugs 1 & 2 are impossible by construction.
- **xmm6-15 (bug 3) still needs a home — see §4.4.**

### 4.2 libm-using LETs (sin/cos/sqrt/pow/…)

These still need a Win64 `call` to the absolute libm address inside the body.
Keep the existing approach (the codegen already bakes `mov rax, addr; call rax`
for libm and uses xmm6-15 to survive the volatile-xmm clobber), but the call now
happens **inside a Forth word**, so:
- Allocate shadow space + 16-align RSP around each libm call (RSP here is the
  Forth return stack — that's fine; it's a stack, restore it after).
- RBX(UP) is callee-saved, so libm preserves it; you can still read
  `[rbx+user_FSP]` after a libm call. (If you prefer, cache FSP in a Forth-scratch
  reg you reload, rather than relying on rcx surviving — libm preserves rbx, not
  rcx. **rcx is volatile across the libm call**, so reload `rcx = [rbx+user_FSP]`
  after libm calls, or keep the FSP base in a value re-derived from rbx.)
  ⚠ This is the one easy mistake: don't assume rcx survives a libm call.

### 4.3 Splicing the assembled bytes into the dictionary

The assembled blob is **position-independent**:
- Internal references (const pool, SSE masks) are **RIP-relative** → valid after
  a *contiguous* copy that preserves internal offsets.
- libm references are **absolute** (`mov rax, addr`) → valid after any copy.
- There are **no absolute self-references**. (Verify the codegen never emits
  one; today it doesn't.)

Therefore: assemble as today, then **`memcpy` the function's bytes to HERE** and
point the xt directly at HERE — no trampoline, no kept-alive JIT module.

Getting bytes + length out of JASM (whose `Jit` only exposes `lookup_addr`):

1. **Length without a JASM change (recommended):** emit a trailing label
   `<fn>_end:` after the const pool (as shown in §4.1). Then
   `len = lookup_addr("<fn>_end") - lookup_addr("<fn>")`. Read `len` bytes from
   the start address (JIT memory is readable) and `memcpy` to HERE.
2. **Or add a JASM API** `Jit::function_bytes(name) -> &[u8]` if you'd rather not
   rely on the end-label trick.

Alignment (critical): the SSE mask constants are accessed by `andpd`/`xorpd`,
which **fault on a non-16-byte-aligned memory operand**. The masks are
`.p2align 4` *within the module*; to keep them 16-aligned after the copy you must
(a) `.p2align 4` the **function start** too (so the masks' offset-from-start is a
multiple of 16), and (b) **align HERE up to 16** before the copy and use that as
the xt. Bump HERE past the copied length afterward.

After copying, the original `Jit` module is **no longer needed** — drop it
(don't push to `LET_JITS`). The dict copy is self-contained (RIP-relative
internals + absolute libm addrs that are stable for the process).

### 4.4 xmm6-15 preservation (bug 3) — pick ONE

The body uses xmm6-15 for named values. Options, recommended first:

- **(A) Save/restore xmm6-15 at the `forth_main` boundary (recommended).**
  Change `kernel/main.masm` so `forth_main` saves xmm6-15 on entry and restores
  on exit (10 × `movups` to reserved stack, ~once per eval). This upgrades the
  boundary contract from *"Forth must not touch xmm6-15"* to *"Forth may use
  xmm6-15"*, making **all** Forth FP code safe (not just LET), at negligible
  per-eval cost. Update the comment at `kernel/main.masm:95`.
- **(B) Save/restore in each LET word.** Prologue saves exactly the xmm6-15 it
  uses (the codegen knows the count = `next - FIRST_NAMED_REG`) to the return
  stack; epilogue restores. Per-call cost; defeats some of the speed win in
  tight loops. Avoid unless (A) is undesirable.

Recommendation: **(A).** It's cheaper in loops and fixes a latent hazard for any
future xmm6-15-using primitive.

### 4.5 `rt_let_compile` changes (`runtime.rs`)

Replace the `emit_let_trampoline` call with the splice:

```rust
// assemble as today -> fn_addr (start) ; also look up "<fn>_end"
let start = jit.lookup_addr(&compiled.fn_name)?;
let end   = jit.lookup_addr(&format!("{}_end", compiled.fn_name))?;
let len   = (end - start) as usize;
let bytes = std::slice::from_raw_parts(start as *const u8, len);

let mut here = read_u64(up + RT_USER_HERE);
here = (here + 15) & !15;                       // 16-align the xt
std::ptr::copy_nonoverlapping(bytes.as_ptr(), here as *mut u8, len);
write_u64(up + RT_USER_HERE, here + len as u64);
// `here` is the xt of the new word; the kernel CREATE/colon path wires it in.
// Do NOT push `jit` into LET_JITS — drop it; the dict copy is self-contained.
```

Delete `emit_let_trampoline` (and its byte-emit helpers if now unused). The
kernel-side `LET` immediate word keeps doing dict-header creation; it just points
the new header's code field at the aligned `here` from above instead of at a
trampoline.

---

## 5. Step-by-step plan

1. **xmm6-15 boundary fix (A)** in `kernel/main.masm` first, in isolation; it's
   independently correct and unblocks any xmm6-15 use. Re-run the suite.
2. **codegen.rs**: switch input-load/output-store/prologue/epilogue to the
   Forth-ABI form (§4.1); add `.p2align 4` at function start and the `<fn>_end:`
   trailing label. Keep the const-pool/mask emission unchanged.
3. **libm path** (§4.2): ensure shadow-space/alignment around libm calls and
   reload `rcx = [rbx+user_FSP]` after each libm call.
4. **runtime.rs**: replace trampoline emit with the byte-splice (§4.5); stop
   keeping LET JIT modules alive.
5. **Delete** `emit_let_trampoline` + the now-dead push/pop/FSP byte helpers.
6. **Tests** (§6).
7. Optionally, later: retarget `CODE:` to the same splice path and drop its
   12-byte JMP trampoline too.

---

## 6. Tests / acceptance criteria

The existing `let_dsl_*` tests in `tests/harness.rs` (and the `jit_compiles_*`
unit tests in `src/let_lang/mod.rs`) must still pass. Add the cases that the old
design never covered — they are the ones that actually exercised the bug:

1. **LET in a `DO` loop with a data-stack sentinel (the regression that bit us).**
   ```forth
   : msum  LET (a,b) -> (s) = a + b END ;
   : t  ( -- n )
       12345                       \ sentinel in TOS
       1000 0 do  3.0e 4.0e msum fdrop  loop
       ;                           \ TOS must still be 12345
   ```
   Assert TOS == 12345 and `depth`/`fdepth` are balanced after. (Fails today:
   the trampoline shreds TOS each iteration.)
2. **LET in a loop preserves xmm6-15 across the boundary.** Drive a render-shaped
   workload (many calls) and confirm no crash; if feasible, a unit test that sets
   a known value in an xmm6-15, calls a LET word in a loop, and checks it
   survived (or rely on the GUI demo below + the headless loop test).
3. **`fdepth`/`depth` balance** asserted after every LET test (the current mbrot
   test only checks the printed values, which is why an FP-stack imbalance would
   slip through — add the depth assertions).
4. **libm LET in a loop**: e.g. `: h LET (x,y)->(d)=hypot(x,y) END ;` called in a
   `DO` loop; assert results and stack balance (covers the rcx-after-libm pitfall
   in §4.2).
5. **End-to-end**: `demos/gfx-canvas-mandelbrot-let.f` renders the full
   640×480 fractal and the window stays up (the user-visible repro). Compare wall
   time against `demos/gfx-canvas-mandelbrot.f` (MASM `fractal-iter`) — the
   native-word LET should be in the same ballpark now that there's no per-call
   trampoline.

---

## 7. Risks & fallbacks

- **JIT memory readability / W^X.** Reading the assembled bytes from
  `lookup_addr` assumes that region is readable. It is today (MCJIT maps RX). If
  a future JASM hardens to X-only, switch to the `Jit::function_bytes` API
  (§4.3 option 2) which can return the bytes pre-load.
- **Const-pool alignment after copy.** The single most likely implementation
  bug. Verify with a LET that uses unary `-` or `abs` (exercises
  `xorpd/andpd [rip+mask]`) *called from the dict copy*; a misalignment faults
  immediately. Mitigation in §4.3 (align function start + align HERE).
- **rcx volatility across libm** (§4.2) — reload FSP after libm calls.
- **Dictionary space.** Each LET word now consumes its full body length in the
  dict (tens to low-hundreds of bytes) instead of a ~36-byte trampoline. Fine for
  the 128 MB region; just don't redefine LET words in a hot loop.
- **Fallback if splicing proves troublesome.** An intermediate that still kills
  the bug class without dict-copying: keep a *one-instruction* `jmp <fn>` stub at
  HERE (like CODE:) but make `<fn>` itself the **Forth-ABI body** from §4.1
  (FP-stack-direct, `ret`). That removes all register marshalling (bugs 1-3 via
  §4.4) without needing byte extraction. The full dict-copy (§4.3) is the
  preferred end state because it also enables near/inline calls and drops the
  kept-alive JIT modules.

---

## 8. Why this is worth it

- Eliminates an entire bug class (host/Forth register leakage) by construction,
  not by patching.
- A pure-arithmetic LET becomes inline, zero-call-overhead native code — the same
  class as the hand-written MASM `fractal-iter`, but written in infix algebra and
  MC-assembled. That's the LET value proposition fully realized: *fast float
  math, safe to call anywhere, including hot loops.*
