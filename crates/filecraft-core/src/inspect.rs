use std::{
    fs::File,
    io::{BufReader, Cursor, Read, Seek},
    path::Path,
};

use image::{
    DynamicImage, ImageDecoder, ImageReader, Limits,
    codecs::{png::PngDecoder, webp::WebPDecoder},
    metadata::Orientation,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    Error, Format, Result,
    recipe::{MAX_IMAGE_BYTES, MAX_PIXELS},
};

#[derive(Debug, Clone, Serialize)]
pub struct Inspection {
    pub name: String,
    pub bytes: u64,
    pub mime: Option<&'static str>,
    pub format: Option<Format>,
    pub raster_format: Option<Format>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub alpha_channel: Option<bool>,
    pub exif: Option<bool>,
    pub icc: Option<bool>,
    pub animated: Option<bool>,
    pub sha256: String,
    pub warnings: Vec<String>,
}

pub(crate) fn kind(header: &[u8]) -> (Option<&'static str>, Option<Format>, &[&str]) {
    if header.starts_with(b"\x89PNG\r\n\x1a\n") {
        (
            Some(Format::Png.mime()),
            Some(Format::Png),
            &["png", "apng"],
        )
    } else if header.starts_with(b"\xff\xd8\xff") {
        (
            Some(Format::Jpeg.mime()),
            Some(Format::Jpeg),
            &["jpg", "jpeg", "jfif", "jpe"],
        )
    } else if header.starts_with(b"RIFF") && header.get(8..12) == Some(b"WEBP") {
        (Some(Format::Webp.mime()), Some(Format::Webp), &["webp"])
    } else if header.starts_with(b"%PDF-") {
        (Some(Format::Pdf.mime()), Some(Format::Pdf), &["pdf"])
    } else if header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a") {
        (Some("image/gif"), None, &["gif"])
    } else if header.starts_with(b"PK\x03\x04") || header.starts_with(b"PK\x05\x06") {
        (
            Some("application/zip"),
            None,
            &[
                "zip", "docx", "xlsx", "pptx", "odt", "ods", "odp", "epub", "jar",
            ],
        )
    } else if header.starts_with(b"fLaC") {
        (Some("audio/flac"), None, &["flac"])
    } else if header.starts_with(b"RIFF") && header.get(8..12) == Some(b"WAVE") {
        (Some("audio/wav"), None, &["wav", "wave"])
    } else if header.get(4..8) == Some(b"ftyp") && is_heic(header) {
        (
            Some(Format::Heic.mime()),
            Some(Format::Heic),
            &["heic", "heif", "hif"],
        )
    } else if header.get(4..8) == Some(b"ftyp") && header.get(8..12) == Some(b"qt  ") {
        (Some(Format::Mov.mime()), Some(Format::Mov), &["mov", "qt"])
    } else if header.get(4..8) == Some(b"ftyp")
        && matches!(
            header.get(8..12),
            Some(
                b"isom"
                    | b"iso2"
                    | b"iso4"
                    | b"iso5"
                    | b"iso6"
                    | b"mp42"
                    | b"mp41"
                    | b"avc1"
                    | b"M4V "
            )
        )
    {
        (Some(Format::Mp4.mime()), Some(Format::Mp4), &["mp4", "m4v"])
    } else {
        (None, None, &[])
    }
}

/// HEIF stills coded with HEVC. AVIF and other HEIF brands are not claimed.
fn is_heic(header: &[u8]) -> bool {
    const HEVC: [&[u8]; 6] = [b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevx"];
    let Some(major) = header.get(8..12) else {
        return false;
    };
    let size = header.get(0..4).map_or(0, |bytes| {
        u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize
    });
    let (compatible, _) = header
        .get(16..size.clamp(16, header.len()))
        .unwrap_or_default()
        .as_chunks::<4>();
    if compatible.contains(b"avif") || compatible.contains(b"avis") {
        return false;
    }
    HEVC.contains(&major)
        || (matches!(major, b"mif1" | b"msf1")
            && compatible
                .iter()
                .any(|brand| HEVC.contains(&brand.as_slice())))
}

/// Iterate ISO BMFF boxes as (type, payload).
fn boxes(mut data: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    std::iter::from_fn(move || {
        let size = u32::from_be_bytes(data.get(0..4)?.try_into().ok()?) as usize;
        let kind: [u8; 4] = data.get(4..8)?.try_into().ok()?;
        let (header, size) = match size {
            0 => (8, data.len()),
            1 => (
                16,
                usize::try_from(u64::from_be_bytes(data.get(8..16)?.try_into().ok()?)).ok()?,
            ),
            size => (8, size),
        };
        let payload = data.get(header..size)?;
        data = &data[size..];
        Some((kind, payload))
    })
}

/// Metadata items stored in a HEIF file, such as EXIF with GPS positions.
pub(crate) fn heif_metadata(bytes: &[u8]) -> Vec<String> {
    let mut found = Vec::new();
    let Some((_, meta)) = boxes(bytes).find(|(kind, _)| kind == b"meta") else {
        return found;
    };
    let Some((_, iinf)) =
        boxes(meta.get(4..).unwrap_or_default()).find(|(kind, _)| kind == b"iinf")
    else {
        return found;
    };
    let entries = match iinf.first() {
        Some(0) => iinf.get(6..),
        Some(_) => iinf.get(8..),
        None => None,
    };
    for (kind, infe) in boxes(entries.unwrap_or_default()) {
        let version = infe.first().copied().unwrap_or(0);
        if kind != *b"infe" || version < 2 {
            continue;
        }
        let id = if version == 2 { 2 } else { 4 };
        let Some(item) = infe.get(4 + id + 2..4 + id + 6) else {
            continue;
        };
        let rest = &infe[4 + id + 6..];
        let content_type = rest.splitn(3, |byte| *byte == 0).nth(1).unwrap_or_default();
        let label = match item {
            b"Exif" => "EXIF",
            b"mime" if content_type == b"application/rdf+xml" => "XMP",
            _ => continue,
        };
        if !found.iter().any(|known| known == label) {
            found.push(label.to_owned());
        }
    }
    found
}

pub(crate) fn open_regular(path: &Path) -> Result<File> {
    if !path.metadata()?.is_file() {
        return Err(Error::Invalid("Choose a regular file".into()));
    }
    let file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(Error::Invalid("Choose a regular file".into()));
    }
    Ok(file)
}

pub(crate) fn name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn inspect(path: &Path) -> Result<Inspection> {
    let file = open_regular(path)?;
    let mut reader = BufReader::new(file);
    let mut header = Vec::with_capacity(32);
    (&mut reader).take(32).read_to_end(&mut header)?;
    reader.rewind()?;
    let (mime, format, extensions) = kind(&header);
    let raster_format = format.filter(|format| format.is_raster());
    let mut result = Inspection {
        name: name(path),
        bytes: 0,
        mime,
        format,
        raster_format,
        width: None,
        height: None,
        alpha_channel: None,
        exif: None,
        icc: None,
        animated: None,
        sha256: String::new(),
        warnings: Vec::new(),
    };
    if let Some(ext) = path.extension().and_then(|ext| ext.to_str())
        && !extensions.is_empty()
        && !extensions
            .iter()
            .any(|known| ext.eq_ignore_ascii_case(known))
    {
        result.warnings.push(format!(
            "Filename extension .{ext} and detected content differ"
        ));
    }
    let mut hasher = Sha256::new();
    result.bytes = std::io::copy(&mut reader, &mut hasher)?;
    result.sha256 = format!("{:x}", hasher.finalize());
    if let Some(format) = raster_format {
        if result.bytes > MAX_IMAGE_BYTES {
            result
                .warnings
                .push("File exceeds the decoder input safety limit".into());
            return Ok(result);
        }
        reader.rewind()?;
        match raster(reader, format).and_then(|(mut decoder, animated)| {
            let (width, height) = decoder.dimensions();
            result.width = Some(width);
            result.height = Some(height);
            result.animated = Some(animated);
            result.alpha_channel = Some(decoder.color_type().has_alpha());
            result.exif = Some(decoder.exif_metadata()?.is_some());
            result.icc = Some(decoder.icc_profile()?.is_some());
            Ok(())
        }) {
            Ok(()) => {}
            Err(error) => result.warnings.push(format!(
                "Image header or metadata could not be read: {error}"
            )),
        }
    }
    Ok(result)
}

fn limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(20_000);
    limits.max_image_height = Some(20_000);
    limits.max_alloc = Some(MAX_PIXELS * 4 + 64 * 1024 * 1024);
    limits
}

fn raster<'a>(
    reader: impl std::io::BufRead + Seek + 'a,
    format: Format,
) -> Result<(Box<dyn ImageDecoder + 'a>, bool)> {
    let (mut decoder, animated): (Box<dyn ImageDecoder>, bool) = match format {
        Format::Png => {
            let decoder = PngDecoder::with_limits(reader, limits())?;
            let animated = decoder.is_apng()?;
            (Box::new(decoder), animated)
        }
        Format::Webp => {
            let decoder = WebPDecoder::new(reader)?;
            let animated = decoder.has_animation();
            (Box::new(decoder), animated)
        }
        Format::Jpeg => {
            let mut reader = ImageReader::with_format(reader, image::ImageFormat::Jpeg);
            reader.limits(limits());
            (Box::new(reader.into_decoder()?), false)
        }
        _ => return Err(Error::Unsupported("Expected a raster input".into())),
    };
    let (width, height) = decoder.dimensions();
    check_pixels(width, height)?;
    decoder.set_limits(limits())?;
    Ok((decoder, animated))
}

pub(crate) fn check_pixels(width: u32, height: u32) -> Result<()> {
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(Error::Unsupported(format!(
            "Image exceeds the {} megapixel decode safety limit",
            MAX_PIXELS / 1_000_000
        )));
    }
    Ok(())
}

pub(crate) struct Decoded {
    pub image: DynamicImage,
    pub metadata: Vec<String>,
    pub orientation_applied: bool,
    /// An RGB ICC profile that output encoders carry over unchanged.
    pub icc: Option<Vec<u8>>,
}

const MAX_ICC_BYTES: usize = 4 * 1024 * 1024;

/// Accept ICC profiles describing RGB data so pixels can keep their values
/// and the profile can travel with them. Anything else needs a color-managed
/// conversion, which FileCraft does not perform.
pub(crate) fn rgb_icc(profile: Vec<u8>) -> Result<Vec<u8>> {
    let declared = profile
        .get(0..4)
        .map(|bytes| u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize);
    if profile.len() < 132
        || profile.len() > MAX_ICC_BYTES
        || declared != Some(profile.len())
        || profile.get(36..40) != Some(b"acsp")
    {
        return Err(Error::Unsupported(
            "The embedded ICC profile is malformed".into(),
        ));
    }
    if profile.get(16..20) != Some(b"RGB ") {
        return Err(Error::Unsupported(
            "Only RGB ICC profiles can be carried over. This image needs a color-managed conversion."
                .into(),
        ));
    }
    Ok(profile)
}

pub(crate) fn decode(bytes: &[u8], format: Format) -> Result<Decoded> {
    let (mut decoder, animated) = raster(Cursor::new(bytes), format)?;
    if animated {
        return Err(Error::Unsupported(
            "Animated images need a frame-preserving engine".into(),
        ));
    }
    let color = decoder.color_type();
    if !matches!(
        color,
        image::ColorType::L8
            | image::ColorType::La8
            | image::ColorType::Rgb8
            | image::ColorType::Rgba8
    ) {
        return Err(Error::Unsupported(
            "Only 8-bit raster input is supported".into(),
        ));
    }
    let icc = match decoder.icc_profile()? {
        Some(_) if !color.has_color() => {
            return Err(Error::Unsupported(
                "Grayscale images with an ICC profile need a color-managed conversion".into(),
            ));
        }
        Some(profile) => Some(rgb_icc(profile)?),
        None => None,
    };
    let mut metadata = Vec::new();
    if decoder.exif_metadata()?.is_some() {
        metadata.push("EXIF".into());
    }
    if decoder.xmp_metadata()?.is_some() {
        metadata.push("XMP".into());
    }
    if decoder.iptc_metadata()?.is_some() {
        metadata.push("IPTC".into());
    }
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    Ok(Decoded {
        image,
        metadata,
        orientation_applied: orientation != Orientation::NoTransforms,
        icc,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ftyp(major: &[u8; 4], compatible: &[&[u8; 4]]) -> Vec<u8> {
        let size = 16 + 4 * compatible.len() as u32;
        let mut header = size.to_be_bytes().to_vec();
        header.extend(b"ftyp");
        header.extend(major);
        header.extend([0, 0, 0, 0]);
        for brand in compatible {
            header.extend(*brand);
        }
        header.resize(32, 0);
        header
    }

    #[test]
    fn recognizes_phone_containers() {
        assert_eq!(
            kind(&ftyp(b"heic", &[b"mif1", b"heic"])).1,
            Some(Format::Heic)
        );
        assert_eq!(
            kind(&ftyp(b"mif1", &[b"mif1", b"heic"])).1,
            Some(Format::Heic)
        );
        assert_eq!(kind(&ftyp(b"mif1", &[b"mif1", b"avif"])).1, None);
        assert_eq!(kind(&ftyp(b"avif", &[b"mif1", b"avif"])).1, None);
        assert_eq!(kind(&ftyp(b"qt  ", &[b"qt  "])).1, Some(Format::Mov));
        assert_eq!(
            kind(&ftyp(b"isom", &[b"isom", b"mp41"])).1,
            Some(Format::Mp4)
        );
    }

    #[test]
    fn reads_heif_metadata_items() {
        let infe = |version: u8, kind: &[u8; 4], extra: &[u8]| {
            let mut payload = vec![version, 0, 0, 0, 0, 1, 0, 0];
            payload.extend(kind);
            payload.extend(extra);
            let mut item = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
            item.extend(b"infe");
            item.extend(payload);
            item
        };
        let mut entries = infe(2, b"hvc1", b"\0");
        entries.extend(infe(2, b"Exif", b"\0"));
        entries.extend(infe(2, b"mime", b"\0application/rdf+xml\0"));
        let mut iinf = vec![0, 0, 0, 0, 0, 3];
        iinf.extend(entries);
        let wrap = |kind: &[u8; 4], payload: Vec<u8>| {
            let mut boxed = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
            boxed.extend(kind);
            boxed.extend(payload);
            boxed
        };
        let mut meta = vec![0, 0, 0, 0];
        meta.extend(wrap(b"iinf", iinf));
        let mut file = ftyp(b"heic", &[b"mif1"])[..20].to_vec();
        file[0..4].copy_from_slice(&20_u32.to_be_bytes());
        file.extend(wrap(b"meta", meta));
        assert_eq!(heif_metadata(&file), ["EXIF", "XMP"]);
        assert!(heif_metadata(b"not a heif file").is_empty());
    }
}
