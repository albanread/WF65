# msbit  ( n -- msb )   index of highest set bit; -1 if n==0

push 0
call msbit
expect -1                # zero → -1 (no bit set)

reset
push 1
call msbit
expect 0                 # bit 0

reset
push 0x80
call msbit
expect 7

reset
push -1
call msbit
expect 63                # all bits set → top is bit 63
