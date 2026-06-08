# -aligned  ( addr -- a-addr )   round down to cell boundary

push 1003
call minus_aligned
expect 1000

reset
push 1000
call minus_aligned
expect 1000