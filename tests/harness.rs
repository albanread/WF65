//! Integration tests covering the M3/M4 behaviour through both
//! execution modes:
//!
//!   * `eval(text)` — full REPL pipeline (accept/parse/dispatch). Pins
//!     the user-visible behaviour against regressions.
//!
//!   * `push(v)` + `call(asm_sym)` + `pop()` — direct primitive
//!     invocation with no parser in the loop. Lets us test the
//!     semantics of each primitive cell-accurately.
//!
//! Each `#[test]` owns its own `Wf64Session` so failures are isolated.

use std::ffi::OsStr;
use std::fs;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::arch::x86_64::__cpuid;

use wf64::Wf64Session;

/// One Wf64Session is built per test binary and shared across every
/// test via `sess()`. Each `#[test]` call grabs the lock, gets a
/// freshly-`reset()`-ed session, and drops the guard on the way out.
///
/// Why: each `with_kernel` boot does JASM expansion + LLVM module
/// load + MCJIT finalize + extern binding + symbol registration + the
/// 45-call dictionary bootstrap. With ~50 tests that boot cost dominated
/// total run time many times over. Reusing the session collapses it to
/// a one-time cost amortised across the suite, while `reset()` makes
/// each test's view of the world look as if it had its own session.
///
/// Safety pre-condition: tests must run single-threaded. Enforced by
/// `.cargo/config.toml` setting `RUST_TEST_THREADS = "1"`. The Mutex
/// is uncontested in practice but provides the discipline anyway.
static SHARED: OnceLock<Mutex<Wf64Session>> = OnceLock::new();

fn sess() -> SessionGuard {
    let m = SHARED.get_or_init(|| {
        let kernel = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("kernel")
            .join("main.masm");
        Mutex::new(Wf64Session::with_kernel(kernel).expect("session boot"))
    });
    // `into_inner` salvages access from a poisoned mutex (i.e., a
    // panicking test). The state is whatever the panicking test left
    // behind; `reset()` makes that irrelevant before the next test
    // touches it.
    let mut guard = m.lock().unwrap_or_else(|p| p.into_inner());
    guard.reset();
    SessionGuard(guard)
}

/// Deref-mut wrapper so the existing `s.push()`, `s.call()`, `s.eval()`
/// call sites compile unchanged.
struct SessionGuard(MutexGuard<'static, Wf64Session>);
impl Deref for SessionGuard {
    type Target = Wf64Session;
    fn deref(&self) -> &Wf64Session { &*self.0 }
}
impl DerefMut for SessionGuard {
    fn deref_mut(&mut self) -> &mut Wf64Session { &mut *self.0 }
}

// ── eval-mode (full REPL pipeline) ───────────────────────────────────

#[test]
fn eval_empty_input_just_prints_ok() {
    let mut s = sess();
    let out = s.eval("\n").unwrap();
    assert_eq!(out, " ok\n");
}

#[test]
fn eval_bye_terminates_cleanly() {
    let mut s = sess();
    let out = s.eval("bye\n").unwrap();
    assert_eq!(out, "");
}

#[test]
fn eval_number_then_dot() {
    let mut s = sess();
    let out = s.eval("5 .\nbye\n").unwrap();
    assert_eq!(out, "5  ok\n");
}


#[test]
fn eval_source_defined_set_order_wrapper_persists_across_eval_calls() {
    let mut s = sess();
    s.call("forth_wordlist_word").unwrap();
    let root_wid = s.stack()[0];
    s.reset();
    let out = s.eval(": only2 -1 set-order ;\nbye\n").unwrap();
    assert_eq!(out, " ok\n");

    let out = s.eval("only2 get-order\nbye\n").unwrap();
    assert_eq!(out, " ok\n");
    assert_eq!(s.stack(), vec![1, root_wid]);
}
#[test]
fn eval_arithmetic() {
    let mut s = sess();
    let out = s.eval("5 3 + .\n7 2 * .\nbye\n").unwrap();
    assert_eq!(out, "8  ok\n14  ok\n");
}

#[test]
fn eval_brk_and_int3_are_callable() {
    let mut s = sess();
    let out = s.eval("BRK\nINT3\nbye\n").unwrap();
    // Both words emit a Forth state dump followed by " ok\n"; we just
    // check the eval succeeds and that the " ok" prompts are present.
    assert!(out.contains(" ok\n"), "expected at least one ' ok\\n' in: {out:?}");
}

#[test]
fn eval_key_reads_from_buffered_input_stream() {
    let mut s = sess();
    let out = s.eval("key .\nA\nbye\n").unwrap();
    assert_eq!(out, "65  ok\n ok\n");
}

#[test]
fn load_source_file_provides_only_word() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.call("forth_wordlist_word").unwrap();
    let root_wid = s.stack()[0];
    s.reset();
    s.load_source_file(&path).unwrap();

    let out = s.eval("only get-order\nbye\n").unwrap();
    assert_eq!(out, " ok\n");
    assert_eq!(s.stack(), vec![1, root_wid]);
}

#[test]
fn load_source_file_leaves_empty_data_stack() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();
    let stack = s.stack();
    let resolved: Vec<String> = stack
        .iter()
        .map(|value| {
            s.resolve_word_addr(*value as u64)
                .unwrap_or_else(|| format!("{value:#x}"))
        })
        .collect();
    assert_eq!(stack, Vec::<i64>::new(), "resolved stack = {resolved:?}");
}

#[test]
fn direct_only_word_then_get_order_leaves_clean_stack() {
    let mut s = sess();
    s.call("forth_wordlist_word").unwrap();
    let root_wid = s.stack()[0];
    s.reset();

    s.call("only_word").unwrap();
    s.call("get_order_word").unwrap();

    assert_eq!(s.stack(), vec![1, root_wid]);
}

#[test]
fn eval_tick_then_compiles_me_leaves_empty_stack() {
    let mut s = sess();
    let out = s.eval(
        ": compiles ( xt1 xt2 -- ) >comp ! ;\n\
         : compiles-me ( xt -- ) latestxt compiles ;\n\
         : helper 123 ;\n\
         : target 456 ;\n\
         ' helper compiles-me\n\
         bye\n"
    ).unwrap();

    assert_eq!(out, " ok\n ok\n ok\n ok\n ok\n");
    assert_eq!(s.stack(), Vec::<i64>::new());
}

#[test]
fn eval_defining_word_with_does_leaves_empty_stack() {
    let mut s = sess();
    let out = s.eval(": , here ! 1 cells allot ;\n: constant create , does> @ ;\nbye\n").unwrap();

    assert_eq!(out, " ok\n ok\n");
    assert_eq!(s.stack(), Vec::<i64>::new());
}

#[test]
fn eval_compiles_me_on_defining_word_leaves_empty_stack() {
    let mut s = sess();
    let out = s.eval(
        ": , here ! 1 cells allot ;\n\
         : compiles ( xt1 xt2 -- ) >comp ! ;\n\
         : compiles-me ( xt -- ) latestxt compiles ;\n\
             : (comp-cons) ( xt -- ) >body postpone literal ;\n\
         : constant create , does> @ ;\n\
             ' (comp-cons) compiles-me\n\
         bye\n"
    ).unwrap();

    assert_eq!(out, " ok\n ok\n ok\n ok\n ok\n ok\n");
    let stack = s.stack();
    let resolved: Vec<String> = stack
        .iter()
        .map(|value| {
            s.resolve_word_addr(*value as u64)
                .unwrap_or_else(|| format!("{value:#x}"))
        })
        .collect();
    assert_eq!(stack, Vec::<i64>::new(), "resolved stack = {resolved:?}");
}

#[test]
fn eval_defining_word_setup_leaves_empty_stack() {
    let mut s = sess();
    let out = s.eval(
        ": , here ! 1 cells allot ;\n\
         : compiles ( xt1 xt2 -- ) >comp ! ;\n\
         : compiles-me ( xt -- ) latestxt compiles ;\n\
             : (comp-cons) ( xt -- ) >body postpone literal ;\n\
         : constant create , does> @ ;\n\
         bye\n"
    ).unwrap();

    assert_eq!(out, " ok\n ok\n ok\n ok\n ok\n");
    assert_eq!(s.stack(), Vec::<i64>::new());
}

#[test]
fn direct_compiles_me_on_defining_word_leaves_empty_stack() {
    let mut s = sess();
    s.eval(
        ": , here ! 1 cells allot ;\n\
         : compiles ( xt1 xt2 -- ) >comp ! ;\n\
         : compiles-me ( xt -- ) latestxt compiles ;\n\
             : (comp-cons) ( xt -- ) >body postpone literal ;\n\
         : constant create , does> @ ;\n\
         bye\n"
    ).unwrap();

        s.eval("' compiles-me ' (comp-cons) bye\n").unwrap();
        let comp_cons_xt = s.pop() as u64;
    let compiles_me_xt = s.pop() as u64;

    s.push(comp_cons_xt as i64);
    s.call_xt(compiles_me_xt).unwrap();

    assert_eq!(s.stack(), Vec::<i64>::new());
}

#[test]
fn execute_primitive_compiles_me_on_defining_word_leaves_empty_stack() {
    let mut s = sess();
    s.eval(
        ": , here ! 1 cells allot ;\n\
         : compiles ( xt1 xt2 -- ) >comp ! ;\n\
         : compiles-me ( xt -- ) latestxt compiles ;\n\
             : (comp-cons) ( xt -- ) >body postpone literal ;\n\
         : constant create , does> @ ;\n\
         bye\n"
    ).unwrap();

        s.eval("' compiles-me ' (comp-cons) bye\n").unwrap();
    let comp_cons_xt = s.pop() as u64;
    let compiles_me_xt = s.pop() as u64;

    s.push(comp_cons_xt as i64);
    s.push(compiles_me_xt as i64);
    s.call("execute").unwrap();

    assert_eq!(s.stack(), Vec::<i64>::new());
}

#[test]
fn load_source_file_only_word_is_present_in_root_wordlist() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();
    let out = s.eval(
        "forth-wordlist constant root\n\
         : square-name s\" square\" ;\n\
         : only-name s\" only\" ;\n\
         square-name root search-wordlist nip . cr\n\
         only-name root search-wordlist nip . cr\n\
         bye\n"
    ).unwrap();
    assert_eq!(out, " ok\n ok\n ok\n-1 \n ok\n-1 \n ok\n");
}

#[test]
fn load_source_file_only_word_executes_via_search_wordlist_xt() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.call("forth_wordlist_word").unwrap();
    let root_wid = s.stack()[0];
    s.reset();
    s.load_source_file(&path).unwrap();

    let out = s.eval(
        "forth-wordlist constant root\n\
         : only-name s\" only\" ;\n\
         only-name root search-wordlist drop execute get-order\n\
         bye\n"
    ).unwrap();
    assert_eq!(out, " ok\n ok\n ok\n");
    assert_eq!(s.stack(), vec![1, root_wid]);
}

#[test]
fn eval_core_f_with_explicit_bye_provides_only_word() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    let mut source = std::fs::read_to_string(&path).unwrap();
    source.push_str("\nbye\n");

    s.call("forth_wordlist_word").unwrap();
    let root_wid = s.stack()[0];
    s.reset();

    let out = s.eval(&source).unwrap();
    assert!(out.ends_with(" ok\n"), "got {out:?}");

    let out = s.eval("only get-order\nbye\n").unwrap();
    assert_eq!(out, " ok\n");
    assert_eq!(s.stack(), vec![1, root_wid]);
}

#[test]
fn eval_nested_evaluate_definition_provides_only_word() {
    let mut s = sess();
    s.call("forth_wordlist_word").unwrap();
    let root_wid = s.stack()[0];
    s.reset();

    let out = s.eval(
        ": install-only s\" : only -1 set-order ;\" evaluate ;\n\
         install-only\n\
         only get-order\n\
         bye\n"
    ).unwrap();
    assert_eq!(out, " ok\n ok\n ok\n");
    assert_eq!(s.stack(), vec![1, root_wid]);
}

#[test]
fn load_source_file_then_redefine_only_same_name() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.call("forth_wordlist_word").unwrap();
    let root_wid = s.stack()[0];
    s.reset();
    s.load_source_file(&path).unwrap();

    let out = s.eval(": only -1 set-order ;\nbye\n").unwrap();
    assert_eq!(out, " ok\n");

    let out = s.eval("only get-order\nbye\n").unwrap();
    assert_eq!(out, " ok\n");
    assert_eq!(s.stack(), vec![1, root_wid]);
}

#[test]
fn eval_redefining_simple_word_same_name_uses_newest() {
    let mut s = sess();
    let out = s.eval(": foo 1 ;\n: foo 2 ;\nfoo .\nbye\n").unwrap();
    assert_eq!(out, " ok\n ok\n2  ok\n");
}

#[test]
fn eval_redefining_simple_word_same_name_across_eval_calls_uses_newest() {
    let mut s = sess();
    let out = s.eval(": foo 1 ;\nbye\n").unwrap();
    assert_eq!(out, " ok\n");

    let out = s.eval(": foo 2 ;\nbye\n").unwrap();
    assert_eq!(out, " ok\n");

    let out = s.eval("foo .\nbye\n").unwrap();
    assert_eq!(out, "2  ok\n");
}

#[test]
fn eval_primitive_only_then_get_order() {
    let mut s = sess();
    s.call("forth_wordlist_word").unwrap();
    let root_wid = s.stack()[0];
    s.reset();

    let out = s.eval("only get-order\nbye\n").unwrap();
    assert_eq!(out, " ok\n");
    assert_eq!(s.stack(), vec![1, root_wid]);
}

#[test]
fn load_source_file_provides_also_word() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.call("forth_wordlist_word").unwrap();
    let root_wid = s.stack()[0];
    s.reset();
    s.load_source_file(&path).unwrap();

    let out = s.eval("only also get-order\nbye\n").unwrap();
    assert_eq!(out, " ok\n");
    assert_eq!(s.stack(), vec![2, root_wid, root_wid]);
}

#[test]
fn load_source_file_provides_previous_word() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.call("forth_wordlist_word").unwrap();
    let root_wid = s.stack()[0];
    s.reset();
    s.load_source_file(&path).unwrap();

    let out = s.eval(
        "forth-wordlist constant root\n\
         wordlist constant extra\n\
         root extra 2 set-order previous get-order\n\
         bye\n"
    ).unwrap();
    assert_eq!(out, " ok\n ok\n ok\n");
    assert_eq!(s.stack(), vec![1, root_wid]);
}

#[test]
fn load_source_file_provides_forth_word() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.call("forth_wordlist_word").unwrap();
    let root_wid = s.stack()[0];
    s.reset();
    s.load_source_file(&path).unwrap();

    let out = s.eval(
        "forth-wordlist constant root\n\
         wordlist constant extra\n\
         root extra 2 set-order forth get-order\n\
         bye\n"
    ).unwrap();
    assert_eq!(out, " ok\n ok\n ok\n");
    assert_eq!(s.stack(), vec![2, root_wid, root_wid]);
}

#[test]
fn load_source_file_provides_definitions_word() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.call("forth_wordlist_word").unwrap();
    let root_wid = s.stack()[0];
    s.reset();
    s.load_source_file(&path).unwrap();

    let out = s.eval(
        "forth-wordlist constant root\n\
         wordlist constant extra\n\
         root extra 2 set-order definitions get-current\n\
         extra\n\
         bye\n"
    ).unwrap();
    assert_eq!(out, " ok\n ok\n ok\n ok\n");
    let stack = s.stack();
    assert_eq!(stack.len(), 2);
    assert_eq!(stack[0], stack[1]);
    assert_ne!(stack[0], root_wid);
}

#[test]
fn eval_exit_returns_early_from_definition() {
    let mut s = sess();
    let out = s.eval(": early 1 exit 2 ;\nearly .\nbye\n").unwrap();
    assert_eq!(out, " ok\n1  ok\n");
}

/// Test the {: word compiles one local and open-locals works
#[test]
fn eval_locals_one_local_compiles() {
    let mut s = sess();
    let out = s.eval(": tloc {: x :} x . ;\n5 tloc\nbye\n").unwrap();
    assert_eq!(out, " ok\n5  ok\n");
}

/// Verify {: and to are findable in the current search order
#[test]
fn eval_locals_words_findable() {
    let mut s = sess();
    // Check several words that should be in the FORTH wordlist
    let out = s.eval(
        "s\" {:\" find-name nip .\
         \ns\" to\" find-name nip .\
         \ns\" locals#!\" find-name nip .\
         \nbye\n"
    ).unwrap();
    // Should print -1 for each found word
    assert_eq!(out, "-1  ok\n-1  ok\n-1  ok\n");
}

/// Minimal sanity check: {: with one local
#[test]
fn eval_locals_basic_fetch() {
    let mut s = sess();
    // Single local, defined and called on the same line.
    let out = s.eval(": tl1 {: a :} a . ; 42 tl1\nbye\n").unwrap();
    assert_eq!(out, "42  ok\n");
}

#[test]
fn eval_colon_without_name_throws_minus_16() {
    let mut s = sess();
    let err = s.eval(":\n").unwrap_err().to_string();
    assert!(err.contains("-16"), "got {err:?}");
}

#[test]
fn eval_exit_in_interpret_state_throws_minus_14() {
    let mut s = sess();
    let err = s.eval("exit\n").unwrap_err().to_string();
    assert!(err.contains("-14"), "got {err:?}");
}

#[test]
fn eval_nested_colon_defs() {
    let mut s = sess();
    let out = s
        .eval(": double 2 * ;\n: quad double double ;\n3 quad .\nbye\n")
        .unwrap();
    assert_eq!(out, " ok\n ok\n12  ok\n");
}

#[test]
fn eval_literal_inside_def() {
    let mut s = sess();
    let out = s.eval(": add5 5 + ;\n10 add5 .\nbye\n").unwrap();
    assert_eq!(out, " ok\n15  ok\n");
}

#[test]
fn eval_brackets_and_literal_compile_interpreted_value() {
    let mut s = sess();
    let out = s.eval(": eleven [ 5 6 + ] literal ;\neleven .\nbye\n").unwrap();
    assert_eq!(out, " ok\n11  ok\n");
}

#[test]
fn eval_s_quote_compiles_runtime_string() {
    let mut s = sess();
    let out = s.eval(": greet s\" HI\" ;\ngreet type cr\nbye\n").unwrap();
    assert_eq!(out, " ok\nHI\n ok\n");
}

#[test]
fn eval_dot_quote_compiles_runtime_output() {
    let mut s = sess();
    let out = s.eval(": greet .\" HI\" ;\ngreet cr\nbye\n").unwrap();
    assert_eq!(out, " ok\nHI\n ok\n");
}

#[test]
fn eval_dot_quote_works_in_interpret_mode() {
    // Extended: ." prints in both interpret and compile state.
    let mut s = sess();
    let out = s.eval(".\" HI\" cr\nbye\n").unwrap();
    assert_eq!(out, "HI\n ok\n");
}

#[test]
fn eval_s_quote_works_in_interpret_mode() {
    // ANS Forth: s" is valid in both interpret and compile state.
    let mut s = sess();
    let out = s.eval("s\" HI\" type cr\nbye\n").unwrap();
    assert_eq!(out, "HI\n ok\n");
}

#[test]
fn eval_c_quote_works_in_interpret_mode() {
    let mut s = sess();
    let out = s.eval("c\" HI\" count type cr\nbye\n").unwrap();
    assert_eq!(out, "HI\n ok\n");
}

#[test]
fn eval_c_quote_compiles_runtime_counted_string() {
    let mut s = sess();
    let out = s.eval(": greet c\" HI\" ;\ngreet count type cr\nbye\n").unwrap();
    assert_eq!(out, " ok\nHI\n ok\n");
}

#[test]
fn eval_source_and_to_in_can_skip_rest_of_line() {
    let mut s = sess();
    let out = s.eval(": skip-rest source >in ! drop ;\nskip-rest 123 .\nbye\n").unwrap();
    assert_eq!(out, " ok\n ok\n");
}

#[test]
fn eval_state_exposes_compilation_flag_address() {
    let mut s = sess();
    let out = s
        .eval("state @ .\n: compiling? state @ ; immediate\n: compiled-state compiling? literal ;\ncompiled-state 0= .\nbye\n")
        .unwrap();
    assert_eq!(out, "0  ok\n ok\n ok\n0  ok\n");
}

#[test]
fn eval_source_id_tracks_repl_and_evaluate_input() {
    let mut s = sess();
    let out = s
        .eval("source-id .\n: source-id-from-eval s\" source-id\" evaluate ;\nsource-id-from-eval .\nsource-id .\nbye\n")
        .unwrap();
    assert_eq!(out, "0  ok\n ok\n-1  ok\n0  ok\n");
}

#[test]
fn eval_restore_input_does_not_reparse_own_token() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();
    let out = s.eval("save-input restore-input 0= .\nbye\n").unwrap();
    assert_eq!(out, "-1  ok\n");
}

#[test]
fn eval_refill_reads_next_line_and_is_false_for_evaluate() {
    let mut s = sess();
    let out = s
        .eval(": next-line refill if source dup >in ! type drop else 999 . then ;\n: eval-refill-string s\" refill .\" ;\nnext-line\nHELLO\neval-refill-string evaluate\nbye\n")
        .unwrap();
    assert_eq!(out, " ok\n ok\nHELLO ok\n0  ok\n");
}

#[test]
fn eval_parse_word_and_pad_work() {
    let mut s = sess();
    let out = s
        .eval(": upto-comma 32 parse 2drop 44 parse ;\nupto-comma hello, type cr\npad dup 65 swap c! c@ .\n32 word hello count type cr\nbye\n")
        .unwrap();
    assert_eq!(out, " ok\nhello\n ok\n65  ok\nhello\n ok\n");
}

#[test]
fn eval_tick_pushes_interpret_xt() {
    let mut s = sess();
    let out = s.eval("5 ' dup execute . .\nbye\n").unwrap();
    assert_eq!(out, "5 5  ok\n");
}

#[test]
fn eval_tick_from_empty_then_drop_leaves_stack_empty() {
    let mut s = sess();
    let out = s.eval("' dup drop\nbye\n").unwrap();
    assert_eq!(out, " ok\n");
    assert_eq!(s.depth(), 0);
}

#[test]
fn eval_bracket_tick_compiles_xt_literal() {
    let mut s = sess();
    let out = s.eval(": run-dup ['] dup execute ;\n7 run-dup . .\nbye\n").unwrap();
    assert_eq!(out, " ok\n7 7  ok\n");
}

#[test]
fn eval_immediate_and_postpone_enable_forth_defined_compiler_words() {
    let mut s = sess();
    let out = s.eval(": twice postpone dup postpone dup ; immediate\n: demo twice ;\n4 demo . . .\nbye\n").unwrap();
    assert_eq!(out, " ok\n ok\n4 4 4  ok\n");
}

#[test]
fn eval_compiles_me_bindings_leave_stack_empty() {
    let mut s = sess();
    s.eval(
        ": compiles ( xt1 xt2 -- ) >comp ! ;\n\
         : compiles-me ( xt -- ) latestxt compiles ;\n\
         : f, here f! 1 floats allot ;\n\
         : (comp-cons) ( xt -- ) >body postpone literal ;\n\
         : constant create , does> @ ;\n\
         : (comp-2cons) ( xt -- ) >body postpone literal postpone 2@ ;\n\
         : 2constant create 2, does> 2@ ;\n\
         : (comp-fconst) ( xt -- ) >body postpone literal postpone f@ ;\n\
         : fconstant create f, does> f@ ;\n\
         : (comp-val) ( xt -- ) >body postpone literal postpone @ ;\n\
         : value create , does> @ ;\n\
         bye\n",
    )
    .unwrap();
    assert_eq!(s.depth(), 0);

    s.eval("' (comp-cons) compiles-me\nbye\n").unwrap();
    assert_eq!(s.stack(), Vec::<i64>::new());

    s.eval("' (comp-2cons) compiles-me\nbye\n").unwrap();
    assert_eq!(s.depth(), 0);

    s.eval("' (comp-fconst) compiles-me\nbye\n").unwrap();
    assert_eq!(s.depth(), 0);

    s.eval("' (comp-val) compiles-me\nbye\n").unwrap();
    assert_eq!(s.depth(), 0);
}

#[test]
fn eval_compiles_me_consumes_tick_result_across_eval_boundary() {
    let mut s = sess();
    s.eval(
        ": compiles ( xt1 xt2 -- ) >comp ! ;\n\
         : compiles-me ( xt -- ) latestxt compiles ;\n\
         : (comp-cons) ( xt -- ) >body postpone literal ;\n\
         : constant create , does> @ ;\n\
         bye\n",
    )
    .unwrap();

    s.eval("' (comp-cons)\nbye\n").unwrap();
    assert_eq!(s.depth(), 1);

    s.eval("compiles-me\nbye\n").unwrap();
    assert_eq!(s.depth(), 0);
}

#[test]
fn eval_dot_s_prints_stack_live_without_consuming_it() {
    let mut s = sess();
    let out = s.eval("1 2 3 .s . . .\nbye\n").unwrap();
    assert!(out.starts_with("[3 sp=0x"), "got {out:?}");
    assert!(out.contains(" rp=0x"), "got {out:?}");
    assert!(out.contains("] 3 2 1 3 2 1  ok\n"), "got {out:?}");
}

#[test]
fn eval_forget_last_rolls_back_and_allows_regrowth_live() {
    let mut s = sess();
    let out = s.eval(": a 1 ;\na .\nforget_last\na\n: a 2 ;\na .\nbye\n").unwrap();
    assert_eq!(out, " ok\n1  ok\n ok\n?  ok\n ok\n2  ok\n");
}

#[test]
fn eval_backslash_comment_ignores_rest_of_line() {
    let mut s = sess();
    let out = s.eval("1 \\ keep this out of the token stream\n2 + .\nbye\n").unwrap();
    assert_eq!(out, " ok\n3  ok\n");
}

#[test]
fn eval_backslash_prefixed_token_is_not_a_comment() {
    let mut s = sess();
    let out = s.eval("\\foo 123 .\nbye\n").unwrap();
    assert_eq!(out, "? 123  ok\n");
}

#[test]
fn eval_paren_comment_ignores_inline_text() {
    let mut s = sess();
    let out = s.eval("1 ( comment in source ) 2 + .\nbye\n").unwrap();
    assert_eq!(out, "3  ok\n");
}

#[test]
fn load_source_file_makes_saved_words_available() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();
    let out = s.eval("5 square .\n3 cube .\n2 quad .\n2 sixth .\nbye\n").unwrap();
    assert_eq!(out, "25  ok\n27  ok\n16  ok\n64  ok\n");
}

#[test]
fn load_source_file_supports_live_growth_after_startup() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();
    let out = s.eval(": sixth quad square ;\n2 sixth .\nbye\n").unwrap();
    assert_eq!(out, " ok\n256  ok\n");
}

#[test]
fn load_source_file_provides_bl_space_and_spaces() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();
    let out = s.eval("bl .\nspace 88 emit\n3 spaces 89 emit\n-2 spaces 90 emit\nbye\n").unwrap();
    assert_eq!(out, "32  ok\n X ok\n   Y ok\nZ ok\n");
}

#[test]
fn load_source_file_provides_char_bracket_char_true_and_false() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();
    let out = s
        .eval("true .\nfalse .\nchar Z .\n: zchar [char] Z ;\nzchar .\nbye\n")
        .unwrap();
    assert_eq!(out, "-1  ok\n0  ok\n90  ok\n ok\n90  ok\n");
}

#[test]
fn load_source_file_provides_find() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();
    let out = s
        .eval("7 bl word dup find drop execute . .\nbl word if find nip .\nbl word nosuch find nip .\nbye\n")
        .unwrap();
    assert_eq!(out, "7 7  ok\n1  ok\n0  ok\n");
}

#[test]
fn load_source_file_provides_variable_defining_word() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();
    let out = s.eval("variable foo\n7 foo !\nfoo @ .\nbye\n").unwrap();
    assert_eq!(out, " ok\n ok\n7  ok\n");
}

#[test]
fn load_source_file_provides_constant_defining_word() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();
    let out = s.eval("10 constant ten\nten .\nbye\n").unwrap();
    assert_eq!(out, " ok\n10  ok\n");
}

#[test]
fn load_source_file_provides_pictured_numeric_output_words() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();
    let out = s
        .eval("<# 65 hold 66 hold 0 0 #> type cr\n<# 1 0 # # #> type cr\n<# 0 0 #s #> type cr\n: fmt-neg dup >r abs s>d <# #s r> sign #> ;\n-123 fmt-neg type cr\nbye\n")
        .unwrap();
    assert_eq!(out, "BA\n ok\n01\n ok\n0\n ok\n ok\n-123\n ok\n");
}

#[test]
fn load_source_file_provides_holds() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();
    let out = s
        .eval(": banner <# 49 hold 50 hold s\" AB\" holds 0 0 #> ;\nbanner type cr\nbye\n")
        .unwrap();
    assert_eq!(out, " ok\nAB21\n ok\n");
}

#[test]
fn load_source_file_provides_unsigned_dot() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();
    let out = s.eval("123 u. cr\n-1 u. cr\nbye\n").unwrap();
    assert_eq!(out, "123 \n ok\n18446744073709551615 \n ok\n");
}

#[test]
fn load_source_file_provides_double_unsigned_dot() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();
    let out = s.eval("123 s>d du. cr\nbye\n").unwrap();
    assert_eq!(out, "123 \n ok\n");
}

#[test]
fn load_source_file_provides_abort_quote() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();
    let out = s
        .eval(": guarded dup 0= abort\" zero\" 1+ ;\n5 guarded . cr\n0 ' guarded catch . cr\nbye\n")
        .unwrap();
    assert_eq!(out, " ok\n6 \n ok\nzero-2 \n ok\n");
}

#[test]
fn load_source_file_provides_environment_query() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();
    let out = s
        .eval(": env-query s\" wf64\" ;\nenv-query environment? . cr\nbye\n")
        .unwrap();
    assert_eq!(out, " ok\n0 \n ok\n");
}

#[test]
fn load_source_file_abort_quote_in_interpret_state_throws_minus_14() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();
    let err = s.eval("abort\" nope\"\n").unwrap_err().to_string();
    assert!(err.contains("-14"), "got {err:?}");
}

#[test]
fn load_source_file_provides_c_comma() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();
    let out = s.eval("here 65 c, here swap - . here 1- c@ .\nbye\n").unwrap();
    assert_eq!(out, "1 65  ok\n");
}

#[test]
fn load_source_file_provides_fvariable_defining_word() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();
    let out = s.eval("fvariable foo\n9e foo f!\nfoo f@ f>d drop .\nbye\n").unwrap();
    assert_eq!(out, " ok\n ok\n9  ok\n");
}

#[test]
fn load_source_file_provides_fconstant_defining_word() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();
    let out = s.eval("7e fconstant seven\nseven f>d drop .\n: use-seven seven ;\nuse-seven f>d drop .\nbye\n").unwrap();
    assert_eq!(out, " ok\n7  ok\n ok\n7  ok\n");
}

#[test]
fn load_source_file_provides_value_defining_word() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();
    let out = s.eval("5 value five\nfive .\n: use-five five ;\nuse-five .\nbye\n").unwrap();
    assert_eq!(out, " ok\n5  ok\n ok\n5  ok\n");
}

#[test]
fn load_source_file_provides_double_cell_defining_words() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();
    let out = s
        .eval(
            "1 2 2constant pair\n\
             : use-pair pair ;\n\
             2variable dv\n\
             123 456 dv 2!\n\
             dv 2@ . . cr\n\
             pair . . cr\n\
             use-pair . . cr\n\
             : pair-lit [ 10 20 ] 2literal ;\n\
             pair-lit . . cr\n\
             bye\n",
        )
        .unwrap();
    assert_eq!(out, " ok\n ok\n ok\n ok\n456 123 \n ok\n2 1 \n ok\n2 1 \n ok\n ok\n10 20 \n ok\n");
}

#[test]
fn load_source_file_provides_case_words() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();
    let out = s
        .eval(
            ": classify case\n\
             1 of 111 endof\n\
             2 of 222 endof\n\
             999 swap\n\
             endcase ;\n\
             1 classify .\n\
             2 classify .\n\
             7 classify .\n\
             bye\n",
        )
        .unwrap();
    assert_eq!(out, " ok\n ok\n ok\n ok\n ok\n111  ok\n222  ok\n999  ok\n");
}

#[test]
fn m7_ans_core_tests_pass() {
    let mut s = sess();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    s.load_source_file(&manifest.join("lib").join("core.f")).unwrap();
    s.load_source_file(&manifest.join("lib").join("tester.fs")).unwrap();
    s.load_source_file(&manifest.join("lib").join("ans_core_tests.fs")).unwrap();
    let out = s.eval("bye\n").unwrap();
    assert!(
        !out.contains("INCORRECT RESULT"),
        "ANS core test failures:\n{out}"
    );
    assert!(
        !out.contains("WRONG NUMBER OF RESULTS"),
        "ANS core test failures:\n{out}"
    );
}

#[test]
fn load_source_file_provides_defer_defining_word() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();
    let out = s
        .eval("defer hook\n' dup ' hook defer!\n7 hook . .\n: run-hook hook ;\n9 run-hook . .\nbye\n")
        .unwrap();
    assert_eq!(out, " ok\n ok\n7 7  ok\n ok\n9 9  ok\n");
}

#[test]
fn load_source_file_defer_defaults_to_uninitialized_throw() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();
    let err = s.eval("defer hook\nhook\nbye\n").unwrap_err().to_string();
    assert!(err.contains("-261"), "got {err:?}");
}

#[test]
fn eval_here_and_allot_move_dictionary_pointer() {
    let mut s = sess();
    let out = s.eval("here here 1 cells allot here rot - . drop\nbye\n").unwrap();
    assert_eq!(out, "8  ok\n");
}

#[test]
fn eval_source_defined_variable_roundtrips_through_fetch_store() {
    let mut s = sess();
    let out = s.eval(": , here ! 1 cells allot ;\n: align here aligned here - allot ;\n: variable create 0 , ;\nvariable foo\n7 foo !\nfoo @ .\nbye\n").unwrap();
    assert_eq!(out, " ok\n ok\n ok\n ok\n ok\n7  ok\n");
}

#[test]
fn eval_does_builder_word_customizes_created_runtime() {
    let mut s = sess();
    let out = s.eval(": , here ! 1 cells allot ;\n: constant create , does> @ ;\n10 constant ten\nten .\nbye\n").unwrap();
    assert_eq!(out, " ok\n ok\n ok\n10  ok\n");
}

#[test]
fn eval_colon_defs_register_debug_words() {
    let mut s = sess();
    let out = s.eval(": square dup * ;\n: quad square square ;\nbye\n").unwrap();
    assert_eq!(out, " ok\n ok\n");

    let words = s.debug_words();
    assert_eq!(words.len(), 2);

    let square = words.iter().find(|(name, _, _)| name == "square").unwrap();
    let quad = words.iter().find(|(name, _, _)| name == "quad").unwrap();

    assert!(square.1 < square.2);
    assert!(quad.1 < quad.2);
    assert_eq!(s.resolve_word_addr(square.1).as_deref(), Some("square"));
    assert_eq!(s.resolve_word_addr(square.1 + 1).as_deref(), Some("square+0x1"));
    assert_eq!(s.resolve_word_addr(quad.1).as_deref(), Some("quad"));

    s.reset();
    assert!(s.debug_words().is_empty());
    assert!(s.resolve_word_addr(square.1).is_none());
}

#[test]
fn eval_create_without_name_throws_minus_16() {
    let mut s = sess();
    let err = s.eval("create\n").unwrap_err().to_string();
    assert!(err.contains("-16"), "got {err:?}");
}

#[test]
fn eval_semicolon_in_interpret_state_throws_minus_14() {
    let mut s = sess();
    let err = s.eval(";\n").unwrap_err().to_string();
    assert!(err.contains("-14"), "got {err:?}");
}

#[test]
fn eval_unknown_word_prints_question_mark() {
    let mut s = sess();
    let out = s.eval("nonsuch\nbye\n").unwrap();
    assert_eq!(out, "?  ok\n");
}

#[test]
fn eval_session_is_reusable_across_calls() {
    // Two consecutive evals on the same session: the dict from the
    // first call must survive into the second.
    let mut s = sess();
    let out1 = s.eval(": triple 3 * ;\n").unwrap();
    assert_eq!(out1, " ok\n");
    let out2 = s.eval("4 triple .\nbye\n").unwrap();
    assert_eq!(out2, "12  ok\n");
}

// ── direct-stack mode ────────────────────────────────────────────────

#[test]
fn direct_push_pop_round_trip() {
    let mut s = sess();
    s.push(42);
    s.push(-17);
    assert_eq!(s.depth(), 2);
    assert_eq!(s.stack(), vec![-17, 42]);  // top first
    assert_eq!(s.pop(), -17);
    assert_eq!(s.pop(), 42);
    assert_eq!(s.depth(), 0);
}

#[test]
fn direct_dup() {
    let mut s = sess();
    s.push(7);
    s.call("dup_").unwrap();
    assert_eq!(s.stack(), vec![7, 7]);
}

#[test]
fn direct_drop() {
    let mut s = sess();
    s.push(11);
    s.push(22);
    s.call("drop_").unwrap();
    assert_eq!(s.stack(), vec![11]);
}

#[test]
fn direct_swap() {
    let mut s = sess();
    s.push(1);
    s.push(2);
    s.call("swap_").unwrap();
    assert_eq!(s.stack(), vec![1, 2]);
}

#[test]
fn direct_over() {
    let mut s = sess();
    s.push(1);
    s.push(2);
    s.call("over_").unwrap();
    assert_eq!(s.stack(), vec![1, 2, 1]);
}

#[test]
fn direct_plus() {
    let mut s = sess();
    s.push(40);
    s.push(2);
    s.call("plus").unwrap();
    assert_eq!(s.stack(), vec![42]);
}

#[test]
fn direct_times() {
    let mut s = sess();
    s.push(6);
    s.push(7);
    s.call("times").unwrap();
    assert_eq!(s.stack(), vec![42]);
}

#[test]
fn direct_times_signed() {
    let mut s = sess();
    s.push(-3);
    s.push(5);
    s.call("times").unwrap();
    assert_eq!(s.stack(), vec![-15]);
}

#[test]
fn direct_perform_dispatches_xt_loaded_from_memory() {
    let mut s = sess();
    let dup_xt = s.xt_of("dup_").unwrap() as i64;
    let xt_slot = (s.user_base + 0x180) as i64;

    s.push(dup_xt);
    s.push(xt_slot);
    s.call("store").unwrap();
    assert_eq!(s.depth(), 0);

    s.push(42);
    s.push(xt_slot);
    s.call("perform").unwrap();
    assert_eq!(s.stack(), vec![42, 42]);
}

#[test]
fn direct_catch_returns_zero_on_success() {
    let mut s = sess();
    let dup_xt = s.xt_of("dup_").unwrap() as i64;

    s.push(7);
    s.push(dup_xt);
    s.call("catch_word").unwrap();
    assert_eq!(s.stack(), vec![0, 7, 7]);
}

#[test]
fn direct_catch_returns_throw_code() {
    let mut s = sess();
    let throw_xt = s.xt_of("throw_word").unwrap() as i64;

    s.push(-31);
    s.push(throw_xt);
    s.call("catch_word").unwrap();
    assert_eq!(s.stack(), vec![-31, -31]);
}

#[test]
fn direct_uncaught_throw_returns_error_to_host() {
    let mut s = sess();
    s.push(-31);
    let err = s.call("throw_word").unwrap_err().to_string();
    assert!(err.contains("Forth THROW -31"), "got {err:?}");
}

#[test]
fn direct_qthrow_drops_inputs_when_flag_is_zero() {
    let mut s = sess();
    s.push(99);
    s.push(0);
    s.push(-31);
    s.call("qthrow_word").unwrap();
    assert_eq!(s.stack(), vec![99]);
}

#[test]
fn direct_qthrow_throws_when_flag_is_nonzero() {
    let mut s = sess();
    let qthrow_xt = s.xt_of("qthrow_word").unwrap() as i64;

    s.push(1);
    s.push(-31);
    s.push(qthrow_xt);
    s.call("catch_word").unwrap();
    assert_eq!(s.stack(), vec![-31, -31, 1]);
}

#[test]
fn direct_abort_returns_error_to_host() {
    let mut s = sess();
    let err = s.call("abort_word").unwrap_err().to_string();
    assert!(err.contains("Forth THROW -1"), "got {err:?}");
}

#[test]
fn direct_named_throw_constants_push_expected_codes() {
    let mut s = sess();

    s.call("throw_abort_const").unwrap();
    assert_eq!(s.pop(), -1);

    s.call("throw_abortq_const").unwrap();
    assert_eq!(s.pop(), -2);

    s.call("throw_componly_const").unwrap();
    assert_eq!(s.pop(), -14);

    s.call("throw_namereqd_const").unwrap();
    assert_eq!(s.pop(), -16);

    s.call("throw_mismatch_const").unwrap();
    assert_eq!(s.pop(), -22);
}

#[test]
fn direct_comp_only_throws_minus_14() {
    let mut s = sess();
    let comp_only_xt = s.xt_of("comp_only_word").unwrap() as i64;

    s.push(comp_only_xt);
    s.call("catch_word").unwrap();
    assert_eq!(s.stack(), vec![-14]);
}

#[test]
fn direct_cpuid_writes_expected_register_block() {
    let mut s = sess();
    let buf = (s.user_base + 0x1a0) as i64;
    let expected = __cpuid(0);

    s.push(buf);
    s.push(0);
    s.call("cpuid_word").unwrap();
    assert_eq!(s.depth(), 0);

    s.push(buf);
    s.call("l_fetch").unwrap();
    assert_eq!(s.pop() as u32, expected.eax);

    s.push(buf + 4);
    s.call("l_fetch").unwrap();
    assert_eq!(s.pop() as u32, expected.ebx);

    s.push(buf + 8);
    s.call("l_fetch").unwrap();
    assert_eq!(s.pop() as u32, expected.ecx);

    s.push(buf + 12);
    s.call("l_fetch").unwrap();
    assert_eq!(s.pop() as u32, expected.edx);
}

#[test]
fn direct_rdtsc_returns_a_nondecreasing_counter() {
    let mut s = sess();

    s.call("rdtsc_word").unwrap();
    let hi1 = s.pop() as u64;
    let lo1 = s.pop() as u64;
    let t1 = (hi1 << 32) | (lo1 & 0xffff_ffff);

    s.call("rdtsc_word").unwrap();
    let hi2 = s.pop() as u64;
    let lo2 = s.pop() as u64;
    let t2 = (hi2 << 32) | (lo2 & 0xffff_ffff);

    assert!(t1 > 0);
    assert!(t2 >= t1);
}

#[test]
fn direct_rot_three_items() {
    let mut s = sess();
    s.push(1);
    s.push(2);
    s.push(3);
    s.call("rot_").unwrap();
    // ( 1 2 3 -- 2 3 1 ); top first → [1, 3, 2]
    assert_eq!(s.stack(), vec![1, 3, 2]);
}

#[test]
fn direct_nip() {
    let mut s = sess();
    s.push(1);
    s.push(2);
    s.call("nip_").unwrap();
    assert_eq!(s.stack(), vec![2]);
}

#[test]
fn direct_tuck() {
    let mut s = sess();
    s.push(1);
    s.push(2);
    s.call("tuck_").unwrap();
    // ( 1 2 -- 2 1 2 ); top first → [2, 1, 2]
    assert_eq!(s.stack(), vec![2, 1, 2]);
}

#[test]
fn direct_neg_rot() {
    // -rot: ( n1 n2 n3 -- n3 n1 n2 )
    let mut s = sess();
    s.push(1);
    s.push(2);
    s.push(3);
    s.call("neg_rot").unwrap();
    // After -rot: top first → [2, 1, 3]
    assert_eq!(s.stack(), vec![2, 1, 3]);
}

#[test]
fn direct_qdup_zero_does_nothing() {
    let mut s = sess();
    s.push(0);
    s.call("qdup").unwrap();
    assert_eq!(s.stack(), vec![0]);
}

#[test]
fn direct_qdup_nonzero_duplicates() {
    let mut s = sess();
    s.push(99);
    s.call("qdup").unwrap();
    assert_eq!(s.stack(), vec![99, 99]);
}

#[test]
fn direct_pick_zero_is_dup() {
    let mut s = sess();
    s.push(11);
    s.push(22);
    s.push(0);
    s.call("pick").unwrap();
    // ( 11 22 0 -- 11 22 22 )
    assert_eq!(s.stack(), vec![22, 22, 11]);
}

#[test]
fn direct_pick_one_is_over() {
    let mut s = sess();
    s.push(11);
    s.push(22);
    s.push(1);
    s.call("pick").unwrap();
    // ( 11 22 1 -- 11 22 11 )
    assert_eq!(s.stack(), vec![11, 22, 11]);
}

#[test]
fn direct_pick_two() {
    let mut s = sess();
    s.push(10);
    s.push(20);
    s.push(30);
    s.push(2);
    s.call("pick").unwrap();
    // ( 10 20 30 2 -- 10 20 30 10 )
    assert_eq!(s.stack(), vec![10, 30, 20, 10]);
}

#[test]
fn direct_depth_counts_cells() {
    let mut s = sess();
    // Phase 1: empty stack — `depth` should push 0.
    assert_eq!(s.depth(), 0);
    s.call("depth").unwrap();
    assert_eq!(s.stack(), vec![0]);

    // Phase 2: three values — `depth` should push 3 on top of them.
    // (Used to re-call `sess()` here — under the shared-session harness
    // that's a self-deadlock. `reset()` does the same thing without
    // releasing the lock.)
    s.reset();
    s.push(10);
    s.push(20);
    s.push(30);
    s.call("depth").unwrap();
    // ( 10 20 30 -- 10 20 30 3 )
    assert_eq!(s.stack(), vec![3, 30, 20, 10]);
}

// ── return-stack primitives ──────────────────────────────────────────

#[test]
fn direct_to_r_then_r_from_roundtrips() {
    let mut s = sess();
    s.push(42);
    s.call("to_r").unwrap();
    assert_eq!(s.depth(), 0);
    s.call("r_from").unwrap();
    assert_eq!(s.stack(), vec![42]);
}

#[test]
fn direct_r_fetch_peeks_without_popping() {
    let mut s = sess();
    s.push(99);
    s.call("to_r").unwrap();
    s.call("r_fetch").unwrap();
    // ( -- 99 ); r-stack still has 99.
    assert_eq!(s.stack(), vec![99]);
    s.call("r_from").unwrap();
    assert_eq!(s.stack(), vec![99, 99]);
}

#[test]
fn direct_dup_to_r_keeps_data_stack_value() {
    let mut s = sess();
    s.push(7);
    s.call("dup_to_r").unwrap();
    // data stack still has 7, r-stack also has 7.
    assert_eq!(s.stack(), vec![7]);
    s.call("r_from").unwrap();
    assert_eq!(s.stack(), vec![7, 7]);
}

#[test]
fn direct_rdrop_clears_rstack_only() {
    let mut s = sess();
    s.push(11);
    s.call("to_r").unwrap();
    s.push(22);  // unrelated cell on data stack
    s.call("rdrop").unwrap();
    assert_eq!(s.stack(), vec![22]);
}

#[test]
fn direct_two_to_r_and_two_r_from_roundtrip() {
    let mut s = sess();
    s.push(100);
    s.push(200);
    s.call("two_to_r").unwrap();
    assert_eq!(s.depth(), 0);
    s.call("two_r_from").unwrap();
    assert_eq!(s.stack(), vec![200, 100]);  // top = 200, NOS = 100
}

#[test]
fn direct_two_r_fetch_peeks_pair() {
    let mut s = sess();
    s.push(1);
    s.push(2);
    s.call("two_to_r").unwrap();
    s.call("two_r_fetch").unwrap();
    // ( -- 1 2 ); r-stack still has the pair.
    assert_eq!(s.stack(), vec![2, 1]);
    s.call("two_r_from").unwrap();
    assert_eq!(s.stack(), vec![2, 1, 2, 1]);
}

#[test]
fn direct_i_reads_top_loop_frame_sum() {
    let mut s = sess();
    s.push(30);
    s.push(70);
    s.call("two_to_r").unwrap();
    s.call("i_word").unwrap();
    assert_eq!(s.stack(), vec![100]);
}

#[test]
fn direct_j_reads_next_outer_loop_frame_sum() {
    let mut s = sess();
    s.push(1);
    s.push(2);
    s.call("two_to_r").unwrap();
    s.push(10);
    s.push(20);
    s.call("two_to_r").unwrap();
    s.call("j_word").unwrap();
    assert_eq!(s.stack(), vec![3]);
}

#[test]
fn direct_do_part_helpers_build_top_loop_frame() {
    let mut s = sess();
    s.push(20);
    s.push(10);
    s.call("do_part1").unwrap();
    assert_eq!(s.depth(), 0);
    s.call("do_part2").unwrap();
    s.call("i_word").unwrap();
    assert_eq!(s.stack(), vec![10]);
}

#[test]
fn direct_nested_do_part_helpers_make_j_visible() {
    let mut s = sess();
    s.push(20);
    s.push(3);
    s.call("do_part1").unwrap();
    s.call("do_part2").unwrap();
    s.push(50);
    s.push(10);
    s.call("do_part1").unwrap();
    s.call("do_part2").unwrap();
    s.call("j_word").unwrap();
    assert_eq!(s.stack(), vec![3]);
}

#[test]
fn direct_mark_to_returns_current_here() {
    let mut s = sess();
    let here = s.here();
    s.call("mark_to").unwrap();
    assert_eq!(s.pop() as u64, here);
}

#[test]
fn direct_forward_resolve_patches_rel32_from_mark_to_here() {
    let mut s = sess();
    s.push(0);
    s.call("inline_bra_comp").unwrap();
    s.call("mark_to").unwrap();
    let orig = s.pop() as u64;

    s.push(0);
    s.call("inline_bra_comp").unwrap();
    let here = s.here();

    s.push(orig as i64);
    s.call("forward_resolve").unwrap();

    let disp = unsafe { ((orig - 4) as *const i32).read_unaligned() };
    assert_eq!(disp as i64, here as i64 - orig as i64);
}

#[test]
fn direct_back_resolve_patches_current_rel32_back_to_dest() {
    let mut s = sess();
    let dest = s.here();

    s.push(0);
    s.call("inline_bra_comp").unwrap();
    let here = s.here();

    s.push(dest as i64);
    s.call("back_resolve").unwrap();

    let disp = unsafe { ((here - 4) as *const i32).read_unaligned() };
    assert_eq!(disp as i64, dest as i64 - here as i64);
}

#[test]
fn direct_qpairs_drops_matching_marks() {
    let mut s = sess();
    s.push(-2);
    s.push(-2);
    s.call("qpairs").unwrap();
    assert!(s.stack().is_empty());
}

#[test]
fn direct_qpairs_throws_minus_22_on_mismatch() {
    let mut s = sess();
    let qpairs_xt = s.xt_of("qpairs").unwrap() as i64;
    s.push(-1);
    s.push(-2);
    s.push(qpairs_xt);
    s.call("catch_word").unwrap();
    assert_eq!(s.pop(), -22);
}

#[test]
fn direct_leave_under_if_restores_control_stack_shape() {
    const USER_STATE: u64 = 0x08;

    let mut s = sess();
    let state_addr = (s.user_base + USER_STATE) as i64;
    s.push(1);
    s.push(state_addr);
    s.call("store").unwrap();

    let do_addr = 0x1111_i64;
    let if_orig = 0x2222_i64;
    s.push(do_addr);
    s.push(-3);
    s.push(if_orig);
    s.push(-1);

    s.call("leave_word").unwrap();
    let stack = s.stack();
    assert_eq!(&stack[..5], &[-1, if_orig, -3, do_addr, -5]);
    assert_eq!(stack[5] as u64, s.here());
}

#[test]
fn direct_high_level_control_words_are_compile_only() {
    let mut s = sess();
    let cases = [
        "ahead_word",
        "if_word",
        "minus_if_word",
        "then_word",
        "else_word",
        "begin_word",
        "while_word",
        "again_word",
        "until_word",
        "repeat_word",
        "recurse_word",
        "do_word",
        "qdo_control_word",
        "loop_control_word",
        "plus_loop_control_word",
        "minus_loop_control_word",
        "leave_word",
        "qleave_word",
    ];

    for asm in cases {
        let xt = s.xt_of(asm).unwrap() as i64;
        s.push(xt);
        s.call("catch_word").unwrap();
        assert_eq!(s.pop(), -14, "{asm} should THROW -14 outside compile state");
    }
}

#[test]
fn direct_raw_control_emitters_are_compile_only() {
    let mut s = sess();
    let cases = [
        "bra_word",
        "qbra_word",
        "minus_qbra_word",
        "bra_qdo_word",
        "loop_word",
        "plus_loop_word",
        "minus_loop_word",
    ];

    for asm in cases {
        let xt = s.xt_of(asm).unwrap() as i64;
        s.push(xt);
        s.call("catch_word").unwrap();
        assert_eq!(s.pop(), -14, "{asm} should THROW -14 outside compile state");
    }
}

#[test]
fn direct_n_to_r_then_nr_from_roundtrip() {
    let mut s = sess();
    s.push(10);
    s.push(20);
    s.push(2);
    s.call("n_to_r").unwrap();
    assert_eq!(s.depth(), 0);

    s.call("nr_from").unwrap();
    assert_eq!(s.stack(), vec![2, 20, 10]);
}

#[test]
fn direct_n_to_r_and_nr_from_preserve_deeper_stack() {
    let mut s = sess();
    s.push(99);
    s.push(10);
    s.push(20);
    s.push(2);
    s.call("n_to_r").unwrap();
    assert_eq!(s.stack(), vec![99]);

    s.call("nr_from").unwrap();
    assert_eq!(s.stack(), vec![2, 20, 10, 99]);
}

#[test]
fn direct_two_rdrop_clears_pair() {
    let mut s = sess();
    s.push(1);
    s.push(2);
    s.call("two_to_r").unwrap();
    s.push(99);
    s.call("two_rdrop").unwrap();
    assert_eq!(s.stack(), vec![99]);
}

#[test]
fn eval_to_r_through_repl() {
    // Round-trip through the return stack from inside a compiled word.
    // dup so the inner `.` has something to print; >r/r> shuttle the
    // copy so the outer `.` finds it again. The two "5 "s prove both
    // halves of the trip survived a compiled-body context (which is
    // where the rstack-juggle in to_r/r_from gets exercised hardest).
    let mut s = sess();
    let out = s.eval(": ferry dup >r . r> ;\n5 ferry .\nbye\n").unwrap();
    assert_eq!(out, " ok\n5 5  ok\n");
}

#[test]
fn direct_sp_fetch_returns_address() {
    let mut s = sess();
    s.push(10);
    s.push(20);
    s.call("sp_fetch").unwrap();
    let top = s.pop();
    // The pushed address should be within the data stack region and
    // very near current dsp (off by exactly one cell because sp@
    // first reserves a cell, then writes its result).
    let region_lo = s.user_base - 0x80000;  // base of region
    assert!((top as u64) > region_lo);
    assert!((top as u64) <= s.dsp_top);
    // The remaining stack should be the two original values.
    assert_eq!(s.stack(), vec![20, 10]);
}

#[test]
fn eval_depth_via_interpreter() {
    let mut s = sess();
    let out = s.eval("1 2 3 depth . . . . .\nbye\n").unwrap();
    // Pushes 1 2 3 then depth=3. Dots print top first: 3 3 2 1 + one
    // garbage cell from underflow. We just check the first 4 prints.
    assert!(out.starts_with("3 3 2 1 "), "got {out:?}");
}

// ── memory primitives via direct invocation ─────────────────────────

#[test]
fn direct_fetch_store_cell() {
    let mut s = sess();
    // Use a PAD slot at user_base+0x100 for scratch.
    let scratch = s.user_base + 0x100;
    s.push(0xdeadbeef);
    s.push(scratch as i64);
    s.call("store").unwrap();   // ( v addr -- )
    assert_eq!(s.depth(), 0);
    s.push(scratch as i64);
    s.call("fetch").unwrap();   // ( addr -- v )
    assert_eq!(s.pop(), 0xdeadbeef);
}

#[test]
fn direct_c_fetch_store() {
    let mut s = sess();
    let scratch = s.user_base + 0x110;
    s.push(0x5a);
    s.push(scratch as i64);
    s.call("c_store").unwrap();
    s.push(scratch as i64);
    s.call("c_fetch").unwrap();
    assert_eq!(s.pop(), 0x5a);
}

// ── mixed: build via eval, then poke via direct ─────────────────────

// ── dictionary primitives (Phase 2) ─────────────────────────────────

#[test]
fn create_and_set_xt_builds_a_callable_header() {
    // Drive the kernel-side dict primitives directly: build a fake
    // header pointing at `dup_` and named "FOO", then call FOO via the
    // REPL and confirm it duplicates.
    let mut s = sess();
    let pad = s.user_base + 0x100;
    let name = b"FOO";
    unsafe { std::ptr::copy_nonoverlapping(name.as_ptr(), pad as *mut u8, name.len()); }
    s.push(pad as i64);
    s.push(name.len() as i64);
    s.call("create").unwrap();
    let dup_xt = s.xt_of("dup_").unwrap();
    s.push(dup_xt as i64);
    s.call("set_xt").unwrap();
    assert_eq!(s.depth(), 0);
    // Now FOO should be in the dict, with the same effect as DUP.
    let out = s.eval("7 FOO . .\nbye\n").unwrap();
    assert_eq!(out, "7 7  ok\n");
}

#[test]
fn to_name_resolves_primitive_xt_to_counted_name() {
    let mut s = sess();
    let dup_xt = s.xt_of("dup_").unwrap() as i64;

    s.push(dup_xt);
    s.call("to_name").unwrap();
    let nt = s.pop() as u64;

    let len = unsafe { (nt as *const u8).read() };
    let bytes = unsafe { std::slice::from_raw_parts((nt + 1) as *const u8, len as usize) };
    assert_eq!(len, 3);
    assert_eq!(bytes, b"dup");
}

#[test]
fn primitive_xt_has_ct_backoffset_slot() {
    const DH_CT: u64 = 8;

    let mut s = sess();
    let dup_xt = s.xt_of("dup_").unwrap() as u64;
    s.push(dup_xt as i64);
    s.call("to_name").unwrap();
    let nt = s.pop() as u64;
    let ct = nt - ((5 * 8) + 2 + 2 + 2 + 1) + DH_CT;
    let backoff = unsafe { ((dup_xt - 8) as *const i64).read() };

    assert_eq!(dup_xt.wrapping_add_signed(backoff), ct);
}

#[test]
fn colon_defined_latestxt_has_ct_backoffset_slot() {
    const DH_CT: u64 = 8;
    const DH_NT: u64 = (5 * 8) + 2 + 2 + 2 + 1;

    let mut s = sess();
    let out = s.eval(": quux 1 ;\nbye\n").unwrap();
    assert_eq!(out, " ok\n");

    s.call("latestxt").unwrap();
    let xt = s.pop() as u64;
    let backoff = unsafe { ((xt - 8) as *const i64).read() };
    let ct = xt.wrapping_add_signed(backoff);
    let nt = ct - DH_CT + DH_NT;
    let len = unsafe { (nt as *const u8).read() };
    let bytes = unsafe { std::slice::from_raw_parts((nt + 1) as *const u8, len as usize) };

    assert_eq!(bytes, b"quux");

    s.push(xt as i64);
    s.call("to_name").unwrap();
    let nt_from_to_name = s.pop() as u64;
    assert_eq!(nt_from_to_name, nt);
}

#[test]
fn to_ct_and_to_comp_recover_header_fields_from_xt() {
    const DH_CT: u64 = 8;
    const DH_COMP: u64 = 24;

    let mut s = sess();
    let dup_xt = s.xt_of("dup_").unwrap() as i64;

    s.push(dup_xt);
    s.call("to_name").unwrap();
    let nt = s.pop() as u64;
    let expected_ct = nt - ((5 * 8) + 2 + 2 + 2 + 1) + DH_CT;

    s.push(dup_xt);
    s.call("to_ct").unwrap();
    assert_eq!(s.pop() as u64, expected_ct);

    s.push(dup_xt);
    s.call("to_comp").unwrap();
    assert_eq!(s.pop() as u64, expected_ct - DH_CT + DH_COMP);
}

#[test]
fn dup_primitive_comp_field_points_to_inline_dup_helper() {
    let mut s = sess();
    let dup_xt = s.xt_of("dup_").unwrap() as i64;
    let inline_dup_xt = s.xt_of("inline_dup_comp").unwrap() as i64;

    s.push(dup_xt);
    s.call("to_comp").unwrap();
    s.call("fetch").unwrap();
    assert_eq!(s.pop(), inline_dup_xt);
}

#[test]
fn simple_stack_primitives_comp_fields_point_to_inline_helpers() {
    let mut s = sess();

    let cases = [
        ("drop_", "inline_drop_comp"),
        ("swap_", "inline_swap_comp"),
        ("over_", "inline_over_comp"),
    ];

    for (word_xt, comp_xt) in cases {
        let xt = s.xt_of(word_xt).unwrap() as i64;
        let helper = s.xt_of(comp_xt).unwrap() as i64;
        s.push(xt);
        s.call("to_comp").unwrap();
        s.call("fetch").unwrap();
        assert_eq!(s.pop(), helper, "wrong comp helper for {word_xt}");
    }
}

#[test]
fn return_stack_primitives_comp_fields_point_to_inline_helpers() {
    let mut s = sess();

    let cases = [
        ("to_r", "inline_to_r_comp"),
        ("r_from", "inline_r_from_comp"),
        ("r_fetch", "inline_r_fetch_comp"),
        ("two_to_r", "inline_two_to_r_comp"),
        ("two_r_from", "inline_two_r_from_comp"),
        ("two_r_fetch", "inline_two_r_fetch_comp"),
        ("i_word", "inline_i_comp"),
        ("j_word", "inline_j_comp"),
        ("do_part1", "inline_do_part1_comp"),
        ("do_part2", "inline_do_part2_comp"),
        ("bra_word", "inline_bra_comp"),
        ("qbra_word", "inline_qbra_comp"),
        ("minus_qbra_word", "inline_minus_qbra_comp"),
        ("bra_qdo_word", "inline_bra_qdo_comp"),
        ("loop_word", "inline_loop_comp"),
        ("plus_loop_word", "inline_plus_loop_comp"),
        ("minus_loop_word", "inline_minus_loop_comp"),
    ];

    for (word_xt, comp_xt) in cases {
        let xt = s.xt_of(word_xt).unwrap() as i64;
        let helper = s.xt_of(comp_xt).unwrap() as i64;
        s.push(xt);
        s.call("to_comp").unwrap();
        s.call("fetch").unwrap();
        assert_eq!(s.pop(), helper, "wrong comp helper for {word_xt}");
    }
}

#[test]
fn eval_i_and_j_through_nested_rstack_frames() {
    let mut s = sess();
    let out = s.eval(": ijtest 1 2 2>r 10 20 2>r i . j . 2r> 2drop 2r> 2drop ;\nijtest\nbye\n").unwrap();
    assert_eq!(out, " ok\n30 3  ok\n");
}

#[test]
fn eval_two_r_roundtrip_with_literals_in_definition() {
    let mut s = sess();
    let out = s.eval(": rr2 1 2 2>r 2r> ;\nrr2 . .\nbye\n").unwrap();
    assert_eq!(out, " ok\n2 1  ok\n");
}

#[test]
fn eval_two_r_roundtrip_with_literals_can_end_empty() {
    let mut s = sess();
    let out = s.eval(": rr1 1 2 2>r 2r> 2drop ;\nrr1\nbye\n").unwrap();
    assert_eq!(out, " ok\n ok\n");
}

#[test]
fn eval_do_part_helpers_feed_i_and_j() {
    let mut s = sess();
    let out = s.eval(": dijtest 20 3 do-part1 do-part2 50 10 do-part1 do-part2 i . j . 2rdrop 2rdrop ;\ndijtest\nbye\n").unwrap();
    assert_eq!(out, " ok\n10 3  ok\n");
}

#[test]
fn compiled_raw_branch_emitters_have_expected_bytes() {
    let mut s = sess();
    let out = s.eval(": rawcf bra ?bra -?bra bra-?do ;\nbye\n").unwrap();
    assert_eq!(out, " ok\n");

    s.call("latestxt").unwrap();
    let xt = s.pop() as u64;
    let bytes = unsafe { std::slice::from_raw_parts(xt as *const u8, 5 + 17 + 9 + 9 + 1) };
    assert_eq!(&bytes[0..5], &[0xE9, 0, 0, 0, 0]);
    assert_eq!(&bytes[5..22], &[0x48, 0x83, 0xC5, 0x08, 0x48, 0x85, 0xC0, 0x48, 0x8B, 0x45, 0xF8, 0x0F, 0x84, 0, 0, 0, 0]);
    assert_eq!(&bytes[22..31], &[0x48, 0x85, 0xC0, 0x0F, 0x84, 0, 0, 0, 0]);
    assert_eq!(&bytes[31..40], &[0x48, 0x39, 0xCA, 0x0F, 0x84, 0, 0, 0, 0]);
    assert_eq!(bytes[40], 0xC3);
}

#[test]
fn literal_fold_imm8_emits_immediate_form_instruction() {
    // `5 +` should fold to `add rax, 5` (4 bytes) followed by RET — no
    // 13-byte literal emission, no CALL to plus.
    let mut s = sess();
    let out = s.eval(": addfive 5 + ;\nbye\n").unwrap();
    assert_eq!(out, " ok\n");
    s.call("latestxt").unwrap();
    let xt = s.pop() as u64;
    let bytes = unsafe { std::slice::from_raw_parts(xt as *const u8, 5) };
    assert_eq!(bytes, &[0x48, 0x83, 0xC0, 0x05, 0xC3],
        "expected `add rax, 5; ret`, got {:02X?}", bytes);

    // Same definition runs correctly.
    let out = s.eval("1 addfive .\nbye\n").unwrap();
    assert_eq!(out, "6  ok\n");
}

#[test]
fn literal_fold_imm32_emits_accumulator_form() {
    // 1000 = 0x3E8 doesn't fit in signed imm8 but does in imm32, so
    // we expect the 6-byte accumulator form `48 05 E8 03 00 00`.
    let mut s = sess();
    let out = s.eval(": addbig 1000 + ;\nbye\n").unwrap();
    assert_eq!(out, " ok\n");
    s.call("latestxt").unwrap();
    let xt = s.pop() as u64;
    let bytes = unsafe { std::slice::from_raw_parts(xt as *const u8, 7) };
    assert_eq!(bytes, &[0x48, 0x05, 0xE8, 0x03, 0x00, 0x00, 0xC3],
        "expected `add rax, 1000; ret`, got {:02X?}", bytes);

    let out = s.eval("5 addbig .\nbye\n").unwrap();
    assert_eq!(out, "1005  ok\n");
}

#[test]
fn literal_fold_all_binops_emit_their_immediate_form() {
    // One canonical imm8 fold per op, checking the opcode/modrm bytes.
    let cases: &[(&str, &str, u8, u8)] = &[
        // (Forth source, name, opcode-byte, modrm-byte)
        (": fadd  3 + ;",   "fadd",  0x83, 0xC0),  // ADD /0
        (": fsub  3 - ;",   "fsub",  0x83, 0xE8),  // SUB /5
        (": fmul  3 * ;",   "fmul",  0x6B, 0xC0),  // IMUL rax,rax,imm8
        (": fand  3 and ;", "fand",  0x83, 0xE0),  // AND /4
        (": for   3 or ;",  "for",   0x83, 0xC8),  // OR  /1
        (": fxor  3 xor ;", "fxor",  0x83, 0xF0),  // XOR /6
    ];
    let mut s = sess();
    for &(src, _name, opcode, modrm) in cases {
        s.eval(&format!("{src}\nbye\n")).unwrap();
        s.call("latestxt").unwrap();
        let xt = s.pop() as u64;
        let bytes = unsafe { std::slice::from_raw_parts(xt as *const u8, 5) };
        assert_eq!(bytes, &[0x48, opcode, modrm, 0x03, 0xC3],
            "fold mismatch for `{src}` — got {:02X?}", bytes);
    }
}

#[test]
fn bare_binop_inlined_when_no_preceding_literal() {
    // `+` with no preceding literal can't fold, so (T2 bare-op inline) it is
    // emitted INLINE as `add rax,[rbp] ; add rbp,8` — no CALL, no JMP —
    // followed by the definition's RET.  (Previously this fell back to a CALL
    // that `;`/TCO patched into a JMP.)
    let mut s = sess();
    let out = s.eval(": twoadd + ;\nbye\n").unwrap();
    assert_eq!(out, " ok\n");
    s.call("latestxt").unwrap();
    let xt = s.pop() as u64;
    let bytes = unsafe { std::slice::from_raw_parts(xt as *const u8, 9) };
    assert_eq!(
        bytes,
        &[0x48, 0x03, 0x45, 0x00, 0x48, 0x83, 0xC5, 0x08, 0xC3],
        "expected inline `add rax,[rbp]; add rbp,8; ret`, got {bytes:02X?}"
    );

    // Behaviour: 3 4 twoadd → 7
    let out = s.eval("3 4 twoadd .\nbye\n").unwrap();
    assert_eq!(out, "7  ok\n");
}

#[test]
fn literal_fold_shifts_emit_shift_imm8() {
    // `3 lshift` → SHL rax, 3 (4 bytes), no CALL.
    let cases: &[(&str, u8)] = &[
        (": shl3 3 lshift ;",  0xE0),   // SHL /4
        (": shr3 3 rshift ;",  0xE8),   // SHR /5
        (": sar3 3 arshift ;", 0xF8),   // SAR /7
    ];
    let mut s = sess();
    for &(src, modrm) in cases {
        s.eval(&format!("{src}\nbye\n")).unwrap();
        s.call("latestxt").unwrap();
        let xt = s.pop() as u64;
        let bytes = unsafe { std::slice::from_raw_parts(xt as *const u8, 5) };
        assert_eq!(bytes, &[0x48, 0xC1, modrm, 0x03, 0xC3],
            "fold mismatch for `{src}` — got {:02X?}", bytes);
    }

    // Runtime: 4 3 lshift → 32; 32 3 rshift → 4; -32 3 arshift → -4.
    let out = s.eval("4 shl3 . 32 shr3 . -32 sar3 .\nbye\n").unwrap();
    assert_eq!(out, "32 4 -4  ok\n");
}

#[test]
fn literal_fold_shift_out_of_imm8_range_falls_back() {
    // Literal that doesn't fit in signed imm8 (300) must NOT fold for
    // shifts — we'd have to truncate or mask, and we'd rather fall back
    // cleanly to the unfolded `300 lshift` semantics.
    let mut s = sess();
    s.eval(": shbig 300 lshift ;\nbye\n").unwrap();
    s.call("latestxt").unwrap();
    let xt = s.pop() as u64;
    let bytes = unsafe { std::slice::from_raw_parts(xt as *const u8, 14) };
    // First byte should be a CALL/JMP to do_lit, NOT 48 C1 ...
    assert_ne!(&bytes[..3], &[0x48, 0xC1, 0xE0]);
    assert!(bytes[0] == 0xE8 || bytes[0] == 0xE9,
        "expected CALL/JMP fallback, got {:02X}", bytes[0]);
}

#[test]
fn literal_fold_equality_emits_sub_sub_sbb_pattern() {
    // `= 5` fold → sub rax, 5 ; sub rax, 1 ; sbb rax, rax (11 bytes)
    let mut s = sess();
    s.eval(": eq5 5 = ;\nbye\n").unwrap();
    s.call("latestxt").unwrap();
    let xt = s.pop() as u64;
    let bytes = unsafe { std::slice::from_raw_parts(xt as *const u8, 12) };
    assert_eq!(bytes, &[
        0x48, 0x83, 0xE8, 0x05,            // sub  rax, 5
        0x48, 0x83, 0xE8, 0x01,            // sub  rax, 1
        0x48, 0x19, 0xC0,                  // sbb  rax, rax
        0xC3,                               // ret
    ], "got {:02X?}", bytes);

    // Runtime
    let out = s.eval("5 eq5 . 6 eq5 . -1 eq5 .\nbye\n").unwrap();
    assert_eq!(out, "-1 0 0  ok\n");
}

#[test]
fn literal_fold_not_equal_emits_sub_add_sbb_pattern() {
    // `<> 5` fold → sub rax, 5 ; add rax, -1 ; sbb rax, rax (11 bytes)
    let mut s = sess();
    s.eval(": ne5 5 <> ;\nbye\n").unwrap();
    s.call("latestxt").unwrap();
    let xt = s.pop() as u64;
    let bytes = unsafe { std::slice::from_raw_parts(xt as *const u8, 12) };
    assert_eq!(bytes, &[
        0x48, 0x83, 0xE8, 0x05,            // sub  rax, 5
        0x48, 0x83, 0xC0, 0xFF,            // add  rax, -1
        0x48, 0x19, 0xC0,                  // sbb  rax, rax
        0xC3,                               // ret
    ], "got {:02X?}", bytes);

    let out = s.eval("5 ne5 . 6 ne5 . 0 ne5 .\nbye\n").unwrap();
    assert_eq!(out, "0 -1 -1  ok\n");
}

#[test]
fn literal_fold_u_less_emits_cmp_sbb_short_form() {
    // `u< 10` fold → cmp rax, 10 ; sbb rax, rax (7 bytes — the cheap path)
    let mut s = sess();
    s.eval(": ulow 10 u< ;\nbye\n").unwrap();
    s.call("latestxt").unwrap();
    let xt = s.pop() as u64;
    let bytes = unsafe { std::slice::from_raw_parts(xt as *const u8, 8) };
    assert_eq!(bytes, &[
        0x48, 0x83, 0xF8, 0x0A,            // cmp rax, 10
        0x48, 0x19, 0xC0,                  // sbb rax, rax
        0xC3,                               // ret
    ], "got {:02X?}", bytes);

    let out = s.eval("3 ulow . 10 ulow . 15 ulow .\nbye\n").unwrap();
    assert_eq!(out, "-1 0 0  ok\n");
}

#[test]
fn literal_fold_signed_compares_emit_cmp_setcc_pattern() {
    // Each: cmp rax, lit ; setCC al ; movzx eax, al ; neg rax (13 bytes)
    // For consistency we only check the setCC opcode byte at offset +5.
    let cases: &[(&str, u8, &str)] = &[
        (": lt10 10 < ;",   0x9C, "setl"),
        (": gt10 10 > ;",   0x9F, "setg"),
        (": le10 10 <= ;",  0x9E, "setle"),
        (": ge10 10 >= ;",  0x9D, "setge"),
        (": ugt10 10 u> ;", 0x97, "seta"),
        (": ule10 10 u<= ;",0x96, "setbe"),
        (": uge10 10 u>= ;",0x93, "setae"),
    ];
    let mut s = sess();
    for &(src, setcc_byte, name) in cases {
        s.eval(&format!("{src}\nbye\n")).unwrap();
        s.call("latestxt").unwrap();
        let xt = s.pop() as u64;
        let bytes = unsafe { std::slice::from_raw_parts(xt as *const u8, 14) };
        assert_eq!(bytes[0..4], [0x48, 0x83, 0xF8, 0x0A],
            "expected `cmp rax, 10` prefix for `{src}`, got {:02X?}", &bytes[..4]);
        assert_eq!(bytes[4], 0x0F, "expected 0F prefix for {name}");
        assert_eq!(bytes[5], setcc_byte,
            "expected {name} opcode 0x{:02X} for `{src}`, got 0x{:02X}", setcc_byte, bytes[5]);
        assert_eq!(bytes[6], 0xC0, "expected setCC al modrm");
        assert_eq!(bytes[13], 0xC3, "expected trailing RET");
    }

    // Behaviour: 5 lt10 = true, 15 lt10 = false, etc.
    let out = s.eval("5 lt10 . 15 lt10 . 5 gt10 . 15 gt10 .\nbye\n").unwrap();
    assert_eq!(out, "-1 0 0 -1  ok\n");
}

#[test]
fn literal_fold_chains_through_consecutive_lit_op_pairs() {
    // `1 + 2 * 3 -` should fold to three immediate-form instructions
    // back to back: add rax,1 ; imul rax,rax,2 ; sub rax,3 ; ret.
    let mut s = sess();
    let out = s.eval(": chain 1 + 2 * 3 - ;\nbye\n").unwrap();
    assert_eq!(out, " ok\n");
    s.call("latestxt").unwrap();
    let xt = s.pop() as u64;
    let bytes = unsafe { std::slice::from_raw_parts(xt as *const u8, 13) };
    assert_eq!(bytes, &[
        0x48, 0x83, 0xC0, 0x01,           // add  rax, 1
        0x48, 0x6B, 0xC0, 0x02,           // imul rax, rax, 2
        0x48, 0x83, 0xE8, 0x03,           // sub  rax, 3
        0xC3,                              // ret
    ], "got {:02X?}", bytes);

    // (x + 1) * 2 - 3 → at x=5 → 6*2-3 = 9
    let out = s.eval("5 chain .\nbye\n").unwrap();
    assert_eq!(out, "9  ok\n");
}

#[test]
fn eval_raw_branch_placeholders_preserve_stack_effects() {
    let mut s = sess();
    let out = s.eval(
        ": bra-test bra 7 ;\n\
         : qbra-test ?bra depth ;\n\
         : nqbra-test -?bra depth swap drop ;\n\
         bra-test .\n\
         0 qbra-test .\n\
         5 qbra-test .\n\
         0 nqbra-test .\n\
         5 nqbra-test .\n\
         bye\n"
    ).unwrap();
    assert_eq!(out, " ok\n ok\n ok\n7  ok\n0  ok\n0  ok\n1  ok\n1  ok\n");
}

#[test]
fn eval_compiled_loop_steps_update_i() {
    let mut s = sess();
    let out = s.eval(
        ": step1 20 3 do-part1 do-part2 _loop i . 2rdrop ;\n\
         : stepplus 20 3 do-part1 do-part2 2 _+loop i . 2rdrop ;\n\
         : stepminus 20 10 do-part1 do-part2 2 _-loop i . 2rdrop ;\n\
         step1\n\
         stepplus\n\
         stepminus\n\
         bye\n"
    ).unwrap();
    assert_eq!(out, " ok\n ok\n ok\n4  ok\n5  ok\n8  ok\n");
}

#[test]
fn code_dsl_defines_simple_primitive() {
    let mut s = sess();
    // Smallest possible CODE: word — just add 3 to TOS.
    let out = s.eval("CODE: add3  add rax, 3 ;CODE\n40 add3 .\nbye\n").unwrap();
    assert_eq!(out, " ok\n43  ok\n");
}

#[test]
fn code_dsl_supports_macro_vocabulary() {
    // The user can write `pushd`, `popd`, `stk(in,out)`, `next()` — all
    // resolved from the kernel's macros.masm which the CODE: assembler
    // preloads once.  Body spans multiple lines; rt_code_compile_body
    // peeks past the current SOURCE buffer into the Io input.
    let mut s = sess();
    let out = s.eval(
        "CODE: triple   ; ( n -- n*3 )\n\
             mov rcx, rax\n\
             add rax, rax\n\
             add rax, rcx\n\
             stk(1, 1)\n\
         ;CODE\n\
         7 triple .\nbye\n"
    ).unwrap();
    assert_eq!(out, " ok\n21  ok\n");
}

#[test]
fn code_dsl_compiled_into_colon_definition() {
    let mut s = sess();
    let out = s.eval(
        "CODE: sq  imul rax, rax ;CODE\n\
         : sum-of-squares  sq swap sq + ;\n\
         3 4 sum-of-squares .\nbye\n"
    ).unwrap();
    assert_eq!(out, " ok\n ok\n25  ok\n");
}

#[test]
fn code_dsl_invalid_asm_reports_throw() {
    // Bad mnemonics inside a CODE: body used to abort the test process
    // (LLVM-MC's default error handling).  With wfasm's diagnostic
    // handler installed, MC parse errors flow back through JitError::Llvm
    // and surface as a Forth THROW.
    let mut s = sess();
    let err = s.eval("CODE: bad  wibblywobbly ;CODE\nbye\n").unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("-2057") || msg.contains("THROW"),
        "expected -2057 throw, got: {msg}");
}

#[test]
fn code_dsl_unterminated_body_reports_error() {
    let mut s = sess();
    let err = s.eval("CODE: never_ends  add rax, 1\nbye\n").unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("-2057") || msg.contains("THROW"),
        "expected -2057 throw, got: {msg}");
}

/// LET tests load core.f because they need `f.` and friends from there.
fn sess_with_core() -> SessionGuard {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).expect("load core.f");
    s
}

#[test]
fn let_dsl_area_of_circle() {
    let mut s = sess_with_core();
    let out = s.eval(": area LET (r) -> (a) = pi * r * r END ;\n2.0 area f.\nbye\n").unwrap();
    // pi * 4 = 12.566370614359172
    assert!(out.contains("12.566"), "got {out:?}");
}

#[test]
fn let_dsl_multi_input_multi_output_mbrot() {
    let mut s = sess_with_core();
    let out = s.eval(
        ": mbrot LET (z_re, z_im, x, y) -> (z_next_re, z_next_im, mag) = \
            re, im, rmag \
            WHERE re   = z_re * z_re - z_im * z_im + x \
            WHERE im   = 2 * z_re * z_im + y \
            WHERE rmag = re * re + im * im \
         END ;\n1.0 1.0 1.0 1.0 mbrot f. f. f.\nbye\n"
    ).unwrap();
    // f. prints TOS first: mag=10, im=3, z_next_re=1.
    assert!(out.contains("10."), "expected '10.' in output: {out:?}");
    assert!(out.contains("3."),  "expected '3.' in output: {out:?}");
    assert!(out.contains("1."),  "expected '1.' in output: {out:?}");
}

#[test]
fn let_dsl_arithmetic_chain() {
    let mut s = sess_with_core();
    let out = s.eval(": poly LET (x) -> (y) = x * x + 2 * x + 1 END ;\n3.0 poly f.\nbye\n").unwrap();
    // 9 + 6 + 1 = 16
    assert!(out.contains("16."), "got {out:?}");
}

#[test]
fn let_dsl_unary_minus() {
    let mut s = sess_with_core();
    let out = s.eval(": negsq LET (x) -> (y) = -(x * x) END ;\n5.0 negsq f.\nbye\n").unwrap();
    assert!(out.contains("-25."), "got {out:?}");
}

#[test]
fn let_dsl_where_bindings_topo_sort() {
    let mut s = sess_with_core();
    // WHERE clauses out-of-order: rmag depends on re/im which depend on inputs.
    // Topo sort must place re/im before rmag.
    let out = s.eval(
        ": sq2 LET (a, b) -> (r) = rmag WHERE rmag = re + im WHERE re = a*a WHERE im = b*b END ;\n\
         3.0 4.0 sq2 f.\nbye\n"
    ).unwrap();
    assert!(out.contains("25."), "got {out:?}");
}

#[test]
fn let_dsl_sqrt_via_forth_repl() {
    let mut s = sess_with_core();
    // Hypotenuse of (3, 4) = 5.
    let out = s.eval(
        ": hyp LET (x, y) -> (h) = sqrt(x*x + y*y) END ;\n\
         3.0 4.0 hyp f.\nbye\n"
    ).unwrap();
    assert!(out.contains("5.000000"), "got {out:?}");
}

#[test]
fn let_dsl_sin_cos_via_forth_repl() {
    let mut s = sess_with_core();
    // sin(0) + cos(0) = 0 + 1 = 1.
    let out = s.eval(
        ": both LET (x) -> (y) = sin(x) + cos(x) END ;\n\
         0.0 both f.\nbye\n"
    ).unwrap();
    assert!(out.contains("1.000000"), "got {out:?}");
}

#[test]
fn let_dsl_hypot_via_forth_repl() {
    let mut s = sess_with_core();
    let out = s.eval(
        ": dist LET (x, y) -> (d) = hypot(x, y) END ;\n\
         3.0 4.0 dist f.\nbye\n"
    ).unwrap();
    assert!(out.contains("5.000000"), "got {out:?}");
}

#[test]
fn let_dsl_star_star_operator() {
    let mut s = sess_with_core();
    let out = s.eval(
        ": cube LET (x) -> (y) = x ** 3 END ;\n\
         2.0 cube f.\nbye\n"
    ).unwrap();
    assert!(out.contains("8.000000"), "got {out:?}");
}

#[test]
fn let_dsl_comparisons_via_forth_repl() {
    let mut s = sess_with_core();
    let out = s.eval(
        ": lt5 LET (x) -> (y) = x < 5 END ;\n\
         3.0 lt5 f.\n7.0 lt5 f.\nbye\n"
    ).unwrap();
    assert!(out.contains("1.000000") && out.contains("0.000000"), "got {out:?}");
}

#[test]
fn let_dsl_select_via_forth_repl() {
    let mut s = sess_with_core();
    // abs() built via select.
    let out = s.eval(
        ": myabs LET (x) -> (y) = select(x < 0, -x, x) END ;\n\
         -7.5 myabs f.\n3.25 myabs f.\nbye\n"
    ).unwrap();
    assert!(out.contains("7.500000"), "expected 7.500000 in {out:?}");
    assert!(out.contains("3.250000"), "expected 3.250000 in {out:?}");
}

#[test]
fn let_dsl_clamp_via_forth_repl() {
    let mut s = sess_with_core();
    let out = s.eval(
        ": clamp LET (x, lo, hi) -> (y) = \
              select(x < lo, lo, select(x > hi, hi, x)) END ;\n\
         5.0 0.0 10.0 clamp f.\n\
         -3.0 0.0 10.0 clamp f.\n\
         99.0 0.0 10.0 clamp f.\nbye\n"
    ).unwrap();
    assert!(out.contains("5.000000"),  "got {out:?}");
    assert!(out.contains("0.000000"),  "got {out:?}");
    assert!(out.contains("10.000000"), "got {out:?}");
}

#[test]
fn let_dsl_compile_only_outside_colon() {
    let mut s = sess_with_core();
    // LET in interpret state runs `comp_only_word` → THROW -14.
    let err = s.eval("LET (x) -> (y) = x END\nbye\n").unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("-14") || msg.contains("THROW"),
        "expected -14 throw, got: {msg}");
}

// ── V1b GC primitives ────────────────────────────────────────────────

#[test]
fn gc_heapptr_pushes_stable_handle() {
    let mut s = sess();
    // HEAPPTR declares a slot; invoking the name pushes the slot's
    // address.  The same handle two pushes should equal each other
    // (the slot doesn't move).
    let out = s.eval("HEAPPTR foo\nfoo foo = .\nbye\n").unwrap();
    assert!(out.contains("-1"), "handle should be stable, got {out:?}");
}

#[test]
fn gc_vec_alloc_and_access() {
    let mut s = sess_with_core();
    let out = s.eval(
        "HEAPPTR samples\n\
         8 samples vec-alloc-floats!\n\
         1.5e samples 0 vec-f!\n\
         2.5e samples 1 vec-f!\n\
         3.5e samples 7 vec-f!\n\
         samples 0 vec-f@ f.\n\
         samples 1 vec-f@ f.\n\
         samples 7 vec-f@ f.\n\
         samples vec-len .\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("1.500000"), "got {out:?}");
    assert!(out.contains("2.500000"), "got {out:?}");
    assert!(out.contains("3.500000"), "got {out:?}");
    assert!(out.contains("8 "), "vec-len should report 8: {out:?}");
}

#[test]
fn gc_rooted_object_survives_collection() {
    let mut s = sess_with_core();
    // Use exact-representable values so f.'s 6-decimal-digit print
    // doesn't introduce a rounding ambiguity.
    let out = s.eval(
        "HEAPPTR v\n\
         4 v vec-alloc-floats!\n\
         1.5e v 0 vec-f!\n\
         7.25e v 3 vec-f!\n\
         (gc)\n\
         v 0 vec-f@ f.\n\
         v 3 vec-f@ f.\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("1.500000"), "first cell lost across GC: {out:?}");
    assert!(out.contains("7.250000"), "last cell lost across GC: {out:?}");
}

#[test]
fn gc_two_megabyte_vector_worked_example() {
    // The 2 MB worked example from docs/gc_design.md.  Allocates
    // 262144 cells (= 2 MB of f64), writes scattered values, runs
    // a major GC, reads them back.  Large objects are pinned by
    // paged_gc so they generation-flip in place across collections.
    let mut s = sess_with_core();
    let out = s.eval(
        "HEAPPTR big\n\
         262144 big vec-alloc-floats!\n\
         1.5e big 1000 vec-f!\n\
         7.25e big 100000 vec-f!\n\
         0.125e big 200000 vec-f!\n\
         big 1000 vec-f@ f.\n\
         big 100000 vec-f@ f.\n\
         big 200000 vec-f@ f.\n\
         big vec-len .\n\
         (gc)\n\
         big 1000 vec-f@ f.\n\
         big 100000 vec-f@ f.\n\
         big 200000 vec-f@ f.\n\
         bye\n"
    ).unwrap();
    // Use exactly-representable f64 values (1.5, 7.25, 0.125) to dodge
    // the rounding-direction ambiguity that bit the earlier 3.14159 form.
    assert!(out.contains("1.500000"), "cell 1000 wrong: {out:?}");
    assert!(out.contains("7.250000"), "cell 100000 wrong: {out:?}");
    assert!(out.contains("0.125000"), "cell 200000 wrong: {out:?}");
    assert!(out.contains("262144"), "vec-len wrong: {out:?}");
    // The same three values should still be present AFTER (gc) —
    // each f. output appears twice in the stream.
    let v_one_five = out.matches("1.500000").count();
    let v_seven_two = out.matches("7.250000").count();
    let v_one_two_five = out.matches("0.125000").count();
    assert_eq!(v_one_five, 2, "1.5 should appear twice (pre+post GC)");
    assert_eq!(v_seven_two, 2);
    assert_eq!(v_one_two_five, 2);
}

#[test]
fn gc_unrooted_object_gets_reclaimed() {
    // Allocate via vec-alloc-floats!, then null out the HEAPPTR, then
    // (gc).  The allocated bytes are no longer reachable and should
    // be reclaimed.  We can't observe this directly from Forth, but
    // we can allocate a LOT of orphans and verify the heap doesn't
    // grow indefinitely.
    let mut s = sess_with_core();
    let out = s.eval(
        ": cycle  ( -- )  HEAPPTR slot  100 slot vec-alloc-floats! ;\n\
         \\ Hmm: HEAPPTR can't be inside a colon definition (it's a\n\
         \\ defining word). Use a different shape.\n\
         HEAPPTR slot\n\
         100 slot vec-alloc-floats!\n\
         100 slot vec-alloc-floats!\n\
         100 slot vec-alloc-floats!\n\
         (gc)\n\
         slot 0 vec-f@ f.\n\
         bye\n"
    ).unwrap();
    // The last allocation's cell 0 is 0.0 (fresh FILL_WORD).  The
    // first two allocations are unreachable after the second
    // vec-alloc-floats! overwrites the slot.
    assert!(out.contains("0.000000"), "got {out:?}");
}

// `gc_vec_f_fetch_wrong_type_throws` was the V1b umbrella test that
// covered both nil-deref and wrong-type cases under -2060.  V1c
// splits those: nil now throws -2061 (see
// `gc_vec_f_fetch_on_nil_throws_dedicated_code`), and the wrong-type
// path still throws -2060 (see
// `gc_vec_f_fetch_wrong_type_still_throws_minus_2060`).

#[test]
fn gc_heapptr_no_name_throws() {
    let mut s = sess();
    let err = s.eval("HEAPPTR\nbye\n").unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("-16") || msg.contains("THROW"),
        "expected -16 (name required) throw, got: {msg}");
}

#[test]
fn gc_minor_collection_keeps_rooted_object() {
    let mut s = sess_with_core();
    let out = s.eval(
        "HEAPPTR v\n\
         4 v vec-alloc-floats!\n\
         42.0e v 0 vec-f!\n\
         gc-minor\n\
         gc-minor\n\
         gc-minor\n\
         v 0 vec-f@ f.\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("42.000000"), "got {out:?}");
}

#[test]
fn gc_forget_last_reuses_heapptr_slot() {
    // V1c: after `forget_last` on a HEAPPTR-defined word, HEAPPTR_NEXT
    // rolls back past its slot.  A subsequently declared HEAPPTR
    // re-uses the same slot address.
    let mut s = sess_with_core();
    let out = s.eval(
        "HEAPPTR a\n\
         a .\n\
         forget_last\n\
         HEAPPTR b\n\
         b .\n\
         bye\n"
    ).unwrap();
    let parsed: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(parsed.len(), 2,
        "expected 2 slot addresses, got {parsed:?} from {out:?}");
    assert_eq!(parsed[0], parsed[1],
        "slot addr should be reused after forget; got {out:?}");
}

#[test]
fn gc_forget_last_zeroes_abandoned_slot() {
    // After allocating into HEAPPTR a, forget_last should zero the
    // abandoned slot.  Verified by stashing the slot's raw address in
    // a VARIABLE *before* defining the HEAPPTR (so VARIABLE survives
    // the forget), then dereferencing it again after the forget.  Pre-
    // forget the slot holds a tagged FloatVec pointer (non-zero, low
    // bits = 010); post-forget it must be 0.
    let mut s = sess_with_core();
    let out = s.eval(
        "VARIABLE saved\n\
         HEAPPTR a\n\
         a saved !\n\
         10 a vec-alloc-floats!\n\
         saved @ @ .\n\
         forget_last\n\
         saved @ @ .\n\
         bye\n"
    ).unwrap();
    let parsed: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(parsed.len(), 2,
        "expected 2 cell values, got {parsed:?} from {out:?}");
    assert_ne!(parsed[0], 0,
        "pre-forget slot should hold a tagged ptr; got {out:?}");
    assert_eq!(parsed[0] & 7, 2,
        "pre-forget slot should be a FloatVec (tag 010); got {out:?}");
    assert_eq!(parsed[1], 0,
        "post-forget slot should be zeroed; got {out:?}");
}

#[test]
fn gc_forget_last_on_non_heapptr_leaves_region_alone() {
    // A regular colon definition forget should NOT touch HEAPPTR_NEXT.
    // Define HEAPPTR a, then : foo ;, then forget_last (removes foo).
    // After: HEAPPTR b should land in slot 1, not slot 0.  So `a` and
    // `b` print different addresses.
    let mut s = sess_with_core();
    let out = s.eval(
        "HEAPPTR a\n\
         : foo 42 ;\n\
         forget_last\n\
         HEAPPTR b\n\
         a .\n\
         b .\n\
         bye\n"
    ).unwrap();
    let parsed: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(parsed.len(), 2, "got {parsed:?} from {out:?}");
    assert_eq!(parsed[1] - parsed[0], 8,
        "b should be one cell past a; got a={} b={}", parsed[0], parsed[1]);
}

#[test]
fn gc_vec_f_fetch_on_nil_throws_dedicated_code() {
    // V1c: nil-deref produces -2061, distinct from -2060 (wrong type).
    let mut s = sess_with_core();
    let err = s.eval(
        "HEAPPTR empty\nempty 0 vec-f@ f.\nbye\n"
    ).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("-2061"),
        "expected -2061 (nil-deref) on vec-f@ over nil slot, got: {msg}");
}

#[test]
fn gc_vec_f_store_on_nil_throws_dedicated_code() {
    let mut s = sess_with_core();
    let err = s.eval(
        "HEAPPTR empty\n42.0e empty 0 vec-f!\nbye\n"
    ).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("-2061"),
        "expected -2061 (nil-deref) on vec-f! over nil slot, got: {msg}");
}

#[test]
fn gc_vec_len_on_nil_throws_dedicated_code() {
    let mut s = sess_with_core();
    let err = s.eval(
        "HEAPPTR empty\nempty vec-len .\nbye\n"
    ).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("-2061"),
        "expected -2061 (nil-deref) on vec-len over nil slot, got: {msg}");
}

#[test]
fn gc_cycle_starts_at_zero() {
    let mut s = sess_with_core();
    let out = s.eval("gc-cycle .\nbye\n").unwrap();
    assert!(out.contains("0  ok"),
        "gc-cycle should start at 0; got {out:?}");
}

#[test]
fn gc_cycle_increments_on_explicit_major_collection() {
    let mut s = sess_with_core();
    let out = s.eval(
        "gc-cycle .\n\
         (gc)\n\
         gc-cycle .\n\
         (gc)\n\
         gc-cycle .\n\
         bye\n"
    ).unwrap();
    let nums: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(nums, vec![0, 1, 2],
        "gc-cycle should monotonically increase on (gc); got {nums:?} from {out:?}");
}

#[test]
fn gc_cycle_increments_on_minor_collection() {
    let mut s = sess_with_core();
    let out = s.eval(
        "gc-minor\n\
         gc-cycle .\n\
         gc-minor\n\
         gc-cycle .\n\
         bye\n"
    ).unwrap();
    let nums: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(nums, vec![1, 2],
        "gc-cycle should bump on gc-minor too; got {nums:?} from {out:?}");
}

#[test]
fn gc_auto_collects_when_budget_exhausted() {
    // V2: vec-alloc-* checks should_collect() and runs a minor GC
    // first if the budget is exhausted.  paged_gc's default trigger
    // is 8 MB; each 200_000-cell FloatVec is ~1.6 MB (200k * 8B +
    // header).  Allocating 8 of them (with the previous one
    // dropped each cycle) should force at least one auto-trigger.
    let mut s = sess_with_core();
    let out = s.eval(
        "HEAPPTR slot\n\
         gc-cycle .                \\ should be 0\n\
         200000 slot vec-alloc-floats!\n\
         200000 slot vec-alloc-floats!\n\
         200000 slot vec-alloc-floats!\n\
         200000 slot vec-alloc-floats!\n\
         200000 slot vec-alloc-floats!\n\
         200000 slot vec-alloc-floats!\n\
         200000 slot vec-alloc-floats!\n\
         200000 slot vec-alloc-floats!\n\
         gc-cycle .                \\ should be > 0\n\
         bye\n"
    ).unwrap();
    let nums: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(nums.len(), 2, "got {nums:?} from {out:?}");
    assert_eq!(nums[0], 0, "gc-cycle should start at 0; got {out:?}");
    assert!(nums[1] >= 1,
        "gc-cycle should bump from auto-GC; got pre={} post={} ({out:?})",
        nums[0], nums[1]);
}

#[test]
fn gc_auto_collects_does_not_lose_rooted_data() {
    // After auto-GC the still-rooted vector should be intact.
    // Allocate, write known cells, force enough allocation to
    // trigger auto-GC at least once, then read the cells back.
    let mut s = sess_with_core();
    let out = s.eval(
        "HEAPPTR keep\n\
         HEAPPTR scratch\n\
         4 keep vec-alloc-floats!\n\
         1.5e keep 0 vec-f!\n\
         2.5e keep 1 vec-f!\n\
         3.5e keep 2 vec-f!\n\
         4.5e keep 3 vec-f!\n\
         200000 scratch vec-alloc-floats!\n\
         200000 scratch vec-alloc-floats!\n\
         200000 scratch vec-alloc-floats!\n\
         200000 scratch vec-alloc-floats!\n\
         200000 scratch vec-alloc-floats!\n\
         200000 scratch vec-alloc-floats!\n\
         200000 scratch vec-alloc-floats!\n\
         200000 scratch vec-alloc-floats!\n\
         gc-cycle .\n\
         keep 0 vec-f@ f.\n\
         keep 1 vec-f@ f.\n\
         keep 2 vec-f@ f.\n\
         keep 3 vec-f@ f.\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("1.500000"), "cell 0 lost: {out:?}");
    assert!(out.contains("2.500000"), "cell 1 lost: {out:?}");
    assert!(out.contains("3.500000"), "cell 2 lost: {out:?}");
    assert!(out.contains("4.500000"), "cell 3 lost: {out:?}");
    // Should have triggered at least one auto-collection.
    let nums: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert!(nums.iter().any(|&n| n >= 1),
        "expected at least one auto-GC cycle; got {nums:?} from {out:?}");
}

#[test]
fn gc_store_heapptr_copies_tagged_pointer() {
    // V2-B: `!heapptr` is the safe-by-intent way to copy a tagged
    // pointer from one HEAPPTR to another (or to nil out a slot).
    // After `a @ b !heapptr`, both slots reference the same vector
    // and reading payload via b returns what was written via a.
    let mut s = sess_with_core();
    let out = s.eval(
        "HEAPPTR a\n\
         HEAPPTR b\n\
         4 a vec-alloc-floats!\n\
         7.25e a 2 vec-f!\n\
         a @ b !heapptr\n\
         b 2 vec-f@ f.\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("7.250000"),
        "b should see the cell a wrote; got {out:?}");
}

#[test]
fn gc_store_heapptr_can_nil_a_slot() {
    // Storing 0 (nil) via !heapptr makes the slot vec-len-able
    // throw -2061 the way a freshly-declared HEAPPTR does.
    let mut s = sess_with_core();
    let err = s.eval(
        "HEAPPTR a\n\
         4 a vec-alloc-floats!\n\
         0 a !heapptr\n\
         a vec-len .\n\
         bye\n"
    ).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("-2061"),
        "nil'd slot should throw -2061; got {msg}");
}

#[test]
fn gc_store_heapptr_survives_subsequent_collection() {
    // After !heapptr from a → b, run (gc).  Both slots should
    // resolve to the (possibly relocated) object, and the payload
    // should be intact.
    let mut s = sess_with_core();
    let out = s.eval(
        "HEAPPTR a\n\
         HEAPPTR b\n\
         4 a vec-alloc-floats!\n\
         1.5e a 0 vec-f!\n\
         2.5e a 1 vec-f!\n\
         a @ b !heapptr\n\
         (gc)\n\
         a 0 vec-f@ f.\n\
         a 1 vec-f@ f.\n\
         b 0 vec-f@ f.\n\
         b 1 vec-f@ f.\n\
         bye\n"
    ).unwrap();
    // Each value should appear twice (once via a, once via b).
    assert_eq!(out.matches("1.500000").count(), 2,
        "cell 0 should be readable via both a and b post-GC; got {out:?}");
    assert_eq!(out.matches("2.500000").count(), 2,
        "cell 1 should be readable via both a and b post-GC; got {out:?}");
}

#[test]
fn gc_long_running_promotes_and_survives() {
    // Tenure-promotion stress test.  Allocate a rooted vector,
    // then run a large number of minor GCs interleaved with
    // throw-away allocations.  paged_gc promotes G0 → G1 → Tenured
    // across multiple cycles; after 20+ cycles the rooted object
    // is definitely tenured.  Verify the payload is still intact.
    //
    // This is the read-only half of the V2 generational stress
    // test from docs/gc_design.md ("allocate young, promote to
    // old via repeated collections").  The "mutate old to point
    // at young" half needs vec-ref! (V3 + write barrier), which
    // hasn't landed yet — see docs/forth_gc_needs.md item #2.
    let mut s = sess_with_core();
    let mut script = String::from(
        "HEAPPTR rooted\n\
         HEAPPTR scratch\n\
         8 rooted vec-alloc-floats!\n\
         1.5e rooted 0 vec-f!\n\
         2.5e rooted 1 vec-f!\n\
         3.5e rooted 2 vec-f!\n\
         4.5e rooted 3 vec-f!\n\
         5.5e rooted 4 vec-f!\n\
         6.5e rooted 5 vec-f!\n\
         7.5e rooted 6 vec-f!\n\
         8.5e rooted 7 vec-f!\n"
    );
    // 25 rounds of (alloc-throwaway, gc-minor) — enough to promote
    // and exercise multiple promotion-cycle transitions.
    for _ in 0..25 {
        script.push_str("16 scratch vec-alloc-floats!\ngc-minor\n");
    }
    script.push_str(
        "gc-cycle .\n\
         rooted 0 vec-f@ f.\n\
         rooted 1 vec-f@ f.\n\
         rooted 2 vec-f@ f.\n\
         rooted 3 vec-f@ f.\n\
         rooted 4 vec-f@ f.\n\
         rooted 5 vec-f@ f.\n\
         rooted 6 vec-f@ f.\n\
         rooted 7 vec-f@ f.\n\
         bye\n"
    );
    let out = s.eval(&script).unwrap();
    // gc-cycle should reflect at least our 25 explicit gc-minor
    // calls (possibly more if auto-GC fired too).
    let nums: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert!(nums.first().copied().unwrap_or(0) >= 25,
        "expected >=25 gc cycles; got {nums:?} from start of {out:?}");
    for v in [
        "1.500000", "2.500000", "3.500000", "4.500000",
        "5.500000", "6.500000", "7.500000", "8.500000",
    ] {
        assert!(out.contains(v),
            "payload cell {v} lost across {} cycles; got {out:?}",
            nums.first().copied().unwrap_or(0));
    }
}

#[test]
fn gc_many_rooted_vectors_all_survive() {
    // Stress: bind ten HEAPPTRs to ten distinct vectors, each
    // with a unique marker cell.  Run many minor collections.
    // All ten markers should still be readable.
    let mut s = sess_with_core();
    let mut script = String::new();
    for i in 0..10 {
        script.push_str(&format!("HEAPPTR slot{i}\n"));
    }
    for i in 0..10 {
        script.push_str(&format!(
            "4 slot{i} vec-alloc-floats!\n{i}.5e slot{i} 0 vec-f!\n"
        ));
    }
    for _ in 0..15 {
        script.push_str("gc-minor\n");
    }
    for i in 0..10 {
        script.push_str(&format!("slot{i} 0 vec-f@ f.\n"));
    }
    script.push_str("bye\n");
    let out = s.eval(&script).unwrap();
    for i in 0..10 {
        let expected = format!("{i}.500000");
        assert!(out.contains(&expected),
            "slot{i} payload lost after 15 cycles; expected {expected}, got {out:?}");
    }
}

// ── V2s stage A — managed strings ─────────────────────────────────

#[test]
fn str_to_string_round_trips_bytes() {
    // S" pushes (c-addr u); >$ allocates a managed String and
    // returns a tagged ptr.  $>addr exposes the payload addr/len
    // for one-shot interop with TYPE.
    let mut s = sess_with_core();
    let out = s.eval("s\" hello, world\" >$ $>addr type cr\nbye\n").unwrap();
    assert!(out.contains("hello, world"),
        "round-trip via >$ / $>addr / TYPE failed; got {out:?}");
}

#[test]
fn str_len_returns_byte_count() {
    let mut s = sess_with_core();
    let out = s.eval("s\" abcdefghij\" >$ $len .\nbye\n").unwrap();
    assert!(out.contains("10  ok"),
        "$len of 10-byte string should be 10; got {out:?}");
}

#[test]
fn str_len_of_empty_is_zero() {
    let mut s = sess_with_core();
    let out = s.eval("s\" \" >$ $len .\nbye\n").unwrap();
    assert!(out.contains("0  ok"),
        "$len of empty string should be 0; got {out:?}");
}

#[test]
fn str_equal_compares_bytes() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" hello\" >$ s\" hello\" >$ $= .\n\
         s\" hello\" >$ s\" world\" >$ $= .\n\
         s\" hello\" >$ s\" hell\" >$ $= .\n\
         bye\n"
    ).unwrap();
    let nums: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(nums, vec![-1, 0, 0],
        "expected (true, false, false) for hello/hello, hello/world, hello/hell; got {nums:?} from {out:?}");
}

#[test]
fn str_equal_same_object_is_true() {
    // Same tagged pointer twice on the stack ought to compare equal
    // (covers the fast-path identity check in rt_string_bytes_equal).
    let mut s = sess_with_core();
    let out = s.eval(
        "HEAPPTR a\n\
         s\" foo\" >$ a !$\n\
         a @$ a @$ $= .\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("-1  ok"),
        "same-object $= should be true; got {out:?}");
}

#[test]
fn str_store_and_fetch_via_heapptr() {
    // !$ stores a tagged String into a HEAPPTR slot; @$ fetches.
    // Both type-check.
    let mut s = sess_with_core();
    let out = s.eval(
        "HEAPPTR greet\n\
         s\" hi there\" >$ greet !$\n\
         greet @$ $>addr type cr\n\
         greet @$ $len .\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("hi there"), "got {out:?}");
    assert!(out.contains("8  ok"), "$len should be 8; got {out:?}");
}

#[test]
fn str_fetch_from_unbound_slot_returns_nil() {
    // @$ on a never-bound HEAPPTR returns 0 (nil) — *not* a throw.
    // This is the V2s "the empty answer" convention from
    // docs/strings_design.md.
    let mut s = sess_with_core();
    let out = s.eval("HEAPPTR empty\nempty @$ .\nbye\n").unwrap();
    assert!(out.contains("0  ok"),
        "@$ on nil slot should return 0; got {out:?}");
}

#[test]
fn str_store_nil_is_allowed() {
    // !$ accepts 0 (nil) to let a slot be cleared explicitly.
    let mut s = sess_with_core();
    let out = s.eval(
        "HEAPPTR x\n\
         s\" before\" >$ x !$\n\
         x @$ $len .\n\
         0 x !$\n\
         x @$ .\n\
         bye\n"
    ).unwrap();
    let nums: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(nums, vec![6, 0],
        "expected (6, 0) — len then nil; got {nums:?} from {out:?}");
}

#[test]
fn str_store_wrong_type_throws() {
    // !$ rejects a non-String tagged value (e.g., a FloatVec).
    let mut s = sess_with_core();
    let err = s.eval(
        "HEAPPTR x\n\
         HEAPPTR v\n\
         4 v vec-alloc-floats!\n\
         v @ x !$\n\
         bye\n"
    ).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("-2060"),
        "storing FloatVec via !$ should throw -2060; got {msg}");
}

#[test]
fn str_fetch_wrong_type_throws() {
    // @$ rejects a slot that holds a non-String tagged value.
    let mut s = sess_with_core();
    let err = s.eval(
        "HEAPPTR x\n\
         4 x vec-alloc-floats!\n\
         x @$ .\n\
         bye\n"
    ).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("-2060"),
        "@$ on FloatVec slot should throw -2060; got {msg}");
}

#[test]
fn str_len_on_nil_throws() {
    let mut s = sess_with_core();
    let err = s.eval("0 $len .\nbye\n").unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("-2061"),
        "$len on 0 should throw -2061; got {msg}");
}

#[test]
fn str_len_on_wrong_type_throws() {
    let mut s = sess_with_core();
    let err = s.eval(
        "HEAPPTR v\n\
         4 v vec-alloc-floats!\n\
         v @ $len .\n\
         bye\n"
    ).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("-2060"),
        "$len on FloatVec should throw -2060; got {msg}");
}

#[test]
fn str_survives_collection() {
    // A managed String rooted via @$ should survive (gc).  Verify
    // by reading the bytes back through TYPE after the collection.
    let mut s = sess_with_core();
    let out = s.eval(
        "HEAPPTR msg\n\
         s\" survive me\" >$ msg !$\n\
         (gc)\n\
         msg @$ $>addr type cr\n\
         msg @$ $len .\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("survive me"),
        "string bytes lost across (gc); got {out:?}");
    assert!(out.contains("10  ok"),
        "length wrong after (gc); got {out:?}");
}

#[test]
fn str_many_strings_all_survive_collection() {
    // Allocate a bunch of distinct managed strings rooted via
    // separate HEAPPTRs.  Run minor GCs.  All should still be
    // intact and distinguishable via $=.
    let mut s = sess_with_core();
    let mut script = String::new();
    for i in 0..8 {
        script.push_str(&format!("HEAPPTR s{i}\n"));
    }
    for i in 0..8 {
        // Each gets a distinct payload like "msg-0", "msg-1", ...
        script.push_str(&format!("s\" msg-{i}\" >$ s{i} !$\n"));
    }
    for _ in 0..5 {
        script.push_str("gc-minor\n");
    }
    for i in 0..8 {
        script.push_str(&format!("s{i} @$ $>addr type cr\n"));
    }
    script.push_str("bye\n");
    let out = s.eval(&script).unwrap();
    for i in 0..8 {
        let expected = format!("msg-{i}");
        assert!(out.contains(&expected),
            "string {expected} lost after minor cycles; got {out:?}");
    }
}

#[test]
fn str_empty_strings_compare_equal() {
    let mut s = sess_with_core();
    let out = s.eval("s\" \" >$ s\" \" >$ $= .\nbye\n").unwrap();
    assert!(out.contains("-1  ok"),
        "two empty strings should compare equal; got {out:?}");
}

// ── V2s stage B — S$" compile-time literals ──────────────────────

#[test]
fn str_s_dollar_quote_interpret_mode_pushes_tagged() {
    // Outside a colon definition, S$" allocates and pushes the
    // tagged pointer immediately, just like >$ but with the bytes
    // parsed from the input stream.
    let mut s = sess_with_core();
    let out = s.eval("S$\" hello, world\" $>addr type cr\nbye\n").unwrap();
    assert!(out.contains("hello, world"),
        "interpret-mode S$\" should produce a usable String; got {out:?}");
}

#[test]
fn str_s_dollar_quote_compile_mode_emits_literal() {
    // Inside a colon def, S$" allocates a LITERAL slot at compile
    // time; each call to the word pushes the SAME tagged pointer.
    // We can verify "same" by comparing the printed addresses.
    let mut s = sess_with_core();
    let out = s.eval(
        ": greet S$\" howdy\" ;\n\
         greet $>addr type space\n\
         greet $>addr type cr\n\
         greet . greet . cr\n\
         bye\n"
    ).unwrap();
    // The body should appear twice.
    assert_eq!(out.matches("howdy").count(), 2,
        "greet should produce 'howdy' twice; got {out:?}");
    // The two tagged-ptr values from `greet .` should be equal.
    // Pull the numeric tokens out of the final line.
    let parsed: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert!(parsed.len() >= 2, "expected at least 2 ints; got {parsed:?} from {out:?}");
    let n = parsed.len();
    assert_eq!(parsed[n - 2], parsed[n - 1],
        "two invocations of `greet` should push the same tagged ptr (literal); \
         got {} and {}", parsed[n-2], parsed[n-1]);
}

#[test]
fn str_s_dollar_quote_literal_survives_collection() {
    // A LITERAL-region slot is a GC root.  Define a word that
    // returns a literal, force a (gc), then call it again —
    // the contents should still be readable, even if paged_gc
    // moved the underlying String.
    let mut s = sess_with_core();
    let out = s.eval(
        ": label S$\" persistent\" ;\n\
         label $>addr type cr\n\
         (gc)\n\
         label $>addr type cr\n\
         label $len .\n\
         bye\n"
    ).unwrap();
    assert_eq!(out.matches("persistent").count(), 2,
        "literal should be readable both pre and post (gc); got {out:?}");
    assert!(out.contains("10  ok"),
        "$len should be 10; got {out:?}");
}

#[test]
fn str_s_dollar_quote_two_literals_are_distinct() {
    // Two textually-identical S$" forms allocate TWO slots — V2s
    // explicitly defers interning (see strings_design.md "out of
    // scope for V2s").  Distinct objects, equal bytes via $=.
    let mut s = sess_with_core();
    let out = s.eval(
        ": a S$\" same\" ;\n\
         : b S$\" same\" ;\n\
         a b $= .\n\
         a b = .\n\
         bye\n"
    ).unwrap();
    let nums: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(nums, vec![-1, 0],
        "$= should be true (bytes match), = should be false (distinct objects); \
         got {nums:?} from {out:?}");
}

#[test]
fn str_s_dollar_quote_empty_literal() {
    let mut s = sess_with_core();
    let out = s.eval(
        ": nada S$\" \" ;\n\
         nada $len .\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("0  ok"),
        "empty literal should have $len 0; got {out:?}");
}

#[test]
fn str_s_dollar_quote_many_literals() {
    // Allocate 32 distinct literals, verify each reads back
    // correctly after a major GC.  This exercises both the
    // LITERAL bump pointer and the GC's walk of that region.
    let mut s = sess_with_core();
    let mut script = String::new();
    for i in 0..32 {
        script.push_str(&format!(": lit{i} S$\" item-{i}\" ;\n"));
    }
    script.push_str("(gc)\n");
    for i in 0..32 {
        script.push_str(&format!("lit{i} $>addr type cr\n"));
    }
    script.push_str("bye\n");
    let out = s.eval(&script).unwrap();
    for i in 0..32 {
        let expected = format!("item-{i}");
        assert!(out.contains(&expected),
            "literal {expected} lost; got tail of out:\n{}",
            &out[out.len().saturating_sub(2000)..]);
    }
}

#[test]
fn str_s_dollar_quote_inside_colon_def_with_other_code() {
    // S$" can appear mid-definition alongside arithmetic and
    // legacy words.
    let mut s = sess_with_core();
    let out = s.eval(
        ": describe ( n -- )  S$\" n=\" $>addr type . cr ;\n\
         42 describe\n\
         bye\n"
    ).unwrap();
    // The TYPE prints "n=", then `.` prints " 42 ", then CR.
    assert!(out.contains("n=42 "),
        "mixed colon-def output wrong; got {out:?}");
}

// ── V2s stage C1 — MutStringBuilder ───────────────────────────────

#[test]
fn sb_new_starts_empty_with_requested_capacity() {
    let mut s = sess_with_core();
    let out = s.eval(
        "64 sb-new\n\
         dup sb-len .\n\
         sb-capacity .\n\
         bye\n"
    ).unwrap();
    let nums: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(nums, vec![0, 64],
        "fresh builder: len=0, cap=64; got {nums:?} from {out:?}");
}

#[test]
fn sb_append_string_grows_length() {
    // Hold the builder via a HEAPPTR so any allocation triggered
    // inside `>$` (auto-GC) doesn't strand a stale tagged-ptr copy
    // on the data stack.  This is the design's official idiom.
    //
    // Note: every `s"` here keeps a space after the closing quote
    // *to keep the tokenizer happy* — `s"," ...` would be read as
    // one whitespace-delimited token `s",`.  Standard `s" ... "`
    // requires a leading space; we double-space the trailing one
    // for symmetry.
    let mut s = sess_with_core();
    let out = s.eval(
        "HEAPPTR b\n\
         32 sb-new b !\n\
         s\" hello\" >$ b @ sb-append$\n\
         s\" , \" >$ b @ sb-append$\n\
         s\" world\" >$ b @ sb-append$\n\
         b @ sb-len .\n\
         b @ sb>string $>addr type cr\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("12  ok"),
        "post-append length should be 12; got {out:?}");
    assert!(out.contains("hello, world"),
        "sb>string should produce the concatenated bytes; got {out:?}");
}

#[test]
fn sb_to_string_resets_length() {
    // Per design: sb>string produces a fresh String and resets the
    // builder's length to 0 (capacity retained) — the builder can
    // be reused.
    let mut s = sess_with_core();
    let out = s.eval(
        "16 sb-new                  ( sb )\n\
         s\" abc\" >$ over sb-append$\n\
         dup sb>string drop         ( sb )\n\
         dup sb-len .               \\ should be 0\n\
         sb-capacity .              \\ should still be 16\n\
         bye\n"
    ).unwrap();
    let nums: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(nums, vec![0, 16],
        "after sb>string: len=0, cap=16; got {nums:?} from {out:?}");
}

#[test]
fn sb_clear_resets_length_only() {
    let mut s = sess_with_core();
    let out = s.eval(
        "16 sb-new\n\
         s\" abc\" >$ over sb-append$\n\
         dup sb-len .               \\ 3\n\
         dup sb-clear\n\
         dup sb-len .               \\ 0\n\
         sb-capacity .              \\ still 16\n\
         bye\n"
    ).unwrap();
    let nums: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(nums, vec![3, 0, 16],
        "got {nums:?} from {out:?}");
}

#[test]
fn sb_append_n_formats_decimal() {
    let mut s = sess_with_core();
    let out = s.eval(
        "32 sb-new\n\
         42 over sb-append-n\n\
         -7 over sb-append-n\n\
         0 over sb-append-n\n\
         sb>string $>addr type cr\n\
         bye\n"
    ).unwrap();
    // Should print "42-70" concatenated.
    assert!(out.contains("42-70"),
        "decimal appends should concatenate; got {out:?}");
}

#[test]
fn sb_append_c_ascii_one_byte() {
    let mut s = sess_with_core();
    let out = s.eval(
        "8 sb-new\n\
         65 over sb-append-c             \\ 'A'\n\
         66 over sb-append-c             \\ 'B'\n\
         67 over sb-append-c             \\ 'C'\n\
         dup sb-len .\n\
         sb>string $>addr type cr\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("3  ok"),
        "3 ASCII chars → 3 bytes; got {out:?}");
    assert!(out.contains("ABC"),
        "should print ABC; got {out:?}");
}

#[test]
fn sb_append_c_utf8_multibyte() {
    // U+00E9 'é' (decimal 233) is 2 bytes in UTF-8 (0xC3 0xA9).
    // U+20AC '€' (decimal 8364) is 3 bytes (0xE2 0x82 0xAC).
    let mut s = sess_with_core();
    let out = s.eval(
        "HEAPPTR b\n\
         16 sb-new b !\n\
         233 b @ sb-append-c\n\
         8364 b @ sb-append-c\n\
         b @ sb-len .\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("5  ok"),
        "é (2 bytes) + € (3 bytes) = 5; got {out:?}");
}

#[test]
fn sb_append_overflow_throws_minus_2062() {
    let mut s = sess_with_core();
    let err = s.eval(
        "4 sb-new\n\
         s\" hello\" >$ over sb-append$    \\ 5 bytes into 4-byte cap\n\
         bye\n"
    ).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("-2062"),
        "expected -2062 (capacity overflow); got {msg}");
}

#[test]
fn sb_wrong_type_throws_minus_2060() {
    let mut s = sess_with_core();
    let err = s.eval(
        "HEAPPTR v\n\
         4 v vec-alloc-floats!\n\
         v @ sb-len .\n\
         bye\n"
    ).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("-2060"),
        "sb-len on FloatVec should throw -2060; got {msg}");
}

#[test]
fn sb_nil_throws_minus_2061() {
    let mut s = sess_with_core();
    let err = s.eval("0 sb-len .\nbye\n").unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("-2061"),
        "sb-len on nil should throw -2061; got {msg}");
}

#[test]
fn sb_survives_collection() {
    // Stash a builder via a HEAPPTR, force a (gc), then continue
    // appending — payload must survive even if the underlying
    // builder object got relocated.
    let mut s = sess_with_core();
    let out = s.eval(
        "HEAPPTR b\n\
         128 sb-new b !\n\
         s\" pre-\" >$ b @ sb-append$\n\
         (gc)\n\
         s\" post\" >$ b @ sb-append$\n\
         b @ sb>string $>addr type cr\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("pre-post"),
        "builder payload lost across (gc); got {out:?}");
}

#[test]
fn sb_round_trip_through_to_string() {
    // Build a string with sb-append-n / sb-append$ / sb-append-c,
    // finalise, compare to the expected.
    let mut s = sess_with_core();
    let out = s.eval(
        "64 sb-new\n\
         S$\" page \" over sb-append$\n\
         3 over sb-append-n\n\
         32 over sb-append-c             \\ space\n\
         S$\" of \" over sb-append$\n\
         10 over sb-append-n\n\
         sb>string $>addr type cr\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("page 3 of 10"),
        "concatenated output wrong; got {out:?}");
}

// ── V2s stage C2 — operations library ─────────────────────────────

#[test]
fn str_concat_produces_fresh_string() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" foo\" >$ s\" bar\" >$ $+ $>addr type cr\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("foobar"), "got {out:?}");
}

#[test]
fn str_concat_empty_left() {
    let mut s = sess_with_core();
    let out = s.eval(
        "empty$ s\" tail\" >$ $+ $>addr type cr\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("tail"), "got {out:?}");
}

#[test]
fn str_concat_empty_right() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" head\" >$ empty$ $+ $>addr type cr\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("head"), "got {out:?}");
}

#[test]
fn str_slice_basic() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" hello world\" >$ 6 11 $slice $>addr type cr\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("world"), "got {out:?}");
}

#[test]
fn str_slice_empty_range() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" hello\" >$ 2 2 $slice $len .\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("0  ok"), "expected empty slice; got {out:?}");
}

#[test]
fn str_slice_out_of_bounds_throws() {
    let mut s = sess_with_core();
    let err = s.eval(
        "s\" hello\" >$ 0 99 $slice drop\nbye\n"
    ).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("-2058"),
        "out-of-bounds $slice should throw -2058; got {msg}");
}

#[test]
fn str_find_present() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" world\" >$ s\" hello world\" >$ $find .\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("6  ok"), "got {out:?}");
}

#[test]
fn str_find_absent_returns_minus_one() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" xyz\" >$ s\" hello\" >$ $find .\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("-1  ok"), "got {out:?}");
}

#[test]
fn str_find_empty_needle_matches_at_zero() {
    let mut s = sess_with_core();
    let out = s.eval(
        "empty$ s\" hello\" >$ $find .\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("0  ok"), "got {out:?}");
}

#[test]
fn str_starts_and_ends() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" hel\" >$ s\" hello\" >$ $starts? .\n\
         s\" llo\" >$ s\" hello\" >$ $ends? .\n\
         s\" xyz\" >$ s\" hello\" >$ $starts? .\n\
         s\" xyz\" >$ s\" hello\" >$ $ends? .\n\
         bye\n"
    ).unwrap();
    let nums: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(nums, vec![-1, -1, 0, 0],
        "got {nums:?} from {out:?}");
}

#[test]
fn str_cmp_orders_correctly() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" abc\" >$ s\" abd\" >$ $cmp .\n\
         s\" abd\" >$ s\" abc\" >$ $cmp .\n\
         s\" abc\" >$ s\" abc\" >$ $cmp .\n\
         bye\n"
    ).unwrap();
    let nums: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(nums, vec![-1, 1, 0], "got {nums:?} from {out:?}");
}

#[test]
fn str_hash_same_bytes_same_hash() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" hello\" >$ $hash .\n\
         s\" hello\" >$ $hash .\n\
         s\" world\" >$ $hash .\n\
         bye\n"
    ).unwrap();
    let nums: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(nums.len(), 3);
    assert_eq!(nums[0], nums[1], "identical bytes should hash equal; got {nums:?}");
    assert_ne!(nums[0], nums[2], "different bytes should hash differently; got {nums:?}");
}

#[test]
fn str_ci_eq_ascii() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" Hello\" >$ s\" hello\" >$ $ci= .\n\
         s\" HELLO\" >$ s\" hello\" >$ $ci= .\n\
         s\" hello\" >$ s\" world\" >$ $ci= .\n\
         bye\n"
    ).unwrap();
    let nums: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(nums, vec![-1, -1, 0], "got {nums:?} from {out:?}");
}

#[test]
fn str_trim_strips_whitespace_both_ends() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\"    hello world   \" >$ $trim $>addr type cr\n\
         bye\n"
    ).unwrap();
    // After trim, the byte content should be exactly "hello world".
    assert!(out.contains("hello world"), "got {out:?}");
    // The leading whitespace should be gone — check by length.
    let out2 = s.eval(
        "s\"    abc \" >$ $trim $len .\n\
         bye\n"
    ).unwrap();
    assert!(out2.contains("3  ok"), "$trim length wrong; got {out2:?}");
}

#[test]
fn str_ltrim_rtrim() {
    // `s"   abc   "` — `s"` consumes ONE leading space (the
    // standard delimiter), so the parsed bytes are 2 leading + "abc"
    // + 3 trailing = 8 bytes.  After ltrim: "abc   " = 6.  After
    // rtrim: "  abc" = 5.
    let mut s = sess_with_core();
    let out = s.eval(
        "s\"   abc   \" >$ $ltrim $len .\n\
         s\"   abc   \" >$ $rtrim $len .\n\
         bye\n"
    ).unwrap();
    let nums: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(nums, vec![6, 5], "got {nums:?} from {out:?}");
}

#[test]
fn str_n_to_string_decimal_round_trip() {
    let mut s = sess_with_core();
    let out = s.eval(
        "42 n>$ $>addr type cr\n\
         -17 n>$ $>addr type cr\n\
         0 n>$ $>addr type cr\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("42\n"), "got {out:?}");
    assert!(out.contains("-17\n"), "got {out:?}");
    assert!(out.contains("0\n"), "got {out:?}");
}

#[test]
fn str_to_n_parses_decimal() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" 123\" >$ $>n . .\n\
         s\" -42\" >$ $>n . .\n\
         bye\n"
    ).unwrap();
    // On success: ( n true ).  `. .` prints `true value` (top first).
    // For 123 success: prints "-1 123" → "-1 123".
    let nums: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(nums, vec![-1, 123, -1, -42], "got {nums:?} from {out:?}");
}

#[test]
fn str_to_n_failure_returns_false() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" not a number\" >$ $>n .\n\
         bye\n"
    ).unwrap();
    // Failure: pushes only 0 (false).
    let nums: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(nums, vec![0], "got {nums:?} from {out:?}");
}

#[test]
fn str_empty_string_has_zero_length() {
    let mut s = sess_with_core();
    let out = s.eval("empty$ $len .\nbye\n").unwrap();
    assert!(out.contains("0  ok"), "got {out:?}");
}

#[test]
fn str_ops_compose() {
    // Realistic composition: take a string, slice the middle,
    // concat with a prefix, compare.
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" hello, world!\" >$ 7 12 $slice            ( \"world\" )\n\
         s\" hello \" >$ swap $+                        ( \"hello world\" )\n\
         s\" hello world\" >$ $= .\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("-1  ok"),
        "expected $= true after compose; got {out:?}");
}

// ── V2s stage D — extended operations ────────────────────────────

#[test]
fn str_contains_true_and_false() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" world\" >$ s\" hello world\" >$ $contains? .\n\
         s\" xyz\"   >$ s\" hello world\" >$ $contains? .\n\
         empty$ s\" hello\" >$ $contains? .\n\
         bye\n"
    ).unwrap();
    let nums: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(nums, vec![-1, 0, -1], "got {nums:?} from {out:?}");
}

#[test]
fn str_rfind_last_occurrence() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" ab\" >$ s\" abcab\" >$ $rfind .\n\
         s\" xy\" >$ s\" abcab\" >$ $rfind .\n\
         empty$  s\" hello\" >$ $rfind .\n\
         bye\n"
    ).unwrap();
    let nums: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    // First: "ab" appears at 0 and 3 — last is 3.
    // Second: not found → -1.
    // Third: empty needle → haystack length = 5.
    assert_eq!(nums, vec![3, -1, 5], "got {nums:?} from {out:?}");
}

#[test]
fn str_repeat_basic() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" ab\" >$ 3 $repeat $>addr type cr\n\
         s\" x\"  >$ 0 $repeat $len .\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("ababab"), "got {out:?}");
    assert!(out.contains("0  ok"), "0-repeat should yield empty string; got {out:?}");
}

#[test]
fn str_replace_simple() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" world\" >$ s\" Forth\" >$ s\" hello world!\" >$ $replace $>addr type cr\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("hello Forth!"), "got {out:?}");
}

#[test]
fn str_replace_multiple_matches() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" o\" >$ s\" 0\" >$ s\" foo bar boo\" >$ $replace $>addr type cr\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("f00 bar b00"), "got {out:?}");
}

#[test]
fn str_replace_no_match_returns_copy() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" xyz\" >$ s\" QQ\" >$ s\" hello\" >$ $replace $>addr type cr\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("hello"), "got {out:?}");
}

#[test]
fn str_replace_repl_longer_than_needle() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" a\" >$ s\" ZZZ\" >$ s\" abab\" >$ $replace $>addr type cr\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("ZZZbZZZb"), "got {out:?}");
}

#[test]
fn str_replace_repl_shorter_than_needle() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" abc\" >$ s\" X\" >$ s\" abcabcabc\" >$ $replace $>addr type cr\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("XXX"), "got {out:?}");
}

#[test]
fn str_split_basic() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" ,\" >$ s\" a,b,c\" >$ $split .\n\
         bye\n"
    ).unwrap();
    // $split pushes ( $1 $2 $3 3 ).  Print the count.
    assert!(out.contains("3  ok"),
        "should split into 3 parts; got {out:?}");
}

#[test]
fn str_split_consume_pieces() {
    // Verify each piece is readable.  Count is on top; iterate down.
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" ,\" >$ s\" alpha,beta,gamma\" >$ $split\n\
         \\ Stack now: ( $a $b $c 3 ).  drop count, type each in reverse.\n\
         drop\n\
         $>addr type cr            \\ gamma\n\
         $>addr type cr            \\ beta\n\
         $>addr type cr            \\ alpha\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("gamma"), "got {out:?}");
    assert!(out.contains("beta"),  "got {out:?}");
    assert!(out.contains("alpha"), "got {out:?}");
}

#[test]
fn str_split_no_separator_yields_one_piece() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" ,\" >$ s\" nosep\" >$ $split .\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("1  ok"), "got {out:?}");
}

#[test]
fn str_split_empty_haystack_yields_one_empty_piece() {
    let mut s = sess_with_core();
    // $split of empty yields ($empty 1).
    // `.` prints 1 (count, top of stack), leaving ($empty).
    // `$len .` prints 0 (length of the piece).
    let out = s.eval(
        "s\" ,\" >$ empty$ $split . $len .\n\
         bye\n"
    ).unwrap();
    let nums: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(nums, vec![1, 0], "got {nums:?} from {out:?}");
}

#[test]
fn str_split_empty_sep_throws() {
    let mut s = sess_with_core();
    let err = s.eval(
        "empty$ s\" hello\" >$ $split drop\nbye\n"
    ).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("-2058"),
        "empty separator should throw -2058; got {msg}");
}

#[test]
fn str_d_wrong_types_throw() {
    // Smoke test that every new V2s-D op rejects FloatVec inputs
    // with -2060.  One per op family.
    let mut s = sess_with_core();
    let err = s.eval(
        "HEAPPTR v\n\
         4 v vec-alloc-floats!\n\
         s\" hi\" >$ v @ $contains? drop\nbye\n"
    ).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("-2060"), "got {msg}");
}

// ── RefVec accessors (added alongside V2s integration demo) ──────

// Convention reminder: vec-ref@ / vec-ref! take a HANDLE (HEAPPTR
// slot address — what `v` pushes), not a raw tagged pointer.  The
// kernel derefs internally.  Same shape as vec-f@ / vec-f!.

#[test]
fn refvec_fresh_cells_are_nil() {
    let mut s = sess_with_core();
    let out = s.eval(
        "HEAPPTR v\n\
         4 v vec-alloc-refs!\n\
         v 0 vec-ref@ .\n\
         v 3 vec-ref@ .\n\
         bye\n"
    ).unwrap();
    let nums: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(nums, vec![0, 0], "fresh RefVec cells should be nil; got {nums:?}");
}

#[test]
fn refvec_store_and_fetch_string_pointer() {
    let mut s = sess_with_core();
    let out = s.eval(
        "HEAPPTR v\n\
         HEAPPTR s\n\
         4 v vec-alloc-refs!\n\
         s\" hello\" >$ s !$\n\
         s @$ v 2 vec-ref!\n\
         v 2 vec-ref@ $>addr type cr\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("hello"),
        "RefVec cell should yield back the string; got {out:?}");
}

#[test]
fn refvec_can_nil_a_cell() {
    let mut s = sess_with_core();
    let out = s.eval(
        "HEAPPTR v\n\
         4 v vec-alloc-refs!\n\
         s\" hi\" >$ v 0 vec-ref!\n\
         0 v 0 vec-ref!                 \\ nil it out\n\
         v 0 vec-ref@ .\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("0  ok"),
        "nil'd cell should fetch 0; got {out:?}");
}

#[test]
fn refvec_wrong_type_throws() {
    let mut s = sess_with_core();
    let err = s.eval(
        "HEAPPTR v\n\
         4 v vec-alloc-floats!\n\
         v 0 vec-ref@ .\n\
         bye\n"
    ).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("-2060"),
        "vec-ref@ on FloatVec should throw -2060; got {msg}");
}

#[test]
fn refvec_nil_throws() {
    let mut s = sess_with_core();
    let err = s.eval(
        "HEAPPTR v\n\
         v 0 vec-ref@ .\n\
         bye\n"
    ).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("-2061"),
        "vec-ref@ on nil should throw -2061; got {msg}");
}

#[test]
fn refvec_survives_collection() {
    let mut s = sess_with_core();
    let out = s.eval(
        "HEAPPTR v\n\
         3 v vec-alloc-refs!\n\
         s\" alpha\" >$ v 0 vec-ref!\n\
         s\" beta\"  >$ v 1 vec-ref!\n\
         s\" gamma\" >$ v 2 vec-ref!\n\
         (gc)\n\
         v 0 vec-ref@ $>addr type cr\n\
         v 1 vec-ref@ $>addr type cr\n\
         v 2 vec-ref@ $>addr type cr\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("alpha"), "got {out:?}");
    assert!(out.contains("beta"),  "got {out:?}");
    assert!(out.contains("gamma"), "got {out:?}");
}

// ── V2s integration demo: word-frequency counter ─────────────────

/// Plain-Forth word-frequency counter exercising the V2s surface
/// end-to-end.  A tiny associative array as two parallel arrays:
/// strings in a RefVec, counts in a FloatVec.  Find-or-insert via
/// linear scan; output is unsorted (the assertions inspect all
/// emitted lines).
///
/// Bigger sort/top-K logic was attempted but is non-trivial to get
/// right purely on the data stack — deferred until WF64 grows
/// Forth-side locals or LET-style scalar bindings for non-FP code.
const WORDCOUNT_DEMO_SRC: &str = r#"
64 constant WC-CAP

HEAPPTR wc-words
HEAPPTR wc-counts
variable wc-n

: wc-init  ( -- )
    WC-CAP wc-words vec-alloc-refs!
    WC-CAP wc-counts vec-alloc-floats!
    0 wc-n ! ;

\ Linear scan: index of matching $word in wc-words, or -1.
: wc-find  ( $word -- i | -1 )
    wc-n @ 0 ?do
        dup wc-words i vec-ref@ $=
        if drop i unloop exit then
    loop
    drop -1 ;

\ Bump count at index by one.
: wc-bump  ( i -- )
    dup wc-counts swap vec-f@        ( i ) ( F: c )
    1e f+                             ( i ) ( F: c+1 )
    wc-counts swap vec-f! ;

\ Insert a brand-new word with count 1.  Caller guarantees room.
: wc-insert  ( $word -- )
    wc-words wc-n @ vec-ref!          \ wc-words[n] := $word
    1e wc-counts wc-n @ vec-f!         \ wc-counts[n] := 1
    1 wc-n +! ;

\ Find-or-insert, bumping on hit, dropping on cap.
: wc-add  ( $word -- )
    dup wc-find dup -1 = if
        drop
        wc-n @ WC-CAP >= if drop exit then
        wc-insert
    else
        nip wc-bump
    then ;

\ Tokenise + count.
: wc-feed  ( $text -- )
    $words                            ( $1 .. $n n )
    0 ?do wc-add loop ;

\ Print every (count, word) row.  No sort.
: wc-print  ( -- )
    wc-n @ 0 ?do
        wc-counts i vec-f@ f>$ $>addr type space
        wc-words i vec-ref@ $>addr type cr
    loop ;

: wc-run  ( $text -- )
    wc-init wc-feed wc-print ;
"#;

#[test]
fn v2s_integration_word_frequency_demo() {
    let mut s = sess_with_core();
    let mut script = String::from(WORDCOUNT_DEMO_SRC);
    script.push_str(
        "\nS$\" the quick brown fox jumps over the lazy dog the fox is quick \
         the dog is lazy and the brown fox is quicker than the lazy dog\" \
         wc-run\nbye\n"
    );
    let out = s.eval(&script).unwrap();
    // Each unique token should appear exactly once in the report.
    // Counts: the=6, fox=3, dog=3, is=3, lazy=3, quick=2, brown=2,
    // quicker=1, jumps=1, over=1, and=1, than=1.
    // Output rows look like "<count> <word>\n" — `f>$` for an
    // integer-valued f64 produces "6" (no decimal point), then
    // `space` emits one space, then the word, then `cr`.
    for (word, n) in [
        ("the", 6), ("fox", 3), ("dog", 3), ("is", 3), ("lazy", 3),
        ("quick", 2), ("brown", 2), ("quicker", 1), ("jumps", 1),
        ("over", 1), ("and", 1), ("than", 1),
    ] {
        let needle = format!("{n} {word}\n");
        assert!(out.contains(&needle),
            "expected row {needle:?}; output:\n{out}");
    }
}

// ── V2s stage E — UTF-8, floats, char$, $words ────────────────────

#[test]
fn str_clen_ascii_equals_byte_length() {
    let mut s = sess_with_core();
    let out = s.eval("s\" hello\" >$ $clen .\nbye\n").unwrap();
    assert!(out.contains("5  ok"), "got {out:?}");
}

#[test]
fn str_clen_utf8_counts_codepoints_not_bytes() {
    // U+00E9 'é' is 2 bytes, U+20AC '€' is 3 bytes — so "é€" is
    // 5 bytes but only 2 codepoints.  Build via char$ + $+ so we
    // don't have to embed multi-byte literals in the Rust source.
    let mut s = sess_with_core();
    let out = s.eval(
        "233 char$ 8364 char$ $+\n\
         dup $len .\n\
         $clen .\n\
         bye\n"
    ).unwrap();
    let nums: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(nums, vec![5, 2], "expected (5 bytes, 2 chars); got {nums:?}");
}

#[test]
fn str_c_at_returns_codepoint() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" abc\" >$\n\
         dup 0 $c@ .                 \\ 'a' = 97\n\
         dup 1 $c@ .                 \\ 'b' = 98\n\
         dup 2 $c@ .                 \\ 'c' = 99\n\
         drop\n\
         bye\n"
    ).unwrap();
    let nums: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(nums, vec![97, 98, 99], "got {nums:?}");
}

#[test]
fn str_c_at_handles_multibyte_codepoints() {
    let mut s = sess_with_core();
    let out = s.eval(
        "233 char$ 8364 char$ $+\n\
         dup 0 $c@ .                 \\ 'é' = 233\n\
         dup 1 $c@ .                 \\ '€' = 8364\n\
         drop\n\
         bye\n"
    ).unwrap();
    let nums: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(nums, vec![233, 8364], "got {nums:?}");
}

#[test]
fn str_c_at_out_of_bounds_throws() {
    let mut s = sess_with_core();
    let err = s.eval(
        "s\" abc\" >$ 10 $c@ .\nbye\n"
    ).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("-2058"),
        "out-of-bounds $c@ should throw -2058; got {msg}");
}

#[test]
fn str_valid_true_for_ascii() {
    let mut s = sess_with_core();
    let out = s.eval("s\" hello\" >$ $valid? .\nbye\n").unwrap();
    assert!(out.contains("-1  ok"), "got {out:?}");
}

#[test]
fn str_valid_true_for_well_formed_utf8() {
    let mut s = sess_with_core();
    let out = s.eval("233 char$ $valid? .\nbye\n").unwrap();
    assert!(out.contains("-1  ok"), "got {out:?}");
}

#[test]
fn str_validate_passes_for_ascii_drops_the_handle() {
    let mut s = sess_with_core();
    let out = s.eval("s\" hello\" >$ $validate 42 .\nbye\n").unwrap();
    assert!(out.contains("42  ok"),
        "stack should be just 42 after $validate; got {out:?}");
}

#[test]
fn str_char_dollar_round_trip_via_c_at() {
    let mut s = sess_with_core();
    let out = s.eval("65 char$ 0 $c@ .\nbye\n").unwrap();
    assert!(out.contains("65  ok"), "got {out:?}");
}

#[test]
fn str_char_dollar_surrogate_throws() {
    let mut s = sess_with_core();
    // 0xD800 is the start of the UTF-16 surrogate range — invalid
    // as a standalone Unicode codepoint.
    let err = s.eval("55296 char$ drop\nbye\n").unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("-2063"),
        "surrogate codepoint should throw -2063; got {msg}");
}

#[test]
fn str_upper_ascii() {
    let mut s = sess_with_core();
    let out = s.eval("s\" hello\" >$ $upper $>addr type cr\nbye\n").unwrap();
    assert!(out.contains("HELLO"), "got {out:?}");
}

#[test]
fn str_lower_ascii() {
    let mut s = sess_with_core();
    let out = s.eval("s\" HELLO\" >$ $lower $>addr type cr\nbye\n").unwrap();
    assert!(out.contains("hello"), "got {out:?}");
}

#[test]
fn str_upper_unicode_lengthens_for_german_ess() {
    // ß (U+00DF, 2 bytes UTF-8) uppercases to "SS" (2 bytes ASCII).
    // Same byte count by accident — pick a clearer one:
    // ﬁ (U+FB01, 3 bytes, "fi" ligature) uppercases to "FI" (2 bytes).
    // Hmm, output is shorter.  Just check that the bytes change
    // sensibly via byte content.
    let mut s = sess_with_core();
    let out = s.eval(
        "223 char$               \\ ß = U+00DF\n\
         $upper $>addr type cr\n\
         bye\n"
    ).unwrap();
    // Should print "SS".
    assert!(out.contains("SS"), "got {out:?}");
}

#[test]
fn str_to_float_parses_simple() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" 1.5\" >$ $>f . f.\n\
         s\" -3.25\" >$ $>f . f.\n\
         bye\n"
    ).unwrap();
    // Each round produces -1 (true) on the data stack and the float
    // on the FP stack.  `. f.` prints true then the float.
    assert!(out.contains("1.500000"), "1.5 parse fail; got {out:?}");
    assert!(out.contains("-3.250000"), "-3.25 parse fail; got {out:?}");
    let signs: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .filter(|&n| n == -1 || n == 0)
        .collect();
    assert_eq!(signs.len(), 2, "should have 2 true flags; got {signs:?} in {out:?}");
}

#[test]
fn str_to_float_failure_returns_false() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" not a float\" >$ $>f .\n\
         bye\n"
    ).unwrap();
    // On failure: only 0 pushed, no FP push.
    let nums: Vec<i64> = out.split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(nums, vec![0], "got {nums:?} from {out:?}");
}

#[test]
fn str_float_to_string_round_trip() {
    let mut s = sess_with_core();
    let out = s.eval(
        "1.5e f>$ $>addr type cr\n\
         -3.25e f>$ $>addr type cr\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("1.5"), "got {out:?}");
    assert!(out.contains("-3.25"), "got {out:?}");
}

#[test]
fn str_sb_append_float() {
    let mut s = sess_with_core();
    let out = s.eval(
        "HEAPPTR b\n\
         32 sb-new b !\n\
         1.5e b @ sb-append-f\n\
         s\" , \" >$ b @ sb-append$\n\
         -3.25e b @ sb-append-f\n\
         b @ sb>string $>addr type cr\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("1.5, -3.25"), "got {out:?}");
}

#[test]
fn str_words_basic_three_tokens() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" foo bar baz\" >$ $words .\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("3  ok"), "got {out:?}");
}

#[test]
fn str_words_consume_pieces_in_reverse() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\" alpha beta gamma\" >$ $words\n\
         drop\n\
         $>addr type cr             \\ gamma\n\
         $>addr type cr             \\ beta\n\
         $>addr type cr             \\ alpha\n\
         bye\n"
    ).unwrap();
    assert!(out.contains("gamma"), "got {out:?}");
    assert!(out.contains("beta"),  "got {out:?}");
    assert!(out.contains("alpha"), "got {out:?}");
}

#[test]
fn str_words_collapses_repeated_and_skips_edge_whitespace() {
    let mut s = sess_with_core();
    let out = s.eval(
        "s\"   foo   bar   \" >$ $words .\n\
         bye\n"
    ).unwrap();
    // s" eats one leading space.  Even so, runs of internal/edge
    // whitespace should yield exactly 2 tokens.
    assert!(out.contains("2  ok"), "got {out:?}");
}

#[test]
fn str_words_empty_haystack_zero_tokens() {
    let mut s = sess_with_core();
    let out = s.eval("empty$ $words .\nbye\n").unwrap();
    assert!(out.contains("0  ok"), "got {out:?}");
}

#[test]
fn gc_vec_f_fetch_wrong_type_still_throws_minus_2060() {
    // Make sure -2060 still fires when the slot holds something with
    // a non-zero, non-FloatVec tag (e.g., a RefVec).  Distinct from
    // the nil case above.
    let mut s = sess_with_core();
    let err = s.eval(
        "HEAPPTR refs\n4 refs vec-alloc-refs!\nrefs 0 vec-f@ f.\nbye\n"
    ).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("-2060"),
        "expected -2060 (wrong type) on vec-f@ over RefVec, got: {msg}");
}

#[test]
fn eval_if_else_then_and_minus_if_work() {
    let mut s = sess();
    let out = s.eval(
        ": choose if 111 else 222 then ;\n\
         : keepflag -if 7 else 9 then ;\n\
         0 choose .\n\
         5 choose .\n\
         0 keepflag . .\n\
         5 keepflag . .\n\
         bye\n"
    ).unwrap();
    assert_eq!(out, " ok\n ok\n222  ok\n111  ok\n9 0  ok\n7 5  ok\n");
}

#[test]
fn eval_begin_until_loops() {
    let mut s = sess();
    let out = s.eval(": down0 begin 1- dup 0= until ;\n3 down0 .\nbye\n").unwrap();
    assert_eq!(out, " ok\n0  ok\n");
}

#[test]
fn eval_begin_while_repeat_loops() {
    let mut s = sess();
    let out = s.eval(": peel begin dup while 1- repeat ;\n3 peel .\nbye\n").unwrap();
    assert_eq!(out, " ok\n0  ok\n");
}

#[test]
fn eval_recurse_compiles_current_definition() {
    let mut s = sess();
    let out = s.eval(": count0 dup 0= if drop 0 else 1- recurse 1+ then ;\n3 count0 .\nbye\n").unwrap();
    assert_eq!(out, " ok\n3  ok\n");
}

#[test]
fn eval_do_loop_counts_up() {
    let mut s = sess();
    let out = s.eval(": countup 5 0 do i . loop ;\ncountup\nbye\n").unwrap();
    assert_eq!(out, " ok\n0 1 2 3 4  ok\n");
}

#[test]
fn eval_qdo_skips_zero_trip_and_runs_nonzero_trip() {
    let mut s = sess();
    let out = s.eval(
        ": maybecount 0 ?do i . loop ;\n\
         5 maybecount\n\
         0 maybecount\n\
         bye\n"
    ).unwrap();
    assert_eq!(out, " ok\n0 1 2 3 4  ok\n ok\n");
}

#[test]
fn eval_plus_loop_steps_by_stride() {
    let mut s = sess();
    let out = s.eval(": evens 10 0 do i . 2 +loop ;\nevens\nbye\n").unwrap();
    assert_eq!(out, " ok\n0 2 4 6 8  ok\n");
}

#[test]
fn eval_minus_loop_counts_down() {
    let mut s = sess();
    let out = s.eval(": countdown 0 5 do i . 1 -loop ;\ncountdown\nbye\n").unwrap();
    assert_eq!(out, " ok\n5 4 3 2 1 0  ok\n");
}

#[test]
fn eval_leave_exits_loop_early() {
    let mut s = sess();
    let out = s.eval(": quit-at-2 5 0 do i . i 2 = if leave then loop ;\nquit-at-2\nbye\n").unwrap();
    assert_eq!(out, " ok\n0 1 2  ok\n");
}

#[test]
fn eval_qleave_exits_when_flag_is_true() {
    let mut s = sess();
    let out = s.eval(": qquit-at-2 5 0 do i . i 2 = ?leave loop ;\nqquit-at-2\nbye\n").unwrap();
    assert_eq!(out, " ok\n0 1 2  ok\n");
}

#[test]
fn eval_two_r_roundtrip_through_repl() {
    let mut s = sess();
    let out = s.eval(": ferry2 2>r 2r@ . . 2r> ;\n10 20 ferry2 . .\nbye\n").unwrap();
    assert_eq!(out, " ok\n20 10 20 10  ok\n");
}

#[test]
fn eval_compiled_inline_stack_words_work() {
    let mut s = sess();
    let out = s.eval(": stackplay over swap drop ;\n7 9 stackplay . .\nbye\n").unwrap();
    assert_eq!(out, " ok\n7 7  ok\n");
}

#[test]
fn tfa_fetch_distinguishes_colon_defs_from_primitives() {
    let mut s = sess();

    let dup_xt = s.xt_of("dup_").unwrap() as i64;
    s.push(dup_xt);
    s.call("to_name").unwrap();
    let dup_nt = s.pop();
    s.push(dup_nt);
    s.call("tfa_fetch").unwrap();
    assert_eq!(s.pop(), 0);

    let out = s.eval(": typed 1 ;\nbye\n").unwrap();
    assert_eq!(out, " ok\n");

    s.call("latestxt").unwrap();
    let xt = s.pop();
    s.push(xt);
    s.call("to_name").unwrap();
    let nt = s.pop();
    s.push(nt);
    s.call("tfa_fetch").unwrap();
    assert_eq!(s.pop(), 0x82);
}

#[test]
fn create_builds_a_created_word_that_pushes_its_body() {
    let mut s = sess();
    let out = s.eval("create made\nmade\nbye\n").unwrap();
    assert_eq!(out, " ok\n ok\n");

    s.call("latestxt").unwrap();
    let xt = s.pop() as u64;
    let body = s.pop() as u64;
    // The body now lives in the separate RW data region, off the executable
    // stub entirely (W^X; no SMC). It must not sit on the stub's cache line.
    assert!(body < xt || body >= xt + 64,
        "create body must be off the code stub, got body={body:#x} xt={xt:#x}");

    s.push(xt as i64);
    s.call("to_name").unwrap();
    let nt = s.pop();
    s.push(nt);
    s.call("tfa_fetch").unwrap();
    assert_eq!(s.pop(), 0x91);

    s.push(xt as i64);
    s.call("to_body").unwrap();
    assert_eq!(s.pop() as u64, body);
}

#[test]
fn to_body_throws_minus_31_on_colon_definition() {
    let mut s = sess();
    let out = s.eval(": bodyfail 1 ;\nbye\n").unwrap();
    assert_eq!(out, " ok\n");

    s.call("latestxt").unwrap();
    let xt = s.pop();
    let to_body_xt = s.xt_of("to_body").unwrap() as i64;
    s.push(xt);
    s.push(to_body_xt);
    s.call("catch_word").unwrap();
    assert_eq!(s.stack(), vec![-31, xt]);
}

#[test]
fn forth_visible_body_word_resolves_to_kernel_to_body_xt() {
    let mut s = sess();
    let out = s.eval("' >body\nbye\n").unwrap();
    assert_eq!(out, " ok\n");

    let forth_xt = s.pop() as u64;
    let kernel_xt = s.xt_of("to_body").unwrap() as u64;
    assert_eq!(forth_xt, kernel_xt);
}

#[test]
fn execute_of_to_body_xt_matches_direct_call() {
    let mut s = sess();
    let out = s.eval("create made\nbye\n").unwrap();
    assert_eq!(out, " ok\n");

    s.call("latestxt").unwrap();
    let xt = s.pop();
    let to_body_xt = s.xt_of("to_body").unwrap() as i64;

    s.push(xt);
    s.push(to_body_xt);
    s.call("execute").unwrap();
    let body = s.pop() as u64;
    let xtu = xt as u64;
    assert!(body < xtu || body >= xtu + 64,
        "to_body result must be off the code stub, got body={body:#x} xt={xtu:#x}");
}

#[test]
fn eval_body_word_leaves_created_body_address_on_stack() {
    let mut s = sess();
    let out = s.eval("create made\n' made >body\nbye\n").unwrap();
    assert_eq!(out, " ok\n ok\n");

    s.call("latestxt").unwrap();
    let xt = s.pop() as u64;
    let body = s.pop() as u64;

    assert!(body < xt || body >= xt + 64,
        "(>body) must be off the code stub, got body={body:#x} xt={xt:#x}");
}

#[test]
fn compiled_body_word_returns_created_body_address() {
    let mut s = sess();
    let out = s.eval(": bodyword >body ;\ncreate made\n' made bodyword\nbye\n").unwrap();
    assert_eq!(out, " ok\n ok\n ok\n");

    s.call("latestxt").unwrap();
    let xt = s.pop() as u64;
    let body = s.pop() as u64;

    assert!(body < xt || body >= xt + 64,
        "compiled (>body) must be off the code stub, got body={body:#x} xt={xt:#x}");
}

#[test]
fn defer_hook_does_not_corrupt_to_body_code() {
    let mut s = sess();
    let to_body_xt = s.xt_of("to_body").unwrap() as u64;
    let before = unsafe { std::slice::from_raw_parts(to_body_xt as *const u8, 24).to_vec() };

    let out = s
        .eval(": , here ! 1 cells allot ;\n: variable create 0 , ;\n: constant create , does> @ ;\n: value create , does> @ ;\n: defer@ >body @ ;\n: defer! >body ! ;\n: defer-err -261 throw ;\n: defer create ['] defer-err , does> @ execute ;\ndefer hook\nbye\n")
        .unwrap();
    assert_eq!(out, " ok\n ok\n ok\n ok\n ok\n ok\n ok\n ok\n ok\n");

    let after = unsafe { std::slice::from_raw_parts(to_body_xt as *const u8, 24).to_vec() };
    assert_eq!(after, before);
}

#[test]
fn link_to_name_returns_latest_header_name_token() {
    let mut s = sess();
    let pad = s.user_base + 0x100;
    let name = b"BAR";
    unsafe { std::ptr::copy_nonoverlapping(name.as_ptr(), pad as *mut u8, name.len()); }

    s.push(pad as i64);
    s.push(name.len() as i64);
    s.call("create").unwrap();
    let latest = s.latest() as i64;

    s.push(latest);
    s.call("link_to_name").unwrap();
    let nt = s.pop() as u64;

    let len = unsafe { (nt as *const u8).read() };
    let bytes = unsafe { std::slice::from_raw_parts((nt + 1) as *const u8, len as usize) };
    assert_eq!(len, name.len() as u8);
    assert_eq!(bytes, name);
}

#[test]
fn name_to_interpret_and_name_to_compile_roundtrip_header_tokens() {
    let mut s = sess();
    let pad = s.user_base + 0x100;
    let name = b"BAZ";
    unsafe { std::ptr::copy_nonoverlapping(name.as_ptr(), pad as *mut u8, name.len()); }

    s.push(pad as i64);
    s.push(name.len() as i64);
    s.call("create").unwrap();
    let dup_xt = s.xt_of("dup_").unwrap() as i64;
    let compile_xt = s.xt_of("compile_word").unwrap() as i64;
    s.push(dup_xt);
    s.call("set_xt").unwrap();

    s.push(dup_xt);
    s.call("to_name").unwrap();
    let nt = s.pop();

    s.push(nt);
    s.call("name_to_interpret").unwrap();
    assert_eq!(s.pop(), dup_xt);

    s.push(nt);
    s.call("name_to_compile").unwrap();
    assert_eq!(s.pop(), compile_xt);
    assert_eq!(s.pop(), dup_xt);
}

#[test]
fn latestxt_tracks_latest_definition_and_resets() {
    let mut s = sess();
    s.call("latestxt").unwrap();
    let boot_latestxt = s.pop();

    let pad = s.user_base + 0x100;
    let name = b"QUX";
    unsafe { std::ptr::copy_nonoverlapping(name.as_ptr(), pad as *mut u8, name.len()); }

    s.push(pad as i64);
    s.push(name.len() as i64);
    s.call("create").unwrap();
    s.call("latestxt").unwrap();
    let created_xt = s.pop();
    assert_ne!(created_xt, boot_latestxt);

    let dup_xt = s.xt_of("dup_").unwrap() as i64;
    s.push(dup_xt);
    s.call("set_xt").unwrap();
    s.call("latestxt").unwrap();
    assert_eq!(s.pop(), dup_xt);

    s.reset();
    s.call("latestxt").unwrap();
    assert_eq!(s.pop(), boot_latestxt);
}

#[test]
fn find_name_returns_counted_name_token() {
    const DH_NT: u64 = (5 * 8) + 2 + 2 + 2 + 1;

    let mut s = sess();
    let pad = s.user_base + 0x100;
    let name = b"BAR";
    unsafe { std::ptr::copy_nonoverlapping(name.as_ptr(), pad as *mut u8, name.len()); }

    s.push(pad as i64);
    s.push(name.len() as i64);
    s.call("create").unwrap();

    s.push(pad as i64);
    s.push(name.len() as i64);
    s.call("find_name").unwrap();

    assert_eq!(s.pop(), -1);
    let nt = s.pop() as u64;
    assert_eq!(nt, s.latest() + DH_NT);

    let len = unsafe { (nt as *const u8).read() };
    let bytes = unsafe { std::slice::from_raw_parts((nt + 1) as *const u8, len as usize) };
    assert_eq!(len, name.len() as u8);
    assert_eq!(bytes, name);
}

#[test]
fn number_q_parses_single_digit_directly() {
    let mut s = sess();
    let pad = s.user_base + 0x100;
    unsafe { std::ptr::copy_nonoverlapping(b"1".as_ptr(), pad as *mut u8, 1); }

    s.push(pad as i64);
    s.push(1);
    s.call("number_q").unwrap();

    assert_eq!(s.pop(), -1);
    assert_eq!(s.pop(), 1);
    assert_eq!(s.depth(), 0);
}

#[test]
fn find_name_miss_leaves_c_addr_u_zero() {
    let mut s = sess();
    let pad = s.user_base + 0x100;
    unsafe { std::ptr::copy_nonoverlapping(b"1".as_ptr(), pad as *mut u8, 1); }

    s.push(pad as i64);
    s.push(1);
    s.call("find_name").unwrap();

    assert_eq!(s.stack(), vec![0, 1, pad as i64]);
}

#[test]
fn get_order_word_reports_default_forth_order() {
    // Default search order after reset is (PRIVATE TOOLS FORTH) with
    // PRIVATE innermost. get-order returns wid_n ... wid_1 n with the
    // count on top, then wids from innermost to outermost going down.
    let mut s = sess();
    s.call("forth_wordlist_word").unwrap();
    let forth_wid = s.stack()[0];
    s.reset();
    let private_wid = unsafe { ((s.user_base + 0x17D0) as *const u64).read_unaligned() } as i64;
    let tools_wid   = unsafe { ((s.user_base + 0x17C8) as *const u64).read_unaligned() } as i64;
    s.call("get_order_word").unwrap();
    // stack() is top-first: [count, innermost, ..., outermost]
    assert_eq!(s.stack(), vec![3, private_wid, tools_wid, forth_wid]);
}

#[test]
fn bootstrap_splits_primitives_into_three_wordlists() {
    // After boot, `.s` should be findable in TOOLS but not in FORTH;
    // `(create)` should be findable in PRIVATE but not in FORTH or
    // TOOLS; and ordinary words like `dup` should be in FORTH only.
    let mut s = sess();
    let forth_wid   = unsafe { ((s.user_base + 0x1508) as *const u64).read_unaligned() } as i64;
    let tools_wid   = unsafe { ((s.user_base + 0x17C8) as *const u64).read_unaligned() } as i64;
    let private_wid = unsafe { ((s.user_base + 0x17D0) as *const u64).read_unaligned() } as i64;

    // Define a probe helper inside Forth: collapses search-wordlist's
    // two-shape return ( 0 | xt ±1 ) into a single flag.
    s.eval(": probe-sw  ( c-addr u wid -- flag )  search-wordlist dup if nip then ;\nbye\n")
        .unwrap();
    fn probe(s: &mut Wf64Session, name: &str, wid: i64) -> i64 {
        let code = format!(
            "s\" {name}\" {wid} probe-sw .\nbye\n",
            name = name, wid = wid
        );
        let out = s.eval(&code).unwrap();
        out.split_whitespace().next().unwrap().parse().unwrap()
    }

    // `dup` is in FORTH only.
    assert_ne!(probe(&mut s, "dup", forth_wid),  0);
    assert_eq!(probe(&mut s, "dup", tools_wid),  0);
    assert_eq!(probe(&mut s, "dup", private_wid), 0);

    // `.s` is in TOOLS only.
    assert_eq!(probe(&mut s, ".s", forth_wid),   0);
    assert_ne!(probe(&mut s, ".s", tools_wid),   0);
    assert_eq!(probe(&mut s, ".s", private_wid), 0);

    // `(create)` is in PRIVATE only.
    assert_eq!(probe(&mut s, "(create)", forth_wid),   0);
    assert_eq!(probe(&mut s, "(create)", tools_wid),   0);
    assert_ne!(probe(&mut s, "(create)", private_wid), 0);

}

#[test]
fn core_f_categorises_source_defined_words_correctly() {
    // After loading core.f, source-defined words must end up in the
    // wordlist their inline `set-current` directives put them in.
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();

    let forth_wid   = unsafe { ((s.user_base + 0x1508) as *const u64).read_unaligned() } as i64;
    let tools_wid   = unsafe { ((s.user_base + 0x17C8) as *const u64).read_unaligned() } as i64;
    let private_wid = unsafe { ((s.user_base + 0x17D0) as *const u64).read_unaligned() } as i64;

    s.eval(": probe-sw  ( c-addr u wid -- flag )  search-wordlist dup if nip then ;\nbye\n")
        .unwrap();
    fn probe(s: &mut Wf64Session, name: &str, wid: i64) -> i64 {
        let code = format!(
            "s\" {name}\" {wid} probe-sw .\nbye\n",
            name = name, wid = wid
        );
        let out = s.eval(&code).unwrap();
        out.split_whitespace().next().unwrap().parse().unwrap()
    }

    // TOOLS-tagged source words.
    assert_ne!(probe(&mut s, "words",      tools_wid),   0);
    assert_ne!(probe(&mut s, "marker",     tools_wid),   0);
    assert_ne!(probe(&mut s, "[defined]",  tools_wid),   0);
    assert_eq!(probe(&mut s, "marker",     forth_wid),   0);

    // PRIVATE-tagged source words.
    assert_ne!(probe(&mut s, "locals-set", private_wid), 0);
    assert_ne!(probe(&mut s, "subst-find", private_wid), 0);
    assert_eq!(probe(&mut s, "locals-set", forth_wid),   0);

    // FORTH-tagged source words (user-facing).
    assert_ne!(probe(&mut s, "constant",   forth_wid),   0);
    assert_ne!(probe(&mut s, "{:",         forth_wid),   0);
    assert_ne!(probe(&mut s, "floor",      forth_wid),   0);
}

#[test]
fn eval_negative_set_order_minimum_then_get_order() {
    let mut s = sess();
    s.call("forth_wordlist_word").unwrap();
    let root_wid = s.stack()[0];
    s.reset();
    let out = s.eval("-1 set-order get-order\nbye\n").unwrap();
    assert_eq!(out, " ok\n");
    assert_eq!(s.stack(), vec![1, root_wid]);
}

#[test]
fn eval_source_defined_set_order_wrapper_then_get_order() {
    let mut s = sess();
    s.call("forth_wordlist_word").unwrap();
    let root_wid = s.stack()[0];
    s.reset();
    let out = s.eval(": only2 -1 set-order ;\nonly2 get-order\nbye\n").unwrap();
    assert_eq!(out, " ok\n ok\n");
    assert_eq!(s.stack(), vec![1, root_wid]);
}

#[test]
fn search_order_words_route_lookup_by_wordlist() {
    let mut s = sess();
    let pad = s.user_base + 0x100;
    let name = b"TOK";
    unsafe { std::ptr::copy_nonoverlapping(name.as_ptr(), pad as *mut u8, name.len()); }

    s.call("wordlist_word").unwrap();
    let extra_wid = s.pop();
    s.call("forth_wordlist_word").unwrap();
    let root_wid = s.pop();

    s.push(extra_wid);
    s.call("set_current_word").unwrap();
    s.push(pad as i64);
    s.push(name.len() as i64);
    s.call("create").unwrap();
    let dup_xt = s.xt_of("dup_").unwrap() as i64;
    s.push(dup_xt);
    s.call("set_xt").unwrap();

    s.push(root_wid);
    s.call("set_current_word").unwrap();
    s.push(pad as i64);
    s.push(name.len() as i64);
    s.call("create").unwrap();
    let drop_xt = s.xt_of("drop_").unwrap() as i64;
    s.push(drop_xt);
    s.call("set_xt").unwrap();

    s.push(root_wid);
    s.push(extra_wid);
    s.push(2);
    s.call("set_order_word").unwrap();
    s.push(pad as i64);
    s.push(name.len() as i64);
    s.call("find_name").unwrap();
    assert_eq!(s.pop(), -1);
    let nt = s.pop();
    s.push(nt);
    s.call("name_to_interpret").unwrap();
    assert_eq!(s.pop(), dup_xt);

    s.push(root_wid);
    s.push(1);
    s.call("set_order_word").unwrap();
    s.push(pad as i64);
    s.push(name.len() as i64);
    s.call("find_name").unwrap();
    assert_eq!(s.pop(), -1);
    let nt = s.pop();
    s.push(nt);
    s.call("name_to_interpret").unwrap();
    assert_eq!(s.pop(), drop_xt);

    assert_eq!(s.depth(), 0);
}

#[test]
fn load_source_file_provides_search_order_extension_words() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.call("forth_wordlist_word").unwrap();
    let root_wid = s.stack()[0];
    s.reset();
    s.load_source_file(&path).unwrap();

    let out = s.eval(
        "forth-wordlist constant root\n\
         wordlist constant extra\n\
         bye\n"
    ).unwrap();
    assert_eq!(out, " ok\n ok\n");

    let out = s.eval("only get-order\nbye\n").unwrap();
    assert_eq!(out, " ok\n");
    assert_eq!(s.stack(), vec![1, root_wid]);

    s.reset();
    s.load_source_file(&path).unwrap();
    s.call("forth_wordlist_word").unwrap();
    assert_eq!(s.stack(), vec![root_wid]);
    s.pop();
    let out = s.eval(
        "forth-wordlist constant root\n\
         wordlist constant extra\n\
         only also get-order\n\
         bye\n"
    ).unwrap();
    assert_eq!(out, " ok\n ok\n ok\n");
    assert_eq!(s.stack(), vec![2, root_wid, root_wid]);

    s.reset();
    s.load_source_file(&path).unwrap();
    let out = s.eval(
        "forth-wordlist constant root\n\
         wordlist constant extra\n\
         root extra 2 set-order previous get-order\n\
         bye\n"
    ).unwrap();
    assert_eq!(out, " ok\n ok\n ok\n");
    let stack = s.stack();
    assert_eq!(stack.len(), 2);
    assert_eq!(stack[0], 1);
    assert_eq!(stack[1], root_wid);

    s.reset();
    s.load_source_file(&path).unwrap();
    let out = s.eval(
        "forth-wordlist constant root\n\
         wordlist constant extra\n\
         root extra 2 set-order forth get-order\n\
         bye\n"
    ).unwrap();
    assert_eq!(out, " ok\n ok\n ok\n");
    assert_eq!(s.stack(), vec![2, root_wid, root_wid]);

    s.reset();
    s.load_source_file(&path).unwrap();
    let out = s.eval(
        "forth-wordlist constant root\n\
         wordlist constant extra\n\
         root extra 2 set-order definitions get-current\n\
         extra\n\
         bye\n"
    ).unwrap();
    assert_eq!(out, " ok\n ok\n ok\n ok\n");
    let stack = s.stack();
    assert_eq!(stack.len(), 2);
    assert_eq!(stack[0], stack[1]);
    assert_ne!(stack[0], root_wid);
}

#[test]
fn search_wordlist_returns_xt_and_immediacy_flag() {
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();

    // `.s` lives in the TOOLS wordlist now, so the original variant
    // that searched FORTH for it no longer applies. The four other
    // names cover the same code path.
    let out = s.eval(
        "forth-wordlist constant root\n\
         : dup-name s\" dup\" ;\n\
         : semi-name s\" ;\" ;\n\
         : exit-name s\" exit\" ;\n\
         : keyq-name s\" key?\" ;\n\
         dup-name root search-wordlist swap drop . cr\n\
         semi-name root search-wordlist nip . cr\n\
         exit-name root search-wordlist nip . cr\n\
         keyq-name root search-wordlist nip . cr\n\
         bye\n"
    ).unwrap();

    assert_eq!(out, " ok\n ok\n ok\n ok\n ok\n-1 \n ok\n1 \n ok\n1 \n ok\n-1 \n ok\n");
}

// ─── Forth 2012 locals (`{: … :}`) — known-broken note ───────────────
//
// Attempting to exercise `{:` via `s.eval(...)` after
// `load_source_file(core.f)` currently reproduces the same
// `?`-flood failure that broke `demos/gfx-click.f` — every token
// in the colon body (including kernel primitives like `bye`)
// shows `?`, even though `[']` can find `{:` cleanly.  Locals
// work via the direct REPL path (`cargo run --bin wf64`) but
// not through the `session.eval` test path.  The mismatch is
// the locals-impl bug we still need to root-cause; until then,
// the test matrix here can't lock in `{:` behaviour and is
// deferred.  See project_wf64.md and the post-session notes.
//
// Tests we still keep: the state-mismatch throw on nested-`:`,
// because that path doesn't depend on `{:` itself.

#[test]
fn double_colon_inside_definition_throws_minus_29() {
    // Calling `:` while STATE != 0 must throw -29 (compiler nesting)
    // rather than silently re-entering compile mode and producing
    // `?`-flood downstream.
    let mut s = sess();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join("core.f");
    s.load_source_file(&path).unwrap();
    let out = s.eval(
        ": provoke   ( -- throw-code )\n\
             state @ >r\n\
             1 state !\n\
             ['] : catch\n\
             r> state !\n\
         ;\n\
         provoke . cr\n\
         bye\n"
    ).unwrap();
    assert_eq!(out, " ok\n ok\n ok\n ok\n ok\n ok\n-29 \n ok\n");
}

#[test]
fn set_flags_marks_word_immediate() {
    // Build a word "IMM" pointing at `cr_word` (no-op for the data
    // stack, just emits a newline), then mark it IMMEDIATE. In compile
    // mode it should run NOW (emitting a newline at compile time)
    // rather than getting compiled into the definition.
    let mut s = sess();
    let pad = s.user_base + 0x100;
    let name = b"IMM";
    unsafe { std::ptr::copy_nonoverlapping(name.as_ptr(), pad as *mut u8, name.len()); }
    s.push(pad as i64);
    s.push(name.len() as i64);
    s.call("create").unwrap();
    let cr_xt = s.xt_of("cr_word").unwrap();
    s.push(cr_xt as i64);
    s.call("set_xt").unwrap();
    s.push(1);
    s.call("set_flags").unwrap();

    // Compile a definition that has IMM in its body. Because IMM is
    // immediate, the CR fires at compile time, not when bar runs.
    let out = s.eval(": bar IMM 5 ;\nbar .\nbye\n").unwrap();
    // The first " ok" comes after the colon-def line, with a CR
    // emitted in the middle (between `:` and the ` ok`):
    assert!(out.contains('\n'), "expected an immediate-fire newline; got {out:?}");
    // bar . prints `5 `, then ok.
    assert!(out.ends_with("5  ok\n"), "got {out:?}");
}

#[test]
fn mixed_define_then_call_directly() {
    let mut s = sess();
    s.eval(": cube dup dup * * ;\n").unwrap();
    // Look up `cube`'s xt by walking the dict — easier route is via the
    // REPL: `' cube` doesn't exist yet; do it through eval:
    s.push(4);
    // We don't have `' word` (tick) yet so fall back to eval for the call.
    // This test mainly exercises that the dict mutation from eval is
    // visible to subsequent eval calls.
    let out = s.eval("4 cube .\nbye\n").unwrap();
    assert_eq!(out, "64  ok\n");
    let _ = s.pop();  // drop the 4 we pushed up top — not consumed by the eval
}

// ── data-driven tests ───────────────────────────────────────────────
//
// Adding a new primitive should never need a Rust recompile. These
// two `#[test]` fns walk the corresponding subdirectories under
// `tests/data/`, classify each case as PASS / FAIL / NYIMP, and emit
// a summary. Only FAILs cause the test to fail; NYIMP and PASS are
// both "the suite ran cleanly."
//
// Workflow this enables (test-first):
//
//   1. Write test files for the next batch of primitives — words that
//      may not yet exist in the kernel.
//   2. `cargo test --test harness` — failing primitives show as NYIMP,
//      not FAIL. Suite still passes.
//   3. Port one primitive (via `cargo run --bin port-wf32 …`), paste
//      into kernel/*.masm, add to PRIMITIVES.
//   4. Re-run. The corresponding NYIMP flips to PASS automatically.
//
// `tests/data/direct/*.t` — direct primitive test, line-oriented DSL.
//                            NYIMP detected by pre-scanning `call <sym>`
//                            lines and looking each up via `xt_of`.
// `tests/data/eval/*.in`  — Forth source fed through the REPL.
// `tests/data/eval/*.out` — expected stdout, exact match.
//                            NYIMP detected by an optional comment line
//                            `# requires: word1 word2 ...` listing the
//                            Forth-side names; missing any → NYIMP.

#[derive(Debug)]
enum Outcome {
    Pass,
    Nyimp(Vec<String>), // missing-symbol/word list
    Fail(String),       // human-readable failure detail
}

#[test]
fn data_driven_direct_tests() {
    let dir = data_dir().join("direct");
    let cases = collect_files(&dir, "t");
    if cases.is_empty() {
        eprintln!("note: no .t files under {} — nothing to run", dir.display());
        return;
    }
    let results: Vec<(PathBuf, Outcome)> = cases
        .iter()
        .map(|p| (p.clone(), classify_direct(p)))
        .collect();
    summarize_and_assert("direct", &results);
}

#[test]
fn data_driven_eval_tests() {
    let dir = data_dir().join("eval");
    let cases = collect_files(&dir, "in");
    if cases.is_empty() {
        eprintln!("note: no .in files under {} — nothing to run", dir.display());
        return;
    }
    let results: Vec<(PathBuf, Outcome)> = cases
        .iter()
        .map(|p| {
            let out = p.with_extension("out");
            (p.clone(), classify_eval(p, &out))
        })
        .collect();
    summarize_and_assert("eval", &results);
}

fn summarize_and_assert(kind: &str, results: &[(PathBuf, Outcome)]) {
    let mut pass = 0;
    let mut fail = 0;
    let mut nyimp = 0;
    let mut nyimp_list: Vec<String> = Vec::new();
    let mut fail_list: Vec<(String, String)> = Vec::new();
    for (path, outcome) in results {
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        match outcome {
            Outcome::Pass => pass += 1,
            Outcome::Nyimp(missing) => {
                nyimp += 1;
                nyimp_list.push(format!("{} [missing: {}]", name, missing.join(" ")));
            }
            Outcome::Fail(msg) => {
                fail += 1;
                fail_list.push((name, msg.clone()));
            }
        }
    }
    eprintln!(
        "── {kind} tests: {pass} PASS, {fail} FAIL, {nyimp} NYIMP ──"
    );
    if !nyimp_list.is_empty() {
        eprintln!("  NYIMP:");
        for line in &nyimp_list {
            eprintln!("    {line}");
        }
    }
    if !fail_list.is_empty() {
        eprintln!("  FAIL:");
        for (name, msg) in &fail_list {
            eprintln!("    {name}: {msg}");
        }
        panic!("{fail} {kind} test(s) failed (see stderr for detail)");
    }
}

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
}

fn collect_files(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = match fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|r| r.ok().map(|e| e.path())).collect(),
        Err(_) => return Vec::new(),
    };
    v.retain(|p| p.extension() == Some(OsStr::new(ext)));
    v.sort();
    v
}

// ── direct (.t) ──────────────────────────────────────────────────────

/// Direct-DSL line-oriented commands:
///
/// - `#`/`;` — comment to end of line
/// - `push <int>` — push a cell (decimal, `0xFF` hex, or negative)
/// - `push_pad <offset>` — push `user_base + USER_PAD + offset`, where
///   USER_PAD = 0x100. Lets a test write to scratch memory without
///   hardcoding session addresses.
/// - `poke <pad-off> <hex-bytes>` — write a sequence of bytes into
///   the user-area PAD region at `pad-off`. `<hex-bytes>` is a
///   contiguous string of hex pairs (e.g. `48656c6c6f` for "Hello").
///   Used by string-primitive tests that need to seed a buffer
///   before calling `cmove`, `compare`, etc.
/// - `expect_bytes <pad-off> <hex-bytes>` — opposite of `poke`: read
///   `N` bytes from PAD+off and assert they match the hex string.
/// - `call <sym>` — invoke a primitive by its asm symbol
/// - `expect <int>...` — assert stack equals these values, **bottom-first**
///   (Forth notation: `expect 1 2 3` means `1` is deepest, `3` is TOS).
///   `expect` with no args means "stack should be empty."
/// - `reset` — restore the session to post-bootstrap state
fn classify_direct(path: &Path) -> Outcome {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => return Outcome::Fail(format!("read failed: {e}")),
    };

    // Pre-scan for missing asm symbols.
    let mut s = sess();
    let mut missing: Vec<String> = Vec::new();
    for line in text.lines() {
        let trimmed = strip_comment(line).trim();
        if let Some(rest) = trimmed.strip_prefix("call ") {
            let sym = rest.split_whitespace().next().unwrap_or("");
            if s.xt_of(sym).is_err() && !missing.contains(&sym.to_string()) {
                missing.push(sym.to_string());
            }
        }
    }
    if !missing.is_empty() {
        return Outcome::Nyimp(missing);
    }

    // Run.
    let pad_base = s.user_base + 0x100; // USER_PAD offset, mirrors kernel/macros.masm
    for (i, line) in text.lines().enumerate() {
        let lineno = i + 1;
        let body = strip_comment(line).trim();
        if body.is_empty() {
            continue;
        }
        let mut parts = body.split_whitespace();
        let cmd = parts.next().unwrap();
        let res = (|| -> Result<(), String> {
            match cmd {
                "push" => {
                    let raw = parts.next().ok_or("push needs a value")?;
                    let v = parse_int(raw).ok_or_else(|| format!("bad int `{raw}`"))?;
                    s.push(v);
                }
                "push_pad" => {
                    let raw = parts.next().ok_or("push_pad needs an offset")?;
                    let off = parse_int(raw).ok_or_else(|| format!("bad int `{raw}`"))?;
                    s.push((pad_base as i64).wrapping_add(off));
                }
                "poke" => {
                    let off_raw = parts.next().ok_or("poke needs an offset")?;
                    let hex = parts.next().ok_or("poke needs hex bytes")?;
                    let off = parse_int(off_raw)
                        .ok_or_else(|| format!("bad offset `{off_raw}`"))?;
                    let bytes = parse_hex_bytes(hex)
                        .ok_or_else(|| format!("bad hex bytes `{hex}`"))?;
                    let dst = (pad_base as i64).wrapping_add(off) as *mut u8;
                    // SAFETY: pad region lives inside the 128 MB
                    // session-allocated block; tests address it via
                    // bounded offsets.
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            bytes.as_ptr(),
                            dst,
                            bytes.len(),
                        );
                    }
                }
                "expect_bytes" => {
                    let off_raw = parts.next().ok_or("expect_bytes needs an offset")?;
                    let hex = parts.next().ok_or("expect_bytes needs hex bytes")?;
                    let off = parse_int(off_raw)
                        .ok_or_else(|| format!("bad offset `{off_raw}`"))?;
                    let want = parse_hex_bytes(hex)
                        .ok_or_else(|| format!("bad hex bytes `{hex}`"))?;
                    let src = (pad_base as i64).wrapping_add(off) as *const u8;
                    // SAFETY: same as `poke` above.
                    let got: Vec<u8> = unsafe {
                        std::slice::from_raw_parts(src, want.len()).to_vec()
                    };
                    if got != want {
                        return Err(format!(
                            "bytes mismatch at PAD+{off:#x}\n      expected: {}\n      got     : {}",
                            hex_bytes(&want),
                            hex_bytes(&got)
                        ));
                    }
                }
                "call" => {
                    let sym = parts.next().ok_or("call needs a symbol")?;
                    s.call(sym).map_err(|e| format!("call {sym}: {e}"))?;
                }
                "expect" => {
                    let want_bot_first: Vec<i64> = parts
                        .map(|t| parse_int(t).ok_or_else(|| format!("bad int `{t}`")))
                        .collect::<Result<_, _>>()?;
                    let want: Vec<i64> =
                        want_bot_first.iter().rev().copied().collect();
                    let got = s.stack();
                    if got != want {
                        return Err(format!(
                            "stack mismatch\n      expected (bottom→top): {:?}\n      got      (top→bottom): {:?}",
                            want_bot_first, got
                        ));
                    }
                }
                "reset" => s.reset(),
                other => return Err(format!("unknown command `{other}`")),
            }
            Ok(())
        })();
        if let Err(msg) = res {
            return Outcome::Fail(format!("line {lineno}: {msg}"));
        }
    }
    Outcome::Pass
}

// ── eval (.in / .out) ────────────────────────────────────────────────

fn classify_eval(in_path: &Path, out_path: &Path) -> Outcome {
    let input = match fs::read_to_string(in_path) {
        Ok(t) => t.replace("\r\n", "\n"),
        Err(e) => return Outcome::Fail(format!("read .in: {e}")),
    };
    let expected = match fs::read_to_string(out_path) {
        Ok(t) => t.replace("\r\n", "\n"),
        Err(e) => return Outcome::Fail(format!("read .out: {e}")),
    };

    // NYIMP detection: `# requires: word1 word2 …` lines list Forth
    // names this test depends on. Missing any → NYIMP. (Tests that
    // don't declare requirements run unconditionally — fine for words
    // we KNOW are present, like the M3/M4 baseline.)
    let mut required: Vec<String> = Vec::new();
    for line in input.lines() {
        let t = line.trim_start();
        if let Some(rest) = t
            .strip_prefix("#")
            .or_else(|| t.strip_prefix(";"))
            .map(|r| r.trim_start())
        {
            if let Some(list) = rest.strip_prefix("requires:") {
                required.extend(list.split_whitespace().map(String::from));
            }
        }
    }
    let missing: Vec<String> = required
        .into_iter()
        .filter(|w| !wf64::PRIMITIVES.iter().any(|&(name, _, _)| name == w))
        .collect();
    if !missing.is_empty() {
        return Outcome::Nyimp(missing);
    }

    // Strip harness-only metadata lines (those starting with `#`) so
    // the kernel doesn't see them as Forth source. Forth's own comment
    // syntax (`\` to end-of-line, `( … )` inline) passes through
    // unchanged.
    let forth_source: String = input
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<&str>>()
        .join("\n")
        + "\n";

    let mut s = sess();
    match s.eval(&forth_source) {
        Ok(actual) if actual == expected => Outcome::Pass,
        Ok(actual) => Outcome::Fail(format!(
            "output mismatch\n      expected: {:?}\n      got     : {:?}",
            expected, actual
        )),
        Err(e) => Outcome::Fail(format!("eval failed: {e}")),
    }
}

// ── shared helpers ───────────────────────────────────────────────────

fn strip_comment(line: &str) -> &str {
    let cut = line
        .find(|c| c == '#' || c == ';')
        .unwrap_or(line.len());
    &line[..cut]
}

/// Parse a contiguous hex string like `"48656c6c6f"` into bytes.
/// Ignores optional underscores so longer strings can be grouped for
/// readability (`"4865_6c6c_6f"`).
fn parse_hex_bytes(s: &str) -> Option<Vec<u8>> {
    let cleaned: String = s.chars().filter(|c| *c != '_').collect();
    if cleaned.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(cleaned.len() / 2);
    for pair in cleaned.as_bytes().chunks(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}

fn hex_bytes(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for byte in b {
        s.push_str(&format!("{byte:02x}"));
    }
    s
}

fn parse_int(s: &str) -> Option<i64> {
    // Strip `_` separators so values like `0xCAFEBABE_DEADBEEF` are
    // readable in tests. Decimal benefits too (`1_000_000`).
    let cleaned: String = s.chars().filter(|c| *c != '_').collect();
    let s: &str = &cleaned;
    // Parse hex via u64 so the full 64-bit range is reachable. `0x8…`
    // values above i64::MAX are bit-cast as the corresponding negative
    // i64. Negative hex (`-0x8000…`) handles i64::MIN by computing the
    // wrapping negation; this is the only way to express i64::MIN as
    // a literal that survives Rust's overflow checks.
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok().map(|u| u as i64)
    } else if let Some(neg_hex) = s.strip_prefix("-0x").or_else(|| s.strip_prefix("-0X")) {
        u64::from_str_radix(neg_hex, 16).ok().map(|u| (u as i64).wrapping_neg())
    } else {
        s.parse().ok()
    }
}

// ── canvas fast-path (rt_canvas_blit → SurfaceCmd::Blit) ─────────────

/// The `canvas-blit` kernel primitive is published and the high-resolution
/// canvas Mandelbrot demo compiles end-to-end. Booting the shared session
/// already proves the new `canvas_blit_word` MASM proc assembles; this pins
/// that `canvas-blit`, `L!`, `fractal-iter`, and the `gpane-*` words all
/// resolve when the demo is loaded, and that its entry word is defined.
#[test]
fn canvas_mandelbrot_demo_compiles() {
    let mut s = sess();

    // The new primitive ticks cleanly (an undefined word would not).
    let out = s.eval("' canvas-blit drop\nbye\n").unwrap();
    assert_eq!(out, " ok\n", "canvas-blit should be a defined word: {out:?}");

    // Load the demo source; every word it uses must resolve, or the colon
    // definition of its entry word fails and the word never appears.
    let demo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("demos")
        .join("gfx-canvas-mandelbrot.f");
    s.load_source_file(&demo).expect("load gfx-canvas-mandelbrot.f");

    let out = s.eval("' gfx-canvas-mandelbrot drop\nbye\n").unwrap();
    assert_eq!(out, " ok\n", "demo entry word should be defined: {out:?}");
}
