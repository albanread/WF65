# cell-  ( addr -- addr-cell )

push 0x1008
call cell_minus
expect 0x1000
