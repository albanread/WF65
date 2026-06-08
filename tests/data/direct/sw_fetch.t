# sw@  ( addr -- w )    fetch 16-bit word, sign-extended

push 0xFFFF
push_pad 0x94
call store
expect

push_pad 0x94
call sw_fetch
expect -1

reset
push 0x7FFF
push_pad 0x94
call store
expect
push_pad 0x94
call sw_fetch
expect 0x7FFF
