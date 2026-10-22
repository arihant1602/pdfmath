# Changelog

## 0.1.0

First release.
- PDF operators: `-` and `+` (merge or drop pages), `*` (repeat), `--` (compress to a target size), and Python slicing with index/slice assignment.
- The `.pm` language: a Python subset with functions, lambdas, comprehensions and f-strings.
- An interactive `>>>` prompt with continuation lines and history.
- CLI modes: `-e` (one expression, with `-o`), `-c`, `-i`, and scripts read from stdin.
- `pdfm mcp`: an MCP server with the `pdf_info`, `pdf_eval` and `pdf_script` tools.
- npm distribution with prebuilt binaries for Linux, macOS and Windows on x64 and arm64.
