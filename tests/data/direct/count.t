# count  ( c-addr -- c-addr+1 u )   read counted string:
#                                    leading byte = length, followed by chars

# Counted string at PAD+0x100: length=5, then "Hello"
poke 0x100 05_48656c6c6f

push_pad 0x100
call count
# Stack: ( c-addr+1 5 ). Verify the bytes that c-addr+1 points at —
# this confirms the address arithmetic worked.
expect_bytes 0x101 48656c6c6f
# Drop the addr, leave length on top, then assert.
call nip_
expect 5
