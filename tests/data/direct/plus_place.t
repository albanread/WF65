# +place  ( addr len dest -- )   append a string to a counted destination

# Append "XYZ" to counted "abc".
poke 0x180 58595a
poke 0x100 03_616263_00
push_pad 0x180
push 3
push_pad 0x100
call plus_place
expect
expect_bytes 0x100 06_61626358595a_00

# Zero-length append leaves destination untouched.
reset
poke 0x120 02_4869_00                 # "Hi"
push_pad 0x180
push 0
push_pad 0x120
call plus_place
expect
expect_bytes 0x120 02_4869_00