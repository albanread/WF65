//! Forth → Factor source transpiler.
//!
//! Mirrors the logic in `forth.preparser` (the Factor vocabulary) but
//! implemented in Rust for use in the NewFactor IDE worker thread.
//!
//! # Rewrites
//!
//! | Forth token | Factor token(s) | Notes |
//! |-------------|-----------------|-------|
//! | `!`         | `var!`          | Store word; `!` is a Factor comment |
//! | `IF`        | `[`             | Open true-branch quotation |
//! | `ELSE`      | `] [`           | Close true, open false |
//! | `THEN` (no ELSE) | `] when`  | Close true, emit `when` |
//! | `THEN` (+ELSE)   | `] if`    | Close false, emit `if` |
//! | `BEGIN`     | `[`             | Open loop-body quotation |
//! | `WHILE`     | `] [`           | Close cond-quot, open body-quot |
//! | `REPEAT`    | `] while`       | Close body-quot, emit `while` |
//! | `UNTIL`     | `] until`       | Close body+cond-quot, emit `until` |
//! | `AGAIN`     | `] loop`        | Close body-quot, emit `loop` |
//!
//! # Limitations (same as the Factor preparser)
//! - `!` inside string literals `"hello ! world"` is wrongly rewritten.
//! - `DO / LOOP / +LOOP / LEAVE / I / J` not yet handled.
//! - `CASE / OF / ENDOF / ENDCASE` not yet handled.
//! - `EXIT` not yet handled.
//!
//! # Comment handling
//! Factor uses `\` (backslash-to-EOL) and `( )` (parentheses) for
//! comments — both of which Factor parses natively.  Forth `.fth` files
//! use the same conventions, so no extra comment handling is needed.

/// Control-structure stack entry.
#[derive(Debug, Clone, PartialEq)]
enum Ctrl {
    If,       // IF seen, no ELSE yet
    IfElse,   // IF + ELSE seen, awaiting THEN
    Begin,    // BEGIN seen, awaiting WHILE/UNTIL/AGAIN
    While,    // BEGIN + WHILE seen, awaiting REPEAT
}

/// Stateful Forth→Factor transpiler.
///
/// The control-structure stack (`ctrl_stack`) persists across [`transpile`]
/// calls, enabling multi-line word definitions typed line-by-line at a REPL.
/// Call [`reset`] to clear the stack for a fresh session or after an error.
pub struct Transpiler {
    ctrl_stack: Vec<Ctrl>,
}

impl Transpiler {
    pub fn new() -> Self {
        Self { ctrl_stack: Vec::new() }
    }

    /// Reset the ctrl-stack.  Call at session start or after a fatal error.
    pub fn reset(&mut self) {
        self.ctrl_stack.clear();
    }

    /// Transpile one or more lines of Forth source to Factor source.
    ///
    /// Lines are split on `\n`; each is processed independently (tokens
    /// cannot span lines).  The ctrl-stack IS preserved across lines within
    /// a single call, and across successive calls.
    pub fn transpile(&mut self, forth_source: &str) -> String {
        let mut out = String::with_capacity(forth_source.len() + 64);
        let mut first = true;
        for line in forth_source.lines() {
            if !first { out.push('\n'); }
            first = false;
            out.push_str(&self.transpile_line(line));
        }
        out
    }

    /// Transpile a single line.
    fn transpile_line(&mut self, line: &str) -> String {
        // Normalise tabs → spaces for uniform tokenisation.
        let line = line.replace('\t', " ");

        let mut result_tokens: Vec<String> = Vec::new();
        let mut chars = line.char_indices().peekable();

        while let Some((i, ch)) = chars.next() {
            if ch.is_ascii_whitespace() {
                continue;
            }

            // Backslash line comment: skip the rest of the line.
            // (Both Forth and Factor use `\` for line comments.)
            if ch == '\\' {
                // Consume to end of line; preserve the `\` as-is so
                // Factor's parser still treats it as a line comment.
                let rest = &line[i..];
                result_tokens.push(rest.to_string());
                break;
            }

            // String literal: "...".  Collect until closing `"` so
            // that `!` inside strings is not rewritten.
            if ch == '"' {
                let start = i;
                let mut end = i + 1;
                let bytes = line.as_bytes();
                while end < bytes.len() && bytes[end] != b'"' {
                    end += 1;
                }
                if end < bytes.len() { end += 1; } // include closing "
                result_tokens.push(line[start..end].to_string());
                // Advance `chars` past the string we consumed.
                // (Easiest: re-derive index from remaining slice.)
                for _ in start + 1..end {
                    chars.next();
                }
                continue;
            }

            // Collect a whitespace-delimited token.
            let start = i;
            let mut end = i + ch.len_utf8();
            while let Some(&(_, nc)) = chars.peek() {
                if nc.is_ascii_whitespace() { break; }
                chars.next();
                end += nc.len_utf8();
            }
            let token = &line[start..end];
            let rewritten = self.rewrite_token(token);
            result_tokens.push(rewritten);
        }

        result_tokens.join(" ")
    }

    /// Rewrite a single whitespace-delimited token.
    ///
    /// Returns a `String` because some rewrites expand one token into
    /// multiple (e.g. `ELSE` → `"] ["` which contains a space).
    fn rewrite_token(&mut self, token: &str) -> String {
        match token {
            // ── Store ──────────────────────────────────────────────
            "!" => "var!".to_string(),

            // ── IF / ELSE / THEN ───────────────────────────────────
            "IF" => {
                self.ctrl_stack.push(Ctrl::If);
                "[".to_string()
            }
            "ELSE" => {
                // Replace IF → IF-ELSE on the stack.
                if let Some(top) = self.ctrl_stack.last_mut() {
                    *top = Ctrl::IfElse;
                }
                "] [".to_string()
            }
            "THEN" => {
                let had_else = self.ctrl_stack.pop() == Some(Ctrl::IfElse);
                if had_else { "] if".to_string() } else { "] when".to_string() }
            }

            // ── BEGIN / WHILE / REPEAT / UNTIL / AGAIN ─────────────
            "BEGIN" => {
                self.ctrl_stack.push(Ctrl::Begin);
                "[".to_string()
            }
            "WHILE" => {
                if let Some(top) = self.ctrl_stack.last_mut() {
                    *top = Ctrl::While;
                }
                "] [".to_string()
            }
            "REPEAT" => {
                self.ctrl_stack.pop();
                "] while".to_string()
            }
            "UNTIL" => {
                self.ctrl_stack.pop();
                "] until".to_string()
            }
            "AGAIN" => {
                self.ctrl_stack.pop();
                "] loop".to_string()
            }

            // ── Pass through ───────────────────────────────────────
            other => other.to_string(),
        }
    }
}

impl Default for Transpiler {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_rewrite() {
        let mut t = Transpiler::new();
        assert_eq!(t.transpile("42 x !"), "42 x var!");
    }

    #[test]
    fn if_then() {
        let mut t = Transpiler::new();
        let src = ": pos ( n -- ) 0 > IF \"pos\" print THEN ;";
        let out = t.transpile(src);
        assert!(out.contains("] when"), "got: {out}");
    }

    #[test]
    fn if_else_then() {
        let mut t = Transpiler::new();
        let src = ": yn ( n -- ) 0 > IF \"yes\" print ELSE \"no\" print THEN ;";
        let out = t.transpile(src);
        assert!(out.contains("] if"), "got: {out}");
    }

    #[test]
    fn begin_until() {
        let mut t = Transpiler::new();
        let src = ": cd ( n -- ) BEGIN dup . 1 - dup 0 < UNTIL drop ;";
        let out = t.transpile(src);
        assert!(out.contains("] until"), "got: {out}");
    }

    #[test]
    fn begin_while_repeat() {
        let mut t = Transpiler::new();
        let src = ": cd ( n -- ) BEGIN dup 0 > WHILE dup . 1 - REPEAT drop ;";
        let out = t.transpile(src);
        assert!(out.contains("] while"), "got: {out}");
    }

    #[test]
    fn string_literal_not_rewritten() {
        let mut t = Transpiler::new();
        // `!` inside a string must NOT become `var!`.
        let src = r#""hello ! world" print"#;
        let out = t.transpile(src);
        assert!(out.contains("\"hello ! world\""), "got: {out}");
        assert!(!out.contains("var!"), "got: {out}");
    }

    #[test]
    fn ctrl_stack_persists_across_calls() {
        let mut t = Transpiler::new();
        // Multi-line input split across calls.
        t.transpile(": foo ( n -- )");
        t.transpile("    0 > IF");
        let line3 = t.transpile("        \"pos\" print THEN ;");
        assert!(line3.contains("] when"), "got: {line3}");
    }
}
