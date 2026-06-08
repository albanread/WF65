# and  ( n1 n2 -- n1&n2 )    bitwise AND
#
# NOT YET PORTED — harness reports as NYIMP.

push 0xFF00
push 0x0FF0
call and_
expect 0x0F00
