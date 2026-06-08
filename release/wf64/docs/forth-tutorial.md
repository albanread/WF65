# Forth Tutorial

A practical introduction to programming in WF64. No prior Forth knowledge needed, but familiarity with any programming language helps.

---

## 1. The stack model

Forth is a stack-based language. Most operations take their inputs from a data stack and push results back onto it. There are no expression trees, no operator precedence rules, no parentheses for grouping — just values and operations in the order you write them.

### Pushing values

Type a number and press Enter. It goes onto the stack:

```forth
42
```

Nothing prints yet. Type `.s` to see the stack without consuming it:

```forth
.s
\ <1> 42
```

`<1>` is the depth; `42` is the only item, shown bottom-first. The topmost item (TOS) is on the right.

### Basic arithmetic

```forth
3 4 +
.s
\ <1> 7
```

`+` consumed two items and pushed one. The stack now holds `7`.

```forth
10 3 -  .s        \ <1> 7
10 3 *  .s        \ <1> 30
10 3 /  .s        \ <1> 3
10 3 mod  .s      \ <1> 1
```

Arithmetic is always postfix: `a b op` means `a op b`.

### Printing

`.` pops and prints TOS as a signed decimal followed by a space:

```forth
3 4 + .           \ 7
```

`cr` emits a newline. You often see them together:

```forth
3 4 + . cr        \ 7 (with newline)
```

---

## 2. Stack manipulation

Forth programs spend a lot of time moving values around the stack.

| Word | Effect | Description |
|------|--------|-------------|
| `dup` | `( x -- x x )` | Copy TOS |
| `drop` | `( x -- )` | Discard TOS |
| `swap` | `( a b -- b a )` | Swap top two |
| `over` | `( a b -- a b a )` | Copy second to top |
| `rot` | `( a b c -- b c a )` | Rotate third to top |
| `nip` | `( a b -- b )` | Drop second |
| `2dup` | `( a b -- a b a b )` | Duplicate top pair |
| `2drop` | `( a b -- )` | Drop top pair |

Watch `.s` as you manipulate:

```forth
1 2 3     .s      \ <3> 1 2 3
swap      .s      \ <3> 1 3 2
rot       .s      \ <3> 3 2 1
drop      .s      \ <2> 3 2
```

A word like `sq` — square the top of stack — naturally uses `dup`:

```forth
: sq  dup * ;
5 sq .            \ 25
```

The `: name ... ;` syntax compiles a new word. After `;` it is immediately available.

---

## 3. Defining words

A colon definition compiles a sequence of words into a new callable word:

```forth
: double  2 * ;
: triple  3 * ;
: sextuple  double triple ;

6 sextuple .      \ 36
```

Stack-effect comments (between `(` and `)`) are convention, not syntax:

```forth
: average  ( a b -- avg )  + 2 / ;
10 20 average .   \ 15
```

### Variables and constants

```forth
variable count
0 count !           \ store 0
count @ .           \ fetch and print: 0
1 count +!          \ add 1 in place
count @ .           \ 1

42 constant answer
answer .            \ 42
```

`@` fetches a cell from an address; `!` stores. `+!` adds directly to memory.

### Values

`value` is like a constant but can be changed with `to`:

```forth
10 value limit
limit .             \ 10
20 to limit
limit .             \ 20
```

`2value` does the same for double-cell pairs.

---

## 4. Control flow

### If / then / else

```forth
: positive? ( n -- )
    0 > if
        ." positive" cr
    else
        ." not positive" cr
    then ;

5 positive?         \ positive
-3 positive?        \ not positive
```

Flags in Forth are `-1` (true) or `0` (false). Any non-zero value is treated as true by `if`.

### Begin / until (post-test loop)

The body runs at least once; exits when the flag on top of stack is non-zero:

```forth
: countdown ( n -- )
    begin
        dup . cr
        1-
        dup 0=
    until
    drop ;

5 countdown
\ 5 4 3 2 1
```

### Begin / while / repeat (pre-test loop)

Exits immediately if the condition is false on first check:

```forth
: count-up ( n -- )
    0 swap          \ start end
    begin
        over over <
    while
        over . cr
        swap 1+ swap
    repeat
    2drop ;

5 count-up          \ 0 1 2 3 4
```

### Do / loop (counted loop)

`do` takes `( limit start -- )` and iterates while `i < limit`:

```forth
: ten-numbers  10 0 do  i .  loop  cr ;
ten-numbers     \ 0 1 2 3 4 5 6 7 8 9
```

`i` is the current loop index. `j` is the outer loop index in nested loops.
`+loop` steps by the given increment:

```forth
: evens  10 0 do  i .  2 +loop  cr ;
evens           \ 0 2 4 6 8
```

`leave` exits the loop early:

```forth
: find-five ( -- )
    10 0 do
        i 5 = if
            ." found it at " i . cr
            leave
        then
    loop ;

find-five       \ found it at 5
```

### Case

```forth
: day-name ( n -- )
    case
        1 of  ." Monday"    endof
        2 of  ." Tuesday"   endof
        3 of  ." Wednesday" endof
        ." other"
    endcase
    cr ;

2 day-name      \ Tuesday
```

---

## 5. Strings

### String literals

`s"` pushes `( addr u )` — address and length:

```forth
s" Hello, Forth!" type cr
```

`."` in a compiled word prints directly:

```forth
: greet  ." Hello from WF64!" cr ;
greet
```

### String operations

```forth
s" Hello" s" World" s+ type cr    \ HelloWorld   (concatenate)
s" Hello" s-upcase type cr        \ HELLO
s" Hello" s-downcase type cr      \ hello
s" Hello" s-trim type cr          \ Hello (leading/trailing spaces removed)
```

### Managed strings (reference-counted)

For strings that outlive a definition or need to be stored, use the managed string API built on the GC heap:

```forth
s" Hello" ms-create value my-str
my-str ms-print cr

s" , World" my-str ms-append
my-str ms-print cr          \ Hello, World
```

Managed strings are garbage-collected; you do not need to free them explicitly.

---

## 6. Return stack

The return stack (RSP) is the CPU call stack. You can use it as temporary storage within a word, but must restore it before any `exit` or `;`:

```forth
: max3 ( a b c -- max )
    rot            \ c a b  (c pushed to return position)
    max            \ c max(a,b)
    max ;          \ max(c, max(a,b))

3 7 2 max3 .    \ 7
```

`>r` moves a value to the return stack; `r>` moves it back:

```forth
: apply-twice ( x xt -- )
    >r              \ save xt
    dup r@ execute  \ x x xt -- x result
    r> execute ;    \ result xt -- final-result

5 ['] sq apply-twice .    \ 625   (5^2 = 25, 25^2 = 625)
```

Never use `>r` / `r>` across `do`/`loop` boundaries without care — the loop frame lives on the return stack.

---

## 7. File I/O

### Loading source files

```forth
include demos/factorial.f
```

This is the most common file operation — loading and evaluating a Forth source file.

### Handle-based I/O

```forth
s" output.txt" r/w create-file throw value fd

s" Hello, file!" fd write-line throw
s" Line 2" fd write-line throw
fd close-file throw
```

`ior` (I/O result) is 0 on success. `throw` propagates a non-zero ior as an exception.

Reading:

```forth
s" output.txt" r/o open-file throw value fd
pad 256 fd read-line throw         \ ( u flag ior )
drop                                \ drop flag
type cr                             \ print the line
fd close-file throw
```

---

## 8. Exceptions

WF64 implements ANS `catch` / `throw`. This is how error propagation works:

```forth
: safe-divide ( a b -- result )
    dup 0= if  -10 throw  then
    / ;

: try-divide ( a b -- )
    ['] safe-divide catch
    dup if
        ." error: " . cr drop drop
    else
        drop . cr
    then ;

10 2 try-divide     \ 5
10 0 try-divide     \ error: -10
```

`throw` with 0 is a no-op. Negative values are ANS error codes; use positive values for application-defined errors.

---

## 9. The development workflow

### forget_last

When you redefine a word with a bug, `forget_last` removes the most recently compiled definition:

```forth
: bad  99 ;
\ oops
forget_last
: bad  42 ;     \ correct version
```

### .s and the stack as a scratchpad

Use the REPL as a calculator. Values accumulate until you use them:

```forth
2 3 4           \ push three values
+ .s            \ add top two: <2> 2 7
+ .             \ add last two, print: 9
```

### Running a file from the editor

Open a `.f` file with File → Open (`Ctrl+O`), edit it, then press `F5` to evaluate the whole buffer. Output appears in the Console or REPL pane. This is the main loop for developing larger programs.

### The stack viewer

View → Stack (`Ctrl+Shift+K`) shows the current data stack live after every eval. It displays each cell as decimal, hex, and a printable-ASCII interpretation. Leave it open while debugging.

---

## 10. Worked example — Fibonacci

```forth
\ Iterative Fibonacci — no recursion, no stack depth growth

: fib ( n -- fib-n )
    dup 1 <= if exit then      \ fib(0)=0, fib(1)=1
    0 1                        \ a b = fib(0) fib(1)
    2 rot                      \ 0 1 n → a b n
    1- 0 do                    \ n-1 iterations
        over + swap            \ advance: b a+b; then swap: a+b b → b a+b
    loop
    nip ;                      \ drop a, return b

0 fib .     \ 0
1 fib .     \ 1
7 fib .     \ 13
10 fib .    \ 55
```

---

## See also

- [Forth Reference](forth-reference.md) — complete word listing with stack effects
- [IDE Guide](ide-guide.md) — editor, crash dump, Demos menu
- [Keyboard Shortcuts](keyboard-shortcuts.md) — quick-reference table
