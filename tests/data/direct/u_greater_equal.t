# u>=  ( u1 u2 -- flag )   unsigned greater-than-or-equal

push 5
push 5
call u_greater_equal
expect -1

reset
push -1
push 1
call u_greater_equal
expect -1

reset
push 1
push -1
call u_greater_equal
expect 0