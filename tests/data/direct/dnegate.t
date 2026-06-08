# dnegate  ( d -- d' )   two's-complement negate

push 10
push 0
call dnegate
expect -10 -1

reset
push -10
push -1
call dnegate
expect 10 0