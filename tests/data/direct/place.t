# place  ( addr len dest -- )   overwrite a counted destination string

# Fresh destination: place "Hello" into counted buffer and keep trailing NUL.
poke 0x180 48656c6c6f
poke 0x100 03_6f6c64_00               # pre-existing "old"
push_pad 0x180
push 5
push_pad 0x100
call place
expect
expect_bytes 0x100 05_48656c6c6f_00

# Empty source clears the counted length and leaves a NUL terminator.
reset
poke 0x120 04_74657374_00             # "test"
push_pad 0x180
push 0
push_pad 0x120
call place
expect
expect_bytes 0x120 00_00