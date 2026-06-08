# lshift  ( u count -- u<<count )
#
# NOT YET PORTED — harness reports as NYIMP.

push 1
push 4
call lshift
expect 16

reset
push 0xFF
push 8
call lshift
expect 0xFF00
