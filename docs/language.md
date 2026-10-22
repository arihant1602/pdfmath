# The .pm language

`.pm` is a subset of Python with first-class PDF values. If something here doesn't say otherwise,
it behaves the way Python does.

## PDF values

A `pdf` is an immutable sequence of pages. Some operations produce a new pdf rather than
changing one: `open()`, slicing, the operators below, `rotate()`, `reverse()` and `blank()`.
Assignment forms like `p[i] = q` and `del p[i]` build a new pdf and rebind the variable.

Pages are *references* to pages in the files you opened. Slicing and merging are instant, even
for large files. The output document is built only by `save()`, `size()` or `--`. When it is
built, the result holds exactly the objects that the chosen pages use: content, fonts, images,
annotations and links. Page attributes inherited from the source's page tree (Resources,
MediaBox, CropBox, Rotate) are copied onto each page.

Document-level structure is not carried over: bookmarks/outlines, AcroForm fields, the structure
tree, and named destinations. A link whose target page isn't in the output stops pointing anywhere.

### Operators

| Expression | Result |
|---|---|
| `a - b` | a's pages, then b's |
| `a + b` | b's pages, then a's |
| `p - n` | without the last n pages (an empty pdf if n ≥ len) |
| `p + n` | without the first n pages |
| `p * n`, `n * p` | p repeated n times |
| `p -- size` | compressed so that the saved file is ≤ size bytes |
| `p[i]` | page i as a one-page pdf; negative i counts from the end |
| `p[a:b:c]` | Python slice |
| `q in p` | every page of q also appears in p |
| `p == q` | same pages, in the same order, with the same rotation |
| `len(p)` | page count |
| `for page in p:` | iterates over one-page pdfs |

`n` must be a non-negative int. A negative `n` raises an error.

Size literals: `500kb`, `1.5mb`, `2gb`, `100b`, and also `k`, `m` and `g`, in any letter
case. The units are binary, so `1kb` is 1024. A plain int means bytes.

`--` is a single token. `p--2` means "compress p to 2 bytes", which will fail. If you need
"minus negative", put a space in: `p - -2`. For non-pdf values, `x -- y` still means
`x - (-y)`, as it does in Python.

### Compression (`--`)

1. **Lossless.** Re-deflate streams at maximum compression, merge identical streams (repeated fonts and images), drop unused objects and XMP metadata, and use object streams with cross-reference streams.
2. **Lossy.** Image XObjects are decoded and re-encoded as baseline JPEG. Supported: 8-bit Gray/RGB/ICC(1 or 3 components), DCT or Flate/LZW. The encoder walks a ladder of `(scale, quality)` steps, from `(100%, q95)` down to `(6%, q15)`, and binary-searches for the mildest step whose *serialized output* fits the target. These are left alone:
   - an image is only replaced when the new encoding is smaller
   - soft masks and stencil masks stay lossless
   - CMYK images, Indexed images, and images smaller than 64×64 are skipped
3. If even the most aggressive step does not fit, nothing is written. The error reports the smallest size that was reached.

The result keeps its exact bytes. `save(p -- 2mb, "out.pdf")` therefore writes precisely the
measured file. If you slice or merge the result afterwards, the document is rebuilt, and the
size guarantee no longer applies.

## Python subset

Supported:
- **Statements:** assignment (including `a = b = 1`, `a, b = b, a`, `+=` and the other augmented forms), `if`/`elif`/`else`, `while`, `for`, `break`, `continue`, `pass`, `def` (with defaults, keyword arguments and `->`/`:` annotations, which are ignored), `return`, `del`, `global`, and `;`-separated simple statements.
- **Expressions:**
  - arithmetic `+ - * / // % **`, with Python's floor division and modulo signs
  - chained comparisons, `in`/`not in`, `is`/`is not`, `and`/`or`/`not`
  - the conditional expression `a if c else b`, `lambda`
  - list comprehensions, including generator arguments like `sum(x for x in xs)`
- **Types:** `int` (64-bit, errors on overflow), `float`, `str`, `bool`, `None`, `list`, `tuple`, `pdf`, and functions.
- **Strings:** `'…'`, `"…"`, `'''…'''`, raw strings `r"…"`, and f-strings with `!r` and format specs (`{x:>8}`, `{x:.2f}`, `{n:,}`, `{x:08.3f}`). Also `%` formatting and `str.format`.
- **Builtins:**
  - `print(*xs, sep=, end=)`, `len`, `range`, `str`, `repr`, `int`, `float`, `bool`, `type`, `isinstance`
  - `list`, `tuple`, `abs`, `round` (half to even), `divmod`, `min`/`max` (`key=`, `default=`), `sum`, `any`, `all`
  - `sorted` (`key=`, `reverse=`), `reversed`, `enumerate` (`start=`), `zip`, `map`, `filter`, `format`, `input`, `exit`, `help`
- **List methods:** `append` `extend` `pop` `insert` `index` `count` `remove` `reverse` `sort` `copy` `clear`
- **String methods:** `upper` `lower` `title` `strip` `lstrip` `rstrip` `split` `join` `replace` `startswith` `endswith` `find` `count` `isdigit` `zfill` `format`
- **PDF methods:** `p.save(path)` `p.rotate(deg)` `p.reverse()` `p.info()` `p.size()` `p.pages()` `p.compress(size)`

Not supported: dicts, sets, classes, `try`/`except`/`raise`, `import`, `with`, generators/`yield`,
closures over enclosing function variables (functions see their own locals and the globals),
`*args`/`**kwargs`, and slicing assignment on strings or tuples. `type(x)` returns the type
name as a string, for example `"pdf"`.

## PDF and file builtins

| Function | Description |
|---|---|
| `open(path, password=None)` | load a pdf. An encrypted file needs its user password. |
| `save(p, path)` | write a pdf. Returns the number of bytes written. |
| `rotate(p, degrees=90)` | add a clockwise rotation, which must be a multiple of 90 |
| `reverse(p)` | pages in reverse order |
| `blank(n=1, width=595, height=842)` | n empty pages, sized in points (A4 by default) |
| `info(p)` | print the page count, source page ranges and page sizes |
| `size(p)` | the number of bytes the pdf would take when saved |
| `pages(p)` | a list of one-page pdfs |
| `compress(p, size)` | same as `p -- size` |
| `glob(pattern)` | sorted matches. `*` and `?` work in the file name part only. |
| `exists(path)` | True if the file or directory exists |
| `run(path)` | execute another `.pm` file in the current session |
| `args` | list of the script's command-line arguments |

## Command line

| Command | |
|---|---|
| `pdfm` | `>>>` prompt on a terminal. When stdin is piped, it runs stdin as a script. |
| `pdfm FILE.pm [ARGS…]` | run a script |
| `pdfm -c CODE [ARGS…]` | run code from a string |
| `pdfm -e EXPR [-o OUT] [FILES…]` | evaluate one expression. Inputs are `p1, p2, …` or `name=path`. |
| `pdfm -i FILE.pm` | run a script, then open the prompt |
| `pdfm -` | read a script from stdin |
| `pdfm --help`, `--help-lang`, `--version` | |

In the prompt, a line ending in `:` or with an open bracket continues on `...`. A blank line
ends the block, and Ctrl-C clears the current input. History is kept in `~/.pdfm_history`.
