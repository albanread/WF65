# base@  ( -- base )   fetch the current numeric base

call base_fetch
expect 10

reset
push 16
call base_store
expect
call base_fetch
expect 16

reset
call octal_word
expect
call base_fetch
expect 8