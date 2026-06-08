# b!  ( b addr -- )    store low byte at addr

push 0xCD
push_pad 0x88
call b_store
expect

push_pad 0x88
call b_fetch
expect 0xCD
