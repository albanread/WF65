# 5*  ( n -- 5n )

push 6
call five_times
expect 30

reset
push -3
call five_times
expect -15