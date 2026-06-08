# count-bits  ( n -- bits )   population count (number of 1 bits)

push 0
call count_bits
expect 0

reset
push -1
call count_bits
expect 64                # all 64 bits set

reset
push 0xFF
call count_bits
expect 8

reset
push 0xAAAAAAAA
call count_bits
expect 16                # 0xA = 1010 → 2 bits per nibble, 8 nibbles
