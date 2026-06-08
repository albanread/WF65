# negate  ( n -- -n )    two's-complement negation

push 42
call negate
expect -42
call negate
expect 42
