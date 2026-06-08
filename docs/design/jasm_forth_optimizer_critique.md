# Adversarial Critique — JASM Forth-Aware Macro-Assembler Optimizer v1

Reviewer: adversarial design critic (no prior context).
Target: `docs/design/jasm_forth_optimizer_v1.md`.
Grounding: `E:\JASM\rust\src\asm\{mod,token,emit,macros}.rs`,
`E:\WF64\kernel\{compile,interp,execute,dict,locals,arith,stack,memory}.masm`,
`E:\WF64\src\{lib,runtime}.rs`.

Verdict up front: **sound-with-fixes**, but the doc oversells its own
novelty and contains at least one outright analysis error plus several
holes in the safety argument that must close before Phase 1. The
*engineering* is buildable and mostly correctly scoped; the *framing*
("JASM-aware macro assembler") is a fig leaf over what is, by the doc's
own architecture, a tiny Forth compiler in Rust.

---

## A. Claim verification (what the doc got right)

These are confirmed against source and are **not** complaints — recording
them so the human knows the foundation is real:

- **JASM is text-only, no instruction model.** Confirmed.
  `TokenKind` (`asm/token.rs:32-79`) is `Comment|Newline|Ident|Number|
  String|MacroParam|Directive|LocalLabel|Punct`; there is no register
  enum, operand struct, or instruction type. `emit()` (`emit.rs:73-112`)
  literally concatenates token text with a `space_before` bool and errors
  on any leftover `@`/`&` token. Encoding is entirely LLVM-MC's, invoked
  at JIT finalize. The doc's §1 is accurate.
- **`stk` is the precedent for "compute in Rust, emit literal asm."**
  Confirmed (`macros.rs:177-197`): it reads two ints, computes
  `(out-in)*cell`, and `emit_line`s one `add`/`sub rbp` or nothing. The
  doc's "renderer is a `stk`-shaped Rust macro" analogy holds.
- **`CODE:` → `rt_code_compile_body` → near arena → xt points at
  `fn_addr`.** Confirmed (`runtime.rs:2704-2796`, `compile.masm:2336-2389`).
  The reuse story for arena placement is real.
- **Per-primitive `dh_comp` is data, not control flow.** Confirmed
  (`lib.rs:1575-1636`): `dup_→inline_dup_comp`, `plus→fold_plus_comp`,
  etc., are a match table at registration. Binding fop-eligible words to a
  new `fop_collect_comp` is mechanically feasible exactly as §8 hopes.
- **The TCO invariant the doc leans on.** Confirmed: `semicolon`
  (`compile.masm:2198-2227`) patches `E8→E9` only when
  `HERE == user_TAIL_CALL`, and `user_TAIL_CALL` is set **only** by
  `compile_comma` (`:107`). Every fold/inline helper *zeroes* it
  (`:204,232,...`). So §5.3's plan to set `user_TAIL_CALL` after the span
  call is necessary and consistent with how TCO already works.

So §1's architectural read is correct. Now the problems.

---

## FATAL

### F1. The "1-deep TOS cache" abstraction does not match the kernel, and `force_settle` is under-specified exactly where it matters

The doc's `AbsStack` (§4.1) models `Cell::Tos | Mem(off) | Const`, with
`Dup` pushing a *second* `Tos`-aliased cell and lazy `rbp_delta`. The
kernel invariant is stricter than the doc states: **TOS is *always* in
`rax`, and every non-top cell is *always* in memory at the canonical
`[rbp+k]`**. There is no encoding in which two live cells share `rax`.

The doc's model allows transient states the kernel's encoding cannot
represent (`cells=[Tos, Tos]`, a `Const` that is logically "on the stack"
but nowhere in memory, a `Mem(k)` whose `k` floats because `rbp_delta`
hasn't been emitted). That is fine *inside* a span — but `force_settle()`
is the only thing that converts back, and the doc never writes its
algorithm. It says (§6) it "materializes every abstract cell into its
canonical slot, emits the single net `rbp` adjustment, leaves TOS in
`rax`." The hard part is the **ordering and offset arithmetic** of doing
that for a mixed `[Mem(−16), Const(7), Tos, Const(3)]` stack with a
nonzero `rbp_delta`, and that is precisely where a silent off-by-one
corrupts memory for some inputs only. A design doc whose entire safety
case rests on `force_settle` must *specify `force_settle`*, with the
spill order and the rule that materialization must happen
**high-address-first** (so writing `Const`s into not-yet-lowered slots
does not clobber a `Mem` cell that is about to be read). As written, the
reader cannot check it, and "differential fuzzing will catch it" (§8) is
not a correctness argument — it is a hope that the random corpus hits the
adversarial offset pattern.

This is Fatal not because it is unfixable but because **the document's
own central invariant is asserted, not demonstrated**, and the failure
mode is silent data-stack corruption.

### F2. Literals are not dispatched through `dh_comp` — the recommended hook misses half its input

§8's "cleanest" hook ("fop-eligible primitives get `dh_comp =
fop_collect_comp`; non-fop words keep `compile_comma` and *are* the flush
trigger") is incompatible with how literals compile. In
`interpret_source` (`interp.masm:235-242`, `.got_number`), a number in
compile state calls `literal` **directly**, never touching `find_name`,
`compile_word`, `to_comp`, `perform`, or any `dh_comp`. Floats
(`.got_float`, `:244-251`) and the locals fast-path
(`check_local_emit_word`, `:65-80`) are likewise off the `dh_comp` path
and emit bytes straight into `HERE` from *inside* `interpret_source`.

Consequences the doc does not address:

- `FopOp::Lit(n)` — the linchpin of every worked example — **cannot** be
  captured by a `dh_comp` swap. The collector *must* also intercept the
  `.got_number` arm of `interpret_source`, i.e. the "no fork of the
  interpreter" promise (§8) is already broken on day one for the most
  common token in the examples.
- The doc claims (§3.1) the hook can live at `compile_word`/`>comp`
  dispatch "rather than deep in `interpret_source`." That is true for
  *words* but false for *literals, floats, and locals*, all of which are
  resolved earlier in `interpret_source` than the `dh_ct` call at
  `.compile_ct` (`:144-149`). The hook surface is therefore strictly
  larger than the doc's two options admit, and it lands in the "intricate
  dispatch loop" §8 itself flags as risky.

Either the doc must move the collector into `interpret_source`'s number /
float / locals arms as well (and own that interpreter surgery honestly),
or it must restrict v1 to **zero literals** — which guts examples (b) and
the `Lit;Plus→PlusImm` fold that motivates the whole `*_imm` family.

---

## MAJOR

### M1. The deviation: this is a Forth compiler in Rust, not "JASM optimizing for Forth"

The owner's brief was explicit: *a Forth-aware macro **assembler**, not a
Forth compiler in LLVM*. The doc moves all intelligence into a Rust pass
over `Vec<FopOp>` and uses JASM purely as a text encoder (§3.3: "JASM/
LLVM-MC do encoding only"). Argue both sides honestly:

- **For "it honors the intent":** the optimizer is not LLVM IR; it knows
  `rax=TOS`/`rbp=DSP`; it emits through JASM's macro/`stk`/arena pipeline;
  the `fop_*` macros are real masm. The *prohibition* the owner actually
  cared about — "don't build a general compiler backend / don't drag in
  LLVM IR" — is respected. The pass is ~200 lines of stack bookkeeping,
  not a backend.
- **Against:** by the doc's own §1 finding, JASM *cannot* optimize — it
  has no instruction model — so "a Forth-aware JASM" is literally
  impossible at the assembler layer. What's left is a front end
  (`FopOp` IR), a middle end (stack-caching pass), and a back end
  (renderer → JASM). That is the textbook shape of a compiler. JASM's
  role collapses to "the thing that turns my chosen instruction text into
  bytes" — which is what *any* assembler does for *any* compiler. The
  `fop_*` macros are decorative: §4.4 says the renderer "prefers direct
  lines" and uses the macros "only for the unoptimized fallback," so in
  the optimized hot path JASM sees plain `add rax, [rbp]` text it does
  nothing special with.

**Verdict for the human:** the design quietly became a small Forth-to-x86
compiler that uses JASM as its assembler. That may well be the *right*
engineering call (you cannot optimize in a text concatenator), but it is a
**reversal of the stated layer**, and the doc should say so in one
sentence at the top instead of claiming it "keeps the spirit of the brief."
If the owner's intent was specifically "the cleverness lives in the
assembler so other assembler users benefit," this design does **not**
deliver that — the cleverness is locked in WF64's Rust runtime and is
useless to any other JASM client. Decide on that axis, not on the LLVM
red herring.

### M2. Analysis error in the headline example — the doc contradicts itself on `dup +`

§4.5(a) claims the headline rule `dup + → add rax,[rbp]; add rbp,8` is
*wrong* for `dup +` (would "add TOS to itself" incorrectly) and that
`dup +` must instead become `add rax, rax`. **This is backwards.** Trace
the kernel encoding: entering, TOS=a in `rax`. `dup` =
`mov [rbp-8],rax; sub rbp,8`, so after it `[rbp] == a`. Then
`add rax,[rbp]; add rbp,8` computes `a + a` and restores `rbp` — **exactly
correct**. The headline rule is *valid as stated* for `dup +`; `add rax,rax`
is merely a 1-byte-shorter equivalent. The doc's §4.5(a) "Note" inventing
a distinction between the `dup +` (alias) case and the `over +` (Mem)
case, and asserting the brief's example asm only works for `over +`, is an
error. It matters because the pass's transfer functions (§4.2) are derived
from this mistaken split: a `Dup` that pushes "a copy of `Tos`" forces the
"both operands alias `rax`" special case, when in reality after any *spill*
the duplicate is an ordinary `Mem` cell and the uniform `add rax,[rbp]`
rule applies. The model is more uniform than the doc thinks, and the
special-casing is a latent bug source.

### M3. Two sources of truth — already diverged in the doc itself

§5 (the prompt's concern) is real and the doc's mitigation ("every
`FopOp` has an equivalent `fop_` macro, testable against STC bytes") is
**already violated in §2**:

- `fop_minus` is written `mov rcx,[rbp]; sub rcx,rax; mov rax,rcx;
  add rbp,8`. The kernel `minus` (`arith.masm:48-53`) is
  `neg rax; add rax,[rbp]; add rbp,8`. Same math, **different bytes**.
  There is no `inline_minus_comp` to compare against (it doesn't exist —
  `minus` uses `fold_minus_comp`, the literal-fold path), so the Phase-0
  "assert fop bytes == inline_*_comp bytes" test (§7) has **nothing to
  diff `fop_minus` against** and silently passes by absence.
- `fop_store` (§2) is `mov rcx,[rbp]; mov [rax],rcx; mov rax,[rbp+8];
  add rbp,16`. Kernel `store` (`memory.masm:25-31`) is
  `mov rcx,[rbp]; mov [rax],rcx; mov rax,[rbp+cell]; stk(2,0)`. These
  happen to agree, but the agreement is coincidental and unenforced.
- Only **9** primitives have an `inline_*_comp` byte emitter
  (`dup/drop/swap/over/>r/r>/r@/2>r/2r>/2r@/i/j/do_part*`). The arithmetic
  fops the optimizer most wants (`+ - * and or xor @ !`) have **no**
  inline emitter — they exist only as `proc` bodies and `fold_*_comp`
  immediate forms. So for the headline ops there is **no golden byte
  source** in the kernel to keep `fops.masm` in sync with. §7 Phase 0's
  premise ("compare fop bytes to inline_*_comp bytes") only covers the
  shuffles, not the arithmetic — the exact ops where a divergence
  silently miscompiles.

This is Major: the doc presents golden-byte testing as the sync mechanism,
but the mechanism has no referent for most fops.

### M4. ROI is thin and the doc half-admits it

Each optimized span still becomes a `call rel32` (or tail `jmp`) from the
colon body (§3.3, §5.3). For the motivating `: foo dup + ;`, the body goes
from `[8-byte dup][5-byte call plus]` (13 B) to `[5-byte call span_fn]`
plus a separate arena function `add rax,rax; ret` (4 B) — i.e. you trade
inline bytes for a call boundary *plus* arena memory *plus* a full
`Assembler.assemble` + MCJIT `finalize` **per span** at `:`-time
(`runtime.rs:2777-2785`; jit finalize is whole-module). §8 concedes this
("a 3-byte add reached by a 5-byte call is *bigger* than inlining 3
bytes") and proposes emitting bytes inline for small spans — which is
**exactly the far-simpler alternative the owner was about to take**:
add `inline_plus_comp`/`inline_fetch_comp`/`inline_one_plus_comp` to the
existing STC peephole (the project already has 9 such emitters and a
working `try_fold_literal`). That alternative needs **zero** new IR, zero
Rust pass, zero arena traffic, zero per-definition JIT latency, and reuses
the proven `inline_*_comp` machinery and its existing tests.

The JASM-optimize path only wins when a span is **long enough** that
(a) cross-op cancellation saves more than the call/arena overhead, and
(b) the saved inner `call`s dominate. The doc never quantifies that
break-even, and its own Phase-1 example (`dup +`) is *below* it. **The
honest conclusion is that v1's showcase case is a net loss versus
bare-op inlining**, and the design's value only appears at span lengths it
never benchmarks. Phase 3 defers the measurement that should gate the
whole project to *after* it's built.

---

## MINOR

### m1. `@` / `!` aliasing the cached cell is hand-waved
§4.2 says `Fetch = mov rax,[rax]`, "stack shape unchanged." True for the
register, but the pass must treat `Fetch` as a **barrier on deferred
`Const`/`Mem` reasoning about that address**: if a `Const(addr)` was
folded and never materialized, `@` still needs the address *in `rax`*
(it is — `Fetch` consumes TOS), but a subsequent `!` to an address that
aliases a still-deferred spilled cell could store-before-load. The doc
asserts `force_settle` handles boundaries but never states that **`Store`
must force all pending spills first** (a store can write any address,
including a stack slot the pass thinks it still "owns" lazily). Memory ops
break the "no observer inside a span" claim (§6) because memory *is* a
shared observer. Needs an explicit rule: `Fetch`/`Store` force-settle the
`rbp_delta` (so `[rbp+k]` offsets are real) before emitting.

### m2. `drop` underflow / empty-cache is undefined
§4.2 `Drop` pops the top `Cell`. If the span begins with `drop` (a word
may legitimately start `: f drop ... ;`), `cells` underflows the abstract
TOS and the pass must reload from real memory — but the abstract model
started with `cells=[Tos]` and has no representation for "consumed the
incoming TOS, next TOS is in caller-provided `[rbp]`." The transfer
function for "pop when only the incoming `Tos` is present" is missing;
it must emit `mov rax,[rbp]; add rbp,8` and *not* underflow the Vec.
Same gap for `+`/`swap`/`over` when the span's net input depth exceeds
what's been pushed within the span (operands come from the caller's
memory stack, at offsets the pass must track relative to entry `rbp`).
The doc only ever traces spans whose operands are produced *within* the
span; real spans consume caller cells.

### m3. `>r`/`r@`/locals "leaf w.r.t. LP/RSP" is asserted, not checked
§5.4 says locals refs are span terminators so the optimizer never touches
LP/RSP. Correct *given* the terminator rule — but the terminator rule for
locals depends on the locals fast-path in `interpret_source:65-80`, which
runs **before** `find_name` and emits inline bytes itself. The collector
must flush the open span *before* `check_local_emit_word` writes its 15
bytes to `HERE`, or the local's fetch lands in the dict body interleaved
with a not-yet-emitted span. The doc lists locals as a terminator but
places the flush trigger at `dh_comp` (§8), which is too late — the locals
emit already happened. Ordering bug, same root cause as F2.

### m4. `recurse`, `[ ]`, `postpone`, immediate words, mid-word `base`
None are addressed. `recurse` (`compile.masm:1334-1344`) emits a
`compile_comma` to `latestxt` — fine as a terminator, but it sets
`user_TAIL_CALL`, interacting with §5.3's tail-call accounting. `[` / `]`
(`:1346-1358`) flip `STATE` mid-definition; a span straddling `[ ... ]`
(literal computed at compile time then `]`) must terminate at `[`. An
immediate word mid-span runs arbitrary code (can move `HERE`, emit
branches) and **must** be a terminator, but the collector can't know a
word is immediate until after `find_name`/`to_comp` — so the flush
decision needs the `dh_ct == execute` test from `interp.masm:138-140`,
which the doc's `dh_comp`-swap hook never sees. `postpone` and mid-word
`base` changes (a number's value depends on `BASE` at parse time, already
resolved before `FopOp::Lit`, so this one is actually fine) deserve at
least a sentence.

### m5. Phase-1 "provable in isolation" overstates what's proven
The Phase-1 acceptance (§7) proves `: foo dup + ;` gives 10 and the span
is `add rax,rax; ret`. That proves the *happy path*, not the invariant.
The one boundary test (`dup + 1 if 7 then`) exercises a single
force-settle shape (TOS-only, `rbp_delta==0`) — the *easy* case. It does
**not** exercise the F1 failure mode (mixed `Mem`/`Const` cells with
nonzero `rbp_delta` at a boundary), because `{Lit,Dup,Plus}` with these
rules never *produces* a multi-cell deferred state — `dup`+`plus` always
collapses immediately. So Phase 1 is "provable" precisely because it is
too small to contain the bug Phase 2 introduces. The differential fuzz
oracle (§7.3) is the right idea, but as specified ("random fop-only
bodies, random initial stacks, compare final stacks") it only checks the
**final** stack, not intermediate memory — a span that corrupts a
caller cell *below* the consumed depth and happens to leave the visible
top correct passes the oracle. The oracle must compare the **entire**
data-stack region (SP0 down to deepest touched slot) **and** `rbp`, not
just `session.stack()`.

### m6. Doc/code drift cited as fact
§1 cites the `CODE:` path as a "12-byte `mov rax,fn_addr; jmp rax`
trampoline" — but that comment in `compile.masm:2329-2333` is **stale**;
the actual code (`:2372-2377`) points the xt directly at `fn_addr` with
no trampoline. The doc inherits a wrong comment as ground truth. Minor,
but it shows the architecture section was written partly from comments,
not only from code.

---

## What would most improve the design (top 3)

1. **Specify `force_settle` as an algorithm, with the high-address-first
   materialization order, and make the differential oracle compare the
   whole touched stack region + `rbp` (not just the visible top).** This
   converts the central safety claim (F1, m5) from assertion to something
   checkable, and closes the silent-corruption class the doc itself names
   as the biggest risk.

2. **Honestly relocate the interception point and the framing.** Admit the
   collector must hook `interpret_source`'s number/float/locals arms
   (F2, m3), not just `dh_comp`; and state in one line that this is a
   small Forth→x86 compiler using JASM as its assembler, not a
   "Forth-aware JASM" (M1) — then let the owner decide if that reversal is
   acceptable. The current framing hides the decision the human most needs
   to make.

3. **Gate the project on the break-even measurement *before* building the
   arena path, and ship the cheap alternative first.** Add
   `inline_plus_comp`/`inline_fetch_comp`/`inline_one_plus_comp` to the
   existing STC peephole (M4) as Phase 0.5 — it captures examples (a) and
   (c) entirely, with no IR, no Rust pass, no per-def JIT, reusing the
   proven `inline_*_comp` + `try_fold_literal` machinery. Only pursue the
   `FopOp` pass for spans long enough that cross-op cancellation beats a
   `call`+arena, and *prove* that length exists with numbers first.

---

## Verdict

**Sound-with-fixes.** The architectural read of JASM (§1) is correct and
the arena reuse is real, so the design is *implementable*. But the
document (a) reverses the owner's stated "optimize in the assembler"
layer without flagging it as a decision, (b) rests its entire correctness
case on an unspecified `force_settle` whose failure is silent corruption,
(c) bases its hook on a `dh_comp` path that literals/floats/locals bypass,
(d) contains a self-contradicting analysis of its own headline example,
and (e) showcases a Phase-1 win (`dup +`) that is actually a net size loss
versus the far cheaper bare-op-inlining alternative the owner was about to
take. None are fatal to the *idea*; all are fatal to *this draft's
argument*. Fix the three items above — specify the settle, relocate and
own the hook+framing, and prove the break-even — and the proposal becomes
defensible. Until then it is a compiler wearing an assembler's name tag.
