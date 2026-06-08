# +!  ( n addr -- )    add n into the cell at addr

# Seed: store 100 into PAD+0x20
push 100
push_pad 0x20
call store
expect

# Add 5 in place
push 5
push_pad 0x20
call plus_store
expect

# Verify
push_pad 0x20
call fetch
expect 105
