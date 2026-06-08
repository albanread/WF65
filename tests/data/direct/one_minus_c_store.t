# 1-c!  ( addr -- )   decrement the byte at addr

poke 0 00
push_pad 0
call one_minus_c_store
expect
expect_bytes 0 ff