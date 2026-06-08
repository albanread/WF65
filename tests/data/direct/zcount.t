# zcount  ( z-addr -- addr len )    NUL-terminated string → (addr, strlen)
#     Unlike `count`, the addr is unchanged — zcount just measures the
#     length up to the terminating NUL via `repnz scasb`.

# ── Case 1: typical Z-string ─────────────────────────────────────────
poke 0x100 48656c6c6f_00       # "Hello\0"
push_pad 0x100
call zcount
call nip_
expect 5

# ── Case 2: empty Z-string (just NUL) ────────────────────────────────
reset
poke 0x110 00
push_pad 0x110
call zcount
call nip_
expect 0

# ── Case 3: single-char ──────────────────────────────────────────────
reset
poke 0x120 41_00              # "A\0"
push_pad 0x120
call zcount
call nip_
expect 1

# ── Case 4: long-ish (16 chars) ──────────────────────────────────────
reset
poke 0x130 4142434445464748_494a4b4c4d4e4f50_00
push_pad 0x130
call zcount
call nip_
expect 16
