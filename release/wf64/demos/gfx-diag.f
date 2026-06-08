\ gfx-diag.f — minimal gpane diagnostic.
\
\ Opens a pane, paints once, waits for ONE close event, exits.
\ No locals, no case dispatch, no rect.  If the pane shows up
\ with a circle inside, the basic gpane wiring works.  If not,
\ open/begin/clear/fill-circle/present is broken.

: gfx-diag
    cr ." gfx-diag: opening pane" cr

    320 240 S" ∴ Diag" gpane-open
    dup 0= if drop ." no UI" cr exit then

    cr ." gfx-diag: pane id = " dup . cr

    dup gpane-begin
    0x202020 gpane-clear
    160 120 60 0xFFCC66 gpane-fill-circle
    gpane-present

    cr ." gfx-diag: painted, entering event loop" cr

    \ Loop until close.  No more drawing.
    begin
        dup -1 gpane-next-event   \ ( id p4 p3 p2 p1 kind )
        \ Print the kind we got so we can see in the console.
        ." [diag] event kind = " dup . cr
        dup ev-close = swap ev-frame-close = or
        \ Stack now: ( id p4 p3 p2 p1 done? )
        \ Drop the 4 params, leave done? on top.
        >r drop drop drop drop r>
    until

    drop
    ." gfx-diag: done" cr
;
