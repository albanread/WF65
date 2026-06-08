# within  ( n low high -- flag )   true if low <= n < high
#                                   (uses unsigned compare per ANS Forth)

push 5
push 0
push 10
call within
expect -1                # 0 <= 5 < 10

reset
push 0
push 0
push 10
call within
expect -1                # 0 <= 0 < 10 (low boundary inclusive)

reset
push 10
push 0
push 10
call within
expect 0                 # 10 NOT < 10 (high boundary exclusive)

reset
push -1
push 0
push 10
call within
expect 0                 # -1 unsigned is huge, NOT in [0, 10)
