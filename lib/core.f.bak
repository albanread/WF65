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

: f, here f! 1 floats allot ;
: fvariable create 1 floats allot ;
 
: (comp-cons) ( xt -- ) >body postpone literal ;
 
: constant create , does> @ ;
 
' (comp-cons) ' constant compiles

: (comp-2cons) ( xt -- ) >body postpone literal postpone 2@ ;

: 2constant create 2, does> 2@ ;

' (comp-2cons) ' 2constant compiles

: (comp-fconst) ( xt -- ) >body postpone literal postpone f@ ;

: fconstant create f, does> f@ ;

' (comp-fconst) ' fconstant compiles
 
: (comp-val) ( xt -- ) >body postpone literal postpone @ ;
 
: value create , does> @ ;
 
' (comp-val) ' value compiles
 
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
 

: square dup * ;        ( n -- n^2 )
: cube dup dup * * ;   ( n -- n^3 )
: quad square square ; ( n -- n^4 )
: sixth cube square ;  ( n -- n^6 )
