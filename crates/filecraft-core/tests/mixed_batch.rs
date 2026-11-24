use std::{fs, path::Path, process::Command};

use filecraft_core::{Control, Format, Recipe, Status, capabilities, inspect, run};
use image::{Rgb, RgbImage};
use lopdf::{Document, Object, Stream, dictionary};
use tempfile::tempdir;

fn pdf_fixture(path: &Path, protected: bool) -> Vec<u8> {
    let mut document = Document::with_version("1.5");
    let pages = document.new_object_id();
    let mut contents = Vec::new();
    for _ in 0..12_000 {
        contents.extend_from_slice(b"q 1 0 0 1 0 0 cm 10 10 20 20 re S Q\n");
    }
    let stream = document.add_object(Stream::new(dictionary! {}, contents));
    let page = document.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages, "Contents" => stream,
        "MediaBox" => vec![0.into(), 0.into(), 600.into(), 800.into()],
        "Resources" => dictionary! {},
    });
    document.objects.insert(
        pages,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Count" => 1, "Kids" => vec![Object::Reference(page)],
        }),
    );
    let mut catalog = dictionary! { "Type" => "Catalog", "Pages" => pages };
    if protected {
        catalog.set(
            "Perms",
            dictionary! { "DocMDP" => dictionary! { "Type" => "Sig" } },
        );
    }
    let root = document.add_object(catalog);
    document.trailer.set("Root", root);
    let mut bytes = Vec::new();
    document.save_to(&mut bytes).unwrap();
    fs::write(path, &bytes).unwrap();
    bytes
}

fn recipe(bytes: u64) -> Recipe {
    serde_json::from_value(serde_json::json!({
        "version": 1, "output_format": "mixed", "max_batch_bytes": bytes
    }))
    .unwrap()
}

#[test]
fn native_pdf_and_image_share_a_verified_budget() {
    let inputs = tempdir().unwrap();
    let out = tempdir().unwrap();
    let pdf = inputs.path().join("forms.pdf");
    let original = pdf_fixture(&pdf, false);
    let photo = inputs.path().join("photo.png");
    RgbImage::from_pixel(100, 60, Rgb([33, 11, 190]))
        .save(&photo)
        .unwrap();
    let report = run(
        &[pdf.clone(), photo],
        out.path(),
        &recipe(5_000),
        &Control::default(),
        |_| {},
    );
    assert_eq!(report.status, Status::Completed, "{:?}", report.error);
    assert_eq!(report.files.len(), 2);
    assert!(report.output_bytes <= 5_000);
    let verified = &report.files[0];
    assert_eq!(verified.pdf_pages, Some(1));
    assert!(!verified.unchanged);
    assert!(verified.output_bytes < verified.input_bytes);
    assert!(
        verified
            .verified_checks
            .contains(&"changed streams decode to original bytes")
    );
    assert_eq!(fs::read(pdf).unwrap(), original);
}

#[test]
fn protected_pdf_is_never_rewritten_to_meet_a_goal() {
    let inputs = tempdir().unwrap();
    let out = tempdir().unwrap();
    let pdf = inputs.path().join("signed.pdf");
    let original = pdf_fixture(&pdf, true);
    let report = run(
        std::slice::from_ref(&pdf),
        out.path(),
        &recipe(original.len() as u64),
        &Control::default(),
        |_| {},
    );
    assert_eq!(report.status, Status::Completed, "{:?}", report.error);
    assert!(report.files[0].unchanged);
    assert_eq!(fs::read(&report.files[0].output_path).unwrap(), original);
    let tight = tempdir().unwrap();
    let rejected = run(
        &[pdf],
        tight.path(),
        &recipe(original.len() as u64 - 1),
        &Control::default(),
        |_| {},
    );
    assert_eq!(rejected.status, Status::Failed);
    assert_eq!(tight.path().read_dir().unwrap().count(), 0);
}

#[test]
fn real_mp4_pdf_and_image_share_a_budget_when_ffmpeg_is_available() {
    if !capabilities()
        .iter()
        .any(|engine| engine.id == "ffmpeg-video" && engine.available)
    {
        assert_ne!(
            std::env::var("FILECRAFT_TEST_REQUIRE_FFMPEG").as_deref(),
            Ok("1"),
            "This environment requires the FFmpeg integration test"
        );
        eprintln!("FFmpeg/FFprobe not installed, skipping the external encoder fixture");
        return;
    }

    let inputs = tempdir().unwrap();
    let out = tempdir().unwrap();
    let pdf = inputs.path().join("forms.pdf");
    pdf_fixture(&pdf, false);
    let photo = inputs.path().join("photo.png");
    RgbImage::from_pixel(120, 80, Rgb([51, 100, 190]))
        .save(&photo)
        .unwrap();
    let video = inputs.path().join("clip.mp4");
    let status = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=24",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=500:sample_rate=44100",
            "-t",
            "2",
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "libx264",
            "-threads:v",
            "2",
            "-crf",
            "8",
            "-c:a",
            "aac",
            "-b:a",
            "192k",
        ])
        .arg(&video)
        .status()
        .unwrap();
    assert!(status.success());
    let original_video = inspect(&video).unwrap();
    let cap = original_video.bytes / 2;
    let paths = [photo, pdf, video.clone()];
    let report = run(
        &paths,
        out.path(),
        &recipe(cap),
        &Control::default(),
        |_| {},
    );
    assert_eq!(report.status, Status::Completed, "{:?}", report.error);
    assert_eq!(report.files.len(), 3);
    assert!(report.output_bytes <= cap && cap < original_video.bytes);
    let output = &report.files[2];
    assert_eq!(output.output_format, Format::Mp4);
    assert!(output.verified_checks.contains(&"complete decode"));
    assert!(output.video_crf.is_some());
    assert_eq!(inspect(&video).unwrap().sha256, original_video.sha256);
    assert_eq!(
        inspect(&output.output_path).unwrap().sha256,
        output.output_sha256
    );

    let impossible = tempdir().unwrap();
    let report = run(
        &paths,
        impossible.path(),
        &recipe(1),
        &Control::default(),
        |_| {},
    );
    assert_eq!(report.status, Status::Failed);
    assert_eq!(impossible.path().read_dir().unwrap().count(), 0);
}
