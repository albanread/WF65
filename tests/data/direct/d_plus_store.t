# d+!  ( d addr -- )   add double d into the pair stored at addr

push 10
push 0
push_pad 0x20
call two_store
expect

push 5
push 0
push_pad 0x20
call d_plus_store
expect

push_pad 0x20
call two_fetch
expect 15 0