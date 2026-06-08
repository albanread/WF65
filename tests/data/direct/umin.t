# umin  ( u1 u2 -- u3 )   unsigned minimum

push 3
push 5
call umin
expect 3

reset
push -1
push 1
call umin
expect 1