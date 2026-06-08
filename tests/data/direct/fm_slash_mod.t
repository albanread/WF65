# fm/mod  ( d n -- rem quot )   floored signed double÷single
#                                Floored: quot rounds toward -∞;
#                                rem has the sign of the divisor.

# Positive ÷ positive: same as symmetric: 100 ÷ 7 → 14 r 2
push 100
push 0
push 7
call fm_slash_mod
expect 2 14

# Negative ÷ positive: -10 ÷ 3 → quot=-4, rem=2 (sign of divisor)
reset
push -10
push -1
push 3
call fm_slash_mod
expect 2 -4
