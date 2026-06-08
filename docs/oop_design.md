# WF64 — Single-Inheritance Class & Object System (design)

Status: **Phases 0–4 landed & green** — the system is complete: MASM dispatch;
the Forth DSL with inheritance/override/late-bound `->`; `super`; early binding;
and per-class ivar scoping. Full harness suite passes (350 tests, 0 fail). This
document is the contract we implement against. It specifies the surface syntax, the runtime data
structures, the dispatch mechanics, the Forth/MASM split, and a phased,
test-backed implementation plan.

Decisions already taken (with the user):

| Fork | Choice |
|---|---|
| Message-send surface | `obj -> msg` — a parsing send operator |
| Binding | early **and** late hybrid (early when the receiver class is known at compile time, late otherwise) |
| Object allocation | named static objects via `new` (heap/GC objects are documented future work) |

---

## 1. Goals & non-goals

**Goals**

- Single inheritance with method override and `super`.
- Polymorphic, late-bound message send by default; early (direct-call) binding
  when the receiver's class is statically known, with no syntax difference.
- Instance variables that read naturally (`x @`, `5 x !`) and are *scoped* —
  invisible outside the class body, no global namespace pollution, name reuse
  (`x`/`y` in many classes) is fine.
- Method names never collide with core words (`.`, `+`, `type`, …).
- Almost all of the system is Forth; only the message-send hot path is MASM.
- Reads like "higher-level code":

  ```forth
  object subclass shape
    cell ivar: cx   cell ivar: cy
    :m at ( x y -- )   cy !  cx ! ;m
    :m draw ( -- )     ." shape@ " cx @ . cy @ . cr ;m
  end-class

  shape subclass circle
    cell ivar: r
    :m radius! ( n -- )      r ! ;m
    :m draw ( -- )           ." circle r=" r @ . ." @ " cx @ . cy @ . cr ;m
  end-class

  circle new c
  3 4 c -> at    10 c -> radius!    c -> draw
  ```

**Non-goals (v1)**

- Multiple inheritance, mixins, interfaces.
- Multiple dispatch.
- Heap/GC-allocated objects and object lifetime (`dispose`) — designed for later
  (§11), not built now.
- Metaclasses / class-side methods beyond the `new` defining word.

---

## 2. Substrate we build on (already in the tree)

All of these exist today and the design uses them directly:

- **`CREATE`/`DOES>`** with a known body shape. A created word is a 24-byte stub
  that pushes its body address; `DOES>` patches the stub tail into a jump into
  the does-code. See [kernel/compile.masm](kernel/compile.masm) (`create_word`,
  `does_word`, `does_runtime_patch`) and `>body`/`to_body` in
  [kernel/dict.masm](kernel/dict.masm). `create_stub_size` is the body offset.
- **Wordlists & search order** — `forth-wordlist`, `tools-wordlist`,
  `private-wordlist`, `set-current`, `get-current`, `get-order` (+ we add a tiny
  `set-order`/`>order` if not already present). Used for ivar scoping and the
  selector namespace. See [lib/core.f](lib/core.f) and `words-in`.
- **Compile-time helper hook** — `compiles` / `compiles-me` set a word's
  `dh_comp` (its compile-time action). This is exactly how an object word will
  publish its class to the compiler for early binding. See
  [lib/core.f](lib/core.f) (`(comp-cons)`, `value`, `constant`).
- **Field/offset machinery** — Forth-2012 `+field`/`field:` already compute
  struct offsets; `ivar:` is a thin specialization of the same idea.
- **Compile introspection** — `postpone`, `literal`, `compile,`, `'`, `[']`,
  `name>interpret`, `latestxt`, `immediate`.
- **Dictionary type tag** — `dh_tfa` already distinguishes word kinds
  (`tfa_tcre`, `tfa_tcol`). We add tags for *class* and *object* words so tools
  can recognize them.

---

## 3. Surface syntax (the user-facing DSL)

### 3.1 Defining a class

```forth
object subclass <name>          \ <name> inherits from object (the root)
<parent> subclass <name>        \ <name> inherits from <parent>
class <name>                    \ shorthand for:  object subclass <name>
```

Inside the class body, between `subclass`/`class` and `end-class`:

```forth
  <size> ivar: <field>          \ declare an instance variable of <size> bytes
  cell   ivar: <field>          \ the common case (one cell)
  :m <selector> ( stack -- )    \ define / override a method
     ...body... ;m
end-class
```

- `ivar:` works exactly like `field:` but the resulting word resolves against
  the **current receiver** (`self`), so inside a method `x` pushes the address
  of this object's `x`. Inherited ivars keep working (offsets are absolute from
  the object base).
- `:m name ( … ) … ;m` defines a method. The method's selector (`name`) is
  registered globally; any class may implement the same selector — that is what
  makes a send polymorphic. A method overrides the inherited one with the same
  selector.
- Inside a method `self` pushes the receiver; `super -> sel` calls the parent
  class's version of `sel`.

### 3.2 Creating and using objects

```forth
<class> new <name>              \ define a named static object
... <name> -> <selector>        \ send a message (args precede the receiver)
```

Examples:

```forth
counter new c
c -> reset
c -> tick   c -> tick
c -> value .                    \ -> 2

circle new c1
3 4 c1 -> at   10 c1 -> radius!   c1 -> draw
```

Sends compose with normal Forth — the receiver is just a value on the stack, so
a receiver can come from anywhere; only the *form* `<receiver-expr> -> <sel>` is
required:

```forth
: redraw-all ( … -- )  shapes @ each-shape  -> draw ;   \ late-bound
```

---

## 4. Runtime data structures

### 4.1 Class structure (lives in the dict heap)

A class is a CREATE'd word whose body is the **class struct**:

```
offset  name        meaning
  +0    class_super  parent class struct ptr, 0 for the root (object)
  +8    class_isize  instance size in bytes, INCLUDING the +0 class-ptr header cell
 +16    class_vt     vtable base: CAP cells, vtable[sel] = method xt
```

`CAP` = `oop-max-selectors` (a constant, default **256**). Class struct size =
`16 + CAP*8` (≈ 2 KB). The class word pushes the struct base.

Macro additions (see §7):

```
@assign class_super = 0
@assign class_isize = 8
@assign class_vt    = 16
```

### 4.2 Object layout

```
offset  meaning
  +0    class ptr  -> the class struct
  +8    ivar0
 +16    ivar1
  …
```

- `self` = the object base (address of the class-ptr cell).
- An ivar at byte-offset `k` (k ≥ 8) is read with `… k self + …`; the `ivar:`
  word encapsulates this so the user just writes `x`.
- Instance size (`class_isize`) includes the +0 header cell; the first ivar of a
  direct subclass of `object` sits at +8 because `object`'s `isize` is 8.

### 4.3 Selectors (the global message namespace)

- A monotonically increasing counter `#selectors` (a `variable`) allocates a
  unique small integer **selector-id** to each *distinct* method name the first
  time it is seen anywhere.
- Selector names live in a dedicated **`selectors` wordlist**. Each entry is a
  CREATE word whose body cell holds its selector-id. This reuses the dictionary
  for name→id resolution but keeps selectors out of the normal search order, so
  `.` as a *method name* never shadows `.` the number printer.
- `:m show …` and `-> show` both resolve `show` in the `selectors` wordlist;
  `:m` allocates a new id if the name is new.

### 4.4 vtable & inheritance (copy-down)

- The **root `object`** vtable is initialized with every slot pointing at
  `(dnu)` — "does not understand" — which throws a clear, named error.
- `subclass` allocates the child struct and **copies the parent's entire vtable
  (all CAP cells) down** into the child, then sets `class_super`/`class_isize`.
- Each `;m` writes `child.vtable[sel] = method-xt` for that method's selector,
  overriding the inherited entry.
- Result: dispatch is a flat, O(1) indexed load — no parent-chain walk at
  send time. A selector a class doesn't implement resolves to the nearest
  ancestor's method, or `(dnu)` if none.

Trade-off: fixed-CAP vtables cost ~2 KB/class and cap the system at `CAP`
distinct selectors. Both are generous for our use and keep dispatch to two
loads. §11 notes a compact (exact-size) variant if we ever need it.

### 4.5 `self`

`self` is the current receiver, stored in a **user-area cell `user_SELF`** for
speed. The send primitive saves the previous `self` on the return stack, sets it
to the receiver for the duration of the method, and restores it on return —
fully reentrant across nested sends and robust to `EXIT` inside a method
(because the save/restore bracket lives in the *send* frame, not the method).

---

## 5. Dispatch mechanics

### 5.1 Calling convention

A method's compiled body has the signature `( i*x -- j*x )` — the **receiver is
not on the data stack**; it has been moved into `self` by the send primitive.
Args that were below the receiver remain on the stack in order. So
`3 4 c -> at` runs `at ( x y -- )` with `x=3 y=4` and `self=c`.

### 5.2 The send primitives (MASM)

Two primitives share one self-bracket. Late binding looks the xt up in the
vtable; early binding is handed the xt directly.

```
; (send)  ( i*x obj sel -- j*x )       late bind: look up in obj's class vtable
proc(send_word)
    mov   rcx, [DSP]                   ; rcx = obj (receiver)
    mov   rdx, [rcx]                   ; rdx = class struct (obj+0)
    mov   r8,  [rdx + class_vt + rax*cell]   ; r8 = method xt (rax = sel-id = TOS)
    mov   TOS, [DSP + cell]            ; raise: drop obj+sel, expose first arg
    add   DSP, 2*cell
    jmp   send_common                  ; r8 = xt, rcx = obj
endp()

; (send-xt)  ( i*x obj xt -- j*x )     early bind: xt already resolved
proc(send_xt_word)
    mov   r8, rax                      ; r8 = xt (TOS)
    mov   rcx, [DSP]                   ; rcx = obj
    mov   TOS, [DSP + cell]
    add   DSP, 2*cell
    ; fallthrough
proc(send_common)                      ; r8 = method xt, rcx = obj
    mov   r9, [UP + user_SELF]         ; old self
    mov   [UP + user_SELF], rcx        ; self := obj
    ; rstack on entry holds our caller's return address. Build a frame so the
    ; method returns into .restore, which restores self and returns to caller.
    push  r9                           ; [old_self][caller_ret]
    lea   r10, [rip + send_restore]
    push  r10                          ; [.restore][old_self][caller_ret]
    jmp   r8                           ; tail into method; its RET pops .restore
endp()

proc(send_restore)
    pop   r9                           ; old_self
    mov   [UP + user_SELF], r9
    ret                                ; -> caller_ret
endp()

; self  ( -- obj )
proc(self_word)
    mov   [DSP - cell], TOS
    mov   TOS, [UP + user_SELF]
    stk(0, 1)
    next()
endp()
```

Notes:
- No caller-saved register is held across the method call; `self` lives in
  memory and the old value rides the return stack — so nested sends and
  recursion are safe, and a method may freely use `>r`/`r>` and `EXIT`.
- `(dnu)` (the default vtable entry) ignores `self`/args and `THROW`s a
  dedicated code (proposed **-2058**, "message not understood"), so an unknown
  message fails loudly instead of crashing.

### 5.3 `->` — the send operator (Forth, immediate parsing word)

`->` parses the selector name and resolves its id in the `selectors` wordlist
(throwing -2058-ish if unknown). Then:

- **Interpret state** (REPL): the receiver is on TOS. `->` performs the send
  immediately via `(send)` — always late-bound (binding choice is invisible at
  the REPL and not worth optimizing).
- **Compile state**: choose early vs late from a compile-time receiver hint:
  - If the *immediately preceding* compiled item was a class-aware word (a named
    object via `new`, or `super`), it recorded its class in `recv-class` and the
    post-push HERE in `recv-here`. If `recv-here = HERE` and `recv-class` is set,
    **early-bind**: read `recv-class.vtable[sel]` now and compile
    `… <method-xt> (send-xt)`.
  - Otherwise **late-bind**: compile `… <sel-id> (send)`.

Reference sketch:

```forth
: ->  ( "selector" -- )                      \ immediate
    parse-name selector-id                   ( sel )       \ resolve or throw
    state @ if
        recv-here @ here = recv-class @ 0<> and if         \ early
            recv-class @ swap vt@                ( xt )     \ class.vtable[sel]
            postpone literal  postpone (send-xt)
            0 recv-here !
        else                                                \ late
            postpone literal  postpone (send)
        then
    else
        (send)                                              \ interpret: TOS=obj
    then ; immediate
```

The receiver hint is produced by giving each `new` object word a custom
`dh_comp` via `compiles-me`:

```forth
: (comp-object) ( xt -- )            \ compile-time action for object words
    >body                            ( obj-base )
    dup postpone literal             \ compile the push of the object address
    @ recv-class !                   \ class = [obj]; record the hint
    here recv-here ! ;
```

`recv-here` is monotone-safe: HERE only increases within a definition, so a
stale hint from an earlier word can never equal the current HERE — the early
path triggers only when an object/`super` was the last thing compiled.

### 5.4 `super`

`super -> sel` invokes the parent's `sel` with `self` unchanged. `super` is an
immediate word usable only inside a method; it pushes `self` as the receiver and
sets the hint to the *statically known* parent class, so the following `->`
early-binds against the parent's vtable (never re-entering an override):

```forth
: super  ( -- )                      \ immediate, inside :m only
    postpone self                    \ receiver = self
    current-class @ class_super + @  recv-class !
    here recv-here ! ; immediate
```

`current-class` is a compile-time variable holding the class being defined,
set by `subclass`/`class` and cleared by `end-class`.

---

## 6. The defining words (Forth)

All of these are ordinary Forth built on §2. Sketches (final form may differ):

```forth
\ --- selector registry -------------------------------------------------
variable #selectors                  0 #selectors !
\ selectors live in their own wordlist `selectors-wl` (a fresh wordlist)

: selector-id ( c-addr u -- sel )    \ resolve, allocating if new
    2dup selectors-wl search-wl  if  nip nip  >body @  exit  then
    \ not found: define a new selector entry holding the next id
    get-current >r  selectors-wl set-current
    create-from-string  #selectors @ ,  r> set-current
    #selectors @  1 #selectors +! ;

\ --- class construction ------------------------------------------------
: vt@ ( class sel -- xt )   cells + class_vt + @ ;
: vt! ( xt class sel -- )   cells + class_vt + ! ;

: make-class ( parent "name" -- )    \ shared by subclass; parent=0 for object
    create  here ( struct )
      over ,                         \ class_super = parent
      ... class_isize from parent ...
      ... copy parent vtable, or fill with (dnu) if parent=0 ...
    \ tag the class word; arm class-body state (ivars wordlist into order,
    \ current-class := struct, save search order)
;

: subclass ( parent "name" -- )  make-class ;
: class    ( "name" -- )         object subclass ;

: end-class ( -- )               \ restore search order & current; clear current-class

\ --- instance variables ------------------------------------------------
\ ivar words live in the per-class ivars wordlist (in the search order only
\ during the class body). Newest-wins shadowing lets many classes reuse `x`.
: ivar: ( offset size "name" -- offset' )
    create  over ,                   \ store this field's byte offset
    +                                \ advance the class's isize accumulator
    does>  @ self + ;                \ runtime: push self + offset

\ --- methods -----------------------------------------------------------
: :m ( "selector" -- )               \ start a method body
    parse-name selector-id  >r       \ R: sel
    here  current-class @  r> vt!     \ class.vtable[sel] := this body xt
    ]  ... enter compile state, no header ... ;
: ;m ( -- )  postpone exit  ... leave compile state ... ; immediate
\ (or reuse the existing `:`/`;` colon machinery with a :noname-style body)

\ --- instantiation -----------------------------------------------------
: new ( class "name" -- )
    create  ( class )                \ <name> will push its body (the object)
    dup ,                            \ cell 0 = class ptr
    class_isize @  cell -  here over erase  allot   \ zero & reserve ivars
    latestxt ['] (comp-object) compiles  \ arm early-binding hint
    drop ;
```

(The exact wiring of `:m`/`;m` onto the existing colon compiler, and the ivar
wordlist push/pop, are settled in Phase 1; the semantics above are fixed.)

---

## 7. Forth / MASM split

**MASM — the message-send hot path only** (new code in a new
`kernel/oop.masm`, `@include`d from [kernel/main.masm](kernel/main.masm)):

| Symbol | Forth name | Purpose |
|---|---|---|
| `send_word` | `(send)` | late-bound dispatch: class → vtable[sel] → bracket+execute |
| `send_xt_word` | `(send-xt)` | early-bound dispatch: xt given, skip the lookup |
| `send_common`/`send_restore` | — | shared self-bracket (internal) |
| `self_word` | `self` | push the current receiver |

Macro additions in [kernel/macros.masm](kernel/macros.masm): `class_super`,
`class_isize`, `class_vt`, and one free user-area cell `user_SELF`. Three
`PRIMITIVES` rows in [src/lib.rs](src/lib.rs): `(send)`, `(send-xt)`, `self`.

**Everything else is Forth**, in a new `lib/oop.f` loaded after
[lib/core.f](lib/core.f): `class`, `subclass`, `object`, `ivar:`, `:m`, `;m`,
`end-class`, `new`, `super`, `->`, `selector-id`, `(dnu)`, `(comp-object)`,
the selector wordlist, and the class/ivar bookkeeping.

Rationale: the defining DSL is all dictionary/compile manipulation that Forth
already does well; only `->`'s inner action (executed on every message at
runtime) deserves to be a few instructions rather than a chain of colon calls.
This matches the project rule: *prefer a colon definition or runtime fn before
adding a new proc; add only the smallest bootstrap-critical MASM.* A pure-Forth
`(send)` is possible and will be kept as a reference/bootstrap fallback, but the
shipped hot path is MASM.

---

## 8. Edge cases & decisions

- **Method-name collisions with core words** — impossible to trigger
  accidentally: method names are only ever parsed by `->`/`:m`/`super` and
  resolved in the `selectors` wordlist; they are never standalone executable
  words. `:m . ( -- ) … ;m` is fine.
- **Unknown message** — `(dnu)` throws -2058 (proposed) with the selector name,
  not a crash.
- **`EXIT` / early return inside a method** — safe; `self` save/restore brackets
  the send, not the method body, so an `EXIT` just returns into `send_restore`.
- **Recursion / re-entrant sends** — safe; each send saves the previous `self`
  on the return stack.
- **`->` in interpret vs compile** — same semantics; only the binding strategy
  differs and only as a speed optimization.
- **Selector cap** — `CAP=256` distinct selectors; exceeding it throws at `:m`
  time. Bump the constant if needed (rebuild).
- **Adding a method to a class after a subclass exists** — not supported;
  classes are closed at `end-class` (the subclass already copied the vtable).
  This is the standard copy-down limitation and is acceptable for v1.
- **`>body` on an object/class word** — returns the object/class struct base, so
  existing tools keep working; new `dh_tfa` tags let tools label them.

---

## 9. Worked example (polymorphism + super)

```forth
object subclass animal
  cell ivar: legs
  :m legs! ( n -- )   legs ! ;m
  :m speak ( -- )     ." ...generic noise..." cr ;m
  :m describe ( -- )  ." an animal with " legs @ . ." legs: " self -> speak ;m
end-class

animal subclass dog
  :m speak ( -- )     ." woof" cr ;m
  :m describe ( -- )  ." (dog) " super -> describe ;m
end-class

dog new rex
4 rex -> legs!
rex -> describe
\ prints:  (dog) an animal with 4 legs: woof
```

- `rex -> describe` late-binds to `dog`'s `describe`.
- `super -> describe` early-binds to `animal`'s `describe` (no infinite loop).
- inside `animal`'s `describe`, `self -> speak` late-binds back to `dog`'s
  `speak` (`self` is still `rex`) — classic virtual dispatch.

---

## 10. Implementation plan (phased, test-first)

Each phase lands with `tests/data/direct/*.t` and/or `tests/data/eval/*.in/.out`
pairs, per the project's NYIMP-driven loop.

**Phase 0 — MASM substrate** ✅ done
- `user_SELF` (0x17F0) + `class_super/isize/vt` macros in
  [kernel/macros.masm](kernel/macros.masm); `self`, `(send)`, `(send-xt)` (plus
  internal `send_enter`/`send_restore`) in [kernel/oop.masm](kernel/oop.masm);
  `@include`d from [kernel/main.masm](kernel/main.masm); 3 rows in `PRIMITIVES`
  ([src/lib.rs](src/lib.rs)).
- Tests: `tests/data/direct/self.t` (top-level receiver is 0);
  `tests/data/eval/oop_send.in` (late `(send)` + early `(send-xt)` both reach a
  method that reads an ivar off `self`); `tests/data/eval/oop_self_nesting.in`
  (nested send proves self save/restore — 44, not 55). All green.

**Phase 1 — class/object core (Forth)** ✅ done
- [lib/oop.f](lib/oop.f): `selector-id` + flat selector table; `object` root;
  `subclass`/`class`/`end-class`; `ivar:`; `:m`/`;m`; `new`; `(dnu)`; late-bound
  `->`. Loaded at boot right after `core.f` ([src/lib.rs](src/lib.rs)).
- Inheritance (vtable copy-down in `subclass`) and method override work in this
  phase already; `super` is Phase 2.
- Tests (eval): `oop_counter` (init/bump/val), `oop_polymorphism` (sq/tri share
  `area`, late-bound to different results), `oop_dnu` (unknown message →
  `THROW -2058`, caught). All green.
- **Two deviations from the original plan, for reset-safety in the shared-session
  harness** (`reset()` only snapshots the forth/tools/private bucket arrays):
  - *Selectors* use a flat name→id buffer allocated below the boot fence
    (stable across `reset()`), not a dedicated wordlist that would dangle.
  - *Ivar words* are created in the forth-wordlist (which `reset()` restores),
    so per-class ivar-name scoping is deferred to Phase 4. Functionally ivars
    work (newest-wins resolves the class's own ivars during its method compile);
    they're just globally visible within a session until reset.
  - Loading `oop.f` at boot shifts `boot_here`; this surfaced a brittle
    `float_literals` eval test that hard-coded an alignment-padding constant
    (`faligned here -`). Fixed it to assert the real invariant
    (`faligned 7 and = 0`) instead.

**Phase 2 — `super`** ✅ done
- `super` (immediate, in `:m`) compiles a push of `self` and hints the parent
  class, so the following `->` early-binds against the parent vtable — no
  re-entry into the override, no infinite loop. (Inheritance/override copy-down
  already landed in Phase 1's `subclass`.)
- Test: `tests/data/eval/oop_super.in` — `super -> noise` reaches `animal`'s
  method (→104), while `self -> speak` inside an inherited method virtually
  dispatches back to `dog`'s override (→9).

**Phase 3 — early binding** ✅ done
- Compile-time receiver hint in the user area (`user_OOP_RECV_CLASS/HERE`,
  zeroed by `reset()`); `(comp-object)` arms it for named objects via
  `compiles-me`; `->` early-binds (`(send-xt)`, no vtable lookup) when the hint
  is fresh (`rec-here = here`), else late-binds (`(send)`).
- Tests: `oop_early.in` (compiled named-object sends); `oop_early_proof.in`
  proves early binding actually fires — after nulling the object's class
  pointer, an early-bound send still returns the right value (a late `(send)`
  would crash reading the null class).
- Debugging note: stdin-pipe testing gives unreliable first-line artifacts;
  validate the OOP layer through the harness (`s.eval`), not `| wf64`.

**Phase 4 — ivar scoping** ✅ done
- ivar words now live in a dedicated `ivars-wl` word list that is spliced to the
  front of the search order only while a class body compiles, then removed at
  `end-class` — so ivar names are scoped to their class and invisible elsewhere
  (`[defined] secret` is 0 after the class), and may be reused across classes.
- Made reset-safe the proper way (see [docs/vocabularies.md](docs/vocabularies.md)):
  `oop.f` publishes the wid to `user_OOP_IVARS_WID`; `src/lib.rs` snapshots its
  buckets at boot and restores them in `reset()`, like FORTH/TOOLS/PRIVATE.
- Test: `tests/data/eval/oop_scope.in` — method resolves the ivar (→42), but
  `[defined] secret` outside the class is 0.

Still nice-to-have (not blocking): `class?`/`object?` predicates, `.class`,
method listing, a `demos/oop-shapes.f`, and full early-binding inlining.

---

## 11. Future work (explicitly out of v1)

- **Heap/GC objects**: `<class> heap-new ( -- obj )` allocating via the newgc
  heap, `obj -> dispose`, object references in GC roots. The class/object/vtable
  model above is allocation-agnostic; only `new` changes.
- **Full early-binding inlining**: fold `self`-setup + a leaf method body inline
  using the existing inline-compiler (`(inline,)`, the `inline` declarator).
- **ivar access inlining**: compile `x @` to a single `mov` off `self` instead
  of a `does>` call.
- **Compact vtables**: size each vtable to `#selectors`-at-`end-class` with a
  `nsel` bound, trading a bounds check for memory if `CAP` ever bites.
- **Class-side methods / constructors with args** and an auto-`init` convention.

---

## 12. Items confirmed / still open

**Confirmed against the current tree:**

- `wordlist`, `also`, `previous`, `set-order`, `search-wordlist` all exist in
  `PRIMITIVES` ([src/lib.rs](src/lib.rs)) — the ivar-scoping and selector-wordlist
  plumbing needs no new dictionary primitives.
- Free user-area cell for `user_SELF`: **`0x17F0`** — the first cell after
  `user_LITERAL_NEXT` (0x17E8) and below the `0x2000` heap region in
  [kernel/macros.masm](kernel/macros.masm). Ample contiguous space follows if a
  second OOP user cell is ever needed.

**Still open (settle in Phase 0/1):**

- Whether to reuse the existing `:`/`;` colon machinery for `:m`/`;m` (preferred)
  vs a bespoke `:noname`-based body.
- Final THROW code for "message not understood" (proposed -2058) — confirm it's
  unused.
