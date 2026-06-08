\ bench/corpus/stack-shuffle.f
\ Transform 3 — rbp-materialization elimination / stack scheduling (headline v2):
\ coalesce rbp data-stack-pointer adjusts and keep TOS in RAX across a span
\ instead of spilling RAX->[rbp] between every op. Each `dup +` is a spill/reload
\ pair; a dense run emits a long MOV[rbp]/rbp-delta ladder the scheduler must
\ collapse. No constant operands, so the folder cannot pre-compute anything.
\ Fingerprint: call_count stays 0 (these are inlined ops); instruction_count and
\ rbp_adjust_count are high today and should drop after v2 (advisory until then).
\
\ Verdict words: long-arith (dense spill ladder), shuffle-a (shuffle + arith span).

\ long-arith ( n -- m ) : ten consecutive `dup +` doublings — one unbroken spill
\ ladder. m = n * 1024.
: long-arith ( n -- m )
  dup +  dup +  dup +  dup +  dup +
  dup +  dup +  dup +  dup +  dup + ;

\ shuffle-a ( a b c -- x ) : interleaves -rot/over/rot/swap/nip with bare + so the
\ scheduler must track TOS across pure-shuffle ops (no arithmetic to anchor RAX)
\ as well as across arithmetic. Consumes 3 cells, leaves 1.
: shuffle-a ( a b c -- x )
  -rot         ( c a b )
  over         ( c a b a )
  +            ( c a a+b )
  rot          ( a a+b c )
  +            ( a a+b+c )
  swap         ( a+b+c a )
  over         ( a+b+c a a+b+c )
  +            ( a+b+c a+a+b+c )
  nip          ( 2a+b+c ) ;

\ load-time self-check: exercise once, leave the stack balanced.
7 long-arith drop
10 20 30 shuffle-a drop
