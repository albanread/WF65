# w!  ( w addr -- )    store low 16 bits at addr

# Pre-seed cell with sentinel pattern, then verify w! only touches 2 bytes.
push 0
push_pad 0x98
call store
expect

push 0xABCD
push_pad 0x98
call w_store
expect

push_pad 0x98
call w_fetch
expect 0xABCD
