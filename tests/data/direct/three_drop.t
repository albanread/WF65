# 3drop  ( x1 x2 x3 -- )

push 1
push 2
push 3
call three_drop
expect

reset
push 100
push 1
push 2
push 3
call three_drop
expect 100
