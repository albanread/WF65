# sm/rem  ( d n -- rem quot )   symmetric signed double÷single
#                                Symmetric: rem has the sign of the
#                                dividend (round-toward-zero on quot).

# Positive ÷ positive: 100 ÷ 7 → quot=14, rem=2
push 100
push 0
push 7
call sm_slash_rem
expect 2 14

# Negative ÷ positive: -10 ÷ 3 → quot=-3, rem=-1 (sign of dividend)
reset
push -10
push -1                  # high = -1 for sign-extension of -10
push 3
call sm_slash_rem
expect -1 -3
