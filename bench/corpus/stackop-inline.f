\ bench/corpus/stackop-inline.f
\ Transform 5 — stack-op inlining (inline_dup_comp/inline_drop_comp/
\ inline_swap_comp, compile.masm:753/791). A regression makes dup/drop/swap/over
\ compile to a 5B CALL instead of inline (~8B dup/drop, ~11B swap). Gate on
\ call_count_E8, NOT byte_length: reverting an inline to a CALL can DROP bytes
\ while ADDING a call. No arithmetic on constants, so do_lit_count stays 0 and
\ this file isolates the stack-op-inline transform.
\
\ Verdict words: mix-stackops (dup/over/swap/drop mix), swaps (swap-dominated).
\ Sanity-only (non-verdict): dups.

\ mix-round ( x y -- x y ) : net-zero interleave exercising over/drop/swap/dup.
: mix-round ( x y -- x y )
  over drop swap dup drop swap ;

\ mix-stackops ( a b -- r ) : churn the two seed cells through mix-round; leave
\ one cell. The loop body is the inlined dup/over/swap/drop span.
: mix-stackops ( a b -- r )
  64 0 ?do
    mix-round
  loop
  nip ;

\ swap-step ( a b -- a' b' ) : four bare swaps plus a small perturbation so
\ swap-swap cannot cancel to nothing.
: swap-step ( a b -- a' b' )
  swap swap
  over +
  swap
  1+
  swap ;

\ clamp ( n -- n' ) : keep magnitudes bounded across the run.
: clamp ( n -- n' ) 4095 and ;

\ swaps ( a b -- r ) : swap-dominated hot word.
: swaps ( a b -- r )
  100 0 do
    swap-step
    swap clamp swap clamp
  loop
  + ;

\ dups ( n -- r ) : sanity-only dup stressor; do not gate the swap transform here.
: dup-step ( n -- n' ) dup dup dup + + + 4095 and ;
: dups ( n -- r ) 100 0 do dup-step loop ;

\ load-time self-check: exercise once, leave the stack balanced.
10 20 mix-stackops drop
10 20 swaps drop
5 dups drop
