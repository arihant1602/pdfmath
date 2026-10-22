// Builtin functions and methods.

use std::cmp::Ordering;
use std::io::Write;
use std::rc::Rc;

use crate::error::{Error, Result};
use crate::interp::Interp;
use crate::pdf::{PageRef, Pdf};
use crate::value::{Value, compare, equals, float_repr};

const BUILTINS: &[&str] = &[
    // pdf
    "open", "save", "rotate", "reverse", "blank", "info", "size", "pages", "compress",
    // files
    "glob", "exists", "run",
    // python
    "print", "len", "range", "str", "int", "float", "bool", "type", "list", "tuple", "repr", "abs",
    "min", "max", "sum", "sorted", "reversed", "enumerate", "zip", "round", "input", "exit", "quit",
    "help", "format", "isinstance", "any", "all", "map", "filter", "divmod",
];

/// Error message prefix used by `exit(n)` when running embedded.
pub const EXIT_PREFIX: &str = "SystemExit: ";

pub fn lookup(name: &str) -> Option<&'static str> {
    BUILTINS.iter().copied().find(|b| *b == name)
}

pub const HELP: &str = r#"pdfmath — PDF arithmetic with a Python-like language

PDF OPERATORS (pages are 0-indexed, like Python)
  a - b          merge: a's pages, then b's
  a + b          merge: b's pages, then a's
  p - n          drop the last n pages
  p + n          drop the first n pages
  p * n          repeat p n times
  p -- 2mb       compress until the file is <= 2mb (units: b, kb, mb, gb; 1kb = 1024 bytes)
  p[i]           one page (negative i counts from the end)
  p[a:b:c]       Python slicing: p[::-1] reversed, p[::2] odd pages, p[1:-1] trim ends
  p[i] = q       replace page i with q's pages (also p[a:b] = q, del p[i], del p[::2])
  len(p)         number of pages
  for page in p  iterate over single-page pdfs

PDF FUNCTIONS
  open(path, password=None)      load a pdf (methods: p.save(path), p.rotate(90), ...)
  save(p, path)                  write a pdf, returns bytes written
  rotate(p, degrees)             rotate every page clockwise (multiple of 90)
  reverse(p)                     pages in reverse order
  blank(n=1, width=595, height=842)   n empty pages (A4 by default, in points)
  info(p)                        print page count, sources and page sizes
  size(p)                        bytes the pdf would take when saved
  pages(p)                       list of single-page pdfs
  compress(p, size)              same as p -- size
  glob(pattern)                  sorted list of matching paths, e.g. glob("scans/*.pdf")
  exists(path)                   True if the path exists
  run(path)                      execute another .pm script in this session

LANGUAGE
  Python syntax: if/elif/else, while, for, def, return, lambda, list comprehensions,
  f-strings, int/float/str/bool/list/tuple, and builtins like print, len, range,
  sorted, enumerate, zip, min, max, sum. Script arguments are in `args`.

EXAMPLE
  a = open("report.pdf")
  b = open("appendix.pdf")
  save((a - b)[::-1], "out.pdf")
"#;

fn arity(name: &str, args: &[Value], min: usize, max: usize) -> Result<()> {
    if args.len() < min || args.len() > max {
        let want = if min == max { format!("{min}") } else { format!("{min} to {max}") };
        return Err(Error::new(format!("TypeError: {name}() takes {want} arguments ({} given)", args.len())));
    }
    Ok(())
}

/// Pull a keyword argument out of `kwargs`, or take it from position `i`.
fn arg(args: &[Value], kwargs: &mut Vec<(String, Value)>, i: usize, name: &str) -> Option<Value> {
    if let Some(k) = kwargs.iter().position(|(n, _)| n == name) {
        return Some(kwargs.remove(k).1);
    }
    args.get(i).cloned()
}

fn no_more_kwargs(fname: &str, kwargs: &[(String, Value)]) -> Result<()> {
    match kwargs.first() {
        Some((k, _)) => Err(Error::new(format!("TypeError: {fname}() got an unexpected keyword argument '{k}'"))),
        None => Ok(()),
    }
}

/// Builtins that write files, run other scripts or interact; unavailable when `Interp::restricted`.
const SIDE_EFFECTS: &[&str] = &["save", "run", "exit", "quit", "input"];

pub fn call(it: &mut Interp, name: &str, args: Vec<Value>, mut kwargs: Vec<(String, Value)>) -> Result<Value> {
    if it.restricted && SIDE_EFFECTS.contains(&name) {
        return Err(Error::new(format!(
            "{name}() is not allowed in an expression here; use the `output` argument to save, or pdf_script for multi-step jobs"
        )));
    }
    let kw = &mut kwargs;
    let result = match name {
        "print" => {
            let sep = arg(&[], kw, 0, "sep").map(|v| v.to_str()).unwrap_or_else(|| " ".into());
            let end = arg(&[], kw, 0, "end").map(|v| v.to_str()).unwrap_or_else(|| "\n".into());
            let line: Vec<String> = args.iter().map(|v| v.to_str()).collect();
            it.write_out(&format!("{}{}", line.join(&sep), end));
            Value::None
        }
        "help" => {
            it.write_out(HELP);
            Value::None
        }
        "exit" | "quit" => {
            let code = args.first().and_then(|v| v.as_int()).unwrap_or(0);
            if it.capture.is_some() {
                // Embedded (MCP): stop the program without killing the server.
                return Err(Error::new(format!("{EXIT_PREFIX}{code}")));
            }
            let _ = std::io::stdout().flush();
            std::process::exit(code as i32);
        }
        "input" => {
            if it.capture.is_some() {
                return Err(Error::new("input() is not available here (no interactive terminal)"));
            }
            if let Some(p) = args.first() {
                print!("{}", p.to_str());
                let _ = std::io::stdout().flush();
            }
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            Value::str(line.trim_end_matches(['\n', '\r']))
        }

        // ---- pdf ----
        "open" => {
            let path = arg(&args, kw, 0, "path").ok_or_else(|| Error::new("TypeError: open() missing path"))?;
            let password = arg(&args, kw, 1, "password");
            let password = match &password {
                Some(Value::None) | None => None,
                Some(v) => Some(v.expect_str("password")?),
            };
            Value::pdf(Pdf::open(&path.expect_str("path")?, password.as_deref())?)
        }
        "save" => {
            arity(name, &args, 1, 2)?;
            let p = args[0].expect_pdf("save() argument 1")?;
            let path = arg(&args, kw, 1, "path").ok_or_else(|| Error::new("TypeError: save() missing path"))?;
            Value::Int(p.save(&path.expect_str("path")?)? as i64)
        }
        "rotate" => {
            arity(name, &args, 1, 2)?;
            let p = args[0].expect_pdf("rotate() argument 1")?;
            let deg = arg(&args, kw, 1, "degrees").unwrap_or(Value::Int(90)).expect_int("degrees")?;
            rotate(&p, deg)?
        }
        "reverse" => {
            arity(name, &args, 1, 1)?;
            let p = args[0].expect_pdf("reverse() argument")?;
            Value::pdf(Pdf::from_pages(p.pages.iter().rev().cloned().collect()))
        }
        "blank" => {
            let n = arg(&args, kw, 0, "n").unwrap_or(Value::Int(1)).expect_int("n")?;
            let w = arg(&args, kw, 1, "width").map(|v| v.as_float()).unwrap_or(Some(595.0));
            let h = arg(&args, kw, 2, "height").map(|v| v.as_float()).unwrap_or(Some(842.0));
            let (Some(w), Some(h)) = (w, h) else { return Err(Error::new("TypeError: blank() width/height must be numbers")) };
            Value::pdf(Pdf::blank(n.max(0) as usize, w, h))
        }
        "info" => {
            arity(name, &args, 1, 1)?;
            let p = args[0].expect_pdf("info() argument")?;
            it.write_out(&info(&p));
            Value::None
        }
        "size" => {
            arity(name, &args, 1, 1)?;
            Value::Int(args[0].expect_pdf("size() argument")?.to_bytes()?.len() as i64)
        }
        "pages" => {
            arity(name, &args, 1, 1)?;
            Value::list(it.iterate(&Value::Pdf(args[0].expect_pdf("pages() argument")?))?)
        }
        "compress" => {
            arity(name, &args, 1, 2)?;
            let size = arg(&args, kw, 1, "size").ok_or_else(|| Error::new("TypeError: compress() missing size"))?;
            it.binary("--", args[0].clone(), size)?
        }

        // ---- files ----
        "glob" => {
            arity(name, &args, 1, 1)?;
            Value::list(glob(&args[0].expect_str("pattern")?)?.into_iter().map(Value::str).collect())
        }
        "exists" => {
            arity(name, &args, 1, 1)?;
            Value::Bool(std::path::Path::new(args[0].expect_str("path")?.as_ref()).exists())
        }
        "run" => {
            arity(name, &args, 1, 1)?;
            let path = args[0].expect_str("path")?;
            let src = std::fs::read_to_string(path.as_ref()).map_err(|e| Error::new(format!("cannot read '{path}': {e}")))?;
            it.run_source(&src).map_err(|e| Error::new(format!("in {path}: {e}")))?;
            Value::None
        }

        // ---- python ----
        "len" => {
            arity(name, &args, 1, 1)?;
            Value::Int(match &args[0] {
                Value::Str(s) => s.chars().count(),
                Value::List(l) => l.borrow().len(),
                Value::Tuple(t) => t.len(),
                Value::Pdf(p) => p.len(),
                v => return Err(Error::new(format!("TypeError: object of type '{}' has no len()", v.type_name()))),
            } as i64)
        }
        "range" => {
            arity(name, &args, 1, 3)?;
            let n: Vec<i64> = args.iter().map(|a| a.expect_int("range() argument")).collect::<Result<_>>()?;
            let (start, stop, step) = match n.len() {
                1 => (0, n[0], 1),
                2 => (n[0], n[1], 1),
                _ => (n[0], n[1], n[2]),
            };
            if step == 0 {
                return Err(Error::new("ValueError: range() arg 3 must not be zero"));
            }
            let count = if step > 0 { (stop - start + step - 1).max(0) / step } else { (start - stop - step - 1).max(0) / -step };
            if count > 50_000_000 {
                return Err(Error::new("MemoryError: range too large"));
            }
            Value::list((0..count).map(|k| Value::Int(start + k * step)).collect())
        }
        "str" => match args.first() {
            None => Value::str(""),
            Some(v) => Value::str(v.to_str()),
        },
        "repr" => {
            arity(name, &args, 1, 1)?;
            Value::str(args[0].repr())
        }
        "format" => {
            arity(name, &args, 1, 2)?;
            let spec = args.get(1).map(|s| s.to_str()).unwrap_or_default();
            Value::str(format_value(&args[0], &spec)?)
        }
        "int" => match args.first() {
            None => Value::Int(0),
            Some(Value::Str(s)) => {
                let base = args.get(1).and_then(|b| b.as_int()).unwrap_or(10) as u32;
                let t = s.trim().replace('_', "");
                Value::Int(i64::from_str_radix(&t, base).map_err(|_| {
                    Error::new(format!("ValueError: invalid literal for int() with base {base}: '{s}'"))
                })?)
            }
            Some(Value::Float(f)) => Value::Int(f.trunc() as i64),
            Some(v) => Value::Int(v.as_int().ok_or_else(|| {
                Error::new(format!("TypeError: int() argument must be a string or a number, not '{}'", v.type_name()))
            })?),
        },
        "float" => match args.first() {
            None => Value::Float(0.0),
            Some(Value::Str(s)) => Value::Float(
                s.trim().parse().map_err(|_| Error::new(format!("ValueError: could not convert string to float: '{s}'")))?,
            ),
            Some(v) => Value::Float(v.as_float().ok_or_else(|| {
                Error::new(format!("TypeError: float() argument must be a string or a number, not '{}'", v.type_name()))
            })?),
        },
        "bool" => Value::Bool(args.first().is_some_and(|v| v.truthy())),
        "type" => {
            arity(name, &args, 1, 1)?;
            Value::str(args[0].type_name())
        }
        "isinstance" => {
            arity(name, &args, 2, 2)?;
            let want: Vec<String> = match &args[1] {
                Value::Tuple(t) => t.iter().map(type_label).collect(),
                v => vec![type_label(v)],
            };
            let t = args[0].type_name();
            Value::Bool(want.iter().any(|w| w == t || (w == "int" && t == "bool")))
        }
        "list" => match args.first() {
            None => Value::list(vec![]),
            Some(v) => Value::list(it.iterate(v)?),
        },
        "tuple" => match args.first() {
            None => Value::Tuple(Rc::new(vec![])),
            Some(v) => Value::Tuple(Rc::new(it.iterate(v)?)),
        },
        "abs" => {
            arity(name, &args, 1, 1)?;
            match &args[0] {
                Value::Float(f) => Value::Float(f.abs()),
                v => Value::Int(v.expect_int("abs() argument")?.abs()),
            }
        }
        "round" => {
            arity(name, &args, 1, 2)?;
            let x = args[0].as_float().ok_or_else(|| Error::new("TypeError: round() needs a number"))?;
            match args.get(1) {
                None => Value::Int(round_half_even(x) as i64),
                Some(d) => {
                    let m = 10f64.powi(d.expect_int("ndigits")? as i32);
                    Value::Float(round_half_even(x * m) / m)
                }
            }
        }
        "divmod" => {
            arity(name, &args, 2, 2)?;
            let q = it.binary("//", args[0].clone(), args[1].clone())?;
            let r = it.binary("%", args[0].clone(), args[1].clone())?;
            Value::Tuple(Rc::new(vec![q, r]))
        }
        "min" | "max" => {
            let key = arg(&[], kw, 0, "key");
            let default = arg(&[], kw, 0, "default");
            let items = if args.len() == 1 { it.iterate(&args[0])? } else { args.clone() };
            if items.is_empty() {
                return default.ok_or_else(|| Error::new(format!("ValueError: {name}() arg is an empty sequence")));
            }
            let want = if name == "min" { Ordering::Less } else { Ordering::Greater };
            let mut best = items[0].clone();
            let mut best_key = keyed(it, &key, &best)?;
            for v in items.into_iter().skip(1) {
                let k = keyed(it, &key, &v)?;
                if compare(&k, &best_key)? == want {
                    best = v;
                    best_key = k;
                }
            }
            best
        }
        "sum" => {
            arity(name, &args, 1, 2)?;
            let mut total = args.get(1).cloned().unwrap_or(Value::Int(0));
            for v in it.iterate(&args[0])? {
                total = it.binary("+", total, v)?;
            }
            total
        }
        "any" => Value::Bool(it.iterate(args.first().unwrap_or(&Value::list(vec![])))?.iter().any(|v| v.truthy())),
        "all" => Value::Bool(it.iterate(args.first().unwrap_or(&Value::list(vec![])))?.iter().all(|v| v.truthy())),
        "sorted" => {
            arity(name, &args, 1, 1)?;
            let mut items = it.iterate(&args[0])?;
            let key = arg(&[], kw, 0, "key");
            let rev = arg(&[], kw, 0, "reverse").is_some_and(|v| v.truthy());
            sort_values(it, &mut items, &key, rev)?;
            Value::list(items)
        }
        "reversed" => {
            arity(name, &args, 1, 1)?;
            match &args[0] {
                Value::Pdf(p) => Value::pdf(Pdf::from_pages(p.pages.iter().rev().cloned().collect())),
                v => Value::list(it.iterate(v)?.into_iter().rev().collect()),
            }
        }
        "enumerate" => {
            arity(name, &args, 1, 2)?;
            let start = arg(&args, kw, 1, "start").unwrap_or(Value::Int(0)).expect_int("start")?;
            Value::list(
                it.iterate(&args[0])?
                    .into_iter()
                    .enumerate()
                    .map(|(i, v)| Value::Tuple(Rc::new(vec![Value::Int(start + i as i64), v])))
                    .collect(),
            )
        }
        "zip" => {
            let seqs: Vec<Vec<Value>> = args.iter().map(|a| it.iterate(a)).collect::<Result<_>>()?;
            let n = seqs.iter().map(|s| s.len()).min().unwrap_or(0);
            Value::list((0..n).map(|i| Value::Tuple(Rc::new(seqs.iter().map(|s| s[i].clone()).collect()))).collect())
        }
        "map" => {
            arity(name, &args, 2, 2)?;
            let mut out = Vec::new();
            for v in it.iterate(&args[1])? {
                out.push(it.call(&args[0], vec![v], vec![])?);
            }
            Value::list(out)
        }
        "filter" => {
            arity(name, &args, 2, 2)?;
            let mut out = Vec::new();
            for v in it.iterate(&args[1])? {
                let keep = match &args[0] {
                    Value::None => v.truthy(),
                    f => it.call(f, vec![v.clone()], vec![])?.truthy(),
                };
                if keep {
                    out.push(v);
                }
            }
            Value::list(out)
        }
        _ => return Err(Error::new(format!("NameError: name '{name}' is not defined"))),
    };
    no_more_kwargs(name, kw)?;
    Ok(result)
}

fn type_label(v: &Value) -> String {
    match v {
        Value::Builtin(b) => b.to_string(),
        Value::Str(s) => s.to_string(),
        other => other.type_name().to_string(),
    }
}

fn keyed(it: &mut Interp, key: &Option<Value>, v: &Value) -> Result<Value> {
    match key {
        Some(Value::None) | None => Ok(v.clone()),
        Some(f) => it.call(f, vec![v.clone()], vec![]),
    }
}

fn sort_values(it: &mut Interp, items: &mut Vec<Value>, key: &Option<Value>, rev: bool) -> Result<()> {
    let mut keyed_items: Vec<(Value, Value)> = Vec::with_capacity(items.len());
    for v in items.drain(..) {
        keyed_items.push((keyed(it, key, &v)?, v));
    }
    let mut err = None;
    keyed_items.sort_by(|a, b| {
        let o = compare(&a.0, &b.0).unwrap_or_else(|e| {
            err.get_or_insert(e);
            Ordering::Equal
        });
        if rev { o.reverse() } else { o }
    });
    if let Some(e) = err {
        return Err(e);
    }
    items.extend(keyed_items.into_iter().map(|(_, v)| v));
    Ok(())
}

fn round_half_even(x: f64) -> f64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 && r % 2.0 != 0.0 { r - x.signum() } else { r }
}

fn rotate(p: &Pdf, deg: i64) -> Result<Value> {
    if deg % 90 != 0 {
        return Err(Error::new(format!("ValueError: rotation must be a multiple of 90, got {deg}")));
    }
    let pages = p.pages.iter().map(|pg| PageRef { rotate: pg.rotate + deg, ..pg.clone() }).collect();
    Ok(Value::pdf(Pdf::from_pages(pages)))
}

pub fn info(p: &Pdf) -> String {
    let mut s = format!("pages: {}\n", p.len());
    // Runs of consecutive pages from the same source.
    let mut runs: Vec<(String, usize, usize)> = Vec::new();
    for pg in &p.pages {
        let idx = pg.src.doc.get_pages().values().position(|id| *id == pg.page).unwrap_or(0);
        match runs.last_mut() {
            Some((name, _, end)) if *name == pg.src.name && *end + 1 == idx => *end = idx,
            _ => runs.push((pg.src.name.clone(), idx, idx)),
        }
    }
    s.push_str("sources:\n");
    for (name, a, b) in &runs {
        if a == b {
            s.push_str(&format!("  {name} page {a}\n"));
        } else {
            s.push_str(&format!("  {name} pages {a}..{b}\n"));
        }
    }
    let mut sizes: Vec<((i64, i64), usize)> = Vec::new();
    for i in 0..p.len() {
        let (w, h) = p.page_size(i);
        let key = (w.round() as i64, h.round() as i64);
        match sizes.iter_mut().find(|(k, _)| *k == key) {
            Some((_, n)) => *n += 1,
            None => sizes.push((key, 1)),
        }
    }
    s.push_str("page sizes (pt):\n");
    for ((w, h), n) in sizes {
        s.push_str(&format!("  {w} x {h}  ({n} page{})\n", if n == 1 { "" } else { "s" }));
    }
    if let Some(b) = &p.bytes {
        s.push_str(&format!("exact size: {} bytes (compressed result)\n", b.len()));
    }
    s
}

/// Wildcards (`*`, `?`) in the last path component only, e.g. `scans/*.pdf`.
fn glob(pattern: &str) -> Result<Vec<String>> {
    let (dir, file_pat) = match pattern.rfind('/') {
        Some(i) => (&pattern[..i + 1], &pattern[i + 1..]),
        None => ("", pattern),
    };
    if dir.contains(['*', '?']) {
        return Err(Error::new("glob(): wildcards are only supported in the file name"));
    }
    let read = std::fs::read_dir(if dir.is_empty() { "." } else { dir });
    let mut out = Vec::new();
    if let Ok(entries) = read {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if (name.starts_with('.') && !file_pat.starts_with('.')) || !wildcard(file_pat.as_bytes(), name.as_bytes()) {
                continue;
            }
            out.push(format!("{dir}{name}"));
        }
    }
    out.sort();
    Ok(out)
}

fn wildcard(p: &[u8], s: &[u8]) -> bool {
    match (p.first(), s.first()) {
        (None, None) => true,
        (Some(b'*'), _) => wildcard(&p[1..], s) || (!s.is_empty() && wildcard(p, &s[1..])),
        (Some(b'?'), Some(_)) => wildcard(&p[1..], &s[1..]),
        (Some(a), Some(b)) if a == b => wildcard(&p[1..], &s[1..]),
        _ => false,
    }
}

// ---- formatting ----

/// A subset of Python's format mini-language: [[fill]align][sign][0][width][,][.precision][type]
pub fn format_value(v: &Value, spec: &str) -> Result<String> {
    if spec.is_empty() {
        return Ok(v.to_str());
    }
    let bad = || Error::new(format!("ValueError: invalid format spec '{spec}'"));
    let c: Vec<char> = spec.chars().collect();
    let mut i = 0;
    let (mut fill, mut align) = (' ', None);
    if c.len() >= 2 && matches!(c[1], '<' | '>' | '^') {
        fill = c[0];
        align = Some(c[1]);
        i = 2;
    } else if matches!(c.first(), Some('<' | '>' | '^')) {
        align = Some(c[0]);
        i = 1;
    }
    let mut plus = false;
    if c.get(i) == Some(&'+') {
        plus = true;
        i += 1;
    }
    if c.get(i) == Some(&'0') {
        fill = '0';
        align.get_or_insert('=');
        i += 1;
    }
    let mut width = 0usize;
    while let Some(d) = c.get(i).and_then(|c| c.to_digit(10)) {
        width = width * 10 + d as usize;
        i += 1;
    }
    let mut comma = false;
    if c.get(i) == Some(&',') {
        comma = true;
        i += 1;
    }
    let mut prec = None;
    if c.get(i) == Some(&'.') {
        i += 1;
        let mut p = 0usize;
        let start = i;
        while let Some(d) = c.get(i).and_then(|c| c.to_digit(10)) {
            p = p * 10 + d as usize;
            i += 1;
        }
        if i == start {
            return Err(bad());
        }
        prec = Some(p);
    }
    let ty = c.get(i).copied();
    if i + ty.map_or(0, |_| 1) != c.len() {
        return Err(bad());
    }
    let mut body = match (ty, v) {
        (Some('d'), _) => v.as_int().ok_or_else(bad)?.to_string(),
        (Some('f' | 'F'), _) => format!("{:.*}", prec.unwrap_or(6), v.as_float().ok_or_else(bad)?),
        (Some('%'), _) => format!("{:.*}%", prec.unwrap_or(6), v.as_float().ok_or_else(bad)? * 100.0),
        (Some('e'), _) => format!("{:.*e}", prec.unwrap_or(6), v.as_float().ok_or_else(bad)?),
        (Some('x'), _) => format!("{:x}", v.as_int().ok_or_else(bad)?),
        (Some('s') | None, Value::Str(s)) => match prec {
            Some(p) => s.chars().take(p).collect(),
            None => s.to_string(),
        },
        (None, Value::Float(f)) => match prec {
            Some(p) => format!("{f:.p$}"),
            None => float_repr(*f),
        },
        (None, _) => v.to_str(),
        _ => return Err(bad()),
    };
    if comma {
        let (sign, rest) = match body.strip_prefix('-') {
            Some(r) => ("-", r),
            None => ("", &body[..]),
        };
        let (int, frac) = rest.split_once('.').map(|(a, b)| (a, Some(b))).unwrap_or((rest, None));
        let mut grouped = String::new();
        for (k, ch) in int.chars().enumerate() {
            if k > 0 && (int.len() - k) % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(ch);
        }
        body = format!("{sign}{grouped}{}", frac.map(|f| format!(".{f}")).unwrap_or_default());
    }
    if plus && !body.starts_with('-') && v.as_float().is_some() {
        body.insert(0, '+');
    }
    let len = body.chars().count();
    if len >= width {
        return Ok(body);
    }
    let pad = width - len;
    let numeric = !matches!(v, Value::Str(_));
    Ok(match align.unwrap_or(if numeric { '>' } else { '<' }) {
        '<' => format!("{body}{}", fill.to_string().repeat(pad)),
        '^' => format!("{}{body}{}", fill.to_string().repeat(pad / 2), fill.to_string().repeat(pad - pad / 2)),
        '=' if body.starts_with(['-', '+']) => format!("{}{}{}", &body[..1], fill.to_string().repeat(pad), &body[1..]),
        _ => format!("{}{body}", fill.to_string().repeat(pad)),
    })
}

/// `"%s and %d" % (a, b)`
pub fn percent_format(fmt: &str, args: &Value, _it: &mut Interp) -> Result<Value> {
    let items: Vec<Value> = match args {
        Value::Tuple(t) => t.as_ref().clone(),
        v => vec![v.clone()],
    };
    let mut out = String::new();
    let mut next = items.iter();
    let chars: Vec<char> = fmt.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '%' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let start = i + 1;
        let mut j = start;
        while j < chars.len() && !chars[j].is_ascii_alphabetic() && chars[j] != '%' {
            j += 1;
        }
        let Some(&ty) = chars.get(j) else { return Err(Error::new("ValueError: incomplete format")) };
        if ty == '%' {
            out.push('%');
        } else {
            let v = next.next().ok_or_else(|| Error::new("TypeError: not enough arguments for format string"))?;
            let flags: String = chars[start..j].iter().collect();
            let spec = match ty {
                's' | 'r' => flags.clone(),
                'i' | 'u' => format!("{flags}d"),
                t => format!("{flags}{t}"),
            };
            let v = if ty == 'r' { Value::str(v.repr()) } else if ty == 's' { Value::str(v.to_str()) } else { v.clone() };
            let spec = spec.replace('-', "<");
            out.push_str(&format_value(&v, &spec)?);
        }
        i = j + 1;
    }
    if next.next().is_some() {
        return Err(Error::new("TypeError: not all arguments converted during string formatting"));
    }
    Ok(Value::str(out))
}

// ---- methods ----

const LIST_METHODS: &[&str] = &["append", "extend", "pop", "insert", "index", "count", "reverse", "sort", "copy", "clear", "remove"];
const STR_METHODS: &[&str] = &[
    "upper", "lower", "strip", "lstrip", "rstrip", "split", "join", "replace", "startswith", "endswith",
    "find", "count", "isdigit", "format", "title", "zfill",
];
const PDF_METHODS: &[&str] = &["save", "rotate", "reverse", "info", "size", "pages", "compress"];

pub fn has_method(v: &Value, name: &str) -> bool {
    let table = match v {
        Value::List(_) => LIST_METHODS,
        Value::Str(_) => STR_METHODS,
        Value::Pdf(_) => PDF_METHODS,
        Value::Tuple(_) => &["index", "count"],
        _ => &[],
    };
    table.contains(&name)
}

pub fn call_method(it: &mut Interp, recv: &Value, name: &str, args: Vec<Value>, kwargs: Vec<(String, Value)>) -> Result<Value> {
    match recv {
        Value::Pdf(_) => {
            let mut full = vec![recv.clone()];
            full.extend(args);
            call(it, name, full, kwargs)
        }
        Value::List(l) => {
            let mut kwargs = kwargs;
            let r = match name {
                "append" => {
                    arity(name, &args, 1, 1)?;
                    l.borrow_mut().push(args[0].clone());
                    Value::None
                }
                "extend" => {
                    arity(name, &args, 1, 1)?;
                    let items = it.iterate(&args[0])?;
                    l.borrow_mut().extend(items);
                    Value::None
                }
                "pop" => {
                    arity(name, &args, 0, 1)?;
                    let mut l = l.borrow_mut();
                    if l.is_empty() {
                        return Err(Error::new("IndexError: pop from empty list"));
                    }
                    let i = match args.first() {
                        Some(i) => crate::slice::index(l.len(), i.expect_int("index")?, "pop")?,
                        None => l.len() - 1,
                    };
                    l.remove(i)
                }
                "insert" => {
                    arity(name, &args, 2, 2)?;
                    let mut l = l.borrow_mut();
                    let n = l.len() as i64;
                    let mut i = args[0].expect_int("index")?;
                    if i < 0 {
                        i = (i + n).max(0);
                    }
                    l.insert(i.min(n) as usize, args[1].clone());
                    Value::None
                }
                "index" => {
                    arity(name, &args, 1, 1)?;
                    let pos = l.borrow().iter().position(|v| equals(v, &args[0]));
                    Value::Int(pos.ok_or_else(|| Error::new(format!("ValueError: {} is not in list", args[0].repr())))? as i64)
                }
                "count" => {
                    arity(name, &args, 1, 1)?;
                    Value::Int(l.borrow().iter().filter(|v| equals(v, &args[0])).count() as i64)
                }
                "remove" => {
                    arity(name, &args, 1, 1)?;
                    let pos = l.borrow().iter().position(|v| equals(v, &args[0]));
                    let pos = pos.ok_or_else(|| Error::new("ValueError: list.remove(x): x not in list"))?;
                    l.borrow_mut().remove(pos);
                    Value::None
                }
                "reverse" => {
                    l.borrow_mut().reverse();
                    Value::None
                }
                "sort" => {
                    let key = arg(&[], &mut kwargs, 0, "key");
                    let rev = arg(&[], &mut kwargs, 0, "reverse").is_some_and(|v| v.truthy());
                    let mut items = std::mem::take(&mut *l.borrow_mut());
                    let r = sort_values(it, &mut items, &key, rev);
                    *l.borrow_mut() = items;
                    r?;
                    Value::None
                }
                "copy" => Value::list(l.borrow().clone()),
                "clear" => {
                    l.borrow_mut().clear();
                    Value::None
                }
                _ => unreachable!(),
            };
            no_more_kwargs(name, &kwargs)?;
            Ok(r)
        }
        Value::Tuple(t) => {
            arity(name, &args, 1, 1)?;
            Ok(match name {
                "index" => Value::Int(
                    t.iter()
                        .position(|v| equals(v, &args[0]))
                        .ok_or_else(|| Error::new("ValueError: tuple.index(x): x not in tuple"))? as i64,
                ),
                _ => Value::Int(t.iter().filter(|v| equals(v, &args[0])).count() as i64),
            })
        }
        Value::Str(s) => str_method(it, s, name, args, kwargs),
        _ => unreachable!(),
    }
}

fn str_method(it: &mut Interp, s: &str, name: &str, args: Vec<Value>, kwargs: Vec<(String, Value)>) -> Result<Value> {
    let chars_arg = |i: usize| -> Result<Option<String>> {
        match args.get(i) {
            None | Some(Value::None) => Ok(None),
            Some(v) => Ok(Some(v.expect_str("argument")?.to_string())),
        }
    };
    if name != "format" {
        no_more_kwargs(name, &kwargs)?;
    }
    Ok(match name {
        "upper" => Value::str(s.to_uppercase()),
        "lower" => Value::str(s.to_lowercase()),
        "title" => Value::str(
            s.split(' ')
                .map(|w| {
                    let mut c = w.chars();
                    c.next().map(|f| f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase()).unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join(" "),
        ),
        "strip" | "lstrip" | "rstrip" => {
            let set = chars_arg(0)?;
            let pred = |c: char| match &set {
                Some(set) => set.contains(c),
                None => c.is_whitespace(),
            };
            Value::str(match name {
                "strip" => s.trim_matches(pred),
                "lstrip" => s.trim_start_matches(pred),
                _ => s.trim_end_matches(pred),
            })
        }
        "split" => {
            let parts: Vec<Value> = match chars_arg(0)? {
                Some(sep) if sep.is_empty() => return Err(Error::new("ValueError: empty separator")),
                Some(sep) => s.split(sep.as_str()).map(Value::str).collect(),
                None => s.split_whitespace().map(Value::str).collect(),
            };
            Value::list(parts)
        }
        "join" => {
            arity(name, &args, 1, 1)?;
            let items = it.iterate(&args[0])?;
            let strs: Vec<String> = items
                .iter()
                .map(|v| match v {
                    Value::Str(x) => Ok(x.to_string()),
                    other => Err(Error::new(format!("TypeError: sequence item: expected str instance, {} found", other.type_name()))),
                })
                .collect::<Result<_>>()?;
            Value::str(strs.join(s))
        }
        "replace" => {
            arity(name, &args, 2, 2)?;
            Value::str(s.replace(args[0].expect_str("old")?.as_ref(), args[1].expect_str("new")?.as_ref()))
        }
        "startswith" | "endswith" => {
            arity(name, &args, 1, 1)?;
            let opts: Vec<Value> = match &args[0] {
                Value::Tuple(t) => t.as_ref().clone(),
                v => vec![v.clone()],
            };
            let mut hit = false;
            for o in opts {
                let o = o.expect_str("prefix")?;
                hit |= if name == "startswith" { s.starts_with(o.as_ref()) } else { s.ends_with(o.as_ref()) };
            }
            Value::Bool(hit)
        }
        "find" => {
            arity(name, &args, 1, 1)?;
            let needle = args[0].expect_str("sub")?;
            Value::Int(s.find(needle.as_ref()).map(|b| s[..b].chars().count() as i64).unwrap_or(-1))
        }
        "count" => {
            arity(name, &args, 1, 1)?;
            Value::Int(s.matches(args[0].expect_str("sub")?.as_ref()).count() as i64)
        }
        "isdigit" => Value::Bool(!s.is_empty() && s.chars().all(|c| c.is_ascii_digit())),
        "zfill" => {
            arity(name, &args, 1, 1)?;
            Value::str(format_value(&Value::str(s), &format!("0>{}", args[0].expect_int("width")?))?)
        }
        "format" => {
            // "{} {name} {0:.2f}".format(...)
            let mut out = String::new();
            let mut auto = 0;
            let mut rest = s;
            while let Some(i) = rest.find(['{', '}']) {
                out.push_str(&rest[..i]);
                let tail = &rest[i..];
                if tail.starts_with("{{") || tail.starts_with("}}") {
                    out.push_str(&tail[..1]);
                    rest = &tail[2..];
                    continue;
                }
                let end = tail.find('}').ok_or_else(|| Error::new("ValueError: unmatched '{' in format string"))?;
                let field = &tail[1..end];
                let (fname, spec) = field.split_once(':').unwrap_or((field, ""));
                let v = if fname.is_empty() {
                    auto += 1;
                    args.get(auto - 1).cloned()
                } else if let Ok(n) = fname.parse::<usize>() {
                    args.get(n).cloned()
                } else {
                    kwargs.iter().find(|(k, _)| k == fname).map(|(_, v)| v.clone())
                };
                let v = v.ok_or_else(|| Error::new(format!("IndexError: format field '{fname}' has no value")))?;
                out.push_str(&format_value(&v, spec)?);
                rest = &tail[end + 1..];
            }
            out.push_str(rest);
            Value::str(out)
        }
        _ => unreachable!(),
    })
}
