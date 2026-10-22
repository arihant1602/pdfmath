use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone)]
pub struct Error {
    pub msg: String,
    pub pos: Option<Pos>,
}

impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Error { msg: msg.into(), pos: None }
    }

    pub fn at(pos: Pos, msg: impl Into<String>) -> Self {
        Error { msg: msg.into(), pos: Some(pos) }
    }

    /// Attach a position if the error does not already carry one.
    pub fn or_at(mut self, pos: Pos) -> Self {
        self.pos.get_or_insert(pos);
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.pos {
            Some(p) => write!(f, "line {}, col {}: {}", p.line, p.col, self.msg),
            None => write!(f, "{}", self.msg),
        }
    }
}

impl From<lopdf::Error> for Error {
    fn from(e: lopdf::Error) -> Self {
        Error::new(format!("pdf error: {e}"))
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::new(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
