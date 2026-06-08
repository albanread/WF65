# largest  ( addr count -- max-addr max )   find highest cell in an array

push 10
push_pad 0x200
call store
expect
push 50
push_pad 0x208
call store
expect
push 20
push_pad 0x210
call store
expect

push_pad 0x200
push 3
call largest
call swap_
push_pad 0x208
call equal
expect 50 -1

# Equal maxima: keep the first max because the primitive only updates
# on strictly-greater values.
reset
push 50
push_pad 0x220
call store
expect
push 50
push_pad 0x228
call store
expect
push 10
push_pad 0x230
call store
expect

push_pad 0x220
push 3
call largest
call swap_
push_pad 0x220
call equal
expect 50 -1