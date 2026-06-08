# JASM Forth-Aware Macro-Assembler Optimizer — Design v1

Status: design / proposal.
Author: WF64 project.
Scope: a Forth-aware optimizing lowering path for straight-line leaf
spans of colon words, built on JASM's existing macro/JIT pipeline and the
near code arena. Hybrid with the existing STC colon compiler, which stays
as bootstrap and as the fallback for everything the optimizer doesn't
handle.

---

## 0. TL;DR and the honest feasibility verdict

The vision — "JASM as a Forth-aware macro assembler that recognizes its
own `fop_` idioms and coalesces redundant stack traffic" — is buildable,
**but not at the layer the vision implicitly assumes.** JASM is *not* an
instruction-level assembler. It is a token-stream macro processor that
expands to MC-flavor **text** and hands that text to LLVM-MC for all
encoding (see §1). It has no `Instruction` IR, no operand model, no
register/memory abstraction. There is therefore nothing inside JASM today
that can "analyze the x86 it emits."

Two consequences drive the entire design:

1. **The Forth-aware pass cannot be a peephole over emitted x86.** There
   is no x86 in JASM's hands — only tokens that *spell* x86. Recognizing
   "a spill followed by a reload cancels" by pattern-matching token lines
   is brittle (whitespace, register aliases, `[rbp-8]` vs `[DSP-cell]`,
   imm folding) and re-derives information we *already have* at the
   moment we choose which `fop_` to emit.

2. **So the optimizer must run at the lowering layer, before text exists,
   as a Rust pass over an explicit `Vec<FopOp>` that WF64 builds.** WF64
   already controls the lowering (it decides, token by token, what each
   Forth word compiles to). We make that decision *abstract* — emit a
   typed op, not bytes — run a small stack-caching optimizer over the op
   vector, and only then render the surviving ops to `fop_*` macro calls
   (or raw asm lines) that go through the normal JASM expand→emit→MCJIT
   path into the near arena.

This keeps the *spirit* of the brief — a Forth-op macro library, a
Forth-aware coalescing pass that knows `rax=TOS`/`rbp=DSP`, one tight
native function per span placed in the arena — while putting the
intelligence where the structure actually lives (the typed op vector in
Rust), not where it has been erased (text, or worse, encoded bytes).

The closest-workable summary: **the `fop_` macros are real and live in
masm; the optimizer is a Rust pass over a typed op IR, not a token
peephole; the renderer emits the optimized ops as asm text through
JASM.** Section 3 states this precisely.

---

## 1. JASM's real architecture (grounded in the source)

Pipeline, from `E:\JASM\rust\src\asm\mod.rs` doc-comment and
`Assembler::assemble_with_path`:

```
text  --lex-->  Vec<Token>  --expand-->  Vec<Token>  --emit-->  String(MC asm)  --> Jit/LLVM-MC --> bytes
        (lex.rs)   (expand.rs: macros,       (emit.rs:                (jit.rs: MCJIT)
                    @if/@rept/@define,        concatenate tokens
                    @macro, Rust macros)      with spacing)
```

Concrete facts that pin the layer:

- **Token model is textual, not instructional.** `TokenKind`
  (`asm/token.rs`) is `Comment | Newline | Ident | Number | String |
  MacroParam | Directive | LocalLabel | Punct`. A line `mov [rbp-8], rax`
  is the token sequence `Ident("mov") Punct([) Ident("rbp") Punct(-)
  Number(8) Punct(]) Punct(,) Ident("rax")`. There is **no** notion of
  "instruction = mnemonic + operands", no register enum, no memory
  operand struct. (token.rs:32-79.)

- **`emit` just concatenates tokens to a string** honoring a per-token
  `space_before` bool; any leftover `Directive`/`MacroParam` token is an
  error. It does no semantic work. (emit.rs:73-112.)

- **All encoding is LLVM-MC's job, triggered at JIT finalize.**
  `Jit::add_asm` calls `LLVMAppendModuleInlineAsm`; the inline-asm parser
  runs only when `finalize()` materializes the module
  (jit.rs:262-272, 390-468). JASM never sees encoded bytes; it cannot,
  for example, "rewind HERE by 13" the way the kernel peephole does — that
  trick is only possible *at Forth runtime* because the kernel writes raw
  bytes into its own dict heap.

- **The one extension point that does arithmetic/logic over tokens is the
  Rust macro.** `Assembler::register_macro(name, closure)` installs a
  `FnMut(&mut RustMacroCtx) -> Result<(),String>`. The closure reads its
  args (`parse_int`, `parse_ident`, `nth_tokens`), reads expander state
  (`lookup_int`), and **emits final asm text** via `ctx.emit_line(s)`
  (whose output is *not* re-expanded). `stk(in,out)` is exactly this: it
  computes `(out-in)*cell` and emits one `add`/`sub rbp` line, or nothing
  (macros.rs:177-197; registered in WF64). This is the proof that
  "compute in Rust, format literal asm out" is the supported idiom — and
  it is the template for how the optimizer's *renderer* will emit.

- **`@macro ... @endmacro` is a pure token-substitution text macro**
  (expand.rs `handle_macro_def`, `expand_macro_call`; macros.rs
  `MacroDef`). It can hold the `fop_` body templates, but it cannot
  inspect or cancel anything — it only pastes tokens.

- **The arena is real and is the placement target.** `CodeArena` +
  `Jit::new_in_arena` route every MCJIT section allocation into a host-
  owned RWX region within rel32 reach of the kernel (jit.rs:118-243), and
  `code_header` reserves the per-function xt back-offset cell so a
  compiled function is a first-class dict word (jit.rs:126-148). WF64
  already drives exactly this in `rt_code_compile_body`
  (`src/runtime.rs:2694-2796`): it wraps a body in `proc/endp`, assembles
  through the shared `Assembler` preloaded with `macros.masm`, JITs into
  the arena, and the kernel's `CODE:` points the xt straight at `fn_addr`
  and writes the back-offset cell (`kernel/compile.masm:2336-2389`).

**Verdict on the three candidate layers from the brief:**

- (a) *peephole over macro-expanded text* — possible but fragile and
  redundant; rejected as the primary mechanism (see §0).
- (b) *a JASM internal instruction model* — does not exist; building a
  full x86 model inside JASM is a large project that duplicates LLVM-MC
  and is explicitly *not* what JASM is for.
- (c) *a small instruction model* — yes, but we build it **in WF64's
  lowering, as a tiny typed Forth-op IR (`FopOp`)**, not as a general x86
  model. This is the chosen layer.

---

## 2. The `fop_` template library (masm)

The `fop_` macros are the inline body templates of the primitives,
written once in a new `kernel/fops.masm` and `@include`-able by the
optimizer's generated source (and usable by hand in `CODE:` bodies). Each
is a JASM `@macro` that pastes the exact instruction text the kernel's
`inline_*_comp` / `fold_*_comp` helpers emit as bytes today — verified
against `kernel/compile.masm`.

These macros are the **fallback / reference encoding**. The optimizer
normally renders optimized asm directly (§4.4), but every `FopOp` has an
equivalent `fop_` macro so that (i) an unoptimized span can be emitted by
simply concatenating `fop_` calls, and (ii) the macros are independently
testable against the STC bytes.

Register/ABI (from `kernel/macros.masm`): `TOS=rax`, `DSP=rbp` (points at
NOS, grows down, push = `sub rbp,8`), `UP=rbx`, `cell=8`,
NOS=`[rbp]`, NNOS=`[rbp+8]`.

```asm
; kernel/fops.masm  —  Forth-op inline body templates.
; Each macro is the verbatim inline body of one primitive, matching the
; bytes kernel/compile.masm's inline_*_comp helpers emit.  No `next()`;
; these are concatenated into a larger function.

; ── stack shuffles ────────────────────────────────────────────────
@macro fop_dup()                 ; ( a -- a a )   inline_dup_comp
    mov  [rbp - 8], rax
    sub  rbp, 8
@endmacro

@macro fop_drop()                ; ( a -- )       inline_drop_comp
    mov  rax, [rbp]
    add  rbp, 8
@endmacro

@macro fop_swap()                ; ( a b -- b a ) inline_swap_comp
    mov  rcx, [rbp]
    mov  [rbp], rax
    mov  rax, rcx
@endmacro

@macro fop_over()                ; ( a b -- a b a ) inline_over_comp
    mov  [rbp - 8], rax
    mov  rax, [rbp]
    sub  rbp, 8
@endmacro

; ── literal ───────────────────────────────────────────────────────
; fop_lit(N): push immediate.  The kernel's STC form is `call do_lit;
; .quad N`; the inline form is the spill+load do_lit performs minus the
; call boundary.  N is a JASM expression (the optimizer substitutes a
; concrete number).
@macro fop_lit(n)                ; ( -- N )
    mov  [rbp - 8], rax
    mov  rax, &n                 ; mov rax, imm64 (MC picks imm32 when it fits)
    sub  rbp, 8
@endmacro

; ── binary arithmetic (consume NOS op TOS -> TOS) ─────────────────
@macro fop_plus()                ; ( a b -- a+b )
    add  rax, [rbp]
    add  rbp, 8
@endmacro

@macro fop_minus()               ; ( a b -- a-b )   a-b, a=NOS b=TOS
    mov  rcx, [rbp]
    sub  rcx, rax
    mov  rax, rcx
    add  rbp, 8
@endmacro

@macro fop_and()
    and  rax, [rbp]
    add  rbp, 8
@endmacro
@macro fop_or()
    or   rax, [rbp]
    add  rbp, 8
@endmacro
@macro fop_xor()
    xor  rax, [rbp]
    add  rbp, 8
@endmacro
@macro fop_star()                ; ( a b -- a*b )
    imul rax, [rbp]
    add  rbp, 8
@endmacro

; ── immediate-folded arithmetic (TOS op= imm) ────────────────────
; Emitted when the optimizer has constant-folded an adjacent literal.
; Matches fold_plus_comp / fold_times_comp byte choices (imm8 vs imm32
; selection is left to MC; we just write the mnemonic).
@macro fop_plus_imm(n)   add  rax, &n   @endmacro
@macro fop_minus_imm(n)  sub  rax, &n   @endmacro
@macro fop_star_imm(n)   imul rax, rax, &n  @endmacro
@macro fop_and_imm(n)    and  rax, &n   @endmacro
@macro fop_or_imm(n)     or   rax, &n   @endmacro
@macro fop_xor_imm(n)    xor  rax, &n   @endmacro

; ── memory ────────────────────────────────────────────────────────
@macro fop_fetch()               ; ( a -- [a] )    @
    mov  rax, [rax]
@endmacro

@macro fop_store()               ; ( x a -- )      !   a=TOS x=NOS
    mov  rcx, [rbp]              ; x
    mov  [rax], rcx
    mov  rax, [rbp + 8]          ; raise: new TOS
    add  rbp, 16
@endmacro

@macro fop_one_plus()  add rax, 1  @endmacro   ; 1+
@macro fop_one_minus() sub rax, 1  @endmacro   ; 1-
```

Notes:

- `fop_fetch` is `mov rax,[rax]` — net stack effect zero; this is the
  bare-`@` body nobody wrote an inline helper for today (the brief calls
  this out). Adding it as an `fop_` is trivial precisely because the op
  vector, not a byte-patcher, decides to use it.
- `fop_minus`/`fop_store` need a scratch (`rcx`) because the result isn't
  in the accumulator's natural position; `rcx` is scratch per the ABI.
- The "exact spill/reload idiom" the optimizer keys on is the **`fop_dup`
  tail** (`mov [rbp-8],rax ; sub rbp,8`) immediately followed by a
  **binary-op head** (`... [rbp] ; add rbp,8`). At the *op* level this is
  `Dup` then `Plus`; the optimizer cancels it structurally (§4), never by
  scanning these two lines of text.

---

## 3. The lowering and the pass pipeline

### 3.1 Where lowering happens, and the key data-flow constraint

WF64 **does not retain a colon word's source token list** — a colon word
is only compiled bytes in the dict heap (confirmed: `dict.masm` has
`>body` for CREATE words but no source/token retention; the STC compiler
in `compile.masm` writes bytes and moves on). Therefore:

> **The primitive sequence for a span must be captured *as it is being
> compiled*, by intercepting the token-by-token compile dispatch — it
> cannot be recovered from the emitted bytes afterward.**

The interception point is the compile-state branch of the dispatch loop
in `kernel/interp.masm` (`interpret_source`, the `.compile_ct` path, and
the literal/number paths) — equivalently, the `compile_word` →
`>comp`/`perform` dispatch in `compile.masm`. Today each found word in
compile state immediately runs its `dh_comp` helper (default
`compile_comma` = emit `call`; or `fold_*_comp`; or `inline_*_comp`). We
introduce a **span accumulator** in front of that.

### 3.2 Two implementation options for the accumulator — chosen: Rust

**Option A (kernel-resident accumulator, masm):** add a compile-time
buffer in the user area that records a typed op per compiled
word/literal; flush it through an optimizer written in masm. Rejected:
re-implements list manipulation and a stack-cache analysis in hand asm;
this is exactly the brittleness we are trying to leave behind, and it
cannot reach JASM/LLVM-MC for encoding.

**Option B (Rust-resident accumulator + optimizer + JASM renderer):**
chosen. A new Rust runtime entry point owns the op vector, the optimizer,
and the renderer; it reuses the existing `CODE:`/`rt_code_compile_body`
machinery to assemble and place the result in the arena.

Concretely, the hybrid colon compiler gains a mode (call it the
*optimizing span collector*). While collecting a straight-line leaf span,
each compiled token, instead of emitting bytes, calls a new runtime
shim `rt_fop_push(up, op_tag, imm)` that appends a `FopOp` to a
thread-local `Vec<FopOp>` (mirroring how `rt_code_compile_body` is a
runtime worker the kernel calls via `win64_call`). At a span boundary
(§5) the kernel calls `rt_fop_flush(up)`, which runs the pass and emits
either:

- a near-arena function for the span (xt patched into a `call rel32` at
  HERE, exactly like an inlined `CODE:` fragment placed mid-body), or
- nothing special — for a trivial/empty span it falls back to STC.

The op IR:

```rust
// crate: wf64 (src/fop.rs), used by runtime.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FopOp {
    Lit(i64),
    Dup, Drop, Swap, Over,
    Plus, Minus, Star, And, Or, Xor,
    Fetch, Store,
    OnePlus, OneMinus,
    // immediate-folded forms produced by the optimizer, never by the front end:
    PlusImm(i64), MinusImm(i64), StarImm(i64),
    AndImm(i64), OrImm(i64), XorImm(i64),
    // an opaque call to a non-fop word that the span tolerates only at its end:
    // (not stored mid-span; see §5 — calls END a span)
}
```

The front-end op set is small and closed — it is exactly the primitives
for which we have `fop_` templates. Any word *not* in this set ends the
span and is threaded as a normal STC `call` (§5). This is what keeps the
pass tractable: it only ever reasons about a handful of known idioms,
honoring the brief's "JASM recognizes ITS OWN idioms" — here, WF64
recognizes its own ops.

### 3.3 Pass pipeline (end to end)

```
compile-state token  ─►  span collector (kernel)  ─►  rt_fop_push  ─►  Vec<FopOp>
                                                                          │
                              span boundary (kernel calls rt_fop_flush)   ▼
                                                          ┌───────────────────────────┐
                                                          │  Forth-aware optimizer     │  (Rust, §4)
                                                          │  - abstract stack model    │
                                                          │  - push/pop cancel         │
                                                          │  - const-fold lit into op  │
                                                          │  - dead-store elim         │
                                                          └───────────────┬───────────┘
                                                                          │ optimized Vec<FopOp>
                                                                          ▼
                                                          render to asm text (fop_* / raw)
                                                                          ▼
                                              JASM Assembler.assemble  (expand → emit)
                                                                          ▼
                                                       LLVM-MC encode  →  near CodeArena
                                                                          ▼
                                           kernel patches `call rel32`(span_fn) at HERE
```

The **only** new JASM-layer artifacts are the `fop_*` macros (§2) plus
the renderer's generated `proc`-wrapped source — both ordinary inputs to
the existing pipeline. We add **no** new pass *inside* JASM. The
"Forth-aware optimize" box is a Rust function on `Vec<FopOp>`. This is the
explicit answer to the brief's "state the layer": **the optimizer runs in
Rust on a typed op vector; JASM/LLVM-MC do encoding only.**

### 3.4 How literals and control flow are represented

- **Literals:** a number in compile state pushes `FopOp::Lit(n)` instead
  of compiling `call do_lit; .quad n`. The optimizer folds it into an
  adjacent op (`Lit(n) Plus → PlusImm(n)`) or, if it survives, the
  renderer emits `fop_lit(n)`.
- **Control flow (IF/THEN/BEGIN/loops), calls to non-fop words, locals,
  `;`/EXIT:** these are **span terminators** (§5). They are *not*
  represented in `FopOp`. The collector flushes the current span (so its
  cached TOS/NOS is spilled to the in-memory stack), then the existing
  STC immediate words (`if_word`, etc.) run unchanged and emit their
  bytes into the dict body as they do today. The optimizer never sees a
  branch.

---

## 4. The stack-caching model and coalescing rules

The optimizer is a single forward pass that maintains an **abstract data
stack** of symbolic cells, tracking which cells are *register-resident*
(currently the cached TOS, i.e. `rax`) vs *spilled* (already in memory at
a known `[rbp+k]`) vs *literal* (a known constant not yet materialized).
This is textbook stack caching with a 1-deep register cache (TOS in
`rax`), matching the kernel's actual invariant.

### 4.1 Abstract state

```rust
enum Cell {
    Tos,            // lives in rax right now (the single cached cell)
    Mem(i32),       // lives in memory at [rbp + off] (off in bytes)
    Const(i64),     // a literal not yet placed anywhere
}
struct AbsStack {
    cells: Vec<Cell>,   // top of vec = top of Forth stack
    rbp_delta: i32,     // net change to rbp not yet emitted (lazy)
}
```

Two ideas make the coalescing fall out for free:

1. **Lazy `rbp`.** We do not emit `sub rbp,8`/`add rbp,8` as we go. We
   track `rbp_delta` and the memory offsets of spilled cells abstractly,
   and emit **one** net `rbp` adjustment when forced (span end, or before
   an op that must touch real memory at a settled offset). A push then a
   pop net to `rbp_delta == 0` and **emit no `rbp` traffic at all** — this
   is the structural form of "spill `mov [rbp-8],rax; sub rbp,8` followed
   by reload `mov rax,[rbp]; add rbp,8` cancels."

2. **Top-of-stack is symbolic.** `Dup` does not emit a spill; it pushes a
   second `Tos`-aliased cell abstractly. The spill is emitted only when a
   *second* distinct value needs `rax`, i.e. lazily, and only if the
   duplicated cell is still live by then.

### 4.2 Per-op transfer functions (the rules)

For each `FopOp`, update `AbsStack` and *defer* emission. Emission is
materialized by a `flush_to(op)` that forces just enough state to be
concrete for `op` to execute, then records the concrete asm.

- **`Lit(n)`** → push `Const(n)`. Emits nothing yet.
- **`Dup`** → duplicate the top `Cell` entry (push a copy of `cells.last`).
  Emits nothing yet. (If top is `Tos`, we now have two `Tos` entries — a
  later forcing of one spills `rax`.)
- **`Drop`** → pop top entry.
  - if it was `Const`/an unspilled duplicate → emits nothing (dead value
    never materialized);
  - if it was `Tos` and the new top is a `Mem(k)` → the new TOS must be
    reloaded: defer a `mov rax,[rbp+k]` + fold the `rbp` raise into
    `rbp_delta`.
- **`Plus`/`And`/`Or`/`Xor`/`Star`** (commutative-ish accumulate) →
  pop two operands `b`(top)`,a`. Cases:
  - `a=Const(x), b=Const(y)` → push `Const(x⊕y)` (full constant fold,
    nothing emitted);
  - one operand `Const(c)`, other is the live TOS → push `Tos`, record
    the **immediate-folded** op (`PlusImm(c)` etc.): `add rax, c`;
  - `a=Mem(k), b=Tos` (the canonical `dup +` / `over +` shape) → push
    `Tos`, emit `add rax,[rbp+k]`, and **drop the consumed memory cell**
    by adjusting `rbp_delta` (the raise) — *no spill, no reload ever
    emitted*. This is the headline collapse.
- **`Minus`/`Store`/`Fetch`** → analogous, using `rcx` scratch where the
  result isn't accumulator-natural (`Minus`, `Store`) or pure register
  rewrite (`Fetch` = `mov rax,[rax]`, stack shape unchanged).
- **`Swap`** → swap the top two `cells` entries. If both are already
  `Mem`/`Const`, emits nothing (pure relabel). If one is `Tos`, force the
  classic 3-move swap only when a later op needs both concrete.
- **`Over`** → push a copy of the second cell; materialization deferred
  like `Dup`.

### 4.3 The three named rules, restated precisely

- **push/pop cancel:** any sequence that raises then lowers the abstract
  stick with no intervening *forced* use of the spilled slot nets
  `rbp_delta` back and emits zero `rbp` instructions and zero spill/reload
  `mov`s. (Subsumes kernel `inline_dup_comp`+`fold`/`bare-+` cases.)
- **redundant TOS reload elimination:** a value already known to be in
  `rax` (`Cell::Tos`) is never re-`mov`'d into `rax`; consumers read it in
  place.
- **dead store elimination:** a `Const`/duplicate cell that is `Drop`ped
  (or overwritten) before being forced to memory emits nothing.
- **constant folding:** `Const op Const → Const`; `Const op Tos →
  *_imm`. Out-of-range immediates (don't fit signed imm32) fall back to
  materializing the literal then the register-op form — same policy the
  kernel folds use (`try_fold_literal` checks imm32 fit).

### 4.4 Renderer

After the pass, the surviving `(op, concrete-asm)` records are emitted as
asm text. The renderer prefers **direct lines** (it already computed the
exact instruction and offset) and uses `fop_*` macros only for the
unoptimized fallback path. A single net `rbp` adjustment (from the final
`rbp_delta`) and any forced spills at the span boundary (§6) are appended.
The text is wrapped in `proc(span_fn_k)/endp()` and assembled via the
existing `CODE:`-style path into the arena.

### 4.5 Worked examples

Forth stack convention in comments: rightmost = TOS.

**(a) `: foo dup + ;`** — op stream `[Dup, Plus]` (then span end at `;`).

Front-end ops: `Dup, Plus`.
Pass trace (start: `cells=[Tos]`, `rbp_delta=0`):
- `Dup`: `cells=[Tos, Tos]`. Emit nothing.
- `Plus`: pop `b=Tos`, `a=Tos`. Both are the same live value in `rax`.
  Special case: `a` is the duplicated TOS; to add TOS to itself we need
  one copy in memory or use `add rax,rax`. The pass recognizes
  `Dup;Plus` (top two cells both alias the single `rax`) and emits the
  optimal `add rax, rax`. push `Tos`. `rbp_delta=0`.

Optimized asm for the span body:
```asm
add  rax, rax        ; was: dup (8B spill+adj) + plus (call or add [rbp]+adj)
```
Span end (`;`): nothing cached to spill beyond TOS (TOS stays in `rax`,
which is the live-stack invariant); emit `ret`.

Comparison: STC today compiles `dup` inline (8 bytes) + `+` as a `call
plus` (5 bytes) + the `plus` body cost, or with the dup+fold path still a
spill/reload pair. Optimized: **3 bytes, no memory traffic.**

> Note on the headline "`dup +` → `add rax,[rbp]; add rbp,8`": that is the
> collapse for `over +`-shaped code where the second operand is a genuine
> NOS *memory* cell. For literal `dup +` (same value twice) the even
> better `add rax,rax` applies. Both are produced by the same op-level
> rules; the difference is whether the second operand cell is `Mem` or an
> alias of `Tos`. The brief's example asm is the `Mem` case; we get it for
> `over +`, and something strictly better for `dup +`.

**(b) `: bar 5 * 2 + ;`** — op stream `[Lit(5), Star, Lit(2), Plus]`.

Wait — `5 *` means "multiply NOS by 5". For a *standalone* `: bar 5 * 2 +
;` the input NOS is the caller's TOS. Trace (start `cells=[Tos]`):
- `Lit(5)`: push `Const(5)`. `cells=[Tos, Const(5)]`.
- `Star`: pop `Const(5)`, `Tos`. One const, other live → `StarImm(5)`:
  emit `imul rax, rax, 5`. push `Tos`.
- `Lit(2)`: push `Const(2)`.
- `Plus`: pop `Const(2)`, `Tos` → `PlusImm(2)`: emit `add rax, 2`.

Optimized span body:
```asm
imul rax, rax, 5
add  rax, 2
```
Two instructions, zero stack memory traffic, zero calls. (STC today: two
literal folds *if* the kernel's `fold_times`/`fold_plus` fire — but those
require the literal to immediately precede the op as `call do_lit;.quad`,
and they still went through the byte-rewind dance. Here it is a direct
consequence of the abstract model.)

**(c) `: g @ 1+ ;`** — op stream `[Fetch, OnePlus]`.

Trace (start `cells=[Tos]`):
- `Fetch`: TOS holds an address; `@` = `mov rax,[rax]`. Stack shape
  unchanged (`cells=[Tos]`). Emit `mov rax, [rax]`.
- `OnePlus`: `add rax, 1`. (Recognized as `OnePlus`, or as `Lit(1) Plus`
  folded to `PlusImm(1)` — same result.)

Optimized span body:
```asm
mov  rax, [rax]
add  rax, 1
```
Compared to STC `call @ ; call 1+` (two calls) or even inline-`@` (which
doesn't exist today) + fold. Tight, branchless, callless.

---

## 5. The hybrid boundary with STC

The optimizer owns **only** maximal straight-line leaf spans of
*fop-eligible* primitives. Everything else stays on the proven STC path.

### 5.1 What starts/extends/ends a span

A span is a run, in compile state, of tokens that each map to a `FopOp`
in the closed set (§3.2). The span **ends** (collector flushes, optimizer
runs, span function is emitted and threaded with a `call rel32`) at the
first of:

1. **a word with no `fop_` mapping** — it is threaded as a normal STC
   `call` after the flush;
2. **any immediate control-flow word** — `if/then/else/begin/while/
   repeat/again/until/do/?do/loop/+loop/leave/recurse/ahead` — these run
   their existing STC byte-emitters after the flush; the optimizer never
   sees a branch or a merge;
3. **a locals reference or `{ ... }` locals declaration** — the kernel's
   locals path (`check_local_emit_word`, `locals.masm`) emits its inline
   fetch/store; flush first;
4. **`;` / `EXIT`** — flush, then the normal `semicolon`/`exit_word` runs
   (including its tail-call patch, see §5.3);
5. **a span length / arena budget cap** (defensive).

### 5.2 Calls to non-compiled / forward-referenced words

A reference to a word that is not in the fop set (the common case for
user-defined words and most primitives) ends the span and threads a
`call rel32` to that word's xt — identical to today's `compile_comma`. A
forward reference (word not yet defined) is impossible in this Forth
(definitions are visible only after creation), so there is no patch-list
to maintain; a not-yet-fop word simply isn't fop-eligible and threads a
call. The optimizer therefore never needs relocation logic.

### 5.3 Tail calls

Tail-call optimization (`;`/`EXIT` patching a trailing `E8` call into
`E9` jmp) is a property of the **STC byte stream**, applied by
`semicolon`/`exit_word` to the last emitted `call`
(`compile.masm:2198-2252`, keyed on `user_TAIL_CALL`). When a span ends
the colon body with a threaded call to a non-fop word, that call is the
tail-call candidate and TCO works unchanged. When a span *is* the tail
(ends in fop ops then `;`), the span function ends in its own `ret`; the
`call rel32 span_fn` we emitted into the body is itself a tail-call
candidate, so `;` patches it to `jmp span_fn` and `span_fn`'s `ret`
returns to the colon word's caller directly. We set `user_TAIL_CALL` to
HERE-after-the-span-call so the existing TCO logic fires with no change.

### 5.4 Locals interaction

`R15=LP` is reserved; a colon def with locals has an active frame.
Locals references are span terminators (§5.1.3), so the optimizer never
manipulates the locals stack. The span function it emits is a leaf with
respect to LP (it touches only `rax`/`rbp`/`rcx`), so it composes with the
frame the surrounding STC body manages. Locals release on `;`/`EXIT`
(`emit_locals_release`) is unaffected.

---

## 6. Correctness invariant

The non-negotiable invariant that makes the hybrid safe:

> **Boundary materialization invariant.** At every point where control
> may leave the optimized span — span end, immediately before a threaded
> `call`, before any branch/loop word, before a locals op, and at the
> word's `ret` — the *in-memory data stack and `rbp` must be in exactly
> the canonical kernel encoding* (TOS cached in `rax`; `[rbp]`=NOS;
> `rbp` = SP0 ± depth·cell per `macros.masm`), as if every op in the span
> had executed via its STC body.**

Operationally the optimizer guarantees this by a **`force_settle()`** run
before emitting any span terminator: it materializes every abstract
`Const`/duplicated/`Mem`-relabeled cell into its canonical memory slot,
emits the single net `rbp` adjustment for `rbp_delta`, and leaves the
logical TOS in `rax`. After `force_settle()` the abstract stack is empty
of deferrals and the memory image is bit-identical to the STC result.

Why this is sufficient:

- Any observer of the stack (a threaded call into a non-fop word, a
  branch target, the interpreter after `;`, the host at the `forth_main`
  boundary) only ever runs *between* spans, i.e. after a `force_settle()`.
- Within a span there is no observer: it is straight-line, leaf, single-
  entry/single-exit, and touches only `rax`/`rbp`/`rcx`(scratch).
- Branches/merges are never inside a span, so there is no merge point at
  which two different abstract states could disagree — the classic stack-
  caching merge problem is *avoided by construction* rather than solved.

`rcx` is the only scratch used and is dead across span boundaries (ABI
scratch), so no save/restore is needed. `xmm` and the FP stack are
untouched by the integer fop set.

---

## 7. Phased plan

### Phase 0 — `fop_` macros + golden bytes (no optimizer yet)

- Add `kernel/fops.masm` with `fop_dup/drop/swap/over/plus/lit/fetch/
  store` (§2).
- Test: assemble each `fop_*` via the existing `Assembler` and assert the
  encoded bytes equal the bytes the corresponding `inline_*_comp` helper
  emits (read both, compare). This validates the templates independently
  of any optimization. (Reuses `tests/harness.rs` session; or a small
  `wfasm`-level unit test.)

### Phase 1 — smallest provable optimizer slice: `dup +`

The minimal end-to-end vertical slice the brief asks for:

1. `FopOp` enum with `{Lit, Dup, Plus}` only; `Vec<FopOp>` + `rt_fop_push`
   / `rt_fop_flush` runtime shims (src/fop.rs + src/runtime.rs), reusing
   `rt_code_compile_body`'s arena/assemble plumbing.
2. Span collector hook in the compile dispatch that recognizes exactly
   `dup` and `+` (and integer literals) as fop-eligible; everything else
   flushes.
3. The optimizer with just rules: push/pop cancel, `Dup;Plus → add rax,
   rax`, `Lit(c);Plus → add rax,c`, and `force_settle`.
4. Renderer + arena placement + `call rel32` patch.

**Provable acceptance:**
- *Behavioral parity:* `: foo dup + ;` then `5 foo .` prints `10`, via
  `eval()` in `tests/harness.rs`; compare against the STC build of the
  same word (compile it both ways behind a flag, run both, assert equal
  results across a value table incl. negatives and overflow edge cases).
- *Tightness:* disassemble/inspect the span function's bytes and assert
  it is `add rax, rax ; ret` (3+1 bytes), and assert the colon body
  contains a single `call rel32`/`jmp rel32` to it. Measure code size vs
  STC and record the delta.
- *Hybrid safety:* `: baz dup + 1 if 7 then ;`-style word (span then
  control flow) returns correct results, proving `force_settle` at the
  span/branch boundary.

### Phase 2 — widen the op set

Add `drop/swap/over/minus/star/and/or/xor/fetch/store/1+/1-` and the
`*_imm` folds; extend tests with examples (b) and (c) and a randomized
differential tester (random short fop-only colon bodies, run STC vs
optimized over random stacks, assert identical final stacks). This is the
strongest correctness lever: **differential testing against the STC
oracle.**

### Phase 3 — measurement + gating

- Code-size and instruction-count report per word (optimized vs STC).
- A runtime flag / declarator to opt a definition in or out (mirrors the
  existing `inline` declarator), so regressions can be bisected.
- Decide default-on policy only after the differential suite is green on
  a large corpus (the demos in `demos/`, `lib/*.f`).

### Test strategy summary

1. **Template golden bytes** (Phase 0).
2. **Behavioral parity via `eval()`** for each worked example.
3. **Differential STC-oracle fuzzing** over random fop-only spans and
   random initial stacks — the core guarantee.
4. **Boundary tests** mixing spans with control flow, calls, locals, and
   tail position.
5. **Size/perf metrics** recorded, not asserted (informational gate).

---

## 8. Risks and open questions

**Biggest risk — the span collector must perfectly preserve STC
observable state at every boundary.** The whole safety argument rests on
`force_settle()` reproducing the exact kernel stack encoding (TOS in
`rax`, `[rbp]`=NOS, precise `rbp`) before any observer. A single
off-by-one in `rbp_delta` accounting or a missed materialization of a
deferred cell corrupts the data stack *silently* and only for some
inputs. Mitigation: differential fuzzing against the STC oracle (Phase 2)
is mandatory and gates default-on; `force_settle` has an assertion mode
that, in debug builds, also emits the STC byte sequence into a scratch
buffer and compares post-state.

Other risks / open questions:

- **Capturing the op stream cleanly.** The dispatch loop in
  `interp.masm` is intricate (empty-stack normalization, trace hooks,
  locals pre-check). Inserting the collector without perturbing those
  paths needs care; the safest hook is at `compile_word`/`>comp` dispatch
  rather than deep in `interpret_source`. Open: do we route via a new
  `dh_comp` helper (`fop_collect_comp`) bound to fop-eligible words, so
  the change is *data* (which words point at the collector) not control-
  flow surgery? This looks cleanest and is the recommended approach —
  fop-eligible primitives get `dh_comp = fop_collect_comp`; the collector
  decides span membership; non-fop words keep `compile_comma` and thereby
  *are* the flush trigger.

- **Per-span JIT cost.** Each span is a separate `Assembler.assemble` +
  MCJIT finalize (whole-module per jit.rs:18-22). For a word with many
  spans this multiplies fixed JIT overhead at *compile* time. Open:
  batch a whole colon word's spans into one module, or keep a warm
  assembler (the `CODE_ASSEMBLER` thread-local already does the latter).
  Runtime cost is unaffected; this is purely `:`-time latency.

- **Is the per-span `call rel32` worth it for tiny spans?** A 3-byte
  `add rax,rax` reached by a 5-byte `call`/`jmp` is *bigger* than inlining
  those 3 bytes directly into the dict body. Open question / likely
  refinement: for spans whose optimized body is below a threshold and is
  *position-independent* (no rel32 inside — true for all integer fops),
  emit the bytes **inline into the dict body** (like `inline_*_comp` does)
  instead of as a separate arena function. The optimizer's output is the
  same; only the placement differs. This may make the arena-function form
  the exception (used for larger spans) rather than the rule. Resolving
  this needs the Phase 1 size measurements.

- **Immediate-range fallback** must match kernel policy exactly
  (`try_fold_literal`'s signed-imm32 test) or folded results diverge for
  large constants. Covered by differential tests but called out as a
  known sharp edge.

- **Interaction with future float/FP ops and `LET`.** The current op set
  is integer-only and leaves the FP stack untouched; extending to FP ops
  would need the same treatment for `xmm`/FSP and is explicitly out of
  scope for v1.

- **No JASM-internal x86 model is being built** — by design. If a future
  need arises to optimize *hand-written* `CODE:` bodies (arbitrary x86,
  not our ops), this design does *not* cover it, and that would require
  the genuinely larger (b)-layer instruction model we deliberately
  declined. v1 optimizes only what WF64 itself lowers.
