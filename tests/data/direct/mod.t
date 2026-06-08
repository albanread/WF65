# mod  ( n1 n2 -- rem )   signed remainder (symmetric — sign of dividend)

push 100
push 7
call mod_
expect 2

reset
push -10
push 3
call mod_
expect -1              # symmetric: rem has sign of dividend
