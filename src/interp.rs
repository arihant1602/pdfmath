// Tree-walking evaluator.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::ast::*;
use crate::builtins;
use crate::error::{Error, Result};
use crate::pdf::{PageRef, Pdf};
use crate::slice;
use crate::value::{Closure, Value, compare, equals};

const MAX_DEPTH: usize = 1000;

enum Flow {
    Normal,
    Break,
    Continue,
    Return(Value),
}

struct Frame {
    locals: HashMap<String, Value>,
    globals: HashSet<String>,
}

pub struct Interp {
    pub globals: HashMap<String, Value>,
    frames: Vec<Frame>,
    /// When set, program output is collected here instead of going to stdout
    /// (the MCP server uses stdout for the protocol).
    pub capture: Option<String>,
    /// Side-effect-free mode: builtins that write files or run other code are disabled.
    pub restricted: bool,
}

impl Default for Interp {
    fn default() -> Self {
        Self::new()
    }
}

impl Interp {
    pub fn new() -> Self {
        Interp { globals: HashMap::new(), frames: Vec::new(), capture: None, restricted: false }
    }

    /// Write program output (print, info, help).
    pub fn write_out(&mut self, s: &str) {
        match &mut self.capture {
            Some(buf) => buf.push_str(s),
            None => {
                use std::io::Write;
                let mut out = std::io::stdout().lock();
                let _ = out.write_all(s.as_bytes());
                let _ = out.flush();
            }
        }
    }

    /// Parse and run source text. Returns the value of the last statement when
    /// it is a bare expression (used by the REPL to echo results).
    pub fn run_source(&mut self, src: &str) -> Result<Option<Value>> {
        let tokens = crate::lexer::tokenize(src).map_err(|e| match e {
            crate::lexer::LexError::Bad(e) => e,
            crate::lexer::LexError::Incomplete => Error::new("syntax error: unexpected end of input (unclosed bracket or string?)"),
        })?;
        let program = crate::parser::parse(tokens)?;
        let mut last = None;
        for (i, stmt) in program.iter().enumerate() {
            if i + 1 == program.len()
                && let StmtKind::Expr(e) = &stmt.kind
            {
                last = Some(self.eval(e)?);
                break;
            }
            match self.exec(stmt)? {
                Flow::Normal => {}
                Flow::Break | Flow::Continue => return Err(Error::at(stmt.pos, "SyntaxError: 'break' or 'continue' outside loop")),
                Flow::Return(_) => return Err(Error::at(stmt.pos, "SyntaxError: 'return' outside function")),
            }
        }
        Ok(last)
    }

    fn exec_block(&mut self, body: &[Stmt]) -> Result<Flow> {
        for stmt in body {
            match self.exec(stmt)? {
                Flow::Normal => {}
                other => return Ok(other),
            }
        }
        Ok(Flow::Normal)
    }

    fn exec(&mut self, stmt: &Stmt) -> Result<Flow> {
        self.exec_inner(stmt).map_err(|e| e.or_at(stmt.pos))
    }

    fn exec_inner(&mut self, stmt: &Stmt) -> Result<Flow> {
        match &stmt.kind {
            StmtKind::Expr(e) => {
                self.eval(e)?;
            }
            StmtKind::Assign(targets, value) => {
                let v = self.eval(value)?;
                for t in targets {
                    self.assign(t, v.clone())?;
                }
            }
            StmtKind::AugAssign(target, op, value) => {
                let cur = self.eval(target)?;
                let rhs = self.eval(value)?;
                if let (Value::List(l), "+") = (&cur, *op) {
                    // `xs += ys` extends in place, like Python.
                    let items = self.iterate(&rhs)?;
                    l.borrow_mut().extend(items);
                } else {
                    let new = self.binary(op, cur, rhs)?;
                    self.assign(target, new)?;
                }
            }
            StmtKind::If(arms, orelse) => {
                for (cond, body) in arms {
                    if self.eval(cond)?.truthy() {
                        return self.exec_block(body);
                    }
                }
                if let Some(body) = orelse {
                    return self.exec_block(body);
                }
            }
            StmtKind::While(cond, body) => {
                while self.eval(cond)?.truthy() {
                    match self.exec_block(body)? {
                        Flow::Break => break,
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        _ => {}
                    }
                }
            }
            StmtKind::For(target, iter, body) => {
                let it = self.eval(iter)?;
                for item in self.iterate(&it)? {
                    self.assign(target, item)?;
                    match self.exec_block(body)? {
                        Flow::Break => break,
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        _ => {}
                    }
                }
            }
            StmtKind::Def(def) => {
                let f = self.make_closure(def)?;
                self.set_var(&def.name, f);
            }
            StmtKind::Return(e) => {
                if self.frames.is_empty() {
                    return Err(Error::new("SyntaxError: 'return' outside function"));
                }
                let v = match e {
                    Some(e) => self.eval(e)?,
                    None => Value::None,
                };
                return Ok(Flow::Return(v));
            }
            StmtKind::Break => return Ok(Flow::Break),
            StmtKind::Continue => return Ok(Flow::Continue),
            StmtKind::Pass => {}
            StmtKind::Del(targets) => {
                for t in targets {
                    self.delete(t)?;
                }
            }
            StmtKind::Global(names) => {
                if let Some(f) = self.frames.last_mut() {
                    f.globals.extend(names.iter().cloned());
                }
            }
        }
        Ok(Flow::Normal)
    }

    fn make_closure(&mut self, def: &Rc<FuncDef>) -> Result<Value> {
        let mut defaults = Vec::new();
        for (_, d) in &def.params {
            defaults.push(match d {
                Some(e) => Some(self.eval(e)?),
                None => None,
            });
        }
        Ok(Value::Func(Rc::new(Closure { def: def.clone(), defaults })))
    }

    // ---- variables ----

    pub fn set_var(&mut self, name: &str, v: Value) {
        match self.frames.last_mut() {
            Some(f) if !f.globals.contains(name) => {
                f.locals.insert(name.to_string(), v);
            }
            _ => {
                self.globals.insert(name.to_string(), v);
            }
        }
    }

    fn get_var(&self, name: &str) -> Result<Value> {
        if let Some(f) = self.frames.last()
            && !f.globals.contains(name)
            && let Some(v) = f.locals.get(name)
        {
            return Ok(v.clone());
        }
        if let Some(v) = self.globals.get(name) {
            return Ok(v.clone());
        }
        if let Some(b) = builtins::lookup(name) {
            return Ok(Value::Builtin(b));
        }
        Err(Error::new(format!("NameError: name '{name}' is not defined")))
    }

    fn assign(&mut self, target: &Expr, v: Value) -> Result<()> {
        match &target.kind {
            ExprKind::Name(n) => {
                self.set_var(n, v);
                Ok(())
            }
            ExprKind::Tuple(items) | ExprKind::List(items) => {
                let vals = self.iterate(&v)?;
                if vals.len() != items.len() {
                    return Err(Error::at(
                        target.pos,
                        format!("ValueError: expected {} values to unpack, got {}", items.len(), vals.len()),
                    ));
                }
                for (t, v) in items.iter().zip(vals) {
                    self.assign(t, v)?;
                }
                Ok(())
            }
            ExprKind::Index(base, idx) => {
                let container = self.eval(base)?;
                match container {
                    Value::List(l) => {
                        match idx.as_ref() {
                            Index::One(i) => {
                                let i = self.eval(i)?.expect_int("list index")?;
                                let mut l = l.borrow_mut();
                                let i = slice::index(l.len(), i, "list assignment")?;
                                l[i] = v;
                            }
                            Index::Slice(a, b, c) => {
                                let (a, b, c) = self.slice_parts(a, b, c)?;
                                let items = self.iterate(&v)?;
                                slice::assign(&mut l.borrow_mut(), a, b, c, items)?;
                            }
                        }
                        Ok(())
                    }
                    Value::Pdf(p) => {
                        // Pdfs are immutable values: build the new page list, then rebind.
                        let new_pages = v.expect_pdf("assigned value")?.pages.clone();
                        let mut pages = p.pages.clone();
                        match idx.as_ref() {
                            Index::One(i) => {
                                let i = self.eval(i)?.expect_int("pdf index")?;
                                let i = slice::index(pages.len(), i, "pdf assignment")?;
                                pages.splice(i..=i, new_pages);
                            }
                            Index::Slice(a, b, c) => {
                                let (a, b, c) = self.slice_parts(a, b, c)?;
                                slice::assign(&mut pages, a, b, c, new_pages)?;
                            }
                        }
                        self.assign(base, Value::pdf(Pdf::from_pages(pages)))
                    }
                    other => Err(Error::at(
                        target.pos,
                        format!("TypeError: '{}' object does not support item assignment", other.type_name()),
                    )),
                }
            }
            _ => Err(Error::at(target.pos, "cannot assign to expression")),
        }
    }

    fn delete(&mut self, target: &Expr) -> Result<()> {
        match &target.kind {
            ExprKind::Name(n) => {
                let removed = match self.frames.last_mut() {
                    Some(f) if !f.globals.contains(n) => f.locals.remove(n).is_some(),
                    _ => self.globals.remove(n).is_some(),
                };
                if !removed {
                    return Err(Error::at(target.pos, format!("NameError: name '{n}' is not defined")));
                }
                Ok(())
            }
            ExprKind::Tuple(items) | ExprKind::List(items) => items.iter().try_for_each(|t| self.delete(t)),
            ExprKind::Index(base, idx) => {
                let container = self.eval(base)?;
                let (a, b, c) = match idx.as_ref() {
                    Index::One(i) => {
                        let i = self.eval(i)?.expect_int("index")?;
                        let len = match &container {
                            Value::List(l) => l.borrow().len(),
                            Value::Pdf(p) => p.len(),
                            _ => 0,
                        };
                        let i = slice::index(len, i, "deletion")? as i64;
                        (Some(i), Some(i + 1), None)
                    }
                    Index::Slice(a, b, c) => self.slice_parts(a, b, c)?,
                };
                match container {
                    Value::List(l) => slice::delete(&mut l.borrow_mut(), a, b, c),
                    Value::Pdf(p) => {
                        let mut pages = p.pages.clone();
                        slice::delete(&mut pages, a, b, c)?;
                        self.assign(base, Value::pdf(Pdf::from_pages(pages)))
                    }
                    other => Err(Error::new(format!(
                        "TypeError: '{}' object doesn't support item deletion",
                        other.type_name()
                    ))),
                }
            }
            _ => Err(Error::at(target.pos, "cannot delete expression")),
        }
    }

    // ---- expressions ----

    pub fn eval(&mut self, e: &Expr) -> Result<Value> {
        self.eval_inner(e).map_err(|err| err.or_at(e.pos))
    }

    fn eval_inner(&mut self, e: &Expr) -> Result<Value> {
        Ok(match &e.kind {
            ExprKind::Int(n) => Value::Int(*n),
            ExprKind::Float(f) => Value::Float(*f),
            ExprKind::Str(s) => Value::str(s),
            ExprKind::Bool(b) => Value::Bool(*b),
            ExprKind::None => Value::None,
            ExprKind::FStr(pieces) => {
                let mut s = String::new();
                for p in pieces {
                    match p {
                        FPiece::Lit(l) => s.push_str(l),
                        FPiece::Expr(e, conv, spec) => {
                            let v = self.eval(e)?;
                            let v = if *conv { Value::str(v.repr()) } else { v };
                            s.push_str(&builtins::format_value(&v, spec)?);
                        }
                    }
                }
                Value::str(s)
            }
            ExprKind::Name(n) => self.get_var(n)?,
            ExprKind::List(items) => {
                let mut v = Vec::with_capacity(items.len());
                for i in items {
                    v.push(self.eval(i)?);
                }
                Value::list(v)
            }
            ExprKind::Tuple(items) => {
                let mut v = Vec::with_capacity(items.len());
                for i in items {
                    v.push(self.eval(i)?);
                }
                Value::Tuple(Rc::new(v))
            }
            ExprKind::Unary(op, inner) => {
                let v = self.eval(inner)?;
                match (*op, &v) {
                    ("not", _) => Value::Bool(!v.truthy()),
                    ("-", Value::Float(f)) => Value::Float(-f),
                    ("-", _) if v.as_int().is_some() => Value::Int(
                        v.as_int().unwrap().checked_neg().ok_or_else(|| Error::new("OverflowError: integer overflow"))?,
                    ),
                    ("+", Value::Float(_)) => v,
                    ("+", _) if v.as_int().is_some() => Value::Int(v.as_int().unwrap()),
                    _ => return Err(Error::new(format!("TypeError: bad operand type for unary {op}: '{}'", v.type_name()))),
                }
            }
            ExprKind::Binary(op, l, r) => {
                let a = self.eval(l)?;
                let b = self.eval(r)?;
                self.binary(op, a, b)?
            }
            ExprKind::Compare(first, rest) => {
                let mut left = self.eval(first)?;
                for (op, re) in rest {
                    let right = self.eval(re)?;
                    if !self.compare_op(op, &left, &right)? {
                        return Ok(Value::Bool(false));
                    }
                    left = right;
                }
                Value::Bool(true)
            }
            ExprKind::And(l, r) => {
                let a = self.eval(l)?;
                if !a.truthy() { a } else { self.eval(r)? }
            }
            ExprKind::Or(l, r) => {
                let a = self.eval(l)?;
                if a.truthy() { a } else { self.eval(r)? }
            }
            ExprKind::IfElse(cond, a, b) => {
                if self.eval(cond)?.truthy() {
                    self.eval(a)?
                } else {
                    self.eval(b)?
                }
            }
            ExprKind::Call(f, args) => {
                let fv = self.eval(f)?;
                let mut pos_args = Vec::new();
                let mut kwargs = Vec::new();
                for a in args {
                    let v = self.eval(&a.value)?;
                    match &a.name {
                        Some(n) => kwargs.push((n.clone(), v)),
                        None => pos_args.push(v),
                    }
                }
                self.call(&fv, pos_args, kwargs)?
            }
            ExprKind::Attr(obj, name) => {
                let v = self.eval(obj)?;
                if !builtins::has_method(&v, name) {
                    return Err(Error::new(format!("AttributeError: '{}' object has no attribute '{name}'", v.type_name())));
                }
                Value::Method(Rc::new((v, name.clone())))
            }
            ExprKind::Index(obj, idx) => {
                let v = self.eval(obj)?;
                self.index(&v, idx)?
            }
            ExprKind::Lambda(def) => self.make_closure(def)?,
            ExprKind::ListComp(elt, clauses) => {
                let mut out = Vec::new();
                self.comprehension(elt, clauses, &mut out)?;
                Value::list(out)
            }
        })
    }

    fn comprehension(&mut self, elt: &Expr, clauses: &[CompFor], out: &mut Vec<Value>) -> Result<()> {
        let Some((c, rest)) = clauses.split_first() else {
            out.push(self.eval(elt)?);
            return Ok(());
        };
        let it = self.eval(&c.iter)?;
        'items: for item in self.iterate(&it)? {
            self.assign(&c.target, item)?;
            for cond in &c.conds {
                if !self.eval(cond)?.truthy() {
                    continue 'items;
                }
            }
            self.comprehension(elt, rest, out)?;
        }
        Ok(())
    }

    fn slice_parts(&mut self, a: &Option<Expr>, b: &Option<Expr>, c: &Option<Expr>) -> Result<(Option<i64>, Option<i64>, Option<i64>)> {
        let mut part = |e: &Option<Expr>| -> Result<Option<i64>> {
            match e {
                None => Ok(None),
                Some(e) => match self.eval(e)? {
                    Value::None => Ok(None),
                    v => Ok(Some(v.expect_int("slice indices")?)),
                },
            }
        };
        Ok((part(a)?, part(b)?, part(c)?))
    }

    fn index(&mut self, v: &Value, idx: &Index) -> Result<Value> {
        match idx {
            Index::One(i) => {
                let i = self.eval(i)?;
                let i = i.as_int().ok_or_else(|| {
                    Error::new(format!("TypeError: {} indices must be integers, not {}", v.type_name(), i.type_name()))
                })?;
                match v {
                    Value::List(l) => {
                        let l = l.borrow();
                        Ok(l[slice::index(l.len(), i, "list")?].clone())
                    }
                    Value::Tuple(t) => Ok(t[slice::index(t.len(), i, "tuple")?].clone()),
                    Value::Str(s) => {
                        let chars: Vec<char> = s.chars().collect();
                        Ok(Value::str(chars[slice::index(chars.len(), i, "string")?].to_string()))
                    }
                    Value::Pdf(p) => {
                        let k = slice::index(p.len(), i, "page")?;
                        Ok(Value::pdf(Pdf::from_pages(vec![p.pages[k].clone()])))
                    }
                    _ => Err(Error::new(format!("TypeError: '{}' object is not subscriptable", v.type_name()))),
                }
            }
            Index::Slice(a, b, c) => {
                let (a, b, c) = self.slice_parts(a, b, c)?;
                match v {
                    Value::List(l) => {
                        let l = l.borrow();
                        Ok(Value::list(slice::indices(l.len(), a, b, c)?.into_iter().map(|i| l[i].clone()).collect()))
                    }
                    Value::Tuple(t) => Ok(Value::Tuple(Rc::new(
                        slice::indices(t.len(), a, b, c)?.into_iter().map(|i| t[i].clone()).collect(),
                    ))),
                    Value::Str(s) => {
                        let chars: Vec<char> = s.chars().collect();
                        Ok(Value::str(slice::indices(chars.len(), a, b, c)?.into_iter().map(|i| chars[i]).collect::<String>()))
                    }
                    Value::Pdf(p) => Ok(Value::pdf(Pdf::from_pages(
                        slice::indices(p.len(), a, b, c)?.into_iter().map(|i| p.pages[i].clone()).collect(),
                    ))),
                    _ => Err(Error::new(format!("TypeError: '{}' object is not subscriptable", v.type_name()))),
                }
            }
        }
    }

    /// Values produced by `for x in v`.
    pub fn iterate(&self, v: &Value) -> Result<Vec<Value>> {
        Ok(match v {
            Value::List(l) => l.borrow().clone(),
            Value::Tuple(t) => t.as_ref().clone(),
            Value::Str(s) => s.chars().map(|c| Value::str(c.to_string())).collect(),
            Value::Pdf(p) => p.pages.iter().map(|pg| Value::pdf(Pdf::from_pages(vec![pg.clone()]))).collect(),
            _ => return Err(Error::new(format!("TypeError: '{}' object is not iterable", v.type_name()))),
        })
    }

    fn compare_op(&self, op: &str, a: &Value, b: &Value) -> Result<bool> {
        use std::cmp::Ordering::*;
        Ok(match op {
            "==" => equals(a, b),
            "!=" => !equals(a, b),
            "<" => compare(a, b)? == Less,
            ">" => compare(a, b)? == Greater,
            "<=" => compare(a, b)? != Greater,
            ">=" => compare(a, b)? != Less,
            "in" | "not in" => {
                let found = match b {
                    Value::Str(s) => s.contains(a.expect_str("left operand of 'in'")?.as_ref()),
                    Value::Pdf(p) => {
                        let needle = a.expect_pdf("left operand of 'in'")?;
                        needle.pages.iter().all(|pg| p.pages.contains(pg))
                    }
                    _ => self.iterate(b)?.iter().any(|x| equals(a, x)),
                };
                found == (op == "in")
            }
            "is" | "is not" => {
                let same = match (a, b) {
                    (Value::None, Value::None) => true,
                    (Value::Bool(x), Value::Bool(y)) => x == y,
                    (Value::List(x), Value::List(y)) => Rc::ptr_eq(x, y),
                    (Value::Pdf(x), Value::Pdf(y)) => Rc::ptr_eq(x, y),
                    (Value::Func(x), Value::Func(y)) => Rc::ptr_eq(x, y),
                    _ => false,
                };
                same == (op == "is")
            }
            _ => unreachable!(),
        })
    }

    pub fn binary(&mut self, op: &str, a: Value, b: Value) -> Result<Value> {
        // The pdf operators first; these are what this language is for.
        match (op, &a, &b) {
            ("-", Value::Pdf(x), Value::Pdf(y)) => return Ok(concat(x, y)),
            ("+", Value::Pdf(x), Value::Pdf(y)) => return Ok(concat(y, x)),
            ("-", Value::Pdf(x), n) if n.as_int().is_some() => {
                let n = non_negative(n.as_int().unwrap(), "pdf - n")?;
                let keep = x.len().saturating_sub(n);
                return Ok(Value::pdf(Pdf::from_pages(x.pages[..keep].to_vec())));
            }
            ("+", Value::Pdf(x), n) if n.as_int().is_some() => {
                let n = non_negative(n.as_int().unwrap(), "pdf + n")?.min(x.len());
                return Ok(Value::pdf(Pdf::from_pages(x.pages[n..].to_vec())));
            }
            ("*", Value::Pdf(x), n) | ("*", n, Value::Pdf(x)) if n.as_int().is_some() => {
                let n = n.as_int().unwrap().max(0) as usize;
                let pages: Vec<PageRef> = (0..n).flat_map(|_| x.pages.iter().cloned()).collect();
                return Ok(Value::pdf(Pdf::from_pages(pages)));
            }
            ("--", Value::Pdf(x), n) => {
                let target = n.as_int().ok_or_else(|| {
                    Error::new(format!("TypeError: pdf -- size needs a size like 2mb or 500kb, not {}", n.type_name()))
                })?;
                let target = non_negative(target, "pdf -- size")?;
                return Ok(Value::pdf(crate::compress::compress_to(x, target as u64)?));
            }
            ("--", _, _) => {
                // Not a pdf: Python reads `a--b` as `a - (-b)`.
                let neg = match &b {
                    Value::Float(f) => Value::Float(-f),
                    v if v.as_int().is_some() => Value::Int(-v.as_int().unwrap()),
                    _ => return Err(type_err("--", &a, &b)),
                };
                return self.binary("-", a, neg);
            }
            (_, Value::Pdf(_), _) | (_, _, Value::Pdf(_)) => return Err(type_err(op, &a, &b)),
            _ => {}
        }

        match (op, &a, &b) {
            ("+", Value::Str(x), Value::Str(y)) => return Ok(Value::str(format!("{x}{y}"))),
            ("+", Value::List(x), Value::List(y)) => {
                let mut v = x.borrow().clone();
                v.extend(y.borrow().iter().cloned());
                return Ok(Value::list(v));
            }
            ("+", Value::Tuple(x), Value::Tuple(y)) => {
                let mut v = x.as_ref().clone();
                v.extend(y.iter().cloned());
                return Ok(Value::Tuple(Rc::new(v)));
            }
            ("*", Value::Str(s), n) | ("*", n, Value::Str(s)) if n.as_int().is_some() => {
                return Ok(Value::str(s.repeat(n.as_int().unwrap().max(0) as usize)));
            }
            ("*", Value::List(l), n) | ("*", n, Value::List(l)) if n.as_int().is_some() => {
                let l = l.borrow();
                let n = n.as_int().unwrap().max(0) as usize;
                return Ok(Value::list((0..n).flat_map(|_| l.iter().cloned()).collect()));
            }
            ("%", Value::Str(fmt), args) => return builtins::percent_format(fmt, args, self),
            _ => {}
        }

        if let (Some(x), Some(y)) = (a.as_int(), b.as_int()) {
            let overflow = || Error::new("OverflowError: integer overflow");
            return Ok(match op {
                "+" => Value::Int(x.checked_add(y).ok_or_else(overflow)?),
                "-" => Value::Int(x.checked_sub(y).ok_or_else(overflow)?),
                "*" => Value::Int(x.checked_mul(y).ok_or_else(overflow)?),
                "/" => {
                    if y == 0 {
                        return Err(Error::new("ZeroDivisionError: division by zero"));
                    }
                    Value::Float(x as f64 / y as f64)
                }
                "//" => {
                    if y == 0 {
                        return Err(Error::new("ZeroDivisionError: integer division or modulo by zero"));
                    }
                    let q = x / y;
                    Value::Int(if x % y != 0 && ((x < 0) != (y < 0)) { q - 1 } else { q })
                }
                "%" => {
                    if y == 0 {
                        return Err(Error::new("ZeroDivisionError: integer division or modulo by zero"));
                    }
                    let r = x % y;
                    Value::Int(if r != 0 && ((r < 0) != (y < 0)) { r + y } else { r })
                }
                "**" => {
                    if y < 0 {
                        Value::Float((x as f64).powf(y as f64))
                    } else {
                        Value::Int(x.checked_pow(y.try_into().map_err(|_| overflow())?).ok_or_else(overflow)?)
                    }
                }
                _ => return Err(type_err(op, &a, &b)),
            });
        }
        if let (Some(x), Some(y)) = (a.as_float(), b.as_float()) {
            return Ok(Value::Float(match op {
                "+" => x + y,
                "-" => x - y,
                "*" => x * y,
                "/" => {
                    if y == 0.0 {
                        return Err(Error::new("ZeroDivisionError: float division by zero"));
                    }
                    x / y
                }
                "//" => (x / y).floor(),
                "%" => x - y * (x / y).floor(),
                "**" => x.powf(y),
                _ => return Err(type_err(op, &a, &b)),
            }));
        }
        Err(type_err(op, &a, &b))
    }

    pub fn call(&mut self, f: &Value, args: Vec<Value>, kwargs: Vec<(String, Value)>) -> Result<Value> {
        match f {
            Value::Builtin(name) => builtins::call(self, name, args, kwargs),
            Value::Method(m) => builtins::call_method(self, &m.0, &m.1, args, kwargs),
            Value::Func(c) => self.call_closure(c, args, kwargs),
            other => Err(Error::new(format!("TypeError: '{}' object is not callable", other.type_name()))),
        }
    }

    fn call_closure(&mut self, c: &Rc<Closure>, args: Vec<Value>, kwargs: Vec<(String, Value)>) -> Result<Value> {
        let def = &c.def;
        if self.frames.len() >= MAX_DEPTH {
            return Err(Error::new("RecursionError: maximum recursion depth exceeded"));
        }
        if args.len() > def.params.len() {
            return Err(Error::new(format!(
                "TypeError: {}() takes {} positional arguments but {} were given",
                def.name,
                def.params.len(),
                args.len()
            )));
        }
        let mut slots: Vec<Option<Value>> = args.into_iter().map(Some).collect();
        slots.resize(def.params.len(), None);
        for (k, v) in kwargs {
            let Some(i) = def.params.iter().position(|(p, _)| *p == k) else {
                return Err(Error::new(format!("TypeError: {}() got an unexpected keyword argument '{k}'", def.name)));
            };
            if slots[i].is_some() {
                return Err(Error::new(format!("TypeError: {}() got multiple values for argument '{k}'", def.name)));
            }
            slots[i] = Some(v);
        }
        let mut locals = HashMap::new();
        for (i, slot) in slots.into_iter().enumerate() {
            let v = match slot.or_else(|| c.defaults[i].clone()) {
                Some(v) => v,
                None => {
                    return Err(Error::new(format!(
                        "TypeError: {}() missing required argument: '{}'",
                        def.name, def.params[i].0
                    )));
                }
            };
            locals.insert(def.params[i].0.clone(), v);
        }
        self.frames.push(Frame { locals, globals: HashSet::new() });
        let result = self.exec_block(&def.body);
        self.frames.pop();
        match result? {
            Flow::Return(v) => Ok(v),
            Flow::Normal => Ok(Value::None),
            Flow::Break | Flow::Continue => Err(Error::new("SyntaxError: 'break' or 'continue' outside loop")),
        }
    }
}

fn concat(first: &Pdf, second: &Pdf) -> Value {
    let mut pages = first.pages.clone();
    pages.extend(second.pages.iter().cloned());
    Value::pdf(Pdf::from_pages(pages))
}

fn non_negative(n: i64, what: &str) -> Result<usize> {
    if n < 0 {
        return Err(Error::new(format!("ValueError: {what}: n must be >= 0, got {n}")));
    }
    Ok(n as usize)
}

fn type_err(op: &str, a: &Value, b: &Value) -> Error {
    Error::new(format!(
        "TypeError: unsupported operand type(s) for {op}: '{}' and '{}'",
        a.type_name(),
        b.type_name()
    ))
}
