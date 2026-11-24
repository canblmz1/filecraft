use filecraft_core::{Control, Format, Phase, Recipe, Status, inspect, run};
use image::{Rgb, RgbImage, Rgba, RgbaImage};
use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};
use tempfile::tempdir;

fn recipe(format: Format) -> Recipe {
    serde_json::from_value(serde_json::json!({ "version": 1, "output_format": format })).unwrap()
}

fn fixture(path: &Path, seed: u32) {
    RgbImage::from_fn(128, 96, |x, y| {
        let hash = (x * 347 + y * 419 + x * y * seed) ^ (y * 53 + x * x);
        Rgb([
            (hash % 256) as u8,
            ((hash / 8) % 256) as u8,
            ((hash / 16) % 256) as u8,
        ])
    })
    .save(path)
    .unwrap();
}

#[test]
fn inspects_signature_and_verifies_lossless_transparency_without_overwriting() {
    let source = tempdir().unwrap();
    let output = tempdir().unwrap();
    let original_path = source.path().join("drawing.png");
    let input = source.path().join("drawing.jpg");
    let transparent =
        RgbaImage::from_fn(41, 28, |x, y| Rgba([x as u8, y as u8, 42, (x * y) as u8]));
    transparent.save(&original_path).unwrap();
    fs::rename(original_path, &input).unwrap();
    let original = fs::read(&input).unwrap();
    let info = inspect(&input).unwrap();
    assert_eq!(info.raster_format, Some(Format::Png));
    assert_eq!(info.alpha_channel, Some(true));
    assert!(!info.warnings.is_empty());

    let preexisting = output.path().join("drawing-filecraft.webp");
    fs::write(&preexisting, b"preserve").unwrap();
    let report = run(
        std::slice::from_ref(&input),
        output.path(),
        &recipe(Format::Webp),
        &Control::default(),
        |_| {},
    );
    assert_eq!(report.status, Status::Completed, "{:?}", report.error);
    let result = &report.files[0];
    assert_ne!(result.output_path, preexisting);
    assert!(result.pixels_identical);
    assert_eq!(
        result.output_bytes,
        fs::metadata(&result.output_path).unwrap().len()
    );
    assert_eq!(
        inspect(&result.output_path).unwrap().sha256,
        result.output_sha256
    );
    assert_eq!(
        image::open(&result.output_path).unwrap().into_rgba8(),
        transparent
    );
    assert_eq!(fs::read(&preexisting).unwrap(), b"preserve");
    assert_eq!(fs::read(&input).unwrap(), original);

    let refused = run(
        &[input],
        output.path(),
        &recipe(Format::Jpeg),
        &Control::default(),
        |_| {},
    );
    assert_eq!(refused.status, Status::Failed);
    assert!(refused.error.unwrap().contains("transparency"));
    assert_eq!(output.path().read_dir().unwrap().count(), 2);
}

#[test]
fn verifies_joint_budget_before_any_publication() {
    let source = tempdir().unwrap();
    let output = tempdir().unwrap();
    let paths = [source.path().join("a.png"), source.path().join("b.png")];
    fixture(&paths[0], 13);
    fixture(&paths[1], 37);
    let mut goal = recipe(Format::Jpeg);
    goal.jpeg_quality = Some(35);
    let baseline = run(&paths, output.path(), &goal, &Control::default(), |_| {});
    assert_eq!(baseline.status, Status::Completed, "{:?}", baseline.error);
    let low_size = baseline.output_bytes;

    let output = tempdir().unwrap();
    goal.jpeg_quality = Some(95);
    goal.max_batch_bytes = Some(low_size);
    let report = run(&paths, output.path(), &goal, &Control::default(), |_| {});
    assert_eq!(report.status, Status::Completed, "{:?}", report.error);
    assert!(report.output_bytes <= low_size);
    assert_eq!(report.files.len(), 2);
    assert!(report.files.iter().all(|file| file.psnr_db.is_some()));

    let empty_output = tempdir().unwrap();
    goal.max_file_bytes = Some(1);
    let report = run(
        &paths,
        empty_output.path(),
        &goal,
        &Control::default(),
        |_| {},
    );
    assert_eq!(report.status, Status::Failed);
    assert_eq!(report.error_code, Some("goal_unmet"));
    assert_eq!(empty_output.path().read_dir().unwrap().count(), 0);

    goal.max_file_bytes = None;
    goal.max_batch_bytes = Some(1);
    let report = run(
        &paths,
        empty_output.path(),
        &goal,
        &Control::default(),
        |_| {},
    );
    assert_eq!(report.status, Status::Failed);
    assert_eq!(report.error_code, Some("goal_unmet"));
    assert_eq!(empty_output.path().read_dir().unwrap().count(), 0);
}

#[test]
fn cancellation_before_publication_removes_candidates() {
    let source = tempdir().unwrap();
    let output = tempdir().unwrap();
    let input = source.path().join("image.png");
    fixture(&input, 13);
    let control = Control::default();
    let verified = AtomicUsize::new(0);
    let report = run(
        &[input],
        output.path(),
        &recipe(Format::Webp),
        &control,
        |progress| {
            if matches!(progress.phase, Phase::Verifying)
                && verified.fetch_add(1, Ordering::Relaxed) == 1
            {
                control.cancel();
            }
        },
    );
    assert_eq!(report.status, Status::Cancelled);
    assert_eq!(output.path().read_dir().unwrap().count(), 0);
}

#[test]
fn strips_exif_after_applying_orientation() {
    let source = tempdir().unwrap();
    let output = tempdir().unwrap();
    let input = source.path().join("oriented.jpg");
    let rgb = RgbImage::from_pixel(17, 12, Rgb([104, 80, 201]));
    rgb.save(&input).unwrap();
    let mut jpeg = fs::read(&input).unwrap();
    let metadata = b"Exif\0\0II\x2a\x00\x08\x00\x00\x00\x01\x00\x12\x01\x03\x00\x01\x00\x00\x00\x06\x00\x00\x00\x00\x00\x00\x00";
    let segment_len = (metadata.len() as u16 + 2).to_be_bytes();
    let segment = [b"\xff\xe1".as_slice(), &segment_len, metadata].concat();
    jpeg.splice(2..2, segment);
    fs::write(&input, jpeg).unwrap();
    assert_eq!(inspect(&input).unwrap().exif, Some(true));

    let report = run(
        &[input],
        output.path(),
        &recipe(Format::Png),
        &Control::default(),
        |_| {},
    );
    assert_eq!(report.status, Status::Completed, "{:?}", report.error);
    let result = &report.files[0];
    assert_eq!((result.width, result.height), (12, 17));
    assert!(result.orientation_applied);
    assert_eq!(result.metadata_removed, ["EXIF"]);
    assert_eq!(inspect(&result.output_path).unwrap().exif, Some(false));
}

#[test]
fn recipe_rejects_unknown_fields_and_conflicting_goals() {
    let unsupported = serde_json::from_str::<Recipe>(
        r#"{"version":1,"output_format":"webp","command":"invalid"}"#,
    );
    assert!(unsupported.is_err());
    let mut conflicting = recipe(Format::Jpeg);
    conflicting.lossless_only = true;
    assert!(conflicting.validate().is_err());
    conflicting.lossless_only = false;
    conflicting.version = 2;
    assert!(conflicting.validate().is_err());
}
