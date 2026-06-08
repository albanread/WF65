# -  ( n1 n2 -- n1-n2 )    signed subtract

push 10
push 3
call minus
expect 7

reset
push 3
push 10
call minus
expect -7
