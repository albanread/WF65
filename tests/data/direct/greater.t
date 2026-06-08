# >  ( n1 n2 -- flag )   signed greater-than

push 5
push 3
call greater
expect -1

reset
push 3
push 5
call greater
expect 0

reset
push 3
push 3
call greater
expect 0
