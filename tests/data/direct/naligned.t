# naligned  ( addr n -- a-addr )   round up to n-byte boundary

push 1001
push 16
call naligned
expect 1008

reset
push 1000
push 16
call naligned
expect 1008

reset
push 1008
push 16
call naligned
expect 1008