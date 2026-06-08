# u2/  ( u -- u/2 )    unsigned (shr) — logical shift right

# -1 as u64 is 0xFFFF_FFFF_FFFF_FFFF; >>1 logical = 0x7FFF_FFFF_FFFF_FFFF
push -1
call u2slash
expect 0x7FFFFFFFFFFFFFFF
