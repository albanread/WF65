# skip  ( addr len char -- addr' len' )    skip leading bytes matching char

# Skip leading spaces: "   Hello" with len=8, char=0x20
poke 0x100 20202048656c6c6f
push_pad 0x100
push 8
push 0x20
call skip
# Result: addr' = addr+3, len' = 5
call nip_
expect 5

# Nothing to skip
reset
poke 0x100 48656c6c6f
push_pad 0x100
push 5
push 0x20
call skip
call nip_
expect 5

# All bytes match → empty result
reset
poke 0x100 2020202020
push_pad 0x100
push 5
push 0x20
call skip
call nip_
expect 0

# Empty input
reset
push_pad 0x100
push 0
push 0x20
call skip
call nip_
expect 0
