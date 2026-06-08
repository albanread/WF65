\ ─────────────────────────────────────────────────────────────────────
\ WF64 single-inheritance object system — Forth layer (Phase 1).
\
\ The message-send hot path (self, (send), (send-xt)) is in
\ kernel/oop.masm.  This file is the whole defining DSL.  See
\ docs/oop_design.md for the model.
\
\   Class struct:   [ super | isize | vtable[CAP] ]   (offsets c.super/c.isize/c.vt)
\   Object layout:  [ class-ptr | ivar0 | ivar1 | ... ]
\
\ Reset-safety note (shared-session test harness): selectors live in a
\ flat name->id buffer allocated here at boot (below the reset fence), so
\ they are stable across reset().  Classes, objects and ivar words are
\ created in the FORTH wordlist, which reset() snapshots and restores.
\ (Per-class ivar-name scoping is a later refinement — see the design.)
\ ─────────────────────────────────────────────────────────────────────

\ CAP — max distinct selectors in a session; also the vtable length.
512 constant oop-cap

\ Class-struct field offsets — mirror kernel/macros.masm class_*.
\ super/isize/vt MUST match the kernel; magic/name are appended AFTER the
\ vtable so the dispatch path's class_vt offset is untouched.
0       constant c.super
1 cells constant c.isize
2 cells constant c.vt
oop-cap cells c.vt +  constant c.magic   \ tag cell: identifies a class struct
c.magic cell +        constant c.name    \ name token (nt) of the class word
c.name cell +         constant c.struct  \ total class-struct size in bytes
$C1A55C1A55  constant oop-magic          \ "CLASS" — unlikely to collide

\ ── compile-time receiver hint (drives early binding) ────────────────
\ A named object (via (comp-object)) or `super` records the receiver's
\ class and the HERE just after the receiver was compiled; `->` consumes
\ the hint when it is still fresh (rec-here = here).  Both live in the
\ user area (user_OOP_RECV_*) so reset() zeroes them — no stale
\ cross-session early-binding after HERE rewinds.
: rec-class  ( -- addr )   base $17F8 + ;
: rec-here   ( -- addr )   base $1800 + ;

\ ── selector registry (flat name->id table) ──────────────────────────
\ Each slot is 32 bytes: a counted string (len byte + up to 31 chars).
\ The selector id IS the slot index, so it indexes the vtable directly.
create sel-names  oop-cap 32 *  allot
variable #sel   0 #sel !

: sel-slot  ( i -- addr )       5 lshift  sel-names + ;     \ i * 32
: sel-name  ( i -- c-addr u )   sel-slot count ;

: (find-sel) ( c-addr u -- id true | false )
    #sel @ 0 ?do
        2dup i sel-name compare 0= if
            2drop i true unloop exit
        then
    loop  2drop false ;

: (intern-sel) ( c-addr u -- id )
    31 min                              ( a u )
    #sel @  dup 1+ #sel !  >r           ( a u )     \ id reserved on rstack
    r@ sel-slot                         ( a u slot )
    2dup c!                             ( a u slot ) \ slot[0] = length
    1+ swap                             ( a slot+1 u )
    cmove                               ( )          \ name bytes
    r> ;                                ( id )

: selector-id ( c-addr u -- id )
    2dup (find-sel) if  >r 2drop r>  else  (intern-sel)  then ;

\ ── vtable accessors ─────────────────────────────────────────────────
: vt@  ( class sel -- xt )      cells swap c.vt + + @ ;
: vt!  ( xt class sel -- )      cells swap c.vt + + ! ;

\ ── does-not-understand ──────────────────────────────────────────────
: (dnu) ( i*x -- )  -2058 throw ;

\ Fill every vtable slot of a class with one xt.
: (init-vtable) ( class xt -- )
    swap c.vt + swap                    ( &vt0 xt )
    oop-cap 0 ?do
        2dup swap i cells + !           \ vt[i] = xt
    loop  2drop ;

\ ── the root class `object` ──────────────────────────────────────────
create object
    0 ,                                 \ c.super = 0
    1 cells ,                           \ c.isize = one header cell
    oop-cap cells allot                 \ vtable storage (filled next)
    oop-magic ,                         \ c.magic = class tag
    latestxt >name ,                    \ c.name = nt of "object"
object  ' (dnu)  (init-vtable)          \ every selector -> (dnu) by default

\ ── class definition ─────────────────────────────────────────────────
variable current-class
variable saved-current

\ Per-class ivar wordlist.  ivar words are defined here and the wordlist is
\ spliced to the FRONT of the search order only while a class body compiles,
\ so ivar names are scoped to their class and invisible elsewhere (and may be
\ reused across classes — newest wins during each class's own compile).  Its
\ wid is published to the user area so reset() restores (clears) its buckets,
\ exactly like FORTH/TOOLS/PRIVATE — without that, scoped ivar entries would
\ dangle after HERE/INDEX rewind. See docs/vocabularies.md.
wordlist constant ivars-wl
ivars-wl  base $1808 +  !

: (push-front-order) ( wid -- )   >r get-order r> swap 1+ set-order ;
: (drop-front-order) ( -- )       get-order swap drop 1- set-order ;

: subclass ( parent "name" -- )
    create  here                        ( parent struct )
    c.struct  allot                     \ reserve the whole struct
    2dup c.super + !                    \ struct.super = parent
    over c.isize + @  over c.isize + !  \ struct.isize = parent.isize
    over c.vt +  over c.vt +  oop-cap cells  move   \ copy parent vtable down
    oop-magic over c.magic + !          \ struct.magic = class tag
    latestxt >name over c.name + !      \ struct.name = nt of this class
    nip                                 ( struct )
    current-class !
    get-current saved-current !          \ open the ivar scope:
    ivars-wl (push-front-order)          \   searched first during the body,
    ivars-wl set-current ;               \   ivar: defines into it

: class ( "name" -- )   object subclass ;

: end-class ( -- )
    (drop-front-order)                   \ close the ivar scope
    saved-current @ set-current
    0 current-class ! ;

\ ── instance variables ───────────────────────────────────────────────
: ivar: ( size "name" -- )
    create
        current-class @ c.isize + @  ,          \ store this field's offset
        current-class @ c.isize +  +!           \ grow class instance size
    does>  @ self + ;                           \ runtime: self + offset

\ ── methods ──────────────────────────────────────────────────────────
: :m ( "selector" -- )                          \ start / override a method
    parse-name selector-id  >r
    :noname                                     ( xt )   \ anonymous body
    current-class @  r>  vt! ;                   \ class.vtable[sel] := xt

: ;m   postpone ;  ; immediate                   \ end a method

\ ── instantiation ────────────────────────────────────────────────────
\ Compile-time action for a named object: emit the push of its address
\ (as usual) and record its class as the early-binding receiver hint.
: (comp-object) ( xt -- )
    dup compile,                                  \ emit the call that pushes obj
    >body @  rec-class !                          \ hint class = object's class
    here rec-here ! ;

: new ( class "name" -- )
    create                                       \ <name> pushes the object body
    dup ,                                         \ object[0] = class ptr
    c.isize + @  cell -                           ( ivar-bytes )
    here over erase                               \ zero the ivar region
    allot
    ['] (comp-object) compiles-me ;              \ arm early binding on this object

\ ── super ────────────────────────────────────────────────────────────
\ Inside a method, `super -> sel` sends sel to the *parent* class's method
\ with self unchanged.  It compiles a push of self and hints the parent
\ class, so the following `->` early-binds against the parent's vtable —
\ never re-entering this class's override.
: super  ( -- )                                  \ immediate; use only in :m
    postpone self                                \ receiver = self
    current-class @ c.super + @  rec-class !      \ hint = parent class
    here rec-here ! ; immediate

\ ── message send ─────────────────────────────────────────────────────
\ obj -> selector
\   Interpret state: late-bound on the object in TOS.
\   Compile state:  early-bound iff a named object / `super` was the
\                   immediately-preceding compiled item (fresh hint);
\                   otherwise late-bound (fully polymorphic).
: ->  ( "selector" -- )                          \ immediate parsing word
    parse-name selector-id                       ( sel )
    state @ if
        rec-here @ here =  rec-class @ 0<> and    ( sel early? )
        0 rec-here !                              \ consume hint (reset-safe)
        if   rec-class @ swap vt@                 ( xt )   \ parent/obj method
             postpone literal  postpone (send-xt)
        else postpone literal  postpone (send)
        then
    else
        (send)                                    \ interpret: TOS = obj
    then ; immediate

\ ── introspection ────────────────────────────────────────────────────
\ class-of ( obj -- class )   the object's class struct (cell 0).
: class-of  ( obj -- class )   @ ;

\ class? ( x -- flag )   true iff x is a class struct.  Guarded so a small
\ integer or stray pointer can't fault: x must be an address in [object, here)
\ whose tag cell (also below here) holds the class magic.  (Class structs are
\ create bodies, which are NOT cell-aligned, so no alignment check here.)
: class?  ( x -- flag )
    dup object u< if drop false exit then
    dup c.magic + here u< 0= if drop false exit then
    c.magic + @ oop-magic = ;

\ object? ( x -- flag )   true iff x is an instance (its class field points
\ to a class, and x is not itself a class).
: object?  ( x -- flag )
    dup class? if drop false exit then
    dup object u< if drop false exit then
    dup here u< 0= if drop false exit then
    @ class? ;

\ is-a? ( obj class -- flag )   true iff obj's class is `class` or a descendant
\ of it (walks the superclass chain).
: is-a?  ( obj class -- flag )
    swap class-of                        ( target cls )
    begin dup while
        2dup = if 2drop true exit then
        c.super + @
    repeat 2drop false ;

\ class-name ( class -- c-addr u )   the class's name string.
: class-name  ( class -- c-addr u )   c.name + @ count ;

\ .class ( obj -- )   print the object's class name (no trailing space).
: .class  ( obj -- )   class-of class-name type ;
