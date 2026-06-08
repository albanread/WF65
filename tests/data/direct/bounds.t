# bounds  ( addr len -- lim first )   compute do-loop bounds.
#     After bounds: stack is (lim, first) bottom-first, so `minus`
#     yields lim - first = len. The simplest possible check.

push_pad 0x100
push 5
call bounds
call minus                      # lim - first = 5
expect 5

# Zero-length: lim should equal first.
reset
push_pad 0x100
push 0
call bounds
call minus
expect 0

# Longer
reset
push_pad 0x100
push 0x10000
call bounds
call minus
expect 0x10000
