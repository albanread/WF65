# upper  ( addr len -- )   uppercase an s-string in place

poke 0x100 4162632d7a7921              # "Abc-zy!"
push_pad 0x100
push 7
call upper
expect
expect_bytes 0x100 4142432d5a5921      # "ABC-ZY!"

reset
poke 0x120 48656c6c6f
push_pad 0x120
push 0
call upper
expect
expect_bytes 0x120 48656c6c6f