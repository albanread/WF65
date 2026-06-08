# uppercase  ( c-addr -- c-addr )   uppercase a counted string in place

poke 0x100 05_48656c4c6f_00            # counted "HelLo"
push_pad 0x100
call uppercase
call drop_
expect
expect_bytes 0x100 05_48454c4c4f_00