# slastchar  ( addr len -- char )   last character of an s-string

# "Hello" length 5 → 'o' = 0x6f
poke 0x100 48656c6c6f
push_pad 0x100
push 5
call slastchar
expect 0x6f

# Single char
reset
poke 0x110 41
push_pad 0x110
push 1
call slastchar
expect 0x41

# Multi-byte string with non-ASCII byte at end
reset
poke 0x120 4142_ff
push_pad 0x120
push 3
call slastchar
expect 0xff
