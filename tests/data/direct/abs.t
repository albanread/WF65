# abs  ( n -- |n| )    absolute value via cqo-then-xor

push -99
call abs
expect 99

reset
push 99
call abs
expect 99

# Edge case: i64::MIN. Negating it overflows; abs returns it
# unchanged in two's complement. Must not trap on the cqo path.
reset
push -0x8000000000000000
call abs
expect -0x8000000000000000
