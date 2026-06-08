\ bench/corpus/bareop-inline.f
\ Transform 2 — bare-op inline (pending). Bare + - * @ ! with NON-constant
\ operands compile to a 5B CALL (0xE8) today; the optimizer inlines them. No
\ literal sits in front of any op, so no fold can fire (do_lit_count stays 0) and
\ this file isolates the inline win. Gate on call_count_E8, not byte_length:
\ an inline body can be larger than the 5B CALL it replaces.
\
\ Verdict words: bare-chain (arithmetic), bare-mem (memory).
\ Sanity-only (non-verdict): bare-add.

\ bare-chain ( a b c d -- r ) : a chain of bare + - * on stack-supplied operands.
\ None are constants, so every one is an inline candidate, never a fold.
: bare-chain ( a b c d -- r )
  +            ( a b c+d )
  rot          ( b c+d a )
  -            ( b s )           \ s = (c+d) - a   bare -
  *            ( u )             \ u = b * s        bare *
  dup +        ( 2u ) ;          \ bare + on non-constant operands

variable slot          \ the cell we read-modify-write
variable pa            \ holds slot's address, fetched at runtime (defeats folding)
variable acc           \ running accumulator, touched via bare +!

\ bare-mem ( n -- r ) : n read-modify-write cycles against `slot` through the
\ runtime address in `pa`, accumulating into `acc` via bare +!. Every @ / ! takes
\ its address off the stack at runtime, so only inlining (not folding) can win.
: bare-mem ( n -- r )
  0 slot !
  0 acc !
  slot pa !
  0 ?do
    pa @            ( addr )
    dup @           ( addr v )
    1+              ( addr v+1 )
    swap !          ( )
    pa @ @          ( v+1 )
    acc +!          ( )
  loop
  acc @ ;

\ bare-add ( a b -- s ) : degenerate sanity stressor, non-verdict.
: bare-add ( a b -- s ) + ;

\ load-time self-check: exercise once, leave the stack balanced.
10 20 30 40 bare-chain drop
8 bare-mem drop
2 3 bare-add drop
