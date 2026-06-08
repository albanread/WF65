# d2/  ( d -- d' )   arithmetic shift double right by one

push 10
push 0
call d_two_slash
expect 5 0

reset
push -10
push -1
call d_two_slash
expect -5 -1