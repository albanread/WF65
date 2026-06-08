\ ── WF64 object-system showcase: a small shape hierarchy ──────────────
\ Demonstrates class / ivar: / :m / new / -> , inheritance & override,
\ polymorphic area, late binding through a variable, and the
\ introspection words (.class / is-a? / object? / class?).
\
\ Run it:   include demos/oop-shapes.f

decimal

class shape
  cell ivar: cx   cell ivar: cy
  :m at        ( x y -- )    cy !  cx ! ;m
  :m moveby    ( dx dy -- )  cy @ +  cy !   cx @ +  cx ! ;m
  :m area      ( -- n )      0 ;m
  :m describe  ( -- )
       ." a " self .class space ." at " cx @ . cy @ . ." area=" self -> area . cr ;m
end-class

shape subclass circle
  cell ivar: r
  :m r!     ( n -- )   r ! ;m
  :m area   ( -- n )   r @ dup * 3 * ;m         \ pi r^2 (pi rounded to 3)
end-class

shape subclass rect
  cell ivar: w   cell ivar: h
  :m w!     ( n -- )   w ! ;m
  :m h!     ( n -- )   h ! ;m
  :m area   ( -- n )   w @ h @ * ;m
end-class

circle new c1
rect   new r1

3 4 c1 -> at    10 c1 -> r!
0 0 r1 -> at     5 r1 -> w!    2 r1 -> h!

cr ." === each shape describes itself (polymorphic area) ===" cr
c1 -> describe
r1 -> describe

\ Late binding through a variable: the SAME code dispatches on whatever
\ object the variable holds at run time.
variable cur
: show-current   cur @ -> describe ;

cr ." === late-bound dispatch through a variable ===" cr
c1 cur !   show-current
r1 cur !   show-current

cr ." === introspection ===" cr
." c1 is-a circle? "  c1 circle is-a? .  cr
." c1 is-a shape?  "  c1 shape  is-a? .  cr
." r1 is-a circle? "  r1 circle is-a? .  cr
." c1 object?      "  c1 object? .  cr
." circle class?   "  circle class? .  cr

cr ." done." cr
