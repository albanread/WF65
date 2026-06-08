# cells  ( n -- n*cell )   multiply by cell-size (8 on WF64)

push 0
call cells
expect 0

reset
push 1
call cells
expect 8

reset
push 5
call cells
expect 40

reset
push -3
call cells
expect -24
