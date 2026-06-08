# upc  ( char -- char )   ASCII uppercase fold for one byte

push 0x61
call upc
expect 0x41

reset
push 0x5a
call upc
expect 0x5a

reset
push 0xe9
call upc
expect 0xe9