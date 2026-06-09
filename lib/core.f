\ Stable source-defined words loaded at startup.

 
: bl 32 ;               ( -- c )
: space bl emit ;       ( -- )
: spaces                ( n -- )
	0max begin dup
	while bl emit 1-
	repeat drop ;

: true -1 ;
: false 0 ;

: environment? ( c-addr u -- false ) 2drop false ;

: c, here c! 1 chars allot ;
: , here ! 1 cells allot ;
: 2, here 2! 2 cells allot ;
: align here aligned here - allot ;
: compiles ( xt1 xt2 -- ) >comp ! ;
: compiles-me ( xt -- ) latestxt compiles ;
: variable create 0 , ;
: 2variable create 0 , 0 , ;

variable hld

: ud/mod ( ud1 u1 -- u2 ud2 )
	over 0=
	if
		um/mod 0
	else
		dup >r 0 swap
		um/mod
		r> swap >r
		um/mod r>
	then ;

: <# pad 256 + hld ! ;

: hold ( char -- ) -1 hld +! hld @ c! ;

: holds ( c-addr u -- )
	begin dup
	while 1- 2dup + c@ hold
	repeat 2drop ;

: # ( ud1 -- ud2 )
	base @ ud/mod rot
	dup 9 > if 7 + then
	48 + hold ;

: #s begin # 2dup or 0= until ;

: sign ( n -- ) 0< if 45 hold then ;

: #> ( xd -- c-addr u ) 2drop hld @ pad 256 + over - ;

: u. ( u -- ) 0 <# #s #> type space ;

\ Redefine . to use pictured-numeric output so it respects BASE.
: . ( n -- )
    dup 0< >r abs 0 <# #s r> sign #> type space ;

: d.  ( d -- )
    dup >r dabs <# #s r> sign #> type space ;

: ud. ( ud -- ) <# #s #> type space ;

: du. ( du -- ) ud. ;

: erase ( addr u -- ) 0 fill ;

: f, here f! 1 floats allot ;
: fvariable create 1 floats allot ;
 
: (comp-cons) ( xt -- ) >body postpone literal ;
 
: constant create , does> @ ;
 
' (comp-cons) compiles-me

: (comp-2cons) ( xt -- ) >body postpone literal postpone 2@ ;

: 2constant create 2, does> 2@ ;
' (comp-2cons) compiles-me

: (comp-fconst) ( xt -- ) >body postpone literal postpone f@ ;

: fconstant create f, does> f@ ;

' (comp-fconst) compiles-me
 
: (comp-val) ( xt -- ) >body postpone literal postpone @ ;
 
: value create , does> @ ;
 
' (comp-val) compiles-me
 
: defer@ ( xt -- xt' ) dup >name tfa@ 145 = if 24 + @ else drop -31 throw then ;
 
: defer! ( xt' xt -- ) dup >name tfa@ 145 = if 24 + ! else drop -31 throw then ;
 
: defer-err -261 throw ;
 
: defer create ['] defer-err , does> @ execute ;

: char parse-name dup 0= if drop throw_namereqd throw then drop c@ ;

: [char] char postpone literal ; immediate

: 2literal postpone swap postpone literal postpone literal ; immediate

: case 0 ; immediate

: of postpone over postpone = postpone if postpone drop ; immediate

: endof postpone else ; immediate

: endcase postpone drop begin ?dup while postpone then repeat ; immediate

: find ( c-addr -- c-addr 0 | xt 1 | xt -1 )
	dup count find-name if
		nip dup name>compile nip ['] execute =
		if name>interpret 1 else name>interpret -1 then
	else
		2drop 0
	then ;
 

\ ANS Forth Core / Core-ext words

tools-wordlist set-current
: ?  @ . ;
forth-wordlist set-current

: buffer: create allot ;

: noop ;

\ TO: parse next name; in interpret state store, in compile state compile a store.
: to
    parse-name
    state @ locals# and if          \ inside a compiled definition with active locals?
        2dup check-local-store       \ try locals table first
        if exit then                 \ found: inline store emitted; done
    then
    find-name dup 0= if drop throw_namereqd throw then
    drop
    name>interpret >body
    state @ if postpone literal ['] ! compile, else ! then
; immediate

: action-of
    parse-name find-name dup 0= if drop throw_namereqd throw then
    drop name>interpret defer@
    state @ if postpone literal else then
; immediate

: is
    parse-name find-name dup 0= if drop throw_namereqd throw then
    drop name>interpret
    state @ if postpone literal ['] defer! compile, else defer! then
; immediate

\ Number output with field width (right-justified)
: .r  ( n width -- )
    >r dup abs 0 <# #s rot sign #> r> over - 0 max spaces type ;
: u.r  ( u width -- )
    >r 0 <# #s #> r> over - 0 max spaces type ;
: d.r  ( d width -- )
    >r dup >r dabs <# #s r> sign #> r> over - 0 max spaces type ;

\ File loading — M6
\ included ( c-addr u -- )
\   Read the named file into a Rust-owned buffer, evaluate it through the
\   normal source pipeline (saving/restoring SOURCE context), then release
\   the buffer.  Nested includes are safe because rt_slurp_file uses a stack.
: included  ( c-addr u -- )
    rt-slurp-file dup 0= if drop -37 throw then
    rt-slurp-len
    ['] evaluate catch
    rt-slurp-pop
    throw ;

: include  parse-name included ;

\ Source-level tools
: .( 41 parse type ; immediate

: square dup * ;        ( n -- n^2 )
: cube dup dup * * ;   ( n -- n^3 )
: quad square square ; ( n -- n^4 )
: sixth cube square ;  ( n -- n^6 )

\ ── String utilities ───────────────────────────────────────────────────────
: -trailing  ( c-addr u -- c-addr u' )
    begin
        dup if 2dup + 1- c@ bl = else 0 then
    while
        1-
    repeat ;

\ ── REPLACES / SUBSTITUTE (Forth 2012 String-Ext) ──────────────────────────
\ A small variable-substitution facility. REPLACES binds a name to a value
\ string; SUBSTITUTE walks a source string and expands %name% references
\ into a user-supplied destination buffer.

\ ── SUBSTITUTE / REPLACES internal state (PRIVATE) ───────────────────────
private-wordlist set-current

16 constant subst-max
create subst-table subst-max 4 cells * allot
variable subst-count
create subst-heap 2048 allot
variable subst-here

: subst-init   subst-heap subst-here !  0 subst-count ! ;
subst-init

: subst-slot  ( i -- slot )      4 cells * subst-table + ;
: subst-name  ( slot -- a u )    dup @ swap cell+ @ ;
: subst-val   ( slot -- a u )    dup 2 cells + @ swap 3 cells + @ ;

\ Copy a transient string into the substitution heap.
: subst-alloc  ( c-addr u -- dst u )
    >r  subst-here @  2dup r@ cmove  nip
    r@ subst-here +!  r> ;

\ Find a name in the substitution table.
: subst-find  ( c-addr u -- idx true | false )
    subst-count @ 0 ?do
        2dup i subst-slot subst-name compare 0= if
            2drop i true unloop exit
        then
    loop  2drop false ;

\ User-facing REPLACES / SUBSTITUTE return to FORTH:
forth-wordlist set-current

\ REPLACES ( c-addr1 u1 c-addr2 u2 -- )
\ Bind name (c-addr2 u2) to value (c-addr1 u1).
: replaces  ( v-addr v-len n-addr n-len -- )
    2dup subst-find if
        \ Existing slot: just rewrite value.
        >r 2drop                       \ drop name      ( r: idx )
        subst-alloc                    \ copy value
        r> subst-slot >r
        r@ 3 cells + !  r> 2 cells + !
    else
        subst-count @ subst-max < if
            subst-count @ subst-slot >r
            1 subst-count +!
            subst-alloc                \ copy name
            r@ cell+ !  r@ !
            subst-alloc                \ copy value
            r@ 3 cells + !  r> 2 cells + !
        else
            2drop 2drop
        then
    then ;

\ SUBSTITUTE state (variables keep stack juggling sane) -- PRIVATE.
private-wordlist set-current
variable sub-src    variable sub-srclen
variable sub-dst    variable sub-dstmax    variable sub-dstlen
variable sub-count

: sub-emit  ( ch -- )
    sub-dstlen @ sub-dstmax @ < if
        sub-dst @ sub-dstlen @ + c!
        1 sub-dstlen +!
    else drop then ;

: sub-emit-str  ( c-addr u -- )
    bounds ?do i c@ sub-emit loop ;

: sub-advance  ( -- )   1 sub-src +!   -1 sub-srclen +! ;
: sub-peek     ( -- ch )  sub-src @ c@ ;
forth-wordlist set-current

\ SUBSTITUTE ( c-addr1 u1 c-addr2 u2 -- c-addr2 u3 n )
\ Copy c-addr1/u1 to c-addr2/u2 expanding %name% (and %% → %).
\ Returns the destination buffer, the produced length, and the
\ count of successful substitutions.
: substitute  ( c-addr1 u1 c-addr2 u2 -- c-addr2 u3 n )
    sub-dstmax !  sub-dst !  sub-srclen !  sub-src !
    0 sub-dstlen !  0 sub-count !
    begin sub-srclen @ while
        sub-peek [char] % <> if
            sub-peek sub-emit  sub-advance
        else
            sub-advance
            sub-srclen @ 0= if
                [char] % sub-emit
            else sub-peek [char] % = if
                [char] % sub-emit  sub-advance
            else
                sub-src @                              \ remember name start
                begin
                    sub-srclen @ if sub-peek [char] % <> else 0 then
                while
                    sub-advance
                repeat
                sub-src @ over -                       \ ( name-addr name-len )
                sub-srclen @ if sub-advance then       \ consume closing %
                2dup subst-find if
                    >r 2drop  r> subst-slot subst-val
                    sub-emit-str
                    1 sub-count +!
                else
                    [char] % sub-emit
                    sub-emit-str
                    [char] % sub-emit
                then
            then then
        then
    repeat
    sub-dst @ sub-dstlen @ sub-count @ ;

\ ── Double-cell additions ────────────────────────────────────────────────

\ D>S ( d -- n )    discard the high cell.
: d>s   ( d -- n )  drop ;

\ Missing double-cell comparisons.
: d<=   ( d1 d2 -- flag )  d>  0= ;
: d>=   ( d1 d2 -- flag )  d<  0= ;
: du<=  ( d1 d2 -- flag )  du> 0= ;
: du>=  ( d1 d2 -- flag )  du< 0= ;

\ Double-cell zero comparisons (D0< D0= exist as primitives).
: d0<>  ( d -- flag )  d0= 0= ;
: d0>=  ( d -- flag )  d0< 0= ;
: d0<=  ( d -- flag )  2dup d0= -rot d0< or ;
: d0>   ( d -- flag )  d0<= 0= ;

\ UD.R ( ud width -- )    Print unsigned double right-justified.
: ud.r  ( ud width -- )
    >r <# #s #> r> over - 0 max spaces type ;

\ ── Misc convenience ─────────────────────────────────────────────────────

\ UNLESS — inverse of IF (immediate).
: unless  postpone 0= postpone if ; immediate

\ THIRD ( a b c -- a b c a )
: third  >r over r> swap ;

\ ?: ( flag a b -- a-or-b )    pick a if flag, b otherwise.
: ?:  ( flag a b -- result )  rot if drop else nip then ;

\ ASSERT ( flag c-addr u -- )    abort with message if flag is false.
: assert  ( flag c-addr u -- )
    rot 0= if type cr abort else 2drop then ;

\ ── F. — print a float in fixed-point notation ────────────────────────────
\ Prints sign, integer part, '.', then 6 fractional digits, then a space.
: f.  ( F: r -- )
    fdup f0< if [char] - emit fnegate then
    fdup f>d
    <# #s #> type
    [char] . emit
    fdup f>d d>f f-
    1000000 s>d d>f f*
    f>d drop
    0 <# # # # # # # #> type
    space ;

\ ── READ-LINE (Forth 2012 File-Access) ───────────────────────────────────
\ Read a line up to LF; strip a trailing CR if present.  Byte-at-a-time
\ implementation (slow but correct); a chunked version would need
\ SetFilePointerEx to push back overrun.

\ READ-LINE state is PRIVATE; READ-LINE itself is FORTH (defined just below).
private-wordlist set-current
variable rl-fid
variable rl-buf
variable rl-max
variable rl-pos
variable rl-ior
variable rl-state                 \ 0=continue 1=EOF-no-data 2=clean-end 3=error
forth-wordlist set-current

: read-line  ( c-addr u1 fileid -- u2 flag ior )
    rl-fid !  rl-max !  rl-buf !
    0 rl-pos !  0 rl-ior !  0 rl-state !
    begin rl-state @ 0= while
        rl-pos @ rl-max @ >= if
            2 rl-state !
        else
            rl-buf @ rl-pos @ + 1 rl-fid @ read-file
            rl-ior !                                ( bytes-read )
            rl-ior @ 0= if
                dup 0= if
                    rl-pos @ 0= if 1 else 2 then rl-state !
                else
                    rl-buf @ rl-pos @ + c@           ( bytes byte )
                    case
                        10 of
                            rl-pos @ 0> if
                                rl-buf @ rl-pos @ + 1- c@ 13 =
                                if -1 rl-pos +! then
                            then
                            2 rl-state !
                        endof
                        1 rl-pos +!
                    endcase
                then
                drop
            else
                drop  3 rl-state !
            then
        then
    repeat
    rl-pos @
    rl-state @ 1 = if 0 else -1 then
    rl-ior @ ;

\ ── Locals: {: x y z :} ─────────────────────────────────────────────────
\ Forth 2012 locals.  Usage:
\     : foo ( a b c -- result )
\         {: x y z :}
\         x y z + + ;
\
\ At run time `c` goes into z (highest offset), `b` into y, `a` into x.
\ TO localname writes to a local; uninitialized locals via the | section.
\
\ Design: locals are compile-time-only -- they never become dictionary
\ entries.  Each local name + byte-offset is recorded in a 16-slot
\ table in the user area (user_LOCALS_TABLE at offset $15C8).  While
\ user_LOCALS_COUNT > 0, the kernel's interpret_source loop checks
\ that table before find-name; on a match it emits a 15-byte inline
\ fetch sequence directly into foo's body.  No CALL overhead, no
\ per-local headers, no garbage left over after ;.
\
\ The kernel's ; and EXIT auto-emit `add r15, N*cell` before the RET
\ based on user_LOCALS_COUNT, then ; zeros that count.

\ Scratch helpers for the locals table -- PRIVATE.  The user-facing
\ {: word itself stays in FORTH (defined just below).
private-wordlist set-current

: locals-table  ( -- addr )       base $15C8 + ;
: locals-slot   ( idx -- addr )   5 lshift  locals-table + ;

variable ls-idx
variable ls-addr
variable ls-len

\ Record a single local in the table.  Each slot is 32 bytes:
\   +0       length (1 byte, capped at 15)
\   +1..+15  name bytes (15 bytes, zero-padded — we erase first
\            so a longer local from a previous definition can't
\            leave stale bytes that a future stricter compare
\            would mistakenly match)
\   +16..+23 byte-offset within the locals frame (8 bytes)
: locals-set  ( idx c-addr u -- )
    ls-len !  ls-addr !  ls-idx !
    ls-len @ 15 > if 15 ls-len ! then
    ls-idx @ locals-slot                       ( slot )
    \ Zero the name area (1 byte length + 15 bytes name) before
    \ writing — see header comment.  Without this a longer local
    \ from a previous definition leaves stale bytes that a future
    \ stricter compare could mistakenly match.
    dup 16 erase
    ls-len @ over c!                            \ length byte
    1+  ls-addr @ swap  ls-len @ cmove          \ name bytes
    ls-idx @ cells  ls-idx @ locals-slot 16 + ! ;  \ byte-offset

: locals-closer?  ( c-addr u -- flag )  s" :}" str= ;
: locals-pipe?    ( c-addr u -- flag )  1 = swap c@ [char] | = and ;
: locals-arrow?   ( c-addr u -- flag )  s" --" str= ;
: locals-skip-to-end ( -- )
    begin parse-name locals-closer? until ;

forth-wordlist set-current

: {:
    0 0 0                               \ ( n-init after-pipe n-total )
    begin
        parse-name dup 0= if
            2drop refill drop false
        else
            2dup locals-closer? if          \ :} — done
                2drop true
            else 2dup locals-pipe? if       \ | — subsequent locals are uninitialized
                2drop
                swap drop -1 swap           \ set after-pipe flag
                false
            else 2dup locals-arrow? if      \ -- starts output-comment; skip to :}
                2drop locals-skip-to-end
                true
            else
                ( n-init after-pipe n-total c-addr u )
                \ Overflow guard: 16 slots (idx 0..15) before user_TOOLS_WID.
                2 pick 15 > if
                    2drop  -29 throw
                then
                2 pick -rot locals-set      \ register at slot index n-total
                1+                          \ n-total++
                over 0= if                  \ if not after-pipe: n-init++ too
                    rot 1+ -rot
                then
                false
            then then then
        then
    until
    ( n-init after-pipe n-total )

    nip                                     \ drop after-pipe → ( n-init n-total )
    dup locals#!                            \ tell ; how many slots to release

    \ Compile n-total (open-locals) — allocates all slots including uninitialized.
    dup postpone literal postpone (open-locals)

    \ Compile (local!) only for n-init stack-initialized locals.
    swap                                    \ ( n-total n-init )
    dup 0 ?do
        dup 1- i - cells postpone literal postpone (local!)
    loop
    2drop ; immediate

\ ── Console / terminal (Facility) ─────────────────────────────────────────
\ Implemented via ANSI escape sequences. Modern Windows terminals
\ (Windows Terminal, conhost on Windows 10+) handle these by default.

\ AT-XY  ( col row -- )  is provided by the kernel as a primitive,
\ same reasoning as PAGE above.  The old colon-def here emitted
\ ANSI ESC[r;cH cursor-position sequences which become visible
\ junk in wf64-ui's console; the kernel primitive instead records
\ the requested (col, row) for the next emit (V1 — still partial;
\ needs streaming-emit IO to fully take effect).

\ PAGE  ( -- )  is provided by the kernel as a primitive — it
\ dispatches to the UI front-end (wf64-ui clears its console
\ scrollback) or is a no-op when running headless.  The Lisp/CL
\ heritage of this core library had a Forth-side fallback that
\ emitted ANSI ESC[2J / ESC[H sequences here, but those just
\ appear as garbage in wf64-ui's text buffer.  If you need ANSI
\ behaviour at a real terminal, redefine PAGE-ANSI yourself —
\ we deliberately leave the standard PAGE wired to the kernel.

\ ── File-Access constants ─────────────────────────────────────────────────
\ R/O W/O R/W BIN are the file-access-method constants used by OPEN-FILE
\ and CREATE-FILE. Values are implementation-defined; the kernel
\ primitives translate them to Win32 access masks.
1 constant r/o
2 constant w/o
3 constant r/w
: bin   ( fam -- fam' )  ;        \ no-op on Windows (binary mode is implicit)

\ ── Dictionary tools ──────────────────────────────────────────────────────

\ FORGET ( "name" -- )    Roll back the dictionary past the named word
\ by calling forget_last repeatedly until the name is no longer findable.
\ Honors the kernel FORGET_FENCE (forget_last is a no-op past it).
tools-wordlist set-current
: forget  ( "name" -- )
    parse-name 2dup find-name
    0= if 2drop 2drop -13 throw then
    drop                            ( c-addr u )
    begin
        2dup find-name
        0= if
            2drop -1                ( c-addr u -1 ) — stop
        else
            drop forget_last 0      ( c-addr u 0 ) — continue
        then
    until
    2drop ;

\ ORDER ( -- )    Display the current search order and current wordlist.
: order  ( -- )
    s" Search order:" type
    get-order
    dup 0 ?do  space dup pick u.  loop drop
    cr
    s" Current: " type  get-current u.  cr ;
forth-wordlist set-current

\ UNDER+ ( n1 n2 n3 -- n1+n3 n2 )    Add n3 to n1 leaving n2 unchanged on top.
: under+  ( n1 n2 n3 -- n1+n3 n2 )  rot + swap ;

\ ── 2VALUE / 2TO (Forth 2012, double-cell value) ─────────────────────────
\ Note: this pair is independent of TO — use 2TO for 2VALUE writes.

: 2value   ( x1 x2 "name" -- )  create , , does> 2@ ;

: 2to
    parse-name find-name dup 0= if drop -13 throw then
    drop name>interpret >body
    state @ if postpone literal ['] 2! compile, else 2! then
; immediate

\ ── String helpers ───────────────────────────────────────────────────────

\ -LEADING ( c-addr u -- c-addr' u' )   strip leading spaces (mirror of -trailing).
: -leading  ( c-addr u -- c-addr' u' )
    begin
        dup if over c@ bl = else 0 then
    while
        1 /string
    repeat ;

\ STARTS-WITH?  ( c-addr u prefix-addr prefix-u -- flag )
\ True iff the string at c-addr/u begins with the prefix.
: starts-with?  ( c-addr u prefix-addr prefix-u -- flag )
    rot over <
    if
        2drop drop 0
    else
        tuck compare 0=
    then ;

\ ENDS-WITH? ( c-addr u suffix-addr suffix-u -- flag )
private-wordlist set-current
variable ew-suffix-u
variable ew-suffix-addr
forth-wordlist set-current

: ends-with?  ( c-addr u suffix-addr suffix-u -- flag )
    ew-suffix-u !   ew-suffix-addr !
    \ ( c-addr u )
    dup ew-suffix-u @ <
    if
        2drop 0
    else
        ew-suffix-u @ - +                            \ tail-addr = c + (u - suffix-u)
        ew-suffix-u @
        ew-suffix-addr @ ew-suffix-u @
        compare 0=
    then ;

\ CONTAINS? ( c-addr u substr-addr substr-u -- flag )   substring present?
: contains?  ( c-addr u substr-addr substr-u -- flag )
    search nip nip ;

\ ── Floating-point helpers ────────────────────────────────────────────────
\ Built on the kernel primitives:  F< F0< F0= FNEGATE F+ F- F* F/ FDUP FSWAP FOVER FDROP

: fabs   ( F: r -- |r| )           fdup f0< if fnegate then ;
: fmax   ( F: r1 r2 -- max )       fover fover f< if fswap then fdrop ;
: fmin   ( F: r1 r2 -- min )       fover fover f< 0= if fswap then fdrop ;

: f=     ( F: r1 r2 -- ; -- flag )  f- f0= ;
: f<>    ( F: r1 r2 -- ; -- flag )  f- f0= 0= ;
: f>     ( F: r1 r2 -- ; -- flag )  fswap f< ;
: f<=    ( F: r1 r2 -- ; -- flag )  fswap f< 0= ;
: f>=    ( F: r1 r2 -- ; -- flag )  f< 0= ;

\ F2* / F2/ — double / halve a float.
: f2*    ( F: r -- 2r )    2e f* ;
: f2/    ( F: r -- r/2 )   2e f/ ;

\ FTRUNC — truncate toward zero via the double-cell integer conversion.
: ftrunc ( F: r -- r' )    f>d d>f ;

\ A 0.5 constant for rounding (computed once at load time).
1e 2e f/ fconstant 0.5e

\ FROUND — round to nearest, ties away from zero.
: fround ( F: r -- r' )
    fdup f0<
    if 0.5e f- ftrunc
    else 0.5e f+ ftrunc
    then ;

\ FLOOR — round toward negative infinity.
: floor ( F: r -- r' )
    fdup ftrunc                                \ ( r trunc )
    fswap fover                                \ ( trunc r trunc )
    f<                                          \ ( trunc ; flag = r < trunc )
    if 1e f- then ;

\ FALOG ( F: r -- 10^r )    Base-10 antilog (Float-ext).
: falog  ( F: r -- 10^r )  10e fswap f** ;

\ F0<> ( F: r -- ; -- flag )
: f0<>   ( F: r -- ; flag )  f0= 0= ;

\ COMPILE-NAME ( c-addr u -- )   compile a call to the named word, or throw -13.
: compile-name  ( c-addr u -- )
    find-name 0= if -13 throw then
    name>compile execute ;

\ INLINE ( -- )    Declarator used right after a colon definition's ;.
\ Measures the body bytes (excluding the trailing RET that ; emitted)
\ and stores the length in the header's dh_ofa field (u16). Sets the
\ word's compile-action to (inline,), so subsequent references to the
\ word copy the body bytes inline instead of emitting a CALL.
\
\ Usage:
\     : square dup * ; inline
\
\ Only LEAF words are inlined.  A body that contains a relative CALL
\ (0xE8) or JMP (0xE9) cannot be copied verbatim — the rel32 displacement
\ is position-relative, so the copy would branch to the wrong address and
\ crash.  `inline` now DETECTS such bodies and refuses (leaves dh_ofa = 0),
\ so the word falls back to a normal CALL.  This makes `inline` safe to
\ apply to any word: leaf bodies are inlined, non-leaf bodies aren't.
\
\ user_LATEST is at base+16; dh_xtptr at header+16; dh_ofa at header+42;
\ dh_comp at header+24.

\ (inline-leaf?) ( xt len -- flag )
\ True iff the body [xt, xt+len) contains no relative CALL/JMP byte, i.e.
\ it is safe to copy verbatim.  Heuristic but conservative: a spurious
\ 0xE8/0xE9 inside an immediate only costs a missed inline (safe CALL).
: (inline-leaf?)  ( xt len -- flag )
    over + swap                          ( end xt )
    begin 2dup u> while                  ( end xt )
        dup c@  dup $E8 = swap $E9 = or
        if 2drop false exit then
        1+
    repeat 2drop true ;

: inline  ( -- )
    base 16 + @                              ( latest )
    here over 16 + @ - 1-                    ( latest length )  \ HERE - xt - 1
    over 16 + @ over (inline-leaf?) 0= if    ( latest length )  \ non-leaf?
        drop 0                               ( latest 0 )        \ refuse -> CALL
    then
    dup $FFFF u> if drop 0 then              ( latest length )  \ clamp; 0 == disable
    over 42 + w!                              ( latest )          \ store dh_ofa
    ['] (inline,) swap 24 + ! ;              ( )                 \ store dh_comp

\ hotvariable  ( "name" -- )   Define a variable whose REFERENCES compile to an
\ inline push of its body address (mov [rbp-8],rax; sub rbp,8; mov rax,imm64)
\ instead of a CALL into the create stub.  Opt-in: use for variables that are
\ hot in an inner loop.  Costs ~13 bytes more than a CALL per reference, so an
\ untagged `variable` stays the cheaper CALL — the human picks which are hot.
: hotvariable  ( "name" -- )
    create 0 ,                               \ same body cell as `variable`
    base 16 + @  ['] (inline-var,) swap 24 + ! ;   \ latest dh_comp := (inline-var,)

\ >FLOAT — string to float. Built on the kernel's float? primitive.
\ float? returns ( -1 ) on success (consumed addr/u, pushed r onto FP),
\ or ( c-addr u 0 ) on failure.
: >float ( c-addr u -- flag ; F: -- r | )
    float?
    if -1 else 2drop 0 then ;

\ ── Input-source manipulation ─────────────────────────────────────────────
\ Direct user-area access (base = UP):
\   user_SOURCE_ID   = 0x28 (40)
\   user_SOURCE_ADDR = 0x30 (48)
\   user_SOURCE_LEN  = 0x38 (56)
\   user_TO_IN       = 0x40 (64) — also reachable via >in
\   user_PARSE_BARRIER = 0x48 (72) — one-shot same-source rewind guard

\ EXECUTE-PARSING ( i*x c-addr u xt -- j*x )
\ Make c-addr/u the current input source and execute xt; restore source on
\ return.  Saves source state on the return stack so it survives across calls.
: execute-parsing
    base 40 + @  >r
    base 48 + @  >r
    base 56 + @  >r
    >in @        >r
    -rot                        ( xt c-addr u )
    base 56 + !
    base 48 + !
    -1 base 40 + !
    0 >in !
    execute
    r> >in !
    r> base 56 + !
    r> base 48 + !
    r> base 40 + ! ;

\ SAVE-INPUT ( -- xn ... x1 n )
\ Implementation: 4 cells (source-id, source-addr, source-len, >in) + count.
: save-input
    base 40 + @
    base 48 + @
    base 56 + @
    >in @
    4 ;

\ RESTORE-INPUT ( xn ... x1 n -- flag )
\ Returns 0 on success.
: restore-input
    >in @ base 72 + !
    drop
    >in !
    base 56 + !
    base 48 + !
    base 40 + !
    0 ;

\ NAME>STRING ( nt -- c-addr u )
\ The name token is the address of a counted name; expose it as (addr len).
: name>string  ( nt -- c-addr u )  count ;

\ ── S\" — escaped string literal (Forth 2012 Core-ext) ───────────────────
\ Recognized escapes: \n \r \t \\ \" \0 \a \b \e \f \l \q \v \xHH
\ Interpret mode: leaves (addr len) pointing into a static 256-byte buffer.
\ Compile mode:  embeds the processed bytes inline via SLITERAL.

\ Scratch buffer + helper words are PRIVATE; s\" itself is FORTH.
private-wordlist set-current
256 buffer: s-q-buf

: s-q-getch  ( -- ch )
    source drop >in @ + c@   1 >in +! ;

: s-q-hex-digit  ( ch -- n )
    dup [char] 0 [char] 9 1+ within if [char] 0 -
    else
        32 or
        dup [char] a [char] f 1+ within if [char] a - 10 +
        else drop 0 then
    then ;

: s-q-escape  ( -- ch )
    s-q-getch
    case
        [char] n of 10 endof
        [char] r of 13 endof
        [char] t of  9 endof
        92        of 92 endof         ( '\' — written as literal since \ starts a line comment )
        [char] " of 34 endof
        [char] 0 of  0 endof
        [char] a of  7 endof
        [char] b of  8 endof
        [char] e of 27 endof
        [char] f of 12 endof
        [char] l of 10 endof
        [char] q of 34 endof
        [char] v of 11 endof
        [char] x of
            s-q-getch s-q-hex-digit 16 *
            s-q-getch s-q-hex-digit +
        endof
        dup
    endcase ;

forth-wordlist set-current
: s\"
    s-q-buf 0                            ( dst count )
    begin
        s-q-getch
        dup [char] " <>
    while
        dup 92 = if drop s-q-escape then     ( 92 = '\' literal )
        2 pick over + c!
        1+
    repeat
    drop
    state @ if sliteral then ; immediate

\ ── SYNONYM ───────────────────────────────────────────────────────────────
\ Define newname so executing it executes oldname. (Immediate-flag of the
\ original is NOT propagated — synonyms are non-immediate.)
: synonym  ( "newname" "oldname" -- )
    create
        parse-name find-name 0= if -13 throw then
        name>interpret ,
    does> @ execute ;

\ ── Forth 2012 Structures (BEGIN-STRUCTURE / FIELD: / +FIELD / ...) ───────
\ Usage:
\   begin-structure point
\     field: .x
\     field: .y
\   end-structure
\   point     \ -> total size (16)
\   create p  point allot
\   42 p .x !  p .x @   \ -> 42

: begin-structure  ( "name" -- addr 0 )
    create  here 0 ,                 \ allocate size cell, leave its addr
    0                                 \ initial offset
    does> @ ;

: end-structure  ( addr size -- )    swap ! ;

: +field   ( n1 n2 "name" -- n3 )
    create  over , +
    does> @ + ;

: field:   ( n1 "name" -- n2 )  aligned 1 cells +field ;
: cfield:  ( n1 "name" -- n2 )  1 +field ;
: 2field:  ( n1 "name" -- n2 )  aligned 2 cells +field ;

\ ── Limit constants ───────────────────────────────────────────────────────
-1                  constant max-u       \ 2^64 - 1
-1 1 rshift         constant max-n       \ 2^63 - 1
1 63 lshift         constant min-n       \ -2^63
255                 constant max-char
1 cells             constant cell        \ 8

\ ?NEGATE ( n flag -- n' )    negate n iff flag is non-zero.
: ?negate  ( n flag -- n' )  if negate then ;

\ HEX. BIN. OCT. DEC. — print n in a fixed base; BASE is preserved.
: hex.  ( n -- )  base @ >r hex      . r> base ! ;
: bin.  ( n -- )  base @ >r 2 base ! . r> base ! ;
: oct.  ( n -- )  base @ >r 8 base ! . r> base ! ;
: dec.  ( n -- )  base @ >r decimal  . r> base ! ;

\ CHAR- ( c-addr -- c-addr-1 )   one byte before c-addr (chars are 1 byte).
: char-  ( c-addr -- c-addr' )  1- ;

\ ── More Core-ext / utility words ─────────────────────────────────────────

\ UNUSED ( -- u )    bytes available in the dictionary heap.
\ user_DICT_END = 0x20 (32), user_HERE = 0x18 (24); base returns UP.
: unused  ( -- u )  base 32 + @  here - ;

\ M+ ( d n -- d' )    add a single to a double.
: m+  ( d n -- d' )  s>d d+ ;

\ DMAX / DMIN ( d1 d2 -- d )
: dmax  ( d1 d2 -- d )  2over 2over d< if 2swap then 2drop ;
: dmin  ( d1 d2 -- d )  2over 2over d< 0= if 2swap then 2drop ;

\ +TO ( n "name" -- )    add n to a VALUE.
: +to
    parse-name find-name dup 0= if drop throw_namereqd throw then
    drop name>interpret >body
    state @ if postpone literal ['] +! compile, else +! then
; immediate



\ BLANK ( c-addr u -- )    fill memory with spaces.
: blank  ( c-addr u -- )  bl fill ;

\ BIN ( -- )    set BASE to 2.  (Mirrors hex / decimal / octal.)
: bin  ( -- )  2 base ! ;

\ MARKER ( "name" -- )  Create a word that, when executed, restores the
\ dictionary state (HERE, LATEST) to what it was just before MARKER ran.
\ user_HERE = 0x18 = 24, user_LATEST = 0x10 = 16 (base = UP).
tools-wordlist set-current
: marker  ( "name" -- )
    base 24 + @  base 16 + @         \ snapshot ( here-before latest-before )
    create swap , ,
    does>
        dup @ base 24 + !            \ restore HERE
        cell+ @ base 16 + !          \ restore LATEST
;
forth-wordlist set-current

\ ── [DEFINED] / [UNDEFINED] / [IF] / [ELSE] / [THEN] (Tools-ext) ──────────
\ User-facing bracket words are TOOLS; the [skip] helper and the two
\ state variables are PRIVATE.
tools-wordlist set-current

: [defined]    ( "name" -- flag )
    parse-name find-name if drop -1 else 2drop 0 then ; immediate

: [undefined]  ( "name" -- flag )
    postpone [defined] 0= ; immediate

private-wordlist set-current
variable bracket-depth
variable bracket-stop-else

\ Scan source forward, tracking nested [IF]/[THEN], stopping at:
\   [THEN] at depth 0, or
\   [ELSE] at depth 0 when bracket-stop-else? is true.
: [skip]  ( stop-at-else? -- )
    bracket-stop-else !
    1 bracket-depth !
    begin
        parse-name dup if
            2dup s" [IF]"   istr= if 2drop 1 bracket-depth +!  false
            else
            2dup s" [THEN]" istr= if 2drop -1 bracket-depth +! bracket-depth @ 0=
            else
            2dup s" [ELSE]" istr= if 2drop bracket-depth @ 1 = bracket-stop-else @ and
            else 2drop false
            then then then
        else
            2drop refill 0=
        then
    until ;

tools-wordlist set-current
: [if]    ( flag -- )   0= if true [skip] then ; immediate
: [else]  ( -- )        false [skip] ;          immediate
: [then]  ( -- )        ;                        immediate
forth-wordlist set-current

\ ── Programmer's tools ─────────────────────────────────────────────────────
\ Each wordlist is a 512-slot bucket array starting at the wid.  Each
\ bucket points to a chain of overlay nodes; each node's [+16] cell
\ holds a pointer back to the dictionary header.  dh_nt (offset 47)
\ points at the counted name string.
tools-wordlist set-current

: words-in  ( wid -- )
    512 0 ?do
        dup i cells + @                       ( wid node )
        begin dup while
            dup 16 + @ 47 + count type space
            @                                  ( wid next-node )
        repeat drop
    loop drop cr ;

: words  ( -- )  get-current words-in ;

\ Per-vocabulary listing words for convenience.
: forth-words    forth-wordlist   words-in ;
: tools-words    tools-wordlist   words-in ;
: private-words  private-wordlist words-in ;

: .byte  ( n -- )
    base @ >r hex
    $FF and 0 <# # # #> type space
    r> base ! ;

: dump  ( addr n -- )
    over + swap
    begin 2dup u< while
        dup c@ .byte
        over 15 and 15 = if cr then
        1+
    repeat 2drop cr ;

\ ─── Graphical-pane event kinds ──────────────────────────────────
\ Tags returned in TOS by `gpane-next-event`.  Mirror EV_* in
\ src/runtime.rs and the IGuiEvent variants in src/igui/channels.rs.
\ Use with CASE for tidy event dispatch.
0  constant ev-none           \ timeout / no event yet
1  constant ev-key            \ ( vkey mods down repeat )
2  constant ev-char           \ ( codepoint mods 0 0 )
3  constant ev-mouse          \ ( x y op mods|button<<8 )
4  constant ev-focus          \ ( gained 0 0 0 )
5  constant ev-resize         \ ( width height 0 0 )
6  constant ev-close          \ pane requested close — ( 0 0 0 0 )
7  constant ev-frame-close    \ IDE frame closing — ( 0 0 0 0 )
13 constant ev-tick           \ ( time_ms 0 0 0 )

\ Mouse op codes (subfield of p3 in EV_MOUSE).  Match `mouse_op`
\ in src/igui/channels.rs.
0 constant mouse-move
1 constant mouse-left-down
2 constant mouse-left-up
3 constant mouse-right-down
4 constant mouse-right-up
5 constant mouse-middle-down
6 constant mouse-middle-up
7 constant mouse-wheel

\ Drop the four params left below an event-kind on the stack.
\ Useful for events whose params you don't care about.
: event-drop ( p4 p3 p2 p1 -- )  drop drop drop drop ;

forth-wordlist set-current
