# 2dup  ( x1 x2 -- x1 x2 x1 x2 )    duplicate top pair

push 10
push 20
call two_dup
expect 10 20 10 20
