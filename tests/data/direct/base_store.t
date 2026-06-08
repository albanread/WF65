# base!  ( base -- )   store the current numeric base

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

reset
call decimal_word
expect
call base_fetch
expect 10