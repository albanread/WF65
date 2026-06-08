# d2*  ( d -- d' )   shift double left by one

push 5
push 0
call d_two_times
expect 10 0

reset
push -1
push 0
call d_two_times
expect -2 1