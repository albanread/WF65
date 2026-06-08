# Keyboard Shortcuts

Complete shortcut reference for WF64. Shortcuts are divided by scope: frame-level accelerators apply whenever the main window has focus; pane-level shortcuts apply only while that pane is active.

---

## Frame-level (always active)

These shortcuts are registered in the Win32 accelerator table and work regardless of which MDI child is focused.

### File

| Shortcut | Action |
|---|---|
| `Ctrl+N` | New editor buffer (fedit) |
| `Ctrl+O` | Open file in editor |
| `Ctrl+S` | Save active editor buffer |
| `Ctrl+Shift+S` | Save active editor buffer as… |
| `Alt+F4` | Exit |

### View — open or focus a pane

| Shortcut | Pane |
|---|---|
| `Ctrl+Shift+R` | Console (interactive Forth terminal) |
| `Ctrl+Shift+P` | REPL (split transcript + input) |
| `Ctrl+Shift+K` | Stack viewer |
| `Ctrl+Shift+L` | Log view |
| `Ctrl+Shift+X` | Crash dump (∴) |

### Forth

| Shortcut | Action |
|---|---|
| `F5` | Run active editor buffer |
| `Ctrl+Shift+F5` | Restart Forth session |

---

## Editor pane (fedit)

| Shortcut | Action |
|---|---|
| `F5` | Evaluate buffer contents |
| `Ctrl+Z` | Undo |
| `Ctrl+Y` | Redo |
| `Ctrl+X` | Cut |
| `Ctrl+C` | Copy |
| `Ctrl+V` | Paste |
| `Ctrl+A` | Select all |
| `Ctrl+Right` | Move to next word |
| `Ctrl+Left` | Move to previous word |

---

## Console pane (Ctrl+Shift+R)

| Shortcut | Action |
|---|---|
| Enter | Submit current input line |
| Backspace | Delete character left of cursor |
| Delete | Delete character right of cursor |
| Left / Right | Move cursor in input |
| Home / End | Jump to start / end of input |
| Up | Recall previous history entry |
| Down | Recall next history entry |
| Ctrl+L | Clear scrollback |
| Ctrl+U | Clear current input line |
| Mouse wheel | Scroll history |

---

## REPL pane (Ctrl+Shift+P)

### Input editing

| Shortcut | Action |
|---|---|
| Enter | Submit input (when syntactically complete) |
| Shift+Enter | Insert newline in input without submitting |
| Backspace | Delete character left of cursor |
| Delete | Delete character right of cursor |
| Left / Right | Move cursor by character |
| Ctrl+Left | Move cursor to previous word |
| Ctrl+Right | Move cursor to next word |
| Home | Move to start of current line |
| End | Move to end of current line |
| Ctrl+A | Move cursor to start of entire input |
| Ctrl+E | Move cursor to end of entire input |
| Ctrl+K | Kill from cursor to end of line |
| Ctrl+U | Kill from cursor to start of line |
| Up | History recall previous (when on first input line) |
| Down | History recall next (when on last input line) |

### Clipboard

| Shortcut | Action |
|---|---|
| Ctrl+C | Copy input to clipboard; if input is empty, copies last transcript line |
| Ctrl+Shift+C | Copy entire transcript as plain text |
| Ctrl+X | Cut input to clipboard and clear input |
| Ctrl+V | Paste clipboard text at cursor |

### Transcript

| Shortcut | Action |
|---|---|
| Ctrl+L | Clear transcript |
| Page Up | Scroll transcript up one page |
| Page Down | Scroll transcript down one page |
| Mouse wheel | Scroll transcript |

---

## Stack viewer (Ctrl+Shift+K)

Read-only. Mouse wheel scrolls when there are more entries than fit the window. No keyboard shortcuts beyond standard MDI window management.

---

## Log view (Ctrl+Shift+L)

Read-only. Mouse wheel scrolls history. No entry-level keyboard shortcuts.

---

## Crash dump pane (Ctrl+Shift+X)

| Shortcut | Action |
|---|---|
| Ctrl+L | Clear all stored dumps |
| Mouse wheel | Scroll dump text |

---

## See also

- [IDE Guide](ide-guide.md) — detailed description of each pane
- [Getting Started](getting-started.md) — REPL basics
- [Forth Reference](forth-reference.md) — core word reference
