# d+  ( d1 d2 -- d3 )   signed double add

push 10
push 0
push 20
push 0
call d_plus
expect 30 0

reset
push -1
push 0
push 1
push 0
call d_plus
expect 0 1