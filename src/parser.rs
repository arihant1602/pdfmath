// Recursive-descent parser following Python's operator precedence.

use std::rc::Rc;

use crate::ast::*;
use crate::error::{Error, Pos, Result};
use crate::lexer::{FPart, Tok, Token};

pub fn parse(tokens: Vec<Token>) -> Result<Vec<Stmt>> {
    let mut p = Parser { toks: tokens, i: 0 };
    let mut out = Vec::new();
    while !p.at(&Tok::Eof) {
        if p.eat(&Tok::Newline) {
            continue;
        }
        out.extend(p.statement()?);
    }
    Ok(out)
}

/// Parse a single expression from source (used for f-string pieces).
pub fn parse_expr_src(src: &str, pos: Pos) -> Result<Expr> {
    let toks = crate::lexer::tokenize(src).map_err(|e| match e {
        crate::lexer::LexError::Bad(e) => e,
        crate::lexer::LexError::Incomplete => Error::at(pos, "f-string: incomplete expression"),
    })?;
    let mut p = Parser { toks, i: 0 };
    let e = p.expr_list().map_err(|e| Error::at(pos, e.msg))?;
    p.eat(&Tok::Newline);
    if !p.at(&Tok::Eof) {
        return Err(Error::at(pos, "f-string: invalid expression"));
    }
    Ok(e)
}

struct Parser {
    toks: Vec<Token>,
    i: usize,
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::Int(n) => n.to_string(),
        Tok::Float(f) => f.to_string(),
        Tok::Str(_) | Tok::FStr(_) => "string".into(),
        Tok::Ident(s) => format!("'{s}'"),
        Tok::Kw(k) => format!("'{k}'"),
        Tok::Op(o) => format!("'{o}'"),
        Tok::Newline => "end of line".into(),
        Tok::Indent => "indent".into(),
        Tok::Dedent => "dedent".into(),
        Tok::Eof => "end of input".into(),
    }
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.i].tok
    }

    fn peek_at(&self, k: usize) -> &Tok {
        &self.toks[(self.i + k).min(self.toks.len() - 1)].tok
    }

    fn pos(&self) -> Pos {
        self.toks[self.i].pos
    }

    fn at(&self, t: &Tok) -> bool {
        self.peek() == t
    }

    fn at_op(&self, op: &str) -> bool {
        matches!(self.peek(), Tok::Op(o) if *o == op)
    }

    fn at_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Kw(k) if *k == kw)
    }

    fn next(&mut self) -> Token {
        let t = self.toks[self.i].clone();
        if self.i < self.toks.len() - 1 {
            self.i += 1;
        }
        t
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.at(t) {
            self.next();
            true
        } else {
            false
        }
    }

    fn eat_op(&mut self, op: &str) -> bool {
        if self.at_op(op) {
            self.next();
            true
        } else {
            false
        }
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.at_kw(kw) {
            self.next();
            true
        } else {
            false
        }
    }

    fn unexpected(&self) -> Error {
        Error::at(self.pos(), format!("syntax error: unexpected {}", describe(self.peek())))
    }

    fn expect_op(&mut self, op: &str) -> Result<()> {
        if self.eat_op(op) {
            Ok(())
        } else {
            Err(Error::at(self.pos(), format!("syntax error: expected '{op}', found {}", describe(self.peek()))))
        }
    }

    fn ident(&mut self) -> Result<String> {
        match self.peek().clone() {
            Tok::Ident(s) => {
                self.next();
                Ok(s)
            }
            _ => Err(Error::at(self.pos(), format!("syntax error: expected a name, found {}", describe(self.peek())))),
        }
    }

    // ---- statements ----

    fn statement(&mut self) -> Result<Vec<Stmt>> {
        let pos = self.pos();
        let compound = match self.peek() {
            Tok::Kw("if") => Some(self.if_stmt()?),
            Tok::Kw("while") => {
                self.next();
                let cond = self.expr()?;
                let body = self.block()?;
                Some(StmtKind::While(cond, body))
            }
            Tok::Kw("for") => {
                self.next();
                let target = self.target_list()?;
                if !self.eat_kw("in") {
                    return Err(Error::at(self.pos(), "syntax error: expected 'in'"));
                }
                let iter = self.expr_list()?;
                let body = self.block()?;
                Some(StmtKind::For(target, iter, body))
            }
            Tok::Kw("def") => {
                self.next();
                let name = self.ident()?;
                self.expect_op("(")?;
                let params = self.params(")")?;
                self.expect_op(")")?;
                if self.eat_op("->") {
                    self.expr()?;
                }
                let body = self.block()?;
                Some(StmtKind::Def(Rc::new(FuncDef { name, params, body })))
            }
            Tok::Indent => return Err(Error::at(pos, "unexpected indent")),
            _ => None,
        };
        if let Some(kind) = compound {
            return Ok(vec![Stmt { kind, pos }]);
        }
        // simple_stmt (';' simple_stmt)* NEWLINE
        let mut out = vec![self.simple()?];
        while self.eat_op(";") {
            if self.at(&Tok::Newline) || self.at(&Tok::Eof) {
                break;
            }
            out.push(self.simple()?);
        }
        if !self.eat(&Tok::Newline) && !self.at(&Tok::Eof) && !self.at(&Tok::Dedent) {
            return Err(self.unexpected());
        }
        Ok(out)
    }

    fn if_stmt(&mut self) -> Result<StmtKind> {
        self.next();
        let mut arms = vec![(self.expr()?, self.block()?)];
        let mut orelse = None;
        loop {
            if self.eat_kw("elif") {
                arms.push((self.expr()?, self.block()?));
            } else if self.eat_kw("else") {
                orelse = Some(self.block()?);
                break;
            } else {
                break;
            }
        }
        Ok(StmtKind::If(arms, orelse))
    }

    fn block(&mut self) -> Result<Vec<Stmt>> {
        self.expect_op(":")?;
        if self.eat(&Tok::Newline) {
            if !self.eat(&Tok::Indent) {
                return Err(Error::at(self.pos(), "expected an indented block"));
            }
            let mut body = Vec::new();
            while !self.eat(&Tok::Dedent) {
                if self.at(&Tok::Eof) {
                    break;
                }
                if self.eat(&Tok::Newline) {
                    continue;
                }
                body.extend(self.statement()?);
            }
            Ok(body)
        } else {
            // One-line body: `if x: y; z`
            let mut body = vec![self.simple()?];
            while self.eat_op(";") {
                if self.at(&Tok::Newline) {
                    break;
                }
                body.push(self.simple()?);
            }
            if !self.eat(&Tok::Newline) && !self.at(&Tok::Eof) {
                return Err(self.unexpected());
            }
            Ok(body)
        }
    }

    fn params(&mut self, close: &str) -> Result<Vec<(String, Option<Expr>)>> {
        let mut params = Vec::new();
        while !self.at_op(close) {
            let name = self.ident()?;
            if close == ")" && self.eat_op(":") {
                self.expr()?; // type annotation, ignored
            }
            let default = if self.eat_op("=") { Some(self.expr()?) } else { None };
            if default.is_none() && params.iter().any(|(_, d): &(String, Option<Expr>)| d.is_some()) {
                return Err(Error::at(self.pos(), "non-default argument follows default argument"));
            }
            params.push((name, default));
            if !self.eat_op(",") {
                break;
            }
        }
        Ok(params)
    }

    fn simple(&mut self) -> Result<Stmt> {
        let pos = self.pos();
        let kind = match self.peek() {
            Tok::Kw("pass") => {
                self.next();
                StmtKind::Pass
            }
            Tok::Kw("break") => {
                self.next();
                StmtKind::Break
            }
            Tok::Kw("continue") => {
                self.next();
                StmtKind::Continue
            }
            Tok::Kw("return") => {
                self.next();
                if self.at(&Tok::Newline) || self.at_op(";") || self.at(&Tok::Eof) {
                    StmtKind::Return(None)
                } else {
                    StmtKind::Return(Some(self.expr_list()?))
                }
            }
            Tok::Kw("del") => {
                self.next();
                let mut targets = vec![self.expr()?];
                while self.eat_op(",") {
                    targets.push(self.expr()?);
                }
                for t in &targets {
                    check_target(t)?;
                }
                StmtKind::Del(targets)
            }
            Tok::Kw("global") => {
                self.next();
                let mut names = vec![self.ident()?];
                while self.eat_op(",") {
                    names.push(self.ident()?);
                }
                StmtKind::Global(names)
            }
            _ => {
                let first = self.expr_list()?;
                if let Tok::Op(op @ ("+=" | "-=" | "*=" | "/=" | "//=" | "%=" | "**=")) = self.peek().clone() {
                    self.next();
                    check_target(&first)?;
                    if matches!(first.kind, ExprKind::Tuple(_)) {
                        return Err(Error::at(pos, "illegal expression for augmented assignment"));
                    }
                    let value = self.expr_list()?;
                    StmtKind::AugAssign(first, &op[..op.len() - 1], value)
                } else if self.at_op("=") {
                    let mut targets = vec![first];
                    let mut value;
                    loop {
                        self.expect_op("=")?;
                        value = self.expr_list()?;
                        if !self.at_op("=") {
                            break;
                        }
                        targets.push(value);
                    }
                    for t in &targets {
                        check_target(t)?;
                    }
                    StmtKind::Assign(targets, value)
                } else {
                    StmtKind::Expr(first)
                }
            }
        };
        Ok(Stmt { kind, pos })
    }

    /// Loop targets: `x` or `i, page` (stops before `in`).
    fn target_list(&mut self) -> Result<Expr> {
        let pos = self.pos();
        let first = self.bitor()?;
        if !self.at_op(",") {
            check_target(&first)?;
            return Ok(first);
        }
        let mut items = vec![first];
        while self.eat_op(",") {
            if self.at_kw("in") {
                break;
            }
            items.push(self.bitor()?);
        }
        let e = Expr { kind: ExprKind::Tuple(items), pos };
        check_target(&e)?;
        Ok(e)
    }

    // ---- expressions ----

    /// `a, b, c` → tuple; a single expression otherwise.
    fn expr_list(&mut self) -> Result<Expr> {
        let pos = self.pos();
        let first = self.expr()?;
        if !self.at_op(",") {
            return Ok(first);
        }
        let mut items = vec![first];
        while self.eat_op(",") {
            if self.ends_expr_list() {
                break;
            }
            items.push(self.expr()?);
        }
        Ok(Expr { kind: ExprKind::Tuple(items), pos })
    }

    fn ends_expr_list(&self) -> bool {
        matches!(self.peek(), Tok::Newline | Tok::Eof | Tok::Op("=" | ")" | "]" | ";" | ":"))
            || matches!(self.peek(), Tok::Op(o) if o.ends_with('=') && *o != "==" && *o != "!=" && *o != "<=" && *o != ">=")
    }

    pub fn expr(&mut self) -> Result<Expr> {
        let pos = self.pos();
        if self.eat_kw("lambda") {
            let params = self.params(":")?;
            self.expect_op(":")?;
            let body_pos = self.pos();
            let body = self.expr()?;
            let ret = Stmt { kind: StmtKind::Return(Some(body)), pos: body_pos };
            return Ok(Expr {
                kind: ExprKind::Lambda(Rc::new(FuncDef { name: "<lambda>".into(), params, body: vec![ret] })),
                pos,
            });
        }
        let e = self.or_expr()?;
        if self.at_kw("if") {
            // Ternary `a if cond else b`; never consumes a comprehension's `if`.
            let save = self.i;
            self.next();
            let cond = self.or_expr()?;
            if !self.eat_kw("else") {
                self.i = save;
                return Ok(e);
            }
            let other = self.expr()?;
            return Ok(Expr { kind: ExprKind::IfElse(Box::new(cond), Box::new(e), Box::new(other)), pos });
        }
        Ok(e)
    }

    fn or_expr(&mut self) -> Result<Expr> {
        let mut l = self.and_expr()?;
        while self.at_kw("or") {
            let pos = self.pos();
            self.next();
            let r = self.and_expr()?;
            l = Expr { kind: ExprKind::Or(Box::new(l), Box::new(r)), pos };
        }
        Ok(l)
    }

    fn and_expr(&mut self) -> Result<Expr> {
        let mut l = self.not_expr()?;
        while self.at_kw("and") {
            let pos = self.pos();
            self.next();
            let r = self.not_expr()?;
            l = Expr { kind: ExprKind::And(Box::new(l), Box::new(r)), pos };
        }
        Ok(l)
    }

    fn not_expr(&mut self) -> Result<Expr> {
        let pos = self.pos();
        if self.eat_kw("not") {
            let e = self.not_expr()?;
            return Ok(Expr { kind: ExprKind::Unary("not", Box::new(e)), pos });
        }
        self.comparison()
    }

    fn comparison(&mut self) -> Result<Expr> {
        let pos = self.pos();
        let first = self.bitor()?;
        let mut rest = Vec::new();
        loop {
            let op = match self.peek() {
                Tok::Op(o @ ("==" | "!=" | "<" | ">" | "<=" | ">=")) => {
                    let o = *o;
                    self.next();
                    o
                }
                Tok::Kw("in") => {
                    self.next();
                    "in"
                }
                Tok::Kw("not") if matches!(self.peek_at(1), Tok::Kw("in")) => {
                    self.next();
                    self.next();
                    "not in"
                }
                Tok::Kw("is") => {
                    self.next();
                    if self.eat_kw("not") { "is not" } else { "is" }
                }
                _ => break,
            };
            rest.push((op, self.bitor()?));
        }
        if rest.is_empty() {
            Ok(first)
        } else {
            Ok(Expr { kind: ExprKind::Compare(Box::new(first), rest), pos })
        }
    }

    // No bitwise operators in the language; this is just the level above arithmetic.
    fn bitor(&mut self) -> Result<Expr> {
        self.additive()
    }

    fn additive(&mut self) -> Result<Expr> {
        let mut l = self.term()?;
        while let Tok::Op(op @ ("+" | "-" | "--")) = *self.peek() {
            let pos = self.pos();
            self.next();
            let r = self.term()?;
            l = Expr { kind: ExprKind::Binary(op, Box::new(l), Box::new(r)), pos };
        }
        Ok(l)
    }

    fn term(&mut self) -> Result<Expr> {
        let mut l = self.unary()?;
        while let Tok::Op(op @ ("*" | "/" | "//" | "%")) = *self.peek() {
            let pos = self.pos();
            self.next();
            let r = self.unary()?;
            l = Expr { kind: ExprKind::Binary(op, Box::new(l), Box::new(r)), pos };
        }
        Ok(l)
    }

    fn unary(&mut self) -> Result<Expr> {
        let pos = self.pos();
        match self.peek() {
            Tok::Op(o @ ("-" | "+")) => {
                let o = *o;
                self.next();
                let e = self.unary()?;
                Ok(Expr { kind: ExprKind::Unary(o, Box::new(e)), pos })
            }
            Tok::Op("--") => {
                // `--x` at the start of an operand is just a double negation.
                self.next();
                let e = self.unary()?;
                let inner = Expr { kind: ExprKind::Unary("-", Box::new(e)), pos };
                Ok(Expr { kind: ExprKind::Unary("-", Box::new(inner)), pos })
            }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> Result<Expr> {
        let base = self.postfix()?;
        if self.at_op("**") {
            let pos = self.pos();
            self.next();
            let exp = self.unary()?;
            return Ok(Expr { kind: ExprKind::Binary("**", Box::new(base), Box::new(exp)), pos });
        }
        Ok(base)
    }

    fn postfix(&mut self) -> Result<Expr> {
        let mut e = self.atom()?;
        loop {
            let pos = self.pos();
            if self.eat_op("(") {
                let args = self.call_args()?;
                self.expect_op(")")?;
                e = Expr { kind: ExprKind::Call(Box::new(e), args), pos };
            } else if self.eat_op("[") {
                let idx = self.subscript()?;
                self.expect_op("]")?;
                e = Expr { kind: ExprKind::Index(Box::new(e), Box::new(idx)), pos };
            } else if self.eat_op(".") {
                let name = self.ident()?;
                e = Expr { kind: ExprKind::Attr(Box::new(e), name), pos };
            } else {
                return Ok(e);
            }
        }
    }

    fn call_args(&mut self) -> Result<Vec<Arg>> {
        let mut args = Vec::new();
        while !self.at_op(")") {
            if let (Tok::Ident(name), Tok::Op("=")) = (self.peek().clone(), self.peek_at(1).clone()) {
                self.next();
                self.next();
                args.push(Arg { name: Some(name), value: self.expr()? });
            } else {
                if args.iter().any(|a: &Arg| a.name.is_some()) {
                    return Err(Error::at(self.pos(), "positional argument follows keyword argument"));
                }
                let pos = self.pos();
                let value = self.expr()?;
                if self.at_kw("for") {
                    // Generator argument: `sum(x for x in xs)`.
                    let clauses = self.comp_clauses()?;
                    args.push(Arg { name: None, value: Expr { kind: ExprKind::ListComp(Box::new(value), clauses), pos } });
                    break;
                }
                args.push(Arg { name: None, value });
            }
            if !self.eat_op(",") {
                break;
            }
        }
        Ok(args)
    }

    fn subscript(&mut self) -> Result<Index> {
        let start = if self.at_op(":") { None } else { Some(self.expr()?) };
        if !self.eat_op(":") {
            return Ok(Index::One(start.unwrap()));
        }
        let stop = if self.at_op(":") || self.at_op("]") { None } else { Some(self.expr()?) };
        let step = if self.eat_op(":") && !self.at_op("]") { Some(self.expr()?) } else { None };
        Ok(Index::Slice(start, stop, step))
    }

    fn comp_clauses(&mut self) -> Result<Vec<CompFor>> {
        let mut clauses = Vec::new();
        while self.eat_kw("for") {
            let target = self.target_list()?;
            if !self.eat_kw("in") {
                return Err(Error::at(self.pos(), "syntax error: expected 'in'"));
            }
            let iter = self.or_expr()?;
            let mut conds = Vec::new();
            while self.eat_kw("if") {
                conds.push(self.or_expr()?);
            }
            clauses.push(CompFor { target, iter, conds });
        }
        Ok(clauses)
    }

    fn atom(&mut self) -> Result<Expr> {
        let pos = self.pos();
        let t = self.next();
        let kind = match t.tok {
            Tok::Int(n) => ExprKind::Int(n),
            Tok::Float(f) => ExprKind::Float(f),
            Tok::Str(mut s) => {
                // Adjacent literals concatenate: "a" "b".
                while let Tok::Str(more) = self.peek().clone() {
                    self.next();
                    s.push_str(&more);
                }
                ExprKind::Str(s)
            }
            Tok::FStr(parts) => {
                let mut pieces = Vec::new();
                for part in parts {
                    pieces.push(match part {
                        FPart::Lit(s) => FPiece::Lit(s),
                        FPart::Expr(src, p) => {
                            let (src, conv, spec) = split_format_spec(&src);
                            FPiece::Expr(parse_expr_src(src, p)?, conv, spec.to_string())
                        }
                    });
                }
                ExprKind::FStr(pieces)
            }
            Tok::Kw("True") => ExprKind::Bool(true),
            Tok::Kw("False") => ExprKind::Bool(false),
            Tok::Kw("None") => ExprKind::None,
            Tok::Ident(name) => ExprKind::Name(name),
            Tok::Op("(") => {
                if self.eat_op(")") {
                    ExprKind::Tuple(vec![])
                } else {
                    let first = self.expr()?;
                    if self.at_kw("for") {
                        let clauses = self.comp_clauses()?;
                        self.expect_op(")")?;
                        return Ok(Expr { kind: ExprKind::ListComp(Box::new(first), clauses), pos });
                    }
                    if self.eat_op(")") {
                        return Ok(first);
                    }
                    let mut items = vec![first];
                    while self.eat_op(",") {
                        if self.at_op(")") {
                            break;
                        }
                        items.push(self.expr()?);
                    }
                    self.expect_op(")")?;
                    ExprKind::Tuple(items)
                }
            }
            Tok::Op("[") => {
                let mut items = Vec::new();
                if !self.at_op("]") {
                    let first = self.expr()?;
                    if self.at_kw("for") {
                        let clauses = self.comp_clauses()?;
                        self.expect_op("]")?;
                        return Ok(Expr { kind: ExprKind::ListComp(Box::new(first), clauses), pos });
                    }
                    items.push(first);
                    while self.eat_op(",") {
                        if self.at_op("]") {
                            break;
                        }
                        items.push(self.expr()?);
                    }
                }
                self.expect_op("]")?;
                ExprKind::List(items)
            }
            Tok::Op("{") => return Err(Error::at(pos, "dicts and sets are not supported")),
            other => {
                return Err(Error::at(pos, format!("syntax error: unexpected {}", describe(&other))));
            }
        };
        Ok(Expr { kind, pos })
    }
}

/// Split `expr!r:spec` into its parts, ignoring ':' and '!' nested in brackets or strings.
fn split_format_spec(src: &str) -> (&str, bool, &str) {
    let mut depth = 0;
    let mut quote: Option<char> = None;
    for (i, c) in src.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(c),
            (None, '(' | '[' | '{') => depth += 1,
            (None, ')' | ']' | '}') => depth -= 1,
            (None, '!') if depth == 0 && !src[i + 1..].starts_with('=') => {
                let rest = &src[i + 1..];
                let (conv, spec) = rest.split_once(':').unwrap_or((rest, ""));
                return (&src[..i], conv.trim() == "r", spec);
            }
            (None, ':') if depth == 0 => return (&src[..i], false, &src[i + 1..]),
            _ => {}
        }
    }
    (src, false, "")
}

fn check_target(e: &Expr) -> Result<()> {
    match &e.kind {
        ExprKind::Name(_) | ExprKind::Index(..) => Ok(()),
        ExprKind::Tuple(items) | ExprKind::List(items) => items.iter().try_for_each(check_target),
        _ => Err(Error::at(e.pos, "cannot assign to expression")),
    }
}
