# */  ( n1 n2 n3 -- (n1*n2)/n3 )
#     Multiplies n1*n2 with double-precision intermediate, then
#     divides by n3 (single). Avoids overflow even when n1*n2
#     would overflow a single cell.

# Trivial: 6 * 7 / 2 = 21
push 6
push 7
push 2
call times_slash
expect 21

# Overflow protection: 0x40000000 * 0x40000000 / 0x40000000 = 0x40000000
reset
push 0x40000000
push 0x40000000
push 0x40000000
call times_slash
expect 0x40000000
