\ gfx-canvas-mandelbrot-let.f — high-res Mandelbrot, LET (LLVM) math.
\
\ Companion to gfx-canvas-mandelbrot.f.  Identical canvas fast path
\ (fill a BGRA framebuffer, ship ONE canvas-blit), but the per-pixel
\ iteration arithmetic comes from the LET extension instead of the
\ hand-written MASM `fractal-iter` primitive.
\
\ LET compiles an infix float expression to native SSE via Rust/LLVM
\ at definition time.  `mbrot-step` does ONE  z <- z^2 + c  step and
\ returns |z'|^2 (escape radius squared) for free, so the Forth outer
\ loop just counts steps until |z|^2 >= 4.
\
\ The instructive A/B: here LLVM compiles the step and a Forth loop
\ drives it (one LET call per iteration); the MASM version runs the
\ whole escape loop in registers (one call per pixel).  Same picture,
\ two codegen paths — compare the speed.

640 constant cml-w
480 constant cml-h
128 constant cml-maxiter

\ Framebuffer: cml-w * cml-h pixels, 4 bytes each.  Every pixel is
\ written before blit, so it never needs zeroing.
cml-w cml-h *  4 *   buffer: cml-fb

\ Complex-plane step per pixel.  The `e` literals MUST equal cml-w / cml-h.
3.5e 640e f/   fconstant cml-dx     \ real span 3.5 across cml-w columns
2.5e 480e f/   fconstant cml-dy     \ imag span 2.5 across cml-h rows

fvariable cml-cre
fvariable cml-cim
variable  cml-row

\ One Mandelbrot iteration, compiled to native SSE by LET/LLVM.
\ ( F: z_re z_im c_re c_im -- z_re' z_im' mag )
\
\ The whole LET form MUST be on one line: the Demos-menu / REPL eval
\ path is line-buffered, so a multi-line LET only sees its first line
\ and throws -2056 (which shows up as "':' without ';'").  Loaded via
\ `include` it could span lines, but here it can't — so keep it flat.
: mbrot-step LET (z_re, z_im, c_re, c_im) -> (z_next_re, z_next_im, mag) = re, im, rmag WHERE re = z_re * z_re - z_im * z_im + c_re WHERE im = 2 * z_re * z_im + c_im WHERE rmag = re * re + im * im END ;

\ Escape-time count for the c currently in cml-cre / cml-cim.  z rides
\ on the FP stack across iterations: mbrot-step consumes (z, c) and
\ leaves (z', mag2); `4.0e f<` tests mag2 < 4 and leaves z' for the
\ next step.  Returns the iteration it escaped on, or cml-maxiter for
\ interior points.
: cml-escape ( -- n )
    0e 0e                       \ z = 0 + 0i  (FP)
    cml-maxiter                 \ default result: interior point
    cml-maxiter 0 do
        cml-cre f@ cml-cim f@   \ FP: z_re z_im c_re c_im
        mbrot-step              \ FP: z_re' z_im' mag2
        4.0e f<                 \ bounded? (mag2 < 4)   FP: z_re' z_im'
        0= if                   \ escaped → record iteration and stop
            drop  i  leave
        then
    loop
    fdrop fdrop                 \ discard the final z (escaped or interior)
;

\ Same 16-step palette as the MASM demo, made opaque (0xFF alpha).
: cml-colour ( n -- argb )
    dup cml-maxiter = if drop 0xFF000000 exit then
    15 and case
        0  of 0x0D1540 endof
        1  of 0x102B80 endof
        2  of 0x1558C8 endof
        3  of 0x3A8CF5 endof
        4  of 0x7DC8FF endof
        5  of 0xB8EEFF endof
        6  of 0xFFFFFF endof
        7  of 0xFFF4A8 endof
        8  of 0xFFCC57 endof
        9  of 0xFFA000 endof
        10 of 0xFF6800 endof
        11 of 0xE83800 endof
        12 of 0xAA1200 endof
        13 of 0x650000 endof
        14 of 0x280000 endof
        15 of 0x080010 endof
    endcase
    0xFF000000 or
;

: cml-render ( -- )
    cml-h 0 do
        i cml-row !
        cml-w 0 do
            i         s>d d>f  cml-dx f*  -2.5e  f+   cml-cre f!
            cml-row @ s>d d>f  cml-dy f*  -1.25e f+   cml-cim f!
            cml-escape
            cml-colour
            cml-row @ cml-w *  i +  4 *  cml-fb +  L!
        loop
    loop
;

: cml-wait ( id -- )
    begin
        dup -1 gpane-next-event
        dup ev-close = swap ev-frame-close = or
        >r  drop drop drop drop  r>
    until
    drop
;

: gfx-canvas-mandelbrot-let
    cr ." rendering " cml-w . ." x " cml-h . ." Mandelbrot (LET / LLVM) ..." cr
    cml-w cml-h  S" ∴ Mandelbrot HD (LET)"  gpane-open
    dup 0= if drop ." (no UI substrate — demo skipped)" cr exit then
    cml-render
    dup cml-fb cml-w cml-h canvas-blit       \ one bulk upload — keeps id
    ." done — close the window to exit" cr
    cml-wait
;
