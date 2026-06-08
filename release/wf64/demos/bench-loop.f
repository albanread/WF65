\ bench-loop.f — tiny tight-loop benchmark
\
\ Counts from 0 to N-1, summing the loop index.  Prints the
\ sum and the depth before/after to show the loop leaves the
\ stack balanced.
\
\ N defaults to one million.  Tweak the literal in `bench-loop`
\ if you want a different size.

: sum-to ( n -- sum )
    \ Sum 0..n-1.  Iterative version using the return stack via
    \ ?do/loop's built-in counter (i).
    0 swap 0 ?do
        i +
    loop
;

: bench-loop
    cr ." === bench loop ===" cr
    ." depth before = " depth . cr

    \ Sum the first million integers.
    1000000 sum-to
    ." sum 0..999999 = " . cr

    ." depth after  = " depth . cr
    ." === done ===" cr
;
