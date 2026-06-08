# Adversarial Critique — JASM Forth Optimizer v2 (Type-Directed Stack Scheduler)

Reviewer: adversarial design critic, no prior context.
Target: `docs/design/jasm_forth_optimizer_v2.md`.
Grounding (every claim below is checked against source, file:line cited):
`E:/JASM/rust/src/asm/{expand,macros}.rs`;
`E:/WF64/kernel/{interp,compile,dict,arith,memory,macros}.masm`;
`E:/WF64/src/{lib,runtime}.rs`; `E:/WF64/tests/harness.rs`.

Verdict up front: **sound-with-fixes.** v2 genuinely closes the two v1
Fatals on paper (it specifies `force_settle` with an ordering proof, and it
moves literal capture into `interpret_source` where literals actually
live). The type-model spine is the right abstraction. But the verification
surfaces **one new Fatal** (a per-fop *type is wrong* against the live
kernel body — `fop_over` and the memory fops have rbp-relative reads the
`MemEffect::None`/depth-delta model omits, which the scheduler's lazy-rbp
will miscompile), a **Major** feasibility gap (the lowering hook is hand-
asm surgery inside the most intricate loop in the kernel, not the "one
hook" the doc implies; and the FOP-tag dispatch is mis-located against the
real `dh_ct`/`dh_comp` split), and a **Major** ROI/oracle gap (the fuzz
oracle as specified cannot observe the very boundary state `force_settle`
is written to satisfy). None kill the idea; all must close before Phase 0.

---

## What v2 got RIGHT (verified, not complaints)

* **FnMut closures carry cross-invocation state.** Confirmed.
  `RustMacroFn = Box<dyn FnMut(...) + 'static>` (`expand.rs:189,419`);
  `invoke_rust_macro` removes the closure, calls it with `&mut State`, and
  reinstalls it unconditionally (`expand.rs:1699–1719`). A closure
  capturing `Rc<RefCell<StackCache>>` is `'static` (owned `Rc`) and mutates
  shared state every call. The mechanism is real. **Claim 2 (a): TRUE.**
* **No token-rewrite pass exists.** Confirmed. `expand_range` is one
  forward `while i < tokens.len()` pass (`expand.rs:738–744`); macros fire
  in order on `Ident`+`(` (`expand.rs:788–799`), each driven to completion
  before `i` advances; `emit_line` lexes text and pushes **final**, non-re-
  expanded tokens (`expand.rs:335–349`). So "expansion order IS schedule
  order" is true **for a flat, top-level FOP stream** (the generated span
  source is top-level, so no `expand_macro_call` re-ordering intervenes).
  **Claim 2 design-choice (a) over (b): correctly reasoned.**
* **`force_settle` is now specified** with deep-cells-first / TOS-last /
  single-rbp-move ordering and a depth-derived offset rule (v2 §3 steps
  1–5). This is exactly the v1-Fatal gap, and it is addressed with a real
  algorithm and an equality argument. **The central v1 hole is closed.**
* **Literal capture is relocated correctly.** `interpret_source`
  (`interp.masm:38`) is indeed the one pump where words (`.compile_ct:144`),
  integers (`.got_number:235`), floats (`.got_float:244`) and locals
  (`check_local_emit_word`, called at `:69`) converge, and integers/floats/
  locals bypass `dh_comp` entirely (they `call literal`/`fliteral`/emit
  inline from inside the pump). Hooking *here* is the only place `lit(n)`
  can be caught. **Claim 4 location: TRUE and is the right fix.**
* **Arena reuse is real.** `rt_code_compile_body` → `with_code_assembler`
  (one session `Assembler` preloaded with `macros.masm`, `register_macro`
  for `stk`) → `Jit::new_in_arena` → `add_asm`/`declare_fn`/`lookup_addr`
  (`runtime.rs:2764–2785,2897–2913`). `rt_span_compile` can clone this.

---

## FATAL

### F1. Two per-fop TYPES are WRONG against the live kernel body — the model omits rbp-relative reads, and the lazy-rbp scheduler will miscompile

The charter's whole bet (Insight 2) is that the *type is the model*. So a
type that under-describes a real body is a miscompile, by construction. Two
fops in v2's table (§0) are under-typed:

**(a) `fop_over` — typed `reads c1, mem None, depth +1`.** The live kernel
body is `over`-inline (`compile.masm:813–832`):
`mov [rbp-8],rax ; mov rax,[rbp] ; sub rbp,8`. The middle instruction
**reads NOS from memory at `[rbp]`** — an *rbp-relative load*. v2's type
says `mem: None` and the scheduler tracks a **lazy `rbp_delta`** that is
"materialized as one `sub/add rbp` only at force_settle *or when a memory
access needs rbp to address a spilled cell*" (§2 `StackCache.rbp_delta`
doc). But `over`'s read of `[rbp]` is exactly such an access, and the type
gives the scheduler **no signal that it is one** — `mem: None` says "touches
no memory the scheduler must order." Concretely: after `lit(7) over`, the
window is `[MemHome(c1)=a, Const(7), Ref(a)]` with `rbp_delta` pending from
the lit push. If the over-template emits `mov rax,[rbp]` against the *un-
materialized* rbp, it reads the wrong cell. The doc's escape hatch
("...or when a memory access needs rbp...") is the right idea but it is
**driven by `MemEffect`, which is `None` for `over`** — so the trigger
never fires. Same latent bug for any straight-line fop whose body reads a
stack cell off rbp.

**(b) `fop_fetch`/`fop_store` — addressing residency is untyped.**
`fop_store`'s kernel body (`memory.masm:25–31`) is
`mov rcx,[rbp] ; mov [rax],rcx ; mov rax,[rbp+cell] ; stk(2,0)`. It reads
NOS *and* NNOS off rbp (`[rbp]` and `[rbp+cell]`) and raises NNOS to TOS.
v2 types it `reads c0,c1 → MEM, depth −2, mem WritesMem`. But it **also
reads c1 and c2 from rbp-relative memory** to perform the raise, and v2's
`MemEffect` axis only models *heap* aliasing (the `@`/`!` barrier), not the
*stack* rbp-relative reads the body actually performs. The scheduler's
"value cell and address cell into registers, emit the store" sketch (§2)
silently assumes the value (NOS) is register-resident; if it is `MemHome`
with `rbp_delta` pending, the load address is wrong.

**Why Fatal, not Major:** the three axes are asserted (§0 "Why this is
sufficient") to be *all* the scheduler needs. They are not: there is a
**fourth effect class the curated set actually exercises — rbp-relative
stack reads whose offset depends on the unmaterialized `rbp_delta`.** The
v1 critique's m2 already noted the model is "more uniform than the doc
thinks" for `dup +`; v2 inherits the opposite error — it is *less* uniform
than the type table admits, because `over`/`@`/`!` read the memory stack
and the type hides it. **Fix:** add a fourth type axis (or fold into
`MemEffect`) — `StackMem { reads_rbp: &[Cell] }` — and make any fop with a
non-empty `reads_rbp` force `rbp_delta` materialization (or address off a
*known* old-rbp + compile-time offset, the same trick `force_settle` uses)
before its template emits. Until that axis exists, "the types are the
model" is false for 3 of the 10 curated fops.

---

## MAJOR

### M1. The "one lowering hook" is hand-assembly surgery in the kernel's most intricate loop, and the FOP-tag dispatch is mis-located against the real `dh_ct`/`dh_comp` split

v2 §4 sells the hook as "one lowering hook in the compile dispatcher" that
"appends a typed-FOP record to a per-definition buffer." Two problems the
doc under-counts:

**(a) It is asm surgery, not a Rust hook, in four arms of a 230-line hand-
written loop.** `interpret_source` (`interp.masm:38–265`) is machine code
with an `r13`-based empty-stack tie-breaker threaded through *every* arm
(`:51,123,141,179,208`), a `PARSE_BARRIER` dance (`:224–233`), trace hooks,
and a locals fast-path that emits 15 bytes inline *before* `find_name`
(`:69`). v2 must insert "is `STATE=1` and is this token FOP-eligible? then
append to buffer instead of dispatching" at `.compile_ct` (`:144`),
`.got_number` (`:235`), `.got_float` (`:244`), and the locals path
(`:69`) — i.e. fork four arms of exactly the loop the v1 critique flagged
as "intricate," each of which currently maintains the `r13`/DSP encoding
invariant the new buffer-append path must *also* preserve. The doc's
"handled by construction" (§4) is true for *semantics* but elides that the
construction is non-trivial asm edits, not a data-table change. The honest
cost is "modify `interpret_source` in 4 places + a span-buffer + a flush
trampoline that calls `rt_span_compile`," not "a hook."

**(b) The FOP-tag location is wrong against the real dispatch.** v2 §4 says
"FOP word at `.compile_ct` → append the word's FOP, identified by a small
tag on the dict header (reuse `dh_stk`/`dh_comp`...)." But at `.compile_ct`
the code does `rcx = [r9+dh_ct]` (`interp.masm:120`) and `call rcx`
(`:148`), and for an *ordinary primitive* `dh_ct = compile_word`
(`dict.masm:536–537`), **not** the inline/fold helper. The inline/fold
helper (`inline_dup_comp`, `fold_plus_comp`, `compile_comma`) lives in
**`dh_comp`** (set by `publish_primitive`, `dict.masm:611`), and is reached
only *indirectly*: `compile_word` → `to_comp` (reads `dh_comp` slot,
`dict.masm:283–290`) → `perform`. So to identify "is this a FOP primitive"
the hook must either (i) inspect `dh_comp` (the helper pointer) and match it
against the known FOP-helper addresses, or (ii) add a real tag bit. `dh_stk`
is a **2-byte** field (`macros.masm:158`) currently holding the stack
effect; overloading it for a FOP-id is possible but is a kernel-header ABI
change touching `publish_primitive` and every consumer of `dh_stk`. The doc
treats this as free ("reuse dh_stk/dh_comp"); it is a small ABI change plus
a pointer-match table, and it lands *before* the `call rcx`, meaning the
hook is at `.found_word`/`.compile_ct` *before* dispatch, not "at
`.compile_ct`" as written. **This is feasible but materially larger and in
a riskier spot than §4 implies.**

### M2. The fuzz oracle as specified cannot observe the boundary state `force_settle` exists to guarantee — so the load-bearing test misses the load-bearing bug

v2 §8 makes the differential fuzzer "the load-bearing test" and says it
compares "the ENTIRE touched stack region AND `rbp` AND the scratch memory
block, byte-for-byte." Check what the harness can actually see:
`Wf64Session::stack()` reads cells from `current_dsp` up to `dsp_top`
(`lib.rs:1703–1709`), and `current_dsp` is reloaded from `USER_DSP_SAVE`
*after* the word returns to Rust (`lib.rs:1758`). On return the kernel is in
the **pure in-memory wire format** (RAX=TOS only *inside* the kernel;
`macros.masm:11–22`). Consequences:

* **`rbp` is not observable by the oracle.** Inside a word, `rbp` is the
  internal DSP; on return it is restored. The "compare `rbp`" the spec
  leans on is an *intra-span boundary* quantity (force_settle sets it at a
  branch/`;`), but the oracle only sees the *net* word-exit memory image
  via `DSP_SAVE`. A span that mis-sets `rbp` at an *internal* boundary
  (e.g. a span followed by `if`, where the branch observes the bad `rbp`)
  is only caught if the *branch path itself* propagates the error into the
  final stack — exactly the silent case force_settle is meant to prevent.
  The v1 critique m5 demanded "compare the whole region + rbp"; v2 adopted
  the words but the harness has **no rbp accessor and no sub-`current_dsp`
  region reader** (`stack()` only reads *above* `current_dsp`). New
  introspection is required and is not scoped.
* **"Entire touched stack region" below the live top is unreadable.**
  `stack()` cannot read cells below `current_dsp`. A scheduler bug that
  corrupts a caller cell *below* the consumed depth and leaves the visible
  top correct passes `stack()`. The oracle needs a raw `peek(addr,len)` and
  must compute the deepest touched slot — neither exists today.

So the test that is supposed to catch silent corruption is, as wired,
blind to the corruption mode (intra-boundary rbp / sub-top memory) that the
entire `force_settle` spec is written to rule out. **Fix is small** (add
`peek`/`rbp` accessors to the harness and have the span emit a probe), but
the doc presents the oracle as sufficient when it is currently not.

### M3. Single-source is achieved for only the 4 shuffle fops; the arithmetic/memory fops have NO kernel inline referent and are pinned only by a test that does not yet exist against a body that differs

v2 §5's rule of record: a fop template's bytes are either `@include`d from
the same table the kernel inline helper uses, **or** golden-byte-tested
against the live primitive. Verify the referents:

* **Shuffles** (`dup/drop/swap/over`): real inline helpers exist
  (`compile.masm:753,772,791,813`) — single-source is achievable. ✔ (modulo
  F1: `over` reads `[rbp]`, so its template is *not* residency-free.)
* **`+ - * @ !`: NO `inline_*_comp` exists** (confirmed: `lib.rs:1577–1627`
  maps `plus→fold_plus_comp`, not an inline; bare ops fall to
  `compile_comma`). So for the **reg-reg** form v2 must synthesize bytes
  with no kernel referent. The kernel bodies are: `plus` =
  `add rax,[rbp]; stk(2,1)` (`arith.masm:23`), `minus` =
  `neg rax; add rax,[rbp]; stk(2,1)` (`arith.masm:48`), `times` =
  `imul rax,[rbp]; stk(2,1)` (`arith.masm:32`). v2's reg-reg sketch for
  `fop_plus` is `add rax,rcx` (§2) — a **different operand form** than the
  kernel's `add rax,[rbp]`. They compute the same value *only if* `rcx`
  holds the same NOS the kernel would read from `[rbp]`; that equivalence
  is the scheduler's responsibility and is **exactly what the golden test
  must pin**, but §5 admits that test "does not exist yet" and the byte
  recipe has no kernel inline to diff against. For `minus` the divergence
  is sharper: kernel uses `neg rax; add rax,[rbp]` (result in rax), so a
  naive `fop_minus` reg-reg `sub rax,rcx` computes `b-a`, the **wrong
  sign** (`-` is `( a b -- a-b )`, a=NOS, b=TOS). The v1 critique M3 flagged
  the identical sign trap in v1's `fop_minus`; v2 does not give the
  `fop_minus` reg-reg bytes at all, so the trap is merely deferred, not
  closed. **Until the golden tests exist and run the live primitive, §5 is
  a promise, and the `minus` sign is an open miscompile waiting in Phase 1.**

---

## MINOR

### m1. The `mem_dirty` single bit is sound but coarser than the doc's own example claims
§2's rule (any store poisons *all* `Loaded` regs; loads never hoisted past
stores) is correct and closes the `over @ ... swap !` / `a @ b ! a @`
aliasing class — verified against the requirement: a post-store `@` sees
`mem_dirty=true` and re-emits the load. Good. But §7's `: h dup @ swap ! ;`
trace claims the load went to `rcx` while `a` stayed in `rax`
(`mov rcx,[rax]`), diverging from kernel `fetch` = `mov rax,[rax]`
(`memory.masm:18`). That is *legal* (different residency, same value) but
it means `fop_fetch`'s template is **not** the kernel `fetch` bytes — so it
falls under §5's "golden test" arm, not the "@include" arm, and there is no
kernel inline `@` to diff against (same hole as M3). Also: `mem_dirty` never
clears within a span except by re-load; a long span with one early store
poisons *all* later loaded values even non-aliasing ones — correct but
leaves performance on the table (acceptable for v1, note it).

### m2. Underflow / caller-cell consumption is still only hand-waved
§2 transfer functions assume operands were pushed *within* the span. A span
that *starts* with `drop`, `+`, `swap`, `@`, or `!` consumes caller cells
that live in memory below entry (`MemHome` with negative index relative to
entry). The depth encoding (`macros.masm:14–17`: depth1 ⇒ DSP=SP0, TOS only
real item; depth0 ⇒ DSP=SP0+cell, TOS don't-care) means a `drop` at entry
depth 1 must emit `mov rax,[rbp]; add rbp,8` and the *next* TOS is the
caller's, addressed off the materialized rbp. v2's `entry_depth` field and
`MemHome(i32)` are the right scaffolding, but §9 risk 2 itself admits "prove
the conservative depth never under-counts live cells a branch target reads"
is **open**. Until that proof exists, a span beginning with consuming ops is
unverified. The worked examples (§7) all conveniently start by *producing*
(dup/lit) — none start with a bare consumer, so the examples dodge the case.

### m3. `[ ]`, postpone, immediate, recurse, multi-line — mostly handled, two gaps
§4 correctly makes `[`/postpone/immediate-exec `SpanEnd` terminators, and
the per-definition buffer keyed to `:` (cleared at `colon`, alongside
`LOCALS_COUNT`/`TAIL_CALL`) handles multi-line by construction. Two gaps:
(i) **immediate detection timing** — the doc must use the `dh_ct == execute`
test (`interp.masm:138–140`) to know a word is immediate, which is available
at `.compile_ct` *before* `call rcx`; the hook must read it there, not after.
(ii) **`recurse`** (not mentioned) emits `compile_comma` to `latestxt` and
**sets `TAIL_CALL`**; as a span terminator it interacts with §6's tail-call
accounting exactly like a normal call, so it is fine — but it must be on the
terminator list explicitly, and the §6 tail-call story ("set TAIL_CALL after
the span call") must not be clobbered by a recurse that precedes `;`.

### m4. Tail-call composition is plausible but unproven for the span-as-tail case
§6 claims if the last body item is a single `call rel32` to a span fn, `;`
patches E8→E9 as today (`semicolon:2198`). Confirmed the patch keys on
`HERE == user_TAIL_CALL` and only `compile_comma` sets `TAIL_CALL`
(v1-critique A, re-verified `lib.rs` fold/inline helpers zero it). So the
flush path that emits the span's `call rel32` must set `TAIL_CALL` to
HERE-after-that-call, exactly as `compile_comma:107` does — the doc says so
but the span flush is a *new* emitter that must replicate that side effect
or TCO silently won't fire (a perf regression, not a miscompile). Note it.

### m5. Per-span MCJIT finalize cost is real and the inline-vs-call threshold is unbounded
Each span → `rt_span_compile` → `Jit::new_in_arena` + `add_asm` +
`declare_fn` + `lookup_addr` (modeled on `runtime.rs:2777–2785`), which is a
**whole-module MCJIT finalize per span**. A colon word with N straight-line
runs separated by control flow pays N finalizes at `:`-time. v1 critique
already flagged this; v2 §9 risk 5 acknowledges the inline-vs-call knob but
does not bound the finalize cost. For the Phase-0 examples (`dup +`,
`5 + 2 +`) the optimized body is 2–4 bytes reached by a 5-byte call — a net
**size loss** vs inlining, same as v1's M4. ROI (below) turns on this.

---

## ROI (Claim 6) — quantified break-even

The cheap alternative is unchanged from the v1 critique: add
`inline_plus_comp`/`inline_minus_comp`/`inline_fetch_comp`/
`inline_store_comp` to the STC peephole, mirroring the 4 existing shuffle
inliners (`compile.masm:753–832`) and the `fold_*_comp` family
(`compile.masm:185–268`). That captures `dup +` (→ `add rax,rax` via a
`dup`-aware fold), `@ 1+` (→ `mov rax,[rax]; add rax,1`), and `5 * 2 +`
(already foldable) with **zero IR, zero arena traffic, zero per-def
finalize**, reusing proven machinery and its tests.

The typed scheduler **strictly wins only** when it does something a per-op
peephole structurally cannot:
* **cross-op settle elision** — e.g. `lit lit + lit *` collapses to one or
  two immediates with no intermediate spill/reload; a per-op peephole sees
  only adjacent pairs and re-spills between ops.
* **dead-store elision across ops** — `dup drop`, `lit drop`, `over drop`
  emit *nothing*; the peephole emits both halves.
* **const-fold chains** — `2 3 + 4 *` → `lit(20)`; peephole can't fold
  across the non-adjacent literals.

Break-even (rough, bytes): the scheduler's fixed overhead per span is a
5-byte `call rel32` + one MCJIT finalize. A span must therefore *save* more
than ~5 bytes of emitted code AND amortize the finalize, which only happens
at roughly **≥4 fusible ops with ≥2 eliminated spills/reloads** (each STC
spill+reload+call is ~13–18 bytes; eliding two recovers ~26–36 bytes,
clearing the call + finalize). Below that, inline peepholes win. **The
Phase-0 slice (`dup +`, `5 + 2 +`) is below break-even** — same conclusion
as the v1 critique, and v2 §9 risk 4 honestly concedes it. The disciplined
path: ship the 4 inline peepholes first (a day's work), measure a real
corpus (`demos/`, `lib/*.f`) for spans of length ≥4 with ≥2 elidable
spills, and only build the scheduler if that population is non-trivial.

## First slice (Claim 7) — provable in isolation? Partly.

Phase 0 (`fop_dup`,`fop_plus`,`fop_begin`,`fop_settle`, 2-cell window,
integers) proves the *spine* (shared cache, literal capture, force_settle,
arena wiring, hybrid fallthrough) — genuinely valuable. **But it is too
small to contain the bugs Phases 1–2 introduce** (the F1 `over`/`@`/`!`
rbp-relative reads, the M3 `minus` sign, the m2 caller-cell consumption):
`dup +` never produces a multi-cell deferred state, never reads `[rbp]`,
never touches the heap. The oracle (M2) is blind to the sub-top/rbp
corruption mode in Phase 0 *and* Phase 2. So Phase 0 "provable" is real for
the plumbing and illusory for the scheduler's hard cases — exactly the v1
critique's m5 objection, **not yet closed**, because the oracle was adopted
in words but not in harness capability.

---

## The 3 highest-leverage changes

1. **Add the fourth type axis (rbp-relative stack reads) and re-type
   `over`/`@`/`!`.** Make any fop with a non-empty `reads_rbp` either force
   `rbp_delta` materialization or address off old-rbp + a compile-time
   offset before its template emits. This converts F1 from a latent
   miscompile into a typed, scheduled effect — and only then is "the types
   are the model" actually true for the curated set. (Closes F1.)

2. **Give the harness the introspection the oracle's correctness argument
   already assumes:** a raw `peek(addr,len)` reader, a post-run `rbp`/`DSP`
   accessor, and a settle-probe so the differential fuzzer can compare the
   *entire* region from `dsp_top` down to the deepest touched slot, plus the
   boundary `rbp`, against the STC oracle — and add boundary tests where a
   span is followed by `if` so an intra-boundary `rbp` error is forced into
   an observable result. (Closes M2; makes Phase 0 actually provable.)

3. **Land the 4 bare-op inline peepholes first and gate the scheduler on a
   measured corpus.** `inline_plus/minus/fetch/store_comp` reuse the proven
   `inline_*_comp` machinery, capture every Phase-0/1 example with no arena
   or finalize cost, and give the golden-byte *referent* §5/M3 currently
   lacks for the arithmetic/memory fops. Build the scheduler only for the
   span population (length ≥4, ≥2 elidable spills) that the corpus proves
   exists. (Closes the ROI gap; de-risks M3.)

---

## VERDICT

**Sound-with-fixes.** v2 is a real advance over v1: it specifies
`force_settle` with a defensible ordering proof, relocates literal capture
to the one point literals are visible (`interpret_source`), and its three-
axis type model is the correct shape for the charter's "typed macros are
the instruction model." The JASM feasibility claim (stateful FnMut fops
sharing one `Rc<RefCell<StackCache>>`, no core change) is verified true.
But three things must close before Phase 0 is safe: (F1) the type model is
*wrong* for `over`/`@`/`!`, which read the rbp-relative stack the model
declares memory-free, and the lazy-rbp scheduler will miscompile them; (M2)
the differential oracle, the design's own load-bearing safety net, cannot
observe the boundary `rbp`/sub-top memory that `force_settle` exists to
protect, so it would pass silent corruption; and (M3/ROI) the
arithmetic/memory fops have no kernel inline referent, the `minus` reg-reg
sign is an open trap, and the showcase slice is below the call+finalize
break-even that the cheaper inline-peephole path clears for free. Fix the
type axis, give the oracle real eyes, and ship the peepholes first — then
the typed-token scheduler is defensible for the spans where it strictly
wins (cross-op fusion and dead-store elision a per-op peephole cannot do).
Until then it is a sound spine wrapped around three unverified edges.
