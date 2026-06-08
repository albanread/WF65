\ fibonacci.f — print the first N Fibonacci numbers
\
\ Two definitions:
\   fib  ( n -- f )   tail-recursive iterative version
\   fibonacci         entry point; prints the first 20

: fib ( n -- f )
    \ Iterative: maintain (a b) and decrement n.
    0 1 rot 0 ?do
        over + swap
    loop
    drop
;

: fibonacci
    cr ." === fibonacci ===" cr
    ." first 20 Fibonacci numbers:" cr
    20 0 ?do
        i fib .
    loop
    cr
    ." === done ===" cr
;
