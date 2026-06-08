# move  ( src dst u -- )    copy u bytes, handling overlap
#                            Picks cmove or cmove> based on direction.

# ── Case 1: non-overlapping forward copy ─────────────────────────────
poke 0x100 48656c6c6f          # src = "Hello"
poke 0x110 0000000000          # dst pre-zeroed
push_pad 0x100
push_pad 0x110
push 5
call move
expect
expect_bytes 0x110 48656c6c6f
expect_bytes 0x100 48656c6c6f  # src untouched

# ── Case 2: zero-length copy (must NOT crash, must NOT touch dst) ────
poke 0x120 deadbeefcafebabe
poke 0x128 ffffffffffffffff   # sentinel pattern at dst
push_pad 0x120
push_pad 0x128
push 0
call move
expect
expect_bytes 0x128 ffffffffffffffff   # untouched

# ── Case 3: single-byte copy ─────────────────────────────────────────
poke 0x130 5a
poke 0x131 00
push_pad 0x130
push_pad 0x131
push 1
call move
expect
expect_bytes 0x131 5a

# ── Case 4: forward overlap (dst > src, ranges overlap) ──────────────
# "ABCDE" at 0x140; shift to 0x141 (overlap by 4 bytes).
# move must pick cmove> here to avoid clobbering src.
poke 0x140 4142434445_00
push_pad 0x140
push_pad 0x141
push 5
call move
expect
# Expected: 0x140..0x146 = "AABCDE" (src[0] kept, then shifted copy)
expect_bytes 0x140 414142434445

# ── Case 5: backward overlap (dst < src) ─────────────────────────────
# "ABCDE" at 0x151; shift to 0x150.
poke 0x150 00_4142434445
push_pad 0x151
push_pad 0x150
push 5
call move
expect
# Expected: 0x150..0x156 = "ABCDEE" (shifted, src[4] kept)
expect_bytes 0x150 414243444545

# ── Case 6: long copy (256 bytes — exercises rep movsb at scale) ─────
# Fill src with sequential bytes via fill-then-poke-pattern.
push_pad 0x200
push 256
push 0x5a
call fill
expect
push_pad 0x300
push 256
push 0xa5
call fill
expect
# Now move src→dst
push_pad 0x200
push_pad 0x300
push 256
call move
expect
# Verify dst is now full of 0x5a (no 0xa5 left)
expect_bytes 0x300 5a5a5a5a5a5a5a5a_5a5a5a5a5a5a5a5a
expect_bytes 0x3f0 5a5a5a5a5a5a5a5a_5a5a5a5a5a5a5a5a
