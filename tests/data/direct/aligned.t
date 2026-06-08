# aligned  ( addr -- a-addr )   round UP to next cell boundary
#                                 (8-byte alignment on WF64)

push 0x1000
call aligned
expect 0x1000            # already aligned

reset
push 0x1001
call aligned
expect 0x1008            # rounds up

reset
push 0x1007
call aligned
expect 0x1008
