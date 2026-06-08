# u>  ( u1 u2 -- flag )   unsigned greater-than

push -1
push 1
call u_greater
expect -1                # unsigned -1 is the largest

reset
push 1
push -1
call u_greater
expect 0
