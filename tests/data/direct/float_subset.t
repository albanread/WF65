# Initial MASM-backed floating-point subset.

# D>F / F>D roundtrip for sign-extended 64-bit doubles.
push -123
push -1
call d_to_f
call f_to_d
expect -123 -1

reset
push 9
push 0
call d_to_f
call fdup
call f_to_d
call f_to_d
expect 9 0 9 0

reset
push 1
push 0
call d_to_f
push 2
push 0
call d_to_f
call fdepth
expect 2

reset
push 1
push 0
call d_to_f
push 2
push 0
call d_to_f
call fswap
call f_to_d
call f_to_d
expect 1 0 2 0

reset
push 1
push 0
call d_to_f
push 2
push 0
call d_to_f
call fover
call f_to_d
call f_to_d
call f_to_d
expect 1 0 2 0 1 0

reset
push 1
push 0
call d_to_f
push 2
push 0
call d_to_f
call fdrop
call f_to_d
expect 1 0

reset
push 10
push 0
call d_to_f
push 5
push 0
call d_to_f
call f_plus
call f_to_d
expect 15 0

reset
push 10
push 0
call d_to_f
push 3
push 0
call d_to_f
call f_minus
call f_to_d
expect 7 0

reset
push 6
push 0
call d_to_f
push 7
push 0
call d_to_f
call f_times
call f_to_d
expect 42 0

reset
push 7
push 0
call d_to_f
push 2
push 0
call d_to_f
call f_slash
call f_to_d
expect 3 0

reset
push 3
push 0
call d_to_f
call f_negate
call f_to_d
expect -3 -1

reset
push 0
push 0
call d_to_f
call f_zero_equal
expect -1

reset
push -1
push -1
call d_to_f
call f_zero_less
expect -1

reset
push 2
push 0
call d_to_f
push 5
push 0
call d_to_f
call f_less
expect -1

reset
push 42
push 0
call d_to_f
push_pad 0x80
call f_store
push_pad 0x80
call f_fetch
call f_to_d
expect 42 0

reset
push_pad 0x80
call float_plus
push_pad 0x88
call minus
expect 0

reset
push 3
call floats
expect 24

reset
push_pad 0x81
call faligned
push_pad 0x88
call minus
expect 0