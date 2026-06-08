# m*  ( n1 n2 -- d )    signed 64×64 → 128-bit double

# Positive: 100 × 200 = 20000
push 100
push 200
call m_times
expect 20000 0

# Negative × positive: -5 × 7 = -35
# As 128-bit: low = -35 (= 0xFFFF…FFDD), high = -1 (all ones, sign-ext)
reset
push -5
push 7
call m_times
expect -35 -1

# Negative × negative = positive
reset
push -5
push -7
call m_times
expect 35 0
