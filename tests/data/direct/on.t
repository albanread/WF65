# on  ( addr -- )   store -1 (all bits) at addr

push 0
push_pad 0x40
call store
expect

push_pad 0x40
call on
expect

push_pad 0x40
call fetch
expect -1
