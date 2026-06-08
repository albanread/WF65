# cmove  ( src dst u -- )   copy u bytes from src to dst, forward

# Source: PAD+0xD0 = "Hello" (5 bytes)
poke 0xD0 48656c6c6f
# Pre-fill destination with zeros so we can see what gets copied.
poke 0xE0 0000000000

push_pad 0xD0
push_pad 0xE0
push 5
call cmove
expect

expect_bytes 0xE0 48656c6c6f      # destination now has "Hello"
expect_bytes 0xD0 48656c6c6f      # source untouched
