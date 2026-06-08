# /  ( n1 n2 -- quot )    signed divide.
#     WF32 implements via idiv (symmetric, round-toward-zero).

push 100
push 7
call slash
expect 14

reset
push -10
push 3
call slash
expect -3              # symmetric: round toward 0
