# 0<>  ( n -- flag )   true (-1) if n != 0, else false (0)

push 0
call zero_not_equal
expect 0

reset
push 42
call zero_not_equal
expect -1

reset
push -1
call zero_not_equal
expect -1
