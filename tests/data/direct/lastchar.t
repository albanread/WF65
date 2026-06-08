# lastchar  ( c-str -- char )   last character of a COUNTED string
#     A counted string is a length byte followed by chars (Pascal-style).
#     `lastchar` reads str[0] as length, returns str[length].

# Counted "Hello": len=5 byte then "Hello"
poke 0x100 05_48656c6c6f
push_pad 0x100
call lastchar
expect 0x6f                    # 'o'

# Single-char counted string: len=1, then "X"
reset
poke 0x110 01_58
push_pad 0x110
call lastchar
expect 0x58

# Long counted string with high-bit byte at end
reset
poke 0x120 03_4142ff
push_pad 0x120
call lastchar
expect 0xff
