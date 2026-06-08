# cells-  ( addr n -- addr-n*cell )

push 1000
push 3
call cells_minus
expect 976

reset
push 1000
push -2
call cells_minus
expect 1016