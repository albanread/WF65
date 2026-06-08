# +cells  ( n addr -- addr+n*cell )

push 3
push 1000
call plus_cells
expect 1024

reset
push -2
push 1000
call plus_cells
expect 984