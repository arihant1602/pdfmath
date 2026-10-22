// Lazy page model. A Pdf value is a list of references to pages in loaded
// source documents; nothing is copied until the result is written out.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use crate::error::{Error, Result};

pub struct SourceDoc {
    pub id: usize,
    pub name: String,
    pub doc: Document,
}

#[derive(Clone)]
pub struct PageRef {
    pub src: Rc<SourceDoc>,
    pub page: ObjectId,
    /// Extra clockwise rotation in degrees (multiple of 90) applied on top of the page's own.
    pub rotate: i64,
}

impl PartialEq for PageRef {
    fn eq(&self, o: &Self) -> bool {
        self.src.id == o.src.id && self.page == o.page && self.rotate.rem_euclid(360) == o.rotate.rem_euclid(360)
    }
}

#[derive(Clone, Default)]
pub struct Pdf {
    pub pages: Vec<PageRef>,
    /// Exact serialized bytes, kept when this value came straight out of `--`
    /// so that saving it reproduces the size that was promised.
    pub bytes: Option<Rc<Vec<u8>>>,
}

thread_local! {
    static NEXT_ID: Cell<usize> = const { Cell::new(1) };
}

fn next_id() -> usize {
    NEXT_ID.with(|c| {
        let id = c.get();
        c.set(id + 1);
        id
    })
}

impl Pdf {
    pub fn from_pages(pages: Vec<PageRef>) -> Pdf {
        Pdf { pages, bytes: None }
    }

    pub fn from_document(doc: Document, name: String) -> Result<Pdf> {
        if doc.is_encrypted() {
            return Err(Error::new(format!(
                "{name}: pdf is encrypted, pass the password: open(path, password=\"...\")"
            )));
        }
        let ids: Vec<ObjectId> = doc.get_pages().into_values().collect();
        let src = Rc::new(SourceDoc { id: next_id(), name, doc });
        Ok(Pdf::from_pages(ids.into_iter().map(|page| PageRef { src: src.clone(), page, rotate: 0 }).collect()))
    }

    pub fn open(path: &str, password: Option<&str>) -> Result<Pdf> {
        let options = match password {
            Some(p) => lopdf::LoadOptions::with_password(p),
            None => lopdf::LoadOptions::default(),
        };
        let doc = Document::load_with_options(path, options).map_err(|e| match e {
            lopdf::Error::IO(io) => Error::new(format!("cannot open '{path}': {io}")),
            other => Error::new(format!("cannot read '{path}': {other}")),
        })?;
        Pdf::from_document(doc, path.to_string())
    }

    pub fn from_bytes(bytes: Vec<u8>, name: String) -> Result<Pdf> {
        let doc = Document::load_mem(&bytes)?;
        let mut pdf = Pdf::from_document(doc, name)?;
        pdf.bytes = Some(Rc::new(bytes));
        Ok(pdf)
    }

    /// `n` empty pages of the given size in points (A4 by default).
    pub fn blank(n: usize, width: f64, height: f64) -> Pdf {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let mut kids = Vec::new();
        for _ in 0..n {
            let content = doc.add_object(Stream::new(Dictionary::new(), Vec::new()));
            let mut page = Dictionary::new();
            page.set("Type", Object::Name(b"Page".to_vec()));
            page.set("Parent", Object::Reference(pages_id));
            page.set("MediaBox", vec![0.into(), 0.into(), Object::Real(width as f32), Object::Real(height as f32)]);
            page.set("Resources", Dictionary::new());
            page.set("Contents", Object::Reference(content));
            kids.push(Object::Reference(doc.add_object(page)));
        }
        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Count", n as i64);
        pages.set("Kids", kids);
        doc.objects.insert(pages_id, Object::Dictionary(pages));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(pages_id));
        let root = doc.add_object(catalog);
        doc.trailer.set("Root", Object::Reference(root));
        Pdf::from_document(doc, "<blank>".into()).expect("blank pdf")
    }

    pub fn len(&self) -> usize {
        self.pages.len()
    }

    /// Build a fresh standalone document containing exactly these pages.
    pub fn materialize(&self) -> Result<Document> {
        if self.pages.is_empty() {
            return Err(Error::new("cannot write a pdf with 0 pages"));
        }
        let mut out = Document::with_version("1.7");
        let pages_id = out.new_object_id();

        // One output page object per occurrence; references to a source page
        // from other objects (annotation /P, link /Dest) go to its first occurrence.
        let page_ids: Vec<ObjectId> = self.pages.iter().map(|_| out.new_object_id()).collect();
        let mut first: HashMap<(usize, ObjectId), ObjectId> = HashMap::new();
        for (p, id) in self.pages.iter().zip(&page_ids) {
            first.entry((p.src.id, p.page)).or_insert(*id);
        }
        let mut maps: HashMap<usize, HashMap<ObjectId, ObjectId>> = HashMap::new();
        let mut version = "1.4".to_string();

        for (p, new_id) in self.pages.iter().zip(&page_ids) {
            let src = &p.src.doc;
            if src.version > version {
                version = src.version.clone();
            }
            let page = src.get_dictionary(p.page)?;
            let map = maps.entry(p.src.id).or_default();
            let mut copier = Copier { src, out: &mut out, map, first: &first, src_id: p.src.id };

            let mut dict = Dictionary::new();
            for (k, v) in page.iter() {
                if matches!(k.as_slice(), b"Parent" | b"Thumb" | b"PieceInfo" | b"B" | b"StructParents" | b"Metadata") {
                    continue;
                }
                dict.set(k.clone(), copier.copy(v));
            }
            // Attributes that may be inherited from the page tree.
            for key in [&b"Resources"[..], b"MediaBox", b"CropBox"] {
                if !dict.has(key)
                    && let Some(v) = inherited(src, page, key)
                {
                    let v = copier.copy(&v);
                    dict.set(key.to_vec(), v);
                }
            }
            if !dict.has(b"MediaBox") {
                dict.set("MediaBox", vec![0.into(), 0.into(), 612.into(), 792.into()]);
            }
            if !dict.has(b"Resources") {
                dict.set("Resources", Dictionary::new());
            }
            let base_rot = inherited(src, page, b"Rotate").and_then(|o| o.as_i64().ok()).unwrap_or(0);
            let rot = (base_rot + p.rotate).rem_euclid(360);
            if rot != 0 {
                dict.set("Rotate", rot);
            } else {
                dict.remove(b"Rotate");
            }
            dict.set("Type", Object::Name(b"Page".to_vec()));
            dict.set("Parent", Object::Reference(pages_id));
            out.objects.insert(*new_id, Object::Dictionary(dict));
        }

        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Count", page_ids.len() as i64);
        pages.set("Kids", page_ids.iter().map(|id| Object::Reference(*id)).collect::<Vec<_>>());
        out.objects.insert(pages_id, Object::Dictionary(pages));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", Object::Reference(pages_id));
        let root = out.add_object(catalog);
        out.trailer.set("Root", Object::Reference(root));
        out.version = version;
        Ok(out)
    }

    /// Serialize to bytes (exact bytes are reused when available).
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        if let Some(b) = &self.bytes {
            return Ok(b.as_ref().clone());
        }
        let mut doc = self.materialize()?;
        doc.compress();
        write_doc(&mut doc)
    }

    pub fn save(&self, path: &str) -> Result<usize> {
        let bytes = self.to_bytes()?;
        std::fs::write(path, &bytes).map_err(|e| Error::new(format!("cannot write '{path}': {e}")))?;
        Ok(bytes.len())
    }

    /// (width, height) in points of page `i` as displayed (after rotation).
    pub fn page_size(&self, i: usize) -> (f64, f64) {
        let p = &self.pages[i];
        let doc = &p.src.doc;
        let Ok(page) = doc.get_dictionary(p.page) else { return (0.0, 0.0) };
        let mb = inherited(doc, page, b"CropBox")
            .or_else(|| inherited(doc, page, b"MediaBox"))
            .and_then(|o| match o {
                Object::Reference(id) => doc.get_object(id).ok().cloned(),
                o => Some(o),
            })
            .and_then(|o| o.as_array().ok().cloned())
            .unwrap_or_default();
        let num = |k: usize| mb.get(k).and_then(|o| o.as_float().ok()).unwrap_or(0.0) as f64;
        let (w, h) = ((num(2) - num(0)).abs(), (num(3) - num(1)).abs());
        let base_rot = inherited(doc, page, b"Rotate").and_then(|o| o.as_i64().ok()).unwrap_or(0);
        if (base_rot + p.rotate).rem_euclid(180) == 90 { (h, w) } else { (w, h) }
    }
}

pub fn write_doc(doc: &mut Document) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    doc.save_modern(&mut buf)?;
    Ok(buf)
}

/// Look up a page attribute, walking up /Parent for inheritable keys.
fn inherited(doc: &Document, page: &Dictionary, key: &[u8]) -> Option<Object> {
    let mut node = page;
    for _ in 0..64 {
        if let Ok(v) = node.get(key) {
            return Some(v.clone());
        }
        let parent = node.get(b"Parent").ok()?.as_reference().ok()?;
        node = doc.get_dictionary(parent).ok()?;
    }
    None
}

/// Deep-copies objects from one source document into the output, renumbering
/// references and never pulling in the source's page tree.
struct Copier<'a> {
    src: &'a Document,
    out: &'a mut Document,
    map: &'a mut HashMap<ObjectId, ObjectId>,
    first: &'a HashMap<(usize, ObjectId), ObjectId>,
    src_id: usize,
}

impl Copier<'_> {
    fn copy(&mut self, obj: &Object) -> Object {
        match obj {
            Object::Reference(id) => self.copy_ref(*id),
            Object::Array(a) => Object::Array(a.iter().map(|o| self.copy(o)).collect()),
            Object::Dictionary(d) => Object::Dictionary(self.copy_dict(d)),
            Object::Stream(s) => {
                let mut s2 = s.clone();
                s2.dict = self.copy_dict(&s.dict);
                Object::Stream(s2)
            }
            other => other.clone(),
        }
    }

    fn copy_dict(&mut self, d: &Dictionary) -> Dictionary {
        let mut out = Dictionary::new();
        for (k, v) in d.iter() {
            out.set(k.clone(), self.copy(v));
        }
        out
    }

    fn copy_ref(&mut self, id: ObjectId) -> Object {
        if let Some(n) = self.first.get(&(self.src_id, id)) {
            return Object::Reference(*n);
        }
        if let Some(n) = self.map.get(&id) {
            return Object::Reference(*n);
        }
        let Ok(obj) = self.src.get_object(id) else { return Object::Null };
        if let Object::Dictionary(d) = obj
            && let Ok(t) = d.get_type()
            && (t == b"Page" || t == b"Pages" || t == b"Catalog")
        {
            // A page that is not part of the output (or the source's page tree).
            return Object::Null;
        }
        let new_id = self.out.new_object_id();
        self.map.insert(id, new_id);
        let copied = self.copy(obj);
        self.out.objects.insert(new_id, copied);
        Object::Reference(new_id)
    }
}
