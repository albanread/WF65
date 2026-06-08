# 4dup  ( x1 x2 x3 x4 -- x1 x2 x3 x4 x1 x2 x3 x4 )

push 10
push 20
push 30
push 40
call four_dup
expect 10 20 30 40 10 20 30 40
