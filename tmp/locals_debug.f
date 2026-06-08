\ Test 1: does {: work at all?
: t1 ( n -- ) {: a :} a . ;
5 t1

\ Test 2: does to work with a value?
variable tvar
: t2 ( n -- ) dup tvar ! tvar @ . ;
7 t2

bye
