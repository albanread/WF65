# du<  ( d1 d2 -- flag )   unsigned double less-than

push 10
push 0
push 20
push 0
call du_less
expect -1

reset
push -1
push 0
push 1
push 1
call du_less
expect -1

reset
push -1
push 1
push 0
push 1
call du_less
expect 0