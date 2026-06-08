# 0<  ( n -- flag )   true (-1) if n < 0

push -1
call zero_less
expect -1

reset
push 0
call zero_less
expect 0

reset
push 1
call zero_less
expect 0

reset
push -0x8000000000000000     # i64::MIN
call zero_less
expect -1
