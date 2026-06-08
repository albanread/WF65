# q@  ( addr -- n )    fetch 64-bit qword
#
# On WF32 q@ was "the 64-bit fetch" returning a double-cell pair.
# On WF64, a qword IS one cell — so q@ ≡ @ for our purposes.
# The PRIMITIVES table aliases q@ to the same xt as @.

push 0x123456789ABCDEF0
push_pad 0xB0
call store
expect

push_pad 0xB0
call q_fetch
expect 0x123456789ABCDEF0
