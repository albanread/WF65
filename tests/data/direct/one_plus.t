# 1+  ( n -- n+1 )

push 41
call one_plus
expect 42

reset
push -1
call one_plus
expect 0
