# 3dup  ( x1 x2 x3 -- x1 x2 x3 x1 x2 x3 )

push 10
push 20
push 30
call three_dup
expect 10 20 30 10 20 30
