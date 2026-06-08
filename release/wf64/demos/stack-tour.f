\ stack-tour.f — a quick tour of the data stack
\
\ Pushes a few values, prints the stack, exercises the
\ standard stack ops, then leaves a clean stack behind.
\
\ Convention for IDE demos: the entry word matches the file
\ stem (here: stack-tour).  The Demos menu loads the file
\ then invokes that word automatically.

: stack-tour
    cr ." === stack tour ===" cr

    \ Push some numbers.
    1 2 3 4 5
    ." after 1 2 3 4 5  -> .s :" cr
    .s cr

    \ swap top two: ... 4 5  becomes  ... 5 4
    swap
    ." after swap        -> .s :" cr
    .s cr

    \ rot rotates the third deepest cell to the top:
    \ ... 3 5 4  becomes  ... 5 4 3
    rot
    ." after rot         -> .s :" cr
    .s cr

    \ Drop the rotated cell, sum the next two with +.
    drop
    +
    ." after drop +      -> .s :" cr
    .s cr

    \ Two arithmetic results on top: 1 2 and 7.
    ." sum of top two = " + . cr

    \ Tidy up so the cushion is the only thing left.
    .s cr
    ." === end of tour ===" cr
;
