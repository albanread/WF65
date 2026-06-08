# tr  ( addr len table -- )   translate a buffer in place via a 256-byte table

# ── Case 1: selective translation in place ──────────────────────────
# Buffer = "abcXYZ". Table maps a/b/c -> A/B/C and X/Y/Z -> x/y/z.
poke 0x100 61626358595a
poke 0x261 41
poke 0x262 42
poke 0x263 43
poke 0x258 78
poke 0x259 79
poke 0x25a 7a
push_pad 0x100
push 6
push_pad 0x200
call tr
expect
expect_bytes 0x100 41424378797a

# ── Case 2: zero-length buffer leaves memory untouched ──────────────
reset
poke 0x120 deadbeef
poke 0x2de aa
push_pad 0x120
push 0
push_pad 0x200
call tr
expect
expect_bytes 0x120 deadbeef