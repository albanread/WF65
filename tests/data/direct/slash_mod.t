# /mod  ( n1 n2 -- rem quot )    signed divide, returns rem AND quot
#
# WF32 implements /mod via `cdq + idiv`, which is SYMMETRIC division
# (round toward zero) — not floored. ANS Forth leaves /MOD's behaviour
# for negative divisors implementation-defined; the WF32 source's
# comment that says "floored" is misleading. Test expectations match
# what idiv actually does.

push 10
push 3
call slash_mod
expect 1 3                  # 10 / 3 = 3 rem 1

reset
push -10
push 3
call slash_mod
expect -1 -3                # symmetric: -10 / 3 = -3 rem -1
                            # (floored would be 2 -4; that's fm/mod)

reset
push 10
push -3
call slash_mod
expect 1 -3                 # symmetric: 10 / -3 = -3 rem 1
