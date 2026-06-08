# WF64 ANS Forth Gap Analysis

**Date:** 2026-05-25  
**Current status:** M1–M7 complete; `cargo test` is green.  
**Definitive source of truth:** `src/lib.rs` `PRIMITIVES`, `lib/core.f`, and the active tests under `tests/`.

---

## 1. Summary

This file replaces the older M4-era analysis. That older snapshot is now badly stale: many words it listed as missing are implemented today either as kernel primitives or as source-defined words in `lib/core.f`.

The current picture is:

- The CORE wordset is largely in place.
- The practical CORE-EXT surface used by the current system and the ANS core tests is also largely in place.
- Control flow, source loading, and the ANS core tests are all working and covered.
- The main remaining gaps are no longer in the basic core language; they are in the broader FILE, MEMORY, and FLOAT extensions, plus a small number of genuinely absent convenience words.

---

## 2. Confirmed Present Now

These were notable historical gaps but are implemented in the current tree.

### 2.1 Core / Core-Ext Words Now Present

- `char`
- `[char]`
- `find` (ANS counted-string wrapper over `find-name`)
- `?`
- `erase`
- `unused`
- `.r`
- `u.r`
- `d.`
- `d.r`
- `ud.`
- `du.`
- `buffer:`
- `value`
- `to`
- `+to`
- `defer`
- `defer!`
- `defer@`
- `action-of`
- `is`
- `2literal`
- `:noname`
- `compile,`
- `c"`
- `roll`
- `included`
- `include`
- `save-input`
- `restore-input`
- `name>string`
- `.(`
- `marker`
- `[defined]`
- `[undefined]`
- `[if]`
- `[else]`
- `[then]`

### 2.2 Other Present Extensions

- `-trailing`
- `dmax`
- `dmin`
- `fabs`
- `fmax`
- `fmin`
- `words`
- `dump`
- string substitution helpers (`replaces`, `substitute`)
- source-defined CASE family (`case`, `of`, `endof`, `endcase`)

### 2.3 Comment Handling

Comment behavior is present, but implemented lexically in `parse_name` rather than by dictionary entries:

- `(` begins a comment only when it is the standalone token `(`
- `\` begins a line comment only when it is the standalone token `\`

That means comment syntax works correctly in source input even though the parser, not a normal word lookup, is what enforces it.

---

## 3. What Is Genuinely Still Missing

This section is intentionally narrower than the old document. It lists words and areas that still appear absent after checking the current primitive table and `lib/core.f`.

### 3.1 Small Language-Surface Gaps

| Word | Status | Notes |
|---|---|---|
| `2value` | **present** | Defined in `lib/core.f`; `2to` is also present. |
| `environment?` | partial | Present only as a stub returning `false`. Good enough for current tests, not a real environment query database. |

### 3.2 FILE Wordset

The ANS FILE wordset is **substantially implemented** as kernel primitives (registered in `src/lib.rs`). Most file-handle words are present.

| Word | Status |
|---|---|
| `open-file` | **present** (kernel primitive) |
| `create-file` | **present** (kernel primitive) |
| `close-file` | **present** (kernel primitive) |
| `read-file` | **present** (kernel primitive) |
| `read-line` | **present** (source-defined in `lib/core.f`) |
| `write-file` | **present** (kernel primitive) |
| `write-line` | **present** (kernel primitive) |
| `flush-file` | **present** (kernel primitive) |
| `file-position` | **present** (kernel primitive) |
| `reposition-file` | **present** (kernel primitive) |
| `file-size` | **present** (kernel primitive) |
| `delete-file` | **present** (kernel primitive) |
| `rename-file` | **present** (kernel primitive) |
| `include-file` | partial — `include` / `included` via Rust helper slurp |
| `resize-file` | missing |
| `file-status` | missing |
| `require` / `required` | missing |

Source loading (`include` / `included`) is source-defined in `lib/core.f` using Rust runtime helpers (`rt_slurp_file`, `rt_slurp_len`, `rt_slurp_pop`) routed through `evaluate`. Direct handle-based file I/O (open, read, write, close) is available as kernel primitives.

### 3.3 MEMORY Wordset

The MEMORY wordset is **fully implemented** as kernel primitives (registered in `src/lib.rs`):

- `allocate` — **present**
- `free` — **present**
- `resize` — **present**

### 3.4 FLOAT / FLOAT-EXT Gaps

Core float arithmetic and transcendental math are both present as kernel primitives. Gaps are now in formatting and parsing only.

**Present:**

- arithmetic: `f+`, `f-`, `f*`, `f/`, `fnegate`, `fabs`, `fmax`, `fmin`
- transcendentals: `fsin`, `fcos`, `ftan`, `fln`, `fexp`, `fsqrt` (all kernel primitives)
- float formatting: `f.` (source-defined in `lib/core.f`)
- comparison: `f<`, `f>`, `f=`, `0f=`, `0f<`
- conversion: `s>f`, `f>s`, `float+`, `floats`

**Still missing:**

- `fe.` / `fs.` (engineering/scientific notation output)
- `precision` / `set-precision` / `represent`
- `>float` (string-to-float parsing)
- `f~` (approximate float comparison)
- `falign` / `faligned`

### 3.5 TOOLS Gaps

Present already:

- `.s`
- `?`
- `dump`
- `words`

Still clearly missing:

- `see`

---

## 4. Current Coverage Notes

### 4.1 Control Flow

This is no longer a gap. The control-flow surface is implemented and covered by harness tests, including:

- `if` / `else` / `then`
- `begin` / `until`
- `begin` / `while` / `repeat`
- `do` / `?do` / `loop` / `+loop` / `-loop`
- `leave` / `?leave`
- `recurse`
- `i` / `j`

### 4.2 ANS Core Tests

The repo now includes:

- `lib/tester.fs`
- `lib/ans_core_tests.fs`
- harness coverage that loads and runs those tests

So the earlier “M7 next” framing is obsolete.

---

## 5. Suggested Next Additions

If the goal is to keep closing real ANS gaps with good cost/benefit, the next sensible order is:

1. real `environment?`
   Reason: current stub is acceptable for tests but not a real implementation.

2. `fe.` / `fs.` / `represent`
   Reason: `f.` exists; adding engineering and scientific notation output rounds out the float-printing surface.

3. `>float`
   Reason: string-to-float parsing is the main remaining conversion gap.

4. `file-status` / `require` / `required`
   Reason: the file-handle core is done; these are the next small layer on top.

5. `see`
   Reason: the TOOLS wordset is otherwise present; a decompiler/disassembler is the obvious remaining gap.

---

## 6. Bottom Line

WF64 covers the CORE and CORE-EXT wordsets well, with the FILE wordset (handle-based I/O), MEMORY wordset (allocate/free/resize), and core FLOAT-EXT (including all transcendentals and `f.`) all now implemented. The remaining gaps are narrow:

- real `environment?` (current stub returns false)
- `fe.` / `fs.` / `represent` / `>float` (float formatting / parsing)
- `file-status` / `require` / `required` (thin layer above existing file I/O)
- `see` (TOOLS decompiler)

Anything that still lists `2value`, `allocate`, `free`, `resize`, `open-file`, `close-file`, `read-file`, `write-file`, `fsin`, `fcos`, `fsqrt`, or `f.` as missing is outdated.
