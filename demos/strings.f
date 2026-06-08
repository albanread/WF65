\ strings.f — managed string surface demo
\
\ Exercises a slice of the V2s string library:
\   S$" lit"      compile-time managed string literal
\   $len          length in bytes
\   $clen         length in codepoints
\   $+            concatenation
\   $upper $lower case conversion
\   $hash         FNV-style 64-bit hash
\   $find         substring search
\   $slice        substring extraction
\
\ Helper:
\   $.            print a $ string (no newline)
\   $.cr          print a $ string + newline

: $. ( $ -- )    dup $>addr swap $len type ;
: $.cr ( $ -- )  $. cr ;

: strings
    cr ." === managed strings ===" cr

    S$" hello"  ." s = "  dup $.  ."   $len = " dup $len .
                          ."   $clen = " $clen . cr

    \ Concatenation: "hello" + " world!"
    S$" hello"  S$" , world!"  $+
    ." concat   -> "  $.cr

    \ Uppercase / lowercase round-trip.
    S$" FoRtH"  $upper  ." upper    -> "  $.cr
    S$" FoRtH"  $lower  ." lower    -> "  $.cr

    \ Hash a few literals and print the values.
    S$" alpha" $hash ." hash alpha = " . cr
    S$" beta"  $hash ." hash beta  = " . cr
    S$" alpha" $hash ." hash alpha = " . ." (stable across calls)" cr

    \ Substring search: find "ll" in "hello".
    S$" hello"  S$" ll"  $find
    ." find ""ll"" in ""hello""  -> index " . cr

    \ Slice "hello"[1..4] = "ell".
    S$" hello"  1 3 $slice
    ." slice [1..3]  -> "  $.cr

    ." === done ===" cr
;
