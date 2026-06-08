# min  ( n1 n2 -- min )   signed minimum

push 3
push 5
call min_
expect 3

reset
push 5
push 3
call min_
expect 3

reset
push -1
push 0
call min_
expect -1                # -1 is less than 0 signed
