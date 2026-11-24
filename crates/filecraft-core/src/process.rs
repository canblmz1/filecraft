use std::{
    fs,
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use image::{
    ExtendedColorType, ImageEncoder,
    codecs::{
        jpeg::JpegEncoder,
        png::{CompressionType, FilterType, PngEncoder},
        webp::WebPEncoder,
    },
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tempfile::{NamedTempFile, TempDir};

use crate::{
    Error, Format, Recipe, Result, capabilities,
    inspect::{Decoded, decode, digest, kind, name, open_regular},
    pdf, video,
};

#[derive(Debug, Default)]
pub struct Control {
    cancelled: AtomicBool,
}

impl Control {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    fn check(&self, deadline: Option<Instant>) -> Result<()> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Err(Error::Timeout);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Inspecting,
    Planning,
    Running,
    Verifying,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Progress {
    pub phase: Phase,
    pub files_finished: usize,
    pub files_total: usize,
    pub file_index: Option<usize>,
    pub candidates_finished: usize,
    pub candidates_total: usize,
    pub steps_finished: usize,
    pub steps_total: usize,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Completed,
    Partial,
    Failed,
    Cancelled,
}

#[derive(Debug, Serialize)]
pub struct BatchReport {
    pub status: Status,
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub goal: Recipe,
    pub duration_ms: u64,
    pub files: Vec<FileReport>,
    pub error: Option<String>,
    pub error_code: Option<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct FileReport {
    pub input_name: String,
    pub input_format: Format,
    pub input_sha256: String,
    pub input_bytes: u64,
    pub output_path: PathBuf,
    pub output_format: Format,
    pub output_bytes: u64,
    pub output_sha256: String,
    pub width: u32,
    pub height: u32,
    pub duration_ms: u64,
    pub jpeg_quality: Option<u8>,
    pub psnr_db: Option<f64>,
    pub pixels_identical: bool,
    pub orientation_applied: bool,
    pub metadata_removed: Vec<String>,
    pub unchanged: bool,
    pub pdf_pages: Option<usize>,
    pub video_duration_seconds: Option<f64>,
    pub video_crf: Option<u8>,
    pub verified_checks: Vec<&'static str>,
    pub unsupported_checks: Vec<&'static str>,
}

struct Input {
    path: PathBuf,
    bytes: u64,
    format: Format,
}

struct Plans {
    input: Input,
    checksum: String,
    dimensions: (u32, u32),
    orientation_applied: bool,
    metadata: Vec<String>,
    candidates: Vec<Candidate>,
    duration_ms: u64,
}

struct Candidate {
    temp: NamedTempFile,
    size: u64,
    checksum: String,
    format: Format,
    evidence: Evidence,
}

enum Evidence {
    Raster { quality: Option<u8>, mse: f64 },
    RasterCopy,
    Pdf { pages: usize, compressed: bool },
    Video { duration: f64, crf: Option<u8> },
}

impl Candidate {
    fn psnr(&self) -> Option<f64> {
        match self.evidence {
            Evidence::Raster { mse, .. } if mse > 0.0 => Some(10.0 * (65_025.0 / mse).log10()),
            _ => None,
        }
    }

    fn penalty(&self) -> f64 {
        match self.evidence {
            Evidence::Raster { mse, .. } => mse,
            Evidence::Video { crf: Some(crf), .. } => f64::from(crf - 17),
            _ => 0.0,
        }
    }
}

pub fn run(
    paths: &[PathBuf],
    destination: &Path,
    recipe: &Recipe,
    control: &Control,
    mut progress: impl FnMut(Progress),
) -> BatchReport {
    let start = Instant::now();
    let deadline = recipe
        .max_seconds
        .and_then(|seconds| start.checked_add(Duration::from_secs(seconds)));
    let mut report = BatchReport {
        status: Status::Failed,
        input_bytes: 0,
        output_bytes: 0,
        goal: recipe.clone(),
        duration_ms: 0,
        files: Vec::new(),
        error: None,
        error_code: None,
    };
    let result = execute(
        paths,
        destination,
        &mut report,
        control,
        deadline,
        &mut progress,
    );
    match result {
        Ok(()) => report.status = Status::Completed,
        Err(error) => {
            report.status = if !report.files.is_empty() {
                Status::Partial
            } else if matches!(error, Error::Cancelled) {
                Status::Cancelled
            } else {
                Status::Failed
            };
            report.error_code = Some(error.code());
            report.error = Some(error.to_string());
        }
    }
    report.duration_ms = elapsed_ms(start);
    report
}

fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn execute(
    paths: &[PathBuf],
    destination: &Path,
    report: &mut BatchReport,
    control: &Control,
    deadline: Option<Instant>,
    progress: &mut impl FnMut(Progress),
) -> Result<()> {
    let mut update = Progress {
        phase: Phase::Inspecting,
        files_finished: 0,
        files_total: paths.len(),
        file_index: None,
        candidates_finished: 0,
        candidates_total: 0,
        steps_finished: 0,
        steps_total: paths.len().saturating_mul(3).saturating_add(1),
    };
    progress(update);
    let recipe = report.goal.clone();
    recipe.validate()?;
    if paths.is_empty() {
        return Err(Error::Invalid("Add at least one input file".into()));
    }
    let destination = destination.canonicalize()?;
    if !destination.is_dir() {
        return Err(Error::Invalid("Choose an output directory".into()));
    }
    let mixed = recipe.output_format == Format::Mixed;
    let engines = capabilities();
    let mut inputs = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        control.check(deadline)?;
        let mut file = open_regular(path).map_err(|error| file_error(path, error))?;
        let bytes = file.metadata()?.len();
        let mut header = Vec::new();
        (&mut file).take(32).read_to_end(&mut header)?;
        let (_, format, _) = kind(&header);
        let format = format.ok_or_else(|| {
            Error::Unsupported(format!("{}: unrecognized input format", name(path)))
        })?;
        let engine = engines
            .iter()
            .find(|engine| {
                engine.input_formats.contains(&format)
                    && (mixed || engine.output_formats.contains(&recipe.output_format))
            })
            .ok_or_else(|| {
                Error::Unsupported(format!(
                    "{}: select Mixed batch for PDF and MP4, or choose a supported raster image",
                    name(path)
                ))
            })?;
        if !engine.available {
            return Err(Error::Unsupported(format!(
                "{}: install FFmpeg with libx264 and AAC plus FFprobe on PATH, then restart FileCraft",
                name(path)
            )));
        }
        if bytes > engine.max_input_bytes {
            return Err(Error::Unsupported(format!(
                "{}: input exceeds the {} MiB limit for this format",
                name(path),
                engine.max_input_bytes / (1024 * 1024)
            )));
        }
        report.input_bytes = report
            .input_bytes
            .checked_add(bytes)
            .ok_or_else(|| Error::Invalid("Input byte count overflow".into()))?;
        inputs.push(Input {
            path: path.clone(),
            bytes,
            format,
        });
        update.files_finished = index + 1;
        update.steps_finished += 1;
        progress(update);
    }
    update.phase = Phase::Planning;
    update.files_finished = 0;
    progress(update);
    if fs2::available_space(&destination)? < 8 * 1024 * 1024 {
        return Err(Error::Io(std::io::Error::other(
            "Output drive has less than 8 MiB of free space",
        )));
    }
    let workspace = tempfile::Builder::new()
        .prefix(".filecraft-stage-")
        .tempdir_in(&destination)?;
    update.steps_finished += 1;
    let mut search_recipe = recipe.clone();
    if mixed
        && recipe
            .max_batch_bytes
            .is_some_and(|cap| report.input_bytes <= cap)
        && recipe
            .max_file_bytes
            .is_none_or(|cap| inputs.iter().all(|input| input.bytes <= cap))
    {
        search_recipe.lossless_only = true;
    }
    let qualities = quality_ladder(&search_recipe);
    let mut plans = Vec::new();
    for (index, input) in inputs.into_iter().enumerate() {
        update.phase = Phase::Running;
        update.file_index = Some(index);
        update.candidates_finished = 0;
        update.candidates_total = if input.format == Format::Mp4 && !search_recipe.lossless_only {
            video::QUALITY.len() + 1
        } else {
            qualities.len()
        };
        progress(update);
        let update_candidate = |phase, count| {
            update.phase = phase;
            update.candidates_finished = count;
            progress(update);
        };
        let result = prepare(
            input,
            &search_recipe,
            &qualities,
            &workspace,
            control,
            deadline,
            update_candidate,
        )?;
        plans.push(result);
        update.files_finished = index + 1;
        update.steps_finished += 1;
        progress(update);
    }
    update.phase = Phase::Verifying;
    update.file_index = None;
    update.files_finished = 0;
    progress(update);
    control.check(deadline)?;
    let selection = choose(&plans, recipe.max_batch_bytes, control, deadline)?;
    for (mut plan, chosen) in plans.into_iter().zip(selection) {
        control.check(deadline)?;
        let candidate = plan.candidates.swap_remove(chosen);
        let psnr_db = candidate.psnr();
        let stem = plan
            .input
            .path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy();
        let stem: String = stem.chars().take(48).collect();
        let output_path = publish(candidate.temp, &destination, &stem, candidate.format)?;
        report.output_bytes += candidate.size;
        let (mut checks, unsupported_checks) = evidence_checks(&candidate.evidence);
        checks.extend(["format signature", "SHA-256"]);
        if recipe.max_file_bytes.is_some() {
            checks.push("per-file byte limit");
        }
        if recipe.max_batch_bytes.is_some() {
            checks.push("planned batch byte limit");
        }
        if recipe.min_psnr_db.is_some() {
            checks.push("minimum PSNR");
        }
        let (
            jpeg_quality,
            pixels_identical,
            unchanged,
            pdf_pages,
            video_duration_seconds,
            video_crf,
        ) = match candidate.evidence {
            Evidence::Raster { quality, mse } => (quality, mse == 0.0, false, None, None, None),
            Evidence::RasterCopy => (None, true, true, None, None, None),
            Evidence::Pdf { pages, compressed } => {
                (None, false, !compressed, Some(pages), None, None)
            }
            Evidence::Video { duration, crf } => {
                (None, false, crf.is_none(), None, Some(duration), crf)
            }
        };
        report.files.push(FileReport {
            input_name: name(&plan.input.path),
            input_format: plan.input.format,
            input_sha256: plan.checksum,
            input_bytes: plan.input.bytes,
            output_path,
            output_format: candidate.format,
            output_bytes: candidate.size,
            output_sha256: candidate.checksum,
            width: plan.dimensions.0,
            height: plan.dimensions.1,
            duration_ms: plan.duration_ms,
            jpeg_quality,
            psnr_db,
            pixels_identical,
            orientation_applied: !unchanged && plan.orientation_applied,
            metadata_removed: if unchanged { Vec::new() } else { plan.metadata },
            unchanged,
            pdf_pages,
            video_duration_seconds,
            video_crf,
            verified_checks: checks,
            unsupported_checks,
        });
        update.files_finished += 1;
        update.steps_finished += 1;
        progress(update);
    }
    Ok(())
}

fn evidence_checks(evidence: &Evidence) -> (Vec<&'static str>, Vec<&'static str>) {
    match evidence {
        Evidence::Raster { mse, .. } => {
            let mut checks = vec![
                "decoded output",
                "orientation-normalized dimensions",
                "EXIF/ICC/XMP/IPTC decoder probes",
            ];
            let mut unchecked = vec![
                "visual color fidelity",
                "sensitive pixels",
                "proprietary metadata beyond the probes",
            ];
            if *mse == 0.0 {
                checks.push("decoded RGBA pixels identical after orientation");
            } else {
                checks.push("RGB PSNR measured");
                unchecked.push("perceptual quality");
            }
            (checks, unchecked)
        }
        Evidence::RasterCopy => (
            vec!["original bytes identical", "raster decoded"],
            vec!["privacy metadata removal"],
        ),
        Evidence::Pdf {
            compressed: true, ..
        } => (
            vec![
                "PDF strict parse",
                "page order",
                "parsed PDF objects and trailer",
                "changed streams decode to original bytes",
            ],
            vec![
                "independent rendering",
                "PDF active content or privacy metadata removal",
            ],
        ),
        Evidence::Pdf {
            compressed: false, ..
        } => (
            vec!["PDF strict parse", "original bytes identical"],
            vec![
                "signature validity",
                "independent rendering",
                "PDF active content or privacy metadata removal",
            ],
        ),
        Evidence::Video { crf, .. } => {
            let checks = if crf.is_some() {
                vec![
                    "complete decode",
                    "H.264 and optional AAC",
                    "dimensions",
                    "video frame count",
                    "duration and audio stream properties",
                ]
            } else {
                vec!["MP4 probed and fully decoded", "original bytes identical"]
            };
            (
                checks,
                vec![
                    "perceptual audio/video quality",
                    "color fidelity",
                    "metadata equivalence or privacy",
                ],
            )
        }
    }
}

fn file_error(path: &Path, error: Error) -> Error {
    let message = format!("{}: {error}", name(path));
    match error {
        Error::Unsupported(_) => Error::Unsupported(message),
        Error::Invalid(_) => Error::Invalid(message),
        Error::Goal(_) => Error::Goal(message),
        Error::Cancelled | Error::Timeout => error,
        Error::Image(image) => Error::Image(image),
        Error::Io(io) => Error::Io(std::io::Error::new(io.kind(), message)),
        Error::Json(_) => error,
    }
}

fn quality_ladder(recipe: &Recipe) -> Vec<Option<u8>> {
    if recipe.output_format != Format::Jpeg {
        return vec![None];
    }
    let preferred = recipe.jpeg_quality.unwrap_or(90);
    if recipe.max_batch_bytes.is_none() && recipe.max_file_bytes.is_none() {
        return vec![Some(preferred)];
    }
    std::iter::once(preferred)
        .chain(
            [88, 80, 70, 60, 48, 35]
                .into_iter()
                .filter(|quality| *quality < preferred),
        )
        .map(Some)
        .collect()
}

fn prepare(
    input: Input,
    recipe: &Recipe,
    qualities: &[Option<u8>],
    workspace: &TempDir,
    control: &Control,
    deadline: Option<Instant>,
    mut update: impl FnMut(Phase, usize),
) -> Result<Plans> {
    let started = Instant::now();
    let path = input.path.clone();
    (|| {
        if input.format == Format::Pdf {
            return prepare_pdf(input, recipe, workspace, || control.check(deadline), started, update);
        }
        if input.format == Format::Mp4 {
            return prepare_video(input, recipe, workspace, || control.check(deadline), started, update);
        }
        let mixed = recipe.output_format == Format::Mixed;
        let output_format = if mixed { Format::Webp } else { recipe.output_format };
        let bytes = snapshot_bytes(&input)?;
        control.check(deadline)?;
        let checksum = digest(&bytes);
        let decoded = decode(&bytes, input.format)?;
        let original = mixed.then_some(bytes);
        let expected = decoded.image.into_rgba8();
        let dimensions = expected.dimensions();
        if recipe.min_width.is_some_and(|width| width > dimensions.0)
            || recipe.min_height.is_some_and(|height| height > dimensions.1)
        {
            return Err(Error::Goal(format!("Oriented size is {} × {} px. Lower the minimum dimensions.", dimensions.0, dimensions.1)));
        }
        if output_format == Format::Jpeg && expected.pixels().any(|pixel| pixel[3] != 255) {
            return Err(Error::Goal("JPEG would discard transparency. Choose PNG or WebP.".into()));
        }
        let rgb = (output_format == Format::Jpeg).then(|| {
            image::ImageBuffer::from_fn(dimensions.0, dimensions.1, |x, y| {
                let [r, g, b, _] = expected.get_pixel(x, y).0;
                image::Rgb([r, g, b])
            })
        });
        let mut candidates = Vec::new();
        let mut smallest = None::<u64>;
        for (index, quality) in qualities.iter().copied().enumerate() {
            control.check(deadline)?;
            update(Phase::Running, index);
            let mut output = Vec::new();
            match output_format {
                Format::Jpeg => {
                    let rgb = rgb.as_ref().ok_or_else(|| Error::Invalid("Missing JPEG input pixels".into()))?;
                    JpegEncoder::new_with_quality(&mut output, quality.unwrap_or(90))
                        .encode(rgb, dimensions.0, dimensions.1, ExtendedColorType::Rgb8)?;
                }
                Format::Png => {
                    PngEncoder::new_with_quality(&mut output, CompressionType::Best, FilterType::Adaptive)
                        .write_image(&expected, dimensions.0, dimensions.1, ExtendedColorType::Rgba8)?;
                }
                Format::Webp => {
                    WebPEncoder::new_lossless(&mut output)
                        .write_image(&expected, dimensions.0, dimensions.1, ExtendedColorType::Rgba8)?;
                }
                _ => return Err(Error::Unsupported("Expected a raster output".into())),
            }
            control.check(deadline)?;
            let size = output.len() as u64;
            if let Some(original) = original.as_deref() && size >= input.bytes {
                smallest = Some(input.bytes);
                if recipe.max_file_bytes.is_none_or(|cap| input.bytes <= cap) {
                    update(Phase::Verifying, index);
                    let (temp, _) = stage_bytes(workspace, original)?;
                    candidates.push(Candidate { temp, size: input.bytes, checksum: checksum.clone(), format: input.format, evidence: Evidence::RasterCopy });
                }
                continue;
            }
            smallest = Some(smallest.map_or(size, |current| current.min(size)));
            if recipe.max_file_bytes.is_some_and(|cap| size > cap) {
                continue;
            }
            update(Phase::Verifying, index);
            if fs2::available_space(workspace.path())? < size {
                return Err(Error::Io(std::io::Error::other("Not enough free space to stage this candidate")));
            }
            let mut temp = NamedTempFile::new_in(workspace.path())?;
            temp.write_all(&output)?;
            temp.as_file().sync_all()?;
            drop(output);
            let staged = fs::read(temp.path())?;
            if staged.len() as u64 != size || kind(&staged).1 != Some(output_format) {
                return Err(Error::Goal("Candidate failed size or format verification".into()));
            }
            let Decoded { image, metadata, orientation_applied } = decode(&staged, output_format)?;
            if !metadata.is_empty() || orientation_applied {
                return Err(Error::Goal("Candidate retained unexpected metadata".into()));
            }
            let observed = image.into_rgba8();
            if observed.dimensions() != dimensions {
                return Err(Error::Goal("Output dimensions changed unexpectedly".into()));
            }
            let mse = measure(&expected, &observed, control, deadline)?;
            if output_format != Format::Jpeg && mse != 0.0 {
                return Err(Error::Goal("Lossless output changed decoded pixels".into()));
            }
            let checksum = digest(&staged);
            let candidate = Candidate { temp, size, checksum, format: output_format, evidence: Evidence::Raster { quality, mse } };
            let meets_psnr = recipe.min_psnr_db.is_none_or(|min| mse == 0.0 || candidate.psnr().is_some_and(|db| db >= f64::from(min)));
            if meets_psnr {
                candidates.push(candidate);
            }
        }
        if candidates.is_empty() {
            return Err(Error::Goal(format!(
                "No candidate met the per-file limits. Smallest attempted: {} bytes. Raise the cap or adjust the format or quality constraints.",
                smallest.unwrap_or(0),
            )));
        }
        pareto(&mut candidates);
        Ok(Plans {
            input, checksum, dimensions, orientation_applied: decoded.orientation_applied,
            metadata: decoded.metadata, candidates, duration_ms: elapsed_ms(started),
        })
    })().map_err(|error| file_error(&path, error))
}

fn snapshot_bytes(input: &Input) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    BufReader::new(open_regular(&input.path)?)
        .take(input.bytes + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != input.bytes || kind(&bytes).1 != Some(input.format) {
        return Err(Error::Invalid(
            "Input changed during analysis. Retry with a stable file.".into(),
        ));
    }
    Ok(bytes)
}

fn ensure_space(workspace: &TempDir, size: u64) -> Result<()> {
    if fs2::available_space(workspace.path())? < size {
        return Err(Error::Io(std::io::Error::other(
            "Not enough free space to stage this candidate",
        )));
    }
    Ok(())
}

fn check_file(
    path: &Path,
    format: Format,
    check: &impl Fn() -> Result<()>,
) -> Result<(u64, String)> {
    let mut reader = BufReader::new(open_regular(path)?);
    let mut header = Vec::new();
    (&mut reader).take(32).read_to_end(&mut header)?;
    if kind(&header).1 != Some(format) {
        return Err(Error::Goal(
            "Candidate has an unexpected file signature".into(),
        ));
    }
    let mut hasher = Sha256::new();
    hasher.update(&header);
    let mut size = header.len() as u64;
    let mut buffer = vec![0_u8; 128 * 1024];
    loop {
        check()?;
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        size += count as u64;
    }
    Ok((size, format!("{:x}", hasher.finalize())))
}

fn stage_bytes(workspace: &TempDir, bytes: &[u8]) -> Result<(NamedTempFile, String)> {
    ensure_space(workspace, bytes.len() as u64)?;
    let mut temp = NamedTempFile::new_in(workspace.path())?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    let mut reader = BufReader::new(temp.reopen()?);
    let mut hasher = Sha256::new();
    let size = std::io::copy(&mut reader, &mut hasher)?;
    let staged = format!("{:x}", hasher.finalize());
    if size != bytes.len() as u64 || staged != digest(bytes) {
        return Err(Error::Goal("Staged bytes failed verification".into()));
    }
    Ok((temp, staged))
}

fn prepare_pdf(
    input: Input,
    recipe: &Recipe,
    workspace: &TempDir,
    check: impl Fn() -> Result<()>,
    started: Instant,
    mut update: impl FnMut(Phase, usize),
) -> Result<Plans> {
    let bytes = snapshot_bytes(&input)?;
    let checksum = digest(&bytes);
    let verified = pdf::optimize(&bytes, &check)?;
    update(Phase::Verifying, 0);
    let compressed = verified.output.is_some();
    let output = verified.output.as_deref().unwrap_or(&bytes);
    let size = output.len() as u64;
    if recipe.max_file_bytes.is_some_and(|cap| size > cap) {
        return Err(Error::Goal(format!(
            "The smallest verified lossless PDF is {size} bytes. Raise the per-file limit."
        )));
    }
    check()?;
    let (temp, output_checksum) = stage_bytes(workspace, output)?;
    Ok(Plans {
        input,
        checksum,
        dimensions: (0, 0),
        metadata: Vec::new(),
        orientation_applied: false,
        candidates: vec![Candidate {
            temp,
            size,
            checksum: output_checksum,
            format: Format::Pdf,
            evidence: Evidence::Pdf {
                pages: verified.pages,
                compressed,
            },
        }],
        duration_ms: elapsed_ms(started),
    })
}

fn prepare_video(
    input: Input,
    recipe: &Recipe,
    workspace: &TempDir,
    check: impl Fn() -> Result<()>,
    started: Instant,
    mut update: impl FnMut(Phase, usize),
) -> Result<Plans> {
    let tools = video::tools()
        .ok_or_else(|| Error::Unsupported("FFmpeg and FFprobe are unavailable".into()))?;
    ensure_space(workspace, input.bytes)?;
    let mut staged = NamedTempFile::new_in(workspace.path())?;
    let mut reader = BufReader::new(open_regular(&input.path)?).take(input.bytes + 1);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 128 * 1024];
    loop {
        check()?;
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        staged.write_all(&buffer[..count])?;
    }
    staged.as_file().sync_all()?;
    let checksum = format!("{:x}", hasher.finalize());
    let (size, staged_checksum) = check_file(staged.path(), Format::Mp4, &check)?;
    if size != input.bytes || staged_checksum != checksum {
        return Err(Error::Invalid(
            "The staged MP4 does not match the input snapshot".into(),
        ));
    }
    let video = tools.probe(staged.path(), &check)?;
    tools.decode(staged.path(), &video, &check)?;
    let dimensions = video.dimensions;
    if recipe.min_width.is_some_and(|width| width > dimensions.0)
        || recipe
            .min_height
            .is_some_and(|height| height > dimensions.1)
    {
        return Err(Error::Goal(format!(
            "Video is {} × {} px. Lower the minimum dimensions.",
            dimensions.0, dimensions.1
        )));
    }
    let mut candidates = Vec::new();
    let fits = |size| recipe.max_file_bytes.is_none_or(|cap| size <= cap);
    let ceiling = input
        .bytes
        .saturating_sub(1)
        .min(recipe.max_file_bytes.unwrap_or(u64::MAX))
        .min(recipe.max_batch_bytes.unwrap_or(u64::MAX));
    if !recipe.lossless_only && ceiling >= 1024 {
        for (index, crf) in video::QUALITY.into_iter().enumerate() {
            check()?;
            update(Phase::Running, index + 1);
            ensure_space(workspace, ceiling)?;
            let temp = NamedTempFile::new_in(workspace.path())?;
            if skip_candidate(tools.encode(staged.path(), temp.path(), crf, ceiling, &check))?
                .is_none()
            {
                continue;
            }
            temp.as_file().sync_all()?;
            update(Phase::Verifying, index + 1);
            let Some((size, output_checksum)) =
                skip_candidate(check_file(temp.path(), Format::Mp4, &check))?
            else {
                continue;
            };
            if size > ceiling {
                continue;
            }
            let Some(probed) = skip_candidate(tools.probe(temp.path(), &check))? else {
                continue;
            };
            if skip_candidate(video.verify_encoded(&probed))?.is_none()
                || skip_candidate(tools.decode(temp.path(), &probed, &check))?.is_none()
            {
                continue;
            }
            candidates.push(Candidate {
                temp,
                size,
                checksum: output_checksum,
                format: Format::Mp4,
                evidence: Evidence::Video {
                    duration: probed.seconds,
                    crf: Some(crf),
                },
            });
        }
    }
    if fits(input.bytes) {
        candidates.push(Candidate {
            temp: staged,
            size: input.bytes,
            checksum: checksum.clone(),
            format: Format::Mp4,
            evidence: Evidence::Video {
                duration: video.seconds,
                crf: None,
            },
        });
    }
    if candidates.is_empty() {
        return Err(Error::Goal(
            "No verified MP4 fits the per-file limit. Increase the limit or use a shorter clip."
                .into(),
        ));
    }
    pareto(&mut candidates);
    Ok(Plans {
        input,
        checksum,
        dimensions,
        metadata: Vec::new(),
        orientation_applied: false,
        candidates,
        duration_ms: elapsed_ms(started),
    })
}

fn skip_candidate<T>(result: Result<T>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(Error::Goal(_) | Error::Invalid(_) | Error::Unsupported(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

fn pareto(candidates: &mut Vec<Candidate>) {
    candidates.sort_by(|a, b| {
        a.size
            .cmp(&b.size)
            .then_with(|| a.penalty().total_cmp(&b.penalty()))
    });
    let mut best = f64::INFINITY;
    candidates.retain(|candidate| {
        let penalty = candidate.penalty();
        let improved = penalty < best;
        if improved {
            best = penalty;
        }
        improved
    });
}

fn measure(
    expected: &image::RgbaImage,
    observed: &image::RgbaImage,
    control: &Control,
    deadline: Option<Instant>,
) -> Result<f64> {
    let mut squared = 0_u64;
    for (index, (lhs, rhs)) in expected.pixels().zip(observed.pixels()).enumerate() {
        if index % 65_536 == 0 {
            control.check(deadline)?;
        }
        if lhs[3] != rhs[3] {
            return Err(Error::Goal("Output alpha values differ".into()));
        }
        for channel in 0..3 {
            let delta = i32::from(lhs[channel]) - i32::from(rhs[channel]);
            squared += (delta * delta) as u64;
        }
    }
    Ok(squared as f64 / (3.0 * f64::from(expected.width()) * f64::from(expected.height())))
}

fn choose(
    plans: &[Plans],
    cap: Option<u64>,
    control: &Control,
    deadline: Option<Instant>,
) -> Result<Vec<usize>> {
    let Some(cap) = cap else {
        return Ok(plans.iter().map(|plan| plan.candidates.len() - 1).collect());
    };
    let minimum: u64 = plans.iter().map(|plan| plan.candidates[0].size).sum();
    let mut remaining = cap.checked_sub(minimum).ok_or_else(|| {
        Error::Goal(format!("Batch target is {cap} bytes. The smallest verified combination needs {minimum} bytes. Increase the batch cap or adjust the goal."))
    })?;
    let mut selection = vec![0; plans.len()];
    loop {
        control.check(deadline)?;
        let mut best = None::<(usize, usize, u64, f64)>;
        for (index, plan) in plans.iter().enumerate() {
            let current = &plan.candidates[selection[index]];
            for (next, candidate) in plan
                .candidates
                .iter()
                .enumerate()
                .skip(selection[index] + 1)
            {
                let cost = candidate.size - current.size;
                if cost > remaining {
                    continue;
                }
                let gain = (current.penalty() - candidate.penalty()) / plan.candidates[0].penalty();
                let score = gain / cost as f64;
                if best.is_none_or(|(_, _, _, incumbent)| score > incumbent) {
                    best = Some((index, next, cost, score));
                }
            }
        }
        let Some((index, next, cost, _)) = best else {
            break;
        };
        remaining -= cost;
        selection[index] = next;
    }
    Ok(selection)
}

fn publish(
    mut temp: NamedTempFile,
    directory: &Path,
    stem: &str,
    format: Format,
) -> Result<PathBuf> {
    for suffix in 0..=10_000 {
        let identifier = if suffix == 0 {
            String::new()
        } else {
            format!("-{suffix}")
        };
        let output = directory.join(format!(
            "{stem}-filecraft{identifier}.{}",
            format.extension()
        ));
        match temp.persist_noclobber(&output) {
            Ok(_) => return Ok(output),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                temp = error.file
            }
            Err(error) => return Err(error.error.into()),
        }
    }
    Err(Error::Io(std::io::Error::other(
        "No unused output name was found. Choose a different folder.",
    )))
}
