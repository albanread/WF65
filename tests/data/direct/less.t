# <  ( n1 n2 -- flag )   signed less-than

push 3
push 5
call less
expect -1                # 3 < 5

reset
push 5
push 3
call less
expect 0                 # 5 < 3 false

reset
push -1
push 0
call less
expect -1                # -1 < 0 true (signed)

reset
push 3
push 3
call less
expect 0
