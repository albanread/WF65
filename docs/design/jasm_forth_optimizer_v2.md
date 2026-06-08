# JASM Forth Optimizer v2 — Type-Directed Stack Scheduler over Typed FOP-Macro Tokens

Status: design. Supersedes v1 (`jasm_forth_optimizer_v1.md`) and answers the
critique (`jasm_forth_optimizer_critique.md`). Designed to the owner's charter:
the optimizer is a *stateful, type-directed stack scheduler that runs inside
JASM over a curated set of typed FOP-macro tokens*. It is **not** a
Forth→LLVM-IR compiler. JASM stays a macro assembler; the new machinery is a
legitimate extension of the macro assembler (assembler-held user state +
a small typed-fop macro library), and LLVM-MC still does all final encoding.

Every claim below is grounded in the current tree. Key anchors:

* JASM macro engine: `E:/JASM/rust/src/asm/expand.rs` — `RustMacroFn = Box<dyn
  FnMut(&mut RustMacroCtx) -> Result<(),String>>` (line ~189); `RustMacroCtx`
  carries `state: &'a mut State` and `output: &'a mut Vec<Token>` (line ~193);
  `invoke_rust_macro` takes the closure out of the map, calls it with `&mut
  State`, reinstalls it (line ~1684); `emit_line` lexes text and pushes **final**
  (not re-expanded) tokens (line ~335). Macros fire on `Ident(name)` followed by
  `(` (line ~790).
* `stk` reference macro: `E:/JASM/rust/src/asm/macros.rs` line ~177 — a Rust
  macro that does arithmetic over args and `emit_line`s `add/sub rbp, imm`.
* STC + peephole: `E:/WF64/kernel/compile.masm` — `fold_plus_comp` (185),
  `inline_dup_comp` (753) / drop / swap / over, `literal` (2030), `fliteral`
  (2048), `semicolon` tail-call E8→E9 patch (2198), `compile_comma` (99).
* Compile dispatch (literal-capture problem): `E:/WF64/kernel/interp.masm`
  `interpret_source` (38) — words → `.compile_ct: call rcx` (=dh_ct hook, 144);
  numbers → `.got_number: call literal` (235); floats → `.got_float: call
  fliteral` (244); locals → `check_local_emit_word` (69). Numbers/floats/locals
  **bypass dh_comp**.
* Canonical stack encoding: `E:/WF64/kernel/macros.masm` lines 11–22 (depth ⇔
  DSP-vs-SP0), `@define TOS rax / DSP rbp / UP rbx` (41), `stk` (199).
* Arena path: `E:/WF64/src/runtime.rs` `rt_code_compile_body` (2694),
  `with_code_assembler` reuses one `Assembler` preloaded with `macros.masm`
  (2897), `register_macro("stk", …)` (2904); `Jit::new_in_arena` +
  `CodeArena::with_code_header` (`E:/JASM/rust/src/jit.rs` 146, 239); the xt
  back-offset cell is written into the arena's reserved header
  (`code_colon_word`, `E:/WF64/kernel/compile.masm` 2336).

---

## 0. THE TYPE SYSTEM (the spine)

A FOP ("Forth op") is the atom the scheduler reasons about. The claim of
Insight 2 is that a small, *typed* set of FOPs is a sufficient instruction
model for Forth optimization — the scheduler needs the types, nothing else
about x86. A FOP's type has exactly three components.

```rust
/// One abstract data-stack cell, named by position at the moment the FOP
/// reads/writes it. c0 = current TOS, c1 = NOS, c2 = NNOS, ... The scheduler
/// renames these as the abstract depth changes; a FOP only ever names the
/// cells it touches relative to the *pre-FOP* top.
type Cell = u8; // 0 = TOS, 1 = NOS, ...

#[derive(Clone, Copy, PartialEq)]
enum MemEffect {
    None,        // touches no memory the scheduler must order
    ReadsMem,    // @  : value depends on a load
    WritesMem,   // !  : a store; barriers memory-derived cached values
}

#[derive(Clone, Copy, PartialEq)]
enum Flow {
    Straight,    // stays inside the span
    CondBranch,  // if -if while until ?do +loop ...   -> span terminator
    UncondBranch,// ahead again leave bra              -> span terminator
    Call,        // any non-fop dict word               -> span terminator
    SpanEnd,     // ;  exit  [  postpone  immediate-exec -> span terminator
}

/// The stack effect, expressed as abstract reads/writes + a net depth delta.
struct StackEffect {
    reads:  &'static [Cell], // cells whose VALUE the FOP consumes
    writes: &'static [Cell], // cells the FOP defines, named in the PRE-fop frame
    depth:  i8,              // net change to abstract depth (push=+1, pop=-1)
}

struct FopType {
    name:   &'static str,
    eff:    StackEffect,
    flow:   Flow,
    mem:    MemEffect,
    /// Constant payload for lit(N); None for the rest.
    imm:    Option<()>, // marker; the actual i64 rides on the token args
}
```

The emit-template is a separate function, *parameterized by where each touched
cell currently lives* (a register in the window, or its memory home), so the
same FOP emits different bytes depending on the scheduler state. **The types are
the model; the emit-templates are the codegen.** They are intentionally split so
the scheduler can reason purely over types and ask the template to materialize
only at the chosen residency.

### The typed table (the curated instruction model)

Net depth and cell touch-sets, with `( before -- after )` for orientation:

| FOP        | effect                | reads        | writes (pre-frame names) | depth | flow      | mem        |
|------------|-----------------------|--------------|--------------------------|-------|-----------|------------|
| `lit(N)`   | `( -- n )`            | —            | c0(new top)              | +1    | Straight  | None       |
| `fop_dup`  | `( a -- a a )`       | c0           | c0(copy→new top)         | +1    | Straight  | None       |
| `fop_drop` | `( a -- )`           | —            | —                        | −1    | Straight  | None       |
| `fop_swap` | `( a b -- b a )`     | c0,c1        | c0,c1                    |  0    | Straight  | None       |
| `fop_over` | `( a b -- a b a )`   | c1           | c0(copy of c1→new top)   | +1    | Straight  | None       |
| `fop_plus` | `( a b -- a+b )`     | c0,c1        | c0(result)               | −1    | Straight  | None       |
| `fop_minus`| `( a b -- a-b )`     | c0,c1        | c0                       | −1    | Straight  | None       |
| `fop_times`| `( a b -- a*b )`     | c0,c1        | c0                       | −1    | Straight  | None       |
| `fop_fetch`| `( a -- v )`         | c0 + MEM     | c0(loaded value)         |  0    | Straight  | ReadsMem   |
| `fop_store`| `( v a -- )`         | c0,c1 → MEM  | MEM                      | −2    | Straight  | WritesMem  |

Why this is *sufficient*: the scheduler only ever needs to know, per FOP,
(i) which abstract cells it consumes and produces and the net depth change —
to keep its register window and lazy `rbp` delta correct; (ii) whether the FOP
ends a span — to know when to `force_settle`; (iii) whether it touches memory
and how — to enforce load/store ordering and the `@`/`!` aliasing barrier.
Nothing in scheduling needs a general x86 model: register allocation is a tiny
fixed window (TOS + 1–2 shadow regs), and the *only* instructions that ever get
emitted are the per-FOP templates plus the settle moves, each a fixed byte
recipe lifted verbatim from the existing kernel primitive bodies (§5). Adding a
FOP = adding a row + a template; it never requires teaching JASM about x86.

---

## 1. JASM REALIZATION

### Can a registered JASM macro carry/mutate assembler-level state across invocations in one assemble?

**Yes — two mechanisms, both already present, and we use both.**

1. **Closure capture (FnMut).** `RustMacroFn = Box<dyn FnMut(...)>`
   (`expand.rs` ~189). `invoke_rust_macro` (~1684) removes the boxed closure
   from `state.rust_macros`, calls it, and reinstalls it. Because it is `FnMut`
   and lives across the whole `assemble`, a closure that captures
   `Rc<RefCell<StackCache>>` mutates that cache on every invocation. Registering
   `fop_dup`, `fop_plus`, … as separate closures that **share one
   `Rc<RefCell<StackCache>>`** gives every FOP a common, persistent scheduler
   state for the duration of one `assemble` call. This needs **no JASM change**.

2. **`&mut State` reach-through.** `RustMacroCtx` already holds `state: &'a mut
   State` (~193). Today `State` exposes no general-purpose user slot, so a FOP
   cannot find a sibling FOP's state *through the ctx*. The shared-`Rc` capture
   in (1) sidesteps that entirely.

### Is there a pre-expansion token-rewrite/schedule pass hook?

**No.** The expander is a single forward pass (`expand_range`, ~738); a Rust
macro's only output channel is `emit_line` (~335), which lexes text and pushes
**final** tokens that are *not* re-expanded. There is no registered "walk the
token vector and rewrite it" phase. So design choice **(b) a token-rewrite pass
over the typed-fop sequence** is not available without new JASM surface.

### Chosen realization: (a) STATEFUL fop macros over an assembler-held StackCache

We pick **(a)**: each FOP is a stateful Rust macro; they share one
`StackCache` via captured `Rc<RefCell<…>>`. This is *exactly* the charter's
"fop macros are stateful, context-sensitive code-generating tokens; the
optimizer runs over the token stream inside JASM and never leaves the macro
assembler." The FOP token stream is produced by the lowering hook of §4 and fed
to JASM as ordinary macro-call text; JASM's existing forward expansion drives
the scheduler one FOP at a time, in order — which is precisely a streaming
schedule pass. No new pass phase is needed because **expansion order is the
schedule order.**

### The one minimal, in-charter JASM addition

The shared-`Rc` capture works with zero JASM changes, but it has a wart: the
*reset point* of the cache. One `Assembler` is reused for the session
(`with_code_assembler`, runtime.rs 2897), and one `assemble` call handles one
whole optimized span (§6), so the cache must start empty at each span. We need a
clean, in-charter way to (re)initialize the shared cache per `assemble` and to
let the lowering hook hand the cache its *entry depth*. Two acceptable forms;
prefer the first:

* **Span-delimiter FOP macros** `fop_begin(depth)` / `fop_settle()` registered
  like any other FOP. `fop_begin` resets the shared `StackCache` and sets the
  abstract entry depth; `fop_settle` runs `force_settle` (§3) and asserts the
  cache is canonical. **Zero JASM core change** — they are just two more
  stateful macros. This is the recommended path.

* **(Optional convenience) assembler-held user state.** If we later want FOPs to
  reach state through the ctx instead of a captured `Rc`, add a single
  `user: HashMap<String, Box<dyn Any>>` field to `State` (expand.rs `State`,
  ~61) plus `RustMacroCtx::user_state_mut::<T>(key)`. This is a small, isolated
  extension localized entirely to `expand.rs` and the `RustMacroCtx` impl; it
  does not touch the lexer, the emitter, or LLVM. It is *not required* for v2 —
  the `Rc` capture covers it — and is listed only so the option is on record.

Net: **v2 needs no change to JASM's core token machinery.** It needs a FOP-macro
library (new Rust functions registered with `register_macro`) and the two
delimiter macros, all built on the existing `FnMut` + `emit_line` surface.

---

## 2. THE SCHEDULER

### State: `StackCache`

```rust
struct StackCache {
    /// Abstract depth at span entry (cells live in memory below SP-relative
    /// home). Set by fop_begin from the lowering hook.
    entry_depth: i32,

    /// The register window: the top K abstract cells, innermost last.
    /// window[len-1] is the logical TOS. K is small (2 is enough for the
    /// curated FOP set; 3 gives headroom). Slot 0 is always rax (canonical
    /// TOS home) when occupied; shadow slots use rcx, rdx (Win64
    /// caller-saved, matching inline_swap_comp's use of rcx).
    window: Vec<CellVal>,

    /// Lazy rbp delta, in CELLS, not yet emitted. Positive = rbp must still
    /// move DOWN by this many cells (net pushes); negative = UP (net pops).
    /// Materialized as one `sub/add rbp, imm` only at force_settle or when a
    /// memory access needs rbp to address a spilled cell.
    rbp_delta: i32,

    /// True once any store has executed in this span and a later @ has not yet
    /// re-validated: memory-derived cached values (provenance Loaded) are
    /// poisoned and must not be reused. Conservative single bit (see ordering).
    mem_dirty: bool,
}

enum CellVal {
    Reg(Reg),            // value lives in a register (rax/rcx/rdx)
    Const(i64),          // known constant (from lit, or const-folded)
    Loaded { reg: Reg }, // value came from a load (@); subject to mem_dirty
    MemHome(i32),        // value is only in its canonical memory home (cell idx)
}
```

### Transfer function per FOP TYPE

The scheduler reads the FOP's `StackEffect` and applies a pure update to
`StackCache`, *deferring* byte emission as long as it legally can. Sketches:

* **`lit(N)`** (`depth +1`, writes c0): push `Const(N)`. Emit nothing.
* **`fop_dup`** (`+1`, reads c0/writes copy): duplicate the top `CellVal`
  descriptor (a `Const` duplicates for free; a `Reg` becomes a second reference
  — materialized to a real second register only if/when both must be live
  simultaneously). Emit nothing yet.
* **`fop_drop`** (`−1`): pop the top descriptor. If it was a `Reg` whose only
  use was this dropped cell, the would-be load is now dead — **dead-store/
  dead-load elimination emerges**.
* **`fop_swap`** (`0`, reads/writes c0,c1): swap the top two descriptors. Pure
  rename; emit nothing (a later settle materializes positions).
* **`fop_over`** (`+1`, reads c1): push a reference to the c1 descriptor.
* **`fop_plus/minus/times`** (`−1`, reads c0,c1/writes c0): if both top
  descriptors are `Const`, **constant-fold** to one `Const` (emit nothing). If
  one is `Const` and the other a `Reg`, emit the immediate form (`add rax,imm` /
  `imul rax,rax,imm`) — exactly the bytes `fold_plus_comp`/`fold_times_comp`
  already produce. If both are `Reg`, emit the reg-reg form (`add rax,rcx`).
  Result descriptor replaces the two.
* **`fop_fetch`** (`0`, reads c0+MEM/writes c0): if `mem_dirty` does not forbid,
  emit a load into the cell's register; mark result `Loaded`. (See ordering.)
* **`fop_store`** (`−2`, reads c0,c1→MEM): materialize the address cell and the
  value cell into registers, emit the store (`mov [reg_addr], reg_val`), pop
  both descriptors, set `mem_dirty = true`.

### How coalescing EMERGES (not special-cased)

* **Spill+reload cancel.** STC emits, per word, "spill old TOS to NOS
  (`mov [rbp-8],rax`), then load new TOS." In the FOP stream, the spill is only
  the *lazy* depth bump (`rbp_delta`) and a descriptor push; the reload is just
  reading a descriptor. Two adjacent ops that would have spilled-then-reloaded
  the same cell never touch memory at all because the descriptor stays in the
  window. The cancellation is the *absence* of emitted bytes, not a rewrite.
* **Redundant TOS reload.** `dup +` (§7) keeps the duplicated value as a second
  reference; no `mov rax,[rbp]` reload is ever emitted.
* **Dead store.** A value pushed (descriptor) and then dropped before any settle
  produces no store: the lazy spill was never realized.
* **Constant fold lit→op.** `5 *` collapses to `Const` or, against a register,
  to one immediate instruction — same bytes as `fold_times_comp`, but reached
  by the generic const-vs-reg rule rather than a per-operator peephole.

### Memory-effect ordering rule (the `@`/`!` correctness invariant)

> **A value obtained from a load (`@`, provenance `Loaded`) MUST NOT be reused
> across any FOP whose `MemEffect` is `WritesMem` to a possibly-aliasing
> address. Conservatively, ANY store (`fop_store`) sets `mem_dirty`, which
> poisons every `Loaded` descriptor currently in the window: a later use of such
> a cell must re-emit the load from its address rather than reuse the stale
> register.** Loads (`ReadsMem`) may be reordered with respect to each other but
> never moved *before* a preceding store or *after* a following store in program
> order; the scheduler simply never hoists a load past a store. Equivalently:
> stores are full memory barriers for cached loaded values; we make no aliasing
> analysis and assume every store may alias every load.

This is the rule §7's `: h dup @ swap ! ;` exercises.

---

## 3. `force_settle` — the correctness crux (full spec)

`force_settle` runs at **every span boundary**: any FOP with `flow !=
Straight` (cond/uncond branch, a `call`/non-fop word) and the trailing `;`/
`exit`/`[`. Its contract: **after it runs, the machine state is byte-for-byte
the canonical encoding STC would have produced at that exact point** —

* logical TOS in `rax`,
* `[rbp] = NOS`, `[rbp+8] = NNOS`, … for the full live depth,
* `rbp` equal to its canonical value for the current abstract depth
  (`SP0_relative` per macros.masm 14–17),
* every memory cell at and below the live top holding its correct value,
* no live value stranded only in a shadow register.

### Materialization order (deterministic, proven equal to STC)

Let the abstract window be `c_{d-1} … c_1 c_0` (c_0 = logical TOS), entry depth
`E`, and `rbp_delta` the pending cell delta. STC's canonical state for depth
`D = E + net_depth` is: `rax = c_0`; for `i in 1..D`, `[rbp + (i-1)*8] = c_i`;
`rbp = SP0 - (D-1)*8`. `force_settle` produces exactly that, in this order:

1. **Resolve the final `rbp` first, on paper.** Compute the target `rbp` from
   the abstract depth `D`. Do **not** emit the `add/sub rbp` yet — we need the
   *old* `rbp` to address already-spilled cells while we store the window. Keep
   both old and target as known offsets.

2. **Settle deep cells outward-in (c_{d-1} first, c_0 last).** For each window
   cell `c_i` with `i` from high to 1 (i.e. NOS-ward cells before TOS):
   materialize its `CellVal` into its canonical memory home `[rbp_old +
   home_off(i)]` where `home_off` is computed from the *target* depth so that
   after the single `rbp` move (step 4) the cell lands at `[rbp + (i-1)*8]`.
   * `Const(n)` → `mov qword ptr [home], n` (or via a scratch reg if n doesn't
     fit imm32).
   * `Reg(r)`/`Loaded{r}` → `mov [home], r`.
   * `MemHome(j)` already in place → if `home == its current home`, emit
     nothing; else `mov` via scratch. Settling outer cells first guarantees we
     never overwrite a not-yet-saved cell (the homes are disjoint and ordered).

3. **Place TOS (c_0) into `rax` last.** If c_0 is already `Reg(rax)`, nothing.
   If `Const(n)` → `mov rax, n`. If in a shadow reg → `mov rax, rshadow`. If
   `MemHome` → `mov rax, [home]`. Doing TOS last means the spills in step 2
   were free to use `rax` as scratch.

4. **Emit the single `rbp` adjustment.** `sub rbp, k*8` (net pushes) or `add
   rbp, k*8` (net pops), where `k = |rbp_delta_final|`. This is exactly one
   `stk`-style instruction (macros.masm `stk`, 199) — the same net adjustment
   STC accumulates across its per-word `stk(in,out)` calls.

5. **Clear scheduler state for the next span:** window now mirrors memory
   (`MemHome` everywhere), `rbp_delta = 0`, `mem_dirty = false`.

### Proof of equality to STC

STC's invariant (macros.masm 11–22) is maintained *after every word*: TOS in
rax, NOS… in `[rbp…]`, rbp tracking depth. Over a straight-line FOP span, the
abstract stack the scheduler simulates is *definitionally* the same value stack
STC would compute (each FOP's `StackEffect` is the abstract image of the same
primitive's concrete effect — §5 makes the bytes identical). `force_settle`
writes precisely the value of each abstract cell to the exact home STC uses for
that depth, and sets rbp to STC's depth-derived value. Therefore the observable
state (registers + the entire `[rbp …]` region up to the live top + rbp) equals
STC's at that boundary. Because every boundary (branch target, call, `;`) is a
point a branch *into* this code, or STC code *after* this code, can observe, and
each such point is force-settled, **any observer sees canonical state.** The
differential fuzzer (§8) checks exactly this region byte-for-byte.

One subtlety the spec nails (the v1 "force_settle unspecified — Fatal" gap):
cells must be settled **before** the `rbp` move, addressed off the *old* `rbp`
using target-derived offsets, so the lone `rbp` adjustment at the end is correct
for all of them simultaneously. Materializing TOS last and deep-cells-first is
what makes a single `rbp` move sufficient.

---

## 4. LITERAL / WORD / STATE CAPTURE — producing the typed-fop stream

This is the §critique "Fatal" item: `dh_comp` cannot see literals because
numbers/floats/locals never reach it. v2 does **not** hook `dh_comp`. It adds
*one lowering hook in the compile dispatcher* that sits at the single point all
three token classes converge: `interpret_source` in `interp.masm`.

### Where the hook lives

`interpret_source` (interp.masm 38) is the *only* compile-time token pump. It
already routes the four cases at distinct labels:

* word found, compile state → `.compile_ct` → `call rcx` (rcx = `dh_ct`), 144.
* integer → `.got_number` → `call literal`, 235/241.
* float → `.got_float` → `call fliteral`, 244/250.
* local (compile state) → `check_local_emit_word`, 69.

v2 inserts a **span accumulator** in front of these. While `STATE=1` *and* the
current token is FOP-eligible (a word whose `dh_comp`/`dh_ct` is one of the
curated FOP primitives, OR an integer literal, OR — later — a local read), the
dispatcher does **not** call `literal`/`dh_ct` immediately. Instead it appends a
typed-FOP record to a per-definition buffer:

* integer `n` at `.got_number` → append `lit(n)` (this is the literal-capture
  fix: the number is caught *here*, where the dispatcher already has it in TOS,
  before `literal` would emit `call do_lit`).
* float at `.got_float` → append `flit(bits)` (float FOPs are a later phase; for
  now floats *end the span* — they are a `Flow::SpanEnd`-class token that forces
  a settle then falls back to `fliteral`).
* FOP word at `.compile_ct` → append the word's FOP (`fop_dup`, `fop_plus`, …),
  identified by a small tag on the dict header (reuse `dh_stk`/`dh_comp` to mark
  "this primitive is FOP-id k").
* local read → append `local_fetch(off)` (later phase; until then, span-end +
  fallback to `check_local_emit_word`).

Any **non-FOP** token (an ordinary word → `Flow::Call`, a control-flow immediate
→ `CondBranch`/`UncondBranch`, `;`/`[`/`postpone`/an immediate word executing →
`SpanEnd`) **terminates the span**: the accumulator is flushed (see below),
then the original dispatch runs unchanged (`call rcx`, `literal`, etc.). This
keeps the hybrid (§6) trivially correct: anything the scheduler doesn't model
falls through to today's STC path.

### How the accumulated span becomes bytes

On span termination the dispatcher flushes the FOP buffer by building one
`proc(span_NNNN) … endp()` JASM source string whose body is `fop_begin(depth)`,
the accumulated FOP macro calls in order, `fop_settle()`, `next()` — then it
hands that to `rt_code_compile_body`'s sibling (`rt_span_compile`, modeled on
runtime.rs 2704) which runs the shared `Assembler` (with the FOP macros
registered alongside `stk`) to expand+emit, JITs into the near `CodeArena`, and
returns `fn_addr`. The dispatcher then emits **one `call rel32` to `fn_addr`**
into the colon body (exactly `compile_comma`, compile.masm 99) — or, for short
spans, copies the emitted bytes inline. The span function is canonical at entry
and exit (force_settle), so a plain `call` composes with surrounding STC code
with no glue. This is the *same* arena mechanism `CODE:` already uses
(runtime.rs 2769–2795, jit.rs `new_in_arena`).

### Multi-line defs, `[ ]`, postpone, immediate words — handled by construction

Because the hook lives *in the token pump itself*, it inherits all of the
pump's existing behavior:

* **Multi-line `:` defs** — `interpret_source` is re-entered per refill; the FOP
  buffer is keyed to the current definition (cleared by `:`/`colon`, compile.masm
  2132, alongside the existing `LOCALS_COUNT`/`TAIL_CALL` resets), so a span
  simply continues across the line boundary until a terminator.
* **`[` / `]`** — `[` is a `SpanEnd` terminator: the span flushes, STATE→0, and
  interpreted code runs normally. `]` re-enters compile and starts a fresh span.
* **`postpone`** (compile.masm 2536) — a non-FOP immediate path; it terminates
  the span and runs unchanged.
* **immediate words executing mid-definition** — reach `.compile_ct` with `ct =
  execute`; they are `SpanEnd` (they may emit control flow / call back into the
  compiler), so the span flushes before they run.

The lowering hook therefore sees literals (the fix), words, floats, locals, and
all the compiler state words — at the one place they are all observable.

---

## 5. TWO SOURCES OF TRUTH — fop templates vs kernel primitive bodies

The critique's real objection: if `fop_dup`'s emit-template and the kernel's
`dup` primitive (and `inline_dup_comp`) drift, optimized code silently diverges
from interpreted code. v2 makes them **one source** where possible and
**golden-byte-locked** everywhere else.

* **Single source (ideal, and achievable today for the inline set).** The byte
  recipes in `inline_dup_comp` / `inline_drop_comp` / `inline_swap_comp` /
  `inline_over_comp` (compile.masm 753–832) are *already* the canonical
  "inline this primitive" bytes. v2's `fop_dup`/`drop`/`swap`/`over`
  emit-templates are defined to emit **those exact byte sequences** in their
  all-memory residency (window empty / canonical). We extract the byte tables
  once (a Rust `const DUP_BYTES: [u8; 8] = …` mirroring compile.masm 755–762)
  and **both** `inline_*_comp` and `fop_*` reference the same table via a
  generator. Concretely: add a build step that emits `kernel/inline_bytes.inc`
  from the Rust FOP table, and have `inline_dup_comp` `@include` it — so the
  kernel inline helper and the FOP template are literally the same bytes by
  construction.
* **`+ - * @ !` have NO `inline_*_comp` referent today** (charter note
  confirmed: compile.masm has `fold_*_comp` for `<lit> op`, and bare `+ * @ !`
  fall to `compile_comma` → a `call`). So for the reg-reg and load/store forms
  there is nothing to share with. Here we use a **golden-byte sync test**: a
  Rust test assembles `: t a b + ;`-shaped spans through the FOP path and
  through a reference that calls the *actual kernel `plus` primitive*, runs both
  on a battery of stacks, and asserts identical results AND identical settled
  stack images. The FOP template for `fop_plus`'s reg-reg case (`add rax,rcx`)
  and the immediate case (reusing `fold_plus_comp`'s `add rax,imm` bytes,
  compile.masm 191–200) are pinned by golden bytes checked in. Any future edit
  to the kernel `plus` body that changes semantics breaks the differential test
  (§8), not production silently.

Rule of record: **a FOP template's bytes must be either (a) `@include`d from the
same table the kernel inline helper uses, or (b) covered by a golden-byte test
that also exercises the live kernel primitive.** No third option.

---

## 6. HYBRID WITH STC

* **What optimizes:** maximal straight-line runs of FOP-eligible tokens
  (`Flow::Straight`): the curated arithmetic/stack/lit set (and later @/!/
  locals). Everything else stays pure STC.
* **Span terminators are exactly the non-straight `Flow` classes:** `CondBranch`
  / `UncondBranch` (if/then/begin/again/while/until/do/loop/leave — compile.masm
  1163+), `Call` (any non-FOP dict word), `SpanEnd` (`;`/`exit`/`[`/`postpone`/
  immediate execution). At each, the span force-settles and STC resumes. This is
  the same boundary discipline STC's tail-call logic already respects
  (semicolon 2198 only optimizes a CALL *immediately* before RET).
* **Optimized span → one near-arena function** (preferred; reuses
  `rt_code_compile_body`/`CodeArena`, runtime.rs 2769) wired into the body with a
  single `call rel32` (`compile_comma`). For very short spans the dispatcher may
  instead `rep movsb` the emitted bytes inline (mirroring `inline_comma_word`,
  compile.masm 715) to dodge the call overhead. Either way the dict word the
  user is defining is unchanged in shape; only its body content differs.
* **Tail-call still works:** if the *last* thing in a definition is a single
  call to a span function, `;` patches E8→E9 exactly as today (semicolon 2216),
  because the span function ends in `ret` (its `next()`).

---

## 7. WORKED EXAMPLES (typed-token level)

Notation: window shown TOS-rightmost; `rbp_delta` in cells; `Δ`=emitted bytes.

### `: foo dup + ;`  →  FOPs: `fop_dup`, `fop_plus`, `SpanEnd(;)`

Entry depth E=1 (foo takes `( a -- )`-ish; abstractly one input cell `a` lives
in rax at entry).

| step      | window (desc)        | rbp_delta | emitted |
|-----------|----------------------|-----------|---------|
| begin(1)  | `[Reg(rax)=a]`       | 0         | —       |
| fop_dup   | `[Reg(rax)=a, Ref(a)]` | +1 (lazy) | — (dup is a second reference, no spill yet) |
| fop_plus  | `[Reg(rax)=a+a]`     | 0         | both operands are the same value `a`; reg-reg add of a value with itself → `add rax, rax` (one instr) |
| settle    | `[MemHome TOS]`      | 0         | TOS already in rax; rbp_delta net 0 → no rbp move |
| ; (SpanEnd)|                     |           | `ret` (next), then `;` may E8→E9 if the span was a tail call |

Final asm for the body span: `add rax, rax ; ret`. STC would have emitted
`mov [rbp-8],rax ; sub rbp,8` (dup) `; mov rax,[rbp] ; add rax,[rbp] (wait: bare
+ is a call)` — i.e. a spill + a call to `plus`. The FOP path collapses it to a
single `add rax,rax`, and force_settle confirms the stack image is identical
(NOS region untouched, rbp unchanged, rax = a+a).

### `: bar 5 * 2 + ;`  →  `lit(5)`, `fop_times`, `lit(2)`, `fop_plus`, `;`

Entry depth E=1, input `x` in rax.

| step       | window                         | emitted |
|------------|--------------------------------|---------|
| begin(1)   | `[Reg(rax)=x]`                 | —       |
| lit(5)     | `[Reg=x, Const(5)]`            | —       |
| fop_times  | `[Reg=x*5]`  (Const×Reg → imm) | `imul rax, rax, 5` (= `fold_times_comp` imm8 bytes, compile.masm 248–251) |
| lit(2)     | `[Reg=x*5, Const(2)]`         | —       |
| fop_plus   | `[Reg=x*5+2]` (Const+Reg→imm)  | `add rax, 2` (= `fold_plus_comp` imm8 bytes, 191–194) |
| settle/;   | canonical                      | `ret`   |

Body span: `imul rax,rax,5 ; add rax,2 ; ret`. The two `lit`s never touched
memory or `do_lit`; constants folded into the operators. This matches what a
hand fold would do and reuses the kernel's own fold byte recipes.

### `: g @ 1+ ;`  →  `fop_fetch`, `lit(1)`, `fop_plus`, `;`  (load demo)

Entry depth E=1, address `a` in rax.

| step       | window                          | mem_dirty | emitted |
|------------|---------------------------------|-----------|---------|
| begin(1)   | `[Reg(rax)=a]`                  | false     | —       |
| fop_fetch  | `[Loaded(rax)=v]` (ReadsMem)    | false     | `mov rax, [rax]` |
| lit(1)     | `[Loaded(rax)=v, Const(1)]`     | false     | —       |
| fop_plus   | `[Reg(rax)=v+1]`               | false     | `add rax, 1` |
| settle/;   | canonical                       |           | `ret`   |

Body: `mov rax,[rax] ; add rax,1 ; ret`. No store occurred, so the loaded value
was freely usable by `+`.

### `: h dup @ swap ! ;`  →  `fop_dup`, `fop_fetch`, `fop_swap`, `fop_store`, `;`  (store/ordering demo)

Entry depth E=1; input is an address `a` in rax. Stack effect: `( a -- )` where
it does `a @ a !`-ish (reads `*a`, writes it back to `a`) — exercises the
aliasing barrier because the loaded value and the store target derive from the
same `a`.

| step       | window (desc, TOS right)             | mem_dirty | emitted | notes |
|------------|--------------------------------------|-----------|---------|-------|
| begin(1)   | `[Reg(rax)=a]`                       | false     | —       | |
| fop_dup    | `[Reg(rax)=a, Ref=a]`               | false     | —       | two refs to `a` |
| fop_fetch  | `[Reg=a, Loaded=v]`  (reads c0=a)    | false     | `mov rcx,[rax]` (load into shadow; keep `a` live in rax) | `v=*a` |
| fop_swap   | `[Loaded=v, Reg=a]`  (rename)        | false     | —       | now TOS=a (rax), NOS=v (rcx) |
| fop_store  | `[]`  (reads v=c1, a=c0 → MEM)       | **true**  | `mov [rax], rcx` | store v to *a; sets mem_dirty |
| settle     | empty → depth E−2+... = E−1? (h is `( a -- )`, net −1 from the input) | | `add rbp, 8`? (only if abstract depth dropped below entry — here net is `dup(+1) fetch(0) swap(0) store(−2) = −1`, so final depth = E−1 = 0; rbp moves up one cell to restore SP0-relative depth 0) | |
| ;          |                                      |           | `ret`   | |

The ordering rule in action: `fop_fetch` produced `Loaded v` in `rcx`. The
**store does not reuse any loaded value that follows it** — here the value being
stored (`v`) was loaded *before* the store, which is legal (program order
preserved: load, then store). `mem_dirty` is set *after* the store; had there
been a *second* `@` of the same-or-aliasing address *after* the `!`, the
scheduler would see `mem_dirty=true` and **re-emit the load** (`mov …,[addr]`)
rather than reuse the pre-store register — that is the barrier. The scheduler
never hoists the load past the store, and never caches a loaded value across the
store. Final settled image: memory at `[a]` now holds `v`, abstract stack
emptied to depth 0, rbp restored — identical to running STC `dup @ swap !`.

---

## 8. PHASED PLAN

**Phase 0 — proof slice (smallest provable end-to-end).**
Implement exactly two FOPs — `fop_dup` and `fop_plus` — plus `fop_begin` /
`fop_settle`, the `StackCache` with a 2-cell window, lazy `rbp_delta`,
spill/reload-cancel and reg-reg/immediate `+`. Wire `rt_span_compile`
(clone of `rt_code_compile_body`) and the lowering hook for *integers and these
two words only*; every other token is a span terminator. Spans → near-arena
function called via `compile_comma`. This proves: stateful shared cache across
FOP macros, the lowering hook capturing literals, force_settle, arena wiring,
hybrid fallthrough — the whole spine — on `: foo dup + ;` and `: bar 5 + ;`.

**Phase 1 — complete the straight-line set.** Add `fop_drop/swap/over/minus/
times`, const-folding, 3-cell window. Golden-byte-lock the bare-op templates
(§5). 

**Phase 2 — memory FOPs.** `fop_fetch`/`fop_store` + the `mem_dirty` barrier.
This is where the aliasing tests bite hardest.

**Phase 3 — locals & floats.** `local_fetch(off)` (reuse the 15-byte inline from
`check_local_emit_word`), then float FOPs.

**Phase 4 — single-source the inline bytes.** Generate `kernel/inline_bytes.inc`
from the Rust FOP table; make `inline_dup_comp` et al `@include` it (§5).

### Test strategy that catches silent stack corruption

* **Differential fuzz vs STC oracle (the load-bearing test).** Generate random
  FOP-eligible token sequences (`{dup,drop,swap,over,+,-,*,@,!,lit}` once each
  phase lands). For each: (a) compile through the *current STC path* into a word
  `oracle`; (b) compile through the *FOP path* into a word `opt`. Seed an
  identical, randomized data-stack region (and, for @/!, a randomized memory
  scratch block). Run each in a `Wf64Session` (tests/harness.rs pattern). **Then
  compare not just the result but the ENTIRE touched stack region AND `rbp` AND
  the scratch memory block, byte-for-byte.** A scheduler bug that leaves a stale
  NOS cell, an off-by-one `rbp`, or a missed aliasing reload shows up as a region
  mismatch even when the top value happens to match — this is the silent-
  corruption detector force_settle's spec is written to satisfy.
* **Golden-byte tests** for every FOP template (§5), asserting the emitted bytes
  equal the checked-in recipe and (for shareable ones) equal the kernel inline
  helper's bytes.
* **Boundary tests:** a span immediately followed by `if`, by a non-FOP call, by
  `;` with and without tail-call eligibility — assert force_settle leaves
  canonical state and the subsequent STC code/branch observes it correctly.
* **`mem_dirty` adversarial tests:** `a @ a !`, `a @ b ! a @` (must reload after
  the store), `a ! a @` — assert the post-store load is re-emitted.

---

## 9. RISKS / OPEN QUESTIONS

1. **Shared-cache reset discipline (biggest correctness risk).** The
   `Rc<RefCell<StackCache>>` persists in the session-lived `Assembler`. If
   `fop_begin` is ever missing, skipped on an error path, or a span aborts
   mid-flush, the next span inherits a dirty cache and emits wrong bytes
   *silently*. Mitigation: `fop_begin` hard-resets and `fop_settle` asserts the
   cache is canonical-and-empty (returns `Err` otherwise, failing the
   `assemble`); plus the differential fuzzer. Still, this is the single thing
   most likely to cause a silent stack-corruption bug, and the reason Phase 0
   exists. **This is the biggest remaining risk.**
2. **Entry depth knowledge.** force_settle and rbp math need the abstract entry
   depth, but Forth words have no declared arity. We pass a conservative entry
   depth (the span's net consumption, clamped) so the scheduler only ever
   *spills to* memory homes it can prove exist; deep cells it didn't bring into
   the window are addressed by `MemHome` and never moved. Open: prove the
   conservative depth never under-counts live cells a branch target reads.
3. **Branch targets mid-span.** A label *inside* what looked like a straight
   span (e.g. a `begin` the user wrote between two arithmetic ops) must force a
   settle *at the label*, because control can re-enter there with a different
   abstract state. The lowering hook already treats control-flow immediates as
   span terminators, so this is handled — but any *future* construct that
   introduces a label without going through those immediates would break the
   invariant. Document: "a new label inside a span ⇒ split the span."
4. **ROI vs just adding bare-op inline helpers.** The critique's thin-ROI point
   stands for Phase 0–1: `dup +` and `5 * 2 +` are also reachable by adding
   `inline_plus_comp` peepholes in pure MASM. The FOP scheduler only clearly
   wins once *cross-op* coalescing and const-folding chains appear (Phase 1+) and
   especially for @/!/locals (Phase 2–3). If Phase 1 doesn't beat hand peepholes
   on a representative corpus, stop and ship the peepholes instead.
5. **Inline-vs-call threshold.** Choosing when to inline span bytes vs emit a
   `call rel32` to an arena function is a tuning knob; a wrong threshold
   regresses code size or speed. Needs measurement, not a priori choice.
6. **`emit_line` tokens are final.** Anything a FOP emits is not re-expanded
   (expand.rs ~335), so FOP templates must emit fully-resolved asm text (no
   nested macro calls). This is fine for fixed byte recipes but forbids
   "FOP emits `stk(…)`"-style convenience — FOPs compute rbp deltas in Rust
   directly, like `stk` itself does (macros.rs 188–195).

---

## Summary (≈14 lines)

* **Type shape:** each FOP carries `{ StackEffect{reads:[Cell], writes:[Cell],
  depth:i8}, Flow{Straight|Cond|Uncond|Call|SpanEnd}, MemEffect{None|ReadsMem|
  WritesMem} }`; emit-templates are separate, parameterized by per-cell
  residency. The typed table for lit/dup/drop/swap/over/+/−/×/@/! is given and
  is a sufficient instruction model — scheduling needs only these three axes.
* **JASM realization:** chosen (a) STATEFUL fop macros sharing one
  `Rc<RefCell<StackCache>>` via `FnMut` closure capture (`RustMacroFn =
  Box<dyn FnMut>`, expand.rs 189); expansion order IS schedule order, so **no new
  pass phase and no JASM-core change** is required. The only addition is two
  ordinary stateful delimiter macros, `fop_begin(depth)`/`fop_settle()`; an
  optional `State.user: HashMap<String,Box<dyn Any>>` slot is recorded but not
  needed.
* **force_settle (one paragraph):** at every non-straight `Flow` and the trailing
  `;`, compute target `rbp` from abstract depth but defer it; settle deep cells
  (NOS-ward) first into their target memory homes addressed off the *old* rbp,
  place TOS into rax last (so spills may scratch rax), then emit the single
  `sub/add rbp,k*8`; result equals STC's canonical encoding (TOS=rax,
  `[rbp+i*8]`=cells, rbp depth-correct) byte-for-byte, proven because each FOP's
  StackEffect is the abstract image of the same primitive and §5 pins the bytes.
* **Memory-ordering rule:** any `fop_store` sets `mem_dirty`, which poisons every
  `Loaded` register; a later use re-emits the load from its address; loads are
  never hoisted past a store. Conservative: every store may alias every load.
* **Literal capture:** one lowering hook in `interpret_source` (interp.masm 38)
  — the single point where words (`.compile_ct`), integers (`.got_number`),
  floats (`.got_float`), and locals (`check_local_emit_word`) converge — appends
  typed FOPs (catching the integer as `lit(n)` *before* `literal` runs, which is
  the v1-Fatal fix); non-FOP tokens flush the span and fall through to today's
  STC path, so `[ ]`/postpone/immediate/multi-line all work by construction.
* **Biggest remaining risk:** the session-lived shared `StackCache` must be
  reset exactly once per span; a missing/aborted `fop_begin`/`fop_settle` causes
  *silent* cross-span stack corruption — mitigated by hard reset + canonical
  assert + the differential fuzzer that compares the whole touched stack region,
  rbp, and scratch memory against the STC oracle.
