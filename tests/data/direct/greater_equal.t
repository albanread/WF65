# >=  ( n1 n2 -- flag )

push 5
push 3
call greater_equal
expect -1

reset
push 5
push 5
call greater_equal
expect -1

reset
push 3
push 5
call greater_equal
expect 0
