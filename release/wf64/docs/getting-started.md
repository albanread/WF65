# Getting Started

## Running WF64

### GUI IDE (recommended)

Double-click **`wf64-ui.exe`**. The MDI frame opens with the Console pane already active and the `>` prompt waiting for input. The Forth system loads `lib/core.f` automatically before the prompt appears — all standard words are available immediately.

---

## The Console pane

When the GUI starts the Console pane opens automatically (View → Console, `Ctrl+Shift+R`). It shows the `>` prompt at the bottom of a continuous scrollback. Type Forth at the prompt and press Enter to evaluate.

The REPL pane (View → REPL, `Ctrl+Shift+P`) is an alternative split-pane view with a separate transcript area and input strip. Both talk to the same Forth session.

---

## First session

```forth
2 3 + .         \ prints 5
.s              \ show the stack (should be empty after .)
: sq ( n -- n^2 ) dup * ;
5 sq .          \ prints 25
```

After each submitted line the IDE appends ` ok` to the transcript (Forth convention).

### Stack display

`.s` prints the stack contents without consuming them, bottom-first, enclosed in `<depth> `:

```forth
1 2 3 .s        \ <3> 1 2 3
```

Use this constantly during development — it is the primary diagnostic.

### Defining words

```forth
: double ( n -- 2n ) 2 * ;
: greet  ( -- )  ." Hello, Forth!" cr ;
greet
```

The `: name ... ;` form compiles `name` into the dictionary. After the `;` the word is immediately available.

### Rolling back a definition

If a newly defined word has a bug, `forget_last` removes it from the dictionary without disturbing anything else:

```forth
: bad-word 99999 ;
forget_last
\ bad-word no longer exists
```

`forget_last` is a development convenience; it removes exactly the last header that was linked into the dictionary.

---

## Startup loading

`lib/core.f` is loaded at startup before the REPL prompt appears. It defines higher-level ANS words on top of the assembled kernel primitives: `variable`, `constant`, pictured-numeric output, `u.`, `s"`, `."`, and more. You can add your own startup code at the end of that file, or use `include` to load additional files:

```forth
include demos/hello.f
```

---

## Quitting

```forth
bye
```

This calls the `bye` primitive, which posts a frame-close event and exits the process cleanly. In the GUI you can also use File → Exit or `Alt+F4`.

---

## Next steps

- [Forth Reference](forth-reference.md) — all the core words with stack effects
- [IDE Guide](ide-guide.md) — crash recovery, log view, editor, menus
- [Keyboard Shortcuts](keyboard-shortcuts.md) — quick-reference table
