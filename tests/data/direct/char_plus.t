# char+  ( c-addr -- c-addr+1 )   advance one char (1 byte)

push 0x1000
call char_plus
expect 0x1001
