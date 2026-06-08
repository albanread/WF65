# d0<  ( d -- flag )   true when signed double is negative

push -10
push -1
call d_zero_less
expect -1

reset
push 10
push 0
call d_zero_less
expect 0