//! WF64 — programmable Forth session.
//!
//! `Wf64Session` is the unit of test isolation and the engine that
//! `src/main.rs` (the interactive REPL) sits on top of. One session
//! owns:
//!
//!   * a JIT'd kernel (assembled from `kernel/*.masm`),
//!   * a 128 MB near-memory region carved into data stack / return
//!     stack / user area / dictionary heap,
//!   * the populated dictionary,
//!   * the current data-stack state (`current_dsp` + `current_tos`).
//!
//! Two modes of operation:
//!
//!   * `eval(input)` — feed text through the REPL (quit) and capture
//!     stdout. Bye is cooperative: it just sets a flag, quit returns
//!     cleanly, and the session is reusable.
//!
//!   * `push(v)` / `pop()` / `call(sym)` — direct primitive invocation
//!     with a pre-staged data stack. No parsing, no dispatch. Lets
//!     unit tests cover each primitive in isolation.
//!
//! Both modes share the same `forth_main` entry point — see
//! `kernel/main.masm`.

#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]

pub mod runtime;
pub mod wf32_port;

use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};
use std::ptr;

use anyhow::{Context, Result};
use wfasm::{Assembler, Jit};

pub mod let_lang;
pub mod gc;

// Optimizer measurement harness (feature = "opt-metrics"). Static, byte-exact
// codegen metrics over compiled word bodies via the existing debug_words()
// ranges + an x86 decoder. Drives the bench/ corpus gate and the opt-bench
// before/after tool. Excluded from the default build graph.
#[cfg(feature = "opt-metrics")]
pub mod opt_metrics;

// Advisory dynamic (rdtsc/utime) timing layer for the optimizer harness. Kept
// strictly off the static gate's path — see src/opt_timing.rs.
#[cfg(feature = "opt-metrics")]
pub mod opt_timing;

// iGui — Windows MDI front-end, ported from NewCormanLisp.
// Lives behind cfg(windows); the module file applies its own
// cfg(windows) gates on the renderer/window code.
#[cfg(windows)]
pub mod igui;

// NewFactor IDE support — Forth→Factor transpiler and in-process
// Factor session.  Windows-only (same gate as igui).
#[cfg(windows)]
pub mod newfactor;

pub const KERNEL_ENTRY: &str = "kernel/main.masm";

/// Locate `kernel/main.masm` for `Wf64Session::new()`.
///
/// Search order — first existing path wins:
///   1. `<exe_dir>/kernel/main.masm` — production layout shipped
///      by `tools/build-release.ps1` (binary + kernel/ + lib/
///      sit side-by-side under `release/wf64/`).
///   2. `<exe_dir>/../../kernel/main.masm` — `cargo run` from any
///      directory while the binary lives under `target/release/`
///      or `target/debug/`.
///   3. `CARGO_MANIFEST_DIR/kernel/main.masm` — repo root fallback
///      (developer-launched `cargo test` etc.).
///   4. `kernel/main.masm` — relative to CWD; matches the
///      historical behaviour and is what users typing
///      `cargo run --bin wf64` from the repo root see.
pub fn default_kernel_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let p = exe_dir.join("kernel").join("main.masm");
            if p.exists() {
                return p;
            }
            if let Some(repo) = exe.ancestors().nth(3) {
                let p = repo.join("kernel").join("main.masm");
                if p.exists() {
                    return p;
                }
            }
        }
    }
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("kernel")
        .join("main.masm");
    if p.exists() {
        return p;
    }
    PathBuf::from(KERNEL_ENTRY)
}

/// (forth_name, asm_symbol, flags). Insertion order = scan order:
/// most-recently-inserted is matched first by `find-name`.
///
/// The bootstrap loops this list inside `Wf64Session::new()` calling
/// `(create)` / `(set-xt)` / `(set-flags)` once per entry. The Rust
/// driver does NOT know the dictionary header layout — that lives
/// entirely in `kernel/dict.masm` and `kernel/macros.masm`.
pub const PRIMITIVES: &[(&str, &str, u8)] = &[
    // Dictionary primitives — must be present from the very first
    // bootstrap step, since the bootstrap itself uses them. They get
    // looked up by JIT address (not by name), so the dict's emptiness
    // at that point is irrelevant; they're "self-publishing".
    ("(create)",    "create",     0),
    ("(set-xt)",    "set_xt",     0),
    ("(set-comp)",  "set_comp",   0),
    ("(set-flags)", "set_flags",  0),
    // Stack ops (WF32 gkernel32.fs:124-195)
    ("dup",        "dup_",       0),
    ("swap",       "swap_",      0),
    ("drop",       "drop_",      0),
    ("rot",        "rot_",       0),
    ("over",       "over_",      0),
    ("-rot",       "neg_rot",    0),
    ("?dup",       "qdup",       0),
    ("nip",        "nip_",       0),
    ("tuck",       "tuck_",      0),
    ("pick",       "pick",       0),
    ("roll",       "roll_",      0),
    ("depth",      "depth",      0),
    // Return-stack ops (WF32 gkernel32.fs:197-273)
    ("dup>r",      "dup_to_r",   0),
    (">r",         "to_r",       0),
    ("r>",         "r_from",     0),
    ("r@",         "r_fetch",    0),
    ("rdrop",      "rdrop",      0),
    ("2>r",        "two_to_r",   0),
    ("2r>",        "two_r_from", 0),
    ("2r@",        "two_r_fetch",0),
    ("i",          "i_word",     0),
    ("j",          "j_word",     0),
    ("do-part1",   "do_part1",   0),
    ("do-part2",   "do_part2",   0),
    ("mark>",      "mark_to",    1),
    ("<resolve",   "back_resolve", 1),
    (">resolve",   "forward_resolve", 1),
    ("?pairs",     "qpairs",     1),
    ("ahead",      "ahead_word", 1),
    ("if",         "if_word",    1),
    ("-if",        "minus_if_word", 1),
    ("then",       "then_word",  1),
    ("else",       "else_word",  1),
    ("begin",      "begin_word", 1),
    ("while",      "while_word", 1),
    ("again",      "again_word", 1),
    ("until",      "until_word", 1),
    ("repeat",     "repeat_word", 1),
    ("recurse",    "recurse_word", 1),
    ("LET",        "let_word",     1),
    ("CODE:",      "code_colon_word", 0),
    // GC primitives (V1b).  See docs/gc_design.md.
    ("HEAPPTR",            "heapptr_word",             0),
    ("(gc)",               "gc_collect_word",          0),
    ("gc-minor",           "gc_collect_minor_word",    0),
    ("gc-cycle",           "gc_cycle_word",            0),
    ("!heapptr",           "store_heapptr_word",       0),
    ("vec-alloc-floats!",  "vec_alloc_floats_store",   0),
    ("vec-alloc-refs!",    "vec_alloc_refs_store",     0),
    ("vec-f@",             "vec_f_fetch",              0),
    ("vec-f!",             "vec_f_store",              0),
    ("vec-ref@",           "vec_ref_fetch",            0),
    ("vec-ref!",           "vec_ref_store",            0),
    ("vec-len",            "vec_len",                  0),
    // Managed strings (V2s stage A).  See docs/strings_design.md.
    (">$",                 "to_string_word",           0),
    ("$len",               "string_len_word",          0),
    ("$>addr",             "string_to_addr_word",      0),
    ("$=",                 "string_equal_word",        0),
    ("@$",                 "fetch_string_word",        0),
    ("!$",                 "store_string_word",        0),
    // V2s stage B: compile-time literal form.
    ("S$\"",               "s_dollar_quote_word",      1),  // IMMEDIATE
    // V2s stage C1: MutStringBuilder.
    ("sb-new",             "sb_new_word",              0),
    ("sb-len",             "sb_len_word",              0),
    ("sb-capacity",        "sb_capacity_word",         0),
    ("sb-clear",           "sb_clear_word",            0),
    ("sb-append$",         "sb_append_string_word",    0),
    ("sb-append-c",        "sb_append_codepoint_word", 0),
    ("sb-append-n",        "sb_append_int_word",       0),
    ("sb>string",          "sb_to_string_word",        0),
    // V2s stage C2: operations library.
    ("$+",                 "string_concat_word",       0),
    ("$slice",             "string_slice_word",        0),
    ("$find",              "string_find_word",         0),
    ("$starts?",           "string_starts_word",       0),
    ("$ends?",             "string_ends_word",         0),
    ("$cmp",               "string_cmp_word",          0),
    ("$hash",              "string_hash_word",         0),
    ("$ci=",               "string_ci_eq_word",        0),
    ("$trim",              "string_trim_word",         0),
    ("$ltrim",             "string_ltrim_word",        0),
    ("$rtrim",             "string_rtrim_word",        0),
    ("n>$",                "int_to_string_word",       0),
    ("$>n",                "string_to_int_word",       0),
    ("empty$",             "empty_string_word",        0),
    // V2s stage D: extended operations.
    ("$contains?",         "string_contains_word",     0),
    ("$rfind",             "string_rfind_word",        0),
    ("$repeat",            "string_repeat_word",       0),
    ("$replace",           "string_replace_word",      0),
    ("$split",             "string_split_word",        0),
    // V2s stage E: UTF-8 awareness, floats, char$, $words.
    ("$clen",              "string_clen_word",         0),
    ("$c@",                "string_cat_word",          0),
    ("$valid?",            "string_valid_word",        0),
    ("$validate",          "string_validate_word",     0),
    ("char$",              "char_to_string_word",      0),
    ("$upper",             "string_upper_word",        0),
    ("$lower",             "string_lower_word",        0),
    ("$>f",                "string_to_float_word",     0),
    ("f>$",                "float_to_string_word",     0),
    ("sb-append-f",        "sb_append_float_word",     0),
    ("$words",             "string_words_word",        0),
    // iGui bridge — Forth-side hooks into the wf64-ui front-end.
    // Both are no-ops when running headless.  See kernel/igui.masm.
    ("page",               "page_word",                0),
    ("at-xy",              "at_xy_word",               0),
    ("bug-rust-panic",     "bug_rust_panic_word",      0),
    ("bug-seh-av",         "bug_seh_av_word",          0),
    // iGui graphical-pane drawing API.  See kernel/igui_gfx.masm
    // for the docs.  Colours are packed 0xRRGGBB in one cell;
    // coordinates and sizes are signed integer pixels.
    ("gpane-open",         "gpane_open_word",          0),
    ("gpane-begin",        "gpane_begin_word",         0),
    ("gpane-present",      "gpane_present_word",       0),
    ("gpane-clear",        "gpane_clear_word",         0),
    ("gpane-fill-rect",    "gpane_fill_rect_word",     0),
    ("gpane-stroke-rect",  "gpane_stroke_rect_word",   0),
    ("gpane-line",         "gpane_line_word",          0),
    ("gpane-fill-circle",  "gpane_fill_circle_word",   0),
    ("gpane-next-event",   "gpane_next_event_word",    0),
    ("fractal-iter",       "fractal_iter_word",        0),
    ("canvas-blit",        "canvas_blit_word",         0),
    ("do",         "do_word",    1),
    ("?do",        "qdo_control_word", 1),
    ("loop",       "loop_control_word", 1),
    ("+loop",      "plus_loop_control_word", 1),
    ("-loop",      "minus_loop_control_word", 1),
    ("unloop",     "unloop_word", 0),
    ("leave",      "leave_word", 1),
    ("?leave",     "qleave_word", 1),
    ("bra",        "bra_word",   0),
    ("?bra",       "qbra_word",  0),
    ("-?bra",      "minus_qbra_word", 0),
    ("bra-?do",    "bra_qdo_word", 0),
    ("_loop",      "loop_word",  0),
    ("_+loop",     "plus_loop_word", 0),
    ("_-loop",     "minus_loop_word", 0),
    ("2rdrop",     "two_rdrop",  0),
    ("n>r",        "n_to_r",     0),
    ("nr>",        "nr_from",    0),
    ("sp@",        "sp_fetch",   0),
    ("sp!",        "sp_store",   0),
    ("rp@",        "rp_fetch",   0),
    ("rp!",        "rp_store",   0),
    // Memory ops
    ("@",          "fetch",      0),
    ("!",          "store",      0),
    ("c@",         "c_fetch",    0),
    ("c!",         "c_store",    0),
    ("here",       "here_word",  0),
    ("allot",      "allot_word", 0),
    ("2@",         "two_fetch",  0),
    ("2!",         "two_store",  0),
    // Non-std memory widths (b/w/L/q from WF32). On WF64, q is a cell.
    ("b@",         "b_fetch",    0),
    ("sb@",        "sb_fetch",   0),
    ("b!",         "b_store",    0),
    ("w@",         "w_fetch",    0),
    ("sw@",        "sw_fetch",   0),
    ("w!",         "w_store",    0),
    ("L@",         "l_fetch",    0),
    ("L!",         "l_store",    0),
    ("q@",         "q_fetch",    0),
    ("q!",         "q_store",    0),
    // Bit ops + cell-size helpers + strings (gkernel32.fs:537-1278)
    ("count-bits", "count_bits", 0),
    ("msbit",      "msbit",      0),
    ("lsbit",      "lsbit",      0),
    ("cells",      "cells",      0),
    ("cells+",     "cells_plus", 0),
    ("+cells",     "plus_cells", 0),
    ("cells-",     "cells_minus",0),
    ("cell+",      "cell_plus",  0),
    ("cell-",      "cell_minus", 0),
    ("char+",      "char_plus",  0),
    ("chars",      "chars",      0),
    ("aligned",    "aligned",    0),
    ("-aligned",   "minus_aligned", 0),
    ("naligned",   "naligned",   0),
    ("fill",       "fill",       0),
    ("cmove",      "cmove",      0),
    ("cmove>",     "cmove_to",   0),
    ("count",      "count",      0),
    ("move",       "move",       0),
    ("zcount",     "zcount",     0),
    ("lastchar",   "lastchar",   0),
    ("slastchar",  "slastchar",  0),
    ("exchange",   "exchange",   0),
    ("bounds",     "bounds",     0),
    ("/string",    "slash_string", 0),
    ("compare",    "compare",      0),
    ("str=",       "str_equal",    0),
    ("istr=",      "istr_equal",   0),
    ("tr",         "tr",           0),
    ("upc",        "upc",          0),
    ("upper",      "upper",        0),
    ("lower",      "lower",        0),
    ("uppercase",  "uppercase",    0),
    ("lowercase",  "lowercase",    0),
    ("+place",     "plus_place",   0),
    ("append",     "plus_place",   0),
    ("place",      "place",        0),
    ("c+place",    "c_plus_place", 0),
    ("skip",       "skip",         0),
    ("-skip",      "minus_skip",   0),
    ("scan",       "scan",         0),
    ("-scan",      "minus_scan",   0),
    ("search",     "search",       0),
    // Double-cell stack ops
    ("2drop",      "two_drop",   0),
    ("2dup",       "two_dup",    0),
    ("2nip",       "two_nip",    0),
    ("2swap",      "two_swap",   0),
    ("2rot",       "two_rot",    0),
    ("2over",      "two_over",   0),
    ("3drop",      "three_drop", 0),
    ("4drop",      "four_drop",  0),
    ("3dup",       "three_dup",  0),
    ("4dup",       "four_dup",   0),
    ("s-reverse",  "s_reverse",  0),
    // Arithmetic
    ("+",          "plus",       0),
    ("*",          "times",      0),
    ("-",          "minus",            0),
    ("fdepth",     "fdepth",           0),
    ("fdrop",      "fdrop",            0),
    ("fdup",       "fdup",             0),
    ("fswap",      "fswap",            0),
    ("fover",      "fover",            0),
    ("d>f",        "d_to_f",           0),
    ("f>d",        "f_to_d",           0),
    ("f@",         "f_fetch",          0),
    ("f!",         "f_store",          0),
    ("float+",     "float_plus",       0),
    ("floats",     "floats",           0),
    ("falign",     "falign",           0),
    ("faligned",   "faligned",         0),
    ("f+",         "f_plus",           0),
    ("f-",         "f_minus",          0),
    ("f*",         "f_times",          0),
    ("f/",         "f_slash",          0),
    ("fnegate",    "f_negate",         0),
    ("f0=",        "f_zero_equal",     0),
    ("f0<",        "f_zero_less",      0),
    ("f<",         "f_less",           0),
    ("negate",     "negate",           0),
    ("abs",        "abs",              0),
    ("1+",         "one_plus",         0),
    ("1-",         "one_minus",        0),
    ("2+",         "two_plus",         0),
    ("2-",         "two_minus",        0),
    ("2*",         "two_times",        0),
    ("3*",         "three_times",      0),
    ("5*",         "five_times",       0),
    ("10*",        "ten_times",        0),
    ("2/",         "two_slash",        0),
    ("u2/",        "u2slash",          0),
    ("+!",         "plus_store",       0),
    ("@+!",        "fetch_plus_store", 0),
    ("1+!",        "one_plus_store",   0),
    ("1-!",        "one_minus_store",  0),
    ("1+c!",       "one_plus_c_store", 0),
    ("1-c!",       "one_minus_c_store",0),
    ("c+!",        "c_plus_store",     0),
    ("d+",         "d_plus",           0),
    ("d+!",        "d_plus_store",     0),
    ("d-",         "d_minus",          0),
    ("dnegate",    "dnegate",          0),
    ("dabs",       "dabs",             0),
    ("d2*",        "d_two_times",      0),
    ("d2/",        "d_two_slash",      0),
    ("s>d",        "s_to_d",           0),
    ("/mod",       "slash_mod",        0),
    ("um*",        "um_times",         0),
    ("m*",         "m_times",          0),
    ("um/mod",     "um_slash_mod",     0),
    ("sm/rem",     "sm_slash_rem",     0),
    ("fm/mod",     "fm_slash_mod",     0),
    ("*/",         "times_slash",      0),
    ("*/mod",      "times_slash_mod",  0),
    ("/",          "slash",            0),
    ("mod",        "mod_",             0),
    // Bitwise / logic
    ("and",        "and_",             0),
    ("or",         "or_",              0),
    ("xor",        "xor_",             0),
    ("invert",     "invert",           0),
    ("lshift",     "lshift",           0),
    ("rshift",     "rshift",           0),
    ("arshift",    "arshift",          0),
    ("on",         "on",               0),
    ("off",        "off",              0),
    // Comparison
    ("0=",         "zero_equal",       0),
    ("0<>",        "zero_not_equal",   0),
    ("0<",         "zero_less",        0),
    ("0>",         "zero_greater",     0),
    ("=",          "equal",            0),
    ("<>",         "not_equal",        0),
    ("<",          "less",             0),
    (">",          "greater",          0),
    ("<=",         "less_equal",       0),
    (">=",         "greater_equal",    0),
    ("u<",         "u_less",           0),
    ("u>",         "u_greater",        0),
    ("u<=",        "u_less_equal",     0),
    ("u>=",        "u_greater_equal",  0),
    ("min",        "min_",             0),
    ("max",        "max_",             0),
    ("0max",       "zero_max",         0),
    ("umin",       "umin",             0),
    ("umax",       "umax",             0),
    ("within",     "within",           0),
    ("d=",         "d_equal",          0),
    ("d0<",        "d_zero_less",      0),
    ("d0=",        "d_zero_equal",     0),
    ("d<",         "d_less",           0),
    ("du<",        "du_less",          0),
    ("du>",        "du_greater",       0),
    ("d>",         "d_greater",        0),
    ("d<>",        "d_not_equal",      0),
    // ANS Forth alias: `not` ≡ `0=`
    ("not",        "zero_equal",       0),
    // Number output
    (".",          "dot",        0),
    (".s",         "dot_s",      0),
    ("digit",      "digit",      0),
    (">number",    "to_number",  0),
    ("base",       "base_word",  0),
    ("base@",      "base_fetch", 0),
    ("base!",      "base_store", 0),
    ("decimal",    "decimal_word", 0),
    ("hex",        "hex_word",   0),
    ("octal",      "octal_word", 0),
    // I/O
    ("emit",       "emit",       0),
    ("type",       "type_word",  0),
    ("key",        "key_word",   0),
    ("key?",       "key_q_word", 0),
    ("cr",         "cr_word",    0),
    ("BRK",        "brk_word",   0),
    ("INT3",       "int3_word",  0),
    ("trace",      "trace_word", 0),
    ("notrace",    "notrace_word", 0),
    ("cpuid",      "cpuid_word", 0),
    ("rdtsc",      "rdtsc_word", 0),
    ("bye",        "bye",        0),
    // Facility-ext (Win32-backed)
    ("ms",         "ms_word",          0),
    ("utime",      "utime_word",       0),
    ("time&date",  "time_and_date_word", 0),
    // Memory-Allocation
    ("allocate",   "allocate_word",    0),
    ("free",       "free_word",        0),
    ("resize",     "resize_word",      0),
    // File-Access
    ("open-file",   "open_file_word",   0),
    ("create-file", "create_file_word", 0),
    ("close-file",  "close_file_word",  0),
    ("read-file",   "read_file_word",   0),
    ("write-file",  "write_file_word",  0),
    ("delete-file", "delete_file_word", 0),
    ("file-position",  "file_position_word",  0),
    ("file-size",      "file_size_word",      0),
    ("reposition-file","reposition_file_word",0),
    ("write-line",     "write_line_word",     0),
    ("flush-file",     "flush_file_word",     0),
    ("rename-file",    "rename_file_word",    0),
    // Floating-point math (msvcrt)
    ("fsqrt",      "fsqrt_word",       0),
    ("fsin",       "fsin_word",        0),
    ("fcos",       "fcos_word",        0),
    ("ftan",       "ftan_word",        0),
    ("fexp",       "fexp_word",        0),
    ("fln",        "fln_word",         0),
    ("flog",       "flog_word",        0),
    ("fatan",      "fatan_word",       0),
    ("fasin",      "fasin_word",       0),
    ("facos",      "facos_word",       0),
    ("f**",        "f_pow_word",       0),
    ("fatan2",     "fatan2_word",      0),
    // Locals-stack scaffolding (R15 = LP)
    ("lp@",        "lp_fetch_word",    0),
    ("lp0@",       "lp0_fetch_word",   0),
    ("lp-limit",   "lp_limit_word",    0),
    ("lp-smoke",   "lp_smoke_word",    0),
    ("(open-locals)",  "open_locals_word",        0),
    ("(close-locals)", "close_locals_word",       0),
    ("(local@)",       "local_fetch_word",        0),
    ("(local!)",       "local_store_word",        0),
    ("locals#",        "locals_count_word",       0),
    ("locals#!",       "locals_count_store_word", 0),
    ("check-local-emit", "check_local_emit_word",  0),
    ("check-local-store","check_local_store_word", 0),
    ("(inline,)",      "inline_comma_word",       0),
    ("(inline-var,)",  "inline_var_comp",         0),
    // Parse & dict
    ("evaluate",   "evaluate_word", 0),
    ("parse-name", "parse_name", 0),
    ("pad",        "pad_word",    0),
    ("parse",      "parse_word",  0),
    ("word",       "word_word",   0),
    ("source",     "source_word", 0),
    ("source-id",  "source_id_word", 0),
    ("refill",     "refill_word", 0),
    ("state",      "state_word", 0),
    (">in",        "to_in_word", 0),
    ("find-name",  "find_name",  0),
    ("forth-wordlist", "forth_wordlist_word", 0),
    ("tools-wordlist", "tools_wordlist_word", 0),
    ("private-wordlist", "private_wordlist_word", 0),
    ("wordlist",   "wordlist_word", 0),
    ("get-current", "get_current_word", 0),
    ("set-current", "set_current_word", 0),
    ("definitions", "definitions_word", 0),
    ("only",        "only_word", 0),
    ("also",        "also_word", 0),
    ("previous",    "previous_word", 0),
    ("forth",       "forth_word", 0),
    ("get-order",  "get_order_word", 0),
    ("set-order",  "set_order_word", 0),
    ("search-wordlist", "search_wordlist_word", 0),
    (">ct",        "to_ct",      0),
    (">comp",      "to_comp",    0),
    (">name",      "to_name",    0),
    ("link>name",  "link_to_name", 0),
    ("name>interpret", "name_to_interpret", 0),
    ("name>compile", "name_to_compile", 0),
    ("tfa@",       "tfa_fetch",  0),
    (">body",      "to_body",    0),
    ("latestxt",   "latestxt",   0),
    ("forget_last", "forget_last_word", 0),
    // Object system — dispatch hot path (see docs/oop_design.md).
    // The class/object DSL itself lives in lib/oop.f.
    ("self",       "self_word",     0),
    ("(send)",     "send_word",     0),
    ("(send-xt)",  "send_xt_word",  0),
    ("largest",    "largest",    0),
    ("number?",    "number_q",   0),
    ("accept",     "accept",     0),
    // Execute & interp
    ("execute",    "execute",    0),
    ("perform",    "perform",    0),
    ("throw_abort", "throw_abort_const", 0),
    ("throw_abortq", "throw_abortq_const", 0),
    ("throw_componly", "throw_componly_const", 0),
    ("throw_namereqd", "throw_namereqd_const", 0),
    ("throw_mismatch", "throw_mismatch_const", 0),
    ("catch",      "catch_word", 0),
    ("throw",      "throw_word", 0),
    ("abort",      "abort_word", 0),
    ("?throw",     "qthrow_word", 0),
    ("(comp-only)", "comp_only_word", 0),
    ("quit",       "quit",       0),
    // Colon compiler (M4)
    ("[",          "left_bracket_word", 1),
    ("]",          "right_bracket_word", 0),
    ("immediate",  "immediate_word", 0),
    ("'",          "tick_word",   0),
    ("[']",        "bracket_tick_word", 1),
    ("literal",    "literal_word", 1),
    ("fliteral",   "fliteral_word", 1),
    ("s\"",       "s_quote_word", 1),
    (".\"",       "dot_quote_word", 1),
    ("c\"",       "c_quote_word", 1),
    ("abort\"",   "abort_quote_word", 1),
    ("postpone",   "postpone_word", 1),
    ("does>",      "does_word",   1),
    ("compile,",   "compile_comma", 0),
    // File include runtime helpers (M6)
    ("rt-slurp-file", "rt_slurp_file_word", 0),
    ("rt-slurp-len",  "rt_slurp_len_word",  0),
    ("rt-slurp-pop",  "rt_slurp_pop_word",  0),
    (":noname",    "noname_word", 0),
    (":",          "colon",      0),
    ("create",     "create_word", 0),
    ("exit",       "exit_word",   1),
    (";",          "semicolon",  1),   // IMMEDIATE
];

/// Kernel-internal helper symbols (JIT-resolvable, NOT in the dict).
pub const KERNEL_HELPERS: &[&str] = &[
    "interpret_source",
    "do_lit",
    "do_flit",
    "do_slit",
    "do_clit",
    "compile_word",
    "compile_comma",
    "compile_comma_no_tco",
    "try_fold_literal",
    "fold_plus_comp",
    "fold_minus_comp",
    "fold_times_comp",
    "fold_and_comp",
    "fold_or_comp",
    "fold_xor_comp",
    "fold_lshift_comp",
    "fold_rshift_comp",
    "fold_arshift_comp",
    "fold_equal_comp",
    "fold_not_equal_comp",
    "fold_u_less_comp",
    "fold_less_comp",
    "fold_greater_comp",
    "fold_less_equal_comp",
    "fold_greater_equal_comp",
    "fold_u_greater_comp",
    "fold_u_less_equal_comp",
    "fold_u_greater_equal_comp",
    "inline_dup_comp",
    "inline_drop_comp",
    "inline_swap_comp",
    "inline_over_comp",
    "inline_leaf_comp",
    "inline_fetch_comp",
    "inline_store_comp",
    "inline_c_fetch_comp",
    "inline_c_store_comp",
    "inline_rot_comp",
    "inline_neg_rot_comp",
    "inline_nip_comp",
    "inline_tuck_comp",
    "inline_to_r_comp",
    "inline_r_from_comp",
    "inline_r_fetch_comp",
    "inline_two_to_r_comp",
    "inline_two_r_from_comp",
    "inline_two_r_fetch_comp",
    "inline_i_comp",
    "inline_j_comp",
    "inline_do_part1_comp",
    "inline_do_part2_comp",
    "inline_bra_comp",
    "inline_qbra_comp",
    "inline_minus_qbra_comp",
    "inline_bra_qdo_comp",
    "inline_loop_comp",
    "inline_plus_loop_comp",
    "inline_minus_loop_comp",
    "inline_unloop_comp",
    "literal",
    "fliteral",
    "sliteral",
    "init_dictionary_overlay",
    "publish_primitive",
];


// ── Memory layout (matches kernel/macros.masm) ───────────────────────

const REGION_SIZE:      usize = 128 * 1024 * 1024;
const DEBUG_META_SIZE:  u64 = 64 * 1024;
const OFFSET_DSP_TOP:   u64 = 0x40000;
const OFFSET_RSP_TOP:   u64 = 0x80000;
const OFFSET_USER_BASE: u64 = 0x80000;
const OFFSET_DICT_BASE: u64 = 0xC0000;

// Locals stack — a separate 1 MB region, R15 = LP grows downward.
// Not constrained to the ±2 GB near-region (only relative addressing
// off R15 is used, so the absolute address doesn't matter).
const LOCALS_REGION_SIZE: usize = 1 * 1024 * 1024;
// Data space for variables / create bodies. Separate PAGE_READWRITE (no-execute)
// VirtualAlloc so writes to a variable body never touch executable code pages
// (no SMC machine clears) and code is never writable through it (W^X). Bodies
// are reached via absolute addresses baked into each create stub, so this region
// can live anywhere in the address space.
const VAR_REGION_SIZE: usize = 16 * 1024 * 1024;

// User-area offsets — mirror kernel/macros.masm.
const USER_BASE_VAR:     u64 = 0x00;
const USER_STATE_VAR:    u64 = 0x08;
const USER_LATEST_VAR:   u64 = 0x10;
const USER_HERE_VAR:     u64 = 0x18;
const USER_DICT_END_VAR: u64 = 0x20;
const USER_PARSE_BARRIER:u64 = 0x48;
const USER_BYE_REQ:      u64 = 0x50;
const USER_DSP_SAVE:     u64 = 0x60;
const USER_SP0:          u64 = 0x68;
const USER_RSP_CURRENT:  u64 = 0x70;
const USER_LATESTXT_VAR: u64 = 0x78;
const USER_HANDLER_VAR:  u64 = 0x80;
const USER_THROW_CODE:   u64 = 0x88;
const USER_FORGET_FENCE: u64 = 0x98;
const USER_FP0:          u64 = 0x1210;
const USER_FSP:          u64 = 0x1218;
const USER_FP_STACK:     u64 = 0x1300;
const USER_CURRENT:      u64 = 0x1500;
const USER_FORTH_WID:    u64 = 0x1508;
const USER_ORDER_COUNT:  u64 = 0x1510;
const USER_INDEX_HERE:   u64 = 0x1518;
const USER_INDEX_LATEST: u64 = 0x1520;
const USER_CONTEXT:      u64 = 0x1528;
const USER_TRACE:        u64 = 0x15A8;  // trace flag; mirrors user_TRACE in macros.masm
const USER_LP0:          u64 = 0x15B0;  // initial LP (top of locals region)
const USER_LP_LIMIT:     u64 = 0x15B8;  // low limit of locals stack
const USER_LOCALS_COUNT: u64 = 0x15C0;  // #locals in current colon def (0 outside)
const USER_LOCALS_TABLE: u64 = 0x15C8;  // 16 * 32-byte locals table
const USER_TOOLS_WID:    u64 = 0x17C8;  // wid of TOOLS wordlist
const USER_PRIVATE_WID:  u64 = 0x17D0;  // wid of PRIVATE wordlist

// HEAPPTR region — bump-allocated array of GC root slots.  See
// docs/gc_design.md and kernel/macros.masm.  `pub(crate)` so the
// runtime module can read them when servicing rt_gc_collect.
pub(crate) const USER_HEAPPTR_NEXT:  u64 = 0x17E0;  // bump pointer (abs addr)
pub(crate) const USER_LITERAL_NEXT:  u64 = 0x17E8;  // bump pointer for LITERAL region
// OOP early-binding receiver hint (lib/oop.f). Cleared on reset so a stale
// HERE value can never trigger a false early-bind after HERE rewinds.
pub(crate) const USER_OOP_RECV_CLASS: u64 = 0x17F8;
pub(crate) const USER_OOP_RECV_HERE:  u64 = 0x1800;
// wid of the per-class ivar wordlist (lib/oop.f). Its bucket array is
// snapshotted at boot and restored on reset so scoped ivar entries added
// during a test don't dangle (same treatment as forth/tools/private).
pub(crate) const USER_OOP_IVARS_WID:  u64 = 0x1808;
// Near RWX arena that runtime CODE:/LET code is assembled into (so it's
// rel32-reachable from the kernel/dict — no far-segment trampoline). Base
// and size are published here at boot for runtime.rs to build a CodeArena.
pub(crate) const USER_JIT_ARENA_BASE: u64 = 0x1810;
pub(crate) const USER_JIT_ARENA_SIZE: u64 = 0x1818;
// Data space (variables / create bodies) lives in a separate RW / no-execute
// region — never in the executable dictionary. Keeps W^X and avoids the
// self-modifying-code machine clear that hit `v ! ; call v` patterns.
pub(crate) const USER_VAR_HERE:  u64 = 0x1820;  // data-space bump pointer
pub(crate) const USER_VAR_LIMIT: u64 = 0x1828;  // end of the data region
/// Size of the runtime JIT code arena (`CODE:`/`LET`). 32 MB — plenty for
/// thousands of small words, well inside the rel32 window.
const JIT_ARENA_SIZE: usize = 32 * 1024 * 1024;
pub(crate) const USER_HEAPPTR_BASE:  u64 = 0x2000;  // first slot of the region
pub(crate) const HEAPPTR_REGION_SIZE: u64 = 0x1000; // 4 KB = 512 slots
pub(crate) const USER_LITERAL_BASE:  u64 = 0x3000;  // first slot of LITERAL region
pub(crate) const LITERAL_REGION_SIZE: u64 = 0x10000; // 64 KB = 8 K slots

/// Words that should be published to the TOOLS wordlist at bootstrap.
/// Debug / inspection primitives the average user doesn't want in their
/// face. Source-defined Tools-ext words (WORDS, FORGET, MARKER, etc.)
/// are placed by core.f via `also tools definitions`.
pub const TOOLS_WORDS: &[&str] = &[
    ".s",
    "BRK", "INT3",
    "trace", "notrace",
    "rdtsc", "cpuid",
];

/// Words that should be published to the PRIVATE wordlist at bootstrap.
/// Compiler internals, locals scaffolding, control-flow helpers --
/// things only the kernel and core.f reach for, never user code.
pub const PRIVATE_WORDS: &[&str] = &[
    // Dictionary builders
    "(create)", "(set-xt)", "(set-comp)", "(set-flags)",
    // Compile-action helpers
    "(comp-cons)", "(comp-2cons)", "(comp-fconst)", "(comp-val)", "(comp-only)",
    // Locals stack
    "(open-locals)", "(close-locals)", "(local@)", "(local!)",
    "check-local-emit", "check-local-store", "locals#", "locals#!",
    "lp@", "lp0@", "lp-limit", "lp-smoke",
    // Control-flow assembly internals
    "mark>", ">resolve", "<resolve", "?pairs",
    "_loop", "_+loop", "_-loop", "bra-?do",
    "do-part1", "do-part2",
    // Throw-code constants
    "throw_namereqd", "throw_componly", "throw_abortq",
    "throw_abort", "throw_mismatch",
    // Forget machinery
    "forget_last", "latestxt",
    // Raw stack-pointer access
    "sp@", "sp!", "rp@", "rp!",
    // Inliner support
    "(inline,)",
];

// (Dictionary header layout deliberately not mirrored here — it lives
// entirely in kernel/macros.masm and kernel/dict.masm. The bootstrap
// builds the dictionary by calling `(create)` / `(set-xt)` /
// `(set-flags)` on the kernel side.)

/// Scratch buffer offset inside the user area for handing names to
/// `(create)`. Maps to `user_PAD` in kernel/macros.masm.
const USER_PAD: u64 = 0x100;

// Dictionary header offsets — mirrored privately for debug metadata.
const DH_LINK:      u64 = 0;
const DH_CT:        u64 = DH_LINK + 8;
const DH_XTPTR:     u64 = DH_CT + 8;
const DH_COMP:      u64 = DH_XTPTR + 8;
const DH_REC:       u64 = DH_COMP + 8;
const DH_VFA:       u64 = DH_REC + 8;
const DH_OFA:       u64 = DH_VFA + 2;
const DH_STK:       u64 = DH_OFA + 2;
const DH_TFA:       u64 = DH_STK + 2;
const DH_NT:        u64 = DH_TFA + 1;
const DH_NAME:      u64 = DH_NT + 1;
const XT_META_OFFSET: u64 = 8;

// ── Win32 FFI for VirtualAlloc2 + MEM_ADDRESS_REQUIREMENTS ───────────

#[repr(C)]
struct MEM_ADDRESS_REQUIREMENTS {
    LowestStartingAddress: *mut c_void,
    HighestEndingAddress:  *mut c_void,
    Alignment:             usize,
}
const MemExtendedParameterAddressRequirements: u64 = 1;

#[repr(C)]
struct MEM_EXTENDED_PARAMETER {
    type_and_reserved: u64,
    pointer:           *mut c_void,
}

const MEM_RESERVE: u32 = 0x00002000;
const MEM_COMMIT:  u32 = 0x00001000;
const PAGE_EXECUTE_READWRITE: u32 = 0x40;
const PAGE_SIZE: usize = 4096;

#[repr(C)]
#[derive(Clone, Copy)]
struct RuntimeFunction {
    BeginAddress: u32,
    EndAddress: u32,
    UnwindData: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UnwindInfo {
    version_and_flags: u8,
    size_of_prolog: u8,
    count_of_codes: u8,
    frame_register_and_offset: u8,
}

impl UnwindInfo {
    const fn leaf() -> Self {
        Self {
            version_and_flags: 1,
            size_of_prolog: 0,
            count_of_codes: 0,
            frame_register_and_offset: 0,
        }
    }
}

type VirtualAlloc2Fn = unsafe extern "system" fn(
    *mut c_void, *mut c_void, usize, u32, u32,
    *mut MEM_EXTENDED_PARAMETER, u32,
) -> *mut c_void;

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const i8) -> *mut c_void;
    fn GetLastError() -> u32;
    fn VirtualProtect(
        lpAddress: *mut c_void,
        dwSize: usize,
        flNewProtect: u32,
        lpflOldProtect: *mut u32,
    ) -> i32;
    fn RtlAddFunctionTable(
        FunctionTable: *const RuntimeFunction,
        EntryCount: u32,
        BaseAddress: u64,
    ) -> u8;
    fn RtlDeleteFunctionTable(FunctionTable: *const RuntimeFunction) -> u8;
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RuntimeWord {
    name: String,
    header: u64,
    start: u64,
    end: u64,
}

struct RegisteredFunctionTable {
    entries: Box<[RuntimeFunction]>,
}

// ── Forth error helpers ───────────────────────────────────────────────────

/// Decorate a raw `"Forth THROW n"` error with a human-readable description
/// and any output the interpreter had already captured before the throw fired.
fn annotate_forth_error(e: anyhow::Error, partial_output: &str) -> anyhow::Error {
    let msg = e.to_string();
    if let Some(tail) = msg.strip_prefix("Forth THROW ") {
        if let Ok(code) = tail.trim().parse::<i64>() {
            let desc = forth_throw_description(code);
            let pre = partial_output.trim_end_matches('\n');
            return if pre.is_empty() {
                anyhow::anyhow!("Forth error ({code}): {desc}")
            } else {
                anyhow::anyhow!("Forth error ({code}): {desc}\n{pre}")
            };
        }
    }
    e // not a structured THROW — pass through unchanged
}

/// Map an ANS-standard THROW code to a short English description.
/// Covers the full ANS table (codes −1 .. −58) plus WF64 extensions.
pub fn forth_throw_description(code: i64) -> &'static str {
    match code {
        -1  => "ABORT",
        -2  => "ABORT\" (with message)",
        -3  => "stack overflow",
        -4  => "stack underflow",
        -5  => "return stack overflow",
        -6  => "return stack underflow",
        -7  => "DO-loop nesting too deep",
        -8  => "dictionary overflow",
        -9  => "invalid memory address",
        -10 => "division by zero",
        -11 => "result out of range",
        -12 => "argument type mismatch",
        -13 => "undefined word",
        -14 => "interpreting a compile-only word",
        -15 => "invalid FORGET",
        -16 => "zero-length definition name",
        -17 => "pictured numeric output string overflow",
        -18 => "parsed string overflow",
        -19 => "definition name too long",
        -20 => "write to a read-only location",
        -21 => "unsupported operation",
        -22 => "control structure mismatch",
        -23 => "address alignment exception",
        -24 => "invalid numeric argument",
        -25 => "return stack imbalance",
        -26 => "loop parameters unavailable",
        -27 => "invalid recursion",
        -28 => "user interrupt",
        -29 => "compiler nesting",
        -30 => "obsolescent feature",
        -31 => ">BODY used on a non-CREATEd word",
        -32 => "invalid name argument (e.g. TO xxx)",
        -33 => "block read exception",
        -34 => "block write exception",
        -35 => "invalid block number",
        -36 => "invalid file position",
        -37 => "file I/O exception",
        -38 => "file not found",
        -39 => "unexpected end of file",
        -40 => "invalid BASE for floating-point conversion",
        -41 => "loss of precision",
        -42 => "floating-point divide by zero",
        -43 => "floating-point result out of range",
        -44 => "floating-point stack overflow",
        -45 => "floating-point stack underflow",
        -46 => "floating-point invalid argument",
        -47 => "compilation word list deleted",
        -48 => "invalid POSTPONE",
        -49 => "search-order overflow",
        -50 => "search-order underflow",
        -51 => "compilation word list changed",
        -52 => "control-flow stack overflow",
        -53 => "exception stack overflow",
        -54 => "floating-point underflow",
        -55 => "floating-point unidentified fault",
        -56 => "QUIT",
        -57 => "exception in character I/O",
        -58 => "[IF]/[ELSE]/[THEN] exception",
        _   => "Forth exception",
    }
}

// ─────────────────────────────────────────────────────────────────────────────

fn align_up(addr: u64, align: u64) -> u64 {
    debug_assert!(align.is_power_of_two());
    (addr + align - 1) & !(align - 1)
}

fn get_virtual_alloc2() -> Result<VirtualAlloc2Fn> {
    let dll: Vec<u16> = "api-ms-win-core-memory-l1-1-6.dll"
        .encode_utf16().chain(std::iter::once(0)).collect();
    let hmod = unsafe { LoadLibraryW(dll.as_ptr()) };
    if hmod.is_null() {
        anyhow::bail!("LoadLibraryW(api-ms-win-core-memory-l1-1-6.dll) failed");
    }
    let name = b"VirtualAlloc2\0";
    let addr = unsafe { GetProcAddress(hmod, name.as_ptr() as *const i8) };
    if addr.is_null() {
        anyhow::bail!("GetProcAddress(VirtualAlloc2) failed");
    }
    Ok(unsafe { std::mem::transmute::<*mut c_void, VirtualAlloc2Fn>(addr) })
}

fn alloc_forth_region(kernel_addr: u64) -> Result<*mut c_void> {
    let va2 = get_virtual_alloc2().context("locate VirtualAlloc2")?;
    const GRANULARITY: u64 = 0x10000;
    const WINDOW:      u64 = 0x70000000;
    let low_raw = kernel_addr.saturating_sub(WINDOW);
    let low_aligned = ((low_raw + GRANULARITY - 1) & !(GRANULARITY - 1))
        .max(GRANULARITY);
    let high_raw = kernel_addr.saturating_add(WINDOW);
    let high_inclusive = (high_raw & !(GRANULARITY - 1)).saturating_sub(1);

    let mut req = MEM_ADDRESS_REQUIREMENTS {
        LowestStartingAddress: low_aligned as *mut c_void,
        HighestEndingAddress:  high_inclusive as *mut c_void,
        Alignment: 0,
    };
    let mut param = MEM_EXTENDED_PARAMETER {
        type_and_reserved: MemExtendedParameterAddressRequirements,
        pointer: &mut req as *mut _ as *mut c_void,
    };
    let base = unsafe {
        va2(
            ptr::null_mut(), ptr::null_mut(), REGION_SIZE,
            MEM_RESERVE | MEM_COMMIT, PAGE_EXECUTE_READWRITE,
            &mut param, 1,
        )
    };
    if base.is_null() {
        let err = unsafe { GetLastError() };
        anyhow::bail!(
            "VirtualAlloc2 returned null (GetLastError = {err}); kernel \
             at {kernel_addr:#018x}, window [{low_aligned:#018x} \
             .. {high_inclusive:#018x}]"
        );
    }
    Ok(base)
}

// Near JIT code arena for runtime CODE:/LET words. Anchored near the Forth
// region (not the kernel): the region is already within ±1.75 GB of the
// kernel, and a ±128 MB window keeps the arena within rel32 (±2 GB) of BOTH
// the kernel and the dict heap, so `call rel32` reaches it either way.
// RWX so the SimpleMCJITMemoryManager's FinalizeMemory can be a no-op.
fn alloc_jit_arena(anchor: u64) -> Result<*mut c_void> {
    let va2 = get_virtual_alloc2().context("locate VirtualAlloc2")?;
    const GRANULARITY: u64 = 0x10000;
    const WINDOW:      u64 = 0x08000000; // ±128 MB
    let low_aligned = ((anchor.saturating_sub(WINDOW) + GRANULARITY - 1)
        & !(GRANULARITY - 1)).max(GRANULARITY);
    let high_inclusive = (anchor.saturating_add(WINDOW) & !(GRANULARITY - 1))
        .saturating_sub(1);
    let mut req = MEM_ADDRESS_REQUIREMENTS {
        LowestStartingAddress: low_aligned as *mut c_void,
        HighestEndingAddress:  high_inclusive as *mut c_void,
        Alignment: 0,
    };
    let mut param = MEM_EXTENDED_PARAMETER {
        type_and_reserved: MemExtendedParameterAddressRequirements,
        pointer: &mut req as *mut _ as *mut c_void,
    };
    let base = unsafe {
        va2(
            ptr::null_mut(), ptr::null_mut(), JIT_ARENA_SIZE,
            MEM_RESERVE | MEM_COMMIT, PAGE_EXECUTE_READWRITE,
            &mut param, 1,
        )
    };
    if base.is_null() {
        let err = unsafe { GetLastError() };
        anyhow::bail!(
            "JIT-arena VirtualAlloc2 returned null (GetLastError = {err}); \
             anchor {anchor:#018x}, window [{low_aligned:#018x} .. {high_inclusive:#018x}]"
        );
    }
    Ok(base)
}

// Locals stack region — a separate 1 MB block. Addressed exclusively
// through R15-relative loads, so it doesn't need to live near the
// kernel; plain VirtualAlloc is enough.
extern "system" {
    fn VirtualAlloc(
        lpAddress: *mut c_void,
        dwSize: usize,
        flAllocationType: u32,
        flProtect: u32,
    ) -> *mut c_void;
    fn VirtualFree(lpAddress: *mut c_void, dwSize: usize, dwFreeType: u32) -> i32;
}
const PAGE_READWRITE: u32 = 0x04;
const MEM_RELEASE:    u32 = 0x8000;

fn alloc_locals_region() -> Result<*mut c_void> {
    let base = unsafe {
        VirtualAlloc(
            ptr::null_mut(),
            LOCALS_REGION_SIZE,
            MEM_RESERVE | MEM_COMMIT,
            PAGE_READWRITE,
        )
    };
    if base.is_null() {
        let err = unsafe { GetLastError() };
        anyhow::bail!("locals region VirtualAlloc returned null (GetLastError = {err})");
    }
    Ok(base)
}

/// RW / no-execute region holding all data-space allocations (variable and
/// create-word bodies). Separate from the executable dictionary for W^X.
fn alloc_var_region() -> Result<*mut c_void> {
    let base = unsafe {
        VirtualAlloc(
            ptr::null_mut(),
            VAR_REGION_SIZE,
            MEM_RESERVE | MEM_COMMIT,
            PAGE_READWRITE,
        )
    };
    if base.is_null() {
        let err = unsafe { GetLastError() };
        anyhow::bail!("data (variable) region VirtualAlloc returned null (GetLastError = {err})");
    }
    Ok(base)
}

// ── Wf64Session ──────────────────────────────────────────────────────

/// `forth_main(target_xt, logical_dsp_in, rsp_top, user_base) → 0`.
type ForthMain = unsafe extern "system" fn(u64, u64, u64, u64) -> u64;

pub struct Wf64Session {
    jit: Jit,
    forth_main: ForthMain,
    #[allow(dead_code)]
    region_base: u64,
    /// Base of the 1 MB locals stack region (separate VirtualAlloc).
    /// Released by `Drop`. R15 (the locals stack pointer) is initialised
    /// to `locals_base + LOCALS_REGION_SIZE` (top) at every `forth_main`
    /// entry and grows downward.
    locals_base: *mut c_void,
    pub dsp_top:   u64,
    pub rsp_top:   u64,
    pub user_base: u64,
    pub dict_base: u64,
    /// Base of the RW / no-execute data region (variable / create bodies).
    var_base: u64,
    debug_meta_base: u64,

    /// Current "logical" data stack pointer. Equal to `dsp_top` when
    /// the stack is empty. Each cell from `current_dsp` upward (towards
    /// `dsp_top`) is one stack entry, top-of-stack first.
    current_dsp: u64,

    /// Runtime-created colon definitions currently visible in the
    /// dictionary, newest or oldest order unspecified.
    runtime_words: Vec<RuntimeWord>,
    debug_synced_here: u64,
    debug_synced_latest: u64,
    debug_function_table: Option<RegisteredFunctionTable>,
    debug_tracking_enabled: bool,

    /// Post-bootstrap HERE — captured once, used by [`reset`] to roll
    /// the dict heap back to "just after the bootstrap primitives were
    /// registered." Everything between `boot_here..` is throwaway: it
    /// was either a colon definition compiled during a test, or stale
    /// bytes from a prior test. Reset just moves HERE back and lets the
    /// next test compile over the top.
    boot_here:   u64,
    /// Post-bootstrap LATEST — companion to `boot_here`. Paired so the
    /// dictionary chain head also rolls back; otherwise `find-name`
    /// would still see test-defined words pointing into freed body
    /// memory.
    boot_latest: u64,
    /// Post-bootstrap latestxt companion so reset restores the same
    /// most-recent definition metadata that the dictionary exposes.
    boot_latestxt: u64,
    boot_index_here: u64,
    boot_index_latest: u64,
    /// Post-bootstrap data-space pointer (USER_VAR_HERE) — reset rolls the
    /// variable region back to here, mirroring boot_here for the code region.
    boot_var_here: u64,
    /// Snapshot of the FORTH-WORDLIST bucket array (512 × 8 bytes)
    /// taken right after bootstrap. `reset()` writes this back so that
    /// overlay nodes allocated by one test cannot leave stale bucket
    /// pointers that create circular chains when the next test reuses
    /// the same overlay addresses.
    boot_wl_buckets: Vec<u64>,
    boot_tools_buckets: Vec<u64>,
    boot_private_buckets: Vec<u64>,
    boot_ivars_buckets: Vec<u64>,
}

// SAFETY: Wf64Session holds raw LLVM pointers via `Jit` that aren't
// `Send` by default. We rely on this Send impl ONLY to park the session
// in a `OnceLock<Mutex<…>>` for cross-test sharing — and tests run
// single-threaded (see .cargo/config.toml RUST_TEST_THREADS=1), so the
// session is never actually accessed from more than one thread. The
// Mutex enforces the serial-access discipline; the Send impl makes the
// type fit through the OnceLock's `Sync` requirement.
unsafe impl Send for Wf64Session {}

impl Wf64Session {
    pub fn new() -> Result<Self> {
        Self::with_kernel(default_kernel_path())
    }

    pub fn with_kernel(kernel_path: impl AsRef<Path>) -> Result<Self> {
        let kernel_path = kernel_path.as_ref();
        if !kernel_path.exists() {
            anyhow::bail!(
                "kernel entry not found at {} — run from the WF64 root, or \
                 pass an explicit path",
                kernel_path.display()
            );
        }

        // SEH dumper is process-wide; install only on first call.
        let _ = wfasm::seh::install();

        // Assemble.
        let mut asm = Assembler::new();
        asm.register_macro("stk", wfasm::asm::macros::stk);
        let asm_text = asm
            .assemble_file(kernel_path)
            .with_context(|| format!("assemble {}", kernel_path.display()))?;

        if std::env::var_os("WF64_DUMP_ASM").is_some() {
            eprintln!("=== assembled kernel ===\n{asm_text}========================");
        }

        // JIT setup + declarations.
        let mut jit = Jit::new("wf64").context("Jit::new")?;
        jit.add_asm(&asm_text).context("jit add_asm")?;
        jit.declare_fn("forth_main", 1).context("declare forth_main")?;
        for &(_, sym, _) in PRIMITIVES {
            jit.declare_fn(sym, 0)
                .with_context(|| format!("declare {sym}"))?;
        }
        for &helper in KERNEL_HELPERS {
            jit.declare_fn(helper, 0)
                .with_context(|| format!("declare helper {helper}"))?;
        }

        // Bind externs.
        let _ = wfasm::win32::bind_externs(&asm, &mut jit, |name| -> Option<*mut c_void> {
            match name {
                "rt_print_int" => Some(runtime::rt_print_int as *mut c_void),
                "rt_dot_s"     => Some(runtime::rt_dot_s     as *mut c_void),
                "rt_emit"      => Some(runtime::rt_emit      as *mut c_void),
                "rt_type"      => Some(runtime::rt_type      as *mut c_void),
                "rt_bye"       => Some(runtime::rt_bye       as *mut c_void),
                "rt_read_line" => Some(runtime::rt_read_line as *mut c_void),
                "rt_read_key"  => Some(runtime::rt_read_key  as *mut c_void),
                "rt_key_q"     => Some(runtime::rt_key_q     as *mut c_void),
                "rt_to_float"    => Some(runtime::rt_to_float    as *mut c_void),
                "rt_forth_brk"   => Some(runtime::rt_forth_brk   as *mut c_void),
                "rt_forth_trace" => Some(runtime::rt_forth_trace  as *mut c_void),
                "rt_slurp_file"  => Some(runtime::rt_slurp_file   as *mut c_void),
                "rt_slurp_len"   => Some(runtime::rt_slurp_len    as *mut c_void),
                "rt_slurp_pop"   => Some(runtime::rt_slurp_pop    as *mut c_void),
                "rt_let_compile" => Some(runtime::rt_let_compile as *mut c_void),
                "rt_code_compile_body" => Some(runtime::rt_code_compile_body as *mut c_void),
                "rt_vec_alloc_floats" => Some(runtime::rt_vec_alloc_floats as *mut c_void),
                "rt_vec_alloc_refs"   => Some(runtime::rt_vec_alloc_refs   as *mut c_void),
                "rt_gc_collect"       => Some(runtime::rt_gc_collect       as *mut c_void),
                "rt_gc_collect_minor" => Some(runtime::rt_gc_collect_minor as *mut c_void),
                "rt_gc_collect_full"  => Some(runtime::rt_gc_collect_full  as *mut c_void),
                "rt_gc_auto_step"     => Some(runtime::rt_gc_auto_step     as *mut c_void),
                "rt_gc_should_collect" => Some(runtime::rt_gc_should_collect as *mut c_void),
                "rt_gc_cycle_count"   => Some(runtime::rt_gc_cycle_count   as *mut c_void),
                "rt_string_from_bytes"  => Some(runtime::rt_string_from_bytes  as *mut c_void),
                "rt_string_bytes_equal" => Some(runtime::rt_string_bytes_equal as *mut c_void),
                "rt_s_literal_compile_at_here" => Some(runtime::rt_s_literal_compile_at_here as *mut c_void),
                "rt_sb_new"             => Some(runtime::rt_sb_new             as *mut c_void),
                "rt_sb_append_bytes"    => Some(runtime::rt_sb_append_bytes    as *mut c_void),
                "rt_sb_append_codepoint"=> Some(runtime::rt_sb_append_codepoint as *mut c_void),
                "rt_sb_append_int"      => Some(runtime::rt_sb_append_int      as *mut c_void),
                "rt_sb_to_string"       => Some(runtime::rt_sb_to_string       as *mut c_void),
                "rt_string_concat"      => Some(runtime::rt_string_concat      as *mut c_void),
                "rt_string_slice"       => Some(runtime::rt_string_slice       as *mut c_void),
                "rt_string_find"        => Some(runtime::rt_string_find        as *mut c_void),
                "rt_string_starts"      => Some(runtime::rt_string_starts      as *mut c_void),
                "rt_string_ends"        => Some(runtime::rt_string_ends        as *mut c_void),
                "rt_string_cmp"         => Some(runtime::rt_string_cmp         as *mut c_void),
                "rt_string_hash"        => Some(runtime::rt_string_hash        as *mut c_void),
                "rt_string_ci_eq"       => Some(runtime::rt_string_ci_eq       as *mut c_void),
                "rt_string_trim"        => Some(runtime::rt_string_trim        as *mut c_void),
                "rt_string_ltrim"       => Some(runtime::rt_string_ltrim       as *mut c_void),
                "rt_string_rtrim"       => Some(runtime::rt_string_rtrim       as *mut c_void),
                "rt_int_to_string"      => Some(runtime::rt_int_to_string      as *mut c_void),
                "rt_string_to_int"      => Some(runtime::rt_string_to_int      as *mut c_void),
                "rt_string_contains"    => Some(runtime::rt_string_contains    as *mut c_void),
                "rt_string_rfind"       => Some(runtime::rt_string_rfind       as *mut c_void),
                "rt_string_repeat"      => Some(runtime::rt_string_repeat      as *mut c_void),
                "rt_string_replace"     => Some(runtime::rt_string_replace     as *mut c_void),
                "rt_string_split_into"  => Some(runtime::rt_string_split_into  as *mut c_void),
                "rt_string_clen"        => Some(runtime::rt_string_clen        as *mut c_void),
                "rt_string_cat"         => Some(runtime::rt_string_cat         as *mut c_void),
                "rt_string_valid"       => Some(runtime::rt_string_valid       as *mut c_void),
                "rt_char_to_string"     => Some(runtime::rt_char_to_string     as *mut c_void),
                "rt_string_upper"       => Some(runtime::rt_string_upper       as *mut c_void),
                "rt_string_lower"       => Some(runtime::rt_string_lower       as *mut c_void),
                "rt_string_to_float"    => Some(runtime::rt_string_to_float    as *mut c_void),
                "rt_float_to_string"    => Some(runtime::rt_float_to_string    as *mut c_void),
                "rt_sb_append_float"    => Some(runtime::rt_sb_append_float    as *mut c_void),
                "rt_string_words_into"  => Some(runtime::rt_string_words_into  as *mut c_void),
                "rt_igui_page"          => Some(runtime::rt_igui_page          as *mut c_void),
                "rt_igui_at_xy"         => Some(runtime::rt_igui_at_xy         as *mut c_void),
                "rt_bug_rust_panic"     => Some(runtime::rt_bug_rust_panic     as *mut c_void),
                "rt_bug_seh_av"         => Some(runtime::rt_bug_seh_av         as *mut c_void),
                "rt_gpane_open"         => Some(runtime::rt_gpane_open         as *mut c_void),
                "rt_gpane_begin"        => Some(runtime::rt_gpane_begin        as *mut c_void),
                "rt_gpane_present"      => Some(runtime::rt_gpane_present      as *mut c_void),
                "rt_gpane_clear"        => Some(runtime::rt_gpane_clear        as *mut c_void),
                "rt_gpane_fill_rect"    => Some(runtime::rt_gpane_fill_rect    as *mut c_void),
                "rt_gpane_stroke_rect"  => Some(runtime::rt_gpane_stroke_rect  as *mut c_void),
                "rt_gpane_line"         => Some(runtime::rt_gpane_line         as *mut c_void),
                "rt_gpane_fill_circle"  => Some(runtime::rt_gpane_fill_circle  as *mut c_void),
                "rt_canvas_blit"        => Some(runtime::rt_canvas_blit        as *mut c_void),
                "rt_doc_open"           => Some(runtime::rt_doc_open           as *mut c_void),
                "rt_doc_set"            => Some(runtime::rt_doc_set            as *mut c_void),
                "rt_doc_append"         => Some(runtime::rt_doc_append         as *mut c_void),
                "rt_gpane_next_event_for" => Some(runtime::rt_gpane_next_event_for as *mut c_void),
                _ => None,
            }
        }).context("bind_externs failed")?;

        // Pick near memory.
        let kernel_addr = jit.lookup_addr("forth_main")
            .context("lookup_addr(forth_main)")?;
        let region_base = alloc_forth_region(kernel_addr)?;
        let region_u64 = region_base as u64;

        // Near JIT code arena for runtime CODE:/LET words (rel32-reachable
        // from both the kernel and the dict region). Published to the user
        // area below; runtime.rs builds a CodeArena from it.
        let jit_arena_base = alloc_jit_arena(region_u64)?;

        // Locals stack — independent 1 MB allocation, R15 grows down from
        // the top. Released by Drop. Far address is fine: only ever
        // accessed via R15-relative loads, no rel32 constraint.
        let locals_base = alloc_locals_region()?;
        let locals_base_u64 = locals_base as u64;
        let locals_top = locals_base_u64 + LOCALS_REGION_SIZE as u64;

        // Data space (variables / create bodies) — separate RW / no-execute
        // region so writes never touch executable code (W^X, no SMC clears).
        let var_base = alloc_var_region()? as u64;

        let dsp_top   = region_u64 + OFFSET_DSP_TOP;
        let rsp_top   = region_u64 + OFFSET_RSP_TOP;
        let user_base = region_u64 + OFFSET_USER_BASE;
        let dict_base = region_u64 + OFFSET_DICT_BASE;
        let debug_meta_base = region_u64 + REGION_SIZE as u64 - DEBUG_META_SIZE;

        // Initialise user area with an *empty* dictionary. The bootstrap
        // below will populate it by calling `(create)` and friends.
        unsafe {
            let up = user_base as *mut c_void;
            write_u64(up, USER_BASE_VAR,     10);
            write_u64(up, USER_STATE_VAR,    0);
            write_u64(up, USER_LATEST_VAR,   0);                 // empty
            write_u64(up, USER_HERE_VAR,     dict_base);
            write_u64(up, USER_VAR_HERE,     var_base);
            write_u64(up, USER_VAR_LIMIT,    var_base + VAR_REGION_SIZE as u64);
            write_u64(up, USER_DICT_END_VAR, debug_meta_base);
            write_u64(up, USER_PARSE_BARRIER, 0);
            write_u64(up, USER_BYE_REQ,      0);
            write_u64(up, USER_SP0,          dsp_top);
            write_u64(up, USER_RSP_CURRENT,  rsp_top);
            write_u64(up, USER_LATESTXT_VAR, 0);
            write_u64(up, USER_HANDLER_VAR,  0);
            write_u64(up, USER_THROW_CODE,   0);
            write_u64(up, USER_FORGET_FENCE, 0);
            write_u64(up, USER_FP0,          user_base + USER_FP_STACK + 0x100);
            write_u64(up, USER_FSP,          user_base + USER_FP_STACK + 0x100);
            write_u64(up, USER_CURRENT,      0);
            write_u64(up, USER_FORTH_WID,    0);
            write_u64(up, USER_ORDER_COUNT,  0);
            write_u64(up, USER_INDEX_HERE,   debug_meta_base);
            write_u64(up, USER_INDEX_LATEST, 0);
            write_u64(up, USER_LP0,          locals_top);
            write_u64(up, USER_LP_LIMIT,     locals_base_u64);
            write_u64(up, USER_LOCALS_COUNT, 0);
            // HEAPPTR region: bump pointer starts at the base (= no
            // slots in use).  The region itself is zero-filled
            // already because the user area was VirtualAlloc'd
            // (pages start zero) and nothing has written to it yet.
            write_u64(up, USER_HEAPPTR_NEXT, user_base + USER_HEAPPTR_BASE);
            // LITERAL region: same shape, distinct region — used by
            // S$" for compile-time string literals.
            write_u64(up, USER_LITERAL_NEXT, user_base + USER_LITERAL_BASE);
            // Publish the near JIT code arena (runtime CODE:/LET).
            write_u64(up, USER_JIT_ARENA_BASE, jit_arena_base as u64);
            write_u64(up, USER_JIT_ARENA_SIZE, JIT_ARENA_SIZE as u64);
        }

        // Register kernel procs with SEH for symbolic crash dumps.
        let proc_names: Vec<&str> = std::iter::once("forth_main")
            .chain(PRIMITIVES.iter().map(|&(_, sym, _)| sym))
            .chain(KERNEL_HELPERS.iter().copied())
            .collect();
        wfasm::seh::register_jit_procs(&mut jit, &proc_names)
            .context("register kernel procs with SEH")?;

        let forth_main: ForthMain = unsafe { jit.lookup_fn("forth_main") }
            .context("lookup forth_main")?;

        let mut session = Wf64Session {
            jit,
            forth_main,
            region_base: region_u64,
            locals_base,
            dsp_top,
            rsp_top,
            user_base,
            dict_base,
            var_base,
            debug_meta_base,
            current_dsp: dsp_top,
            runtime_words: Vec::new(),
            debug_synced_here: dict_base,
            debug_synced_latest: 0,
            debug_function_table: None,
            debug_tracking_enabled: false,
            // Provisional — overwritten after bootstrap. We need the
            // session constructed to call `bootstrap_dictionary` (which
            // mutates `self.jit` and the user area), so the post-boot
            // snapshot can only happen after.
            boot_here: 0,
            boot_latest: 0,
            boot_latestxt: 0,
            boot_index_here: 0,
            boot_index_latest: 0,
            boot_var_here: 0,
            boot_wl_buckets: Vec::new(),
            boot_tools_buckets: Vec::new(),
            boot_private_buckets: Vec::new(),
            boot_ivars_buckets: Vec::new(),
        };

        let xt_init_dictionary_overlay = session.xt_of("init_dictionary_overlay")?;
        session.call_xt(xt_init_dictionary_overlay)?;

        // Bootstrap the dictionary via kernel primitives. From this
        // point on, the kernel's view of HERE/LATEST is the only one
        // we have — Rust knows nothing about the header layout.
        session.bootstrap_dictionary()?;

        // Load the standard source library (lib/core.f) *before* taking
        // the boot snapshot.  This ensures that reset() rolls back only
        // to the post-core.f state, so source-defined words like `{:`,
        // `to`, `if`, `then`, `begin`, `while`, etc. are always present
        // without having to re-load them per test.  main.rs and wf64-ui
        // no longer need to call load_source_file("lib/core.f") because
        // with_kernel() / new() now does it.
        let core_path = kernel_path
            .parent()
            .and_then(|p| p.parent())
            .unwrap_or(Path::new("."))
            .join("lib")
            .join("core.f");
        if core_path.exists() {
            session.load_source_file(&core_path)
                .with_context(|| format!("boot: load {}", core_path.display()))?;
        }

        // Load the object system (lib/oop.f) on top of core.f, still
        // *before* the boot snapshot so its selector table sits below the
        // reset fence and its words survive reset() like core.f's do.
        let oop_path = core_path.with_file_name("oop.f");
        if oop_path.exists() {
            session.load_source_file(&oop_path)
                .with_context(|| format!("boot: load {}", oop_path.display()))?;
        }

        session.boot_here   = session.here();
        session.boot_latest = session.latest();
        session.boot_latestxt = session.user_u64(USER_LATESTXT_VAR);
        session.boot_index_here = session.user_u64(USER_INDEX_HERE);
        // Data-space pointer after core.f/oop.f — reset rolls the variable
        // region back to here so post-bootstrap bodies survive but test bodies
        // are reclaimed (mirrors boot_here for the code region).
        session.boot_var_here = session.user_u64(USER_VAR_HERE);
        session.boot_index_latest = session.user_u64(USER_INDEX_LATEST);
        // Snapshot the FORTH-WORDLIST bucket array so reset() can
        // restore it. Without this, overlay nodes allocated by a test
        // leave stale bucket heads that create circular chains when the
        // next test reuses the same overlay addresses.
        let forth_wid = session.user_u64(USER_FORTH_WID);
        let tools_wid_snap   = session.user_u64(USER_TOOLS_WID);
        let private_wid_snap = session.user_u64(USER_PRIVATE_WID);
        session.boot_wl_buckets = unsafe {
            std::slice::from_raw_parts(forth_wid as *const u64, 512)
                .to_vec()
        };
        session.boot_tools_buckets = unsafe {
            std::slice::from_raw_parts(tools_wid_snap as *const u64, 512)
                .to_vec()
        };
        session.boot_private_buckets = unsafe {
            std::slice::from_raw_parts(private_wid_snap as *const u64, 512)
                .to_vec()
        };
        // OOP ivar wordlist (created by lib/oop.f, which publishes its wid to
        // USER_OOP_IVARS_WID). Snapshot its buckets so reset() can clear the
        // scoped ivar entries a test adds. Empty Vec if oop.f wasn't loaded.
        let ivars_wid_snap = session.user_u64(USER_OOP_IVARS_WID);
        session.boot_ivars_buckets = if ivars_wid_snap != 0 {
            unsafe {
                std::slice::from_raw_parts(ivars_wid_snap as *const u64, 512).to_vec()
            }
        } else {
            Vec::new()
        };
        session.write_user_u64(USER_FORGET_FENCE, session.boot_latest);
        session.debug_synced_here = session.boot_here;
        session.debug_synced_latest = session.boot_latest;
        session.debug_tracking_enabled = true;

        if std::env::var_os("WF64_BOOT_INFO").is_some() {
            eprintln!("WF64 boot info:");
            eprintln!("  kernel addr  = {kernel_addr:#018x}");
            eprintln!("  region base  = {region_u64:#018x}");
            eprintln!("  HERE         = {:#018x}", session.here());
            eprintln!("  LATEST       = {:#018x}", session.latest());
            eprintln!("  primitives   = {}", PRIMITIVES.len());
        }

        Ok(session)
    }

    /// Populate the initial dictionary by calling the kernel-side
    /// primitive publisher once per primitive. Runs after the session
    /// is otherwise fully constructed.
    fn bootstrap_dictionary(&mut self) -> Result<()> {
        // Collect the xts first to avoid mutable-borrow conflicts on
        // self.jit during the iteration.
        let entries: Vec<(&'static str, &'static str, u64, u8)> = PRIMITIVES.iter()
            .map(|&(forth_name, asm_sym, flags)| -> Result<_> {
                let xt = self.jit.lookup_addr(asm_sym)
                    .with_context(|| format!("lookup_addr({asm_sym}) for `{forth_name}`"))?;
                Ok((forth_name, asm_sym, xt, flags))
            })
            .collect::<Result<_>>()?;

        let xt_publish_primitive = self.xt_of("publish_primitive")?;

        // Create the TOOLS and PRIVATE wordlists by carving them out of
        // the index arena (same layout that the kernel's `wordlist`
        // primitive uses). Each is wl_size = 512 * 8 = 4096 bytes,
        // zeroed.
        const WL_SIZE: u64 = 512 * 8;
        let tools_wid = self.allocate_wordlist(WL_SIZE);
        let private_wid = self.allocate_wordlist(WL_SIZE);
        self.write_user_u64(USER_TOOLS_WID,   tools_wid);
        self.write_user_u64(USER_PRIVATE_WID, private_wid);

        let forth_wid = self.user_u64(USER_FORTH_WID);
        let tools_set:   std::collections::HashSet<&str> = TOOLS_WORDS.iter().copied().collect();
        let private_set: std::collections::HashSet<&str> = PRIVATE_WORDS.iter().copied().collect();

        let pad = self.user_base + USER_PAD;

        for (forth_name, asm_sym, xt, flags) in entries {
            // Pick the destination wordlist for this primitive.
            let target_wid = if private_set.contains(forth_name) {
                private_wid
            } else if tools_set.contains(forth_name) {
                tools_wid
            } else {
                forth_wid
            };
            self.write_user_u64(USER_CURRENT, target_wid);

            // Copy the name bytes into PAD.
            let name = forth_name.as_bytes();
            assert!(!name.is_empty() && name.len() <= 255,
                "primitive name `{forth_name}` length out of range");
            unsafe {
                std::ptr::copy_nonoverlapping(name.as_ptr(), pad as *mut u8, name.len());
            }
            let comp_xt = self.primitive_comp_helper(asm_sym)?.unwrap_or(0);
            self.push(pad as i64);
            self.push(name.len() as i64);
            self.push(xt as i64);
            self.push(comp_xt as i64);
            self.push(flags as i64);
            self.call_xt(xt_publish_primitive)?;
            let header = self.latest();
            self.write_primitive_xt_backref(xt, header)?;
        }

        // Restore CURRENT = FORTH for any subsequent definitions.
        self.write_user_u64(USER_CURRENT, forth_wid);

        // While core.f is being loaded, the search order needs all
        // three wordlists visible so that core.f source can reference
        // PRIVATE primitives like `(open-locals)` and the {: parser
        // helpers. The caller (or core.f itself) is responsible for
        // narrowing the search order back to just FORTH at the end of
        // loading.
        self.write_user_u64(USER_ORDER_COUNT, 3);
        // Search order: index 0 = innermost = PRIVATE, then TOOLS, then FORTH.
        let context = self.user_base + USER_CONTEXT;
        unsafe {
            (context as *mut u64).offset(0).write_unaligned(private_wid);
            (context as *mut u64).offset(1).write_unaligned(tools_wid);
            (context as *mut u64).offset(2).write_unaligned(forth_wid);
        }

        // Stack should be empty after the bootstrap.
        debug_assert_eq!(self.depth(), 0,
            "bootstrap left {} cells on the stack", self.depth());
        Ok(())
    }

    /// Carve a fresh wordlist bucket area out of the downward-growing
    /// index arena. Mirrors what the kernel's `wordlist` primitive
    /// does, but is callable from Rust at bootstrap before any kernel
    /// state is settled.
    fn allocate_wordlist(&mut self, wl_size: u64) -> u64 {
        let index_here = self.user_u64(USER_INDEX_HERE);
        let new_wl = index_here - wl_size;
        self.write_user_u64(USER_INDEX_HERE, new_wl);
        unsafe {
            std::ptr::write_bytes(new_wl as *mut u8, 0, wl_size as usize);
        }
        new_wl
    }

    fn primitive_comp_helper(&mut self, asm_sym: &str) -> Result<Option<u64>> {
        let helper = match asm_sym {
            "dup_" => Some("inline_dup_comp"),
            "drop_" => Some("inline_drop_comp"),
            "swap_" => Some("inline_swap_comp"),
            "over_" => Some("inline_over_comp"),
            // Bare memory ops: inline the load/store instead of a CALL. A bare
            // @ / ! has no foldable literal operand, so inlining is the only win.
            "fetch" => Some("inline_fetch_comp"),
            "store" => Some("inline_store_comp"),
            "c_fetch" => Some("inline_c_fetch_comp"),
            "c_store" => Some("inline_c_store_comp"),
            // Remaining stack shuffles (rot/-rot/nip/tuck) — inline register
            // moves instead of a CALL.
            "rot_" => Some("inline_rot_comp"),
            "neg_rot" => Some("inline_neg_rot_comp"),
            "nip_" => Some("inline_nip_comp"),
            "tuck_" => Some("inline_tuck_comp"),
            // Unary zero-compares: copy the (leaf) primitive body inline.
            "zero_equal" => Some("inline_leaf_comp"),
            "zero_less" => Some("inline_leaf_comp"),
            // Unary arithmetic: single-instruction leaf bodies.
            "one_plus" => Some("inline_leaf_comp"),
            "one_minus" => Some("inline_leaf_comp"),
            "negate" => Some("inline_leaf_comp"),
            "two_times" => Some("inline_leaf_comp"),
            "two_slash" => Some("inline_leaf_comp"),
            "invert" => Some("inline_leaf_comp"),
            // Double-cell stack ops: branch-free leaf bodies (2swap's
            // xchg-rbp,rsp / push-pop trick is self-contained).
            "two_dup" => Some("inline_leaf_comp"),
            "two_drop" => Some("inline_leaf_comp"),
            "two_swap" => Some("inline_leaf_comp"),
            "two_over" => Some("inline_leaf_comp"),
            // cells (shl rax,3), division (cqo;idiv — hardware-traps on /0 like
            // the primitive), and +! (add [rax],rcx) — all branch-free leaves.
            "cells" => Some("inline_leaf_comp"),
            "slash" => Some("inline_leaf_comp"),
            "mod_" => Some("inline_leaf_comp"),
            "slash_mod" => Some("inline_leaf_comp"),
            "plus_store" => Some("inline_leaf_comp"),
            // min/max (cmov) and abs (branchless cqo/xor/sub) — leaf bodies.
            "min_" => Some("inline_leaf_comp"),
            "max_" => Some("inline_leaf_comp"),
            "abs" => Some("inline_leaf_comp"),
            "to_r" => Some("inline_to_r_comp"),
            "r_from" => Some("inline_r_from_comp"),
            "r_fetch" => Some("inline_r_fetch_comp"),
            "two_to_r" => Some("inline_two_to_r_comp"),
            "two_r_from" => Some("inline_two_r_from_comp"),
            "two_r_fetch" => Some("inline_two_r_fetch_comp"),
            "i_word" => Some("inline_i_comp"),
            "j_word" => Some("inline_j_comp"),
            "do_part1" => Some("inline_do_part1_comp"),
            "do_part2" => Some("inline_do_part2_comp"),
            "bra_word" => Some("inline_bra_comp"),
            "qbra_word" => Some("inline_qbra_comp"),
            "minus_qbra_word" => Some("inline_minus_qbra_comp"),
            "bra_qdo_word" => Some("inline_bra_qdo_comp"),
            "loop_word" => Some("inline_loop_comp"),
            "plus_loop_word" => Some("inline_plus_loop_comp"),
            "minus_loop_word" => Some("inline_minus_loop_comp"),
            "unloop_word" => Some("inline_unloop_comp"),
            // TCO-unsafe primitives: they pop / juggle the return address
            // themselves, so the tail-call-opt `; / EXIT` patch (CALL→JMP)
            // would feed them a wrong rstack and crash.
            "rdrop"       => Some("compile_comma_no_tco"),
            "two_rdrop"   => Some("compile_comma_no_tco"),
            "n_to_r"      => Some("compile_comma_no_tco"),
            "nr_from"     => Some("compile_comma_no_tco"),
            // Literal-folding binops: emit an immediate-form instruction
            // when the preceding bytes are `call do_lit ; .quad N`.
            // Falls back to a normal CALL on any non-match.
            "plus"        => Some("fold_plus_comp"),
            "minus"       => Some("fold_minus_comp"),
            "times"       => Some("fold_times_comp"),
            "and_"        => Some("fold_and_comp"),
            "or_"         => Some("fold_or_comp"),
            "xor_"        => Some("fold_xor_comp"),
            "lshift"      => Some("fold_lshift_comp"),
            "rshift"      => Some("fold_rshift_comp"),
            "arshift"     => Some("fold_arshift_comp"),
            "equal"       => Some("fold_equal_comp"),
            "not_equal"   => Some("fold_not_equal_comp"),
            "u_less"      => Some("fold_u_less_comp"),
            "less"        => Some("fold_less_comp"),
            "greater"     => Some("fold_greater_comp"),
            "less_equal"  => Some("fold_less_equal_comp"),
            "greater_equal" => Some("fold_greater_equal_comp"),
            "u_greater"   => Some("fold_u_greater_comp"),
            "u_less_equal" => Some("fold_u_less_equal_comp"),
            "u_greater_equal" => Some("fold_u_greater_equal_comp"),
            _ => None,
        };
        helper
            .map(|name| {
                self.jit.lookup_addr(name)
                    .with_context(|| format!("lookup_addr({name}) for comp helper of `{asm_sym}`"))
            })
            .transpose()
    }

    fn write_primitive_xt_backref(&self, xt: u64, header: u64) -> Result<()> {
        let ct = header + DH_CT;
        let offset = ct as i64 - xt as i64;
        let slot = xt - XT_META_OFFSET;
        let page_mask = (PAGE_SIZE as u64) - 1;
        let page = slot & !page_mask;
        let end_page = (slot + (std::mem::size_of::<i64>() as u64) - 1) & !page_mask;
        let protect_size = if end_page == page { PAGE_SIZE } else { PAGE_SIZE * 2 };
        let mut old_protect = 0;
        unsafe {
            let ok = VirtualProtect(
                page as *mut c_void,
                protect_size,
                PAGE_EXECUTE_READWRITE,
                &mut old_protect,
            );
            if ok == 0 {
                anyhow::bail!(
                    "VirtualProtect RWX failed for primitive xt metadata at {slot:#x}: {}",
                    GetLastError()
                );
            }

            (slot as *mut i64).write(offset);

            let mut restore_unused = 0;
            let ok = VirtualProtect(
                page as *mut c_void,
                protect_size,
                old_protect,
                &mut restore_unused,
            );
            if ok == 0 {
                anyhow::bail!(
                    "VirtualProtect restore failed for primitive xt metadata at {slot:#x}: {}",
                    GetLastError()
                );
            }
        }
        Ok(())
    }

    /// Address of a primitive (or any JITed symbol).
    pub fn xt_of(&mut self, asm_sym: &str) -> Result<u64> {
        self.jit.lookup_addr(asm_sym)
            .with_context(|| format!("lookup_addr({asm_sym})"))
    }

    /// User-area cell read.
    fn user_u64(&self, off: u64) -> u64 {
        unsafe { ((self.user_base + off) as *const u64).read_unaligned() }
    }

    fn write_user_u64(&self, off: u64, v: u64) {
        unsafe { ((self.user_base + off) as *mut u64).write_unaligned(v); }
    }

    /// HERE — next free byte in the dictionary heap.
    pub fn here(&self)   -> u64 { self.user_u64(USER_HERE_VAR) }
    /// STATE — 0 interpret, 1 compile.
    pub fn state(&self)  -> u64 { self.user_u64(USER_STATE_VAR) }
    /// LATEST — head of dictionary chain.
    pub fn latest(&self) -> u64 { self.user_u64(USER_LATEST_VAR) }

    /// Returns the current data stack, top first. `stack()[0]` is TOS.
    pub fn stack(&self) -> Vec<i64> {
        let depth = ((self.dsp_top - self.current_dsp) / 8) as usize;
        (0..depth).map(|i| {
            let addr = self.current_dsp + (i as u64) * 8;
            unsafe { (addr as *const i64).read_unaligned() }
        }).collect()
    }

    pub fn depth(&self) -> usize {
        ((self.dsp_top - self.current_dsp) / 8) as usize
    }

    /// Push a cell onto the data stack.
    pub fn push(&mut self, v: i64) {
        self.current_dsp -= 8;
        unsafe { (self.current_dsp as *mut i64).write_unaligned(v); }
    }

    /// Preload `n` zero "cushion" cells onto the data stack.
    /// Interactive sessions (wf64-ui) call this once at boot so a
    /// few accidental over-drops at the REPL don't immediately
    /// crash the worker thread.  Tests skip the cushion so they
    /// see a truly empty stack.
    ///
    /// The cushion cells are real stack cells in every sense —
    /// `.s`, `depth`, and the stack viewer pane all show them.
    /// Users can drop them at any time; they regenerate only on
    /// a fresh boot or Forth → Restart.
    pub fn push_stack_cushion(&mut self, n: usize) {
        for _ in 0..n {
            self.push(0);
        }
    }

    /// Pop a cell. Panics on underflow — tests should know what's there.
    pub fn pop(&mut self) -> i64 {
        assert!(self.current_dsp < self.dsp_top, "stack underflow in pop()");
        let v = unsafe { (self.current_dsp as *const i64).read_unaligned() };
        self.current_dsp += 8;
        v
    }

    /// Invoke any JITed entry point. `target_xt` may be a primitive's
    /// xt or any other code address (e.g., a colon-defined word's body).
    /// Captures the resulting data-stack state.
    ///
    /// Wire-format with the kernel: pure in-memory stack. `current_dsp`
    /// is the address of the top cell (or `dsp_top` when empty);
    /// `forth_main` translates to/from its internal register-cached
    /// TOS via prologue/epilogue.
    pub fn call_xt(&mut self, target_xt: u64) -> Result<()> {
        self.write_user_u64(USER_BYE_REQ, 0);
        self.write_user_u64(USER_THROW_CODE, 0);
        let fm = self.forth_main;
        unsafe { fm(target_xt, self.current_dsp, self.rsp_top, self.user_base); }
        self.current_dsp = self.user_u64(USER_DSP_SAVE);
        let throw_code = self.user_u64(USER_THROW_CODE) as i64;
        if throw_code != 0 {
            self.write_user_u64(USER_THROW_CODE, 0);
            anyhow::bail!("Forth THROW {throw_code}");
        }
        if self.debug_tracking_enabled {
            self.refresh_runtime_debug_info()?;
        }
        Ok(())
    }

    /// Invoke a primitive by its asm symbol.
    pub fn call(&mut self, asm_sym: &str) -> Result<()> {
        let xt = self.xt_of(asm_sym)?;
        self.call_xt(xt)
    }

    /// Feed text through the REPL (quit). Returns captured stdout.
    ///
    /// On a Forth-level error (uncaught `THROW`), returns `Err` with:
    ///   - a human-readable description of the throw code, and
    ///   - any output the interpreter captured *before* the throw
    ///     (e.g. `"FOO ? "` when an unknown word is echoed at interpret
    ///     time, or partial print output before a `THROW` in a word body).
    /// This output is attached to the error message so the UI can display
    /// it without losing context.
    pub fn eval(&mut self, input: &str) -> Result<String> {
        // Always start in interpret mode.  A previous eval that ended
        // mid-definition (`:` without `;`) would otherwise leave STATE=1,
        // causing every subsequent call to compile rather than interpret.
        self.write_user_u64(USER_STATE_VAR, 0);

        let io = runtime::Io::Buffered {
            input: input.as_bytes().to_vec(),
            in_cursor: 0,
            pending_key: None,
            output: Vec::new(),
        };
        let xt_quit = self.xt_of("quit")?;
        let mut call_err: Result<()> = Ok(());
        let (_, io_after) = runtime::with_io(io, || {
            call_err = self.call_xt(xt_quit);
        });
        // Capture output *before* checking for errors — the interpreter
        // may have written useful context (a "? " marker, partial print
        // output, etc.) before the THROW fired.
        let output = match io_after {
            runtime::Io::Buffered { output, .. } => {
                String::from_utf8_lossy(&output).into_owned()
            }
            runtime::Io::Live { .. } => unreachable!("eval installed Buffered"),
        };

        // If STATE is still 1 after the eval, a `:` was opened but `;`
        // was never reached.  Roll back the incomplete definition so the
        // dictionary doesn't contain a word with no EXIT at its tail.
        if self.user_u64(USER_STATE_VAR) != 0 {
            let _ = self.call("forget_last_word");
            self.write_user_u64(USER_STATE_VAR, 0);
            let base = match call_err {
                Ok(()) => anyhow::anyhow!("incomplete definition — ':' without ';'"),
                Err(e) => anyhow::anyhow!(
                    "incomplete definition — ':' without ';' (also: {e})"
                ),
            };
            return Err(annotate_forth_error(base, &output));
        }

        match call_err {
            Ok(()) => Ok(output),
            Err(e) => Err(annotate_forth_error(e, &output)),
        }
    }

    /// Load a Forth source file through the normal REPL pipeline.
    ///
    /// This is intentionally simple for the current bootstrap stage:
    /// the host reads the file and feeds it to `quit`, so source files
    /// use the same parser/compiler path as interactive input.
    pub fn load_source_file(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let source = fs::read_to_string(path)
            .with_context(|| format!("read Forth source {}", path.display()))?;
        self.eval(&source)
            .with_context(|| format!("load Forth source {}", path.display()))?;
        Ok(())
    }

    /// Runtime-created Forth words currently visible in the dictionary.
    /// Each tuple is `(name, start_addr, end_addr)`.
    pub fn debug_words(&self) -> Vec<(String, u64, u64)> {
        self.runtime_words
            .iter()
            .map(|word| (word.name.clone(), word.start, word.end))
            .collect()
    }

    /// Like [`Self::debug_words`] but also returns each word's `dh_tfa` type-flag
    /// byte: `0x82` = colon definition (real code the optimizer transforms),
    /// `0x91` = CREATE-flavoured (constant/variable/buffer — whose "body" is
    /// data, not code). Lets the optimizer measurement harness skip data words.
    pub fn debug_words_typed(&self) -> Vec<(String, u64, u64, u8)> {
        self.runtime_words
            .iter()
            .map(|word| {
                let tfa = unsafe { ((word.header + DH_TFA) as *const u8).read() };
                (word.name.clone(), word.start, word.end, tfa)
            })
            .collect()
    }

    /// Resolve an address into the currently-visible runtime-created
    /// Forth word that contains it.
    pub fn resolve_word_addr(&self, addr: u64) -> Option<String> {
        self.runtime_words
            .iter()
            .find(|word| word.start <= addr && addr < word.end)
            .map(|word| {
                let off = addr - word.start;
                if off == 0 {
                    word.name.clone()
                } else {
                    format!("{}+0x{off:x}", word.name)
                }
            })
    }

    /// Roll the session back to its post-bootstrap state — empty data
    /// stack, empty return stack, dictionary trimmed to just the
    /// primitives, STATE = interpret, BYE_REQ cleared. The JIT module
    /// and the primitive headers themselves stay untouched, which is
    /// what makes sharing one session across many tests cheap: the
    /// boot cost is amortised, and per-test reset is a handful of
    /// pointer writes.
    ///
    /// Test-defined colon definitions compiled into `boot_here..HERE`
    /// are abandoned — the bytes remain in memory but become
    /// unreachable once HERE rolls back and LATEST is unhitched from
    /// them. Next test compiles right over the top.
    pub fn reset(&mut self) {
        self.clear_runtime_unwind_table();
        self.current_dsp = self.dsp_top;
        self.write_user_u64(USER_BASE_VAR,     10);
        self.write_user_u64(USER_HERE_VAR,     self.boot_here);
        self.write_user_u64(USER_VAR_HERE,     self.boot_var_here);
        self.write_user_u64(USER_LATEST_VAR,   self.boot_latest);
        self.write_user_u64(USER_STATE_VAR,    0);
        self.write_user_u64(USER_PARSE_BARRIER, 0);
        self.write_user_u64(USER_BYE_REQ,      0);
        self.write_user_u64(USER_RSP_CURRENT,  self.rsp_top);
        self.write_user_u64(USER_DSP_SAVE,     self.dsp_top);
        self.write_user_u64(USER_LATESTXT_VAR, self.boot_latestxt);
        self.write_user_u64(USER_HANDLER_VAR,  0);
        self.write_user_u64(USER_THROW_CODE,   0);
        self.write_user_u64(USER_TRACE,        0);
        self.write_user_u64(USER_LOCALS_COUNT, 0);
        // OOP early-binding hint: clear so a stale HERE can't false-match
        // a fresh receiver after HERE rewinds (see lib/oop.f).
        self.write_user_u64(USER_OOP_RECV_CLASS, 0);
        self.write_user_u64(USER_OOP_RECV_HERE,  0);
        self.write_user_u64(USER_FP0,          self.user_base + USER_FP_STACK + 0x100);
        self.write_user_u64(USER_FSP,          self.user_base + USER_FP_STACK + 0x100);
        // HEAPPTR + LITERAL region reset: clear both slot regions,
        // rewind both bump pointers to their respective bases, drop
        // the GC heap so the next test gets a fresh one.
        let heapptr_base = self.user_base + USER_HEAPPTR_BASE;
        let literal_base = self.user_base + USER_LITERAL_BASE;
        unsafe {
            std::ptr::write_bytes(
                heapptr_base as *mut u8,
                0,
                HEAPPTR_REGION_SIZE as usize,
            );
            std::ptr::write_bytes(
                literal_base as *mut u8,
                0,
                LITERAL_REGION_SIZE as usize,
            );
        }
        self.write_user_u64(USER_HEAPPTR_NEXT, heapptr_base);
        self.write_user_u64(USER_LITERAL_NEXT, literal_base);
        gc::reset_wf_heap();
        self.write_user_u64(USER_INDEX_HERE,   self.boot_index_here);
        self.write_user_u64(USER_INDEX_LATEST, self.boot_index_latest);
        let forth_wid = self.user_u64(USER_FORTH_WID);
        // Restore the FORTH-WORDLIST bucket chains to their
        // post-bootstrap state. Any overlays allocated since boot used
        // addresses that will be reused by the next test; without this
        // restore the stale bucket heads produce circular chains and
        // hang find-name.
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.boot_wl_buckets.as_ptr(),
                forth_wid as *mut u64,
                512,
            );
        }
        // Same for TOOLS and PRIVATE -- without this, overlay entries
        // added to those wordlists during a test leave stale bucket
        // heads that create circular chains on the next test.
        let tools_wid_r = self.user_u64(USER_TOOLS_WID);
        let private_wid_r = self.user_u64(USER_PRIVATE_WID);
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.boot_tools_buckets.as_ptr(),
                tools_wid_r as *mut u64,
                512,
            );
            std::ptr::copy_nonoverlapping(
                self.boot_private_buckets.as_ptr(),
                private_wid_r as *mut u64,
                512,
            );
        }
        // Same for the OOP ivar wordlist, if oop.f published one. Restoring
        // its (boot-empty) buckets clears the scoped ivar entries a test
        // added, so the next test doesn't chase rewound overlay nodes.
        let ivars_wid_r = self.user_u64(USER_OOP_IVARS_WID);
        if ivars_wid_r != 0 && self.boot_ivars_buckets.len() == 512 {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.boot_ivars_buckets.as_ptr(),
                    ivars_wid_r as *mut u64,
                    512,
                );
            }
        }
        self.write_user_u64(USER_CURRENT,      forth_wid);
        // Default search order: PRIVATE TOOLS FORTH (innermost first).
        // The three-wordlist organisation lets callers see the
        // categorisation via get-order while everything stays findable
        // by default. Narrowing (e.g. `only forth`) is opt-in.
        let tools_wid   = self.user_u64(USER_TOOLS_WID);
        let private_wid = self.user_u64(USER_PRIVATE_WID);
        self.write_user_u64(USER_ORDER_COUNT,  3);
        let context = self.user_base + USER_CONTEXT;
        unsafe {
            (context as *mut u64).offset(0).write_unaligned(private_wid);
            (context as *mut u64).offset(1).write_unaligned(tools_wid);
            (context as *mut u64).offset(2).write_unaligned(forth_wid);
        }
        self.runtime_words.clear();
        self.debug_synced_here = self.boot_here;
        self.debug_synced_latest = self.boot_latest;
    }

    /// Run the REPL with live stdin/stdout. Used by `src/main.rs`.
    /// Returns when the user types `bye` or stdin hits EOF.
    pub fn run_interactive(&mut self) -> Result<()> {
        let xt_quit = self.xt_of("quit")?;
        let mut err = Ok(());
        let (_, _) = runtime::with_io(runtime::Io::Live { pending_key: None }, || {
            err = self.call_xt(xt_quit);
        });
        err
    }

    fn refresh_runtime_debug_info(&mut self) -> Result<()> {
        let here = self.here();
        let latest = self.latest();
        if here == self.debug_synced_here && latest == self.debug_synced_latest {
            return Ok(());
        }

        let words = self.scan_runtime_words()?;
        self.install_runtime_unwind_table(&words)?;
        wfasm::seh::register_many(
            words
                .iter()
                .map(|word| (format!("forth:{}", word.name), word.start, "forth_word")),
        );
        self.runtime_words = words;
        self.debug_synced_here = here;
        self.debug_synced_latest = latest;
        Ok(())
    }

    fn scan_runtime_words(&self) -> Result<Vec<RuntimeWord>> {
        let mut headers = Vec::new();
        let mut header = self.latest();
        while header != 0 && header != self.boot_latest {
            headers.push(header);
            header = self.read_u64(header + DH_LINK);
        }

        let mut words = Vec::with_capacity(headers.len());
        let mut end = self.here();
        for header in headers {
            let start = self.read_u64(header + DH_XTPTR);
            if self.dict_base <= start && start < end {
                let name = self.read_name(header)?;
                words.push(RuntimeWord { name, header, start, end });
            }
            end = header;
        }
        words.reverse();
        Ok(words)
    }

    fn install_runtime_unwind_table(&mut self, words: &[RuntimeWord]) -> Result<()> {
        self.clear_runtime_unwind_table();
        if words.is_empty() {
            return Ok(());
        }

        let unwind_size = std::mem::size_of::<UnwindInfo>() as u64;
        let required = align_up(unwind_size * words.len() as u64, 4);
        if required > DEBUG_META_SIZE {
            anyhow::bail!(
                "runtime debug metadata exhausted: need {required} bytes for {} words, have {DEBUG_META_SIZE}",
                words.len()
            );
        }

        let mut unwind_cursor = self.debug_meta_base;
        let mut entries = Vec::with_capacity(words.len());
        for word in words {
            unwind_cursor = align_up(unwind_cursor, 4);
            let unwind_rva = (unwind_cursor - self.region_base) as u32;
            unsafe {
                (unwind_cursor as *mut UnwindInfo).write_unaligned(UnwindInfo::leaf());
            }
            unwind_cursor += unwind_size;

            entries.push(RuntimeFunction {
                BeginAddress: (word.start - self.region_base) as u32,
                EndAddress: (word.end - self.region_base) as u32,
                UnwindData: unwind_rva,
            });
        }

        let entries = entries.into_boxed_slice();
        let ok = unsafe {
            RtlAddFunctionTable(entries.as_ptr(), entries.len() as u32, self.region_base)
        };
        if ok == 0 {
            anyhow::bail!("RtlAddFunctionTable failed for {} runtime words", entries.len());
        }
        self.debug_function_table = Some(RegisteredFunctionTable { entries });
        Ok(())
    }

    fn clear_runtime_unwind_table(&mut self) {
        if let Some(table) = self.debug_function_table.take() {
            unsafe {
                let _ = RtlDeleteFunctionTable(table.entries.as_ptr());
            }
        }
    }

    fn read_u64(&self, addr: u64) -> u64 {
        unsafe { (addr as *const u64).read_unaligned() }
    }

    fn read_name(&self, header: u64) -> Result<String> {
        let len = unsafe { ((header + DH_NT) as *const u8).read() } as usize;
        let bytes = unsafe { std::slice::from_raw_parts((header + DH_NAME) as *const u8, len) };
        String::from_utf8(bytes.to_vec()).context("runtime dictionary name was not valid UTF-8")
    }
}

impl Drop for Wf64Session {
    fn drop(&mut self) {
        self.clear_runtime_unwind_table();
        if !self.locals_base.is_null() {
            unsafe { VirtualFree(self.locals_base, 0, MEM_RELEASE); }
            self.locals_base = ptr::null_mut();
        }
        if self.var_base != 0 {
            unsafe { VirtualFree(self.var_base as *mut c_void, 0, MEM_RELEASE); }
            self.var_base = 0;
        }
    }
}

unsafe fn write_u64(base: *mut c_void, off: u64, v: u64) {
    let ptr = (base as *mut u8).add(off as usize) as *mut u64;
    ptr.write_unaligned(v);
}

// `default_kernel_path` lives at the top of this file alongside
// KERNEL_ENTRY — the historical stub here was a single
// `PathBuf::from(KERNEL_ENTRY)` line; the real impl with exe-dir
// fallback search supersedes it for release-packaged binaries.
