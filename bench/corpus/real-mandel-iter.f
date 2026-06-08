\ bench/corpus/real-mandel-iter.f
\ Transform 6 — real-world cross-interaction via an integer fixed-point Mandelbrot
\ escape-iteration count (NO gfx/canvas — pure compute, headless). Fixed point has
\ 8 fractional bits (1.0 = 256). Combines: T1 (fold on 256/1024 constants), T2
\ (bare * + - inline on running z values), T3 (scheduling over a tight body),
\ T5 (dup/2dup stack-ops). Bounded by maxiter, single result on the stack.
\
\ Verdict word: mandel-iter ( cx cy maxiter -- count ).

variable mi-cx
variable mi-cy
variable mi-zx
variable mi-zy
variable mi-cnt

: mandel-iter ( cx cy maxiter -- count )
  >r                                    \ R: maxiter ;  stack ( cx cy )
  mi-cy !  mi-cx !
  0 mi-zx !  0 mi-zy !  0 mi-cnt !
  r> 0 ?do
    mi-zx @ dup *  256 /                ( zx2 )
    mi-zy @ dup *  256 /                ( zx2 zy2 )
    2dup +  1024 >  if  2drop  leave  then
    mi-zx @ mi-zy @ *  256 /  2*  mi-cy @ +   ( zx2 zy2 zynew )
    >r                                  ( zx2 zy2 )  R: zynew
    -  mi-cx @ +                        ( zxnew )
    mi-zx !  r> mi-zy !
    1 mi-cnt +!
  loop
  mi-cnt @ ;

\ load-time self-check: a small bounded instance, leave the stack balanced.
-64 0 16 mandel-iter drop
