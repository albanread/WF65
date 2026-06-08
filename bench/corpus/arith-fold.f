\ bench/corpus/arith-fold.f
\ Transform 1 — literal folding (fold_plus_comp/fold_minus_comp/fold_times_comp,
\ compile.masm:185/213/242). Every binop here has a numeric/constant operand, so
\ each "<lit> +", "<lit> *", "<lit> -" is a fold candidate. A missed fold reverts
\ to a 13B do_lit literal + 5B operator CALL — visible in byte_length, call_count
\ and do_lit_count. Operands are DISTINCT primes so no constant-run special-case
\ can carry the metric. Headless, pure compute, result left on the stack.
\
\ Verdict words: fold-mix, addk.  Sanity-only (non-verdict): fold-poly, fold-chain.

100 constant k

\ fold-mix ( n -- r ) : long chain of immediate-foldable binops with the input
\ kept data-dependent (so r is not a compile-time constant). dup/swap hold a
\ second live value across part of the span.
: fold-mix ( n -- r )
  3 +  7 *  11 -  13 +  17 *  19 -  23 +  29 *  31 -  37 +
  dup  41 *  swap  43 +  47 *  -
  53 +  59 *  61 -  67 +  71 *  73 -  79 + ;

\ addk ( n -- n+k ) : single CONSTANT-operand add. A constant compiles via
\ `create , does> @` (core.f:78), so `k +` should fold to an immediate ADD.
: addk ( n -- n+k ) k + ;

\ --- sanity stressors (broaden coverage; not used for the verdict) ---
2 constant a   3 constant b   7 constant c
: fold-poly ( n -- m ) a + b * c - ;
: fold-chain ( n -- n+8 ) 1+ 1+ 1+ 1+ 1+ 1+ 1+ 1+ ;

\ load-time self-check: exercise once, leave the stack balanced.
10 fold-mix drop
10 addk drop
