# d>  ( d1 d2 -- flag )   signed double greater-than

push 20
push 0
push 10
push 0
call d_greater
expect -1

reset
push 5
push 0
push -10
push -1
call d_greater
expect -1

reset
push 10
push 0
push 20
push 0
call d_greater
expect 0

reset
push 10
push 0
push 10
push 0
call d_greater
expect -1