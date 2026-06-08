# um*  ( u1 u2 -- ud )    unsigned 64×64 → 128-bit double
#                          Result: ( low high ) with high on top.

# No overflow: 100 × 200 = 20000
push 100
push 200
call um_times
expect 20000 0           # low=20000, high=0

# Full 64-bit overflow: 0xFFFF…FFFF × 2 = 0x1FFFF…FFFE
# low half = 0xFFFFFFFFFFFFFFFE (= -2 as i64), high half = 1
reset
push -1
push 2
call um_times
expect -2 1
