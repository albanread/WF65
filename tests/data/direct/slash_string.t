# /string  ( addr len n -- addr+n len-n )   advance a string by n chars

# "Hello" (5) /string 2 → ("llo", 3)
poke 0x100 48656c6c6f
push_pad 0x100
push 5
push 2
call slash_string
# Drop the addr, leave just the new length to verify.
call nip_
expect 3
# Verify the new addr points at "llo"
# (We need to recompute the addr first since nip cleared it.)
reset
poke 0x110 48656c6c6f
push_pad 0x110
push 5
push 2
call slash_string
# Now top=3, NOS=PAD+0x112. Check bytes at the new addr.
expect_bytes 0x112 6c6c6f         # "llo"

# Edge: skip 0 → unchanged
reset
push_pad 0x100
push 7
push 0
call slash_string
expect_bytes 0x100 48656c6c6f
call nip_
expect 7

# Edge: skip all → ("", 0)
reset
push_pad 0x100
push 5
push 5
call slash_string
call nip_
expect 0
