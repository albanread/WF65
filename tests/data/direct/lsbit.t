# lsbit  ( n -- lsb )   index of lowest set bit; -1 if n==0

push 0
call lsbit
expect -1

reset
push 1
call lsbit
expect 0

reset
push 0x80
call lsbit
expect 7                 # only bit 7 set

reset
push 0x8000000000000000  # only top bit set (i64::MIN bit pattern)
call lsbit
expect 63
