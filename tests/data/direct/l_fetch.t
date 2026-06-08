# L@  ( addr -- n )    fetch 32-bit dword, zero-extended into cell
#
# Subtle: WF32 had 32-bit cells so `mov eax, [eax]` was a full cell.
# On WF64 cells are 64 bits, but L@ specifically means "fetch the
# 32-bit dword" — we want `mov eax, dword [rax]` which zeroes the
# upper 32 bits of rax (x86-64 implicit zero-extension on 32-bit ops).

push 0
push_pad 0xA0
call store
expect

# Use store to put a 64-bit value with a known low 32 bits.
push 0x123456789ABCDEF0
push_pad 0xA0
call store
expect

# L@ should read the low 32 bits.
push_pad 0xA0
call l_fetch
expect 0x9ABCDEF0
