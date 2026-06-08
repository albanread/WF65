# 1-!  ( addr -- )    decrement the cell at addr in place

push 10
push_pad 0x30
call store
expect

push_pad 0x30
call one_minus_store
expect

push_pad 0x30
call fetch
expect 9
