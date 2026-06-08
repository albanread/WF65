# number?  ( c-addr u -- n -1 | c-addr u 0 )

# Decimal positive.
poke 0x100 31323334                 # "1234"
push_pad 0x100
push 4
call number_q
expect 1234 -1

# Decimal negative.
reset
poke 0x110 2d3432                   # "-42"
push_pad 0x110
push 3
call number_q
expect -42 -1

# Hex in BASE 16, lower-case letters accepted like >number.
reset
push 16
push_pad -0x100
call store
expect
poke 0x120 3261                     # "2a"
push_pad 0x120
push 2
call number_q
expect 42 -1

# Invalid digit preserves the original (addr len) and pushes 0.
reset
push 10
push_pad -0x100
call store
expect
poke 0x130 313278                   # "12x"
push_pad 0x130
push 3
call number_q
call drop_                         # drop failure flag
call nip_                          # keep preserved len
expect 3

# Restore BASE for later files.
reset
push 10
push_pad -0x100
call store
expect