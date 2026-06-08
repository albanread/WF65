# u<=  ( u1 u2 -- flag )   unsigned less-than-or-equal

push 5
push 5
call u_less_equal
expect -1

reset
push 1
push -1
call u_less_equal
expect -1

reset
push -1
push 1
call u_less_equal
expect 0