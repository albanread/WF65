# max  ( n1 n2 -- max )   signed maximum

push 3
push 5
call max_
expect 5

reset
push 5
push 3
call max_
expect 5

reset
push -1
push 0
call max_
expect 0
