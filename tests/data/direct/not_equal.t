# <>  ( n1 n2 -- flag )

push 42
push 42
call not_equal
expect 0

reset
push 41
push 42
call not_equal
expect -1
