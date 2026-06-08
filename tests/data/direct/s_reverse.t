# s-reverse  ( x[k-1] ... x1 x0 k -- x0 x1 ... x[k-1] )

# One item: drops k and leaves the single item unchanged.
push 42
push 1
call s_reverse
expect 42

# Odd count.
reset
push 1
push 2
push 3
push 4
push 5
push 5
call s_reverse
expect 5 4 3 2 1

# Even count.
reset
push 10
push 20
push 30
push 40
push 4
call s_reverse
expect 40 30 20 10

# Preserve deeper stack items below the reversed slice.
reset
push 99
push 1
push 2
push 3
push 3
call s_reverse
expect 99 3 2 1