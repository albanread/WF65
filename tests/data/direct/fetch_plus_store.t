# @+!  ( n addr -- old )   fetch old cell, add n into memory, return old

push 10
push_pad 0
call store
expect

push 5
push_pad 0
call fetch_plus_store
expect 10

push_pad 0
call fetch
expect 10 15