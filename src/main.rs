mod ast;
mod builtins;
mod compress;
mod error;
mod interp;
mod lexer;
mod parser;
mod pdf;
mod repl;
mod slice;
mod value;

use std::io::{IsTerminal, Read};

use interp::Interp;
use value::Value;

const USAGE: &str = "\
pdfm — PDF arithmetic and scripting (Python-like .pm language)

USAGE
  pdfm                         interactive >>> prompt (runs stdin as a script if piped)
  pdfm FILE.pm [ARGS...]       run a script; ARGS are in the `args` list
  pdfm -c CODE [ARGS...]       run code given as a string
  pdfm -e EXPR [-o OUT] [FILES...]
                               evaluate one expression; FILES are opened and bound to
                               p1, p2, ... (or NAME=path.pdf binds NAME). With -o the
                               resulting pdf is written to OUT.
  pdfm -i FILE.pm              run a script, then stay in the prompt
  pdfm -                       read a script from stdin
  pdfm --help-lang             language and operator cheat-sheet

EXAMPLES
  pdfm -e 'p1 - p2' -o merged.pdf a.pdf b.pdf      a then b
  pdfm -e 'p1 + p2' -o merged.pdf a.pdf b.pdf      b then a
  pdfm -e 'p1[::-1]' -o reversed.pdf a.pdf
  pdfm -e 'p1 - 2' -o trimmed.pdf a.pdf            drop the last 2 pages
  pdfm -e 'p1 -- 1mb' -o small.pdf scan.pdf        compress to <= 1 MiB
";

fn main() {
    // Deeply nested scripts and PDFs recurse; give the interpreter a large stack.
    let child = std::thread::Builder::new().stack_size(256 * 1024 * 1024).spawn(real_main).expect("spawn interpreter thread");
    let code = child.join().unwrap_or(101);
    std::process::exit(code);
}

fn real_main() -> i32 {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut interp = Interp::new();

    let set_args = |interp: &mut Interp, rest: &[String]| {
        interp.globals.insert("args".into(), Value::list(rest.iter().map(Value::str).collect()));
    };

    let Some(first) = argv.first() else {
        set_args(&mut interp, &[]);
        if std::io::stdin().is_terminal() {
            repl::run(&mut interp);
            return 0;
        }
        return run_stdin(&mut interp);
    };

    match first.as_str() {
        "-h" | "--help" => {
            print!("{USAGE}");
            0
        }
        "--help-lang" => {
            print!("{}", builtins::HELP);
            0
        }
        "-V" | "--version" => {
            println!("pdfm {}", env!("CARGO_PKG_VERSION"));
            0
        }
        "-" => {
            set_args(&mut interp, &argv[1..]);
            run_stdin(&mut interp)
        }
        "-c" => {
            let Some(code) = argv.get(1) else { return usage_error("-c needs code") };
            set_args(&mut interp, &argv[2..]);
            report(interp.run_source(code).map(|_| ()), "<string>")
        }
        "-e" => eval_mode(&mut interp, &argv[1..]),
        "-i" => {
            let Some(path) = argv.get(1) else { return usage_error("-i needs a script") };
            set_args(&mut interp, &argv[2..]);
            run_file(&mut interp, path);
            repl::run(&mut interp);
            0
        }
        flag if flag.starts_with('-') && flag.len() > 1 => usage_error(&format!("unknown option {flag}")),
        path => {
            set_args(&mut interp, &argv[1..]);
            run_file(&mut interp, path)
        }
    }
}

fn usage_error(msg: &str) -> i32 {
    eprintln!("pdfm: {msg}\n\n{USAGE}");
    2
}

fn report(r: error::Result<()>, origin: &str) -> i32 {
    match r {
        Ok(()) => 0,
        Err(e) => {
            match e.pos {
                Some(p) => eprintln!("{origin}:{}:{}: {}", p.line, p.col, e.msg),
                None => eprintln!("{origin}: {}", e.msg),
            }
            1
        }
    }
}

fn run_file(interp: &mut Interp, path: &str) -> i32 {
    match std::fs::read_to_string(path) {
        Ok(src) => report(interp.run_source(&src).map(|_| ()), path),
        Err(e) => {
            eprintln!("pdfm: cannot read '{path}': {e}");
            1
        }
    }
}

fn run_stdin(interp: &mut Interp) -> i32 {
    let mut src = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut src) {
        eprintln!("pdfm: cannot read stdin: {e}");
        return 1;
    }
    report(interp.run_source(&src).map(|_| ()), "<stdin>")
}

fn eval_mode(interp: &mut Interp, rest: &[String]) -> i32 {
    let Some(expr) = rest.first() else { return usage_error("-e needs an expression") };
    let mut out: Option<&str> = None;
    let mut files = Vec::new();
    let mut i = 1;
    while i < rest.len() {
        if rest[i] == "-o" {
            let Some(o) = rest.get(i + 1) else { return usage_error("-o needs a path") };
            out = Some(o);
            i += 2;
        } else {
            files.push(rest[i].as_str());
            i += 1;
        }
    }
    let mut n = 0;
    for f in files {
        let (name, path) = match f.split_once('=') {
            Some((name, path)) if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') => {
                (name.to_string(), path)
            }
            _ => {
                n += 1;
                (format!("p{n}"), f)
            }
        };
        match pdf::Pdf::open(path, None) {
            Ok(p) => {
                interp.globals.insert(name, Value::pdf(p));
            }
            Err(e) => {
                eprintln!("pdfm: {}", e.msg);
                return 1;
            }
        }
    }
    interp.globals.insert("args".into(), Value::list(vec![]));
    let result = match interp.run_source(expr) {
        Ok(v) => v,
        Err(e) => return report(Err(e), "<expr>"),
    };
    match (result, out) {
        (Some(Value::Pdf(p)), Some(path)) => match p.save(path) {
            Ok(bytes) => {
                println!("wrote {path} ({} pages, {})", p.len(), compress::human(bytes as u64));
                0
            }
            Err(e) => report(Err(e), "<expr>"),
        },
        (_, Some(_)) => {
            eprintln!("pdfm: -o given but the expression did not produce a pdf");
            1
        }
        (Some(Value::None) | None, None) => 0,
        (Some(v), None) => {
            println!("{}", v.repr());
            0
        }
    }
}
