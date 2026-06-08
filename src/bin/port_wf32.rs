//! Port a WF32 primitive to WF64 .masm form.
//!
//! Usage:
//!
//! ```text
//! cargo run --bin port-wf32 -- NAME[:SYM] [NAME[:SYM]...]
//! ```
//!
//! `NAME` is the WF32 Forth name as it appears in `gkernel32.fs`
//! (e.g. `dup`, `+`, `/mod`, `>r`, `0=`). `SYM` overrides the WF64
//! asm symbol; if omitted, a best-effort mangling is used (`>` →
//! `_to_`, `+` → `plus`, etc.). Reserved x86 mnemonics and JASM
//! keywords get a trailing underscore (`dup` → `dup_`).
//!
//! Separator is `:` rather than `=` because Forth names like `0=`
//! and `<=` contain `=` themselves.
//!
//! The translated `proc(...) ... endp()` block is printed to stdout.
//! Paste it into the appropriate kernel/*.masm file, then add the
//! matching `(forth_name, asm_sym, flags)` entry to PRIMITIVES in
//! src/lib.rs and write a test.
//!
//! Examples:
//!
//! ```text
//! cargo run --bin port-wf32 -- '+'
//! cargo run --bin port-wf32 -- '/mod' negate abs
//! cargo run --bin port-wf32 -- '0=:zero_equal' '<>:not_equal'
//! ```
//!
//! The translator is best-effort. It surfaces ambiguities (RPN
//! expressions, unknown identifiers, missing in/out annotations) as
//! `; TODO:` comments so you can spot them at review time.

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use wf64::wf32_port::{index, translate};

/// Default location of the WF32 kernel source. Override with the
/// `WF32_KERNEL` env var if you have a checkout elsewhere.
const DEFAULT_WF32_KERNEL: &str = r"E:\wf32\src\kernel\gkernel32.fs";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        eprintln!(
            "usage: cargo run --bin port-wf32 -- NAME[:SYM] [NAME[:SYM]...]\n\
             \n\
             NAME is the WF32 Forth name (must match `gkernel32.fs`).\n\
             SYM  is the WF64 asm symbol (defaults to a best-effort mangling).\n\
             Separator is `:` (not `=`) because Forth names may contain `=`.\n\
             \n\
             env: WF32_KERNEL overrides the kernel source path\n\
                  (default: {DEFAULT_WF32_KERNEL})\n"
        );
        std::process::exit(if args.is_empty() { 2 } else { 0 });
    }
    let kernel_path = std::env::var_os("WF32_KERNEL")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_WF32_KERNEL));
    let blocks = index(&kernel_path)
        .map_err(|e| anyhow!("read {}: {e}", kernel_path.display()))?;

    let mut wrote_any = false;
    for arg in args {
        let (name, sym_opt) = match arg.split_once(':') {
            Some((n, s)) => (n.to_string(), Some(s.to_string())),
            None => (arg.clone(), None),
        };
        let block = blocks
            .iter()
            .find(|b| b.name == name)
            .ok_or_else(|| {
                anyhow!(
                    "WF32 primitive `{name}` not found in {} ({} blocks scanned)",
                    kernel_path.display(),
                    blocks.len()
                )
            })?;
        let sym = sym_opt.unwrap_or_else(|| mangle_default(&name));
        if wrote_any {
            println!();
        }
        print!("{}", translate(block, &sym));
        eprintln!(
            "; ↑ {} → {} ({} body lines, stack-effect {})",
            name,
            sym,
            block.body_lines.len(),
            match block.stack_effect {
                Some((i, o)) => format!("({i} -- {o})"),
                None => "<none>".into(),
            }
        );
        wrote_any = true;
    }
    Ok(())
}

/// Best-effort name → asm-symbol mangling. Conservative — when in
/// doubt, the caller should override with `name=sym`.
fn mangle_default(name: &str) -> String {
    // Reserved mnemonics or JASM keywords that would shadow the macro
    // get a trailing underscore. Matches the existing PRIMITIVES table.
    const RESERVED: &[&str] = &[
        "dup", "drop", "swap", "rot", "over", "and", "or", "not", "nip",
        "tuck", "if", "then", "else",
    ];
    if RESERVED.contains(&name) {
        return format!("{}_", name);
    }
    // Character-by-character mangling for Forth-shaped names. Aligned
    // with the conventions already established in PRIMITIVES.
    let mut out = String::new();
    let mut chars = name.chars().peekable();
    let mut first = true;
    while let Some(c) = chars.next() {
        let piece: &str = match c {
            '>' => if first { "to_" } else { "_to_" },
            '<' => if first { "from_" } else { "_from_" },
            '@' => "_fetch",
            '!' => "_store",
            '+' => "plus",
            '-' => "minus",
            '*' => "times",
            '/' => "slash",
            '?' => if first { "q" } else { "_q" },
            '=' => "_equal",
            '0' if first => "zero_",
            '1' if first => "one_",
            '2' if first => "two_",
            '3' if first => "three_",
            '4' if first => "four_",
            _ => {
                out.push(c);
                first = false;
                continue;
            }
        };
        out.push_str(piece);
        first = false;
    }
    // Collapse the common `_minus_minus` (from `--`) into a clean form.
    out = out.replace("__", "_");
    out.trim_matches('_').to_string()
}

#[cfg(test)]
mod tests {
    use super::mangle_default;

    #[test]
    fn mangle_to_r() {
        assert_eq!(mangle_default(">r"), "to_r");
    }

    #[test]
    fn mangle_r_from() {
        assert_eq!(mangle_default("r>"), "r_to");
        // ^ note: best-effort; caller should override to `r_from`. The
        // mangling rule doesn't know which side of `>` we mean.
    }

    #[test]
    fn mangle_dup_gets_underscore() {
        assert_eq!(mangle_default("dup"), "dup_");
    }

    #[test]
    fn mangle_fetch() {
        assert_eq!(mangle_default("@"), "fetch");
    }

    #[test]
    fn mangle_zero_equal() {
        assert_eq!(mangle_default("0="), "zero_equal");
    }
}
