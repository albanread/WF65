# arshift  ( n count -- n' )   arithmetic (signed) right shift

push -16
push 2
call arshift
expect -4

reset
push 16
push 2
call arshift
expect 4
