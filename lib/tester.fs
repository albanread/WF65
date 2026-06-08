\ ANS Forth tester framework (John Hayes style)
\ T{ ... -> ... }T captures actual results and compares with expected.
\
\ Usage:
\   T{ <test-code> -> <expected-values> }T
\   testing s" section name"
\
\ Uses BEGIN/WHILE/REPEAT instead of DO/LOOP to avoid any interaction
\ between the biased DO frame on RSP and variable reads inside the body.

decimal

variable actual-depth
create actual-results 64 cells allot

variable start-depth
variable error-count
variable ->idx

: T{    ( -- )  depth start-depth ! ;

\ -> stores results bottom-first: actual-results[0]=bottom, [N-1]=top.
\ ->idx counts down from N to 0, storing TOS each iteration at [->idx].
: ->    ( results... -- )
    depth start-depth @ - actual-depth !
    actual-depth @ ->idx !
    begin ->idx @ while
        ->idx @ 1- ->idx !
        actual-results ->idx @ cells +
        !
    repeat ;

\ }T compares actual-results[j] with the j-th expected value (from bottom).
\ Expected stack at entry: [exp0 exp1 ... expN-1] with expN-1 on top.
\ pick(N-j) with actual-results[j] also on stack gives exp[j].
: }T    ( expected... -- )
    depth start-depth @ - actual-depth @ <> if
        s" WRONG NUMBER OF RESULTS: " type
        1 error-count +!
    else
        0 ->idx !
        begin ->idx @ actual-depth @ < while
            actual-results ->idx @ cells + @
            actual-depth @ ->idx @ - pick
            <> if
                s" INCORRECT RESULT: " type
                1 error-count +!
                actual-depth @ ->idx !
            else
                ->idx @ 1+ ->idx !
            then
        repeat
    then
    begin depth start-depth @ > while drop repeat ;

: testing   ( c-addr u -- )  s" Testing " type type cr ;

: tally     ( -- )
    error-count @
    dup 0= if
        drop s" All tests passed." type cr
    else
        0 <# #s #> type s"  errors." type cr
    then ;
