# Install prompt for AI agents

Copy the block below and paste it into Claude Code, Codex, Cursor, Gemini CLI or any coding
agent that can run shell commands. The agent installs `pdfm`, registers it as an MCP server so
it can edit PDFs through tool calls from then on, checks that it works, and learns the syntax.
After that, just ask for PDF jobs ("merge these three and drop the last page", "get this under
2 MB", ...).

```text
Install the pdfmath command-line tool (command name: pdfm) on this machine, verify it, and learn how to use it.

1. Check whether it is already installed by running `pdfm --version`. If it prints a version, skip to step 4.
2. Install it:
   a. If npm is available, run `npm install -g @awryhant/pdfmath`.
   b. If that fails (no npm, package not found, or no prebuilt binary for this platform) and Rust's
      `cargo` is available, run `cargo install --locked --git https://github.com/arihant1602/pdfmath`
      (this needs Rust 1.88 or newer).
   c. If neither works, stop and tell me what is missing. Do not install Node.js or Rust, and do not use sudo, without asking me first.
3. If a global npm install fails with a permissions error (EACCES), do not retry with sudo. Tell me, and suggest
   `npm config set prefix ~/.local` (with ~/.local/bin on PATH) as the fix.
4. Verify: `pdfm -e 'len(blank(3) - blank(2))'` must print 5.
5. Register pdfm as an MCP server for yourself, so that you edit PDFs through its tools from now on. The server command is `pdfm mcp` (stdio).
   - Claude Code: `claude mcp add --scope user pdfmath -- pdfm mcp`
   - Codex CLI: `codex mcp add pdfmath -- pdfm mcp`
   - Gemini CLI: `gemini mcp add pdfmath pdfm mcp`
   - Cursor, Claude Desktop or any other client: add {"mcpServers": {"pdfmath": {"command": "pdfm", "args": ["mcp"]}}} to that client's MCP config file, merging it with any servers that are already there.
   If a server named pdfmath already exists, leave it alone. Then tell me to restart the session so the tools load.
   The tools are pdf_info (inspect), pdf_eval (one expression over named inputs, saved to `output`) and pdf_script (a multi-line program).
6. Run `pdfm --help` and `pdfm --help-lang` and read the output. Key facts:
   - Pages are 0-indexed, and slicing works exactly like Python lists (p[::-1], p[1:-1], p[::2]).
   - `a - b` merges a then b. `a + b` merges b then a.
   - `p - n` drops the last n pages. `p + n` drops the first n.
   - `p -- 2mb` compresses to at most 2 MiB, or fails with an error naming the smallest size it can reach.
   - One-liners: pdfm -e 'EXPR' -o out.pdf file1.pdf file2.pdf   (the files are bound to p1, p2, ...)
   - Scripts: .pm files use Python syntax; run them with `pdfm script.pm args...`.
   - Errors go to stderr as `origin:line:col: message`, with exit code 1.
7. Reply with the installed version, where you registered the MCP server, and a three-line cheat-sheet. From now on, use the pdfmath MCP tools
   (or the pdfm command when the tools aren't loaded yet) for PDF tasks I give you.
   Never overwrite one of my input files unless I ask you to, and check the result (for example with `pdfm -e 'len(p1)' out.pdf`).
```

## Installing by hand

```sh
npm install -g @awryhant/pdfmath                                    # prebuilt binary, no Rust needed
cargo install --locked --git https://github.com/arihant1602/pdfmath   # build from source
claude mcp add --scope user pdfmath -- pdfm mcp            # register with Claude Code (see docs/mcp.md for others)
```
