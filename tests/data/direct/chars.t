# chars  ( n -- n )   multiply by char-size (1 byte) — no-op on byte chars

push 7
call chars
expect 7

reset
push 0
call chars
expect 0
