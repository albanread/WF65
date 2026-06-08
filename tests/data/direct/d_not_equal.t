# d<>  ( d1 d2 -- flag )   true when either half differs

push 10
push 0
push 11
push 0
call d_not_equal
expect -1

reset
push 10
push 0
push 10
push 0
call d_not_equal
expect 0