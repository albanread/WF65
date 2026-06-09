\ bench/corpus/hotvar.f
\ Opt-in variable inlining. A regular `variable` reference compiles to a CALL
\ into the create stub (pushes the body address); a `hotvariable` reference
\ compiles to an inline body-address push (no CALL). The two words below are
\ identical except for the variable's flavour, so the per-word call_count delta
\ is exactly the variable-access inlining: cold-sum keeps its CALLs, hot-sum
\ drops them. Headless, pure compute, one result on the stack.
\
\ Verdict words: cold-sum (regular variable), hot-sum (hotvariable).

variable     cv          \ cold: references compile to a CALL
hotvariable  hv          \ hot:  references compile to an inline address push

: cold-sum ( n -- s )
  0 cv !
  0 ?do  cv @  i +  cv !  loop
  cv @ ;

: hot-sum ( n -- s )
  0 hv !
  0 ?do  hv @  i +  hv !  loop
  hv @ ;

\ load-time self-check: both compute 0+1+...+(n-1); leave the stack balanced.
10 cold-sum drop
10 hot-sum drop
