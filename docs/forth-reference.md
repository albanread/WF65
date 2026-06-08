# Forth Reference

Quick reference for the WF64 core vocabulary. Stack effects use the notation `( before -- after )`. `n` = signed cell, `u` = unsigned cell, `f` = flag (0 or -1), `c` = character, `addr` = address, `x` = any cell.

---

## Stack manipulation

| Word | Stack effect | Description |
|---|---|---|
| `dup` | `( x -- x x )` | Duplicate top |
| `drop` | `( x -- )` | Discard top |
| `swap` | `( x1 x2 -- x2 x1 )` | Swap top two |
| `over` | `( x1 x2 -- x1 x2 x1 )` | Copy second to top |
| `rot` | `( x1 x2 x3 -- x2 x3 x1 )` | Rotate third to top |
| `nip` | `( x1 x2 -- x2 )` | Drop second |
| `tuck` | `( x1 x2 -- x2 x1 x2 )` | Copy top under second |
| `2dup` | `( x1 x2 -- x1 x2 x1 x2 )` | Duplicate top pair |
| `2drop` | `( x1 x2 -- )` | Drop top pair |
| `2swap` | `( x1 x2 x3 x4 -- x3 x4 x1 x2 )` | Swap top two pairs |
| `2over` | `( x1 x2 x3 x4 -- x1 x2 x3 x4 x1 x2 )` | Copy second pair to top |
| `depth` | `( -- n )` | Number of items on stack |
| `noop` | `( -- )` | No operation |

---

## Arithmetic

| Word | Stack effect | Description |
|---|---|---|
| `+` | `( n1 n2 -- n3 )` | Add |
| `-` | `( n1 n2 -- n3 )` | Subtract |
| `*` | `( n1 n2 -- n3 )` | Multiply (low cell) |
| `/` | `( n1 n2 -- n3 )` | Divide (signed) |
| `mod` | `( n1 n2 -- n3 )` | Remainder (signed) |
| `/mod` | `( n1 n2 -- rem quot )` | Divide and remainder |
| `negate` | `( n -- -n )` | Negate |
| `abs` | `( n -- \|n\| )` | Absolute value |
| `max` | `( n1 n2 -- n3 )` | Larger of two |
| `min` | `( n1 n2 -- n3 )` | Smaller of two |
| `1+` | `( n -- n+1 )` | Increment |
| `1-` | `( n -- n-1 )` | Decrement |
| `2+` | `( n -- n+2 )` | Add 2 |
| `2-` | `( n -- n-2 )` | Subtract 2 |
| `2*` | `( n -- n*2 )` | Arithmetic left shift |
| `2/` | `( n -- n/2 )` | Arithmetic right shift |
| `cell+` | `( addr -- addr+8 )` | Advance by one cell |
| `cells` | `( n -- n*8 )` | Cells to bytes |
| `um*` | `( u1 u2 -- ud )` | Unsigned double multiply |
| `um/mod` | `( ud u -- rem quot )` | Unsigned double divide |

---

## Comparison

| Word | Stack effect | Description |
|---|---|---|
| `=` | `( x1 x2 -- f )` | Equal |
| `<>` | `( x1 x2 -- f )` | Not equal |
| `<` | `( n1 n2 -- f )` | Less than (signed) |
| `>` | `( n1 n2 -- f )` | Greater than (signed) |
| `<=` | `( n1 n2 -- f )` | Less or equal (signed) |
| `>=` | `( n1 n2 -- f )` | Greater or equal (signed) |
| `0=` | `( n -- f )` | Equal to zero |
| `0<>` | `( n -- f )` | Not equal to zero |
| `0<` | `( n -- f )` | Negative |
| `0>` | `( n -- f )` | Positive |
| `u<` | `( u1 u2 -- f )` | Less than (unsigned) |
| `u>` | `( u1 u2 -- f )` | Greater than (unsigned) |
| `within` | `( n lo hi -- f )` | `lo <= n < hi` |

Flags are -1 (true) or 0 (false) in WF64, following ANS convention.

---

## Logic

| Word | Stack effect | Description |
|---|---|---|
| `and` | `( x1 x2 -- x3 )` | Bitwise AND |
| `or` | `( x1 x2 -- x3 )` | Bitwise OR |
| `xor` | `( x1 x2 -- x3 )` | Bitwise XOR |
| `invert` | `( x -- ~x )` | Bitwise NOT |
| `lshift` | `( x n -- x' )` | Logical left shift |
| `rshift` | `( x n -- x' )` | Logical right shift |
| `arshift` | `( x n -- x' )` | Arithmetic right shift |

---

## Memory

| Word | Stack effect | Description |
|---|---|---|
| `@` | `( addr -- x )` | Fetch cell (8 bytes) |
| `!` | `( x addr -- )` | Store cell |
| `c@` | `( addr -- c )` | Fetch byte |
| `c!` | `( c addr -- )` | Store byte |
| `2@` | `( addr -- x1 x2 )` | Fetch double cell |
| `2!` | `( x1 x2 addr -- )` | Store double cell |
| `+!` | `( n addr -- )` | Add to cell in memory |
| `fill` | `( addr u c -- )` | Fill `u` bytes with `c` |
| `move` | `( src dst u -- )` | Copy `u` bytes (handles overlap) |
| `cmove` | `( src dst u -- )` | Copy `u` bytes forward |
| `here` | `( -- addr )` | Current dictionary pointer |
| `allot` | `( n -- )` | Advance `here` by `n` bytes |

---

## Control flow

### Colon definitions

```forth
: name  ( stack-effect )
    ...body...
;
```

Compiles `name` into the dictionary. `;` ends compilation and links the header.

### Conditionals

```forth
flag if
    \ executed when flag is non-zero (true)
then

flag if
    \ true branch
else
    \ false branch
then
```

### Begin loops

```forth
\ post-test (execute body at least once)
begin
    ...body...
flag until           \ exit when flag is non-zero

\ pre-test (may not execute body)
begin
flag while
    ...body...
repeat
```

### Counted loops

```forth
limit start do
    i .              \ i is the loop index
loop

limit start do
    i .
2 +loop              \ step by 2 each iteration

limit start do
    dup 0= if leave then   \ leave exits the loop early
    1-
loop
```

### Execute

```forth
' word execute       \ execute a word by xt
```

---

## I/O

| Word | Stack effect | Description |
|---|---|---|
| `.` | `( n -- )` | Print signed decimal + space |
| `u.` | `( u -- )` | Print unsigned decimal + space |
| `emit` | `( c -- )` | Emit one character |
| `cr` | `( -- )` | Emit newline |
| `space` | `( -- )` | Emit a space |
| `spaces` | `( n -- )` | Emit `n` spaces |
| `type` | `( addr u -- )` | Print string of `u` chars |
| `.s` | `( -- )` | Print stack contents (non-destructive) |
| `."` | `( -- )` | Compile-time: print literal string |
| `s"` | `( -- addr u )` | Compile-time: string literal |
| `key` | `( -- c )` | Read one character |
| `accept` | `( buf max -- n )` | Read up to `max` chars into buf |

Example:

```forth
: greet  ." Hello from WF64!" cr ;
: show-n ( n -- ) ." n = " . cr ;
```

---

## Defining words

| Word | Stack effect | Description |
|---|---|---|
| `create` | `( "name" -- )` | Create a dictionary entry with no defined behavior; body follows `here` |
| `variable` | `( "name" -- )` | Allocate a cell variable |
| `2variable` | `( "name" -- )` | Allocate a double-cell variable |
| `constant` | `( n "name" -- )` | Compile-time constant |
| `allot` | `( n -- )` | Reserve `n` bytes in the dictionary |
| `,` | `( x -- )` | Append a cell to the dictionary |
| `c,` | `( c -- )` | Append a byte to the dictionary |

```forth
variable counter
0 counter !
counter @ 1+ counter !
counter @ .           \ prints 1

42 constant answer
answer .              \ prints 42
```

---

## File I/O

| Word | Stack effect | Description |
|---|---|---|
| `open-file` | `( addr u fam -- fd ior )` | Open file by name; fam: 0=read, 1=write, 2=read/write |
| `create-file` | `( addr u fam -- fd ior )` | Create or truncate file |
| `close-file` | `( fd -- ior )` | Close file descriptor |
| `read-file` | `( addr u fd -- u2 ior )` | Read up to `u` bytes |
| `read-line` | `( addr u fd -- u2 flag ior )` | Read one line |
| `write-file` | `( addr u fd -- ior )` | Write `u` bytes |
| `write-line` | `( addr u fd -- ior )` | Write line + newline |
| `flush-file` | `( fd -- ior )` | Flush write buffers |
| `file-position` | `( fd -- ud ior )` | Current position |
| `reposition-file` | `( ud fd -- ior )` | Seek to position |
| `file-size` | `( fd -- ud ior )` | File size in bytes |
| `delete-file` | `( addr u -- ior )` | Delete file by name |
| `rename-file` | `( addr u addr2 u2 -- ior )` | Rename file |
| `include` | `( "filename" -- )` | Load and evaluate a source file |
| `included` | `( addr u -- )` | Load and evaluate by address/length |

`ior` is 0 on success, non-zero on error (maps to a Windows error code).

---

## Dynamic memory

| Word | Stack effect | Description |
|---|---|---|
| `allocate` | `( u -- addr ior )` | Allocate `u` bytes on the heap |
| `free` | `( addr -- ior )` | Free a heap allocation |
| `resize` | `( addr u -- addr2 ior )` | Resize a heap allocation |

---

## Return stack

| Word | Stack effect | Description |
|---|---|---|
| `>r` | `( x -- ) ( R: -- x )` | Move TOS to return stack |
| `r>` | `( -- x ) ( R: x -- )` | Move top of return stack to data stack |
| `r@` | `( -- x ) ( R: x -- x )` | Copy top of return stack |
| `rdrop` | `( -- ) ( R: x -- )` | Discard top of return stack |
| `2>r` | `( x1 x2 -- ) ( R: -- x1 x2 )` | Move pair to return stack |
| `2r>` | `( -- x1 x2 ) ( R: x1 x2 -- )` | Move pair from return stack |

The return stack is the CPU call/ret stack (STC Forth). The `>r`/`r>`/`rdrop` primitives juggle the return address around the user value — do not mix with raw `>r` inside a `do/loop` without care.

---

## Pictured numeric output

```forth
: .hex  ( u -- )  base @ swap  16 base !  u.  base ! ;
```

Lower-level: `<#` ... `#s` ... `#>` build a number string in the pad; `type` prints it.

---

## See also

- [Getting Started](getting-started.md) — REPL basics and startup loading
- [IDE Guide](ide-guide.md) — crash recovery and the other panes
- [Keyboard Shortcuts](keyboard-shortcuts.md) — shortcut reference
