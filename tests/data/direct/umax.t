# umax  ( u1 u2 -- u3 )   unsigned maximum

push 3
push 5
call umax
expect 5

reset
push -1
push 1
call umax
expect -1