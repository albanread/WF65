//! LET — a small infix-algebraic DSL embedded in Forth.
//!
//! `LET ( in1, in2, ... ) -> ( out1, out2, ... ) = expr_list WHERE ... END`
//!
//! Compiles to a stand-alone Win64 function `(inputs*, outputs*) -> ()`
//! whose body lives in a fresh JIT module.  The Forth `LET` immediate
//! word reads source up to `END`, calls [`compile`], stores the resulting
//! function in the session so its code stays mapped, and emits a small
//! trampoline at HERE that loads the inputs from the Forth FP stack,
//! calls the compiled function, and pushes the outputs back.
//!
//! Scope of the MVP:
//! * Operators: `+ - * /` and unary `-`.
//! * Constants: `pi`, `e`, plus any numeric literals.
//! * WHERE bindings with dependency-ordered eval and cycle detection.
//! * Multiple inputs and multiple outputs.
//!
//! Not yet implemented (future work):
//! * Function calls (`sin`, `cos`, `sqrt`, `pow`, `hypot`, ...).
//! * `**` operator (needs `pow`).
//! * `select(cond, a, b)` / `IF/THEN/ELSE`.

pub mod parser;
pub mod codegen;

use std::collections::HashMap;

pub use parser::{LetError, LetForm};

/// Map from libm function name (e.g. "sin") to its absolute address in
/// the host process. Populated by the caller (typically the WF64 runtime)
/// before calling [`compile`], so the LET codegen can bake direct
/// `mov rax, addr ; call rax` sequences instead of relying on LLVM-MC
/// symbol resolution (which doesn't auto-find ucrtbase exports on
/// Windows MCJIT and is awkward to teach).
pub type LibmTable = HashMap<String, u64>;

/// Result of compiling one LET form to MC-flavour Intel asm text.
#[derive(Debug)]
pub struct CompiledLet {
    /// Function name as emitted into the asm. Must be unique per JIT module.
    pub fn_name: String,
    /// Asm source text ready for `Jit::add_asm`.
    pub asm_text: String,
    /// Number of inputs the function reads from `[rcx + i*8]`.
    pub n_inputs: usize,
    /// Number of outputs the function writes to `[rdx + i*8]` (with
    /// `outputs[0]` being the rightmost result = FP stack TOS).
    pub n_outputs: usize,
}

/// Parse and lower a LET form. Does not JIT-compile.
///
/// `libm_table`, if present, supplies absolute addresses for libm
/// functions the LET source may reference (sin, cos, sqrt, pow, ...).
/// Missing entries cause an error at lower time.  Pass an empty map
/// for LETs that don't use any libm functions.
pub fn compile(source: &str, fn_name: &str, libm_table: &LibmTable) -> Result<CompiledLet, LetError> {
    let form = parser::parse(source)?;
    let asm_text = codegen::lower(&form, fn_name, libm_table)?;
    Ok(CompiledLet {
        fn_name: fn_name.to_string(),
        asm_text,
        n_inputs: form.inputs.len(),
        n_outputs: form.outputs.len(),
    })
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use wfasm::Jit;

    /// Take a LET source, compile it, JIT-load it, then call it as a
    /// Win64 fn(*const f64, *mut f64) -> () and check the outputs.
    ///
    /// `inputs` and `expected` are in **Forth FP-stack order**: index 0
    /// is TOS (lowest address), index N-1 is the deepest cell. So if the
    /// LET signature is `(a, b, c) -> (...)`, the user pushes a, then b,
    /// then c — at call time the stack reads `[c, b, a]` and our test
    /// passes `&[c, b, a]`.
    fn run_let(source: &str, fn_name: &str, inputs: &[f64], expected: &[f64]) {
        // Get JASM's SEH dumper installed so access violations etc.
        // produce a readable register/stack dump instead of a silent exit.
        let _ = wfasm::seh::install();

        let libm = crate::runtime::libm_address_table();
        let compiled = compile(source, fn_name, &libm)
            .unwrap_or_else(|e| panic!("compile failed: {e}\nsource: {source}"));
        let mut jit = Jit::new(&format!("let_test_{fn_name}")).expect("Jit::new");
        jit.add_asm(&compiled.asm_text)
            .unwrap_or_else(|e| panic!("add_asm failed: {e:?}\nasm:\n{}", compiled.asm_text));
        // Declare the symbol in IR so MCJIT keeps it after link.
        jit.declare_fn(fn_name, 0).expect("declare_fn");
        let addr = jit
            .lookup_addr(fn_name)
            .unwrap_or_else(|e| panic!("lookup_addr failed: {e:?}\nasm:\n{}", compiled.asm_text));

        // Win64: rcx = inputs, rdx = outputs.
        let f: unsafe extern "system" fn(*const f64, *mut f64) = unsafe { std::mem::transmute(addr) };
        let mut outputs = vec![0.0_f64; compiled.n_outputs];
        unsafe { f(inputs.as_ptr(), outputs.as_mut_ptr()); }

        for (i, (got, want)) in outputs.iter().zip(expected.iter()).enumerate() {
            let diff = (got - want).abs();
            assert!(
                diff < 1e-9,
                "output[{i}]: got {got}, expected {want} (diff {diff})\nasm:\n{}",
                compiled.asm_text,
            );
        }
        // Keep the Jit alive until after we're done with the fn pointer.
        drop(jit);
    }

    #[test]
    fn jit_compiles_identity() {
        // Outputs convention: outputs[0] is the rightmost result.
        run_let("LET (x) -> (y) = x END", "let_id", &[42.0], &[42.0]);
    }

    #[test]
    fn jit_compiles_arithmetic() {
        run_let(
            "LET (x) -> (y) = x * x + 1 END",
            "let_quad",
            &[5.0],
            &[26.0],
        );
    }

    #[test]
    fn jit_compiles_area_of_circle() {
        run_let(
            "LET (r) -> (a) = pi * r * r END",
            "let_area_ic",
            &[2.0],
            &[std::f64::consts::PI * 4.0],
        );
    }

    #[test]
    fn jit_compiles_multi_input_multi_output() {
        // Forth: `10. 3. addsub`  pushes a=10 first then b=3, so memory
        // reads [b=3, a=10] = inputs `&[3.0, 10.0]` in our convention.
        // Outputs: declared (diff, sum); sum is last-declared so it ends
        // up at TOS → outputs `&[sum, diff]` = `&[13.0, 7.0]`.
        run_let(
            "LET (a, b) -> (diff, sum) = a - b, a + b END",
            "let_addsub",
            &[3.0, 10.0],     // [b=TOS, a=NOS]
            &[13.0, 7.0],     // [sum=TOS, diff=NOS]
        );
    }

    #[test]
    fn jit_compiles_mbrot_step() {
        // Forth call: `1. 1. 1. 1. mbrot`. Inputs in memory: [y, x, z_im, z_re].
        // Outputs declared (z_next_re, z_next_im, mag); mag is TOS.
        run_let(
            "LET (z_re, z_im, x, y) -> (z_next_re, z_next_im, mag) = \
                re, im, rmag \
                WHERE re   = z_re * z_re - z_im * z_im + x \
                WHERE im   = 2 * z_re * z_im + y \
                WHERE rmag = re * re + im * im \
             END",
            "let_mbrot",
            &[1.0, 1.0, 1.0, 1.0],
            // re   = 1 - 1 + 1 = 1
            // im   = 2 * 1 * 1 + 1 = 3
            // rmag = 1 + 9 = 10
            &[10.0, 3.0, 1.0],   // [mag, im, re]
        );
    }

    #[test]
    fn jit_compiles_unary_minus() {
        run_let("LET (x) -> (y) = -x END", "let_neg", &[7.5], &[-7.5]);
    }

    #[test]
    fn jit_compiles_negative_zero_handles_correctly() {
        // -0.0 is its own bit pattern; the sign-mask XOR should flip it.
        run_let("LET (x) -> (y) = -x END", "let_neg0", &[0.0], &[-0.0]);
    }

    #[test]
    fn jit_compiles_division() {
        // Forth: `100. 8. div`  → b=8 at TOS, a=100 at NOS.
        // memory order (TOS first): [b=8, a=100].
        run_let("LET (a, b) -> (q) = a / b END", "let_div", &[8.0, 100.0], &[12.5]);
    }

    // ── SSE intrinsics ───────────────────────────────────────────────

    #[test]
    fn jit_compiles_sqrt_intrinsic() {
        run_let("LET (x) -> (y) = sqrt(x) END", "let_sqrt", &[16.0], &[4.0]);
    }

    #[test]
    fn jit_compiles_abs_intrinsic() {
        run_let("LET (x) -> (y) = abs(x) END", "let_abs_neg", &[-3.5], &[3.5]);
        run_let("LET (x) -> (y) = abs(x) END", "let_abs_pos", &[7.25], &[7.25]);
    }

    #[test]
    fn jit_compiles_min_max_intrinsics() {
        // memory layout: [b, a]
        run_let("LET (a, b) -> (m) = min(a, b) END", "let_min", &[7.0, 3.0], &[3.0]);
        run_let("LET (a, b) -> (m) = max(a, b) END", "let_max", &[7.0, 3.0], &[7.0]);
    }

    #[test]
    fn jit_compiles_floor_ceil_round_trunc() {
        run_let("LET (x) -> (y) = floor(x) END", "let_floor", &[2.7], &[2.0]);
        run_let("LET (x) -> (y) = ceil(x)  END", "let_ceil",  &[2.3], &[3.0]);
        run_let("LET (x) -> (y) = round(x) END", "let_round", &[2.5], &[2.0]); // banker's: 2.5 → 2
        run_let("LET (x) -> (y) = trunc(x) END", "let_trunc", &[-2.7], &[-2.0]);
    }

    // ── libm functions ───────────────────────────────────────────────

    #[test]
    fn jit_compiles_sin_cos() {
        // sin(0) = 0, cos(0) = 1
        run_let("LET (x) -> (y) = sin(x) END", "let_sin_zero", &[0.0], &[0.0]);
        run_let("LET (x) -> (y) = cos(x) END", "let_cos_zero", &[0.0], &[1.0]);
    }

    #[test]
    fn jit_compiles_sin_of_pi_over_2() {
        // sin(pi/2) ≈ 1.0
        let half_pi = std::f64::consts::FRAC_PI_2;
        run_let("LET (x) -> (y) = sin(x) END", "let_sin_pi2", &[half_pi], &[1.0]);
    }

    #[test]
    fn jit_compiles_pow_via_libm() {
        run_let(
            "LET (b, e) -> (r) = pow(b, e) END",
            "let_pow_explicit",
            &[3.0, 2.0],  // [e=3, b=2] — e is TOS, b deeper
            &[8.0],       // 2^3 = 8
        );
    }

    #[test]
    fn jit_compiles_star_star_operator() {
        // ** is desugared to pow(lhs, rhs) by A-normal form.
        run_let(
            "LET (x) -> (y) = x ** 3 END",
            "let_cube_via_starstar",
            &[2.0],
            &[8.0],
        );
    }

    #[test]
    fn jit_compiles_hypot() {
        // hypot(3, 4) = 5
        run_let(
            "LET (a, b) -> (r) = hypot(a, b) END",
            "let_hypot",
            &[4.0, 3.0],  // [b=4, a=3] in memory
            &[5.0],
        );
    }

    #[test]
    fn libm_atan2_direct_callable() {
        // Sanity check: the address GetProcAddress gave us is in fact a
        // working atan2 — call it directly from Rust before involving the
        // JIT.  If THIS crashes too, the address itself is bogus and the
        // problem isn't in our codegen.
        let libm = crate::runtime::libm_address_table();
        let addr = *libm.get("atan2").expect("atan2 in libm table");
        let f: unsafe extern "system" fn(f64, f64) -> f64 = unsafe { std::mem::transmute(addr) };
        let r = unsafe { f(1.0, 1.0) };
        assert!((r - std::f64::consts::FRAC_PI_4).abs() < 1e-10, "got {r}");
    }

    #[test]
    fn jit_compiles_atan2() {
        // atan2(1, 1) = pi/4
        let pi_4 = std::f64::consts::FRAC_PI_4;
        run_let(
            "LET (y, x) -> (a) = atan2(y, x) END",
            "let_atan2",
            &[1.0, 1.0],
            &[pi_4],
        );
    }

    #[test]
    fn jit_compiles_exp_log_roundtrip() {
        // log(exp(x)) ≈ x
        run_let(
            "LET (x) -> (y) = log(exp(x)) END",
            "let_explog",
            &[2.0],
            &[2.0],
        );
    }

    #[test]
    fn jit_compiles_nested_calls_via_a_normal_form() {
        // sqrt(sin(x)*sin(x) + cos(x)*cos(x)) = 1.0
        run_let(
            "LET (x) -> (y) = sqrt(sin(x)*sin(x) + cos(x)*cos(x)) END",
            "let_sin2_plus_cos2",
            &[1.5],
            &[1.0],
        );
    }

    // ── Comparison operators and select ──────────────────────────────

    #[test]
    fn jit_compiles_less_than_yields_1_or_0() {
        run_let("LET (x) -> (y) = x < 5 END", "let_lt_true",  &[3.0], &[1.0]);
        run_let("LET (x) -> (y) = x < 5 END", "let_lt_false", &[7.0], &[0.0]);
        run_let("LET (x) -> (y) = x < 5 END", "let_lt_eq",    &[5.0], &[0.0]);
    }

    #[test]
    fn jit_compiles_all_comparison_operators() {
        run_let("LET (a, b) -> (y) = a == b END", "let_eq_t", &[3.0, 3.0], &[1.0]);
        run_let("LET (a, b) -> (y) = a == b END", "let_eq_f", &[3.0, 4.0], &[0.0]);
        run_let("LET (a, b) -> (y) = a != b END", "let_ne_t", &[3.0, 4.0], &[1.0]);
        // Inputs in memory: [b, a]. a=10, b=5 → "10 > 5" true.
        run_let("LET (a, b) -> (y) = a > b  END", "let_gt", &[5.0, 10.0], &[1.0]);
        run_let("LET (a, b) -> (y) = a >= b END", "let_ge_eq", &[3.0, 3.0], &[1.0]);
        run_let("LET (a, b) -> (y) = a <= b END", "let_le_eq", &[3.0, 3.0], &[1.0]);
    }

    #[test]
    fn jit_compiles_compare_result_as_arithmetic_value() {
        // (x < 0) * -1 + (x >= 0) * 1  ⇒ sign function returning ±1.
        run_let(
            "LET (x) -> (y) = (x < 0) * -1 + (x >= 0) * 1 END",
            "let_sign", &[5.0], &[1.0],
        );
        run_let(
            "LET (x) -> (y) = (x < 0) * -1 + (x >= 0) * 1 END",
            "let_sign_neg", &[-5.0], &[-1.0],
        );
    }

    #[test]
    fn jit_compiles_select_basic() {
        // select(cond, then, else): cond != 0 → then, else → else.
        run_let("LET () -> (y) = select(1, 99, 42) END", "let_sel_true",  &[], &[99.0]);
        run_let("LET () -> (y) = select(0, 99, 42) END", "let_sel_false", &[], &[42.0]);
    }

    #[test]
    fn jit_compiles_select_with_comparison() {
        // abs() implementable as select(x < 0, -x, x).
        run_let(
            "LET (x) -> (y) = select(x < 0, -x, x) END",
            "let_abs_via_select_pos", &[3.5], &[3.5],
        );
        run_let(
            "LET (x) -> (y) = select(x < 0, -x, x) END",
            "let_abs_via_select_neg", &[-3.5], &[3.5],
        );
    }

    #[test]
    fn jit_compiles_clamp_via_nested_select() {
        // clamp(x, lo, hi). Inputs in memory: [hi, lo, x] (TOS first).
        // x clamped to [lo, hi]: select(x < lo, lo, select(x > hi, hi, x))
        let src = "LET (x, lo, hi) -> (y) = \
                       select(x < lo, lo, select(x > hi, hi, x)) END";
        run_let(src, "let_clamp_in",    &[10.0, 0.0, 5.0],  &[5.0]);   // x=5 in range
        run_let(src, "let_clamp_below", &[10.0, 0.0, -3.0], &[0.0]);   // x=-3 → lo=0
        run_let(src, "let_clamp_above", &[10.0, 0.0, 99.0], &[10.0]);  // x=99 → hi=10
    }

    #[test]
    fn jit_compiles_smoothstep_via_select() {
        // smoothstep(0, 1, t) = clamped t * t * (3 - 2*t).
        let src = "LET (t) -> (y) = \
                       u * u * (3 - 2 * u) \
                       WHERE u = select(t < 0, 0, select(t > 1, 1, t)) \
                   END";
        run_let(src, "let_smooth_low",  &[-1.0], &[0.0]);
        run_let(src, "let_smooth_mid",  &[0.5],  &[0.5]);     // 0.25 * 2 = 0.5
        run_let(src, "let_smooth_high", &[2.0],  &[1.0]);
    }

    #[test]
    fn jit_compiles_distance_2d() {
        // The point-distance example from the spec — Euclidean distance via hypot.
        // Inputs in memory: [y2, x2, y1, x1].
        run_let(
            "LET (x1, y1, x2, y2) -> (d) = hypot(x2 - x1, y2 - y1) END",
            "let_dist",
            &[200.0, 200.0, 100.0, 100.0],
            &[141.4213562373095],
        );
    }
}

