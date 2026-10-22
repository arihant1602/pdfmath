// End-to-end tests: run the `pdfm` binary and inspect the PDFs it writes.
// Fixture page k of a file has MediaBox width `base + k`, so page order is checkable.

use std::path::{Path, PathBuf};
use std::process::Command;

use lopdf::{Dictionary, Document, Object, Stream};

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pdfm-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A pdf whose pages have widths base, base+1, ... Pages inherit Resources and
/// MediaBox height from the page tree, to exercise attribute flattening.
fn fixture(path: &Path, base: i64, n: usize) {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font = doc.add_object(Dictionary::from_iter(vec![
        ("Type", Object::Name(b"Font".to_vec())),
        ("Subtype", Object::Name(b"Type1".to_vec())),
        ("BaseFont", Object::Name(b"Helvetica".to_vec())),
    ]));
    let mut kids = vec![];
    for k in 0..n {
        let text = format!("BT /F1 12 Tf 10 10 Td (page {k}) Tj ET");
        let content = doc.add_object(Stream::new(Dictionary::new(), text.into_bytes()));
        let page = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Page".to_vec())),
            ("Parent", Object::Reference(pages_id)),
            ("MediaBox", vec![0.into(), 0.into(), (base + k as i64).into(), 500.into()].into()),
            ("Contents", Object::Reference(content)),
        ]);
        kids.push(Object::Reference(doc.add_object(page)));
    }
    let resources = Dictionary::from_iter(vec![("Font", Dictionary::from_iter(vec![("F1", Object::Reference(font))]).into())]);
    let pages = Dictionary::from_iter(vec![
        ("Type", Object::Name(b"Pages".to_vec())),
        ("Kids", kids.into()),
        ("Count", (n as i64).into()),
        ("Resources", resources.into()),
    ]);
    doc.objects.insert(pages_id, Object::Dictionary(pages));
    let catalog = doc.add_object(Dictionary::from_iter(vec![
        ("Type", Object::Name(b"Catalog".to_vec())),
        ("Pages", Object::Reference(pages_id)),
    ]));
    doc.trailer.set("Root", catalog);
    doc.save(path).unwrap();
}

/// A pdf with one large, noisy RGB image per page (hard to compress losslessly).
fn image_fixture(path: &Path, pages: usize) {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let (w, h) = (900usize, 900usize);
    let mut kids = vec![];
    let mut seed: u32 = 12345;
    for _ in 0..pages {
        let mut px = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                let noise = (seed >> 24) as u8 / 4;
                px.push(((x * 255 / w) as u8).wrapping_add(noise));
                px.push(((y * 255 / h) as u8).wrapping_add(noise));
                px.push(128u8.wrapping_add(noise));
            }
        }
        let mut img = Stream::new(
            Dictionary::from_iter(vec![
                ("Type", Object::Name(b"XObject".to_vec())),
                ("Subtype", Object::Name(b"Image".to_vec())),
                ("Width", (w as i64).into()),
                ("Height", (h as i64).into()),
                ("ColorSpace", Object::Name(b"DeviceRGB".to_vec())),
                ("BitsPerComponent", 8.into()),
            ]),
            px,
        );
        img.compress().unwrap();
        let img_id = doc.add_object(img);
        let content = doc.add_object(Stream::new(Dictionary::new(), b"q 500 0 0 500 0 0 cm /Im0 Do Q".to_vec()));
        let res = Dictionary::from_iter(vec![("XObject", Dictionary::from_iter(vec![("Im0", Object::Reference(img_id))]).into())]);
        let page = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Page".to_vec())),
            ("Parent", Object::Reference(pages_id)),
            ("MediaBox", vec![0.into(), 0.into(), 500.into(), 500.into()].into()),
            ("Resources", res.into()),
            ("Contents", Object::Reference(content)),
        ]);
        kids.push(Object::Reference(doc.add_object(page)));
    }
    let pages_dict = Dictionary::from_iter(vec![
        ("Type", Object::Name(b"Pages".to_vec())),
        ("Kids", kids.into()),
        ("Count", (pages as i64).into()),
    ]);
    doc.objects.insert(pages_id, Object::Dictionary(pages_dict));
    let catalog = doc.add_object(Dictionary::from_iter(vec![
        ("Type", Object::Name(b"Catalog".to_vec())),
        ("Pages", Object::Reference(pages_id)),
    ]));
    doc.trailer.set("Root", catalog);
    doc.save(path).unwrap();
}

fn pdfm(dir: &Path, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_pdfm")).args(args).current_dir(dir).output().unwrap();
    (out.status.success(), String::from_utf8_lossy(&out.stdout).into(), String::from_utf8_lossy(&out.stderr).into())
}

/// Page widths of a saved pdf, in order.
fn widths(path: &Path) -> Vec<i64> {
    let doc = Document::load(path).unwrap();
    doc.get_pages()
        .values()
        .map(|id| {
            let page = doc.get_dictionary(*id).unwrap();
            let mb = page.get(b"MediaBox").unwrap().as_array().unwrap();
            mb[2].as_float().unwrap().round() as i64
        })
        .collect()
}

fn setup(name: &str) -> PathBuf {
    let d = tmpdir(name);
    fixture(&d.join("a.pdf"), 100, 5); // widths 100..104
    fixture(&d.join("b.pdf"), 200, 2); // widths 200..201
    d
}

fn eval(d: &Path, expr: &str) -> Vec<i64> {
    let (ok, _, err) = pdfm(d, &["-e", expr, "-o", "out.pdf", "a=a.pdf", "b=b.pdf"]);
    assert!(ok, "{expr}: {err}");
    widths(&d.join("out.pdf"))
}

#[test]
fn merge_order() {
    let d = setup("merge");
    assert_eq!(eval(&d, "a - b"), vec![100, 101, 102, 103, 104, 200, 201]);
    assert_eq!(eval(&d, "a + b"), vec![200, 201, 100, 101, 102, 103, 104]);
}

#[test]
fn drop_pages() {
    let d = setup("drop");
    assert_eq!(eval(&d, "a - 2"), vec![100, 101, 102]);
    assert_eq!(eval(&d, "a + 2"), vec![102, 103, 104]);
    assert_eq!(eval(&d, "a + 1 - 1"), vec![101, 102, 103]);
}

#[test]
fn slicing() {
    let d = setup("slice");
    assert_eq!(eval(&d, "a[::-1]"), vec![104, 103, 102, 101, 100]);
    assert_eq!(eval(&d, "a[1:-1:2]"), vec![101, 103]);
    assert_eq!(eval(&d, "a[-1]"), vec![104]);
    assert_eq!(eval(&d, "a[::2] - b[-1]"), vec![100, 102, 104, 201]);
}

#[test]
fn repeated_pages() {
    let d = setup("repeat");
    assert_eq!(eval(&d, "a[0] * 3 - a[0]"), vec![100, 100, 100, 100]);
    let doc = Document::load(d.join("out.pdf")).unwrap();
    assert_eq!(doc.get_pages().len(), 4);
}

#[test]
fn slice_assignment_and_del() {
    let d = setup("assign");
    std::fs::write(
        d.join("s.pm"),
        "a = open('a.pdf')\nb = open('b.pdf')\na[1] = b\ndel a[-1]\na[::2] = a[::2][::-1]\nsave(a, 'out.pdf')\n",
    )
    .unwrap();
    let (ok, _, err) = pdfm(&d, &["s.pm"]);
    assert!(ok, "{err}");
    // after a[1] = b: 100 200 201 102 103 104; del a[-1]: 100 200 201 102 103
    // even positions (100, 201, 103) reversed in place
    assert_eq!(widths(&d.join("out.pdf")), vec![103, 200, 201, 102, 100]);
}

#[test]
fn inherited_resources_survive() {
    let d = setup("inherit");
    eval(&d, "a[2:3]");
    let doc = Document::load(d.join("out.pdf")).unwrap();
    let id = *doc.get_pages().values().next().unwrap();
    let page = doc.get_dictionary(id).unwrap();
    let res = page.get(b"Resources").unwrap();
    let res = match res {
        Object::Reference(r) => doc.get_dictionary(*r).unwrap(),
        o => o.as_dict().unwrap(),
    };
    assert!(res.get(b"Font").is_ok(), "font resources were lost");
    assert!(String::from_utf8_lossy(&doc.get_page_content(id)).contains("page 2"));
}

#[test]
fn rotation() {
    let d = setup("rotate");
    eval(&d, "rotate(a[0], 90) - a[1].rotate(-90)");
    let doc = Document::load(d.join("out.pdf")).unwrap();
    let rots: Vec<i64> = doc
        .get_pages()
        .values()
        .map(|id| doc.get_dictionary(*id).unwrap().get(b"Rotate").unwrap().as_i64().unwrap())
        .collect();
    assert_eq!(rots, vec![90, 270]);
}

#[test]
fn compress_to_target() {
    let d = tmpdir("compress");
    image_fixture(&d.join("img.pdf"), 2);
    let original = std::fs::metadata(d.join("img.pdf")).unwrap().len();
    assert!(original > 600 * 1024, "fixture too small: {original}");
    let (ok, _, err) = pdfm(&d, &["-e", "p1 -- 200kb", "-o", "small.pdf", "img.pdf"]);
    assert!(ok, "{err}");
    let size = std::fs::metadata(d.join("small.pdf")).unwrap().len();
    assert!(size <= 200 * 1024, "{size} > 200kb");
    assert_eq!(Document::load(d.join("small.pdf")).unwrap().get_pages().len(), 2);

    let (ok, _, err) = pdfm(&d, &["-e", "p1 -- 2kb", "-o", "tiny.pdf", "img.pdf"]);
    assert!(!ok);
    assert!(err.contains("cannot compress to"), "{err}");
    assert!(!d.join("tiny.pdf").exists());
}

#[test]
fn script_errors_have_positions() {
    let d = tmpdir("errors");
    std::fs::write(d.join("bad.pm"), "x = 1\ny = x + nope\n").unwrap();
    let (ok, _, err) = pdfm(&d, &["bad.pm"]);
    assert!(!ok);
    assert!(err.starts_with("bad.pm:2:"), "{err}");
    assert!(err.contains("NameError"), "{err}");
}

#[test]
fn stdin_script_and_args() {
    let d = tmpdir("stdin");
    let mut child = Command::new(env!("CARGO_BIN_EXE_pdfm"))
        .args(["-", "hello"])
        .current_dir(&d)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child.stdin.take().unwrap().write_all(b"for a in args:\n    print(a.upper())\nprint(len(blank(3) - blank(2)))\n").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "HELLO\n5\n");
}

#[test]
fn hostile_image_dimensions_do_not_crash() {
    let d = tmpdir("hostile");
    image_fixture(&d.join("img.pdf"), 1);
    let mut doc = Document::load(d.join("img.pdf")).unwrap();
    for obj in doc.objects.values_mut() {
        if let Object::Stream(s) = obj
            && s.dict.has(b"Width")
        {
            s.dict.set("Width", i64::MAX / 2);
            s.dict.set("Height", -5);
        }
    }
    doc.save(d.join("bad.pdf")).unwrap();
    let (ok, _, err) = pdfm(&d, &["-e", "p1 -- 10kb", "-o", "o.pdf", "bad.pdf"]);
    assert!(!ok);
    assert!(err.contains("cannot compress to"), "{err}");
}
