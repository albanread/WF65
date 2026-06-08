# 2rot  ( x1 x2 x3 x4 x5 x6 -- x3 x4 x5 x6 x1 x2 )    rotate three pairs

push 1
push 2
push 3
push 4
push 5
push 6
call two_rot
expect 3 4 5 6 1 2
