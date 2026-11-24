use std::{
    collections::{BTreeMap, HashMap},
    io::Cursor,
};

use image::{
    ExtendedColorType, ImageDecoder, ImageEncoder,
    codecs::jpeg::{JpegDecoder, JpegEncoder},
    imageops::FilterType,
};
use lopdf::{Dictionary, Document, LoadOptions, Object, ObjectId, Stream, content::Content};
use serde::Serialize;

use crate::{Error, Result};

pub(crate) const MAX_BYTES: u64 = 100 * 1024 * 1024;
/// Largest decoded page image or stream compared during verification.
const MAX_STREAM_BYTES: usize = 64 * 1024 * 1024;
const MAX_CONTENT_BYTES: usize = 16 * 1024 * 1024;
/// Small images such as logos and signatures are left alone.
const MIN_IMAGE_PIXELS: u64 = 160_000;
const MAX_FORM_DEPTH: u8 = 4;

/// A lossy PDF candidate: page images above `dpi` are downsampled and every
/// eligible page image is stored as JPEG at `quality`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Level {
    pub dpi: f64,
    pub quality: u8,
}

pub(crate) const LEVELS: [Level; 3] = [
    Level {
        dpi: 200.0,
        quality: 82,
    },
    Level {
        dpi: 150.0,
        quality: 72,
    },
    Level {
        dpi: 120.0,
        quality: 62,
    },
];
/// Lowest RGB PSNR accepted for a re-encoded page image unless the recipe
/// sets its own floor. Images that fall below it keep their original bytes.
pub(crate) const MIN_PSNR: u8 = 30;

#[derive(Debug, Clone, Serialize)]
pub struct ImageSummary {
    pub images_reencoded: usize,
    pub max_dpi: u32,
    pub jpeg_quality: u8,
    pub min_psnr_db: f64,
}

impl ImageSummary {
    pub(crate) fn penalty(&self) -> f64 {
        f64::from(100 - self.jpeg_quality) + (200.0 / f64::from(self.max_dpi.max(1)) - 1.0) * 50.0
    }
}

pub(crate) struct PdfResult {
    pub pages: usize,
    pub output: Option<Vec<u8>>,
}

fn load(bytes: &[u8]) -> Result<Document> {
    Document::load_mem_with_options(
        bytes,
        LoadOptions {
            strict: true,
            max_decompressed_size: Some(16 * 1024 * 1024),
            ..Default::default()
        },
    )
    .map_err(|error| Error::Invalid(format!("PDF could not be parsed strictly: {error}")))
}

fn protected(doc: &Document) -> bool {
    let mut pending: Vec<_> = doc.objects.values().collect();
    while let Some(object) = pending.pop() {
        match object {
            Object::Dictionary(dict) | Object::Stream(lopdf::Stream { dict, .. }) => {
                if dict.has(b"ByteRange")
                    || dict.has(b"Perms")
                    || [b"Type".as_slice(), b"FT"].into_iter().any(|key| {
                        dict.get(key)
                            .and_then(Object::as_name)
                            .is_ok_and(|name| name == b"Sig" || name == b"DocTimeStamp")
                    })
                {
                    return true;
                }
                pending.extend(dict.iter().map(|(_, value)| value));
            }
            Object::Array(items) => pending.extend(items),
            _ => {}
        }
    }
    false
}

/// Documents FileCraft may rewrite. Others are only ever copied.
fn rewritable(doc: &Document) -> bool {
    !(doc.is_encrypted()
        || doc.encryption_state.is_some()
        || doc.xref_section_count > 1
        || doc.linearization.is_some()
        || protected(doc))
}

fn trailer(dict: &Dictionary) -> Dictionary {
    let mut trailer = dict.clone();
    for field in [
        b"Size".as_slice(),
        b"Prev",
        b"XRefStm",
        b"Type",
        b"W",
        b"Index",
        b"Length",
        b"Filter",
        b"DecodeParms",
    ] {
        trailer.remove(field);
    }
    trailer
}

fn objects(doc: &Document) -> impl Iterator<Item = (&lopdf::ObjectId, &Object)> {
    doc.objects
        .iter()
        .filter(|(_, object)| !matches!(object.type_name(), Ok(b"XRef" | b"ObjStm")))
}

/// Flate-compress unfiltered streams and confirm each decodes to its old bytes.
fn compress_streams(doc: &mut Document, check: &impl Fn() -> Result<()>) -> Result<bool> {
    let mut changed = false;
    for object in doc.objects.values_mut() {
        check()?;
        let Object::Stream(stream) = object else {
            continue;
        };
        if !stream.allows_compression
            || stream.dict.has(b"Filter")
            || stream.dict.has(b"DecodeParms")
            || stream.dict.has(b"F")
            || matches!(
                stream.dict.get(b"Type").and_then(Object::as_name),
                Ok(b"XRef" | b"ObjStm")
            )
        {
            continue;
        }
        let original = stream.content.clone();
        stream
            .compress()
            .map_err(|error| Error::Goal(format!("PDF compression failed: {error}")))?;
        if stream.dict.has(b"Filter") {
            let decoded = stream
                .decompressed_content_with_limit(original.len())
                .map_err(|error| {
                    Error::Goal(format!("PDF stream could not be verified: {error}"))
                })?;
            if decoded != original {
                return Err(Error::Goal(
                    "Compressed PDF stream changed its decoded bytes".into(),
                ));
            }
            changed = true;
        }
    }
    Ok(changed)
}

pub(crate) fn optimize(bytes: &[u8], check: impl Fn() -> Result<()>) -> Result<PdfResult> {
    let mut doc = load(bytes)?;
    check()?;
    let page_order = doc.get_pages();
    let mut result = PdfResult {
        pages: page_order.len(),
        output: None,
    };
    if page_order.is_empty() {
        return Err(Error::Invalid("PDF has no readable pages".into()));
    }
    if !rewritable(&doc) || !compress_streams(&mut doc, &check)? {
        return Ok(result);
    }
    let original_trailer = trailer(&doc.trailer);
    let mut candidate = Vec::new();
    doc.save_to(&mut candidate)?;
    check()?;
    if candidate.len() < bytes.len() {
        let reloaded = load(&candidate)?;
        if page_order == reloaded.get_pages()
            && original_trailer == trailer(&reloaded.trailer)
            && objects(&doc).eq(objects(&reloaded))
        {
            result.output = Some(candidate);
        }
    }
    Ok(result)
}

/// A verified lossy candidate and what changed in it.
pub(crate) struct Recompressed {
    pub output: Vec<u8>,
    pub summary: ImageSummary,
}

#[derive(Clone, Copy, PartialEq)]
enum Encoding {
    Raw,
    Dct,
}

#[derive(Clone, Copy)]
struct PageImage {
    width: u32,
    height: u32,
    channels: u8,
    encoding: Encoding,
}

type Matrix = [f64; 6];
const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

fn multiply(m: Matrix, n: Matrix) -> Matrix {
    [
        m[0] * n[0] + m[1] * n[2],
        m[0] * n[1] + m[1] * n[3],
        m[2] * n[0] + m[3] * n[2],
        m[2] * n[1] + m[3] * n[3],
        m[4] * n[0] + m[5] * n[2] + n[4],
        m[4] * n[1] + m[5] * n[3] + n[5],
    ]
}

fn number(object: &Object) -> Option<f64> {
    object
        .as_float()
        .ok()
        .map(f64::from)
        .filter(|value| value.is_finite())
}

fn matrix(objects: &[Object]) -> Option<Matrix> {
    let values: Vec<f64> = objects.iter().map(number).collect::<Option<_>>()?;
    values.try_into().ok()
}

fn resolve<'a>(doc: &'a Document, object: &'a Object) -> Option<&'a Object> {
    doc.dereference(object).ok().map(|(_, object)| object)
}

fn channels(doc: &Document, space: &Object) -> Option<u8> {
    match resolve(doc, space)? {
        Object::Name(name) if name == b"DeviceGray" => Some(1),
        Object::Name(name) if name == b"DeviceRGB" => Some(3),
        Object::Array(items) if items.len() == 2 && items[0].as_name().ok()? == b"ICCBased" => {
            let profile = resolve(doc, &items[1])?.as_stream().ok()?;
            match profile.dict.get(b"N").ok()?.as_i64().ok()? {
                1 => Some(1),
                3 => Some(3),
                _ => None,
            }
        }
        _ => None,
    }
}

fn page_image(doc: &Document, stream: &Stream) -> Option<PageImage> {
    let dict = &stream.dict;
    if dict.get(b"Subtype").and_then(Object::as_name).ok()? != b"Image"
        || [
            b"ImageMask".as_slice(),
            b"Mask",
            b"Decode",
            b"SMaskInData",
            b"Alternates",
            b"F",
            b"JBIG2Globals",
        ]
        .into_iter()
        .any(|key| dict.has(key))
        || dict.get(b"BitsPerComponent").ok()?.as_i64().ok()? != 8
    {
        return None;
    }
    let width = u32::try_from(dict.get(b"Width").ok()?.as_i64().ok()?).ok()?;
    let height = u32::try_from(dict.get(b"Height").ok()?.as_i64().ok()?).ok()?;
    if width == 0
        || height == 0
        || u64::from(width) * u64::from(height) < MIN_IMAGE_PIXELS
        || crate::inspect::check_pixels(width, height).is_err()
    {
        return None;
    }
    let channels = channels(doc, dict.get(b"ColorSpace").ok()?)?;
    let filters = stream.filters().unwrap_or_default();
    let encoding = match filters.as_slice() {
        [] | [b"FlateDecode"] | [b"LZWDecode"] => Encoding::Raw,
        [b"DCTDecode"] if !dict.has(b"DecodeParms") => Encoding::Dct,
        _ => return None,
    };
    Some(PageImage {
        width,
        height,
        channels,
        encoding,
    })
}

struct Walker<'a, C> {
    doc: &'a Document,
    check: &'a C,
    /// Lowest placement resolution in pixels per inch for each drawn image.
    dpi: HashMap<ObjectId, f64>,
}

impl<C: Fn() -> Result<()>> Walker<'_, C> {
    fn xobject(&self, resources: Option<&Dictionary>, name: &[u8]) -> Option<ObjectId> {
        let xobjects = resolve(self.doc, resources?.get(b"XObject").ok()?)?
            .as_dict()
            .ok()?;
        xobjects.get(name).ok()?.as_reference().ok()
    }

    fn walk(
        &mut self,
        content: &[u8],
        resources: Option<&Dictionary>,
        start: Matrix,
        depth: u8,
    ) -> Result<()> {
        (self.check)()?;
        let Ok(content) = Content::decode(content) else {
            return Ok(());
        };
        let mut ctm = start;
        let mut saved = Vec::new();
        for operation in &content.operations {
            match operation.operator.as_str() {
                "q" => saved.push(ctm),
                "Q" => ctm = saved.pop().unwrap_or(start),
                "cm" => {
                    if let Some(m) = matrix(&operation.operands) {
                        ctm = multiply(m, ctm);
                    }
                }
                "Do" => {
                    let Some(name) = operation.operands.first().and_then(|o| o.as_name().ok())
                    else {
                        continue;
                    };
                    let Some(id) = self.xobject(resources, name) else {
                        continue;
                    };
                    let Ok(stream) = self.doc.get_object(id).and_then(Object::as_stream) else {
                        continue;
                    };
                    match stream.dict.get(b"Subtype").and_then(Object::as_name) {
                        Ok(b"Image") => self.place(id, stream, ctm),
                        Ok(b"Form") if depth < MAX_FORM_DEPTH => {
                            let Ok(form) =
                                stream.decompressed_content_with_limit(MAX_CONTENT_BYTES)
                            else {
                                continue;
                            };
                            let form_matrix = stream
                                .dict
                                .get(b"Matrix")
                                .ok()
                                .and_then(|m| m.as_array().ok())
                                .and_then(|m| matrix(m))
                                .unwrap_or(IDENTITY);
                            let form_resources = stream
                                .dict
                                .get(b"Resources")
                                .ok()
                                .and_then(|r| resolve(self.doc, r))
                                .and_then(|r| r.as_dict().ok())
                                .or(resources);
                            self.walk(
                                &form,
                                form_resources,
                                multiply(form_matrix, ctm),
                                depth + 1,
                            )?;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn place(&mut self, id: ObjectId, stream: &Stream, ctm: Matrix) {
        let (Some(width), Some(height)) = (
            stream.dict.get(b"Width").ok().and_then(number),
            stream.dict.get(b"Height").ok().and_then(number),
        ) else {
            return;
        };
        let drawn_width = ctm[0].hypot(ctm[1]) / 72.0;
        let drawn_height = ctm[2].hypot(ctm[3]) / 72.0;
        if drawn_width <= 0.0 || drawn_height <= 0.0 {
            return;
        }
        let dpi = (width / drawn_width).min(height / drawn_height);
        let entry = self.dpi.entry(id).or_insert(dpi);
        *entry = entry.min(dpi);
    }
}

fn placements(doc: &Document, check: &impl Fn() -> Result<()>) -> Result<HashMap<ObjectId, f64>> {
    let mut walker = Walker {
        doc,
        check,
        dpi: HashMap::new(),
    };
    for page in doc.get_pages().into_values() {
        let content = doc
            .get_page_content_with_limit(page, MAX_CONTENT_BYTES)
            .map_err(|error| {
                Error::Unsupported(format!("PDF page content is too large: {error}"))
            })?;
        let (inline, referenced) = doc
            .get_page_resources(page)
            .map_err(|error| Error::Invalid(format!("PDF page resources are invalid: {error}")))?;
        let resources = inline.or_else(|| {
            referenced
                .first()
                .and_then(|id| doc.get_dictionary(*id).ok())
        });
        walker.walk(&content, resources, IDENTITY, 0)?;
    }
    Ok(walker.dpi)
}

fn pixels(stream: &Stream, image: PageImage) -> Option<Vec<u8>> {
    let expected = image.width as usize * image.height as usize * usize::from(image.channels);
    if expected > MAX_STREAM_BYTES {
        return None;
    }
    match image.encoding {
        Encoding::Dct => {
            let decoded = jpeg(&stream.content)?;
            (decoded.0 == (image.width, image.height) && decoded.1 == image.channels)
                .then_some(decoded.2)
        }
        Encoding::Raw => {
            let mut bytes = stream
                .decompressed_content_with_limit(MAX_STREAM_BYTES)
                .ok()?;
            if bytes.len() < expected {
                return None;
            }
            bytes.truncate(expected);
            Some(bytes)
        }
    }
}

/// Decode a baseline or progressive JPEG to 8-bit gray or RGB samples.
fn jpeg(bytes: &[u8]) -> Option<((u32, u32), u8, Vec<u8>)> {
    let mut decoder = JpegDecoder::new(Cursor::new(bytes)).ok()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_STREAM_BYTES as u64);
    decoder.set_limits(limits).ok()?;
    let channels = match decoder.color_type() {
        image::ColorType::L8 => 1,
        image::ColorType::Rgb8 => 3,
        _ => return None,
    };
    let dimensions = decoder.dimensions();
    let mut buffer = vec![0; usize::try_from(decoder.total_bytes()).ok()?];
    decoder.read_image(&mut buffer).ok()?;
    Some((dimensions, channels, buffer))
}

fn resize(samples: Vec<u8>, image: PageImage, width: u32, height: u32) -> Option<Vec<u8>> {
    if (width, height) == (image.width, image.height) {
        return Some(samples);
    }
    Some(if image.channels == 1 {
        image::imageops::resize(
            &image::GrayImage::from_raw(image.width, image.height, samples)?,
            width,
            height,
            FilterType::Triangle,
        )
        .into_raw()
    } else {
        image::imageops::resize(
            &image::RgbImage::from_raw(image.width, image.height, samples)?,
            width,
            height,
            FilterType::Triangle,
        )
        .into_raw()
    })
}

fn psnr(expected: &[u8], observed: &[u8]) -> f64 {
    let squared: u64 = expected
        .iter()
        .zip(observed)
        .map(|(a, b)| {
            let delta = i64::from(*a) - i64::from(*b);
            (delta * delta) as u64
        })
        .sum();
    if squared == 0 {
        return f64::INFINITY;
    }
    let mse = squared as f64 / expected.len() as f64;
    10.0 * (65_025.0 / mse).log10()
}

struct Planned {
    width: u32,
    height: u32,
    channels: u8,
}

/// Re-encode one page image. Returns `None` when the image should keep its bytes.
fn reencode(
    stream: &Stream,
    image: PageImage,
    dpi: f64,
    level: Level,
    floor: f64,
) -> Option<(Stream, Planned, f64)> {
    let scale = (level.dpi / dpi).min(1.0);
    let (width, height) = if scale < 0.98 {
        (
            ((f64::from(image.width) * scale).round() as u32).max(1),
            ((f64::from(image.height) * scale).round() as u32).max(1),
        )
    } else {
        (image.width, image.height)
    };
    let unchanged_size = (width, height) == (image.width, image.height);
    let reference = resize(pixels(stream, image)?, image, width, height)?;
    let mut output = Vec::new();
    let color = if image.channels == 1 {
        ExtendedColorType::L8
    } else {
        ExtendedColorType::Rgb8
    };
    JpegEncoder::new_with_quality(&mut output, level.quality)
        .write_image(&reference, width, height, color)
        .ok()?;
    let limit = if image.encoding == Encoding::Dct && unchanged_size {
        stream.content.len() * 85 / 100
    } else {
        stream.content.len()
    };
    if output.len() >= limit {
        return None;
    }
    let (dimensions, channels, observed) = jpeg(&output)?;
    if dimensions != (width, height) || channels != image.channels {
        return None;
    }
    let quality = psnr(&reference, &observed);
    if quality < floor {
        return None;
    }
    let mut dict = stream.dict.clone();
    dict.set("Width", i64::from(width));
    dict.set("Height", i64::from(height));
    dict.set("BitsPerComponent", 8_i64);
    dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
    dict.remove(b"DecodeParms");
    let mut replacement = Stream::new(dict, output);
    replacement.allows_compression = false;
    Some((
        replacement,
        Planned {
            width,
            height,
            channels,
        },
        quality,
    ))
}

const REWRITTEN_KEYS: [&[u8]; 6] = [
    b"Width",
    b"Height",
    b"Filter",
    b"DecodeParms",
    b"Length",
    b"BitsPerComponent",
];

fn without(dict: &Dictionary, keys: &[&[u8]]) -> Dictionary {
    let mut dict = dict.clone();
    for key in keys {
        dict.remove(key);
    }
    dict
}

/// Confirm a lossy candidate against the original document: same pages and
/// trailer, every untouched object identical after decoding, and every
/// re-encoded image a JPEG of the planned size.
fn verify(
    original: &Document,
    candidate: &[u8],
    replaced: &HashMap<ObjectId, Planned>,
    check: &impl Fn() -> Result<()>,
) -> Result<()> {
    let reloaded = load(candidate)?;
    let failed = |reason: &str| {
        Err(Error::Goal(format!(
            "PDF candidate failed verification: {reason}"
        )))
    };
    if original.get_pages() != reloaded.get_pages() {
        return failed("page order changed");
    }
    if trailer(&original.trailer) != trailer(&reloaded.trailer) {
        return failed("trailer changed");
    }
    let before: BTreeMap<_, _> = objects(original).collect();
    let after: BTreeMap<_, _> = objects(&reloaded).collect();
    if !before.keys().eq(after.keys()) {
        return failed("object set changed");
    }
    for (id, old) in before {
        check()?;
        let new = after[id];
        if let Some(planned) = replaced.get(id) {
            let (Ok(old), Ok(new)) = (old.as_stream(), new.as_stream()) else {
                return failed("re-encoded image is not a stream");
            };
            let decoded = jpeg(&new.content);
            if without(&old.dict, &REWRITTEN_KEYS) != without(&new.dict, &REWRITTEN_KEYS)
                || new.filters().unwrap_or_default() != [b"DCTDecode".as_slice()]
                || !decoded.is_some_and(|(dimensions, channels, _)| {
                    dimensions == (planned.width, planned.height) && channels == planned.channels
                })
                || new.dict.get(b"Width").and_then(Object::as_i64).ok()
                    != Some(i64::from(planned.width))
                || new.dict.get(b"Height").and_then(Object::as_i64).ok()
                    != Some(i64::from(planned.height))
            {
                return failed("a re-encoded image does not match its plan");
            }
            continue;
        }
        if old == new {
            continue;
        }
        match (old.as_stream(), new.as_stream()) {
            (Ok(old), Ok(new)) => {
                let keys: [&[u8]; 3] = [b"Filter", b"DecodeParms", b"Length"];
                if without(&old.dict, &keys) != without(&new.dict, &keys) {
                    return failed("a stream dictionary changed");
                }
                if old.content == new.content && old.dict == new.dict {
                    continue;
                }
                let decoded = |stream: &Stream| {
                    stream
                        .decompressed_content_with_limit(MAX_STREAM_BYTES)
                        .ok()
                };
                match (decoded(old), decoded(new)) {
                    (Some(a), Some(b)) if a == b => {}
                    _ => return failed("an untouched stream decodes differently"),
                }
            }
            _ => return failed("an untouched object changed"),
        }
    }
    Ok(())
}

/// Build one lossy candidate. Returns `None` when the document cannot be
/// rewritten or no page image benefits at this level.
pub(crate) fn recompress(
    bytes: &[u8],
    level: Level,
    floor: u8,
    check: &impl Fn() -> Result<()>,
) -> Result<Option<Recompressed>> {
    let original = load(bytes)?;
    if original.get_pages().is_empty() || !rewritable(&original) {
        return Ok(None);
    }
    let placed = placements(&original, check)?;
    let mut doc = original.clone();
    let mut replaced = HashMap::new();
    let mut lowest = f64::INFINITY;
    let mut ids: Vec<_> = placed.into_iter().collect();
    ids.sort_by_key(|(id, _)| *id);
    for (id, dpi) in ids {
        check()?;
        let Some(Ok(stream)) = original.objects.get(&id).map(Object::as_stream) else {
            continue;
        };
        let Some(image) = page_image(&original, stream) else {
            continue;
        };
        let Some((replacement, planned, quality)) =
            reencode(stream, image, dpi, level, f64::from(floor))
        else {
            continue;
        };
        lowest = lowest.min(quality);
        doc.objects.insert(id, Object::Stream(replacement));
        replaced.insert(id, planned);
    }
    if replaced.is_empty() {
        return Ok(None);
    }
    compress_streams(&mut doc, check)?;
    let mut output = Vec::new();
    doc.save_to(&mut output)?;
    check()?;
    if output.len() >= bytes.len() {
        return Ok(None);
    }
    verify(&original, &output, &replaced, check)?;
    Ok(Some(Recompressed {
        output,
        summary: ImageSummary {
            images_reencoded: replaced.len(),
            max_dpi: level.dpi as u32,
            jpeg_quality: level.quality,
            min_psnr_db: (lowest.min(100.0) * 100.0).round() / 100.0,
        },
    }))
}
