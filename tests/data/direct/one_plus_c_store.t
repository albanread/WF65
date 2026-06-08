# 1+c!  ( addr -- )   increment the byte at addr

poke 0 7f
push_pad 0
call one_plus_c_store
expect
expect_bytes 0 80