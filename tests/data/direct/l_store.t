# L!  ( n addr -- )    store low 32 bits at addr (doesn't touch upper 32)

# Pre-seed cell with a recognisable pattern.
push -1
push_pad 0xA8
call store
expect

# L! the low 32 bits.
push 0x12345678
push_pad 0xA8
call l_store
expect

# Read full cell: low 32 = 0x12345678, upper 32 untouched (still 0xFFFFFFFF).
push_pad 0xA8
call fetch
expect 0xFFFFFFFF12345678     # parse_int handles the u64-to-i64 bit-cast
