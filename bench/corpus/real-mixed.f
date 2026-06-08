\ bench/corpus/real-mixed.f
\ Transform 6 — real-world cross-interaction across three structurally different
\ kernels so a win must generalize, not special-case one shape:
\   fact     : constant-compare fold (1 >) + bare * + recursion (NOT tail: the
\              trailing * keeps recurse out of tail position, so no JMP expected).
\   dot-prod : bare @ inline + cells-offset arith + scheduling over the loop body.
\   str-hash : literal-fold (33 *) + bare c@/+ + shuffle across the span.
\ Distinct helper/constant names so the three coexist in one file. Pure compute,
\ bounded, each verdict word leaves a single result on the stack.
\
\ Verdict words: fact, dot-prod, str-hash.

\ --- fact -----------------------------------------------------------------
: fact ( n -- n! )
  dup 1 >
  if dup 1- recurse * then ;

\ --- dot-prod -------------------------------------------------------------
256 constant dp-N
dp-N cells buffer: dp-A
dp-N cells buffer: dp-B

: dp-setup ( -- )                       \ A[i]=i+1 , B[i]=(i+1)*2
  dp-N 0 ?do
    i 1+        dp-A i cells + !
    i 1+ 2 *    dp-B i cells + !
  loop ;

: dot-prod ( -- sum )
  0
  dp-N 0 ?do
    dp-A i cells + @
    dp-B i cells + @
    *  +
  loop ;

\ --- str-hash (djb2-style integer hash) -----------------------------------
256 constant sh-LEN
sh-LEN buffer: sh-S

: sh-fill ( -- )                        \ deterministic bytes 0..255 (c! masks low 8 bits)
  sh-LEN 0 ?do  i  sh-S i +  c!  loop ;

: str-hash ( -- h )
  5381
  sh-LEN 0 ?do
    33 *                                \ literal-fold (IMUL imm)
    sh-S i + c@                         \ bare + then byte fetch
    +                                   \ bare + accumulates
  loop ;

\ fill the buffers once at load (no I/O, stack balanced)
dp-setup
sh-fill

\ load-time self-check: exercise once, leave the stack balanced.
5 fact drop
dot-prod drop
str-hash drop
