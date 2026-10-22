// `pdfm mcp`: a Model Context Protocol server over stdio (newline-delimited JSON-RPC),
// so AI agents can edit PDFs through tool calls.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use crate::builtins::EXIT_PREFIX;
use crate::interp::Interp;
use crate::json::{self, Json};
use crate::value::Value;

const SEMANTICS: &str = "Operators (pages are 0-indexed): a - b = a's pages then b's; \
a + b = b's pages then a's (note the order); p - n drops the LAST n pages; p + n drops the FIRST n pages; \
p * n repeats; p -- 2mb compresses to <= that size (units b/kb/mb/gb, 1kb = 1024 bytes; fails with the smallest \
achievable size if impossible); p[i] is one page (negative counts from the end); p[a:b:c] is Python slicing \
(p[::-1] reverses, p[::2] keeps pages 0,2,4..., p[1:-1] trims both ends); rotate(p, 90) rotates clockwise; \
blank(n) makes n empty A4 pages; len(p) is the page count.";

const INSTRUCTIONS: &str = "pdfmath edits PDFs locally and offline. Use pdf_info to inspect a file, pdf_eval for \
one-expression edits (merge, cut, reorder, rotate, compress) and pdf_script for multi-step jobs. \
Always write results to a new output path unless the user asked to overwrite a file, and report the output path, \
page count and size back to the user.";

fn tools() -> Json {
    let s = |t: &str| Json::obj(vec![("type", Json::str(t))]);
    let described = |t: &str, d: &str| Json::obj(vec![("type", Json::str(t)), ("description", Json::str(d))]);
    let cwd = described("string", "Directory that relative paths are resolved against (default: the server's working directory).");
    Json::Arr(vec![
        Json::obj(vec![
            ("name", Json::str("pdf_info")),
            ("title", Json::str("Inspect a PDF")),
            (
                "description",
                Json::str("Page count, page sizes (points) and file size of a PDF. Use before editing to know what you are working with."),
            ),
            (
                "inputSchema",
                Json::obj(vec![
                    ("type", Json::str("object")),
                    (
                        "properties",
                        Json::obj(vec![
                            ("path", described("string", "Path to the PDF.")),
                            ("password", described("string", "User password, for encrypted PDFs.")),
                            ("cwd", cwd.clone()),
                        ]),
                    ),
                    ("required", Json::Arr(vec![Json::str("path")])),
                ]),
            ),
            ("annotations", Json::obj(vec![("readOnlyHint", Json::Bool(true)), ("openWorldHint", Json::Bool(false))])),
        ]),
        Json::obj(vec![
            ("name", Json::str("pdf_eval")),
            ("title", Json::str("Edit PDFs with one expression")),
            (
                "description",
                Json::str(format!(
                    "Merge, cut, reorder, rotate or compress PDFs with one pdfmath expression and save the result. \
                     Bind input files with `inputs` (e.g. {{\"a\": \"report.pdf\", \"b\": \"appendix.pdf\"}}), then refer to them by name \
                     in `expression` (e.g. \"a - b[2:]\"). Set `output` to write the resulting PDF; it must be a new file unless \
                     `overwrite` is true. The expression cannot write files itself (save/run are disabled). {SEMANTICS} \
                     Examples: \"a - b\" (a then b), \"a[::-1]\", \"a + 1 - 1\" (drop first and last page), \"a -- 1mb\", \
                     \"a[0] - rotate(a[1:], 90)\"."
                )),
            ),
            (
                "inputSchema",
                Json::obj(vec![
                    ("type", Json::str("object")),
                    (
                        "properties",
                        Json::obj(vec![
                            ("expression", described("string", "A pdfmath expression over the bound input names.")),
                            (
                                "inputs",
                                Json::obj(vec![
                                    ("type", Json::str("object")),
                                    ("description", Json::str("Map of variable name to PDF path, e.g. {\"a\": \"in.pdf\"}.")),
                                    ("additionalProperties", s("string")),
                                ]),
                            ),
                            ("output", described("string", "Where to save the resulting PDF. Omit to just evaluate (e.g. len(a)).")),
                            (
                                "overwrite",
                                described("boolean", "Allow `output` to replace an existing file (including an input). Only set this if the user asked to overwrite."),
                            ),
                            ("cwd", cwd.clone()),
                        ]),
                    ),
                    ("required", Json::Arr(vec![Json::str("expression")])),
                ]),
            ),
            // Not destructive: without `overwrite` it only ever creates a new file.
            ("annotations", Json::obj(vec![("destructiveHint", Json::Bool(false)), ("openWorldHint", Json::Bool(false))])),
        ]),
        Json::obj(vec![
            ("name", Json::str("pdf_script")),
            ("title", Json::str("Run a pdfmath script")),
            (
                "description",
                Json::str(format!(
                    "Run a multi-line pdfmath (.pm) program for jobs that need loops, conditions or several outputs. \
                     The language is Python syntax (def, for, if, list comprehensions, f-strings) with PDF values. \
                     Functions: open(path, password=None), save(p, path) -> bytes written, glob(\"dir/*.pdf\") -> sorted paths, \
                     info(p), size(p), pages(p), rotate(p, deg), reverse(p), blank(n), exists(path), print(...). \
                     {SEMANTICS} Text printed by the program is returned. Example: \
                     \"out = blank(0)\\nfor f in glob('scans/*.pdf'):\\n    out = out - open(f)\\nprint(save(out -- 5mb, 'all.pdf'))\""
                )),
            ),
            (
                "inputSchema",
                Json::obj(vec![
                    ("type", Json::str("object")),
                    (
                        "properties",
                        Json::obj(vec![
                            ("code", described("string", "The program source (newlines and indentation as in Python).")),
                            (
                                "args",
                                Json::obj(vec![
                                    ("type", Json::str("array")),
                                    ("items", s("string")),
                                    ("description", Json::str("Values for the `args` list inside the program.")),
                                ]),
                            ),
                            ("cwd", cwd),
                        ]),
                    ),
                    ("required", Json::Arr(vec![Json::str("code")])),
                ]),
            ),
            ("annotations", Json::obj(vec![("destructiveHint", Json::Bool(true)), ("openWorldHint", Json::Bool(false))])),
        ]),
    ])
}

pub fn serve() -> i32 {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let replies: Vec<Json> = match json::parse(&line) {
            Ok(Json::Arr(batch)) => batch.iter().filter_map(handle).collect(),
            Ok(msg) => handle(&msg).into_iter().collect(),
            Err(e) => vec![error_reply(Json::Null, -32700, &format!("parse error: {e}"))],
        };
        for r in replies {
            if writeln!(stdout, "{r}").and_then(|_| stdout.flush()).is_err() {
                return 0;
            }
        }
    }
    0
}

fn error_reply(id: Json, code: i64, msg: &str) -> Json {
    Json::obj(vec![
        ("jsonrpc", Json::str("2.0")),
        ("id", id),
        ("error", Json::obj(vec![("code", Json::Num(code as f64)), ("message", Json::str(msg))])),
    ])
}

/// Handle one message; notifications (no id) get no reply.
fn handle(msg: &Json) -> Option<Json> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Json::Obj(vec![]));
    let id = id?;
    let result = match method {
        "initialize" => {
            let version = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or("2025-06-18").to_string();
            Json::obj(vec![
                ("protocolVersion", Json::str(version)),
                ("capabilities", Json::obj(vec![("tools", Json::obj(vec![("listChanged", Json::Bool(false))]))])),
                (
                    "serverInfo",
                    Json::obj(vec![("name", Json::str("pdfmath")), ("version", Json::str(env!("CARGO_PKG_VERSION")))]),
                ),
                ("instructions", Json::str(INSTRUCTIONS)),
            ])
        }
        "ping" => Json::Obj(vec![]),
        "tools/list" => Json::obj(vec![("tools", tools())]),
        "tools/call" => {
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(Json::Obj(vec![]));
            let (text, is_error) = match call_tool(name, &args) {
                Ok(t) => (t, false),
                Err(e) => (e, true),
            };
            Json::obj(vec![
                ("content", Json::Arr(vec![Json::obj(vec![("type", Json::str("text")), ("text", Json::str(text))])])),
                ("isError", Json::Bool(is_error)),
            ])
        }
        "resources/list" => Json::obj(vec![("resources", Json::Arr(vec![]))]),
        "prompts/list" => Json::obj(vec![("prompts", Json::Arr(vec![]))]),
        _ => return Some(error_reply(id, -32601, &format!("method not found: {method}"))),
    };
    Some(Json::obj(vec![("jsonrpc", Json::str("2.0")), ("id", id), ("result", result)]))
}

/// Runs `f` with the process working directory switched to `cwd` (the server
/// handles one request at a time, so this is safe).
fn in_dir<T>(cwd: Option<&str>, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let Some(dir) = cwd else { return f() };
    let prev = std::env::current_dir().map_err(|e| e.to_string())?;
    std::env::set_current_dir(dir).map_err(|e| format!("cannot use cwd '{dir}': {e}"))?;
    let r = f();
    let _ = std::env::set_current_dir(prev);
    r
}

fn str_arg<'a>(args: &'a Json, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

fn call_tool(name: &str, args: &Json) -> Result<String, String> {
    let cwd = str_arg(args, "cwd");
    match name {
        "pdf_info" => {
            let path = str_arg(args, "path").ok_or("missing 'path'")?;
            let password = str_arg(args, "password");
            in_dir(cwd, || {
                let p = crate::pdf::Pdf::open(path, password).map_err(|e| e.msg)?;
                let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                Ok(format!("file: {path}\nfile size: {}\n{}", crate::compress::human(bytes), crate::builtins::info(&p)))
            })
        }
        "pdf_eval" => {
            let expr = str_arg(args, "expression").ok_or("missing 'expression'")?;
            let output = str_arg(args, "output");
            let overwrite = matches!(args.get("overwrite"), Some(Json::Bool(true)));
            in_dir(cwd, || {
                let mut it = Interp::new();
                it.capture = Some(String::new());
                // The expression itself may not write files: the only write is `output`.
                it.restricted = true;
                let mut input_paths = Vec::new();
                if let Some(Json::Obj(inputs)) = args.get("inputs") {
                    for (var, path) in inputs {
                        let path = path.as_str().ok_or_else(|| format!("inputs.{var} must be a path string"))?;
                        if !var.chars().all(|c| c.is_alphanumeric() || c == '_') || var.starts_with(|c: char| c.is_ascii_digit()) {
                            return Err(format!("invalid input name '{var}' (use letters, digits and _)"));
                        }
                        let p = crate::pdf::Pdf::open(path, None).map_err(|e| e.msg)?;
                        it.globals.insert(var.clone(), Value::pdf(p));
                        input_paths.push(path.to_string());
                    }
                }
                if let Some(out) = output
                    && !overwrite
                    && Path::new(out).exists()
                {
                    let what = if input_paths.iter().any(|p| same_file(p, out)) { "input file" } else { "existing file" };
                    return Err(format!(
                        "refusing to overwrite {what} '{out}': choose another output path, or set overwrite=true if the user asked for it"
                    ));
                }
                let value = it.run_source(expr).map_err(|e| e.to_string())?;
                let mut text = it.capture.take().unwrap_or_default();
                match (value, output) {
                    (Some(Value::Pdf(p)), Some(out)) => {
                        let bytes = p.save(out).map_err(|e| e.msg)?;
                        text.push_str(&format!("wrote {out} ({} pages, {})", p.len(), crate::compress::human(bytes as u64)));
                    }
                    (_, Some(_)) => return Err("`output` was given but the expression did not produce a pdf".into()),
                    (Some(v), None) if !matches!(v, Value::None) => text.push_str(&v.repr()),
                    _ => {}
                }
                Ok(if text.is_empty() { "ok".into() } else { text })
            })
        }
        "pdf_script" => {
            let code = str_arg(args, "code").ok_or("missing 'code'")?;
            let script_args: Vec<Value> = match args.get("args") {
                Some(Json::Arr(a)) => a.iter().map(|v| Value::str(v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))).collect(),
                _ => vec![],
            };
            in_dir(cwd, || {
                let mut it = Interp::new();
                it.capture = Some(String::new());
                it.globals.insert("args".into(), Value::list(script_args));
                let result = it.run_source(code);
                let out = it.capture.take().unwrap_or_default();
                match result {
                    Ok(_) => Ok(if out.is_empty() { "ok (no output)".into() } else { out }),
                    Err(e) if e.msg.starts_with(EXIT_PREFIX) => {
                        let code = &e.msg[EXIT_PREFIX.len()..];
                        if code == "0" { Ok(out) } else { Err(format!("{out}exited with code {code}")) }
                    }
                    Err(e) => Err(format!("{out}{}error: {e}", if out.is_empty() || out.ends_with('\n') { "" } else { "\n" })),
                }
            })
        }
        _ => Err(format!("unknown tool '{name}'")),
    }
}

fn same_file(a: &str, b: &str) -> bool {
    let canon = |p: &str| std::fs::canonicalize(p).unwrap_or_else(|_| absolute(Path::new(p)));
    canon(a) == canon(b)
}

fn absolute(p: &Path) -> PathBuf {
    std::env::current_dir().map(|d| d.join(p)).unwrap_or_else(|_| p.to_path_buf())
}
