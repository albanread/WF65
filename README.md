# WF64

A Forth in LLVM/custom MASM — under development.


This is an application-level Forth rather than an embedded-systems Forth, aimed at writing Windows programs.

Like my other compilers, exe generation is not a priority; native code generation is. The bring-up is from source direct to memory.

It's open source, so it's nice to just change it and run, without compile/link/crash/repeat rituals.

In theory, as a compiler, it can also be adapted to emit an exe — but it would lose features.

LLVM is used in the core. After Forth is built, the Forth compiler itself does not use LLVM.

The Forth compiler here is based on the WF32 STC compiler.

-----------------------

Here is the story: writing the Forth compiler in Rust, like the other compilers here, was not satisfying — Forth does not fit well there.

The shape is much better if we write Forth in masm.

For this we build a macro assembler. All we need are the macros: the first step is to use the LLVM MCJIT MASM-flavoured assembler, then add a parser on top to make it a useful macro assembler. That assembler can read `.masm` files and generate the Forth kernel.

This lets the Forth kernel be implemented in assembly (see the JASM project). The kernel was ported from WF32 with a fair amount of automated extraction and testing.

The WF32 primitives and STC compiler are the starting point; on top of that we overlay a port of ANS Forth.


This does lead to some layers

----------------------------------------
MASM kernel - assembly language
Can invoke windows API also
----------------------------------------
ANSI Forth Core, some MASM, some high level
----------------------------------------
ANSI Forth in Forth
----------------------------------------
Escape hatch - CODE uses MASM
----------------------------------------
Escape hatch - LET infix expressions
----------------------------------------
Paged garbage collector
----------------------------------------
New strings
----------------------------------------
Interactive forth REPL
----------------------------------------
User application
-----------------------------------------

Apart from lets say 'implementation details' this is a very conventional FORTH right up to the ANS layer.

If we wanted to bootstrap a ANS FORTH we could do; we could create an exe at 'ANSI Forth in Forth' level.

This is a conventional Forth, up to the first escape hatch — `CODE` — which lets Forth define new
primitives using the same macro assembler the kernel uses.

After that the LET infix operator is a dense floating-point expression evaluator. Its main purpose is
to make the floating-point code shorter and easier to read; emitting register-allocated SSE that LLVM
or libm could plausibly produce is the side benefit.

The paged GC, is my own GC that I also use with Lisp, Dylan etc this gives us a managed heap for data.
It creates data outside the dictionary for us.

The GC allows us to add New strings, which is a powerful dynamic strings library.

The way I look at this is, its normal FORTH with extensions, similar to my other compilers.



