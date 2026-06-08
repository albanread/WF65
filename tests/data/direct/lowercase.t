# lowercase  ( c-addr -- c-addr )   lowercase a counted string in place

poke 0x100 05_48654c4c4f_00            # counted "HeLLO"
push_pad 0x100
call lowercase
call drop_
expect
expect_bytes 0x100 05_68656c6c6f_00