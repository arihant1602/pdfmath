use crate::error::Pos;

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub pos: Pos,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    Str(String),
    FStr(Vec<FPiece>),
    Bool(bool),
    None,
    Name(String),
    List(Vec<Expr>),
    Tuple(Vec<Expr>),
    Unary(&'static str, Box<Expr>),
    Binary(&'static str, Box<Expr>, Box<Expr>),
    /// Chained comparison: `a < b <= c`.
    Compare(Box<Expr>, Vec<(&'static str, Expr)>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    IfElse(Box<Expr>, Box<Expr>, Box<Expr>),
    Call(Box<Expr>, Vec<Arg>),
    Attr(Box<Expr>, String),
    Index(Box<Expr>, Box<Index>),
    Lambda(std::rc::Rc<FuncDef>),
    /// `[elt for target in iter if cond ...]`, possibly with several `for` clauses.
    ListComp(Box<Expr>, Vec<CompFor>),
}

#[derive(Debug, Clone)]
pub struct CompFor {
    pub target: Expr,
    pub iter: Expr,
    pub conds: Vec<Expr>,
}

#[derive(Debug, Clone)]
pub enum FPiece {
    Lit(String),
    /// `{expr!r:spec}`: the expression, whether `!r` was given, and the format spec.
    Expr(Expr, bool, String),
}

#[derive(Debug, Clone)]
pub enum Index {
    One(Expr),
    Slice(Option<Expr>, Option<Expr>, Option<Expr>),
}

#[derive(Debug, Clone)]
pub struct Arg {
    pub name: Option<String>,
    pub value: Expr,
}

#[derive(Debug)]
pub struct FuncDef {
    pub name: String,
    pub params: Vec<(String, Option<Expr>)>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub struct Stmt {
    pub kind: StmtKind,
    pub pos: Pos,
}

#[derive(Debug, Clone)]
pub enum StmtKind {
    Expr(Expr),
    /// `a = b = value`: every target receives the value.
    Assign(Vec<Expr>, Expr),
    AugAssign(Expr, &'static str, Expr),
    If(Vec<(Expr, Vec<Stmt>)>, Option<Vec<Stmt>>),
    While(Expr, Vec<Stmt>),
    For(Expr, Expr, Vec<Stmt>),
    Def(std::rc::Rc<FuncDef>),
    Return(Option<Expr>),
    Break,
    Continue,
    Pass,
    Del(Vec<Expr>),
    Global(Vec<String>),
}
