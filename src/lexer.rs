// Tokenizer for the .pm language: a Python subset with indentation blocks.

use crate::error::{Error, Pos};

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Int(i64),
    Float(f64),
    Str(String),
    FStr(Vec<FPart>),
    Ident(String),
    Kw(&'static str),
    Op(&'static str),
    Newline,
    Indent,
    Dedent,
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FPart {
    Lit(String),
    Expr(String, Pos),
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub pos: Pos,
}

const KEYWORDS: &[&str] = &[
    "def", "if", "elif", "else", "while", "for", "in", "return", "break", "continue", "pass",
    "del", "and", "or", "not", "True", "False", "None", "global", "is", "lambda",
];

// Longest first so that greedy matching works.
const OPS: &[&str] = &[
    "**=", "//=", "--", "**", "//", "==", "!=", "<=", ">=", "+=", "-=", "*=", "/=", "%=", "->",
    "+", "-", "*", "/", "%", "<", ">", "=", "(", ")", "[", "]", "{", "}", ",", ":", ".", ";",
];

/// Why lexing stopped early; the REPL uses `Incomplete` to ask for more lines.
#[derive(Debug)]
pub enum LexError {
    Incomplete,
    Bad(Error),
}

pub fn tokenize(src: &str) -> Result<Vec<Token>, LexError> {
    Lexer { chars: src.chars().collect(), i: 0, line: 1, col: 1 }.run()
}

struct Lexer {
    chars: Vec<char>,
    i: usize,
    line: usize,
    col: usize,
}

impl Lexer {
    fn peek(&self, k: usize) -> Option<char> {
        self.chars.get(self.i + k).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.chars.get(self.i).copied()?;
        self.i += 1;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn pos(&self) -> Pos {
        Pos { line: self.line, col: self.col }
    }

    fn err(&self, msg: impl Into<String>) -> LexError {
        LexError::Bad(Error::at(self.pos(), msg))
    }

    fn run(mut self) -> Result<Vec<Token>, LexError> {
        let mut out: Vec<Token> = Vec::new();
        let mut indents: Vec<usize> = vec![0];
        let mut depth: Vec<char> = Vec::new();
        let mut at_line_start = true;

        loop {
            if at_line_start && depth.is_empty() {
                // Measure indentation, skipping blank and comment-only lines.
                let mut width = 0;
                while let Some(c) = self.peek(0) {
                    match c {
                        ' ' => width += 1,
                        '\t' => width = (width / 8 + 1) * 8,
                        '\r' => {}
                        _ => break,
                    }
                    self.bump();
                }
                match self.peek(0) {
                    None => break,
                    Some('\n') => {
                        self.bump();
                        continue;
                    }
                    Some('#') => {
                        while self.peek(0).is_some_and(|c| c != '\n') {
                            self.bump();
                        }
                        continue;
                    }
                    _ => {}
                }
                let pos = self.pos();
                let cur = *indents.last().unwrap();
                if width > cur {
                    indents.push(width);
                    out.push(Token { tok: Tok::Indent, pos });
                } else {
                    while width < *indents.last().unwrap() {
                        indents.pop();
                        out.push(Token { tok: Tok::Dedent, pos });
                    }
                    if width != *indents.last().unwrap() {
                        return Err(LexError::Bad(Error::at(pos, "unindent does not match any outer indentation level")));
                    }
                }
                at_line_start = false;
            }

            let Some(c) = self.peek(0) else { break };
            let pos = self.pos();
            match c {
                '\n' => {
                    self.bump();
                    if depth.is_empty() {
                        if !matches!(out.last().map(|t| &t.tok), Some(Tok::Newline) | None) {
                            out.push(Token { tok: Tok::Newline, pos });
                        }
                        at_line_start = true;
                    }
                }
                ' ' | '\t' | '\r' => {
                    self.bump();
                }
                '#' => {
                    while self.peek(0).is_some_and(|c| c != '\n') {
                        self.bump();
                    }
                }
                '\\' if self.peek(1) == Some('\n') => {
                    self.bump();
                    self.bump();
                    if self.peek(0).is_none() {
                        return Err(LexError::Incomplete);
                    }
                }
                '0'..='9' => out.push(Token { tok: self.number()?, pos }),
                '.' if self.peek(1).is_some_and(|d| d.is_ascii_digit()) => {
                    out.push(Token { tok: self.number()?, pos })
                }
                '"' | '\'' => {
                    let s = self.string()?;
                    out.push(Token { tok: Tok::Str(s), pos });
                }
                c if c.is_alphabetic() || c == '_' => {
                    let mut s = String::new();
                    while let Some(c) = self.peek(0).filter(|c| c.is_alphanumeric() || *c == '_') {
                        s.push(c);
                        self.bump();
                    }
                    let is_prefix = matches!(s.as_str(), "f" | "F" | "r" | "R");
                    if is_prefix && matches!(self.peek(0), Some('"' | '\'')) {
                        let tok = if s.eq_ignore_ascii_case("f") {
                            let fpos = self.pos();
                            let raw = self.string()?;
                            Tok::FStr(split_fstring(&raw, fpos)?)
                        } else {
                            Tok::Str(self.raw_string()?)
                        };
                        out.push(Token { tok, pos });
                    } else if let Some(kw) = KEYWORDS.iter().find(|k| **k == s) {
                        out.push(Token { tok: Tok::Kw(kw), pos });
                    } else {
                        out.push(Token { tok: Tok::Ident(s), pos });
                    }
                }
                _ => {
                    let op = OPS
                        .iter()
                        .find(|op| op.chars().enumerate().all(|(k, oc)| self.peek(k) == Some(oc)))
                        .ok_or_else(|| self.err(format!("unexpected character '{c}'")))?;
                    for _ in 0..op.chars().count() {
                        self.bump();
                    }
                    match *op {
                        "(" | "[" | "{" => depth.push(c),
                        ")" | "]" | "}" => {
                            let want = match c {
                                ')' => '(',
                                ']' => '[',
                                _ => '{',
                            };
                            if depth.pop() != Some(want) {
                                return Err(LexError::Bad(Error::at(pos, format!("unmatched '{c}'"))));
                            }
                        }
                        _ => {}
                    }
                    out.push(Token { tok: Tok::Op(op), pos });
                }
            }
        }

        if !depth.is_empty() {
            return Err(LexError::Incomplete);
        }
        let pos = self.pos();
        if !matches!(out.last().map(|t| &t.tok), Some(Tok::Newline) | None) {
            out.push(Token { tok: Tok::Newline, pos });
        }
        while indents.len() > 1 {
            indents.pop();
            out.push(Token { tok: Tok::Dedent, pos });
        }
        out.push(Token { tok: Tok::Eof, pos });
        Ok(out)
    }

    /// Numbers, with optional size suffixes: `500kb`, `1.5mb`, `2g`, `100b`.
    fn number(&mut self) -> Result<Tok, LexError> {
        let mut s = String::new();
        let mut is_float = false;
        while let Some(c) = self.peek(0) {
            if c.is_ascii_digit() || c == '_' {
                s.push(c);
            } else if c == '.' && !is_float && self.peek(1).is_none_or(|d| d.is_ascii_digit() || !d.is_alphabetic()) {
                is_float = true;
                s.push(c);
            } else if (c == 'e' || c == 'E')
                && self.peek(1).is_some_and(|d| d.is_ascii_digit() || ((d == '-' || d == '+') && self.peek(2).is_some_and(|d| d.is_ascii_digit())))
            {
                is_float = true;
                s.push(c);
                self.bump();
                s.push(self.peek(0).unwrap());
            } else {
                break;
            }
            self.bump();
        }
        let s = s.replace('_', "");
        let mut suffix = String::new();
        while let Some(c) = self.peek(0).filter(|c| c.is_alphanumeric() || *c == '_') {
            suffix.push(c);
            self.bump();
        }
        let value: f64 = s.parse().map_err(|_| self.err(format!("invalid number '{s}'")))?;
        if suffix.is_empty() {
            return Ok(if is_float {
                Tok::Float(value)
            } else {
                Tok::Int(s.parse().map_err(|_| self.err(format!("integer too large '{s}'")))?)
            });
        }
        let mult: f64 = match suffix.to_ascii_lowercase().as_str() {
            "b" => 1.0,
            "k" | "kb" | "kib" => 1024.0,
            "m" | "mb" | "mib" => 1024.0 * 1024.0,
            "g" | "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
            _ => return Err(self.err(format!("invalid number suffix '{suffix}' (size units are b, kb, mb, gb)"))),
        };
        Ok(Tok::Int((value * mult).floor() as i64))
    }

    fn string(&mut self) -> Result<String, LexError> {
        let quote = self.bump().unwrap();
        let triple = self.peek(0) == Some(quote) && self.peek(1) == Some(quote);
        if triple {
            self.bump();
            self.bump();
        }
        let mut s = String::new();
        loop {
            let Some(c) = self.bump() else { return Err(LexError::Incomplete) };
            if c == quote {
                if !triple {
                    return Ok(s);
                }
                if self.peek(0) == Some(quote) && self.peek(1) == Some(quote) {
                    self.bump();
                    self.bump();
                    return Ok(s);
                }
                s.push(c);
            } else if c == '\n' && !triple {
                return Err(self.err("unterminated string literal"));
            } else if c == '\\' {
                let Some(e) = self.bump() else { return Err(LexError::Incomplete) };
                match e {
                    'n' => s.push('\n'),
                    't' => s.push('\t'),
                    'r' => s.push('\r'),
                    '0' => s.push('\0'),
                    '\\' => s.push('\\'),
                    '\'' => s.push('\''),
                    '"' => s.push('"'),
                    '\n' => {}
                    other => {
                        s.push('\\');
                        s.push(other);
                    }
                }
            } else {
                s.push(c);
            }
        }
    }

    fn raw_string(&mut self) -> Result<String, LexError> {
        let quote = self.bump().unwrap();
        let mut s = String::new();
        loop {
            match self.bump() {
                None => return Err(LexError::Incomplete),
                Some(c) if c == quote => return Ok(s),
                Some('\n') => return Err(self.err("unterminated string literal")),
                Some(c) => s.push(c),
            }
        }
    }
}

fn split_fstring(raw: &str, pos: Pos) -> Result<Vec<FPart>, LexError> {
    let mut parts = Vec::new();
    let mut lit = String::new();
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '{' && chars.get(i + 1) == Some(&'{') {
            lit.push('{');
            i += 2;
        } else if c == '}' && chars.get(i + 1) == Some(&'}') {
            lit.push('}');
            i += 2;
        } else if c == '{' {
            let mut depth = 1;
            let mut j = i + 1;
            while j < chars.len() {
                match chars[j] {
                    '{' | '[' | '(' => depth += 1,
                    '}' | ']' | ')' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            if j >= chars.len() {
                return Err(LexError::Bad(Error::at(pos, "f-string: expecting '}'")));
            }
            if !lit.is_empty() {
                parts.push(FPart::Lit(std::mem::take(&mut lit)));
            }
            parts.push(FPart::Expr(chars[i + 1..j].iter().collect(), pos));
            i = j + 1;
        } else if c == '}' {
            return Err(LexError::Bad(Error::at(pos, "f-string: single '}' is not allowed")));
        } else {
            lit.push(c);
            i += 1;
        }
    }
    if !lit.is_empty() {
        parts.push(FPart::Lit(lit));
    }
    Ok(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        tokenize(s).unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn sizes() {
        assert_eq!(toks("2mb")[0], Tok::Int(2 * 1024 * 1024));
        assert_eq!(toks("1.5KB")[0], Tok::Int(1536));
        assert_eq!(toks("300")[0], Tok::Int(300));
    }

    #[test]
    fn double_minus_is_one_op() {
        assert_eq!(toks("p -- 1mb")[1], Tok::Op("--"));
        assert_eq!(toks("p - -1")[1], Tok::Op("-"));
    }

    #[test]
    fn indentation() {
        let t = toks("if x:\n    y\nz\n");
        assert!(t.contains(&Tok::Indent));
        assert!(t.contains(&Tok::Dedent));
    }

    #[test]
    fn incomplete() {
        assert!(matches!(tokenize("f(1,\n"), Err(LexError::Incomplete)));
        assert!(matches!(tokenize("'''abc"), Err(LexError::Incomplete)));
    }
}
