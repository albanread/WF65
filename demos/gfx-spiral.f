\ gfx-spiral.f — draws a spiral of small coloured circles
\
\ Tight inline loop pushing many draw commands into a single batch.
\ Integer-only — no trig: positions come from a 16-entry lookup of
\ (dx, dy) ×100 around a 16-gon, scaled by the growing radius.
\ Colours cycle through a 16-step palette.
\
\ The `0x` prefix forces hex parsing for the colour literals;
\ decimal stays the default for the geometry math.

0x0A0F18  constant BG-DARK

\ 16-step palette walking the hue ring.
: hue ( i -- rgb )
    16 mod
    case
        0  of 0xFFCC66 endof
        1  of 0xFFB266 endof
        2  of 0xFF9966 endof
        3  of 0xFF7766 endof
        4  of 0xFF66AA endof
        5  of 0xCC66FF endof
        6  of 0x9966FF endof
        7  of 0x6677FF endof
        8  of 0x66AAFF endof
        9  of 0x66DDFF endof
        10 of 0x66FFDD endof
        11 of 0x66FFAA endof
        12 of 0x77FF66 endof
        13 of 0xAAFF66 endof
        14 of 0xDDFF66 endof
        15 of 0xFFEE66 endof
    endcase
;

\ Unit-circle step on a 16-gon, components scaled by 100 so we
\ can do integer multiply-then-divide.
: dxdy ( i -- dx*100 dy*100 )
    16 mod
    case
        0  of  100    0 endof
        1  of   92   38 endof
        2  of   70   70 endof
        3  of   38   92 endof
        4  of    0  100 endof
        5  of  -38   92 endof
        6  of  -70   70 endof
        7  of  -92   38 endof
        8  of -100    0 endof
        9  of  -92  -38 endof
        10 of  -70  -70 endof
        11 of  -38  -92 endof
        12 of    0 -100 endof
        13 of   38  -92 endof
        14 of   70  -70 endof
        15 of   92  -38 endof
    endcase
;

\ Spiral position for step i — radius grows linearly with i.
\ Centre fixed at (240, 180); radius = i * 4 pixels.
: place ( i -- cx cy )
    dup dxdy            \ ( i dx dy ) — dx,dy still ×100
    rot 4 *             \ ( dx dy r ) — r in pixels
    >r                  \ ( dx dy ) — r on rstack
    r@ * 100 / 180 +    \ ( dx cy )
    swap                \ ( cy dx )
    r> * 100 / 240 +    \ ( cy cx )
    swap                \ ( cx cy )
;

: gfx-spiral
    cr ." opening graphical pane ..." cr

    480 360  S" ∴ Spiral"  gpane-open
    dup 0= if
        drop ." (no UI substrate — demo skipped)" cr exit
    then

    dup gpane-begin
    BG-DARK gpane-clear

    \ 48 small circles arranged in an outward spiral.
    48 0 ?do
        i place         \ ( cx cy )
        12              \ radius
        i hue           \ rgb
        gpane-fill-circle
    loop

    gpane-present
    drop

    ." done — see the Spiral window" cr
;
