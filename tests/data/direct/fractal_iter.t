# fractal-iter  ( maxiter -- n )  ( F: z0x z0y cx cy -- )
#
# FP stack protocol: push z0x z0y cx cy in that order (cy on FP-TOS).
# To push a float we use: push N; call s_to_d; call d_to_f
# s>d sign-extends, d>f converts; for small non-negative integers the
# result is exact IEEE-754 double.
#
# Interior: c=(0,0) z0=(0,0) maxiter=8 → all iterations consumed → 8
# Exterior: c=(2,0) z0=(0,0) maxiter=8 → |z|²=4 after first step → 1

# ── z0x ──────────────────────────────────────────────────────────────────
# push float helper sequence used for each float value below:
#   push <N>  call s_to_d  call d_to_f

# Interior: c=(0+0i), z₀=(0+0i).  z never escapes; iter count = maxiter.
reset
push 0
call s_to_d
call d_to_f
push 0
call s_to_d
call d_to_f
push 0
call s_to_d
call d_to_f
push 0
call s_to_d
call d_to_f
push 8
call fractal_iter_word
expect 8

# Exterior: c=(2+0i), z₀=(0+0i).
# Step 0: z=0, new_zx=0-0+2=2, new_zy=0 → z=2+0i
# Step 1: |z|²=4 ≥ 4 → escaped with iter=1
reset
push 0
call s_to_d
call d_to_f
push 0
call s_to_d
call d_to_f
push 2
call s_to_d
call d_to_f
push 0
call s_to_d
call d_to_f
push 8
call fractal_iter_word
expect 1

# maxiter=0 → loop never runs → return 0 regardless of c.
reset
push 0
call s_to_d
call d_to_f
push 0
call s_to_d
call d_to_f
push 0
call s_to_d
call d_to_f
push 0
call s_to_d
call d_to_f
push 0
call fractal_iter_word
expect 0
