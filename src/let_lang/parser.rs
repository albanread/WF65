//! Parser for the LET DSL.
//!
//! Grammar (informal):
//!
//! ```text
//! let-form  := 'LET' '(' ident-list ')' '->' '(' ident-list ')' '='
//!              expr (',' expr)* where-clause* 'END'
//! where     := 'WHERE' ident '=' expr
//! expr      := add-expr
//! add-expr  := mul-expr (('+' | '-') mul-expr)*
//! mul-expr  := pow-expr (('*' | '/') pow-expr)*
//! pow-expr  := unary ('**' pow-expr)?     // right-associative
//! unary     := '-' unary | postfix
//! postfix   := primary call-args?
//! primary   := number | ident | '(' expr ')'
//! ```
//!
//! Comment forms inherit Forth conventions: `\` to end-of-line, `( ... )` inline.

use std::fmt;

#[derive(Debug, Clone)]
pub enum Expr {
    Lit(f64),
    Var(String),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Neg(Box<Expr>),
    Call(String, Vec<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add, Sub, Mul, Div, Pow,
    // Comparison ops: each yields 1.0 (true) or 0.0 (false).
    Eq, Ne, Lt, Gt, Le, Ge,
}

#[derive(Debug)]
pub struct LetForm {
    pub inputs:  Vec<String>,
    pub outputs: Vec<String>,
    pub results: Vec<Expr>,
    pub wheres:  Vec<(String, Expr)>,
}

#[derive(Debug, Clone)]
pub struct LetError {
    pub message: String,
    pub pos: usize,
}

impl fmt::Display for LetError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "LET error at byte {}: {}", self.pos, self.message)
    }
}

impl std::error::Error for LetError {}

// ── Lexer ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    LParen, RParen, Comma, Equals, Arrow,
    Plus, Minus, Star, Slash, StarStar,
    EqEq, NotEq, Less, Greater, LessEq, GreaterEq,
    LetKw, EndKw, WhereKw,
    Ident(String),
    Num(f64),
    Eof,
}

struct Lexer<'s> {
    src: &'s [u8],
    pos: usize,
}

impl<'s> Lexer<'s> {
    fn new(src: &'s str) -> Self { Self { src: src.as_bytes(), pos: 0 } }

    fn skip_ws(&mut self) {
        loop {
            while self.pos < self.src.len() {
                let c = self.src[self.pos];
                if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
                    self.pos += 1;
                } else {
                    break;
                }
            }
            // `\` line comment (must be followed by whitespace or EOL to count).
            if self.pos < self.src.len() && self.src[self.pos] == b'\\' {
                let next = self.src.get(self.pos + 1).copied().unwrap_or(b' ');
                if next == b' ' || next == b'\t' || next == b'\n' || next == b'\r' {
                    while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
                        self.pos += 1;
                    }
                    continue;
                }
            }
            // `( ... )` Forth inline comment — only when followed by whitespace,
            // so we don't gobble grouping parens.
            if self.pos + 1 < self.src.len()
                && self.src[self.pos] == b'('
                && (self.src[self.pos + 1] == b' ' || self.src[self.pos + 1] == b'\t')
            {
                self.pos += 1;
                while self.pos < self.src.len() && self.src[self.pos] != b')' {
                    self.pos += 1;
                }
                if self.pos < self.src.len() { self.pos += 1; }
                continue;
            }
            break;
        }
    }

    fn next_tok(&mut self) -> Result<(Tok, usize), LetError> {
        self.skip_ws();
        let start = self.pos;
        if self.pos >= self.src.len() {
            return Ok((Tok::Eof, start));
        }
        let c = self.src[self.pos];
        match c {
            b'(' => { self.pos += 1; Ok((Tok::LParen, start)) }
            b')' => { self.pos += 1; Ok((Tok::RParen, start)) }
            b',' => { self.pos += 1; Ok((Tok::Comma, start)) }
            b'=' => {
                if self.src.get(self.pos + 1) == Some(&b'=') {
                    self.pos += 2;
                    Ok((Tok::EqEq, start))
                } else {
                    self.pos += 1;
                    Ok((Tok::Equals, start))
                }
            }
            b'!' => {
                if self.src.get(self.pos + 1) == Some(&b'=') {
                    self.pos += 2;
                    Ok((Tok::NotEq, start))
                } else {
                    return Err(LetError {
                        message: "stray '!' (did you mean '!='?)".into(),
                        pos: start,
                    });
                }
            }
            b'<' => {
                if self.src.get(self.pos + 1) == Some(&b'=') {
                    self.pos += 2;
                    Ok((Tok::LessEq, start))
                } else {
                    self.pos += 1;
                    Ok((Tok::Less, start))
                }
            }
            b'>' => {
                if self.src.get(self.pos + 1) == Some(&b'=') {
                    self.pos += 2;
                    Ok((Tok::GreaterEq, start))
                } else {
                    self.pos += 1;
                    Ok((Tok::Greater, start))
                }
            }
            b'+' => { self.pos += 1; Ok((Tok::Plus, start)) }
            b'-' => {
                if self.src.get(self.pos + 1) == Some(&b'>') {
                    self.pos += 2;
                    Ok((Tok::Arrow, start))
                } else {
                    self.pos += 1;
                    Ok((Tok::Minus, start))
                }
            }
            b'*' => {
                if self.src.get(self.pos + 1) == Some(&b'*') {
                    self.pos += 2;
                    Ok((Tok::StarStar, start))
                } else {
                    self.pos += 1;
                    Ok((Tok::Star, start))
                }
            }
            b'/' => { self.pos += 1; Ok((Tok::Slash, start)) }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let mut end = self.pos + 1;
                while end < self.src.len() {
                    let ch = self.src[end];
                    if ch.is_ascii_alphanumeric() || ch == b'_' { end += 1; }
                    else { break; }
                }
                let word = std::str::from_utf8(&self.src[self.pos..end])
                    .map_err(|_| LetError {
                        message: "non-UTF8 identifier".into(),
                        pos: start,
                    })?
                    .to_string();
                self.pos = end;
                let tok = match word.as_str() {
                    "LET" => Tok::LetKw,
                    "END" => Tok::EndKw,
                    "WHERE" => Tok::WhereKw,
                    _ => Tok::Ident(word),
                };
                Ok((tok, start))
            }
            c if c.is_ascii_digit() || c == b'.' => {
                let mut end = self.pos;
                let mut has_dot = false;
                while end < self.src.len() {
                    let ch = self.src[end];
                    if ch.is_ascii_digit() { end += 1; }
                    else if ch == b'.' && !has_dot { has_dot = true; end += 1; }
                    else if ch == b'e' || ch == b'E' {
                        end += 1;
                        if end < self.src.len()
                            && (self.src[end] == b'+' || self.src[end] == b'-')
                        {
                            end += 1;
                        }
                    } else { break; }
                }
                // Reject a lone "." with no digits.
                if end == self.pos + 1 && self.src[self.pos] == b'.' {
                    return Err(LetError {
                        message: "lone '.' isn't a number".into(),
                        pos: start,
                    });
                }
                let s = std::str::from_utf8(&self.src[self.pos..end]).unwrap();
                let n: f64 = s.parse().map_err(|_| LetError {
                    message: format!("invalid number '{s}'"),
                    pos: start,
                })?;
                self.pos = end;
                Ok((Tok::Num(n), start))
            }
            _ => Err(LetError {
                message: format!("unexpected character '{}'", c as char),
                pos: start,
            }),
        }
    }
}

// ── Parser ───────────────────────────────────────────────────────────

struct Parser<'s> {
    lex: Lexer<'s>,
    cur: (Tok, usize),
}

impl<'s> Parser<'s> {
    fn new(src: &'s str) -> Result<Self, LetError> {
        let mut lex = Lexer::new(src);
        let cur = lex.next_tok()?;
        Ok(Self { lex, cur })
    }

    fn bump(&mut self) -> Result<(Tok, usize), LetError> {
        let prev = std::mem::replace(&mut self.cur, self.lex.next_tok()?);
        Ok(prev)
    }

    fn expect(&mut self, t: &Tok) -> Result<(), LetError> {
        if std::mem::discriminant(&self.cur.0) == std::mem::discriminant(t) {
            self.bump()?;
            Ok(())
        } else {
            Err(LetError {
                message: format!("expected {t:?}, got {:?}", self.cur.0),
                pos: self.cur.1,
            })
        }
    }

    fn ident_list(&mut self) -> Result<Vec<String>, LetError> {
        self.expect(&Tok::LParen)?;
        let mut out = Vec::new();
        if self.cur.0 != Tok::RParen {
            loop {
                if let Tok::Ident(name) = &self.cur.0 {
                    let n = name.clone();
                    self.bump()?;
                    out.push(n);
                } else {
                    return Err(LetError {
                        message: format!("expected identifier, got {:?}", self.cur.0),
                        pos: self.cur.1,
                    });
                }
                if self.cur.0 == Tok::Comma { self.bump()?; }
                else { break; }
            }
        }
        self.expect(&Tok::RParen)?;
        Ok(out)
    }

    fn parse_form(&mut self) -> Result<LetForm, LetError> {
        self.expect(&Tok::LetKw)?;
        let inputs = self.ident_list()?;
        self.expect(&Tok::Arrow)?;
        let outputs = self.ident_list()?;
        self.expect(&Tok::Equals)?;
        let mut results = Vec::new();
        loop {
            results.push(self.parse_expr()?);
            if self.cur.0 == Tok::Comma { self.bump()?; }
            else { break; }
        }
        let mut wheres = Vec::new();
        while self.cur.0 == Tok::WhereKw {
            self.bump()?;
            let name = if let Tok::Ident(n) = &self.cur.0 {
                let n = n.clone();
                self.bump()?;
                n
            } else {
                return Err(LetError {
                    message: format!("WHERE expects identifier, got {:?}", self.cur.0),
                    pos: self.cur.1,
                });
            };
            self.expect(&Tok::Equals)?;
            let e = self.parse_expr()?;
            wheres.push((name, e));
        }
        self.expect(&Tok::EndKw)?;
        if outputs.len() != results.len() {
            return Err(LetError {
                message: format!(
                    "LET declares {} outputs but body has {} result expressions",
                    outputs.len(), results.len()
                ),
                pos: 0,
            });
        }
        Ok(LetForm { inputs, outputs, results, wheres })
    }

    fn parse_expr(&mut self) -> Result<Expr, LetError> { self.parse_compare() }

    /// Comparisons (`< > <= >= == !=`) at one precedence level below
    /// `+`/`-`.  Left-associative; the result of each comparison is a
    /// numeric value (1.0 true, 0.0 false) that can flow into arithmetic
    /// or into `select(cond, ...)`.
    fn parse_compare(&mut self) -> Result<Expr, LetError> {
        let mut lhs = self.parse_add()?;
        loop {
            let op = match self.cur.0 {
                Tok::Less      => BinOp::Lt,
                Tok::Greater   => BinOp::Gt,
                Tok::LessEq    => BinOp::Le,
                Tok::GreaterEq => BinOp::Ge,
                Tok::EqEq      => BinOp::Eq,
                Tok::NotEq     => BinOp::Ne,
                _ => break,
            };
            self.bump()?;
            let rhs = self.parse_add()?;
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_add(&mut self) -> Result<Expr, LetError> {
        let mut lhs = self.parse_mul()?;
        loop {
            let op = match self.cur.0 {
                Tok::Plus  => BinOp::Add,
                Tok::Minus => BinOp::Sub,
                _ => break,
            };
            self.bump()?;
            let rhs = self.parse_mul()?;
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_mul(&mut self) -> Result<Expr, LetError> {
        let mut lhs = self.parse_pow()?;
        loop {
            let op = match self.cur.0 {
                Tok::Star  => BinOp::Mul,
                Tok::Slash => BinOp::Div,
                _ => break,
            };
            self.bump()?;
            let rhs = self.parse_pow()?;
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_pow(&mut self) -> Result<Expr, LetError> {
        let lhs = self.parse_unary()?;
        if self.cur.0 == Tok::StarStar {
            self.bump()?;
            let rhs = self.parse_pow()?;       // right-associative
            Ok(Expr::Bin(BinOp::Pow, Box::new(lhs), Box::new(rhs)))
        } else {
            Ok(lhs)
        }
    }

    fn parse_unary(&mut self) -> Result<Expr, LetError> {
        if self.cur.0 == Tok::Minus {
            self.bump()?;
            let e = self.parse_unary()?;
            Ok(Expr::Neg(Box::new(e)))
        } else {
            self.parse_primary()
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, LetError> {
        let (tok, pos) = self.bump()?;
        match tok {
            Tok::Num(n) => Ok(Expr::Lit(n)),
            Tok::Ident(name) => {
                if self.cur.0 == Tok::LParen {
                    self.bump()?;
                    let mut args = Vec::new();
                    if self.cur.0 != Tok::RParen {
                        loop {
                            args.push(self.parse_expr()?);
                            if self.cur.0 == Tok::Comma { self.bump()?; }
                            else { break; }
                        }
                    }
                    self.expect(&Tok::RParen)?;
                    Ok(Expr::Call(name, args))
                } else {
                    Ok(Expr::Var(name))
                }
            }
            Tok::LParen => {
                let e = self.parse_expr()?;
                self.expect(&Tok::RParen)?;
                Ok(e)
            }
            other => Err(LetError {
                message: format!("unexpected token in expression: {other:?}"),
                pos,
            }),
        }
    }
}

pub fn parse(source: &str) -> Result<LetForm, LetError> {
    let mut p = Parser::new(source)?;
    let form = p.parse_form()?;
    if p.cur.0 != Tok::Eof {
        return Err(LetError {
            message: format!("trailing tokens after END: {:?}", p.cur.0),
            pos: p.cur.1,
        });
    }
    Ok(form)
}

// ── Unit tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> LetForm {
        parse(s).unwrap_or_else(|e| panic!("parse failed: {e}"))
    }

    #[test]
    fn parses_minimal_let() {
        let f = p("LET (r) -> (a) = r END");
        assert_eq!(f.inputs, vec!["r"]);
        assert_eq!(f.outputs, vec!["a"]);
        assert_eq!(f.results.len(), 1);
        assert!(f.wheres.is_empty());
    }

    #[test]
    fn parses_arithmetic_with_precedence() {
        let f = p("LET (x) -> (y) = 1 + 2 * 3 END");
        // y = 1 + (2 * 3), so top-level op is Add
        match &f.results[0] {
            Expr::Bin(BinOp::Add, ..) => {},
            other => panic!("expected Add at top, got {other:?}"),
        }
    }

    #[test]
    fn parses_where_clauses() {
        let f = p("LET (x, y) -> (mag) = m WHERE m = x*x + y*y END");
        assert_eq!(f.wheres.len(), 1);
        assert_eq!(f.wheres[0].0, "m");
    }

    #[test]
    fn parses_mbrot() {
        let f = p("\
            LET (z_re, z_im, x, y) -> (z_next_re, z_next_im, mag) = \
                re, im, rmag \
                WHERE re   = (z_re * z_re) - (z_im * z_im) + x \
                WHERE im   = (2 * z_re * z_im) + y \
                WHERE rmag = (re * re) + (im * im) \
            END");
        assert_eq!(f.inputs.len(), 4);
        assert_eq!(f.outputs.len(), 3);
        assert_eq!(f.results.len(), 3);
        assert_eq!(f.wheres.len(), 3);
    }

    #[test]
    fn rejects_arity_mismatch() {
        let e = parse("LET (x) -> (a, b) = x END").unwrap_err();
        assert!(e.message.contains("outputs"));
    }

    #[test]
    fn rejects_trailing_garbage() {
        let e = parse("LET (x) -> (y) = x END oops").unwrap_err();
        assert!(e.message.contains("trailing"));
    }

    #[test]
    fn parses_pow_right_associative() {
        let f = p("LET (x) -> (y) = x ** 2 ** 3 END");
        // x ** (2 ** 3)
        match &f.results[0] {
            Expr::Bin(BinOp::Pow, _, r) => match r.as_ref() {
                Expr::Bin(BinOp::Pow, _, _) => {},
                _ => panic!("inner Pow expected on right"),
            },
            _ => panic!("Pow at top expected"),
        }
    }

    #[test]
    fn parses_unary_minus() {
        let f = p("LET (x) -> (y) = -x END");
        match &f.results[0] {
            Expr::Neg(_) => {},
            _ => panic!("expected Neg"),
        }
    }

    #[test]
    fn parses_function_call_syntax() {
        let f = p("LET (x) -> (y) = sin(x) END");
        match &f.results[0] {
            Expr::Call(name, args) => {
                assert_eq!(name, "sin");
                assert_eq!(args.len(), 1);
            }
            _ => panic!("expected Call"),
        }
    }

    #[test]
    fn lexes_line_comments() {
        let f = p("LET (x) -> (y) = x \\ ignored\n END");
        assert_eq!(f.inputs.len(), 1);
    }
}
