# 2@  ( a-addr -- x1 x2 )    fetch a pair: x2 at addr, x1 at addr+cell
#                              leaves them as ( ... x1 x2 ) with x2 on top

# Seed two cells: 100 at PAD+0x50, 200 at PAD+0x58
push 200
push_pad 0x50
call store
expect
push 100
push_pad 0x58
call store
expect

# 2@ from PAD+0x50 → ( 100 200 )
push_pad 0x50
call two_fetch
expect 100 200
