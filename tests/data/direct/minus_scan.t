# -scan  ( addr len char -- addr' len' )    scan BACKWARD from end of buffer.
#     On hit: returns ( addr+match_pos+1, len-match_pos-1 ) — the
#             trailing slice AFTER the matched char (matches typical
#             tokeniser use: find a delimiter, take what comes after).
#     On miss: returns ( addr, len ) — input unchanged.
#     Empty input: returns ( addr, 0 ).

# "ab cd" scan back for ' ' → match at index 2 → slice "cd" = len 2
poke 0x100 6162206364
push_pad 0x100
push 5
push 0x20
call minus_scan
call nip_
expect 2

# No match → input unchanged (len = 5)
reset
poke 0x100 6162636465
push_pad 0x100
push 5
push 0x20
call minus_scan
call nip_
expect 5

# Empty input → length 0 out
reset
push_pad 0x100
push 0
push 0x20
call minus_scan
call nip_
expect 0

# Match at the LAST byte → empty trailing slice
reset
poke 0x110 6162636420            # "abcd "
push_pad 0x110
push 5
push 0x20
call minus_scan
call nip_
expect 0

# Match at the FIRST byte → whole rest is the slice
reset
poke 0x120 2061626364            # " abcd"
push_pad 0x120
push 5
push 0x20
call minus_scan
call nip_
expect 4
