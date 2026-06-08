# 2/  ( n -- n/2 )    signed (sar) — rounds toward -infinity for negatives

push 42
call two_slash
expect 21

# -7 sar 1 = -4 (rounds DOWN, not toward zero). This is what
# Forth ANS specifies for 2/ via "arithmetic shift right."
reset
push -7
call two_slash
expect -4
