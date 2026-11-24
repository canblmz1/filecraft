use lopdf::{Dictionary, Document, LoadOptions, Object};

use crate::{Error, Result};

pub(crate) const MAX_BYTES: u64 = 32 * 1024 * 1024;

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
    if doc.is_encrypted()
        || doc.encryption_state.is_some()
        || doc.xref_section_count > 1
        || doc.linearization.is_some()
        || protected(&doc)
    {
        return Ok(result);
    }
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
    if !changed {
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
