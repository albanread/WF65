# IDE Guide

WF64's GUI is a Windows MDI application rendered with Direct2D and DirectWrite. Every pane is an independent MDI child window; arrange them however you like with the standard `Window` menu verbs or by dragging title bars.

The title bar uses the `∴` (therefore, U+2234) prefix — three dots stacked vertically, visually a Forth data stack.

---

## Console pane

View → Console (`Ctrl+Shift+R`)

The console is a continuous scrollback terminal. Output from the Forth worker accumulates at the top; the current input line sits at the bottom. This is the most direct analogue of a classic Forth terminal.

**Keyboard (console input line)**

| Key | Action |
|---|---|
| Enter | Submit current line to the Forth worker |
| Backspace / Delete | Delete character left / right of cursor |
| Left / Right | Move cursor |
| Home / End | Jump to start / end of input |
| Up / Down | Cycle through input history |
| Ctrl+L | Clear scrollback |
| Ctrl+U | Clear the current input line |
| Mouse wheel | Scroll history |

History is process-wide and survives a Forth restart.

---

## REPL pane

View → REPL (`Ctrl+Shift+P`)

The REPL pane is a split-pane variant: a read-only **transcript** area above a draggable splitter, and a multi-line **input editor** below. This gives more screen real estate to the output while keeping the input in a fixed location.

**Transcript area**

Scrollable; shows all submitted input and the resulting output interleaved with ` ok` lines. Scroll with the mouse wheel or Page Up / Page Down keys.

**Input editor**

Multi-line; Enter submits when the input looks syntactically complete. Shift+Enter forces a newline without submitting (useful for multi-line definitions).

**Keyboard (input editor)**

| Key | Action |
|---|---|
| Enter | Submit (if complete) |
| Shift+Enter | Insert newline in input without submitting |
| Up / Down | History recall (when cursor is on first/last line) |
| Ctrl+L | Clear transcript |
| Ctrl+A | Move cursor to start of input |
| Ctrl+E | Move cursor to end of input |
| Ctrl+K | Kill from cursor to end of line |
| Ctrl+U | Kill from cursor to start of line |
| Ctrl+C | Copy input to clipboard (or last transcript line if input is empty) |
| Ctrl+Shift+C | Copy entire transcript as plain text |
| Ctrl+X | Cut input to clipboard and clear it |
| Ctrl+V | Paste clipboard text at cursor |
| Ctrl+Left / Right | Move cursor by whole word |
| Home / End | Move to start / end of current line |
| Page Up / Page Down | Scroll transcript |

---

## Stack viewer

View → Stack (`Ctrl+Shift+K`)

Shows the current Forth data stack, top-of-stack first. Updated automatically after each eval. Each row shows the cell value in decimal, hexadecimal, and an ASCII/interpretation column. Read-only; scroll with the mouse wheel.

---

## Log view

View → Log (`Ctrl+Shift+L`)

A diagnostic ring buffer (capped entries, newest at top). The Forth runtime and the iGui layer write diagnostic messages here. Adjacent repeated messages coalesce into a single entry with a repeat count. Log entries survive a Forth worker crash — you can see what was happening in the moments before a fault. Read-only; scroll with the mouse wheel.

---

## Crash dump pane

View → Crash Dump (`Ctrl+Shift+X`)

The crash dump pane is normally empty. It opens automatically when:

- The Forth worker thread takes an SEH exception (access violation, illegal instruction, etc.) that is caught by the vectored exception handler, or
- The Forth worker panics (Rust panic, which is also caught and reported here).

**What it shows**

Each crash report contains:

- A header line with the fault type and address
- A register snapshot: RAX (TOS), RBP (DSP), RBX (UP), RSP, R12, and the full general-purpose register file
- A stack-words listing: the top N values on the data stack decoded as cell integers, with hex alongside

**After a crash**

The Forth worker thread has exited. The supervisor thread detects this, displays the dump, then spawns a fresh worker with a new `Wf64Session` (fresh kernel, fresh dictionary). The IDE remains usable. Close the crash-dump pane manually with the MDI close button if you want to reclaim the screen space.

**Clearing the dump**

Within the crash-dump pane, `Ctrl+L` clears all stored dumps.

---

## Editor pane (fedit)

File → New (`Ctrl+N`) opens a text-editor MDI child. This is `fedit` — a rope-based editor with syntax highlighting for Forth source.

| Action | Key |
|---|---|
| Open file | `Ctrl+O` |
| Save | `Ctrl+S` |
| Save As | `Ctrl+Shift+S` |
| Run buffer (eval entire file) | `F5` |
| Undo | `Ctrl+Z` |
| Redo | `Ctrl+Y` |
| Cut / Copy / Paste | `Ctrl+X` / `Ctrl+C` / `Ctrl+V` |
| Select All | `Ctrl+A` |
| Word right / left | `Ctrl+Right` / `Ctrl+Left` |

Pressing F5 sends the editor's entire content to the Forth worker as an `EvalBuffer` event. Output appears in the Console or REPL transcript. This is the main workflow for developing Forth source files: edit in `fedit`, F5 to reload.

---

## Demos menu

If a `demos/` directory exists alongside the executable, WF64 auto-populates a **Demos** menu with one entry per `.f` file found there. Selecting an entry runs that file through the Forth worker. This is useful for distributing example scripts alongside the binary.

---

## Forth menu

| Item | Key | Action |
|---|---|---|
| Restart | `Ctrl+Shift+F5` | Tear down the current session and boot a fresh one |
| Run Buffer | `F5` | Evaluate the active editor buffer |

---

## Window menu (MDI verbs)

Standard Win32 MDI window management: Tile Horizontally, Tile Vertically, Cascade, Arrange Icons, Close All.

---

## See also

- [Keyboard Shortcuts](keyboard-shortcuts.md) — full shortcut table
- [Getting Started](getting-started.md) — REPL basics
- [Forth Reference](forth-reference.md) — core word list
