# 2*  ( n -- 2n )    arithmetic shift left (sign-preserving via signed mult)

push 21
call two_times
expect 42

reset
push -5
call two_times
expect -10
