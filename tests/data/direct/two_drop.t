# 2drop  ( x1 x2 -- )    drop two cells

push 1
push 2
call two_drop
expect

reset
push 10
push 20
push 30
call two_drop
expect 10
