# Install prompt for AI agents

Copy the block below and paste it into Claude Code, Codex, Cursor, or any coding agent that can
run shell commands. The agent installs `pdfm`, checks that it works, and learns the syntax so
you can ask it for PDF jobs right away ("merge these three and drop the last page", "get this
under 2 MB", ...).

```text
Install the pdfmath command-line tool (command name: pdfm) on this machine, verify it, and learn how to use it.

1. Check whether it is already installed by running `pdfm --version`. If it prints a version, skip to step 4.
2. Install it:
   a. If npm is available, run `npm install -g pdfmath`.
   b. If that fails (no npm, package not found, or no prebuilt binary for this platform) and Rust's
      `cargo` is available, run `cargo install --locked --git https://github.com/arihant1602/pdfmath`
      (this needs Rust 1.88 or newer).
   c. If neither works, stop and tell me what is missing. Do not install Node.js or Rust, and do not use sudo, without asking me first.
3. If a global npm install fails with a permissions error (EACCES), do not retry with sudo. Tell me, and suggest
   `npm config set prefix ~/.local` (with ~/.local/bin on PATH) as the fix.
4. Verify: `pdfm -e 'len(blank(3) - blank(2))'` must print 5.
5. Run `pdfm --help` and `pdfm --help-lang` and read the output. Key facts:
   - Pages are 0-indexed, and slicing works exactly like Python lists (p[::-1], p[1:-1], p[::2]).
   - `a - b` merges a then b. `a + b` merges b then a.
   - `p - n` drops the last n pages. `p + n` drops the first n.
   - `p -- 2mb` compresses to at most 2 MiB, or fails with an error naming the smallest size it can reach.
   - One-liners: pdfm -e 'EXPR' -o out.pdf file1.pdf file2.pdf   (the files are bound to p1, p2, ...)
   - Scripts: .pm files use Python syntax; run them with `pdfm script.pm args...`.
   - Errors go to stderr as `origin:line:col: message`, with exit code 1.
6. Reply with the installed version and a three-line cheat-sheet. From now on, use pdfm for PDF tasks I give you.
   Never overwrite one of my input files unless I ask you to, and check the result (for example with `pdfm -e 'len(p1)' out.pdf`).
```

## Installing by hand

```sh
npm install -g pdfmath                                    # prebuilt binary, no Rust needed
cargo install --locked --git https://github.com/arihant1602/pdfmath   # build from source
```
