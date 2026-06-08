# rshift  ( u count -- u' )   logical (unsigned) right shift

push 0x100
push 4
call rshift
expect 0x10

reset
push -1
push 1
call rshift
expect 0x7FFFFFFFFFFFFFFF
