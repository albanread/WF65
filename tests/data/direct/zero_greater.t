# 0>  ( n -- flag )   true (-1) if n > 0

push 1
call zero_greater
expect -1

reset
push 0
call zero_greater
expect 0

reset
push -1
call zero_greater
expect 0
