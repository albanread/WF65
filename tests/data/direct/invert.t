# invert  ( n -- ~n )    bitwise NOT (one's complement)
#
# NOT YET PORTED — harness reports as NYIMP.

push 0
call invert
expect -1

reset
push -1
call invert
expect 0
