# scan  ( addr len char -- addr' len' )    advance past leading non-char bytes,
#                                            return (addr-of-char, remaining-len).

# Find the space in "ab cd"
poke 0x100 6162206364
push_pad 0x100
push 5
push 0x20
call scan
# Result: addr' = addr+2 (= location of ' '), len' = 3
call nip_
expect 3

# Char not present → addr += len, len' = 0
reset
poke 0x100 6162636465
push_pad 0x100
push 5
push 0x20
call scan
call nip_
expect 0

# Char at index 0
reset
poke 0x100 20414243
push_pad 0x100
push 4
push 0x20
call scan
call nip_
expect 4

# Empty input
reset
push_pad 0x100
push 0
push 0x20
call scan
call nip_
expect 0
