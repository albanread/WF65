\ bench/corpus/tco-tail.f
\ Transform 4 — tail-call optimization. A word's FINAL 0xE8 CALL to another word
\ is rewritten in place to a 0xE9 JMP; byte_length is unchanged, so the gate is
\ the disassembler's tail_is_jmp predicate, not the noisy raw jmp_count. None of
\ these words end in >r/r>, so none hit the compile_comma_no_tco exclusion
\ (compile.masm:124). Pure compute, bounded, terminating, one result on the stack.
\
\ Verdict words: relay (tail delegation), countdown (tail self-recursion).

\ bump ( n -- n+1 ) : does genuine work so payload is not a no-op body.
: bump ( n -- n+1 ) 1+ ;

\ payload ( n -- (n+1)^2 ) : real work then a tail-delegation to `square`
\ (core.f:190) — its last compiled action is one user-word CALL -> JMP.
: payload ( n -- r ) bump square ;

\ relay ( n -- r ) : pure tail delegation — the headline TCO case. Its only
\ action is a single user-word CALL to payload, which must become a JMP.
: relay ( n -- r ) payload ;

\ relay2 ( n -- r ) : one more level, to confirm chained tail calls all rewrite.
: relay2 ( n -- r ) relay ;

\ countdown ( n -- 0 ) : self tail-recursion. The trailing `countdown` is the TCO
\ target; once it becomes a JMP, tail_is_jmp must stay true.
: countdown ( n -- 0 )
  dup 0= if exit then
  1-
  countdown ;

\ load-time self-check: exercise once, leave the stack balanced.
5 relay drop
50 countdown drop
