# 1-  ( n -- n-1 )

push 43
call one_minus
expect 42

reset
push 0
call one_minus
expect -1
