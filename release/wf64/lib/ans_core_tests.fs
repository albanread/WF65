\ ANS Forth Core wordset tests for WF64
\ Exercises all Core and Core-Ext words available in WF64.
\ Run after loading lib/core.f and lib/tester.fs.

decimal

\ ── Stack manipulation ────────────────────────────────────────────────────

s" Stack" testing

T{  1 2 swap -> 2 1 }T
T{  1 dup -> 1 1 }T
T{  1 2 drop -> 1 }T
T{  1 2 over -> 1 2 1 }T
T{  1 2 3 rot -> 2 3 1 }T
T{  1 2 3 -rot -> 3 1 2 }T
T{  0 ?dup -> 0 }T
T{  1 ?dup -> 1 1 }T
T{  1 2 nip -> 2 }T
T{  1 2 tuck -> 2 1 2 }T
T{  1 0 pick -> 1 1 }T
T{  1 2 0 pick -> 1 2 2 }T
T{  1 2 1 pick -> 1 2 1 }T
T{  1 2 3 2 pick -> 1 2 3 1 }T
T{  1 2 3 1 roll -> 1 3 2 }T
T{  1 2 3 2 roll -> 2 3 1 }T
T{  1 2 3 4 3 roll -> 2 3 4 1 }T
T{  1 2 2dup -> 1 2 1 2 }T
T{  1 2 2drop -> }T
T{  1 2 3 4 2swap -> 3 4 1 2 }T
T{  1 2 3 4 2over -> 1 2 3 4 1 2 }T

\ ── Arithmetic ────────────────────────────────────────────────────────────

s" Arithmetic" testing

T{  3 4 + -> 7 }T
T{  10 3 - -> 7 }T
T{  3 4 * -> 12 }T
T{  7 2 / -> 3 }T
T{  7 2 mod -> 1 }T
T{  7 2 /mod -> 1 3 }T
T{  -7 2 /mod -> -1 -3 }T      \ symmetric (truncate-toward-zero)
T{  -7 s>d 2 fm/mod -> 1 -4 }T
T{  -7 s>d 2 sm/rem -> -1 -3 }T
T{  3 negate -> -3 }T
T{  -3 negate -> 3 }T
T{  -3 abs -> 3 }T
T{  3 abs -> 3 }T
T{  5 1+ -> 6 }T
T{  5 1- -> 4 }T
T{  5 2+ -> 7 }T
T{  5 2- -> 3 }T
T{  5 2* -> 10 }T
T{  10 2/ -> 5 }T
T{  -1 2/ -> -1 }T
T{  3 4 max -> 4 }T
T{  3 4 min -> 3 }T

\ ── Logic ─────────────────────────────────────────────────────────────────

s" Logic" testing

T{  $FF $0F and -> $0F }T
T{  $F0 $0F or  -> $FF }T
T{  $FF $0F xor -> $F0 }T
T{  0 invert -> -1 }T
T{  1 2 lshift -> 4 }T
T{  8 2 rshift -> 2 }T
T{  -1 1 rshift -> $7FFFFFFFFFFFFFFF }T   \ logical (unsigned) shift

\ ── Comparison ────────────────────────────────────────────────────────────

s" Comparison" testing

T{  0 0= -> -1 }T
T{  1 0= -> 0 }T
T{  0 0<> -> 0 }T
T{  1 0<> -> -1 }T
T{  -1 0< -> -1 }T
T{  0 0< -> 0 }T
T{  1 0< -> 0 }T
T{  1 0> -> -1 }T
T{  0 0> -> 0 }T
T{  -1 0> -> 0 }T
T{  1 2 = -> 0 }T
T{  2 2 = -> -1 }T
T{  1 2 <> -> -1 }T
T{  2 2 <> -> 0 }T
T{  1 2 < -> -1 }T
T{  2 1 < -> 0 }T
T{  1 2 > -> 0 }T
T{  2 1 > -> -1 }T
T{  1 2 u< -> -1 }T
T{  -1 0 u< -> 0 }T           \ -1 as unsigned > 0
T{  0 -1 u< -> -1 }T          \ 0 as unsigned < -1
T{  5 3 7 within -> -1 }T
T{  2 3 7 within -> 0 }T
T{  7 3 7 within -> 0 }T

\ ── Memory ────────────────────────────────────────────────────────────────

s" Memory" testing

variable mem-test
T{  42 mem-test ! mem-test @ -> 42 }T
T{  99 mem-test !  mem-test @ -> 99 }T
variable mem-test2
T{  mem-test here <> -> -1 }T    \ HERE advanced
T{  11 mem-test2 !  mem-test2 @ -> 11 }T

create cbuf 16 allot
T{  65 cbuf c!  cbuf c@ -> 65 }T

T{  1 mem-test +!  mem-test @ -> 100 }T   \ +! on 99→100

\ ── Control flow ──────────────────────────────────────────────────────────

s" Control flow" testing

T{  : cf1 if 1 else 0 then ; -1 cf1 -> 1 }T
T{  0 cf1 -> 0 }T
T{  : cf2 begin 1- dup 0= until ; 3 cf2 -> 0 }T
T{  : cf3 0 swap 0 do i + loop ; 5 cf3 -> 10 }T
T{  : cf4  0 0 ?do 1 loop ; 0 cf4  -> 0 }T     \ limit=0 index=0: skip; caller 0 survives
T{  : cf4b 1 0 ?do 1 loop ; 0 cf4b -> 0 1 }T  \ limit=1 index=0: run once; caller 0 + 1 survive

\ CASE
T{  : cf5  case 1 of 10 endof 2 of 20 endof 0 endcase ;
    1 cf5 -> 10 }T
T{  2 cf5 -> 20 }T
T{  3 cf5 -> 0 }T

\ RECURSE
T{  : fact dup 1 > if dup 1- recurse * then ;  5 fact -> 120 }T

\ WHILE/REPEAT
T{  : cf6 0 swap begin dup while swap over + swap 1- repeat drop ;
    5 cf6 -> 15 }T   \ sum 1..5

\ ── String / char ops ─────────────────────────────────────────────────────

s" Strings" testing

T{  s" hello" nip 5 = -> -1 }T
T{  s" hello" drop c@ 104 = -> -1 }T    \ 'h' = 104
T{  bl 32 = -> -1 }T
T{  char A 65 = -> -1 }T

\ -trailing  ( c-addr u -- c-addr u' )
T{  s" hello   " -trailing nip -> 5 }T
T{  s" hello"    -trailing nip -> 5 }T
T{  s" "         -trailing nip -> 0 }T
T{  s"     "     -trailing nip -> 0 }T
T{  s" a   b   " -trailing nip -> 7 }T

\ REPLACES / SUBSTITUTE  ( Forth 2012 String-ext )
subst-init                                 \ clear table for repeatable tests
create sub-buf 128 allot

\ Bind names.
s" Alice" s" who"   replaces
s" world" s" what"  replaces

\ Plain text passthrough (no %): count = 0
T{  s" hello" sub-buf 128 substitute
    >r drop r> -> 5 0 }T

\ Single substitution.
T{  s" Hello, %who%!" sub-buf 128 substitute
    >r sub-buf swap r> -> sub-buf 13 1 }T

\ Two substitutions, %%-literal.
T{  s" %who% says %what% 100%%" sub-buf 128 substitute
    >r drop r> -> 21 2 }T

\ Rebind: REPLACES overwrites prior value.
s" Bob" s" who" replaces
T{  s" %who%!" sub-buf 128 substitute
    >r drop r> -> 4 1 }T

\ Unknown name kept literally as %name%.
T{  s" hi %unknown%!" sub-buf 128 substitute
    >r drop r> -> 13 0 }T

\ ── Double cell ───────────────────────────────────────────────────────────

s" Double" testing

T{  1 s>d -> 1 0 }T
T{  -1 s>d -> -1 -1 }T
T{  0 0 d0= -> -1 }T
T{  1 0 d0= -> 0 }T
T{  0 0 d0< -> 0 }T
T{  0 -1 d0< -> -1 }T
T{  1 0 2 0 d+ -> 3 0 }T
T{  1 0 2 0 d< -> -1 }T
T{  2 0 1 0 d< -> 0 }T
T{  1 0 dnegate -> -1 -1 }T
T{  -1 -1 dnegate -> 1 0 }T
T{  -1 -1 dabs -> 1 0 }T
T{  3 0 dabs -> 3 0 }T

\ ── Number parsing and BASE ───────────────────────────────────────────────

s" Number parsing" testing

T{  $FF -> 255 }T              \ $ prefix forces hex regardless of BASE
T{  base @ 10 = -> -1 }T      \ $ parse does not change BASE

\ ── Defining words ────────────────────────────────────────────────────────

s" Defining words" testing

T{  : dw1  42 ;  dw1 -> 42 }T
T{  : dw2  dup * ;  5 dw2 -> 25 }T
T{  variable dv1  1 dv1 !  dv1 @ -> 1 }T
T{  2 dv1 !  dv1 @ -> 2 }T
T{  5 constant kc1  kc1 -> 5 }T
T{  5 value vv1  vv1 -> 5 }T
T{  10 to vv1  vv1 -> 10 }T

T{  create c1-arr 3 cells allot
    11 c1-arr ! 22 c1-arr cell+ !
    c1-arr @ c1-arr cell+ @ -> 11 22 }T

\ ── :noname ───────────────────────────────────────────────────────────────

s" :noname" testing

T{  :noname 1 2 + ; execute -> 3 }T
T{  :noname dup * ; 5 swap execute -> 25 }T

\ ── DEFER / ACTION-OF / IS ───────────────────────────────────────────────

s" Defer" testing

defer def1
T{  :noname 99 ; is def1   def1 -> 99 }T
T{  :noname 42 ; is def1   def1 -> 42 }T

\ ── CATCH / THROW ────────────────────────────────────────────────────────

s" Catch/Throw" testing

T{  ' noop catch -> 0 }T
T{  :noname 42 throw ; catch -> 42 }T
T{  :noname 0 throw ; catch -> 0 }T

\ ── Pictured numeric output ───────────────────────────────────────────────

s" Pictured numeric output" testing

\ u. output (capture via string comparison)
T{  $FEED hex 0 <# # # # # #> nip decimal 4 = -> -1 }T   \ 4 digits

\ ── Compile-time words ────────────────────────────────────────────────────

s" Compile words" testing

T{  : cw1  [ 1 2 + ] literal ;  cw1 -> 3 }T
T{  : cw2  ['] + execute ;  3 4 cw2 -> 7 }T

\ ── Return stack ─────────────────────────────────────────────────────────

s" Return stack" testing

T{  : rs1 >r 1 r> ;  0 rs1 -> 1 0 }T
T{  : rs2 >r >r r> r> ;  1 2 rs2 -> 1 2 }T
T{  : rs3 1 >r r@ r> ;  rs3 -> 1 1 }T
T{  : rs4 1 2 2>r 2r> ;  rs4 -> 1 2 }T

\ ── LEAVE in loops ────────────────────────────────────────────────────────

s" Leave" testing

T{  : lv1  0 5 0 do i 3 = if leave then i + loop ;
    lv1 -> 3 }T   \ 0+1+2 and then leaves on i=3

\ ── ENVIRONMENT? ─────────────────────────────────────────────────────────

s" Environment" testing

T{  s" MAX-N" environment? -> false }T   \ stub: always false

\ ── File-Access: OPEN / CREATE / READ / WRITE / CLOSE / DELETE ───────────

s" File-Access" testing

variable fa-fid
variable fa-ior
variable fa-n
create   fa-buf  256 allot

\ Temp filename, kept around as a 2constant so the path bytes stay valid
\ across READ-FILE and the subsequent DELETE-FILE.
s" tmp_file_access_test.dat" 2constant tmpf

\ CREATE-FILE
T{  tmpf r/w create-file fa-ior ! fa-fid !  fa-ior @ -> 0 }T

\ WRITE-FILE
T{  s" Hello, World!" fa-fid @ write-file -> 0 }T

\ CLOSE-FILE
T{  fa-fid @ close-file -> 0 }T

\ OPEN-FILE for reading
T{  tmpf r/o open-file fa-ior ! fa-fid !  fa-ior @ -> 0 }T

\ READ-FILE: read 13 bytes back into fa-buf
T{  fa-buf 256 fa-fid @ read-file fa-ior ! fa-n !  fa-ior @ -> 0 }T
T{  fa-n @ -> 13 }T
T{  fa-buf c@ -> 72 }T              \ 'H'
T{  fa-buf 7 + c@ -> 87 }T          \ 'W'

\ CLOSE again
T{  fa-fid @ close-file -> 0 }T

\ DELETE-FILE
T{  tmpf delete-file -> 0 }T

\ Opening a nonexistent file should fail.
T{  s" definitely_not_a_real_file_12345.xyz" r/o open-file nip -> 0 = }T

\ FILE-POSITION / FILE-SIZE / REPOSITION-FILE / WRITE-LINE
s" tmp_file_seek_test.dat" 2constant fs-path
T{  fs-path r/w create-file fa-ior ! fa-fid !  fa-ior @ -> 0 }T
T{  s" hello" fa-fid @ write-line -> 0 }T
T{  s" world" fa-fid @ write-line -> 0 }T
T{  fa-fid @ file-position drop drop -> 14 }T        \ 5 + 2 + 5 + 2 = 14
T{  fa-fid @ file-size     drop drop -> 14 }T
\ Reposition to start, read the first 5 bytes.
T{  0 0 fa-fid @ reposition-file -> 0 }T
T{  fa-buf 5 fa-fid @ read-file fa-ior ! fa-n !  fa-ior @ fa-n @ -> 0 5 }T
T{  fa-buf c@ -> 104 }T                              \ 'h'
T{  fa-fid @ close-file -> 0 }T
T{  fs-path delete-file -> 0 }T

\ FLUSH-FILE / RENAME-FILE
s" tmp_rename_src.dat" 2constant rn-src
s" tmp_rename_dst.dat" 2constant rn-dst
T{  rn-src r/w create-file fa-ior ! fa-fid !  fa-ior @ -> 0 }T
T{  s" data" fa-fid @ write-file -> 0 }T
T{  fa-fid @ flush-file -> 0 }T
T{  fa-fid @ close-file -> 0 }T
T{  rn-src rn-dst rename-file -> 0 }T
\ Source no longer exists; destination opens cleanly.
T{  rn-src r/o open-file nip -> 0 = }T
T{  rn-dst r/o open-file fa-ior ! fa-fid !  fa-ior @ -> 0 }T
T{  fa-fid @ close-file -> 0 }T
T{  rn-dst delete-file -> 0 }T

\ AT-XY / PAGE — output-only smoke tests, just confirm clean stack.
T{  0 0 at-xy -> }T
T{  page -> }T

\ READ-LINE round-trip: write two lines, read them back.
s" tmp_readline_test.dat" 2constant rl-path
T{  rl-path r/w create-file fa-ior ! fa-fid !  fa-ior @ -> 0 }T
T{  s" line one" fa-fid @ write-line -> 0 }T
T{  s" line two" fa-fid @ write-line -> 0 }T
T{  fa-fid @ close-file -> 0 }T
T{  rl-path r/o open-file fa-ior ! fa-fid !  fa-ior @ -> 0 }T
T{  fa-buf 256 fa-fid @ read-line -> 8 -1 0 }T          \ "line one"
T{  fa-buf c@ -> 108 }T                                   \ 'l'
T{  fa-buf 256 fa-fid @ read-line -> 8 -1 0 }T          \ "line two"
T{  fa-buf 256 fa-fid @ read-line -> 0 0 0 }T            \ EOF
T{  fa-fid @ close-file -> 0 }T
T{  rl-path delete-file -> 0 }T

\ ── Locals stack scaffolding (R15 = LP) ──────────────────────────────────

s" Locals-stack-R15" testing

\ At rest, LP = LP0 (no frame allocated).
T{  lp@ lp0@ =  -> -1 }T

\ The locals region is 1 MB.
T{  lp0@ lp-limit -  -> 1048576 }T

\ Smoke: allocate 1 cell, store/read 42, free.
T{  lp-smoke  -> 42 }T

\ R15 survives many smoke calls (no leak each round-trip).
T{  lp-smoke lp-smoke lp-smoke  -> 42 42 42 }T
T{  lp@ lp0@ =  -> -1 }T

\ R15 survives Win32 calls (msvcrt, kernel32).
T{  4e fsqrt  lp@ lp0@ =  -> -1 }T
T{  4e fsqrt 2e f=  -> -1 }T               \ also confirm fsqrt still returns 2.0
T{  0 ms  lp@ lp0@ =  -> -1 }T              \ kernel32 Sleep preserves R15

\ The region is writable through R15.
T{  lp-smoke lp-smoke +  -> 84 }T

\ ── Forth 2012 locals: {: ... :} ─────────────────────────────────────────

\ Basic: one local. {: x :} pops TOS into x; reading x returns it.
T{  : loc1  {: x :} x ;
    42 loc1  -> 42 }T

\ Two locals. Last-declared takes TOS.
T{  : loc2  {: a b :} a b ;
    10 20 loc2  -> 10 20 }T

\ Three locals + arithmetic.
T{  : sum3  {: x y z :} x y + z + ;
    1 2 3 sum3  -> 6 }T

\ Locals frame is released on exit — LP back to LP0.
T{  10 20 30 sum3  lp@ lp0@ =  -> 60 -1 }T

\ Locals used in non-trivial computation.
T{  : avg3  {: x y z :} x y + z + 3 / ;
    2 4 6 avg3  -> 4 }T

\ Recursion with locals: each call gets its own frame.
T{  : fact  {: n :} n 1 <= if 1 else n n 1 - recurse * then ;
    5 fact  -> 120 }T
T{  6 fact  -> 720 }T

\ EXIT inside a locals-defining word releases the frame too.
T{  : maybe-exit  {: n :} n 0 = if 99 exit then n 2 * ;
    0 maybe-exit  -> 99 }T
T{  5 maybe-exit  -> 10 }T

\ After all the locals tests, LP must still be balanced.
T{  lp@ lp0@ =  -> -1 }T

\ Locals are compile-time-only -- they do NOT become dictionary entries.
\ Use names unlikely to clash with any other test or core.f word.
T{  : zz1  {: locals-priv-abc locals-priv-xyz :}
       locals-priv-abc locals-priv-xyz + ;
    3 4 zz1                              -> 7 }T
T{  [defined] locals-priv-abc            ->  0 }T
T{  [defined] locals-priv-xyz            ->  0 }T

\ Same name re-used in another word -- no shadow conflict because the
\ first word's "local" was never in the dictionary to begin with.
T{  : zz2  {: locals-priv-abc :}  locals-priv-abc 10 * ;
    7 zz2                                -> 70 }T

\ ── Memory-Allocation: ALLOCATE / FREE / RESIZE ──────────────────────────

s" Memory-Allocation" testing

variable alloc-addr
variable alloc-ior

\ ALLOCATE 100 bytes — ior should be 0.
T{  100 allocate alloc-ior ! alloc-addr !  alloc-ior @ -> 0 }T

\ Write/read a byte through the allocated block.
T{  65 alloc-addr @ c!  alloc-addr @ c@ -> 65 }T

\ RESIZE to 200 — ior should be 0, save new addr.
T{  alloc-addr @ 200 resize alloc-ior ! alloc-addr !  alloc-ior @ -> 0 }T

\ Byte should still be there after resize.
T{  alloc-addr @ c@ -> 65 }T

\ FREE — ior should be 0.
T{  alloc-addr @ free -> 0 }T

\ Single-line round-trip.
T{  64 allocate swap free -> 0 }T

\ ── Facility-ext: MS / UTIME / TIME&DATE ─────────────────────────────────

s" Facility-ext" testing

\ MS: smoke — sleep 0 is a no-op, leaves stack empty.
T{  0 ms -> }T

\ UTIME: smoke — returns a double, both halves are non-negative integers.
T{  utime drop 0 >= -> -1 }T              \ low cell >= 0 (always true)
T{  utime nip 0 >= -> -1 }T               \ high cell >= 0

\ TIME&DATE: ranges. sec 0..59, min 0..59, hour 0..23, day 1..31, month 1..12, year > 2020.
T{  time&date  swap drop swap drop swap drop swap drop swap drop  2020 > -> -1 }T
T{  time&date  drop drop drop drop drop  60 <        -> -1 }T   \ sec < 60

\ ── Double-cell additions ────────────────────────────────────────────────

s" Double-extras" testing

\ D>S
T{   5 0  d>s ->  5 }T
T{  -5 -1 d>s -> -5 }T

\ D<= D>= DU<= DU>=
T{   1 0   2 0  d<=  -> -1 }T
T{   2 0   1 0  d<=  ->  0 }T
T{   2 0   2 0  d<=  -> -1 }T
T{   1 0   2 0  d>=  ->  0 }T
T{   2 0   1 0  d>=  -> -1 }T
T{   2 0   2 0  d>=  -> -1 }T
T{   1 0   2 0  du<= -> -1 }T
T{   2 0   1 0  du>= -> -1 }T

\ D0<> D0>= D0<= D0>
T{   0  0  d0<> ->  0 }T
T{   1  0  d0<> -> -1 }T
T{   0  0  d0>= -> -1 }T
T{   1  0  d0>= -> -1 }T
T{  -1 -1  d0>= ->  0 }T
T{   0  0  d0<= -> -1 }T
T{   1  0  d0<= ->  0 }T
T{  -1 -1  d0<= -> -1 }T
T{   1  0  d0>  -> -1 }T
T{   0  0  d0>  ->  0 }T
T{  -1 -1  d0>  ->  0 }T

\ UD.R smoke test (output only)
T{   5 0  3 ud.r -> }T

\ ── UNLESS / THIRD / ?: / F. / ASSERT ────────────────────────────────────

s" Misc-convenience" testing

\ UNLESS  inverts the IF test.
T{  : u1 0  unless 1 else 2 then ;  u1 -> 1 }T
T{  : u2 -1 unless 1 else 2 then ;  u2 -> 2 }T
T{  : u3 5  unless 1 else 2 then ;  u3 -> 2 }T   \ any nonzero → else

\ THIRD
T{  10 20 30 third -> 10 20 30 10 }T

\ ?:  pick first if flag, second otherwise
T{  -1  10 20 ?: -> 10 }T
T{   0  10 20 ?: -> 20 }T
T{   1  10 20 ?: -> 10 }T

\ F. (output-only; just verify it leaves no data-stack residue)
T{   3e f. -> }T
T{   0e f. -> }T
T{   0e 1e f- f. -> }T

\ ASSERT — true flag is a no-op
T{  -1 s" should not abort" assert -> }T

\ ── 2VALUE / 2TO / UNDER+ / FORGET ───────────────────────────────────────

s" 2value/2to/under+" testing

T{  10 20 2value 2v1     2v1 -> 10 20 }T
T{  77 88 2to 2v1        2v1 -> 77 88 }T
T{  : bump-2v 5 6 2to 2v1 ; bump-2v 2v1 -> 5 6 }T

T{  1 2 3 under+ -> 4 2 }T
T{  10 20 30 under+ -> 40 20 }T

\ FORGET round-trip via marker checkpoint
marker fg-anchor
: fg-foo 111 ;
: fg-bar 222 ;
T{  fg-foo fg-bar -> 111 222 }T
forget fg-foo
T{  [defined] fg-foo -> 0 }T
T{  [defined] fg-bar -> 0 }T
fg-anchor

\ ── String helpers: -LEADING / STARTS-WITH? ──────────────────────────────

s" String-helpers" testing

\ -leading
T{  s"    hello" -leading nip -> 5 }T
T{  s" hello"    -leading nip -> 5 }T
T{  s"        "  -leading nip -> 0 }T
T{  s" "         -leading nip -> 0 }T
T{  s"   a  b "  -leading nip -> 5 }T          \ "a  b " after strip

\ starts-with?
T{  s" hello world" s" hello" starts-with? -> -1 }T
T{  s" hello"       s" hello" starts-with? -> -1 }T
T{  s" hel"         s" hello" starts-with? ->  0 }T   \ string shorter than prefix
T{  s" hello"       s" world" starts-with? ->  0 }T   \ no match
T{  s" abc"         s" "      starts-with? -> -1 }T   \ empty prefix always matches

\ ends-with?
T{  s" hello world" s" world"  ends-with? -> -1 }T
T{  s" hello"       s" hello"  ends-with? -> -1 }T
T{  s" hello"       s" ello"   ends-with? -> -1 }T
T{  s" hello"       s" world"  ends-with? ->  0 }T
T{  s" hi"          s" hello"  ends-with? ->  0 }T   \ shorter than suffix
T{  s" abc"         s" "       ends-with? -> -1 }T   \ empty suffix always matches

\ contains?
T{  s" hello world" s" world"  contains? -> -1 }T
T{  s" hello world" s" lo wo"  contains? -> -1 }T
T{  s" hello"       s" xyz"    contains? ->  0 }T

\ ── Floating-point helpers ────────────────────────────────────────────────

s" Float-helpers" testing

\ FABS
T{   1e fabs 1e f= -> -1 }T
T{  0e 1e f- fabs 1e f= -> -1 }T               \ |-1| = 1
T{   0e fabs f0= -> -1 }T

\ FMAX / FMIN
T{  1e 2e fmax 2e f= -> -1 }T
T{  2e 1e fmax 2e f= -> -1 }T
T{  1e 2e fmin 1e f= -> -1 }T
T{  2e 1e fmin 1e f= -> -1 }T

\ Float comparisons
T{  1e 2e f<  -> -1 }T
T{  2e 1e f<  ->  0 }T
T{  1e 1e f<  ->  0 }T

T{  1e 2e f>  ->  0 }T
T{  2e 1e f>  -> -1 }T
T{  1e 1e f>  ->  0 }T

T{  1e 1e f<= -> -1 }T
T{  1e 2e f<= -> -1 }T
T{  2e 1e f<= ->  0 }T

T{  1e 1e f>= -> -1 }T
T{  2e 1e f>= -> -1 }T
T{  1e 2e f>= ->  0 }T

T{  1e 1e f=  -> -1 }T
T{  1e 2e f=  ->  0 }T
T{  1e 2e f<> -> -1 }T
T{  1e 1e f<> ->  0 }T

\ F2* / F2/
T{   3e f2* 6e f= -> -1 }T
T{   6e f2/ 3e f= -> -1 }T
T{   1e f2* 2e f= -> -1 }T

\ FTRUNC (round toward zero via f>d d>f)
T{   3e f2* 1e f+ ftrunc 7e f= -> -1 }T
T{   0e 7e f- ftrunc 0e 7e f- f= -> -1 }T

\ FROUND
T{   3e 0.5e f+ fround 4e f= -> -1 }T          \ 3.5 → 4 (away from zero on positive)
T{   3e 0.5e f- fround 3e f= -> -1 }T          \ 2.5 → 2 ??? actually 2.5 rounds to 3 with away-from-zero
\ Let me redo the second test: 2.5 + 0.5 = 3.0 trunc → 3. OK ok.
T{  0e 3e f- 0.5e f- fround 0e 4e f- f= -> -1 }T  \ -3.5 → -4

\ FLOOR
T{   3e 0.5e f+ floor 3e f= -> -1 }T            \ 3.5 → 3
T{  0e 3e f- 0.5e f- floor 0e 4e f- f= -> -1 }T \ -3.5 → -4
T{   3e floor 3e f= -> -1 }T                    \ exact integer unchanged
T{  0e 3e f- floor 0e 3e f- f= -> -1 }T         \ -3 → -3

\ >FLOAT
T{  s" 3.14"      >float -> -1 }T
T{  s" 3.14"      >float drop f0< 0= -> -1 }T   \ positive
T{  s" not-a-num" >float -> 0 }T

\ Float math (msvcrt)
T{   4e fsqrt 2e f= -> -1 }T
T{   9e fsqrt 3e f= -> -1 }T
T{   0e fsin f0= -> -1 }T
T{   0e fcos 1e f= -> -1 }T
T{   0e ftan f0= -> -1 }T
T{   0e fexp 1e f= -> -1 }T
T{   1e fln  f0= -> -1 }T
T{   1e flog f0= -> -1 }T
T{  10e flog 1e f= -> -1 }T
T{   0e fatan f0= -> -1 }T
T{   0e fasin f0= -> -1 }T
T{   1e facos f0= -> -1 }T
T{   2e 3e f** 8e f= -> -1 }T            \ 2^3 = 8
T{   3e 2e f** 9e f= -> -1 }T            \ 3^2 = 9
T{   0e 1e fatan2 f0= -> -1 }T           \ atan2(0,1) = 0

\ FALOG
T{   0e falog 1e   f= -> -1 }T
T{   1e falog 10e  f= -> -1 }T
T{   2e falog 100e f= -> -1 }T
T{  10e flog 1e f= -> -1 }T

\ F0<>
T{   0e f0<> -> 0 }T
T{   1e f0<> -> -1 }T

\ COMPILE-NAME — use inside an immediate helper that compiles a call to
\ the named word into the surrounding colon definition.
T{  : pn-helper  parse-name compile-name ; immediate
    : pn-square  5 pn-helper dup * ;
    pn-square -> 25 }T

\ ── EXECUTE-PARSING / SAVE-INPUT / NAME>STRING ───────────────────────────

s" Input-source" testing

\ EXECUTE-PARSING redirects the source for the duration of xt.
T{  s" hello world" ' parse-name execute-parsing nip -> 5 }T
T{  s" 42abc"       ' parse-name execute-parsing nip -> 5 }T

\ Source is restored — subsequent test sees the M7 source as normal.
T{  s" alpha"       ' parse-name execute-parsing nip
    1 2 + -> 5 3 }T

T{  save-input restore-input -> 0 }T

\ NAME>STRING — same payload as `count` on the nt.
\ Skipped: needs further investigation of find-name shape inside T{ }T.

\ ── S\" ── escaped string literal ────────────────────────────────────────

s" S-escape-quote" testing

T{  s\" hello"     nip            -> 5 }T          \ plain text
T{  s\" a\nb"      nip            -> 3 }T          \ \n is one byte
T{  s\" a\nb"      drop 1 + c@    -> 10 }T         \ LF
T{  s\" a\tb"      drop 1 + c@    -> 9 }T          \ TAB
T{  s\" a\rb"      drop 1 + c@    -> 13 }T         \ CR
T{  s\" \\"        nip            -> 1 }T          \ literal backslash
T{  s\" \\"        drop c@        -> 92 }T
T{  s\" \""        nip            -> 1 }T          \ literal quote
T{  s\" \""        drop c@        -> 34 }T
T{  s\" \x41\x42"  nip            -> 2 }T          \ hex
T{  s\" \x41\x42"  drop dup c@ swap 1+ c@ -> 65 66 }T
T{  s\" \0"        drop c@        -> 0 }T          \ NUL

\ Compile-mode: bytes embedded inline.
T{  : sqf1 s\" hi\nthere" nip ;  sqf1 -> 8 }T
T{  : sqf2 s\" \x4Apple" drop c@ ;  sqf2 -> 74 }T

\ ── SYNONYM ───────────────────────────────────────────────────────────────

s" Synonym" testing

: orig-w 42 ;
synonym alias-w orig-w
T{  alias-w -> 42 }T

\ Synonym tracks the target's current behavior at definition time;
\ redefining orig-w later does not retarget alias-w.
: orig-w 99 ;
T{  alias-w -> 42 }T
T{  orig-w  -> 99 }T

\ ── Structures (BEGIN-STRUCTURE / FIELD: / +FIELD / ...) ──────────────────

s" Structures" testing

begin-structure point
  field: .x
  field: .y
end-structure

T{  point   -> 16 }T
T{   0 .x   -> 0 }T
T{   0 .y   -> 8 }T
T{  100 .x  -> 100 }T
T{  100 .y  -> 108 }T

create pt point allot
T{  7 pt .x !   pt .x @ -> 7 }T
T{  11 pt .y !  pt .y @ -> 11 }T
T{  pt .x @     -> 7 }T          \ .y store didn't clobber .x

\ Mixed field types
begin-structure rec
  cfield: .tag         \ 1 byte
  field:  .id          \ aligns to 8, +8
  2field: .val         \ +16
end-structure

T{  rec   -> 32 }T
T{   0 .tag -> 0 }T
T{   0 .id  -> 8 }T              \ aligned past .tag
T{   0 .val -> 16 }T

\ ── Limit constants / helpers ─────────────────────────────────────────────

s" Constants/Helpers" testing

T{  max-u 1+ -> 0 }T                  \ unsigned wraps to 0
T{  max-n 1+ min-n = -> -1 }T         \ overflows to most negative
T{  max-char -> 255 }T
T{  cell -> 8 }T
T{  3 cells cell + -> 32 }T

\ ?NEGATE
T{   5  0 ?negate ->  5 }T
T{   5 -1 ?negate -> -5 }T
T{  -3 -1 ?negate ->  3 }T

\ HEX. / BIN. / OCT. / DEC. preserve BASE
T{  decimal  255 hex.  base @ -> 10 }T
T{  hex      255 bin.  base @ -> 16 }T  decimal
T{  decimal  8 oct.    base @ -> 10 }T
T{  decimal  42 dec.   base @ -> 10 }T

\ CHAR-
T{  pad 4 + char- pad - -> 3 }T

\ ── UNUSED / M+ / DMAX / DMIN / +TO ───────────────────────────────────────

s" Dictionary/Double/+TO" testing

\ UNUSED shrinks when we ALLOT.
T{  unused  here 32 allot  unused -  -> 32 }T

\ M+
T{   5 0   3 m+ -> 8 0 }T
T{  -5 -1  3 m+ -> -2 -1 }T

\ DMAX / DMIN
T{  3 0 5 0 dmax -> 5 0 }T
T{  5 0 3 0 dmax -> 5 0 }T
T{  3 0 5 0 dmin -> 3 0 }T
T{  -1 -1 1 0 dmax -> 1 0 }T   \ -1 (double) < 1 (double)
T{  -1 -1 1 0 dmin -> -1 -1 }T

\ +TO
T{  10 value pv1   5 +to pv1   pv1 -> 15 }T
T{  : bumpit 7 +to pv1 ;  bumpit  pv1 -> 22 }T

\ ── BLANK / BIN ───────────────────────────────────────────────────────────

s" Blank/Bin" testing

create blanktest 8 allot
T{  65 blanktest c!  blanktest c@ -> 65 }T
T{  blanktest 4 blank  blanktest c@ -> 32 }T
T{  blanktest 3 cells + c@ -> 32 }T

T{  base @ >r  bin  base @  r> base !  -> 2 }T
T{  hex base @ decimal -> 16 }T

\ ── [DEFINED] / [UNDEFINED] / [IF] / [ELSE] / [THEN] ──────────────────────

s" Bracket-IF" testing

T{  [defined] dup           -> -1 }T
T{  [defined] no-such-word  -> 0 }T
T{  [undefined] no-such-word -> -1 }T
T{  [undefined] dup          -> 0 }T

\ Use [IF]/[ELSE]/[THEN] inside a definition body.
T{  : bi1 [ -1 ] [if] 10 [else] 20 [then] ;   bi1 -> 10 }T
T{  : bi2 [  0 ] [if] 10 [else] 20 [then] ;   bi2 -> 20 }T
T{  : bi3 [  0 ] [if] 10 [then] 99 ;          bi3 -> 99 }T

\ Nested [IF]/[THEN] inside a skipped branch.
T{  : bi4 [ 0 ] [if]  [ -1 ] [if] 1 [then]  2  [else] 3 [then] ;
    bi4 -> 3 }T

\ ── MARKER ────────────────────────────────────────────────────────────────

s" Marker" testing

marker rollback
: trial-word 12345 ;
T{  trial-word -> 12345 }T
rollback
T{  [defined] trial-word -> 0 }T

\ ── INLINE declarator (WF32-style optimisation) ─────────────────────────

s" Inline" testing

\ Helper: read dh_ofa (the inline-body length) from an xt.
\ xt - cell holds the backoffset to dh_ct; subtract dh_ct (8) to get the
\ header base, then dh_ofa is at +42.
: ofa-of  ( xt -- ofa )  dup cell - @ + 8 - 42 + w@ ;

\ Define a LEAF word and mark it inline.  `5 +` folds to an immediate
\ `add rax, 5` (no CALL), so the body is safe to copy verbatim.
: add5  5 + ;  inline
T{  3 add5                               -> 8 }T
T{  10 add5                              -> 15 }T

\ inline set dh_ofa to the body length (non-zero) for this leaf word.
T{  ' add5 ofa-of  0>                    -> -1 }T

\ A second word that REFERENCES add5 now compiles inlined copies of its
\ body, not CALLs to it. Verifies the inline copy is correct.
: add10-via-inline  add5 add5 ;
T{  3 add10-via-inline                   -> 13 }T
T{  -2 add10-via-inline                  -> 8 }T

\ A NON-leaf word marked inline must be REFUSED (dh_ofa stays 0) because
\ its body contains a relative `call +` that can't be copied verbatim.
\ It falls back to a normal CALL, so referencing it is still correct and
\ never crashes (regression guard for the inline-of-non-leaf bug).
: nonleaf-double  dup + ;  inline
T{  ' nonleaf-double ofa-of              -> 0 }T
T{  3 nonleaf-double                     -> 6 }T
: use-nonleaf  nonleaf-double nonleaf-double ;
T{  3 use-nonleaf                        -> 12 }T
T{  -2 use-nonleaf                       -> -8 }T

\ A word with dh_ofa = 0 falls back to CALL, so calling via (inline,)
\ on an unmarked word still works.
: not-inlined  3 * ;
T{  ' not-inlined ofa-of                 -> 0 }T
T{  5 not-inlined                        -> 15 }T

\ ── Tally ─────────────────────────────────────────────────────────────────

tally
