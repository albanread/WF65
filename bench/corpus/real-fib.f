\ bench/corpus/real-fib.f
\ Transform 6 — real-world cross-interaction. Iterative Fibonacci over a counted
\ ?do/loop combines: T1 (constant-operand loop control), T2 (bare + on the two
\ running values), T5 (over/swap inline), and loop machinery T3 must not perturb.
\ A regression in any one transform moves at least one metric and the per-word
\ delta localizes which broke. Pure compute, bounded, single result on the stack.
\
\ Verdict word: fib-iter.

\ fib-iter ( n -- fib ) : advance a (a,b) pair n times.
: fib-iter ( n -- fib )
  0 1 rot              ( 0 1 n )    \ seed a=0 b=1, n is the loop limit
  0 ?do
    over + swap        ( a b -- a+b a )
  loop
  drop ;               ( a b -- a )

\ load-time self-check: exercise once, leave the stack balanced.
30 fib-iter drop
