# 2over  ( x1 x2 x3 x4 -- x1 x2 x3 x4 x1 x2 )    copy deeper pair to top

push 1
push 2
push 3
push 4
call two_over
expect 1 2 3 4 1 2
