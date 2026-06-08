# Tracing Forth in WF64

This note describes the debugging tools WF64 now has for following Forth execution without dropping immediately into raw JIT or Windows exception state.

The short version is:

1. `trace` prints each interpreted word just before it executes, together with the current data stack.
2. `.s` prints the live stack without consuming it.
3. `BRK` and `INT3` print a Forth-oriented state dump and then trigger a native breakpoint.
4. The Rust harness can drive all of this through ordinary `eval(...)` calls, so debugging stays close to user-visible behavior.

That combination turned out to be enough to find a real stack-discipline bug in `execute` without inventing a separate debugging runtime.

## Why WF64 needed this

WF64 is not interpreting a bytecode with a friendly VM frame object sitting nearby. It is a subroutine-threaded Forth where:

1. `RAX` is the cached data-stack top (`TOS`)
2. `RBP` is the data-stack pointer (`DSP`), pointing at `NOS`
3. `RBX` is the user area pointer (`UP`)
4. `RSP` is the real machine return stack and also the Forth return stack

That design is fast and direct, but it makes debugging less forgiving. A one-cell mistake in stack discipline does not stay local for long. If the cached `TOS` and memory stack drift apart, later failures can show up far away from the real cause.

For that reason, the useful question in WF64 debugging is usually not "did this line run?" but:

1. what word is about to run,
2. what does the stack look like at that exact point,
3. what does the return stack look like when things break,
4. what user-area state is live when the fault happens.

## The user-facing tools

### `trace`

`trace` enables per-word tracing in the outer interpreter. Once enabled, every interpreted word prints before execution.

Example:

```forth
trace
only get-order
bye
```

Typical output shape:

```text
» only              ( empty )
» get-order         ( empty )
 ok
```

The format is intentionally simple:

1. the word name,
2. the logical stack in TOS-first order,
3. one line per executed word.

`notrace` turns it back off.

### `.s`

`.s` is still the cheapest spot check. It prints the current logical stack without consuming it.

That matters in WF64 because the logical stack is reconstructed from:

1. the cached `TOS` register,
2. the memory stack below `DSP`,
3. the empty-stack convention derived from `SP0`.

So `.s` is often the first check after a suspicious word sequence.

### `BRK` and `INT3`

`BRK` and `INT3` are non-fatal debugging words for the live system.

Both do the same two-step sequence:

1. call a Rust helper that prints a Forth-tuned state dump,
2. execute `int 3` so a native debugger can trap at the same point.

The state dump includes:

1. the data stack,
2. the return stack,
3. selected user variables,
4. the current search-order state.

This is much more useful than a raw register dump when the bug is semantic rather than purely native.

## How `trace` is wired

The trace switch is a user-area flag:

```text
user_TRACE = 0x15A8
```

The words themselves are trivial:

```masm
proc(trace_word)
    mov     qword ptr [UP + user_TRACE], 1
    next()
endp()

proc(notrace_word)
    mov     qword ptr [UP + user_TRACE], 0
    next()
endp()
```

The interesting part is in the interpreter. In interpret state, WF64 already knows:

1. the header name token,
2. the xt it is about to execute,
3. the current cached `TOS`,
4. the current `DSP`,
5. `SP0` from the user area.

So the interpreter checks `user_TRACE` and, if enabled, calls `rt_forth_trace` before `execute`:

```masm
cmp     qword ptr [UP + user_TRACE], 0
jz      .do_exec
push    rdx
lea     rcx, [r9 + dh_nt]
mov     rdx, TOS
mov     r8,  DSP
mov     r9,  [UP + user_SP0]
win64_call(rt_forth_trace)
pop     rdx
```

That placement is deliberate. It shows the machine state at the last moment before the word runs, not after the damage is already done.

On the Rust side, `rt_forth_trace` reconstructs the logical stack from the cached register and memory stack and prints a compact one-line record.

## How `BRK` is wired

`BRK` and `INT3` are implemented in the kernel in [kernel/io.masm](e:/WF64/kernel/io.masm). They preserve `TOS`, marshal five arguments to `rt_forth_brk`, restore the Win64 call frame correctly, and then fire `int 3`.

The Rust helper prints:

1. the logical data stack,
2. up to 16 return-stack cells,
3. `BASE`, `STATE`, `HERE`, `LATEST`,
4. `CURRENT`, `FORTH-WID`, `ORDER_COUNT`, and `CONTEXT[i]`.

That last part is important. Search-order bugs often look like parser or interpreter failures until you see the actual live wordlist state.

## A practical workflow

When a Forth-level behavior looks wrong in WF64, the shortest useful loop is usually:

1. reproduce it with a small `eval(...)` input in the harness,
2. add `trace` if the failure is about word execution order,
3. add `.s` immediately after the suspicious word if the failure is about stack effect,
4. use `BRK` if you need the return stack or user-area state,
5. reduce to the smallest word sequence that still fails,
6. only then inspect the assembly.

That order matters because the first readable symptom is often enough to avoid an unnecessary deep dive.

## Case study: the `only get-order` failure

The recent search-order bug is a good example.

The visible failure looked like this:

```forth
only get-order
```

The test expected the stack to end as:

```text
[ 1 root_wid ]
```

Instead it came back empty.

At first glance that suggested a search-order bug in `only`, `get-order`, or their user-area state. But the new diagnostics made the shape of the failure much clearer.

### Step 1: trace showed the right words running

With tracing enabled, WF64 showed:

```text
» only              ( empty )
» get-order         ( empty )
```

That immediately ruled out a plain lookup failure. The words were found and executed.

### Step 2: `.s` showed the stack was empty after the sequence

Running:

```forth
only get-order .s
bye
```

showed the stack was still logically empty at the point where `.s` executed.

That meant `get-order` was not merely returning the wrong values. Its results were being lost, or the stack bookkeeping was already broken before the next word.

### Step 3: a smaller reproducer removed search order from the picture

The decisive test was simpler than `only get-order`:

```forth
42
.s
bye
```

That failed too.

Once a plain literal could disappear across the same execution path, the search-order code stopped being the main suspect. The bug had to be in generic interpreter or execution plumbing.

### Step 4: the actual bug was in `execute`

The problem turned out to be the empty-stack convention around `execute` and `perform`.

WF64 uses a cached `TOS`, so the logical stack can contain exactly one item even when there is nothing below `DSP`. In that state, `DSP` is already above the memory-backed portion of the stack.

The old `execute` path always did this:

```masm
mov     rcx, TOS
mov     TOS, [DSP]
add     DSP, cell
jmp     rcx
```

That is correct when there is a real `NOS` in memory. It is wrong when the xt is the only logical item. In that case it advanced `DSP` past the empty-stack sentinel and invented a nonexistent `NOS`.

The fix was to detect that case and leave the stack logically empty instead of over-popping it.

So the debugging path was:

1. trace proved execution happened,
2. `.s` proved the stack effect was wrong,
3. a smaller reproducer proved the bug was generic,
4. the assembly read became obvious and short.

That is exactly the kind of debugging loop these tools are meant to support.

## Why this level of tooling is enough for now

WF64 does not need a heavy debugger protocol yet.

At the current stage, the highest-value diagnostics are:

1. precise pre-execution word tracing,
2. accurate logical stack printing,
3. a breakpoint dump that speaks in Forth concepts,
4. reproducible harness tests that can drive the same code paths.

Those four pieces are already enough to answer most of the practical questions that come up while bringing up a Forth system.

## Files involved

The current tracing and breakpoint path lives mainly in:

1. [kernel/interp.masm](e:/WF64/kernel/interp.masm)
2. [kernel/io.masm](e:/WF64/kernel/io.masm)
3. [kernel/execute.masm](e:/WF64/kernel/execute.masm)
4. [src/runtime.rs](e:/WF64/src/runtime.rs)
5. [src/lib.rs](e:/WF64/src/lib.rs)
6. [tests/harness.rs](e:/WF64/tests/harness.rs)

## Closing note

The useful outcome of the detour was not just the one bug fix. WF64 now has a debugging path that stays at the Forth level as long as possible and only drops to native details when it has to.

That is the right bias for this project. Most of the hard bugs here are not Windows ABI bugs or JIT loader bugs. They are Forth semantics expressed through a very thin machine-level substrate. The better the system can explain itself in Forth terms, the faster those bugs get fixed.