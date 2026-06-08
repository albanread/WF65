# str=  ( a1 len1 a2 len2 -- flag )   case-sensitive equality
#                                      Forth flag: -1 equal, 0 not.

poke 0x100 48656c6c6f
poke 0x110 48656c6c6f
push_pad 0x100
push 5
push_pad 0x110
push 5
call str_equal
expect -1

# Length mismatch: not equal regardless of content
reset
push_pad 0x100
push 5
push_pad 0x110
push 4
call str_equal
expect 0

# Same length, different content
reset
poke 0x100 48656c6c6f
poke 0x110 48656c6c70
push_pad 0x100
push 5
push_pad 0x110
push 5
call str_equal
expect 0

# Both empty → equal
reset
push_pad 0x100
push 0
push_pad 0x110
push 0
call str_equal
expect -1
