# dabs  ( d -- |d| )   absolute value of signed double

push -10
push -1
call dabs
expect 10 0

reset
push 10
push 0
call dabs
expect 10 0