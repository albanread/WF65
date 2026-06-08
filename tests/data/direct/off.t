# off  ( addr -- )   store 0 at addr

push -1
push_pad 0x48
call store
expect

push_pad 0x48
call off
expect

push_pad 0x48
call fetch
expect 0
