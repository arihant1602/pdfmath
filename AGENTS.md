# Notes for AI agents

There are two audiences here: agents that **use** `pdfm` to do PDF work, and agents that
**change** this repository.

To install `pdfm` on a machine, follow [docs/AGENT_PROMPT.md](docs/AGENT_PROMPT.md).

## Using pdfm

- Never use the interactive prompt. Use one of these instead:
  - `pdfm -e 'EXPR' -o out.pdf in1.pdf in2.pdf` for one expression. The inputs are `p1`, `p2`, …, or a name with `name=path.pdf`.
  - `pdfm -c 'CODE'` for several statements.
  - `pdfm job.pm`, or `pdfm - < job.pm`, for scripts.
- Results:
  - Exit code 0 means success and 1 means an error. Errors are printed to stderr as `origin:line:col: Kind: message`.
  - With `-o`, success prints `wrote out.pdf (N pages, SIZE)`.
- Inspecting files: `pdfm -e 'len(p1)' f.pdf` prints the page count. `pdfm -e 'info(p1)' f.pdf` prints the sources and page sizes.
- Operator semantics are deliberately unusual. Don't guess from math intuition:
  - `a - b` gives a then b, and `a + b` gives **b then a**.
  - `p - n` drops the last n pages and `p + n` drops the first n.
  - `p -- 5mb` compresses to at most the given size.
- Pages are 0-indexed, and slicing matches Python lists exactly.
- Write output to a new file. Don't overwrite inputs unless the user asks.
- Full reference: `pdfm --help-lang` and [docs/language.md](docs/language.md).

## Changing this repository

- Rust 2024 edition, minimum Rust 1.88. Keep dependencies minimal: a new crate needs a strong reason.
- Everything must run offline. No network access at runtime.
- Before committing, `cargo clippy --all-targets -- -D warnings` and `cargo test` must pass.
- Layout:
  - `src/lexer.rs` handles tokens and indentation, and `src/parser.rs` builds the AST (`src/ast.rs`).
  - `src/interp.rs` is the evaluator. Its `binary()` is where the pdf operators live.
  - `src/builtins.rs` holds the builtin functions and methods. The `--help-lang` text (`HELP`) is there too.
  - `src/pdf.rs` is the lazy page model, and `materialize()` copies only the objects that are reachable.
  - `src/compress.rs` implements `--`: lossless steps first, then the JPEG ladder with binary search.
  - `src/slice.rs` implements Python's slice rules.
- Tests: `tests/cli.rs` builds fixture PDFs whose pages have distinct widths, so page order can be asserted.
- Releases:
  1. Bump `version` in `Cargo.toml`.
  2. Run `scripts/sync-version.sh`.
  3. Commit, then push a `vX.Y.Z` tag.
  4. `.github/workflows/release.yml` then builds every platform and publishes to npm, using the `NPM_TOKEN` secret.
