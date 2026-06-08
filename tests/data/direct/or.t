# or  ( n1 n2 -- n1|n2 )    bitwise OR
#
# NOT YET PORTED — harness reports as NYIMP.

push 0xFF00
push 0x00FF
call or_
expect 0xFFFF
