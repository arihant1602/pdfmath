// Interactive `>>>` prompt with Python-style continuation lines.

use rustyline::error::ReadlineError;
use rustyline::history::FileHistory;
use rustyline::{Config, Editor};

use crate::interp::Interp;
use crate::lexer::{LexError, tokenize};

pub fn run(interp: &mut Interp) {
    println!("pdfmath {} — type help() for the cheat-sheet, Ctrl-D to exit", env!("CARGO_PKG_VERSION"));
    let config = Config::builder().auto_add_history(false).build();
    let mut rl: Editor<(), FileHistory> = match Editor::with_config(config) {
        Ok(rl) => rl,
        Err(e) => {
            eprintln!("cannot start the line editor: {e}");
            return;
        }
    };
    let history = std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".pdfm_history"));
    if let Some(h) = &history {
        let _ = rl.load_history(h);
    }

    let mut buf = String::new();
    let mut in_block = false;
    loop {
        let prompt = if buf.is_empty() { ">>> " } else { "... " };
        match rl.readline(prompt) {
            Ok(line) => {
                if buf.is_empty() && line.trim().is_empty() {
                    continue;
                }
                buf.push_str(&line);
                buf.push('\n');
                if opens_block(&line) {
                    in_block = true;
                }
                let blank = line.trim().is_empty();
                let incomplete = matches!(tokenize(&buf), Err(LexError::Incomplete));
                if incomplete || (in_block && !blank) {
                    continue;
                }
                let _ = rl.add_history_entry(buf.trim_end());
                match interp.run_source(&buf) {
                    Ok(Some(v)) if !matches!(v, crate::value::Value::None) => println!("{}", v.repr()),
                    Ok(_) => {}
                    Err(e) => eprintln!("{e}"),
                }
                buf.clear();
                in_block = false;
            }
            Err(ReadlineError::Interrupted) => {
                println!("KeyboardInterrupt");
                buf.clear();
                in_block = false;
            }
            Err(ReadlineError::Eof) => break,
            Err(e) => {
                eprintln!("{e}");
                break;
            }
        }
    }
    if let Some(h) = &history {
        let _ = rl.save_history(h);
    }
}

/// Does this line end with `:` (ignoring a trailing comment)?
fn opens_block(line: &str) -> bool {
    let mut quote: Option<char> = None;
    let mut code = line;
    for (i, c) in line.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(c),
            (None, '#') => {
                code = &line[..i];
                break;
            }
            _ => {}
        }
    }
    code.trim_end().ends_with(':')
}
