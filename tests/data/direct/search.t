# search  ( a1 len1 a2 len2 -- a3 len3 flag )    find a2 inside a1
#     On hit:  a3 = position of match, len3 = bytes remaining from there, flag = -1
#     On miss: a3 = a1,                 len3 = len1,                       flag = 0
#
# Tests drop a3 and len3 with two `nip_`s to leave just the flag for
# `expect`, and `expect_bytes` checks the position when it matters.

# ── Hit in the middle ────────────────────────────────────────────────
poke 0x100 48656c6c6f20776f726c64           # "Hello world"
poke 0x120 6c6f                              # "lo"
push_pad 0x100
push 11
push_pad 0x120
push 2
call search
# Position should be at byte 3 — verify via the bytes there.
expect_bytes 0x103 6c6f20776f726c64
# Clear stack down to flag.
call nip_
call nip_
expect -1

# ── Hit at start ─────────────────────────────────────────────────────
reset
poke 0x100 48656c6c6f
poke 0x120 4865                              # "He"
push_pad 0x100
push 5
push_pad 0x120
push 2
call search
call nip_
call nip_
expect -1

# ── Miss ─────────────────────────────────────────────────────────────
reset
poke 0x100 48656c6c6f
poke 0x120 7878                              # "xx"
push_pad 0x100
push 5
push_pad 0x120
push 2
call search
call nip_
call nip_
expect 0

# ── Empty needle: always matches at start ────────────────────────────
reset
poke 0x100 48656c6c6f
push_pad 0x100
push 5
push_pad 0x120
push 0
call search
call nip_
call nip_
expect -1
