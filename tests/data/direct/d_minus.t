# d-  ( d1 d2 -- d3 )   signed double subtract

push 30
push 0
push 20
push 0
call d_minus
expect 10 0

reset
push 0
push 1
push 1
push 0
call d_minus
expect -1 0