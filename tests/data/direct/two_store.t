# 2!  ( x1 x2 a-addr -- )   store a pair: x2 at addr, x1 at addr+cell

push 100
push 200
push_pad 0x60
call two_store
expect

# Verify: addr → x2, addr+cell → x1
push_pad 0x60
call fetch
expect 200

reset
push_pad 0x68
call fetch
expect 100
