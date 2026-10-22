# Typical page surgery on one document.
#   pdfm examples/booklet.pm input.pdf
p = open(args[0])

cover = p[0]
body = p + 1 - 1                    # drop first and last page
odd = body[::2]                     # 1st, 3rd, 5th ... page of the body
even = body[1::2]

def interleave(a, b):
    out = a[:0]                     # empty pdf
    for i in range(max(len(a), len(b))):
        if i < len(a): out = out - a[i]
        if i < len(b): out = out - b[i]
    return out

save(cover - interleave(even, odd) - p[-1], "reordered.pdf")
save(rotate(body, 90), "landscape.pdf")
save(p -- 1mb, "small.pdf")         # compress to <= 1 MiB
print("wrote reordered.pdf, landscape.pdf, small.pdf")
