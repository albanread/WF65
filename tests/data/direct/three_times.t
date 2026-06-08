# 3*  ( n -- 3n )

push 7
call three_times
expect 21

reset
push -4
call three_times
expect -12