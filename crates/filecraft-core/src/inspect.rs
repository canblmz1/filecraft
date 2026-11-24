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
    limits.max_image_width = Some(12_000);
    limits.max_image_height = Some(12_000);
    limits.max_alloc = Some(160 * 1024 * 1024);
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
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(Error::Unsupported(
            "Image exceeds the 24 megapixel decode safety limit".into(),
        ));
    }
    decoder.set_limits(limits())?;
    Ok((decoder, animated))
}

pub(crate) struct Decoded {
    pub image: DynamicImage,
    pub metadata: Vec<String>,
    pub orientation_applied: bool,
}

pub(crate) fn decode(bytes: &[u8], format: Format) -> Result<Decoded> {
    let (mut decoder, animated) = raster(Cursor::new(bytes), format)?;
    if animated {
        return Err(Error::Unsupported(
            "Animated images need a frame-preserving engine".into(),
        ));
    }
    if !matches!(
        decoder.color_type(),
        image::ColorType::L8
            | image::ColorType::La8
            | image::ColorType::Rgb8
            | image::ColorType::Rgba8
    ) {
        return Err(Error::Unsupported(
            "Only 8-bit raster input is supported".into(),
        ));
    }
    if decoder.icc_profile()?.is_some() {
        return Err(Error::Unsupported(
            "Embedded ICC profile requires a color-managed conversion".into(),
        ));
    }
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
    })
}
