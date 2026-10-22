// `pdf -- size`: shrink a pdf until its serialized size is <= the target.
//
// 1. Lossless: recompress streams, merge duplicate streams, drop unused objects,
//    and pack objects into object streams.
// 2. Lossy: re-encode images as JPEG along a ladder of (scale, quality) steps,
//    binary-searching for the mildest step that fits. Every step is measured on
//    the real serialized output, so the result is guaranteed to fit.

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};

use lopdf::{Document, Object, ObjectId};

use crate::error::{Error, Result};
use crate::pdf::{Pdf, write_doc};

/// From mildest to most aggressive: (scale factor, JPEG quality).
const LADDER: &[(f32, u8)] = &[
    (1.0, 95),
    (1.0, 90),
    (1.0, 85),
    (1.0, 75),
    (1.0, 65),
    (1.0, 55),
    (1.0, 45),
    (0.85, 45),
    (0.7, 45),
    (0.6, 40),
    (0.5, 40),
    (0.42, 35),
    (0.35, 35),
    (0.3, 30),
    (0.25, 30),
    (0.2, 25),
    (0.16, 25),
    (0.12, 20),
    (0.09, 20),
    (0.06, 15),
];

pub fn compress_to(pdf: &Pdf, target: u64) -> Result<Pdf> {
    let mut base = pdf.materialize()?;
    lossless(&mut base);
    let lossless_bytes = write_doc(&mut base.clone())?;
    if lossless_bytes.len() as u64 <= target {
        return Pdf::from_bytes(lossless_bytes, "<compressed>".into());
    }

    let images = decode_images(&base);
    if images.is_empty() {
        return Err(Error::new(format!(
            "cannot compress to {}: smallest achievable is {} (no images to downsample)",
            human(target),
            human(lossless_bytes.len() as u64)
        )));
    }

    let mut cache: HashMap<usize, Vec<u8>> = HashMap::new();
    let mut trial = |step: usize| -> Result<usize> {
        if let Some(b) = cache.get(&step) {
            return Ok(b.len());
        }
        let bytes = apply_step(&base, &images, LADDER[step])?;
        let n = bytes.len();
        cache.insert(step, bytes);
        Ok(n)
    };

    // Binary search for the mildest step that fits. Size falls (almost)
    // monotonically along the ladder; whatever we return was measured.
    let last = LADDER.len() - 1;
    if trial(last)? as u64 > target {
        let smallest = cache[&last].len().min(lossless_bytes.len());
        return Err(Error::new(format!(
            "cannot compress to {}: smallest achievable is {}",
            human(target),
            human(smallest as u64)
        )));
    }
    let (mut lo, mut hi) = (0, last);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if trial(mid)? as u64 <= target {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    let bytes = cache.remove(&hi).unwrap();
    Pdf::from_bytes(bytes, "<compressed>".into())
}

pub fn human(n: u64) -> String {
    let n = n as f64;
    if n >= 1024.0 * 1024.0 {
        format!("{:.2}mb", n / 1024.0 / 1024.0)
    } else if n >= 1024.0 {
        format!("{:.1}kb", n / 1024.0)
    } else {
        format!("{n}b")
    }
}

fn lossless(doc: &mut Document) {
    for obj in doc.objects.values_mut() {
        if let Object::Stream(s) = obj {
            s.dict.remove(b"Metadata");
            let plain_flate = matches!(s.filters().as_deref(), Ok([b"FlateDecode"])) && !s.dict.has(b"DecodeParms");
            if plain_flate {
                let mut recompressed = s.clone();
                if recompressed.decompress().is_ok() && recompressed.compress().is_ok() && recompressed.content.len() < s.content.len() {
                    *s = recompressed;
                }
            } else if s.allows_compression {
                let _ = s.compress();
            }
        }
    }
    dedupe_streams(doc);
    doc.prune_objects();
}

/// Point every reference to an identical stream at a single copy.
fn dedupe_streams(doc: &mut Document) {
    let mut seen: HashMap<u64, Vec<ObjectId>> = HashMap::new();
    let mut remap: HashMap<ObjectId, ObjectId> = HashMap::new();
    for (id, obj) in &doc.objects {
        let Object::Stream(s) = obj else { continue };
        let mut h = DefaultHasher::new();
        format!("{:?}", s.dict).hash(&mut h);
        s.content.hash(&mut h);
        let bucket = seen.entry(h.finish()).or_default();
        let dup = bucket.iter().find(|other| match &doc.objects[*other] {
            Object::Stream(o) => o.content == s.content && format!("{:?}", o.dict) == format!("{:?}", s.dict),
            _ => false,
        });
        match dup {
            Some(keep) => {
                remap.insert(*id, *keep);
            }
            None => bucket.push(*id),
        }
    }
    if remap.is_empty() {
        return;
    }
    fn rewrite(o: &mut Object, remap: &HashMap<ObjectId, ObjectId>) {
        match o {
            Object::Reference(id) => {
                if let Some(n) = remap.get(id) {
                    *id = *n;
                }
            }
            Object::Array(a) => a.iter_mut().for_each(|x| rewrite(x, remap)),
            Object::Dictionary(d) => d.iter_mut().for_each(|(_, x)| rewrite(x, remap)),
            Object::Stream(s) => s.dict.iter_mut().for_each(|(_, x)| rewrite(x, remap)),
            _ => {}
        }
    }
    for obj in doc.objects.values_mut() {
        rewrite(obj, &remap);
    }
    for id in remap.keys() {
        doc.objects.remove(id);
    }
}

struct Image {
    id: ObjectId,
    width: usize,
    height: usize,
    /// 1 (gray) or 3 (rgb).
    comps: usize,
    pixels: Vec<u8>,
    encoded_len: usize,
}

fn decode_images(doc: &Document) -> Vec<Image> {
    // Soft masks and stencil masks stay lossless.
    let mut masks: HashSet<ObjectId> = HashSet::new();
    for obj in doc.objects.values() {
        if let Object::Stream(s) = obj {
            for key in [&b"SMask"[..], b"Mask"] {
                if let Ok(Object::Reference(id)) = s.dict.get(key) {
                    masks.insert(*id);
                }
            }
        }
    }
    let mut out = Vec::new();
    for (id, obj) in &doc.objects {
        let Object::Stream(s) = obj else { continue };
        let d = &s.dict;
        if masks.contains(id)
            || !matches!(d.get(b"Subtype").and_then(|o| o.as_name()), Ok(b"Image"))
            || d.get(b"ImageMask").and_then(|o| o.as_bool()).unwrap_or(false)
        {
            continue;
        }
        let num = |k: &[u8]| d.get(k).and_then(|o| o.as_i64()).unwrap_or(0) as usize;
        let (w, h) = (num(b"Width"), num(b"Height"));
        if w * h < 64 * 64 || s.content.len() < 4096 {
            continue;
        }
        let Some(comps) = color_components(doc, d.get(b"ColorSpace").ok()) else { continue };
        let filters: Vec<Vec<u8>> = s.filters().map(|f| f.iter().map(|x| x.to_vec()).collect()).unwrap_or_default();
        let pixels = if filters == [b"DCTDecode".to_vec()] {
            let mut dec = jpeg_decoder::Decoder::new(s.content.as_slice());
            let Ok(px) = dec.decode() else { continue };
            let Some(info) = dec.info() else { continue };
            let got = match info.pixel_format {
                jpeg_decoder::PixelFormat::L8 => 1,
                jpeg_decoder::PixelFormat::RGB24 => 3,
                _ => continue,
            };
            if got != comps || info.width as usize != w || info.height as usize != h {
                continue;
            }
            px
        } else if filters.iter().all(|f| f == b"FlateDecode" || f == b"LZWDecode") {
            if num(b"BitsPerComponent") != 8 {
                continue;
            }
            let Ok(px) = s.decompressed_content() else { continue };
            if px.len() < w * h * comps {
                continue;
            }
            px[..w * h * comps].to_vec()
        } else {
            continue;
        };
        out.push(Image { id: *id, width: w, height: h, comps, pixels, encoded_len: s.content.len() });
    }
    out
}

/// Component count for colour spaces we can safely re-encode as baseline JPEG.
fn color_components(doc: &Document, cs: Option<&Object>) -> Option<usize> {
    let cs = match cs? {
        Object::Reference(id) => doc.get_object(*id).ok()?,
        o => o,
    };
    match cs {
        Object::Name(n) if n == b"DeviceRGB" || n == b"CalRGB" => Some(3),
        Object::Name(n) if n == b"DeviceGray" || n == b"CalGray" => Some(1),
        Object::Array(a) => match a.first()?.as_name().ok()? {
            b"ICCBased" => {
                let stream = doc.get_object(a.get(1)?.as_reference().ok()?).ok()?.as_stream().ok()?;
                match stream.dict.get(b"N").ok()?.as_i64().ok()? {
                    1 => Some(1),
                    3 => Some(3),
                    _ => None,
                }
            }
            b"CalRGB" => Some(3),
            b"CalGray" => Some(1),
            _ => None,
        },
        _ => None,
    }
}

fn apply_step(base: &Document, images: &[Image], (scale, quality): (f32, u8)) -> Result<Vec<u8>> {
    let mut doc = base.clone();
    for img in images {
        let (w, h, px) = if scale < 1.0 {
            let nw = ((img.width as f32 * scale).round() as usize).max(1);
            let nh = ((img.height as f32 * scale).round() as usize).max(1);
            (nw, nh, downscale(&img.pixels, img.width, img.height, img.comps, nw, nh))
        } else {
            (img.width, img.height, img.pixels.clone())
        };
        if w > u16::MAX as usize || h > u16::MAX as usize {
            continue;
        }
        let mut jpeg = Vec::new();
        let color = if img.comps == 1 { jpeg_encoder::ColorType::Luma } else { jpeg_encoder::ColorType::Rgb };
        if jpeg_encoder::Encoder::new(&mut jpeg, quality).encode(&px, w as u16, h as u16, color).is_err() {
            continue;
        }
        if jpeg.len() >= img.encoded_len {
            continue;
        }
        if let Some(Object::Stream(s)) = doc.objects.get_mut(&img.id) {
            s.dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
            s.dict.remove(b"DecodeParms");
            s.dict.set("Width", w as i64);
            s.dict.set("Height", h as i64);
            s.dict.set("BitsPerComponent", 8);
            s.set_content(jpeg);
            s.allows_compression = false;
        }
    }
    write_doc(&mut doc)
}

/// Box-filter downscale (averages every source pixel that falls in each target pixel).
fn downscale(src: &[u8], w: usize, h: usize, comps: usize, nw: usize, nh: usize) -> Vec<u8> {
    let mut out = vec![0u8; nw * nh * comps];
    for y in 0..nh {
        let y0 = y * h / nh;
        let y1 = ((y + 1) * h / nh).max(y0 + 1).min(h);
        for x in 0..nw {
            let x0 = x * w / nw;
            let x1 = ((x + 1) * w / nw).max(x0 + 1).min(w);
            let mut acc = [0u32; 3];
            for sy in y0..y1 {
                let row = &src[(sy * w + x0) * comps..(sy * w + x1) * comps];
                for p in row.chunks_exact(comps) {
                    for c in 0..comps {
                        acc[c] += p[c] as u32;
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            for c in 0..comps {
                out[(y * nw + x) * comps + c] = ((acc[c] + n / 2) / n) as u8;
            }
        }
    }
    out
}
