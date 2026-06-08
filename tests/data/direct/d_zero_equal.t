# d0=  ( d -- flag )   true when both halves are zero

push 0
push 0
call d_zero_equal
expect -1

reset
push 0
push 1
call d_zero_equal
expect 0