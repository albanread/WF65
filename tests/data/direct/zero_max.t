# 0max  ( n -- max(n, 0) )

push 7
call zero_max
expect 7

reset
push -12
call zero_max
expect 0