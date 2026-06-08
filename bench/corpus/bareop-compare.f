\ bench/corpus/bareop-compare.f
\ Comparison / logic / shift bare-op inlining. With non-constant operands none
\ of these can fold, so the only win is inlining the CALL (via inline_leaf_comp,
\ which copies the leaf primitive body). A regression re-introduces the CALL,
\ visible in call_count_E8. Headless, pure compute, one result on the stack.
\
\ Verdict words: cmp-chain (= <> < >), logic-chain (and or xor), zero-chain (0= 0<).

\ cmp-chain ( a b -- r ) : several bare comparisons combined with bare or.
: cmp-chain ( a b -- r )
  2dup <   >r            \ f1 = a<b
  2dup >   >r            \ f2 = a>b
  2dup <>                \ f3 = a<>b
  r> or  r> or           \ f3 | f2 | f1
  >r 2drop r> ;

\ logic-chain ( a b -- r ) : bare and / or / xor on non-constant operands.
: logic-chain ( a b -- r )
  2dup and  >r           \ a & b
  2dup or   >r           \ a | b
  2dup xor               \ a ^ b
  r> xor  r> xor          \ combine
  >r 2drop r> ;

\ shift-chain ( a b -- r ) : bare lshift / rshift / arshift (b kept small).
: shift-chain ( a b -- r )
  7 and                  \ keep the shift count in 0..7 (non-constant operand)
  2dup lshift  >r
  2dup rshift  >r
  2dup arshift
  r> xor  r> xor
  >r 2drop r> ;

\ zero-chain ( n -- r ) : bare 0= / 0< combined.
: zero-chain ( n -- r )
  dup 0=  >r
  dup 0<  r> or
  >r drop r> ;

\ load-time self-check: exercise once, leave the stack balanced.
3 5 cmp-chain drop
12 10 logic-chain drop
255 3 shift-chain drop
-7 zero-chain drop
