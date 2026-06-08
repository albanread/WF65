# u<  ( u1 u2 -- flag )   unsigned less-than
#
# -1 as u64 is 0xFFFF…FFFF — the largest unsigned value. So
# (-1) u< (1) is FALSE because -1 unsigned > 1.

push 3
push 5
call u_less
expect -1

reset
push -1
push 1
call u_less
expect 0                 # unsigned -1 is BIG, not less than 1
