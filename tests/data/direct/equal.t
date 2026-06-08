# =  ( n1 n2 -- flag )

push 42
push 42
call equal
expect -1

reset
push 41
push 42
call equal
expect 0

reset
push -1
push -1
call equal
expect -1
