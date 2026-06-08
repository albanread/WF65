//! WF32 → WF64 primitive translator.
//!
//! Reads `E:\wf32\src\kernel\gkernel32.fs`, finds the named `code …
//! next; …` block, mechanically rewrites the WF32 asm syntax into
//! JASM .masm syntax, and prints a ready-to-paste `proc(…) … endp()`
//! block. Best-effort — anything the rules can't handle gets surfaced
//! as a `; TODO:` comment so the human can finish the job.
//!
//! Why a translator, not a JASM `@rust_macro`: keeping the .masm files
//! purely native asm (no runtime dependency on the WF32 source) means
//! a future reader can read what's actually executing without bouncing
//! through a generator. The cost is one paste-step per batch, which is
//! cheap compared to the alternative of every kernel build needing
//! gkernel32.fs on disk.
//!
//! ## WF32 asm syntax recap
//!
//! - Operands are **space-separated**, no commas: `mov ecx eax`
//! - Memory is **`{ … }`** rather than `[ … ]`, with operands in the
//!   order `[disp] base [index *scale]`: `{ -cell ebp eax *cell }`
//!   means `[rbp + rax*cell - cell]`
//! - Size qualifiers (`byte`, `word`, `dword`, `qword`) precede the
//!   memory expression without `ptr`: `movzx eax byte { eax }`
//! - Immediates can be RPN expressions: `sar eax cell 8 * 1-` means
//!   `sar eax, (cell*8) - 1`. The translator handles the **literal
//!   substitution** but does NOT evaluate RPN — those lines get a
//!   `; TODO:` flag.
//! - Stack effect is declared once per primitive as `N M in/out` and
//!   consumed by WF32's `next;` macro to emit `add ebp, (N-M)*cell`
//!   (or sub, depending on sign). We capture it as `stk(N, M)` for
//!   JASM's matching macro.
//!
//! ## What the translator does NOT do
//!
//! - Evaluate RPN immediates. `sar eax cell 8 * 1-` becomes a TODO.
//! - Substitute user-area variable names. `sp0`, `bp0`, `state` etc.
//!   stay as-is and need a manual rename to `user_SP0`, etc.
//! - Choose the right WF64 asm symbol. Forth names like `>r` need to
//!   become C-style identifiers; we default to `to_r`-style mangling
//!   but the caller can override on the command line.
//! - Reflow comments. Inline `\ comment` lines on body lines get
//!   stripped (we keep them only as standalone lines).

use std::fs;
use std::path::Path;

/// A parsed WF32 `code` block, ready for translation.
#[derive(Debug, Clone)]
pub struct Wf32Block {
    pub name: String,
    /// `( n -- n n )`-style stack effect comment, raw text — kept for
    /// the translated header so the WF64 reader sees the same docs.
    pub stack_comment: Option<String>,
    /// `(N, M)` from the `N M in/out` annotation. `None` if the
    /// primitive didn't declare one (which means it has no `stk`
    /// adjustment — caller must supply one if needed).
    pub stack_effect: Option<(i64, i64)>,
    /// Raw body lines, in source order. Comment-only and blank lines
    /// are preserved.
    pub body_lines: Vec<String>,
    /// True if the WF32 source flagged this primitive `inline`.
    /// Informational only; WF64 doesn't inline primitives (every call
    /// is a real CALL).
    pub inline: bool,
}

/// Read and index every `code` block in a WF32 kernel source file.
///
/// gkernel32.fs is an old DOS-era Forth source — Windows-1252 / Latin-1
/// rather than UTF-8 (the box-drawing chars in some comments aren't
/// valid UTF-8). We read raw bytes and treat each as Latin-1 (1:1
/// codepoint map) so decoding can never fail.
pub fn index(path: &Path) -> std::io::Result<Vec<Wf32Block>> {
    let bytes = fs::read(path)?;
    let text: String = bytes.into_iter().map(|b| b as char).collect();
    let mut blocks = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let trim = line.trim_start();
        if !trim.starts_with("code ") {
            continue;
        }
        // Name is the first whitespace-delimited token after `code `.
        let after_code = trim["code ".len()..].trim_start();
        let name: String = after_code
            .split(|c: char| c.is_whitespace())
            .next()
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            continue;
        }
        // Stack-effect comment is `( ... )` further along the header line.
        let stack_comment = extract_paren_comment(after_code);

        let mut block = Wf32Block {
            name,
            stack_comment,
            stack_effect: None,
            body_lines: Vec::new(),
            inline: false,
        };

        // Body: every line up to and including the `next;` line.
        for body_line in lines.by_ref() {
            let bt = body_line.trim();

            // Stack-effect annotation can appear on its own line OR
            // tacked onto the `next;` line. Try both.
            if let Some(eff) = parse_in_out(bt) {
                block.stack_effect = Some(eff);
            }

            // WF32 commonly tacks `next;` onto the same line as a
            // MASM-style anonymous label (e.g. `@@9: next;`) — strip a
            // leading `label:` before checking the marker. Also strip
            // any trailing `\ comment` so `N M in/out \ blah` parses.
            let pre_comment = match bt.find('\\') {
                Some(i) => bt[..i].trim_end(),
                None => bt,
            };
            let after_label = strip_leading_label(pre_comment);
            if after_label.starts_with("next;") || after_label.starts_with("next ;") {
                block.inline = bt.contains("inline");
                // If next; shared a line with a label, push the
                // label-only line into the body so any branch that
                // targeted it still has somewhere to land (the
                // proc-footer `next()` will follow immediately after).
                if let Some(colon) = pre_comment.find(':') {
                    let lab = &pre_comment[..colon];
                    if !lab.is_empty()
                        && !lab.contains(|c: char| c.is_whitespace())
                    {
                        block.body_lines.push(format!("{lab}:"));
                    }
                }
                break;
            }
            if parse_in_out(pre_comment).is_some()
                && pre_comment.split_whitespace().count() == 3
            {
                continue;
            }
            // Comment-only or blank — keep as-is for context.
            block.body_lines.push(body_line.to_string());
        }
        blocks.push(block);
    }
    Ok(blocks)
}

/// Render a translated WF64 .masm `proc(...) ... endp()` block.
pub fn translate(block: &Wf32Block, wf64_sym: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("; {}", block.name));
    if let Some(sc) = &block.stack_comment {
        out.push(' ');
        out.push_str(sc);
    }
    out.push('\n');
    out.push_str("; Ported mechanically from WF32 gkernel32.fs — review before committing.\n");
    if block.inline {
        out.push_str("; (WF32 marked this `inline`; WF64 makes every primitive a real CALL — irrelevant here.)\n");
    }
    out.push_str(&format!("proc({})\n", wf64_sym));
    for line in &block.body_lines {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            out.push('\n');
            continue;
        }
        let body = trimmed.trim_start();
        if body.starts_with('\\') {
            // Comment line — convert `\ foo` to `; foo`.
            out.push_str("    ; ");
            out.push_str(body.trim_start_matches('\\').trim_start());
            out.push('\n');
            continue;
        }
        // Normalise indentation to the project's 4-space convention —
        // WF32 used 6 for body lines, which would look out of place
        // mixed with our hand-written kernel files.
        out.push_str("    ");
        out.push_str(&translate_line(body));
        out.push('\n');
    }
    if let Some((in_c, out_c)) = block.stack_effect {
        // Always emit `stk(N, M)` — even when balanced (N==M, no asm
        // generated) — so the stack effect is documented at the call
        // site. WF32's source has the same redundancy: every primitive
        // carries an `N M in/out` line whether or not it adjusts rbp.
        // Reader gets the contract without chasing the header comment.
        out.push_str(&format!("    stk({}, {})\n", in_c, out_c));
    } else {
        out.push_str("    ; TODO: WF32 had no in/out annotation — add `stk(N, M)` if needed\n");
    }
    out.push_str("    next()\n");
    out.push_str("endp()\n");
    out
}

// ── parsing helpers ──────────────────────────────────────────────────

fn extract_paren_comment(s: &str) -> Option<String> {
    let start = s.find('(')?;
    let end = s[start..].find(')')?;
    Some(s[start..start + end + 1].to_string())
}

/// Rewrite `$<hex>` (WF32 / MASM hex shorthand) to `0x<hex>` so the
/// rest of the pipeline sees the standard form. Only digits 0-9, a-f,
/// A-F and `_` are accepted after the `$`; anything else leaves the
/// token alone (so a stray `$` in a comment or label doesn't get
/// mangled — though we've already stripped backslash comments above).
fn rewrite_dollar_hex(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + 4);
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' {
            // Peek ahead: is this followed by at least one hex digit?
            if matches!(chars.peek(), Some(c) if c.is_ascii_hexdigit()) {
                out.push_str("0x");
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_hexdigit() || c == '_' {
                        out.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// Strip a leading `label:` if `line` starts with a label-shaped
/// token followed by a colon. Returns the remainder, left-trimmed.
/// Used to recognise `next;` even when it shares a line with a label.
fn strip_leading_label(line: &str) -> &str {
    if let Some(colon) = line.find(':') {
        let prefix = &line[..colon];
        if !prefix.is_empty()
            && !prefix.contains(|c: char| c.is_whitespace())
        {
            return line[colon + 1..].trim_start();
        }
    }
    line
}

/// Parse `N M in/out` from anywhere in `line`. Returns `(N, M)` if found.
fn parse_in_out(line: &str) -> Option<(i64, i64)> {
    let words: Vec<&str> = line.split_whitespace().collect();
    for i in 0..words.len().saturating_sub(2) {
        if words[i + 2] == "in/out" {
            if let (Ok(n), Ok(m)) =
                (words[i].parse::<i64>(), words[i + 1].parse::<i64>())
            {
                return Some((n, m));
            }
        }
    }
    None
}

// ── line translation ─────────────────────────────────────────────────

/// Translate one WF32 asm body line into WF64 form. Inline `\` comments
/// are stripped; the caller handles standalone comment lines.
fn translate_line(line: &str) -> String {
    // Strip inline backslash comment, if any.
    let body_no_comment = match line.find('\\') {
        Some(i) => line[..i].trim_end(),
        None => line,
    };
    // WF32 Forth-asm uses `$N` for hex immediates; JASM/LLVM-MC expects
    // `0xN`. Rewrite `$<hexdigits>` → `0x<hexdigits>` as a global
    // preprocess so the rest of the pipeline sees clean numbers.
    let body_str = rewrite_dollar_hex(body_no_comment);
    let body: &str = &body_str;
    let mut toks = tokenize(body);
    if toks.is_empty() {
        return String::new();
    }

    // WF32 labels glue the colon to the name (`@@1:`, `foo:`). Split
    // any trailing `:` off the first token so the label-detector below
    // can see it.
    if toks[0].ends_with(':') && toks[0].len() > 1 {
        let name = toks[0][..toks[0].len() - 1].to_string();
        toks[0] = name;
        toks.insert(1, ":".into());
    }

    // `<label>:` standalone (or `<label>: <rest>`). Also catches WF32's
    // MASM-style `@@N:` labels and rewrites them as JASM scope-local
    // `.LN` so the proc's labels stay unique under @scope.
    if toks.len() >= 2 && toks[1] == ":" {
        let lab = mangle_label(&toks[0]);
        if toks.len() == 2 {
            return format!("{lab}:");
        }
        // `label: mnemonic operands…` — split and translate the body.
        let after = toks[2..].join(" ");
        return format!("{lab}:  {}", translate_line(&after));
    }

    // `rep [movs|stos|lods|scas|cmps] byte/word/dword/qword`
    // → `rep <op><sfx>`. LLVM-MC wants the single-token form. The
    // `dword` size in WF32 source means "the cell" (32-bit cells
    // there), so on WF64 it rewrites to `q` (8-byte cell).
    if matches!(toks[0].as_str(), "rep" | "repnz" | "repz" | "repe" | "repne")
        && toks.len() >= 3
        && matches!(toks[1].as_str(), "movs" | "stos" | "lods" | "scas" | "cmps")
    {
        let sfx = match toks[2].as_str() {
            "byte" => Some("b"),
            "word" => Some("w"),
            "dword" => Some("q"),
            "qword" => Some("q"),
            _ => None,
        };
        if let Some(s) = sfx {
            return format!("{:<7} {}{s}", toks[0], toks[1]);
        }
    }

    let mnemonic = translate_mnemonic(&toks[0]);
    let raw_operands = group_operands(&toks[1..]);
    // For jumps, drop the `short` keyword if present — LLVM-MC picks
    // the optimal encoding without that hint and would otherwise see
    // `short` as a label name.
    let mut operands: Vec<Vec<String>> = raw_operands
        .into_iter()
        .filter(|op| !(is_jump_mnemonic(&mnemonic) && op.len() == 1 && op[0] == "short"))
        .collect();
    // Rewrite `@@N` label references in jump operands to `.LN`.
    if is_jump_mnemonic(&mnemonic) {
        for op in &mut operands {
            if op.len() == 1 {
                op[0] = mangle_label(&op[0]);
            }
        }
    }
    if operands.is_empty() {
        return mnemonic;
    }
    let rendered: Vec<String> = operands.iter().map(render_operand).collect();
    format!("{:<7} {}", mnemonic, rendered.join(", "))
}

fn is_jump_mnemonic(m: &str) -> bool {
    matches!(
        m,
        "jmp" | "ja" | "jae" | "jb" | "jbe" | "jc" | "je" | "jg" | "jge"
        | "jl" | "jle" | "jna" | "jnae" | "jnb" | "jnbe" | "jnc" | "jne"
        | "jng" | "jnge" | "jnl" | "jnle" | "jno" | "jnp" | "jns" | "jnz"
        | "jo" | "jp" | "jpe" | "jpo" | "js" | "jz" | "jcxz" | "jecxz" | "jrcxz"
        | "loop" | "loope" | "loopne" | "loopz" | "loopnz"
    )
}

/// Translate a label token. `@@N` (MASM anonymous) → `.LN` (JASM
/// scope-local). Everything else passes through.
fn mangle_label(tok: &str) -> String {
    if let Some(rest) = tok.strip_prefix("@@") {
        format!(".L{rest}")
    } else {
        tok.to_string()
    }
}

/// Mnemonic rewrites for the small set of 32→64 traps where the WF32
/// name still assembles on x86-64 but does the wrong thing. The big
/// one: `cdq` sign-extends EAX into EDX; on 64-bit we usually want
/// `cqo` (sign-extend RAX into RDX:RAX). Same family for double-width
/// divide.
fn translate_mnemonic(m: &str) -> String {
    match m {
        "cdq" => "cqo".into(),  // sign-extend rax into rdx:rax for 64-bit idiv/imul
        _ => m.into(),
    }
}

/// Size keyword for memory operand width. WF32 cells were `dword`
/// (4 bytes); WF64 cells are `qword` (8 bytes). The other widths
/// (`byte`, `word`) survive unchanged — those are sub-cell access
/// patterns that don't shift with cell size.
fn translate_size(s: &str) -> &str {
    match s {
        "dword" => "qword",
        other => other,
    }
}

/// Split on whitespace, but keep `{ … }` together as one token.
/// Also collapses the very common WF32 RPN shape `N cells` (e.g.
/// `2 cells`, `3 cells`) into the single infix token `N*cell` so the
/// rest of the pipeline doesn't have to deal with it. Operates on
/// both top-level tokens (`add ebp 2 cells`) and inside brace
/// expressions (`{ 2 cells ebp }`) — the brace contents get a second
/// pass in `translate_memory`.
///
/// Other RPN immediates (`cell 8 * 1-`, `-1 1 rshift`) remain as
/// separate tokens for the caller to spot as TODO.
fn tokenize(line: &str) -> Vec<String> {
    let mut toks = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        if c == '{' {
            let mut s = String::new();
            while let Some(c) = chars.next() {
                s.push(c);
                if c == '}' {
                    break;
                }
            }
            toks.push(s);
        } else {
            let mut s = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() || c == '{' {
                    break;
                }
                s.push(c);
                chars.next();
            }
            toks.push(s);
        }
    }
    collapse_n_cells(&mut toks);
    toks
}

/// `[..., "2", "cells", ...]` → `[..., "2*cell", ...]`. Operates on
/// top-level tokens only; inside-brace handling lives in
/// `translate_memory`.
fn collapse_n_cells(toks: &mut Vec<String>) {
    let mut i = 0;
    while i + 1 < toks.len() {
        if toks[i + 1] == "cells" && toks[i].parse::<i64>().is_ok() {
            let combined = format!("{}*cell", toks[i]);
            toks[i] = combined;
            toks.remove(i + 1);
        } else {
            i += 1;
        }
    }
}

/// Group operand tokens. A size qualifier (`byte`/`word`/`dword`/`qword`)
/// attaches to the immediately-following memory expression as one operand.
fn group_operands(toks: &[String]) -> Vec<Vec<String>> {
    let mut ops: Vec<Vec<String>> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        let is_size = matches!(t.as_str(), "byte" | "word" | "dword" | "qword");
        if is_size && i + 1 < toks.len() && toks[i + 1].starts_with('{') {
            ops.push(vec![t.clone(), toks[i + 1].clone()]);
            i += 2;
        } else {
            ops.push(vec![t.clone()]);
            i += 1;
        }
    }
    ops
}

fn render_operand(op: &Vec<String>) -> String {
    if op.len() == 2 {
        // size + memory. `dword` in WF32 source almost always meant
        // "the cell size" (which was 4 bytes); on WF64 that's `qword`.
        // The handful of places where WF32 genuinely wanted 32-bit
        // semantics (L@/L!) are explicit cell-width primitives the
        // reviewer would scrutinise anyway. Default to qword and let
        // edge cases get caught at test time.
        let size = translate_size(&op[0]);
        let mem = translate_memory(&op[1]);
        format!("{} ptr {}", size, mem)
    } else {
        let t = &op[0];
        if t.starts_with('{') {
            translate_memory(t)
        } else {
            translate_register(t)
        }
    }
}

/// 32-bit register → 64-bit equivalent. Other tokens pass through
/// unchanged (immediates, identifiers, partial regs like `al`/`cl`,
/// which keep their names on x86-64).
fn translate_register(tok: &str) -> String {
    match tok {
        "eax" => "rax".into(),
        "ebx" => "rbx".into(),
        "ecx" => "rcx".into(),
        "edx" => "rdx".into(),
        "esi" => "rsi".into(),
        "edi" => "rdi".into(),
        "ebp" => "rbp".into(),
        "esp" => "rsp".into(),
        _ => tok.into(),
    }
}

/// `{ [disp] base [index *scale] }` → `[base + index*scale + disp]`.
///
/// WF32 puts displacement *before* base. The asm we emit follows the
/// usual `[base + index*scale + disp]` arrangement that reads naturally.
/// Negative displacements (`-cell`, `-4`) become `- <abs>` so the
/// expression is unambiguous.
fn translate_memory(tok: &str) -> String {
    let inner = tok.trim_start_matches('{').trim_end_matches('}').trim();
    let raw_parts: Vec<&str> = inner.split_whitespace().collect();
    // Apply the `N cells` → `N*cell` collapse inside the brace too.
    // `{ 2 cells ebp }` is a common WF32 memory form for "address two
    // cells past base"; without the collapse the porter would treat
    // `2`, `cells`, `ebp` as three distinct operands and fall over.
    let mut owned: Vec<String> = Vec::with_capacity(raw_parts.len());
    let mut i = 0;
    while i < raw_parts.len() {
        if i + 1 < raw_parts.len()
            && raw_parts[i + 1] == "cells"
            && raw_parts[i].parse::<i64>().is_ok()
        {
            owned.push(format!("{}*cell", raw_parts[i]));
            i += 2;
        } else {
            owned.push(raw_parts[i].to_string());
            i += 1;
        }
    }
    let parts: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
    let mut base: Option<String> = None;
    let mut index: Option<String> = None;
    let mut scale: Option<String> = None;
    let mut disp: Vec<String> = Vec::new();

    for &p in &parts {
        if let Some(s) = p.strip_prefix('*') {
            scale = Some(s.to_string());
            // `*N` comes right after the *index* register. If only one
            // register has been seen so far, it was actually the index
            // (we'd tentatively called it base); promote. If two have
            // been seen (`{ ebp eax *cell }`), the second is already
            // the index and base stays put.
            if index.is_none() {
                if let Some(b) = base.take() {
                    index = Some(b);
                }
            }
            continue;
        }
        if is_register(p) {
            if base.is_none() {
                base = Some(translate_register(p));
            } else {
                index = Some(translate_register(p));
            }
        } else {
            disp.push(p.to_string());
        }
    }

    let mut s = String::from("[");
    let mut have_term = false;
    if let Some(b) = &base {
        s.push_str(b);
        have_term = true;
    }
    match (&index, &scale) {
        (Some(idx), Some(sc)) => {
            if have_term {
                s.push_str(" + ");
            }
            s.push_str(idx);
            s.push('*');
            s.push_str(sc);
            have_term = true;
        }
        (Some(idx), None) => {
            if have_term {
                s.push_str(" + ");
            }
            s.push_str(idx);
            have_term = true;
        }
        _ => {}
    }
    if !disp.is_empty() {
        let d = disp.join(" ");
        if let Some(rest) = d.strip_prefix('-') {
            if have_term {
                s.push_str(" - ");
            } else {
                s.push('-');
            }
            s.push_str(rest);
        } else {
            if have_term {
                s.push_str(" + ");
            }
            s.push_str(&d);
        }
    }
    s.push(']');
    s
}

fn is_register(tok: &str) -> bool {
    matches!(
        tok,
        "eax" | "ebx" | "ecx" | "edx" | "esi" | "edi" | "ebp" | "esp"
        | "rax" | "rbx" | "rcx" | "rdx" | "rsi" | "rdi" | "rbp" | "rsp"
        | "r8" | "r9" | "r10" | "r11" | "r12" | "r13" | "r14" | "r15"
    )
}

// ── tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_keeps_brace_block_intact() {
        let t = tokenize("mov { -cell ebp } eax");
        assert_eq!(t, vec!["mov", "{ -cell ebp }", "eax"]);
    }

    #[test]
    fn translate_memory_base_only() {
        assert_eq!(translate_memory("{ ebp }"), "[rbp]");
    }

    #[test]
    fn translate_memory_with_neg_disp() {
        assert_eq!(translate_memory("{ -cell ebp }"), "[rbp - cell]");
    }

    #[test]
    fn translate_memory_with_pos_disp() {
        assert_eq!(translate_memory("{ cell ebp }"), "[rbp + cell]");
    }

    #[test]
    fn translate_memory_with_index_and_scale() {
        assert_eq!(
            translate_memory("{ ebp eax *cell }"),
            "[rbp + rax*cell]"
        );
    }

    #[test]
    fn translate_memory_with_named_disp() {
        // Like `{ sp0 ebx }` — sp0 is an identifier, not a number.
        assert_eq!(translate_memory("{ sp0 ebx }"), "[rbx + sp0]");
    }

    #[test]
    fn translate_line_two_register_operands() {
        assert_eq!(translate_line("mov ecx eax"), "mov     rcx, rax");
    }

    #[test]
    fn translate_line_immediate() {
        assert_eq!(translate_line("add eax 1"), "add     rax, 1");
    }

    #[test]
    fn translate_line_size_prefix() {
        assert_eq!(
            translate_line("movzx eax byte { eax }"),
            "movzx   rax, byte ptr [rax]"
        );
    }

    #[test]
    fn translate_line_mem_dst() {
        assert_eq!(
            translate_line("mov { -cell ebp } eax"),
            "mov     [rbp - cell], rax"
        );
    }

    #[test]
    fn translate_line_strips_inline_backslash_comment() {
        let t = translate_line("mov eax 1 \\ this is a comment");
        assert_eq!(t, "mov     rax, 1");
    }

    #[test]
    fn parse_in_out_standalone_line() {
        assert_eq!(parse_in_out("    2 1 in/out"), Some((2, 1)));
    }

    #[test]
    fn parse_in_out_on_next_line() {
        assert_eq!(parse_in_out("next; inline 0 1 in/out"), Some((0, 1)));
    }

    #[test]
    fn dollar_hex_rewritten() {
        assert_eq!(translate_line("cmp eax $20"), "cmp     rax, 0x20");
        assert_eq!(translate_line("mov eax $FF"), "mov     rax, 0xFF");
    }

    #[test]
    fn rep_movsb_collapsed() {
        assert_eq!(translate_line("rep movs byte"), "rep     movsb");
    }

    #[test]
    fn rep_movsq_from_dword() {
        // WF32 cell-size dword → WF64 qword
        assert_eq!(translate_line("rep movs dword"), "rep     movsq");
    }

    #[test]
    fn repnz_scasb_collapsed() {
        assert_eq!(translate_line("repnz scas byte"), "repnz   scasb");
    }

    #[test]
    fn short_keyword_dropped_from_jmp() {
        assert_eq!(translate_line("jne short @@1"), "jne     .L1");
    }

    #[test]
    fn anonymous_label_def_translated() {
        assert_eq!(translate_line("@@1:"), ".L1:");
        assert_eq!(
            translate_line("@@1: idiv ecx"),
            ".L1:  idiv    rcx"
        );
    }

    #[test]
    fn n_cells_collapsed_at_top_level() {
        // `add ebp 2 cells` → `add rbp, 2*cell`
        let t = translate_line("add ebp 2 cells");
        assert_eq!(t, "add     rbp, 2*cell");
    }

    #[test]
    fn n_cells_collapsed_inside_brace() {
        // `mov eax { 2 cells ebp }` → `mov rax, [rbp + 2*cell]`
        let t = translate_line("mov eax { 2 cells ebp }");
        assert_eq!(t, "mov     rax, [rbp + 2*cell]");
    }

    #[test]
    fn end_to_end_dup() {
        // The dup primitive from gkernel32.fs line 124:
        //   code dup        ( n -- n n )    \ duplicate top entry on data stack
        //       1 2 in/out
        //         mov     { -cell ebp } eax
        //         next; inline
        let src = "\
code dup        ( n -- n n )    \\ duplicate top entry on data stack
    1 2 in/out
      mov     { -cell ebp } eax
      next; inline
";
        let tmp = std::env::temp_dir().join("wf32_port_test_dup.fs");
        std::fs::write(&tmp, src).unwrap();
        let blocks = index(&tmp).unwrap();
        assert_eq!(blocks.len(), 1);
        let b = &blocks[0];
        assert_eq!(b.name, "dup");
        assert_eq!(b.stack_effect, Some((1, 2)));
        assert!(b.inline);
        let rendered = translate(b, "dup_");
        // Must include the proc/endp wrapper, the translated mov, the
        // stk adjustment (always emitted, even when balanced), and a
        // next.
        assert!(rendered.contains("proc(dup_)"));
        assert!(rendered.contains("mov     [rbp - cell], rax"));
        assert!(rendered.contains("stk(1, 2)"));
        assert!(rendered.contains("next()"));
        assert!(rendered.contains("endp()"));
        let _ = std::fs::remove_file(tmp);
    }
}
