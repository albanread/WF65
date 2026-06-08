# */mod  ( n1 n2 n3 -- rem (n1*n2)/n3 )
#     Like */, also returns the remainder.

# 10 * 7 / 3 = 70/3 = 23 rem 1
push 10
push 7
push 3
call times_slash_mod
expect 1 23
