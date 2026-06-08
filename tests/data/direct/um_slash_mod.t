# um/mod  ( ud u -- ur uq )   unsigned double÷single → ( rem quot )

# 100 ÷ 7 = 14 rem 2.  ud=( low=100 high=0 ); divisor=7.
push 100
push 0
push 7
call um_slash_mod
expect 2 14

# Larger: double-cell dividend.
# ud = high*2^64 + low. Pick high=1, low=0 → 2^64. Divide by 2.
# Expected: 2^63 quot, 0 rem.
reset
push 0
push 1
push 2
call um_slash_mod
expect 0 -0x8000000000000000   # 2^63 as i64 = i64::MIN bit pattern
