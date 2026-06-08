# c+place  ( char dest -- )   append one character to a counted string

# Append '!'.
poke 0x100 02_4869_00                 # "Hi"
push 0x21
push_pad 0x100
call c_plus_place
expect
expect_bytes 0x100 03_486921_00

# Append a high-bit byte and keep trailing NUL.
reset
poke 0x120 01_41_00
push 0xff
push_pad 0x120
call c_plus_place
expect
expect_bytes 0x120 02_41ff_00