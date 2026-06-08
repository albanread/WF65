# cells+  ( addr n -- addr+n*cell )

push 1000
push 3
call cells_plus
expect 1024

reset
push 1000
push -2
call cells_plus
expect 984