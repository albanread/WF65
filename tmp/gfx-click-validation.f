\ gfx-click.f — interactive click counter.
\
\ Opens a graphical pane and runs an event loop.  Left-click on
\ the square cycles its colour through six hues and bumps a
\ counter.  Close the pane (or the IDE frame) to exit.
\
\ Demonstrates the full Forth gpane workflow:
\   - gpane-open / -begin / draws / -present for painting
\   - gpane-next-event for input
\   - locals + IF dispatch over event kinds
\
\ See lib/core.f for the ev-* and mouse-* constants used below.
\
\ Why an IF chain instead of CASE: WF64's `endcase` emits a
\ runtime DROP that fires in the no-match path (drops the test
\ value).  Bodies that match jump past it via ENDOF→ELSE→THEN, so
\ their result survives.  But for an unhandled event kind (e.g.
\ ev-focus fires when the pane first opens) the no-match path
\ runs, and endcase's drop eats whatever the default body pushed
\ — including our `done?` flag.  Easier to skip CASE entirely
\ here and use plain IFs that never have a default-trap.

0x10131A  constant CC-BG

\ Six bright hues; (count mod 6) picks one.
: cc-colour ( n -- rgb )
    6 mod case
        0 of 0xF24C4C endof   \ red
        1 of 0xF2A632 endof   \ orange
        2 of 0xF2EB32 endof   \ yellow
        3 of 0x4CD94C endof   \ green
        4 of 0x4C8CF2 endof   \ blue
        5 of 0xCC66F2 endof   \ violet
    endcase
;

\ Paint the entire scene for the given click count.
: cc-paint ( id count -- )
    {: pane n :}
    pane dup gpane-begin
    CC-BG gpane-clear
    \ The interactive square: top-left (50,60), 200x200.
    50 60 200 200  n cc-colour  gpane-fill-rect
    gpane-present
    drop
;

\ Is (x, y) inside the square at (50,60)-(250,260)?
: cc-hit? ( x y -- ? )
    {: x y :}
    x  50 >=   x 250 <=  and
    y  60 >=   y 260 <=  and
    and
;

\ Dispatch a single event.  Returns ( id count' done? ).
\ Stack in:  ( id count p4 p3 p2 p1 kind )
\ Stack out: ( id count' done? )
\
\ For ev-mouse: p1=x, p2=y, p3=op, p4=mods|button<<8 — see
\ decode_event in src/runtime.rs.  Locals make the body read
\ naturally, no swap/rot juggling.
: cc-handle ( id count p4 p3 p2 p1 kind -- id count' done? )
    {: p4 p3 p2 p1 kind :}

    \ Exit on close or frame-close.
    kind ev-close = kind ev-frame-close = or if
        true exit
    then

    \ Repaint on resize so the rectangle fills the new area.
    kind ev-resize = if
        2dup cc-paint
    then

    \ Left-click on the square: bump counter + repaint.
    kind ev-mouse =
    p3 mouse-left-down =
    and if
        p1 p2 cc-hit? if
            1+
            2dup cc-paint
        then
    then

    \ All non-exit branches fall through here.
    false
;

: gfx-click
    cr ." opening click counter ..." cr

    480 360  S" ∴ Click Counter"  gpane-open
    dup 0= if drop ." (no UI substrate — demo skipped)" cr exit then

    0                          \ initial count
    2dup cc-paint              \ initial render

    \ Event loop — block indefinitely (-1) so we wake exactly on
    \ events for our pane (plus globals like ev-frame-close).
    begin
        over -1 gpane-next-event   \ ( id count p4 p3 p2 p1 kind )
        cc-handle                  \ ( id count' done? )
    until

    2drop
    ." done — click counter closed" cr
;

gfx-click
bye
