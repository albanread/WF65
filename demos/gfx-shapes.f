\ gfx-shapes.f — opens a graphical pane and draws a static scene
\
\ Demonstrates the iGui graphics API:
\   gpane-open          ( w h c-addr u -- id )
\   gpane-begin         ( id -- )
\   gpane-clear         ( rgb -- )
\   gpane-fill-rect     ( x y w h rgb -- )
\   gpane-stroke-rect   ( x y w h thick rgb -- )
\   gpane-line          ( x0 y0 x1 y1 thick rgb -- )
\   gpane-fill-circle   ( cx cy r rgb -- )
\   gpane-present       ( -- )
\
\ Colours pack as 24-bit RGB into one cell.  The `0x` / `0X`
\ prefix forces hex parsing regardless of BASE, so colour
\ constants read naturally.  Coordinates stay plain decimal.

0x101418  constant BG
0xFFCC66  constant AMBER
0x4D9BF5  constant SKY
0xF07178  constant SALMON
0x76C893  constant MINT
0xE0E4E8  constant CREAM

: gfx-shapes
    cr ." opening graphical pane ..." cr

    \ Open a 480 x 360 window titled "Shapes".
    480 360  S" ∴ Shapes"  gpane-open
    dup 0= if
        drop ." (no UI substrate — demo skipped)" cr exit
    then

    \ Begin a draw batch for the new pane.
    dup gpane-begin

    \ Dark background.
    BG gpane-clear

    \ Three filled rectangles, evenly spaced.
    30 60 100 80   AMBER   gpane-fill-rect
    190 60 100 80  SKY     gpane-fill-rect
    350 60 100 80  SALMON  gpane-fill-rect

    \ Stroked outline around the trio.
    20 50 440 100 3  CREAM  gpane-stroke-rect

    \ A line crossing the lower half.
    20 270  460 220  4  MINT  gpane-line

    \ Three circles of different sizes.
    100 220 30  AMBER   gpane-fill-circle
    240 240 50  SKY     gpane-fill-circle
    380 220 30  SALMON  gpane-fill-circle

    \ Commit the batch — paints next frame.
    gpane-present
    drop  \ drop the pane id we kept on the stack

    ." done — see the Shapes window" cr
;
