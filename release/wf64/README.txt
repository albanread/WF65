WF64 - 64-bit STC Forth IDE
============================

Quick start
-----------
Double-click wf64-ui.exe.  The IDE opens to a Forth console pane.
Type at the > prompt and press Enter:

    : square  dup * ;
    7 square .

Should print 7 ok and 49 ok.

The Demos menu loads ready-to-run example programs.
View menu opens the stack viewer (Ctrl+Shift+K), log pane
(Ctrl+Shift+L), and additional REPL panes (Ctrl+Shift+P).
File -> New (Ctrl+N) opens the Forth source editor.

Help -> Documentation (F1) opens the full user guide in a
document pane inside the IDE - rendered Markdown with a sidebar
to browse pages.  No external viewer required.

Layout
------
    wf64-ui.exe   - the IDE binary (drag a shortcut to your desktop)
    LLVM-C.dll    - LLVM runtime (must stay alongside wf64-ui.exe)
    kernel\       - JIT-assembled Forth primitives (loaded at boot)
    lib\          - Forth standard library (core.f)
    demos\        - sample programs reachable via the Demos menu
    docs\         - user guide and reference (shown in-window via Help)

Where things live at runtime
----------------------------
The IDE looks for kernel\ and lib\ next to wf64-ui.exe first.
You can move the whole folder anywhere as long as the layout
above stays intact.
