use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

use crate::ast::FuncDef;
use crate::error::{Error, Result};
use crate::pdf::Pdf;

#[derive(Clone)]
pub enum Value {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Rc<str>),
    List(Rc<RefCell<Vec<Value>>>),
    Tuple(Rc<Vec<Value>>),
    Pdf(Rc<Pdf>),
    Func(Rc<Closure>),
    Builtin(&'static str),
    /// A method bound to its receiver, e.g. `xs.append`.
    Method(Rc<(Value, String)>),
}

pub struct Closure {
    pub def: Rc<FuncDef>,
    pub defaults: Vec<Option<Value>>,
}

impl Value {
    pub fn str(s: impl AsRef<str>) -> Value {
        Value::Str(Rc::from(s.as_ref()))
    }

    pub fn list(v: Vec<Value>) -> Value {
        Value::List(Rc::new(RefCell::new(v)))
    }

    pub fn pdf(p: Pdf) -> Value {
        Value::Pdf(Rc::new(p))
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::None => "NoneType",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Str(_) => "str",
            Value::List(_) => "list",
            Value::Tuple(_) => "tuple",
            Value::Pdf(_) => "pdf",
            Value::Func(_) => "function",
            Value::Builtin(_) | Value::Method(_) => "builtin_function_or_method",
        }
    }

    pub fn truthy(&self) -> bool {
        match self {
            Value::None => false,
            Value::Bool(b) => *b,
            Value::Int(n) => *n != 0,
            Value::Float(f) => *f != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::List(l) => !l.borrow().is_empty(),
            Value::Tuple(t) => !t.is_empty(),
            Value::Pdf(p) => !p.pages.is_empty(),
            _ => true,
        }
    }

    /// Integer view, treating bools as ints like Python does.
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(n) => Some(*n),
            Value::Bool(b) => Some(*b as i64),
            _ => None,
        }
    }

    pub fn as_float(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            _ => self.as_int().map(|n| n as f64),
        }
    }

    pub fn expect_int(&self, what: &str) -> Result<i64> {
        self.as_int()
            .ok_or_else(|| Error::new(format!("TypeError: {what} must be an int, not {}", self.type_name())))
    }

    pub fn expect_str(&self, what: &str) -> Result<Rc<str>> {
        match self {
            Value::Str(s) => Ok(s.clone()),
            _ => Err(Error::new(format!("TypeError: {what} must be a str, not {}", self.type_name()))),
        }
    }

    pub fn expect_pdf(&self, what: &str) -> Result<Rc<Pdf>> {
        match self {
            Value::Pdf(p) => Ok(p.clone()),
            _ => Err(Error::new(format!("TypeError: {what} must be a pdf, not {}", self.type_name()))),
        }
    }

    /// `str(x)`
    pub fn to_str(&self) -> String {
        match self {
            Value::Str(s) => s.to_string(),
            _ => self.repr(),
        }
    }

    /// `repr(x)`
    pub fn repr(&self) -> String {
        match self {
            Value::None => "None".into(),
            Value::Bool(true) => "True".into(),
            Value::Bool(false) => "False".into(),
            Value::Int(n) => n.to_string(),
            Value::Float(f) => float_repr(*f),
            Value::Str(s) => str_repr(s),
            Value::List(l) => format!("[{}]", l.borrow().iter().map(|v| v.repr()).collect::<Vec<_>>().join(", ")),
            Value::Tuple(t) if t.len() == 1 => format!("({},)", t[0].repr()),
            Value::Tuple(t) => format!("({})", t.iter().map(|v| v.repr()).collect::<Vec<_>>().join(", ")),
            Value::Pdf(p) => {
                let n = p.pages.len();
                format!("<pdf: {n} page{}>", if n == 1 { "" } else { "s" })
            }
            Value::Func(c) => format!("<function {}>", c.def.name),
            Value::Builtin(name) => format!("<built-in function {name}>"),
            Value::Method(m) => format!("<method {} of {}>", m.1, m.0.type_name()),
        }
    }
}

pub fn float_repr(f: f64) -> String {
    if f.is_nan() {
        "nan".into()
    } else if f.is_infinite() {
        if f > 0.0 { "inf".into() } else { "-inf".into() }
    } else if f == f.trunc() && f.abs() < 1e16 {
        format!("{f:.1}")
    } else {
        format!("{f}")
    }
}

fn str_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::new();
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

pub fn equals(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::None, Value::None) => true,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::List(x), Value::List(y)) => {
            Rc::ptr_eq(x, y) || {
                let (x, y) = (x.borrow(), y.borrow());
                x.len() == y.len() && x.iter().zip(y.iter()).all(|(a, b)| equals(a, b))
            }
        }
        (Value::Tuple(x), Value::Tuple(y)) => x.len() == y.len() && x.iter().zip(y.iter()).all(|(a, b)| equals(a, b)),
        (Value::Pdf(x), Value::Pdf(y)) => x.pages == y.pages,
        (Value::Func(x), Value::Func(y)) => Rc::ptr_eq(x, y),
        (Value::Builtin(x), Value::Builtin(y)) => x == y,
        _ => match (a.as_float(), b.as_float()) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
    }
}

pub fn compare(a: &Value, b: &Value) -> Result<Ordering> {
    match (a, b) {
        (Value::Str(x), Value::Str(y)) => Ok(x.cmp(y)),
        (Value::List(x), Value::List(y)) => seq_cmp(&x.borrow(), &y.borrow()),
        (Value::Tuple(x), Value::Tuple(y)) => seq_cmp(x, y),
        _ => match (a.as_float(), b.as_float()) {
            (Some(x), Some(y)) => Ok(x.partial_cmp(&y).unwrap_or(Ordering::Equal)),
            _ => Err(Error::new(format!(
                "TypeError: '<' not supported between instances of '{}' and '{}'",
                a.type_name(),
                b.type_name()
            ))),
        },
    }
}

fn seq_cmp(x: &[Value], y: &[Value]) -> Result<Ordering> {
    for (a, b) in x.iter().zip(y.iter()) {
        if !equals(a, b) {
            return compare(a, b);
        }
    }
    Ok(x.len().cmp(&y.len()))
}
