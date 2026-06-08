\ factorial.f — recursive factorial
\
\ Two styles:
\   factorial-iter ( n -- n! )   loop based, no recursion
\   factorial-rec  ( n -- n! )   recursive (uses Forth's
\                                recurse to refer to itself)

: factorial-iter ( n -- n! )
    1 swap 1+ 1 ?do
        i *
    loop
;

: factorial-rec ( n -- n! )
    dup 1 <= if
        drop 1
    else
        dup 1- recurse *
    then
;

: factorial
    cr ." === factorial ===" cr
    ." n     n! (iter)        n! (rec)" cr
    13 0 ?do
        i 5 .r space
        i factorial-iter 14 .r space space
        i factorial-rec  14 .r cr
    loop
    ." === done ===" cr
;
