use std::{fs::File, io::Read, path::Path};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Jpeg,
    Png,
    Webp,
    Pdf,
    Mp4,
    Mixed,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Pdf => "pdf",
            Self::Mp4 => "mp4",
            Self::Mixed => unreachable!("a batch mode has no file extension"),
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Webp => "image/webp",
            Self::Pdf => "application/pdf",
            Self::Mp4 => "video/mp4",
            Self::Mixed => unreachable!("a batch mode has no MIME type"),
        }
    }

    pub(crate) fn is_raster(self) -> bool {
        matches!(self, Self::Jpeg | Self::Png | Self::Webp)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub version: u32,
    pub output_format: Format,
    pub max_file_bytes: Option<u64>,
    pub max_batch_bytes: Option<u64>,
    pub min_width: Option<u32>,
    pub min_height: Option<u32>,
    pub jpeg_quality: Option<u8>,
    pub min_psnr_db: Option<u8>,
    #[serde(default)]
    pub lossless_only: bool,
    pub max_seconds: Option<u64>,
}

impl Recipe {
    pub fn read(path: &Path) -> Result<Self> {
        let mut bytes = Vec::new();
        File::open(path)?.take(65_537).read_to_end(&mut bytes)?;
        if bytes.len() > 65_536 {
            return Err(Error::Invalid("Recipe exceeds 64 KiB".into()));
        }
        let recipe: Self = serde_json::from_slice(&bytes)?;
        recipe.validate()?;
        Ok(recipe)
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            return Err(Error::Unsupported(format!(
                "Recipe version {} is unsupported",
                self.version
            )));
        }
        if self.output_format == Format::Mixed && self.max_batch_bytes.is_none() {
            return Err(Error::Invalid(
                "Set a total size limit for a mixed batch".into(),
            ));
        }
        if matches!(self.output_format, Format::Pdf | Format::Mp4) {
            return Err(Error::Unsupported(
                "Choose Mixed batch for PDF or MP4 processing".into(),
            ));
        }
        if [self.max_file_bytes, self.max_batch_bytes, self.max_seconds].contains(&Some(0))
            || [self.min_width, self.min_height].contains(&Some(0))
            || self.max_seconds.is_some_and(|seconds| seconds > 86_400)
        {
            return Err(Error::Invalid(
                "Limits must be positive and the time budget may be at most 24 hours".into(),
            ));
        }
        const MAX_PRECISE_UI_INTEGER: u64 = (1 << 53) - 1;
        if [self.max_file_bytes, self.max_batch_bytes]
            .into_iter()
            .flatten()
            .any(|bytes| bytes > MAX_PRECISE_UI_INTEGER)
        {
            return Err(Error::Invalid(
                "A byte limit exceeds the supported integer precision".into(),
            ));
        }
        if self.output_format == Format::Jpeg {
            if self.lossless_only {
                return Err(Error::Goal(
                    "Choose PNG or WebP for lossless encoding".into(),
                ));
            }
            if self
                .jpeg_quality
                .is_some_and(|quality| !(35..=100).contains(&quality))
                || self.min_psnr_db.is_some_and(|db| !(1..=100).contains(&db))
            {
                return Err(Error::Invalid(
                    "JPEG quality must be 35–100 and minimum PSNR must be 1–100 dB".into(),
                ));
            }
        } else if self.jpeg_quality.is_some() || self.min_psnr_db.is_some() {
            return Err(Error::Goal(
                "JPEG quality controls require JPEG output".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Capability {
    pub interface_version: u32,
    pub id: &'static str,
    pub version: &'static str,
    pub license: &'static str,
    pub input_formats: &'static [Format],
    pub output_formats: &'static [Format],
    pub operations: &'static [&'static str],
    pub constraints: &'static [&'static str],
    pub validation: &'static [&'static str],
    pub external_dependencies: &'static [&'static str],
    pub max_pixels: u64,
    pub max_input_bytes: u64,
    pub cancellation: &'static str,
    pub available: bool,
}

pub(crate) const MAX_PIXELS: u64 = 24_000_000;
pub(crate) const MAX_IMAGE_BYTES: u64 = 128 * 1024 * 1024;
const FORMATS: &[Format] = &[Format::Jpeg, Format::Png, Format::Webp];

pub fn capabilities() -> [Capability; 3] {
    [
        Capability {
            interface_version: 1,
            id: "native-raster",
            version: env!("CARGO_PKG_VERSION"),
            license: "MIT (adapter); MIT OR Apache-2.0 (image crate)",
            input_formats: FORMATS,
            output_formats: FORMATS,
            operations: &[
                "inspect",
                "convert",
                "batch_byte_budget",
                "reencode_without_ancillary_metadata",
            ],
            constraints: &[
                "static 8-bit raster",
                "one decode/encode per worker",
                "no embedded ICC conversion",
            ],
            validation: &[
                "decode",
                "format",
                "dimensions",
                "byte limits",
                "SHA-256",
                "pixel equality or RGB PSNR",
                "EXIF/ICC/XMP/IPTC decoder probes",
            ],
            external_dependencies: &[],
            max_pixels: MAX_PIXELS,
            max_input_bytes: MAX_IMAGE_BYTES,
            cancellation: "cooperative between codec operations",
            available: true,
        },
        Capability {
            interface_version: 1,
            id: "native-pdf",
            version: env!("CARGO_PKG_VERSION"),
            license: "MIT (adapter and lopdf)",
            input_formats: &[Format::Pdf],
            output_formats: &[Format::Pdf],
            operations: &["mixed_batch", "lossless_pdf_stream_compression"],
            constraints: &[
                "Strict parsing",
                "signed, encrypted, incremental and linearized PDFs are kept byte-for-byte when readable",
            ],
            validation: &[
                "SHA-256",
                "page order",
                "parsed objects and trailer",
                "decompressed bytes for changed streams",
            ],
            external_dependencies: &[],
            max_pixels: 0,
            max_input_bytes: crate::pdf::MAX_BYTES,
            cancellation: "cooperative between PDF operations",
            available: true,
        },
        crate::video::capability(),
    ]
}
