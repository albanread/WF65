# digit  ( char base -- n flag )

# Decimal digit.
push 0x37
push 10
call digit
expect 7 -1

# Hex digit, upper-case input (matches WF32's `digit`).
reset
push 0x46
push 16
call digit
expect 15 -1

# Out of range for the base: flag false, original char left in NOS.
reset
push 0x38
push 8
call digit
expect 0x38 0