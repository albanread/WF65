# c+!  ( c addr -- )   add low byte of c into the byte at addr

poke 0 10
push 5
push_pad 0
call c_plus_store
expect
expect_bytes 0 15

reset
poke 0 fe
push 5
push_pad 0
call c_plus_store
expect
expect_bytes 0 03