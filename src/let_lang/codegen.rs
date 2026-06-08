//! LET codegen: AST → raw MC-flavour Intel asm text.
//!
//! Calling convention (Win64):
//!   rcx = inputs pointer  — the i-th DECLARED input is read from
//!         [rcx + (n_in - 1 - i) * 8].  This matches Forth FP-stack
//!         layout: the last-declared input is TOS (lowest address),
//!         the first-declared input is the deepest stack cell.
//!   rdx = outputs pointer — the i-th DECLARED output is written to
//!         [rdx + (n_out - 1 - i) * 8].  Again matches Forth: the
//!         last-declared output ends up at TOS after the call.
//!
//! Register policy:
//!   xmm6..xmm15 — NAMED values (inputs + WHERE bindings).  These are
//!                 callee-saved per Win64 ABI, so any libm call we emit
//!                 inside the function body leaves them intact.
//!   xmm0..xmm5  — scratch for sub-expression evaluation.  Caller-saved;
//!                 libm calls clobber them, which is fine because the
//!                 A-normal-form pre-pass guarantees no scratch is live
//!                 across a call.
//!   r12         — holds the outputs pointer across the body.  rdx
//!                 (which is how Win64 passes it to us) is caller-
//!                 saved and gets clobbered by libm calls, so we copy
//!                 it into r12 at entry and use that for every output
//!                 store.  r12 is itself callee-saved; we push/pop it
//!                 to keep our own callers happy.
//!
//! Function calls are pre-lifted into synthetic WHERE bindings so each
//! call's body is `arg_setup; call; move_result`, with no sub-expression
//! partial values live in caller-saved xmm regs when the call fires.
//!
//! All computation lives in xmm0..xmm15. Each "named" value (input or
//! WHERE binding) gets a fixed register for the duration of the function;
//! sub-expression evaluation uses the remaining xmm regs as scratch.
//!
//! For an expression of depth d, peak register pressure is
//! `(named regs) + d`. With 16 xmm regs and typical LETs having ~3-6
//! named values, we comfortably handle expressions up to ~10 deep.

use std::collections::HashMap;
use super::parser::{LetForm, Expr, BinOp};
use super::{LetError, LibmTable};

const TOTAL_XMM: u8 = 16;
/// Named values live at xmm6+ (callee-saved per Win64), scratch at xmm0..5.
const FIRST_NAMED_REG: u8 = 6;
const FIRST_SCRATCH_REG: u8 = 0;
const SCRATCH_COUNT: u8 = 6;

/// Kind of a function call seen in the LET source.
#[derive(Clone, Copy)]
enum FnKind {
    /// One-instruction SSE intrinsic that doesn't need a libm call.
    Intrinsic(IntrinsicKind),
    /// Resolved via dynamic linking — MCJIT looks the symbol up at
    /// finalize time through the runtime memory manager (which on
    /// Windows finds ucrtbase.dll's exports automatically).
    Libm { arity: usize, symbol: &'static str },
    /// `select(cond, then, else)` — branchless conditional via cmpsd +
    /// andpd/andnpd/orpd blend.  Special-cased because it's 3-arg and
    /// the codegen pattern is different from other functions.
    Select,
}

#[derive(Clone, Copy)]
enum IntrinsicKind {
    Sqrt,   // sqrtsd
    Abs,    // andpd with sign-mask clear
    Min,    // minsd
    Max,    // maxsd
    Floor,  // roundsd imm 1
    Ceil,   // roundsd imm 2
    Round,  // roundsd imm 0
    Trunc,  // roundsd imm 3
}

fn classify_function(name: &str) -> Option<FnKind> {
    use FnKind::*;
    use IntrinsicKind::*;
    let intr = |k| Some(Intrinsic(k));
    match name {
        "sqrt"  => intr(Sqrt),
        "abs"   => intr(Abs),
        "fabs"  => intr(Abs),       // C math alias
        "min"   => intr(Min),
        "max"   => intr(Max),
        "floor" => intr(Floor),
        "ceil"  => intr(Ceil),
        "round" => intr(Round),
        "trunc" => intr(Trunc),
        "sin"   => Some(Libm { arity: 1, symbol: "sin" }),
        "cos"   => Some(Libm { arity: 1, symbol: "cos" }),
        "tan"   => Some(Libm { arity: 1, symbol: "tan" }),
        "asin"  => Some(Libm { arity: 1, symbol: "asin" }),
        "acos"  => Some(Libm { arity: 1, symbol: "acos" }),
        "atan"  => Some(Libm { arity: 1, symbol: "atan" }),
        "exp"   => Some(Libm { arity: 1, symbol: "exp" }),
        "log"   => Some(Libm { arity: 1, symbol: "log" }),
        "log2"  => Some(Libm { arity: 1, symbol: "log2" }),
        "log10" => Some(Libm { arity: 1, symbol: "log10" }),
        "atan2" => Some(Libm { arity: 2, symbol: "atan2" }),
        "pow"   => Some(Libm { arity: 2, symbol: "pow" }),
        "hypot" => Some(Libm { arity: 2, symbol: "hypot" }),
        "fmod"  => Some(Libm { arity: 2, symbol: "fmod" }),
        "select" => Some(Select),
        _ => None,
    }
}

fn intrinsic_arity(k: IntrinsicKind) -> usize {
    use IntrinsicKind::*;
    match k {
        Min | Max => 2,
        Sqrt | Abs | Floor | Ceil | Round | Trunc => 1,
    }
}

pub fn lower(form: &LetForm, fn_name: &str, libm_table: &LibmTable) -> Result<String, LetError> {
    // 1. Resolve every Var reference / Call name.
    for e in &form.results { validate_expr(e, form)?; }
    for (_, e) in &form.wheres { validate_expr(e, form)?; }

    // 2. A-normal form pre-pass: lift every Call out of nested
    //    contexts into a fresh WHERE binding.  After this transform,
    //    every Call appears only as the RHS of a binding, so when we
    //    emit the call we can use xmm0 (and xmm1) freely without
    //    worrying about a partial sub-expression value living there.
    let mut form = clone_form(form);
    a_normal_form(&mut form);

    // 3. Topo-sort WHERE bindings (now including the lifted calls).
    let order = topo_sort_wheres(&form.wheres)?;

    // 4. Assign one xmm reg per named value (input + WHERE binding).
    //    Named values live at xmm6+ so libm calls (which clobber
    //    xmm0..xmm5 per Win64) leave them intact.
    let mut vars: HashMap<String, u8> = HashMap::new();
    let mut next: u8 = FIRST_NAMED_REG;
    for input in &form.inputs {
        vars.insert(input.clone(), next);
        next += 1;
    }
    for &i in &order {
        vars.insert(form.wheres[i].0.clone(), next);
        next += 1;
    }
    if next > TOTAL_XMM {
        return Err(LetError {
            message: format!(
                "LET has {} named values; supported maximum is {} (xmm{}..xmm{})",
                next - FIRST_NAMED_REG, TOTAL_XMM - FIRST_NAMED_REG,
                FIRST_NAMED_REG, TOTAL_XMM - 1,
            ),
            pos: 0,
        });
    }

    // 4. Emit.
    let mut s = String::new();
    let mut const_pool: Vec<u64> = Vec::new();   // raw bits, dedup by bit-equal

    // MC defaults to AT&T syntax on x86_64; switch on Intel.
    s.push_str("    .intel_syntax noprefix\n");
    s.push_str("    .text\n");
    s.push_str(&format!("    .globl {fn_name}\n"));
    s.push_str(&format!("{fn_name}:\n"));

    // Prologue: save r12 (callee-saved) and stash the outputs pointer
    // there.  libm calls below will clobber rdx (caller-saved), but
    // not r12, so output stores can use [r12 + off] reliably.
    // The push also shifts rsp by 8: entry rsp ≡ 8 (mod 16), so after
    // push r12 rsp ≡ 0 (mod 16), which is what the libm-call prologue
    // assumes when it sub-by-32 (no extra alignment pad needed).
    s.push_str("    push r12\n");
    s.push_str("    mov r12, rdx\n");

    // Load inputs.  Declared input `i` lives at [rcx + (n_in-1-i)*8]
    // because the Forth FP stack puts the LAST-declared input at TOS
    // (lowest address) and the first-declared input deepest.
    let n_in = form.inputs.len();
    for (i, name) in form.inputs.iter().enumerate() {
        let r = vars[name];
        let off = (n_in - 1 - i) * 8;
        s.push_str(&format!(
            "    movsd xmm{r}, qword ptr [rcx + {off}]\n",
        ));
    }

    // Emit WHERE bindings in dependency order.  After A-normal form,
    // each binding's RHS is either a Call (handle specially) or a
    // call-free expression (recurse with scratch starting at xmm0).
    for &i in &order {
        let (name, expr) = &form.wheres[i];
        let target = vars[name];
        match expr {
            Expr::Call(name, args) => {
                emit_call(name, args, target, &vars, &mut const_pool, libm_table, fn_name, &mut s)?;
            }
            _ => {
                emit_expr(expr, target, &vars, &mut const_pool, FIRST_SCRATCH_REG, fn_name, &mut s)?;
            }
        }
    }

    // Emit results.  Each result expression is call-free after the
    // A-normal-form pass (every Call became a binding).  We evaluate
    // into xmm0 (first scratch) then store to outputs.
    let n_out = form.results.len();
    let scratch_for_result = FIRST_SCRATCH_REG;
    for (i, expr) in form.results.iter().enumerate() {
        emit_expr(expr, scratch_for_result, &vars, &mut const_pool, FIRST_SCRATCH_REG + 1, fn_name, &mut s)?;
        let off = (n_out - 1 - i) * 8;
        // Output store via r12 (the saved outputs ptr), not rdx — which
        // was clobbered by any libm call in the body above.
        s.push_str(&format!(
            "    movsd qword ptr [r12 + {off}], xmm{scratch_for_result}\n",
        ));
    }

    // Epilogue: restore r12 (the trampoline expects it preserved) and
    // return to the caller.
    s.push_str("    pop r12\n");
    s.push_str("    ret\n");

    // Constant pool (rodata-style, but the assembler emits it into .text
    // alongside the code — which is fine for MCJIT, the bytes are
    // executable-AND-readable in our scheme).
    if !const_pool.is_empty() {
        s.push_str("    .p2align 3\n");
        for (i, bits) in const_pool.iter().enumerate() {
            let f = f64::from_bits(*bits);
            s.push_str(&format!(
                "{fn_name}$$const_{i}: .quad 0x{:016X}    # {f}\n", bits,
            ));
        }
    }
    // Bitmasks used by the codegen: sign-bit for unary negate,
    // abs-mask for fabs (sign cleared), and one_bits (low qword = the
    // IEEE-754 bit pattern of 1.0) for converting compare-mask results
    // to a real 1.0 / 0.0 value.  All three are 16 bytes so the SSE
    // logical ops can operate on them as xmmword memory operands.
    s.push_str("    .p2align 4\n");
    s.push_str(&format!(
        "{fn_name}$$sign_mask: .quad 0x8000000000000000, 0x0000000000000000\n",
    ));
    s.push_str("    .p2align 4\n");
    s.push_str(&format!(
        "{fn_name}$$abs_mask:  .quad 0x7FFFFFFFFFFFFFFF, 0xFFFFFFFFFFFFFFFF\n",
    ));
    s.push_str("    .p2align 4\n");
    s.push_str(&format!(
        "{fn_name}$$one_bits:  .quad 0x3FF0000000000000, 0x0000000000000000\n",
    ));

    Ok(s)
}

/// Emit code that leaves `expr`'s value in xmm{target}.
/// `next_scratch` is the first xmm reg index available for sub-expression
/// scratch (guaranteed not to alias any named value).
fn emit_expr(
    expr: &Expr,
    target: u8,
    vars: &HashMap<String, u8>,
    const_pool: &mut Vec<u64>,
    next_scratch: u8,
    fn_name: &str,
    out: &mut String,
) -> Result<(), LetError> {
    match expr {
        Expr::Lit(n) => {
            emit_load_const(target, *n, const_pool, fn_name, out);
        }
        Expr::Var(name) => {
            // Local bindings (inputs + WHEREs) win over built-in constants:
            // if a LET shadows `pi` or `e` with its own binding name, the
            // binding should be used.
            if let Some(&src) = vars.get(name) {
                if src != target {
                    out.push_str(&format!("    movsd xmm{target}, xmm{src}\n"));
                }
            } else if let Some(c) = known_constant(name) {
                emit_load_const(target, c, const_pool, fn_name, out);
            } else {
                return Err(LetError {
                    message: format!("undefined name '{name}'"),
                    pos: 0,
                });
            }
        }
        Expr::Bin(op, l, r) => {
            // LHS into target, RHS into next_scratch, then combine.
            // ** is desugared to pow(lhs, rhs) by the A-normal-form
            // pass, so a Pow binop reaching here is a bug.
            if next_scratch >= FIRST_SCRATCH_REG + SCRATCH_COUNT {
                return Err(LetError {
                    message: "LET expression too deep for caller-saved scratch registers (xmm0..xmm5)".into(),
                    pos: 0,
                });
            }
            // Comparison ops `>` / `>=` have no direct SSE encoding;
            // we swap operands and use the corresponding `<` / `<=`.
            let (l_first, r_first, eff_op) = match op {
                BinOp::Gt => (r, l, BinOp::Lt),
                BinOp::Ge => (r, l, BinOp::Le),
                _ => (l, r, *op),
            };
            emit_expr(l_first, target, vars, const_pool, next_scratch, fn_name, out)?;
            emit_expr(r_first, next_scratch, vars, const_pool, next_scratch + 1, fn_name, out)?;
            match eff_op {
                BinOp::Add => out.push_str(&format!("    addsd xmm{target}, xmm{next_scratch}\n")),
                BinOp::Sub => out.push_str(&format!("    subsd xmm{target}, xmm{next_scratch}\n")),
                BinOp::Mul => out.push_str(&format!("    mulsd xmm{target}, xmm{next_scratch}\n")),
                BinOp::Div => out.push_str(&format!("    divsd xmm{target}, xmm{next_scratch}\n")),
                BinOp::Pow => unreachable!("** should have been desugared by A-normal form"),
                // Comparisons: cmpCCsd target, next_scratch produces a
                // mask (all-1s if true else all-0s) in target's low 64
                // bits. We then `andpd` with a constant whose low qword
                // is the bit pattern of 1.0 to convert the mask into
                // 1.0 (true) or 0.0 (false) — a real numeric value the
                // user can flow into arithmetic or feed to `select`.
                BinOp::Eq => out.push_str(&format!("    cmpeqsd xmm{target}, xmm{next_scratch}\n")),
                BinOp::Ne => out.push_str(&format!("    cmpneqsd xmm{target}, xmm{next_scratch}\n")),
                BinOp::Lt => out.push_str(&format!("    cmpltsd xmm{target}, xmm{next_scratch}\n")),
                BinOp::Le => out.push_str(&format!("    cmplesd xmm{target}, xmm{next_scratch}\n")),
                BinOp::Gt | BinOp::Ge => unreachable!("rewritten via operand swap above"),
            }
            if matches!(eff_op, BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le) {
                out.push_str(&format!(
                    "    andpd xmm{target}, xmmword ptr [rip + {fn_name}$$one_bits]\n",
                ));
            }
        }
        Expr::Neg(e) => {
            emit_expr(e, target, vars, const_pool, next_scratch, fn_name, out)?;
            out.push_str(&format!(
                "    xorpd xmm{target}, xmmword ptr [rip + {fn_name}$$sign_mask]\n",
            ));
        }
        Expr::Call(name, _) => {
            // After A-normal form, calls only appear as a binding's RHS,
            // which the WHERE-binding loop handles via emit_call.
            return Err(LetError {
                message: format!("internal: unexpected Call('{name}') in expression position after A-normal form"),
                pos: 0,
            });
        }
    }
    Ok(())
}

/// Emit a function call binding (post A-normal form, args are always
/// Var or Lit so we can load them directly into xmm0/xmm1 without
/// further sub-expression evaluation).
fn emit_call(
    name: &str,
    args: &[Expr],
    target: u8,
    vars: &HashMap<String, u8>,
    const_pool: &mut Vec<u64>,
    libm_table: &LibmTable,
    fn_name: &str,
    out: &mut String,
) -> Result<(), LetError> {
    let kind = classify_function(name).ok_or_else(|| LetError {
        message: format!("unknown function '{name}'"),
        pos: 0,
    })?;
    let arity = match kind {
        FnKind::Intrinsic(k) => intrinsic_arity(k),
        FnKind::Libm { arity, .. } => arity,
        FnKind::Select => 3,
    };
    if args.len() != arity {
        return Err(LetError {
            message: format!(
                "'{name}' takes {arity} argument(s), got {}",
                args.len()
            ),
            pos: 0,
        });
    }

    match kind {
        FnKind::Intrinsic(k) => emit_intrinsic(k, args, target, vars, const_pool, fn_name, out),
        FnKind::Libm { symbol, .. } => {
            let addr = libm_table.get(symbol).copied().ok_or_else(|| LetError {
                message: format!(
                    "libm function '{symbol}' is not in this build's address table; \
                     check that the WF64 runtime resolved it via GetProcAddress \
                     (typical cause: typo in LIBM_FUNCTIONS or a stale ucrtbase build)"
                ),
                pos: 0,
            })?;
            emit_libm_call(symbol, addr, args, target, vars, const_pool, fn_name, out)
        }
        FnKind::Select => emit_select(args, target, vars, const_pool, fn_name, out),
    }
}

/// Materialise a leaf argument (Var, Lit, or a previously-bound Call
/// result reached via Var) into the given xmm register.  Post A-normal
/// form, args are never compound expressions.
fn load_leaf_into(
    arg: &Expr,
    target: u8,
    vars: &HashMap<String, u8>,
    const_pool: &mut Vec<u64>,
    fn_name: &str,
    out: &mut String,
) -> Result<(), LetError> {
    match arg {
        Expr::Lit(v) => {
            emit_load_const(target, *v, const_pool, fn_name, out);
            Ok(())
        }
        Expr::Var(name) => {
            // Same precedence as emit_expr: local bindings shadow
            // built-in constants.
            if let Some(&src) = vars.get(name) {
                if src != target {
                    out.push_str(&format!("    movsd xmm{target}, xmm{src}\n"));
                }
                Ok(())
            } else if let Some(c) = known_constant(name) {
                emit_load_const(target, c, const_pool, fn_name, out);
                Ok(())
            } else {
                Err(LetError {
                    message: format!("undefined name '{name}'"),
                    pos: 0,
                })
            }
        }
        _ => Err(LetError {
            message: "internal: A-normal form should have lifted nested expressions out of call args".into(),
            pos: 0,
        }),
    }
}

/// Emit an SSE intrinsic call (single instruction, no libm needed).
fn emit_intrinsic(
    k: IntrinsicKind,
    args: &[Expr],
    target: u8,
    vars: &HashMap<String, u8>,
    const_pool: &mut Vec<u64>,
    fn_name: &str,
    out: &mut String,
) -> Result<(), LetError> {
    use IntrinsicKind::*;
    match k {
        Sqrt => {
            // Load arg into target, sqrtsd in place.
            load_leaf_into(&args[0], target, vars, const_pool, fn_name, out)?;
            out.push_str(&format!("    sqrtsd xmm{target}, xmm{target}\n"));
        }
        Abs => {
            // abs(x) = x with sign bit cleared.
            load_leaf_into(&args[0], target, vars, const_pool, fn_name, out)?;
            out.push_str(&format!(
                "    andpd xmm{target}, xmmword ptr [rip + {fn_name}$$abs_mask]\n",
            ));
        }
        Min | Max => {
            // Load arg0 into target, arg1 into a temp scratch, then
            // minsd/maxsd target, scratch.
            load_leaf_into(&args[0], target, vars, const_pool, fn_name, out)?;
            // Pick a scratch reg that isn't target.
            let tmp = if target == FIRST_SCRATCH_REG {
                FIRST_SCRATCH_REG + 1
            } else {
                FIRST_SCRATCH_REG
            };
            load_leaf_into(&args[1], tmp, vars, const_pool, fn_name, out)?;
            let m = match k { Min => "minsd", Max => "maxsd", _ => unreachable!() };
            out.push_str(&format!("    {m} xmm{target}, xmm{tmp}\n"));
        }
        Floor | Ceil | Round | Trunc => {
            // roundsd target, src, mode  (SSE 4.1).
            // Modes: 0=nearest, 1=floor, 2=ceil, 3=truncate.
            let mode: u8 = match k {
                Round => 0,
                Floor => 1,
                Ceil  => 2,
                Trunc => 3,
                _ => unreachable!(),
            };
            load_leaf_into(&args[0], target, vars, const_pool, fn_name, out)?;
            out.push_str(&format!(
                "    roundsd xmm{target}, xmm{target}, {mode}\n",
            ));
        }
    }
    Ok(())
}

/// Emit a libm call as `mov rax, <abs_addr>; call rax`. Baking the
/// absolute address in avoids depending on MCJIT's symbol resolver,
/// which doesn't auto-find ucrtbase.dll exports on Windows.
fn emit_libm_call(
    symbol: &str,
    addr: u64,
    args: &[Expr],
    target: u8,
    vars: &HashMap<String, u8>,
    const_pool: &mut Vec<u64>,
    fn_name: &str,
    out: &mut String,
) -> Result<(), LetError> {
    // Load args into xmm0 (and xmm1 for 2-arg calls) — Win64 passes
    // the first 4 doubles in xmm0..xmm3.  Two-arg case: load arg1
    // FIRST (into xmm1) so arg0 isn't clobbered by anything in arg1.
    if args.len() == 2 {
        load_leaf_into(&args[1], 1, vars, const_pool, fn_name, out)?;
    }
    load_leaf_into(&args[0], 0, vars, const_pool, fn_name, out)?;

    // Win64 alignment: at entry to this LET function rsp ≡ 8 (mod 16);
    // the prologue `push r12` made rsp ≡ 0 (mod 16).  We need rsp to be
    // 0 (mod 16) at the CALL instruction, so just sub 32 for shadow
    // space (no extra alignment pad).  Use movabs so MC unambiguously
    // emits the imm64 form — libm addresses are above 2^32 on Windows
    // and an ordinary `mov rax, imm` can be encoded as a sign-extended
    // imm32, which would silently truncate the address.
    out.push_str("    sub rsp, 32\n");
    out.push_str(&format!(
        "    movabs rax, 0x{addr:x}        # &{symbol}\n",
    ));
    out.push_str("    call rax\n");
    out.push_str("    add rsp, 32\n");

    // Result is in xmm0.  Move to target if different.
    if target != 0 {
        out.push_str(&format!("    movsd xmm{target}, xmm0\n"));
    }
    Ok(())
}

/// Emit `select(cond, then, else)` as a branchless blend.
///
/// `cond` arrives as a numeric value where 0.0 means false and anything
/// else means true (so it composes naturally with comparison ops, which
/// produce 0.0/1.0, and with arithmetic flags users build by hand).
///
/// Strategy:
///   1. Load cond into a scratch reg, materialise the mask
///      `mask = (cond != 0)` via `cmpneqsd cond_reg, zero`.
///      Now cond_reg's low qword is all-1s if true, all-0s if false.
///   2. Load `then` and `else` into other scratch regs.
///   3. Compute `target = (mask & then) | (~mask & else)`:
///        movsd target, mask
///        andnpd target, else        ; target = ~mask & else
///        andpd  mask,   then        ; mask   =  mask & then
///        orpd   target, mask        ; target = blended result
///
/// All three intermediate regs live in xmm0..xmm5 (scratch), so this
/// safely composes with the surrounding A-normal-form binding pattern.
fn emit_select(
    args: &[Expr],
    target: u8,
    vars: &HashMap<String, u8>,
    const_pool: &mut Vec<u64>,
    fn_name: &str,
    out: &mut String,
) -> Result<(), LetError> {
    // Pin three caller-saved scratch regs for cond, then, else.  These
    // never collide with named values (xmm6+) and we don't need to
    // worry about them surviving any call — emit_select is leaf code.
    let r_cond = FIRST_SCRATCH_REG;
    let r_then = FIRST_SCRATCH_REG + 1;
    let r_else = FIRST_SCRATCH_REG + 2;
    let r_zero = FIRST_SCRATCH_REG + 3;

    load_leaf_into(&args[0], r_cond, vars, const_pool, fn_name, out)?;
    load_leaf_into(&args[1], r_then, vars, const_pool, fn_name, out)?;
    load_leaf_into(&args[2], r_else, vars, const_pool, fn_name, out)?;

    // Materialise 0.0 in r_zero so we can compare against it.  Use
    // xorpd self,self — clears both 64-bit lanes — instead of loading
    // a constant; saves a memory access.
    out.push_str(&format!("    xorpd xmm{r_zero}, xmm{r_zero}\n"));
    // mask = (cond != 0)
    out.push_str(&format!("    cmpneqsd xmm{r_cond}, xmm{r_zero}\n"));
    // target = mask  (we'll andn with else, then or with masked then)
    out.push_str(&format!("    movsd xmm{target}, xmm{r_cond}\n"));
    out.push_str(&format!("    andnpd xmm{target}, xmm{r_else}\n"));
    out.push_str(&format!("    andpd  xmm{r_cond}, xmm{r_then}\n"));
    out.push_str(&format!("    orpd   xmm{target}, xmm{r_cond}\n"));
    Ok(())
}

fn emit_load_const(target: u8, value: f64, pool: &mut Vec<u64>, fn_name: &str, out: &mut String) {
    let bits = value.to_bits();
    let idx = pool.iter().position(|&b| b == bits).unwrap_or_else(|| {
        pool.push(bits);
        pool.len() - 1
    });
    out.push_str(&format!(
        "    movsd xmm{target}, qword ptr [rip + {fn_name}$$const_{idx}]\n",
    ));
}

fn known_constant(name: &str) -> Option<f64> {
    match name {
        "pi" => Some(std::f64::consts::PI),
        "e"  => Some(std::f64::consts::E),
        _    => None,
    }
}

fn validate_expr(expr: &Expr, form: &LetForm) -> Result<(), LetError> {
    match expr {
        Expr::Lit(_) => Ok(()),
        Expr::Var(name) => {
            if known_constant(name).is_some()
                || form.inputs.iter().any(|n| n == name)
                || form.wheres.iter().any(|(n, _)| n == name)
            {
                Ok(())
            } else {
                Err(LetError {
                    message: format!("undefined name '{name}'"),
                    pos: 0,
                })
            }
        }
        Expr::Bin(_, l, r) => { validate_expr(l, form)?; validate_expr(r, form) }
        Expr::Neg(e) => validate_expr(e, form),
        Expr::Call(name, args) => {
            let kind = classify_function(name).ok_or_else(|| LetError {
                message: format!("unknown function '{name}'"),
                pos: 0,
            })?;
            let arity = match kind {
                FnKind::Intrinsic(k) => intrinsic_arity(k),
                FnKind::Libm { arity, .. } => arity,
                FnKind::Select => 3,
            };
            if args.len() != arity {
                return Err(LetError {
                    message: format!("'{name}' takes {arity} argument(s), got {}", args.len()),
                    pos: 0,
                });
            }
            for a in args { validate_expr(a, form)?; }
            Ok(())
        }
    }
}

/// Deep-clone a LetForm so we can run destructive transforms on it.
fn clone_form(form: &LetForm) -> LetForm {
    LetForm {
        inputs:  form.inputs.clone(),
        outputs: form.outputs.clone(),
        results: form.results.iter().map(clone_expr).collect(),
        wheres:  form.wheres.iter().map(|(n, e)| (n.clone(), clone_expr(e))).collect(),
    }
}

fn clone_expr(e: &Expr) -> Expr {
    match e {
        Expr::Lit(v) => Expr::Lit(*v),
        Expr::Var(n) => Expr::Var(n.clone()),
        Expr::Bin(op, l, r) => Expr::Bin(*op, Box::new(clone_expr(l)), Box::new(clone_expr(r))),
        Expr::Neg(e) => Expr::Neg(Box::new(clone_expr(e))),
        Expr::Call(n, a) => Expr::Call(n.clone(), a.iter().map(clone_expr).collect()),
    }
}

/// A-normal form pre-pass.  Two transformations:
///
/// 1. `**` (Pow binop) is rewritten to `pow(lhs, rhs)`, since the
///    intrinsic + libm machinery handles function calls but no
///    immediate-form instruction exists for x86 floating-point power.
/// 2. Every `Call(f, args)` that appears inside a Bin / Neg / argument
///    position is lifted to a fresh WHERE binding `_call_NNN`, with
///    the Call replaced by `Var(_call_NNN)` in place.  After this,
///    every Call lives only as the RHS of a binding — its argument
///    positions are guaranteed to be Lit or Var (specifically, post-
///    lift the args may be Vars that resolve to OTHER lifted call
///    results — that's fine, topo sort orders them).
fn a_normal_form(form: &mut LetForm) {
    // First, desugar ** to pow() everywhere.
    for (_, e) in form.wheres.iter_mut() { desugar_pow(e); }
    for e in form.results.iter_mut()     { desugar_pow(e); }

    // Then lift calls.  We need to walk every existing binding's RHS
    // and every result expression, collecting newly-synthesised
    // bindings as we go.
    let mut counter: usize = 0;
    let mut new_bindings: Vec<(String, Expr)> = Vec::new();

    // Process existing wheres in place.
    let original_wheres: Vec<(String, Expr)> = std::mem::take(&mut form.wheres);
    for (name, expr) in original_wheres {
        let lifted = lift_calls(expr, &mut new_bindings, &mut counter, /* at_binding_root */ true);
        form.wheres.push((name, lifted));
    }
    // Process results.
    let original_results: Vec<Expr> = std::mem::take(&mut form.results);
    for expr in original_results {
        let lifted = lift_calls(expr, &mut new_bindings, &mut counter, /* at_binding_root */ false);
        form.results.push(lifted);
    }
    // Append the lifted bindings.  Topo sort in lower() handles the
    // dependency ordering — _call_NNN bindings reference inputs and/or
    // earlier _call_MMM, exactly the same way user WHEREs do.
    form.wheres.extend(new_bindings);
}

fn desugar_pow(e: &mut Expr) {
    match e {
        Expr::Lit(_) | Expr::Var(_) => {}
        Expr::Bin(op, l, r) => {
            desugar_pow(l);
            desugar_pow(r);
            if *op == BinOp::Pow {
                let lhs = std::mem::replace(l.as_mut(), Expr::Lit(0.0));
                let rhs = std::mem::replace(r.as_mut(), Expr::Lit(0.0));
                *e = Expr::Call("pow".to_string(), vec![lhs, rhs]);
            }
        }
        Expr::Neg(inner) => desugar_pow(inner),
        Expr::Call(_, args) => for a in args { desugar_pow(a); },
    }
}

/// Walk `e` and lift every nested Call into a fresh binding.  If
/// `at_binding_root` is true AND the top-level node is a Call, leave
/// it in place (it IS already a binding's RHS); recurse into its args.
fn lift_calls(
    e: Expr,
    new_bindings: &mut Vec<(String, Expr)>,
    counter: &mut usize,
    at_binding_root: bool,
) -> Expr {
    match e {
        Expr::Lit(_) | Expr::Var(_) => e,
        Expr::Bin(op, l, r) => {
            let l = lift_calls(*l, new_bindings, counter, false);
            let r = lift_calls(*r, new_bindings, counter, false);
            Expr::Bin(op, Box::new(l), Box::new(r))
        }
        Expr::Neg(inner) => Expr::Neg(Box::new(lift_calls(*inner, new_bindings, counter, false))),
        Expr::Call(name, args) => {
            // Lift args first.  Each arg is in non-root position so
            // any Call inside it gets lifted.
            let new_args: Vec<Expr> = args
                .into_iter()
                .map(|a| lift_calls(a, new_bindings, counter, false))
                .collect();
            // Ensure args are now leaves (Var or Lit).  If any is still
            // a Bin/Neg, lift it too — emit_call requires Var/Lit leaves
            // because the Win64 calling convention shoves args directly
            // into xmm0/xmm1.
            let leaf_args: Vec<Expr> = new_args
                .into_iter()
                .map(|a| match a {
                    Expr::Lit(_) | Expr::Var(_) => a,
                    other => {
                        *counter += 1;
                        let fresh = format!("_arg_{:04}", counter);
                        new_bindings.push((fresh.clone(), other));
                        Expr::Var(fresh)
                    }
                })
                .collect();

            let call = Expr::Call(name, leaf_args);
            if at_binding_root {
                call
            } else {
                *counter += 1;
                let fresh = format!("_call_{:04}", counter);
                new_bindings.push((fresh.clone(), call));
                Expr::Var(fresh)
            }
        }
    }
}

fn topo_sort_wheres(wheres: &[(String, Expr)]) -> Result<Vec<usize>, LetError> {
    let n = wheres.len();
    let mut name_to_idx: HashMap<&str, usize> = HashMap::new();
    for (i, (name, _)) in wheres.iter().enumerate() {
        if name_to_idx.insert(name.as_str(), i).is_some() {
            return Err(LetError {
                message: format!("duplicate WHERE binding '{name}'"),
                pos: 0,
            });
        }
    }
    let mut deps: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, (_, expr)) in wheres.iter().enumerate() {
        collect_deps(expr, &name_to_idx, &mut deps[i]);
    }
    // Kahn's. in_deg[i] = how many other bindings i depends on.
    let mut in_deg: Vec<usize> = deps.iter().map(|d| d.len()).collect();
    let mut queue: Vec<usize> = (0..n).filter(|&i| in_deg[i] == 0).collect();
    let mut order = Vec::with_capacity(n);
    while let Some(i) = queue.pop() {
        order.push(i);
        // Anyone depending on i loses one in-degree.
        for j in 0..n {
            if deps[j].contains(&i) {
                in_deg[j] -= 1;
                if in_deg[j] == 0 { queue.push(j); }
            }
        }
    }
    if order.len() != n {
        return Err(LetError {
            message: "circular dependency in WHERE clauses".into(),
            pos: 0,
        });
    }
    Ok(order)
}

fn collect_deps(expr: &Expr, name_to_idx: &HashMap<&str, usize>, deps: &mut Vec<usize>) {
    match expr {
        Expr::Lit(_) => {}
        Expr::Var(n) => {
            if let Some(&idx) = name_to_idx.get(n.as_str()) {
                if !deps.contains(&idx) { deps.push(idx); }
            }
        }
        Expr::Bin(_, l, r) => { collect_deps(l, name_to_idx, deps); collect_deps(r, name_to_idx, deps); }
        Expr::Neg(e) => collect_deps(e, name_to_idx, deps),
        Expr::Call(_, args) => for a in args { collect_deps(a, name_to_idx, deps); },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::parser::parse;

    fn empty_libm() -> LibmTable { LibmTable::new() }

    #[test]
    fn lower_minimal() {
        let form = parse("LET (r) -> (a) = r END").unwrap();
        let asm = lower(&form, "let_test1", &empty_libm()).unwrap();
        assert!(asm.contains("let_test1:"));
        // Named values now live at xmm6+ (callee-saved) so libm calls
        // don't clobber them; input 0 lands in xmm6.
        assert!(asm.contains("movsd xmm6, qword ptr [rcx + 0]"));
        assert!(asm.contains("ret"));
    }

    #[test]
    fn lower_area_of_circle() {
        let form = parse("LET (r) -> (a) = pi * r * r END").unwrap();
        let asm = lower(&form, "let_area", &empty_libm()).unwrap();
        assert!(asm.contains("mulsd"));
        assert!(asm.contains("let_area$$const_"));
    }

    #[test]
    fn lower_detects_cycle() {
        let e = lower(
            &parse("LET (x) -> (y) = a WHERE a = b WHERE b = a END").unwrap(),
            "let_cycle",
            &empty_libm(),
        )
        .unwrap_err();
        assert!(e.message.contains("circular"));
    }

    #[test]
    fn lower_undefined_name() {
        let e = lower(
            &parse("LET (x) -> (y) = z END").unwrap(),
            "let_undef",
            &empty_libm(),
        )
        .unwrap_err();
        assert!(e.message.contains("undefined"));
    }

    #[test]
    fn lower_unknown_function_errors() {
        let e = lower(
            &parse("LET (x) -> (y) = wibblywobbly(x) END").unwrap(),
            "let_bad",
            &empty_libm(),
        )
        .unwrap_err();
        assert!(e.message.contains("wibblywobbly"));
    }

    #[test]
    fn lower_mbrot_compiles() {
        let form = parse("\
            LET (z_re, z_im, x, y) -> (z_next_re, z_next_im, mag) = \
                re, im, rmag \
                WHERE re   = (z_re * z_re) - (z_im * z_im) + x \
                WHERE im   = (2 * z_re * z_im) + y \
                WHERE rmag = (re * re) + (im * im) \
            END").unwrap();
        let asm = lower(&form, "let_mbrot", &empty_libm()).unwrap();
        // 4 inputs + 3 wheres = 7 named regs starting at xmm6:
        //   xmm6 (z_re) at offset 24 — deepest
        //   xmm7 (z_im) at offset 16
        //   xmm8 (x)    at offset 8
        //   xmm9 (y)    at offset 0 — TOS
        assert!(asm.contains("movsd xmm6, qword ptr [rcx + 24]"));
        assert!(asm.contains("movsd xmm7, qword ptr [rcx + 16]"));
        assert!(asm.contains("movsd xmm8, qword ptr [rcx + 8]"));
        assert!(asm.contains("movsd xmm9, qword ptr [rcx + 0]"));
        // re/im/rmag at xmm10..xmm12.
        assert!(asm.contains("xmm12"));
    }
}
