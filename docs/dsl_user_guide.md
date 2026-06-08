# LET and CODE: — a working guide

Two tiny escape hatches for when stack Forth isn't the right tool.

`LET` is an infix algebra mini-language for dense floating-point math.
`CODE:` is raw assembly with the WF64 macro vocabulary, for primitives
the kernel doesn't already provide.

Both compile to native machine code at the moment you type them, get
spliced into the dictionary as ordinary Forth words, and from then on
are indistinguishable from any other word — you can call them, compose
them, redefine them, and forget them.

This guide is meant to be read top to bottom in one sitting and then
kept open while you're working.

---

## When to reach for which

Use `LET` when:

- You're writing a numerical function (geometry, physics, signal
  processing, shaders, statistics).
- The body is naturally an expression, not a sequence of stack moves.
- You want LLVM's view of register allocation, common-subexpression
  elimination, and FMA fusion instead of pop-pop-push at every step.

Use `CODE:` when:

- You need an instruction Forth doesn't expose (POPCNT, rdtsc,
  prefetch, SIMD, RDRAND, anything intrinsic).
- You're chasing the last few percent on a hot primitive that the
  literal-folding peephole couldn't crunch enough.
- You want to talk to a Win32 API or a Rust runtime function the
  kernel doesn't already wrap.
- You like assembly.

Use neither when:

- A regular `: …  ;` definition works. Stack Forth is short and
  readable; these DSLs cost more lines for trivial things.

---

## LET in 90 seconds

```forth
: area    LET (r) -> (a) = pi * r * r END ;
2.0 area f.    \ → 12.566370
```

Read the signature left to right:

- `LET (r)` — declare one input named `r`.
- `-> (a)` — declare one output named `a`.
- `= pi * r * r` — the body. Every name on the right must be either an
  input, a WHERE binding (below), a literal number, or one of the
  built-in constants `pi` / `e`.
- `END` — closes the form. The enclosing `;` then closes the colon
  definition as usual.

LET inputs and outputs follow Forth's FP-stack convention: the
*last-declared* input is TOS at call time, and the *last-declared*
output ends up at TOS after the call. So in `: f LET (x, y) -> (a, b)
…`, you push x first, then y; on return, b is on top of the stack.

### Multiple results

```forth
: divmod  LET (a, b) -> (q, r) =
              a / b, a - q * b
              WHERE q = floor(a / b)
          END ;
17.0 5.0 divmod f. f.    \ prints r=2 then q=3
```

The body's comma-separated expression list is in declaration order, so
`q = a / b`, `r = a - q * b`. Forth `f.` prints TOS first, which is the
*last* declared output (`r` here).

### WHERE bindings

If you want to name a sub-expression — to give it a meaningful name or
to avoid repeating it — use `WHERE`:

```forth
: dist2d  LET (x1, y1, x2, y2) -> (d) =
              sqrt(dx * dx + dy * dy)
              WHERE dx = x2 - x1
              WHERE dy = y2 - y1
          END ;
```

WHERE bindings can appear in any order — LET topologically sorts them
by dependency. Cycles are reported at compile time.

### What's in the box

Arithmetic: `+  -  *  /` and unary `-`.

Power: `**` (right-associative). Underneath it calls `pow`, so any
exponent works.

Comparisons: `<  >  <=  >=  ==  !=`. Each yields 1.0 (true) or 0.0
(false) — a real number you can multiply, add, or feed to `select`.

Conditional: `select(cond, then, else)`. Branchless. `cond` is
"truthy" if non-zero.

SSE intrinsics (single-instruction): `sqrt  abs  min  max  floor  ceil
round  trunc`.

libm: `sin  cos  tan  asin  acos  atan  atan2  exp  log  log2  log10
pow  hypot  fmod`.

Constants: `pi`, `e`.

### Things that surprise people

- LET inputs and outputs live in xmm6-xmm15 (callee-saved) so libm
  calls don't trash them. The raw budget is 10 named values, but
  every call to a libm function (sin, cos, pow, ...) gets lifted by
  the A-normal-form pre-pass into its own synthetic WHERE binding,
  which consumes one of those slots. Realistic ceiling for a libm-
  heavy body is **about 7-8 user-named values** (inputs + WHEREs);
  pure-arithmetic bodies can use all 10.

- `==` is comparison; the body uses one `=` for the binding and `==`
  for equality.

- Forth-style `( comment )` doesn't work inside LET. The LET parser
  does support `\` to end of line and `( … )` *if* the open paren is
  followed by whitespace — same convention the kernel uses.

- A local binding shadows a built-in constant. `LET (e) -> (y) = e END`
  treats `e` as the input, not 2.71828…. Use a different name if you
  want the constant.

- `select(cond, then, else)` evaluates **both** branches before
  picking. If `then` or `else` is expensive (a `pow` call, say), it
  runs whether you needed it or not. That's the cost of being
  branchless. If you genuinely need a branch — say to avoid `sqrt` of
  a negative — write two separate words and let the caller pick.

---

## LET — five practical problems

### 1. Point in circle?

```forth
: in-circle   ( cx cy r px py -- f )
    LET (cx, cy, r, px, py) -> (inside) =
        (dx*dx + dy*dy <= r*r) * -1
        WHERE dx = px - cx
        WHERE dy = py - cy
    END ;
```

Returns -1.0 or 0.0, ready to feed straight into IF. (We multiply the
comparison's 1.0 by -1 to match Forth's "true = -1" convention.)

### 2. Linear interpolation

```forth
: lerp   ( a b t -- result )    \ result = a + t*(b-a)
    LET (a, b, t) -> (y) = a + t * (b - a) END ;
```

Compiles to roughly: `subsd; mulsd; addsd`. Three instructions.

### 3. Quadratic formula, real roots

```forth
: quad-real?   ( a b c -- f )       \ true iff discriminant ≥ 0
    LET (a, b, c) -> (ok) = (b*b - 4*a*c >= 0) * -1 END ;

: quad-roots   ( a b c -- r1 r2 )   \ both real roots
    LET (a, b, c) -> (r1, r2) =
        (-b + d) / (2*a),
        (-b - d) / (2*a)
        WHERE d = sqrt(b*b - 4*a*c)
    END ;
```

`d` is computed once via the WHERE binding, then reused for both roots.
Without WHERE you'd write `sqrt(b*b - 4*a*c)` twice and waste a call.

### 4. Mandelbrot iteration

```forth
: mbrot-step
    LET (z_re, z_im, c_re, c_im) -> (z_next_re, z_next_im, mag2) =
        re, im, rmag
        WHERE re   = z_re*z_re - z_im*z_im + c_re
        WHERE im   = 2 * z_re * z_im + c_im
        WHERE rmag = re*re + im*im
    END ;
```

The escape-radius squared comes out for free as the third return value,
so an outer loop can do `mbrot-step over 4.0 f< IF leave THEN`
without a separate distance calculation.

### 5. Clamp and smoothstep

```forth
: clamp     ( x lo hi -- y )
    LET (x, lo, hi) -> (y) =
        select(x < lo, lo, select(x > hi, hi, x))
    END ;

: smooth    ( t -- y )      \ smoothstep(0, 1, t)
    LET (t) -> (y) =
        u * u * (3 - 2 * u)
        WHERE u = select(t < 0, 0, select(t > 1, 1, t))
    END ;
```

Both branchless. Both fit on a screen.

---

## CODE: in 90 seconds

```forth
CODE: add3      add rax, 3 ;CODE
40 add3 .    \ → 43
```

`CODE:` reads the name (`add3`), then everything up to `;CODE`, wraps
it in the kernel's standard `proc(name) … endp()` macros, hands it to
JASM, and JIT-compiles it. The new word's xt is a 12-byte trampoline
in the dictionary that jumps into the compiled function.

**Multi-line bodies and the REPL**: a multi-line CODE: body works fine
when WF64 reads from a file or from piped stdin. In an **interactive
terminal session** the body must be on one line — the live-input
path can't peek ahead to find `;CODE`. Save multi-line work to a `.f`
file and `include` it, or compress to one line. The fix is in
`peek_until_code_terminator`'s Live branch if it bites you.

You write **real** kernel assembly — same registers, same macros, same
conventions as `kernel/*.masm`:

- `rax` is TOS. Reading it gives you the user's top-of-stack value.
- `rbp` is DSP. `[rbp]` is NOS, `[rbp + cell]` is NNOS, and so on.
  `cell = 8`.
- `rbx` is UP. The user area is at offsets off rbx.
- `rsp` is the return stack. **You must preserve return addresses if
  you touch it.** See the `>r` pattern below.
- `r12-r15`, `rdi`, `rsi`, `rbp`, `rbx`, `xmm6-xmm15` are all
  callee-saved per Win64. If you write to them you must restore on
  exit. The macros do this for you when you use them.

A trailing `next()` (= `ret`) is auto-appended, so you don't have to
write the `ret` yourself. If your code already ends with an explicit
`ret` or branch, the auto-appended `ret` is unreachable and harmless.

### What you can use inside CODE:

The full kernel macro vocabulary is preloaded into the JASM assembler:

| Macro              | What it does                                       |
|--------------------|----------------------------------------------------|
| `stk(in, out)`     | Adjust DSP for net stack effect. Emits add/sub.    |
| `pushd(value)`     | Push a register/imm: spill TOS to NOS, set new TOS |
| `popd()`           | Drop TOS, raise NOS into RAX                       |
| `next()`           | `ret` — but auto-appended at end                   |
| `win64_call(fn)`   | The shadow-space + alignment dance for Win64 calls |
| `brk()`            | `int 3` — non-fatal breakpoint, dumps state        |

Plus the `@assign` symbols: `cell`, `user_FSP`, `user_HERE`,
`user_LATEST`, every primitive name…

Comments inside CODE: are assembly style — `; rest of line`. Forth-
style `( … )` doesn't work here; it's not Forth source.

---

## CODE: — five practical problems

### 1. POPCNT — count set bits

```forth
CODE: popcount  ( n -- count )
    popcnt rax, rax
;CODE

255 popcount .    \ → 8
```

x86 has a hardware POPCNT instruction; no Forth equivalent without
this trick.

### 2. RDTSC — cycle counter

```forth
CODE: rdtsc-low  ( -- cycles )      \ low 32 bits only
    pushd(0)
    rdtsc
    mov eax, eax        ; zero-extend
;CODE
```

`rdtsc` returns 64 bits split across edx:eax. The kernel's own
`rdtsc` word merges them; this stripped-down version is fine for
timing tight loops.

### 3. Bit reverse — 8-bit lookup, expressed as asm

```forth
CODE: rev8  ( x -- reversed )      \ reverse low 8 bits
    mov rcx, rax
    xor rax, rax
    mov rdx, 8
.loop:
    shr rcx, 1
    rcl rax, 1
    sub rdx, 1
    jnz .loop
;CODE

$AA rev8 .x        \ → 55  (0b10101010 → 0b01010101)
```

Local labels (`.loop`) are scoped per CODE: definition, so they don't
collide between definitions.

### 4. Saturating add — pure SSE, no branches

```forth
CODE: sat-add+   ( a b -- min(a+b, 2^63-1) )
    add rax, [rbp]
    mov rcx, $7FFFFFFFFFFFFFFF
    cmovo rax, rcx       ; if signed overflow, clamp
    stk(2, 1)
;CODE
```

`cmovo` is "conditional move on overflow." On overflow we clamp to
`INT64_MAX`; otherwise the sum stays. Branchless saturation.

### 5. Memory prefetch hint

```forth
CODE: prefetch  ( addr -- )
    prefetcht0 byte ptr [rax]
    stk(1, 0)
    mov rax, [rbp]
;CODE
```

Tells the CPU "I'll be reading from this address soon — start pulling
it into L1." For chasing pointers in hot loops.

---

## How CODE: and LET fit together

They share infrastructure (delimited source capture, fresh-JIT, splice
bytes at HERE) but specialise differently:

|                          | `LET`                            | `CODE:`                  |
|--------------------------|----------------------------------|--------------------------|
| Source language          | infix algebra                    | x86-64 MASM              |
| Parser                   | custom Rust (~400 lines)         | JASM (reused)            |
| Codegen                  | SSE register allocator           | direct MC encoding       |
| Use case                 | dense float math                 | exactly the bytes you mean |
| Cost per call site       | ~36-byte trampoline + the body   | 12-byte JMP trampoline   |
| When you'd reach for it  | "this expression has 5+ ops"     | "I need POPCNT"          |

A typical app uses both. Hot inner numerics in LET, syscalls or weird
intrinsics in CODE:, glue and control flow in stack Forth.

---

## Errors and how to read them

Both DSLs run at compile time and throw Forth errors on failure.

### LET errors

LET prints its parse/compile message to stderr and THROWs a -2056. The
message tells you the byte offset into the LET form's source:

```
LET compile error: undefined name 'foo' at byte 17
```

Typical causes: typo in a name; a WHERE binding referencing itself or
a cycle; using `=` where you meant `==`; using `pow` (or any libm) by
accident with a typo'd name.

### CODE: errors

CODE: throws -2057. There are two paths:

- **Forth-side errors** (no `;CODE` found before EOF) print a clear
  message and throw cleanly.
- **MC parse errors** (your asm is invalid) also throw, but the MC
  error message has the lexical detail — usually a `<inline asm>:N:M:
  unexpected token` style message on the line above the THROW. That
  comes from LLVM-MC's diagnostic handler — see the "graceful MC
  errors" commit if you're curious.

### Live REPL caveat

Multi-line CODE: bodies work in piped stdin (`cat foo.f | wf64`) but
in an interactive terminal session the body must be on one line. The
fix is somewhere in `peek_until_code_terminator`'s Live branch if you
hit this — it's a `BufRead::read_line` loop and could be smarter.

---

## Inspecting what came out

To see the bytes a `CODE:` or `LET` actually emitted:

```forth
: foo  CODE: imul rax, rax ;CODE ;
' foo dup .    \ xt
30 dump        \ first 30 bytes — you'll see the trampoline + body
```

(Requires `dump` from core.f, which loads automatically in the
default REPL.)

To see the asm a LET form generated *before* it's encoded, run a quick
Rust test that calls `wf64::let_lang::compile` and prints the
`asm_text` field. There's an example in
`src/let_lang/mod.rs`'s `run_let` test helper.

---

## A few honest limits

LET:
- No function calls between LETs yet — each LET is its own JIT'd
  function and doesn't know about the others'. If you want shared
  math, write it inline as a WHERE binding.
- No auto-differentiation. The AST would support it; nobody's built
  the gradient pass.

CODE:
- LLVM-MC errors are caught but not super-readable. If you'd rather
  see a clean parse error, use a kernel `.masm` file and rebuild.
- The 12-byte trampoline means each call goes `CALL trampoline; JMP
  fn` rather than `CALL fn` directly. ~3-5 cycles per call site.
  Usually invisible; if you're calling it 10 million times per second
  in a hot loop, consider porting to the kernel proper.

---

## Where to go next

- `tests/harness.rs` — all the `let_dsl_*` and `code_dsl_*` tests are
  short, executable examples worth grepping when you can't remember a
  syntax detail.
- `src/let_lang/codegen.rs` — read top to bottom in one sitting if
  you want to know exactly what LET emits and why.
- `kernel/macros.masm` — the macro vocabulary CODE: inherits.
