# WF64

WF64 is a 64-bit subroutine-threaded Forth for Windows x86-64, built on the JASM macro assembler and LLVM MCJIT. The binary is a Direct2D MDI GUI with an interactive REPL, a live stack viewer, a diagnostic log pane, and a crash-dump viewer.

## Design notes

- **Direct-call threading (STC).** Every word is a native machine-code procedure. Calling one word from another is a plain `call rel32` — no inner interpreter, no threaded-code dispatch table.
- **JASM kernel.** The primitives are written in MASM-flavoured assembly using the JASM macro assembler, which expands into LLVM-MC object code at process start. Editing a primitive means editing a `.masm` file and running `cargo run` — no separate assembler invocation, no `link` step.
- **ANS Forth surface.** The CORE, CORE-EXT, FILE, and MEMORY wordsets are largely in place; transcendental float math (fsin, fcos, fsqrt, …) and `f.` are implemented. Remaining gaps are narrow: float formatting extras (`fe.`/`fs.`), `>float`, `see`, and a real `environment?`. See [ANS Gap Analysis](ANS_GAP_ANALYSIS.md) for the current accounting. `lib/core.f` provides the higher-level vocabulary built from primitives at startup.
- **Windows MDI GUI.** The IDE is a Win32 MDI application rendered with Direct2D and DirectWrite. Each pane — REPL, stack, log, crash dump — is an independent MDI child you can tile, maximise, or close.
- **Crash recovery.** A vectored exception handler (VEH) intercepts SEH faults in the Forth worker thread, captures a register snapshot and stack listing, and presents them in the crash-dump pane. The rest of the IDE stays up; close the dump and restart to continue.
- **Paged GC and managed strings.** A page-heap garbage collector (shared with NCL and NewOpenDylan) provides a managed heap for dynamic data outside the dictionary. The managed-string library is built on top of it.

## Feature highlights

- Interactive REPL with input history, transcript scrollback, and clipboard support
- Live stack viewer updated after every eval (View → Stack, `Ctrl+Shift+K`)
- Log view for diagnostic output (View → Log, `Ctrl+Shift+L`)
- Crash-dump pane showing registers + stack words after an SEH fault (`Ctrl+Shift+X`)
- Editor pane (`fedit`) with F5 to run buffer, undo/redo, word navigation
- Demos menu auto-populated from `.f` files in the `demos/` directory
- `CODE` escape hatch — define new primitives inline using JASM assembly
- `LET` infix expression evaluator for compact floating-point work
- `forget_last` to roll back the most recent definition during development

## Pages

| Page | What |
|---|---|
| [Getting Started](getting-started.md) | Running WF64, first REPL session, basic workflow |
| [Forth Tutorial](forth-tutorial.md) | Learning Forth with WF64 — stack model, words, control flow |
| [Forth Reference](forth-reference.md) | Core word reference with stack effects |
| [IDE Guide](ide-guide.md) | REPL pane, log view, crash dump, editor, menus |
| [Keyboard Shortcuts](keyboard-shortcuts.md) | Complete shortcut table for all panes |

## Quick start

Double-click **`wf64-ui.exe`**. Type at the `>` prompt and press Enter:

```forth
2 3 + .
```

Should print `5 ok`. The [Getting Started](getting-started.md) guide walks through the first session from there.
