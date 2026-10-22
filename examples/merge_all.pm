# Merge every pdf in a folder (alphabetical order) into one file.
#   pdfm examples/merge_all.pm scans/ merged.pdf
folder = args[0] if len(args) > 0 else "."
out = args[1] if len(args) > 1 else "merged.pdf"

files = [f for f in glob(folder.rstrip("/") + "/*.pdf") if f != out]
if not files:
    print(f"no pdfs in {folder}")
    exit(1)

result = open(files[0])
for f in files[1:]:
    result = result - open(f)      # '-' appends: left then right

n = save(result, out)
print(f"{len(files)} files -> {out}: {len(result)} pages, {n} bytes")
