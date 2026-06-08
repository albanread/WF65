# lower  ( addr len -- )   lowercase an s-string in place

poke 0x100 48654c4c4f2d21              # "HeLLO-!"
push_pad 0x100
push 7
call lower
expect
expect_bytes 0x100 68656c6c6f2d21      # "hello-!"

reset
poke 0x120 68656c6c6f
push_pad 0x120
push 0
call lower
expect
expect_bytes 0x120 68656c6c6f