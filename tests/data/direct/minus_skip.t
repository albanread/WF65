# -skip  ( addr len char -- addr len' )    remove TRAILING bytes matching char

# Trailing spaces: "Hello   " with len=8, char=0x20 → len' = 5
poke 0x100 48656c6c6f202020
push_pad 0x100
push 8
push 0x20
call minus_skip
call nip_
expect 5

# No trailing match
reset
poke 0x100 48656c6c6f
push_pad 0x100
push 5
push 0x20
call minus_skip
call nip_
expect 5

# All match
reset
poke 0x100 20202020
push_pad 0x100
push 4
push 0x20
call minus_skip
call nip_
expect 0
