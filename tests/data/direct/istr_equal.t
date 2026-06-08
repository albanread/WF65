# istr=  ( a1 len1 a2 len2 -- flag )   case-INSENSITIVE equality

# Same letters, different case
poke 0x100 48454c4c4f                    # "HELLO"
poke 0x110 68656c6c6f                    # "hello"
push_pad 0x100
push 5
push_pad 0x110
push 5
call istr_equal
expect -1

# Mixed case
reset
poke 0x100 48656c4c6f                    # "HelLo"
poke 0x110 68454c4c4f                    # "hELLO"
push_pad 0x100
push 5
push_pad 0x110
push 5
call istr_equal
expect -1

# Truly different
reset
poke 0x100 48656c6c6f
poke 0x110 576f726c64                    # "World"
push_pad 0x100
push 5
push_pad 0x110
push 5
call istr_equal
expect 0
