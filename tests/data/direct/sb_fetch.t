# sb@  ( addr -- b )    fetch byte (signed, sign-extended)

# Store 0xFF; sign-extended fetch should give -1.
push 0xFF
push_pad 0x84
call c_store
expect

push_pad 0x84
call sb_fetch
expect -1

# And 0x7F stays positive.
reset
push 0x7F
push_pad 0x84
call c_store
expect
push_pad 0x84
call sb_fetch
expect 0x7F
