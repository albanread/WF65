# d<  ( d1 d2 -- flag )   signed double less-than

push 10
push 0
push 20
push 0
call d_less
expect -1

reset
push -10
push -1
push 5
push 0
call d_less
expect -1

reset
push 20
push 0
push 10
push 0
call d_less
expect 0