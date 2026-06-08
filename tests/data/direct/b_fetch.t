# b@  ( addr -- b )    fetch byte (unsigned, zero-extended)

# Seed PAD+0x80 with 0xAB
push 0xAB
push_pad 0x80
call c_store
expect

push_pad 0x80
call b_fetch
expect 0xAB
