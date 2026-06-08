# exchange  ( addr1 addr2 -- )   swap the cell contents at two addresses

# Seed two cells with distinct values
push 0xAAAA_AAAA_AAAA_AAAA
push_pad 0x100
call store
expect
push 0xBBBB_BBBB_BBBB_BBBB
push_pad 0x108
call store
expect

# Exchange
push_pad 0x100
push_pad 0x108
call exchange
expect

# Verify the swap
push_pad 0x100
call fetch
expect 0xBBBB_BBBB_BBBB_BBBB

reset
push_pad 0x108
call fetch
expect 0xAAAA_AAAA_AAAA_AAAA
