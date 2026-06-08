# xor  ( n1 n2 -- n3 )   bitwise XOR

push 0xF0
push 0x0F
call xor_
expect 0xFF

reset
push 0xFFFF
push 0xFFFF
call xor_
expect 0
