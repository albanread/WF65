# 2swap  ( x1 x2 x3 x4 -- x3 x4 x1 x2 )    swap pairs

push 1
push 2
push 3
push 4
call two_swap
expect 3 4 1 2
