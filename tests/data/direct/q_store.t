# q!  ( n addr -- )    store 64-bit qword. On WF64, q! ≡ ! (cell-sized).

push 0xCAFEBABEDEADBEEF
push_pad 0xB8
call q_store
expect

push_pad 0xB8
call fetch
expect 0xCAFEBABEDEADBEEF
