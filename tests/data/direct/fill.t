# fill  ( c-addr u byte -- )   write `byte` into `u` consecutive bytes at c-addr

# Pre-poke distinctive pattern so we can spot what fill changes.
poke 0xC0 ffffffffffffffff_ffffffffffffffff

# Fill 5 bytes starting at PAD+0xC0 with 0xAA.
push_pad 0xC0
push 5
push 0xAA
call fill
expect

# First 5 bytes should be 0xAA, the rest of the cell untouched.
expect_bytes 0xC0 aaaaaaaaaaffffff_ffffffffffffffff
