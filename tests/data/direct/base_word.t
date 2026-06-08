# base  ( -- addr )   return the address of USER_BASE so @ and ! work

call base_word
call fetch
expect 10

reset
push 16
call base_word
call store
expect
call base_word
call fetch
expect 16

reset
call base_word
push 8
call swap_
call store
expect
call base_word
call fetch
expect 8