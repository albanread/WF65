# cmove>  ( src dst u -- )   copy u bytes from src to dst, BACKWARD
#                              Use when dst > src and ranges overlap.

# Overlapping forward shift in PAD: "ABCDE" at +0xF0 → shift by 1 byte.
# cmove> handles this correctly; cmove would clobber.
poke 0xF0 4142434445_00       # "ABCDE\0"

push_pad 0xF0
push_pad 0xF1                  # dst = src + 1
push 5
call cmove_to                  # = cmove>
expect

# Result: original src "ABCDE\0" overlapped with shifted copy.
# After cmove>: bytes at 0xF0..0xF6 should be "AABCDE"
expect_bytes 0xF0 414142434445
