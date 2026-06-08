# 1+!  ( addr -- )    increment the cell at addr in place

push 10
push_pad 0x28
call store
expect

push_pad 0x28
call one_plus_store
expect

push_pad 0x28
call one_plus_store
expect

push_pad 0x28
call fetch
expect 12
