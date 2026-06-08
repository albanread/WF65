# cell+  ( addr -- addr+cell )

push 0x1000
call cell_plus
expect 0x1008
