# compare  ( a1 len1 a2 len2 -- n )    case-sensitive string compare.
#                                       0 = equal, -1 = a1 < a2, 1 = a1 > a2.

# ── equal ────────────────────────────────────────────────────────────
poke 0x100 48656c6c6f
poke 0x110 48656c6c6f
push_pad 0x100
push 5
push_pad 0x110
push 5
call compare
expect 0

# ── different but same length ───────────────────────────────────────
reset
poke 0x100 48656c6c6f
poke 0x110 48656c6c70                    # "Hellp" — last byte differs
push_pad 0x100
push 5
push_pad 0x110
push 5
call compare
expect -1                                 # a1 < a2

# Reverse: a2 < a1
reset
poke 0x100 48656c6c70
poke 0x110 48656c6c6f
push_pad 0x100
push 5
push_pad 0x110
push 5
call compare
expect 1

# ── prefix: shorter is less ──────────────────────────────────────────
reset
poke 0x100 4142
poke 0x110 414243
push_pad 0x100
push 2
push_pad 0x110
push 3
call compare
expect -1

# ── both empty ──────────────────────────────────────────────────────
reset
push_pad 0x100
push 0
push_pad 0x110
push 0
call compare
expect 0
