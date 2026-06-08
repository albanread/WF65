# >number  ( ud1 c-addr1 u1 -- ud2 c-addr2 u2 )

# Case 1: decimal full parse in base 10.
push 10
push_pad -0x100                      # user_BASE lives at user_base + 0x00
call store
expect
poke 0x100 31323334                 # "1234"
push 0
push 0
push_pad 0x100
push 4
call to_number
call nip_                           # drop addr, keep remaining len
expect 1234 0 0

# Case 2: stop at first non-digit, leave addr/len pointing at the tail.
reset
push 10
push_pad -0x100
call store
expect
poke 0x110 313278                   # "12x"
push 0
push 0
push_pad 0x110
push 3
call to_number
call over_                          # duplicate addr
call c_fetch                        # fetch first unconsumed byte
call nip_                           # drop len, keep byte
call nip_                           # drop addr, keep byte
expect 12 0 0x78

# Case 3: hex parse honours BASE.
reset
push 16
push_pad -0x100
call store
expect
poke 0x120 41424344                 # "ABCD"
push 0
push 0
push_pad 0x120
push 4
call to_number
call nip_
expect 0xabcd 0 0

# Restore BASE to 10 for later tests.
reset
push 10
push_pad -0x100
call store
expect