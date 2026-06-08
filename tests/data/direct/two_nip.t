# 2nip  ( x1 x2 x3 x4 -- x3 x4 )    drop the deeper pair

push 1
push 2
push 3
push 4
call two_nip
expect 3 4
