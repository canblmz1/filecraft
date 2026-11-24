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
use tempfile::{NamedTempFile, TempDir};

use crate::{
    Error, Format, Recipe, Result, capabilities,
    inspect::{Decoded, decode, digest, kind, name, open_regular},
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
    quality: Option<u8>,
    mse: f64,
}

impl Candidate {
    fn psnr(&self) -> Option<f64> {
        (self.mse > 0.0).then(|| 10.0 * (65_025.0 / self.mse).log10())
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
    let engine = capabilities()
        .into_iter()
        .find(|engine| engine.available && engine.output_formats.contains(&recipe.output_format))
        .ok_or_else(|| {
            Error::Unsupported("No engine is available for the requested output format".into())
        })?;
    let mut inputs = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        control.check(deadline)?;
        let mut file = open_regular(path).map_err(|error| file_error(path, error))?;
        let bytes = file.metadata()?.len();
        let mut header = Vec::new();
        (&mut file).take(32).read_to_end(&mut header)?;
        let (_, format, _) = kind(&header);
        let format = format
            .filter(|format| engine.input_formats.contains(format))
            .ok_or_else(|| {
                Error::Unsupported(format!(
                    "{}: choose a static JPEG, PNG or WebP image",
                    name(path)
                ))
            })?;
        if bytes > engine.max_input_bytes {
            return Err(Error::Unsupported(format!(
                "{}: input exceeds the 128 MiB decoder safety limit",
                name(path)
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
    let qualities = quality_ladder(&recipe);
    if fs2::available_space(&destination)? < 8 * 1024 * 1024 {
        return Err(Error::Io(std::io::Error::other(
            "Output drive has less than 8 MiB of free space",
        )));
    }
    let workspace = tempfile::Builder::new()
        .prefix(".filecraft-stage-")
        .tempdir_in(&destination)?;
    update.steps_finished += 1;
    let mut plans = Vec::new();
    for (index, input) in inputs.into_iter().enumerate() {
        update.phase = Phase::Running;
        update.file_index = Some(index);
        update.candidates_finished = 0;
        update.candidates_total = qualities.len();
        progress(update);
        let result = prepare(
            input,
            &recipe,
            &qualities,
            &workspace,
            control,
            deadline,
            |phase, count| {
                update.phase = phase;
                update.candidates_finished = count;
                progress(update);
            },
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
        let output_path = publish(candidate.temp, &destination, &stem, recipe.output_format)?;
        report.output_bytes += candidate.size;
        let mut checks = vec![
            "decoded output",
            "format signature",
            "orientation-normalized dimensions",
            "SHA-256",
            "EXIF/ICC/XMP/IPTC decoder probes",
        ];
        let mut unsupported_checks = vec![
            "visual color fidelity",
            "sensitive content visible in pixels",
            "proprietary metadata beyond the probes",
        ];
        if recipe.max_file_bytes.is_some() {
            checks.push("per-file byte limit");
        }
        if recipe.max_batch_bytes.is_some() {
            checks.push("planned batch byte limit");
        }
        if candidate.mse == 0.0 {
            checks.push("decoded RGBA pixels identical after orientation");
        } else {
            checks.push("RGB PSNR measured");
            unsupported_checks.push("perceptual quality");
        }
        if recipe.min_psnr_db.is_some() {
            checks.push("minimum PSNR");
        }
        report.files.push(FileReport {
            input_name: name(&plan.input.path),
            input_format: plan.input.format,
            input_sha256: plan.checksum,
            input_bytes: plan.input.bytes,
            output_path,
            output_format: recipe.output_format,
            output_bytes: candidate.size,
            output_sha256: candidate.checksum,
            width: plan.dimensions.0,
            height: plan.dimensions.1,
            duration_ms: plan.duration_ms,
            jpeg_quality: candidate.quality,
            psnr_db,
            pixels_identical: candidate.mse == 0.0,
            orientation_applied: plan.orientation_applied,
            metadata_removed: plan.metadata,
            verified_checks: checks,
            unsupported_checks,
        });
        update.files_finished += 1;
        update.steps_finished += 1;
        progress(update);
    }
    Ok(())
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
        let mut bytes = Vec::new();
        BufReader::new(open_regular(&path)?).take(input.bytes + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 != input.bytes || kind(&bytes).1 != Some(input.format) {
            return Err(Error::Invalid("Input changed during analysis. Retry with a stable file.".into()));
        }
        control.check(deadline)?;
        let checksum = digest(&bytes);
        let decoded = decode(&bytes, input.format)?;
        drop(bytes);
        let expected = decoded.image.into_rgba8();
        let dimensions = expected.dimensions();
        if recipe.min_width.is_some_and(|width| width > dimensions.0)
            || recipe.min_height.is_some_and(|height| height > dimensions.1)
        {
            return Err(Error::Goal(format!("Oriented size is {} × {} px. Lower the minimum dimensions.", dimensions.0, dimensions.1)));
        }
        if recipe.output_format == Format::Jpeg && expected.pixels().any(|pixel| pixel[3] != 255) {
            return Err(Error::Goal("JPEG would discard transparency. Choose PNG or WebP.".into()));
        }
        let rgb = (recipe.output_format == Format::Jpeg).then(|| {
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
            match recipe.output_format {
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
            }
            control.check(deadline)?;
            let size = output.len() as u64;
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
            if staged.len() as u64 != size || kind(&staged).1 != Some(recipe.output_format) {
                return Err(Error::Goal("Candidate failed size or format verification".into()));
            }
            let Decoded { image, metadata, orientation_applied } = decode(&staged, recipe.output_format)?;
            if !metadata.is_empty() || orientation_applied {
                return Err(Error::Goal("Candidate retained unexpected metadata".into()));
            }
            let observed = image.into_rgba8();
            if observed.dimensions() != dimensions {
                return Err(Error::Goal("Output dimensions changed unexpectedly".into()));
            }
            let mse = measure(&expected, &observed, control, deadline)?;
            if recipe.output_format != Format::Jpeg && mse != 0.0 {
                return Err(Error::Goal("Lossless output changed decoded pixels".into()));
            }
            let checksum = digest(&staged);
            let candidate = Candidate { temp, size, checksum, quality, mse };
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
        candidates.sort_by(|a, b| a.size.cmp(&b.size).then_with(|| a.mse.total_cmp(&b.mse)));
        let mut best_error = f64::INFINITY;
        candidates.retain(|candidate| {
            let improved = candidate.mse < best_error;
            if improved { best_error = candidate.mse; }
            improved
        });
        Ok(Plans {
            input, checksum, dimensions, orientation_applied: decoded.orientation_applied,
            metadata: decoded.metadata, candidates, duration_ms: elapsed_ms(started),
        })
    })().map_err(|error| file_error(&path, error))
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
                let gain = (current.mse - candidate.mse) / plan.candidates[0].mse;
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
