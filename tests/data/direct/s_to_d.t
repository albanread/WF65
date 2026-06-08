# s>d  ( n -- d )   sign-extend single to double

push 10
call s_to_d
expect 10 0

reset
push -10
call s_to_d
expect -10 -1