# w@  ( addr -- w )    fetch 16-bit word, zero-extended

# Store 0x1234 as two bytes (little-endian) at PAD+0x90.
# Simpler: store 0x00001234 as full cell, then w@ reads low 16 bits.
push 0x1234
push_pad 0x90
call store
expect

push_pad 0x90
call w_fetch
expect 0x1234
