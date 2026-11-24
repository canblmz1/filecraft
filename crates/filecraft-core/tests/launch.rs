//! Real-world inputs: phone JPEGs with ICC profiles, scanned PDFs, HEIC
//! photos and HDR HEVC clips. All fixtures are synthetic.

use std::{fs, io::Cursor, path::Path, process::Command};

use filecraft_core::{Control, Format, Recipe, Status, capabilities, inspect, run};
use image::{
    ExtendedColorType, ImageDecoder, ImageEncoder, Rgb, RgbImage,
    codecs::jpeg::{JpegDecoder, JpegEncoder},
};
use lopdf::{Document, Object, Stream, dictionary};
use tempfile::tempdir;

fn mixed(bytes: u64) -> Recipe {
    serde_json::from_value(serde_json::json!({
        "version": 1, "output_format": "mixed", "max_batch_bytes": bytes
    }))
    .unwrap()
}

/// A minimal RGB display profile header with an empty tag table.
fn icc_profile() -> Vec<u8> {
    let mut profile = vec![0_u8; 132];
    profile[0..4].copy_from_slice(&132_u32.to_be_bytes());
    profile[12..16].copy_from_slice(b"mntr");
    profile[16..20].copy_from_slice(b"RGB ");
    profile[20..24].copy_from_slice(b"XYZ ");
    profile[36..40].copy_from_slice(b"acsp");
    profile
}

fn photo(width: u32, height: u32) -> RgbImage {
    RgbImage::from_fn(width, height, |x, y| {
        let texture = (x * 7919 + y * 104_729) % 23;
        Rgb([
            ((x * 255 / width) + texture) as u8,
            ((y * 255 / height) + texture / 2) as u8,
            (((x + y) * 127 / (width + height)) + 60 + texture) as u8,
        ])
    })
}

fn jpeg(image: &RgbImage, quality: u8, icc: Option<Vec<u8>>) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut bytes, quality);
    if let Some(icc) = icc {
        encoder.set_icc_profile(icc).unwrap();
    }
    encoder
        .write_image(
            image,
            image.width(),
            image.height(),
            ExtendedColorType::Rgb8,
        )
        .unwrap();
    bytes
}

fn embedded_icc(bytes: &[u8]) -> Option<Vec<u8>> {
    JpegDecoder::new(Cursor::new(bytes))
        .unwrap()
        .icc_profile()
        .unwrap()
}

#[test]
fn mixed_batch_reencodes_phone_jpegs_and_keeps_their_icc_profile() {
    let source = tempdir().unwrap();
    let output = tempdir().unwrap();
    let path = source.path().join("IMG_2041.jpg");
    let original = jpeg(&photo(640, 480), 98, Some(icc_profile()));
    fs::write(&path, &original).unwrap();
    assert_eq!(inspect(&path).unwrap().icc, Some(true));

    let cap = original.len() as u64 * 6 / 10;
    let report = run(
        std::slice::from_ref(&path),
        output.path(),
        &mixed(cap),
        &Control::default(),
        |_| {},
    );
    assert_eq!(report.status, Status::Completed, "{:?}", report.error);
    let file = &report.files[0];
    assert_eq!(file.output_format, Format::Jpeg);
    assert!(file.output_bytes <= cap);
    assert!(file.jpeg_quality.is_some_and(|quality| quality <= 90));
    assert!(
        file.psnr_db.is_some_and(|db| db >= 32.0),
        "{:?}",
        file.psnr_db
    );
    assert!(file.icc_profile_preserved);
    assert!(
        file.verified_checks
            .contains(&"ICC profile bytes carried over")
    );
    let written = fs::read(&file.output_path).unwrap();
    assert_eq!(embedded_icc(&written), Some(icc_profile()));
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn photos_that_already_fit_are_not_degraded() {
    let source = tempdir().unwrap();
    let output = tempdir().unwrap();
    let path = source.path().join("photo.jpg");
    let original = jpeg(&photo(320, 240), 90, None);
    fs::write(&path, &original).unwrap();
    let report = run(
        std::slice::from_ref(&path),
        output.path(),
        &mixed(original.len() as u64 * 4),
        &Control::default(),
        |_| {},
    );
    assert_eq!(report.status, Status::Completed, "{:?}", report.error);
    assert!(report.files[0].pixels_identical);
    assert!(report.files[0].psnr_db.is_none());
}

/// Two pages scanned at 300 dpi: a JPEG-coded grayscale page and an
/// uncompressed RGB page, each drawn through the page content stream.
fn scanned_pdf(path: &Path) -> Vec<u8> {
    let mut document = Document::with_version("1.5");
    let pages = document.new_object_id();
    let gray = image::GrayImage::from_fn(900, 1200, |x, y| {
        let line = (y / 24) % 3 == 0 && (x / 9 + y / 24) % 5 != 0;
        image::Luma([if line {
            40
        } else {
            235 + ((x * 31 + y * 17) % 9) as u8
        }])
    });
    let mut scan = Vec::new();
    JpegEncoder::new_with_quality(&mut scan, 95)
        .write_image(&gray, 900, 1200, ExtendedColorType::L8)
        .unwrap();
    let rgb = photo(600, 800);
    let images = [
        Stream::new(
            dictionary! {
                "Type" => "XObject", "Subtype" => "Image", "Width" => 900, "Height" => 1200,
                "ColorSpace" => "DeviceGray", "BitsPerComponent" => 8, "Filter" => "DCTDecode",
            },
            scan,
        ),
        Stream::new(
            dictionary! {
                "Type" => "XObject", "Subtype" => "Image", "Width" => 600, "Height" => 800,
                "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8,
            },
            rgb.into_raw(),
        ),
    ];
    let placements: [&[u8]; 2] = [
        b"q 216 0 0 288 72 400 cm /Im0 Do Q",
        b"q 144 0 0 192 72 400 cm /Im0 Do Q",
    ];
    let mut kids = Vec::new();
    for (image, placement) in images.into_iter().zip(placements) {
        let image = document.add_object(image);
        let content = document.add_object(Stream::new(dictionary! {}, placement.to_vec()));
        kids.push(Object::Reference(document.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages, "Contents" => content,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => image } },
        })));
    }
    document.objects.insert(
        pages,
        Object::Dictionary(dictionary! { "Type" => "Pages", "Count" => 2, "Kids" => kids }),
    );
    let catalog = document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
    document.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    document.save_to(&mut bytes).unwrap();
    fs::write(path, &bytes).unwrap();
    bytes
}

#[test]
fn scanned_pdf_pages_are_downsampled_and_verified() {
    let source = tempdir().unwrap();
    let output = tempdir().unwrap();
    let path = source.path().join("forms-scan.pdf");
    let original = scanned_pdf(&path);
    let cap = original.len() as u64 / 4;
    let report = run(
        std::slice::from_ref(&path),
        output.path(),
        &mixed(cap),
        &Control::default(),
        |_| {},
    );
    assert_eq!(report.status, Status::Completed, "{:?}", report.error);
    let file = &report.files[0];
    assert!(file.output_bytes <= cap);
    assert_eq!(file.pdf_pages, Some(2));
    let images = file
        .pdf_images
        .as_ref()
        .expect("page images were re-encoded");
    assert_eq!(images.images_reencoded, 2);
    assert!(images.min_psnr_db >= 30.0);
    assert!(
        file.verified_checks
            .contains(&"unchanged objects decode identically")
    );

    let written = Document::load(&file.output_path).unwrap();
    assert_eq!(written.get_pages().len(), 2);
    for (page, inches) in written.get_pages().into_values().zip([3.0, 2.0]) {
        let image = written
            .get_page_images(page)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let dpi = image.width as f64 / inches;
        assert!(dpi <= f64::from(images.max_dpi) + 1.0, "{dpi} dpi");
        assert_eq!(image.filters, Some(vec!["DCTDecode".to_string()]));
    }
    assert_eq!(fs::read(&path).unwrap(), original);

    let lossless = tempdir().unwrap();
    let report = run(
        std::slice::from_ref(&path),
        lossless.path(),
        &mixed(original.len() as u64),
        &Control::default(),
        |_| {},
    );
    assert_eq!(report.status, Status::Completed, "{:?}", report.error);
    assert!(report.files[0].pdf_images.is_none());
}

fn engine(id: &str) -> bool {
    capabilities()
        .iter()
        .any(|engine| engine.id == id && engine.available)
}

fn ffmpeg_lists(argument: &str, name: &str) -> bool {
    Command::new("ffmpeg")
        .args(["-hide_banner", argument])
        .output()
        .is_ok_and(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line.split_whitespace().nth(1) == Some(name))
        })
}

#[test]
fn heic_photos_become_upright_srgb_jpegs() {
    if !engine("ffmpeg-heif") {
        eprintln!("FFmpeg 7.1+ with HEVC is not installed, skipping the HEIC fixture");
        return;
    }
    let source = tempdir().unwrap();
    let output = tempdir().unwrap();
    let path = source.path().join("IMG_0007.HEIC");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rotated-p3-grid.heic");
    fs::copy(&fixture, &path).unwrap();
    let info = inspect(&path).unwrap();
    assert_eq!(info.format, Some(Format::Heic));

    let report = run(
        std::slice::from_ref(&path),
        output.path(),
        &mixed(10_000_000),
        &Control::default(),
        |_| {},
    );
    assert_eq!(report.status, Status::Completed, "{:?}", report.error);
    let file = &report.files[0];
    assert_eq!(file.output_format, Format::Jpeg);
    assert_eq!((file.width, file.height), (640, 480));
    assert_eq!(file.metadata_removed, ["EXIF"]);
    assert!(file.psnr_db.is_some_and(|db| db >= 32.0));
    let decoded = image::open(&file.output_path).unwrap().into_rgb8();
    assert_eq!(decoded.dimensions(), (640, 480));
    // The Display P3 patch (180, 90, 90) sits in the lower right after rotation.
    let [red, green, _] = decoded.get_pixel(520, 390).0;
    assert!(red >= 187 && green <= 88, "sRGB patch is {red}, {green}");
    assert_eq!(fs::read(&path).unwrap(), fs::read(&fixture).unwrap());

    let mut convert: Recipe =
        serde_json::from_value(serde_json::json!({ "version": 1, "output_format": "jpeg" }))
            .unwrap();
    convert.jpeg_quality = Some(85);
    let report = run(
        &[path],
        tempdir().unwrap().path(),
        &convert,
        &Control::default(),
        |_| {},
    );
    assert_eq!(report.status, Status::Completed, "{:?}", report.error);
}

#[test]
fn hdr_hevc_mov_becomes_upright_sdr_h264_mp4() {
    if !engine("ffmpeg-video")
        || !ffmpeg_lists("-encoders", "libx265")
        || !ffmpeg_lists("-filters", "zscale")
    {
        eprintln!("FFmpeg with libx265 and zscale is not installed, skipping the HDR MOV fixture");
        return;
    }
    let source = tempdir().unwrap();
    let flat = source.path().join("flat.mov");
    let clip = source.path().join("IMG_0042.MOV");
    let made = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x240:rate=30",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
            "-t",
            "2",
            "-c:v",
            "libx265",
            "-preset",
            "ultrafast",
            "-x265-params",
            "log-level=error:colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc",
            "-pix_fmt",
            "yuv420p10le",
            "-color_primaries",
            "bt2020",
            "-color_trc",
            "arib-std-b67",
            "-colorspace",
            "bt2020nc",
            "-tag:v",
            "hvc1",
            "-c:a",
            "aac",
            "-ac",
            "2",
            "-timecode",
            "01:00:00:00",
            "-f",
            "mov",
        ])
        .arg(&flat)
        .status()
        .unwrap()
        .success()
        && Command::new("ffmpeg")
            .args(["-v", "error", "-display_rotation", "90", "-i"])
            .arg(&flat)
            .args(["-map", "0", "-c", "copy", "-f", "mov"])
            .arg(&clip)
            .status()
            .unwrap()
            .success();
    if !made {
        eprintln!("This FFmpeg cannot write a rotated HEVC MOV, skipping the HDR MOV fixture");
        return;
    }
    let original = fs::read(&clip).unwrap();
    assert_eq!(inspect(&clip).unwrap().format, Some(Format::Mov));

    let output = tempdir().unwrap();
    let report = run(
        std::slice::from_ref(&clip),
        output.path(),
        &mixed(50_000_000),
        &Control::default(),
        |_| {},
    );
    assert_eq!(report.status, Status::Completed, "{:?}", report.error);
    let file = &report.files[0];
    assert_eq!(file.output_format, Format::Mp4);
    assert_eq!((file.width, file.height), (240, 320));
    assert!(file.video_tone_mapped);
    assert_eq!(file.video_crf, Some(23));
    assert!(
        file.unsupported_checks
            .contains(&"HDR to SDR tone mapping appearance")
    );
    let probe = Command::new("ffprobe")
        .args(["-v", "error", "-show_streams", "-of", "json"])
        .arg(&file.output_path)
        .output()
        .unwrap();
    let probe: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    let streams = probe["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 2);
    assert_eq!(streams[0]["codec_name"], "h264");
    assert_eq!(streams[0]["color_transfer"], "bt709");
    assert!(streams[0].get("side_data_list").is_none());
    assert_eq!(fs::read(&clip).unwrap(), original);

    let keep: Recipe = serde_json::from_value(serde_json::json!({
        "version": 1, "output_format": "mixed", "max_batch_bytes": 50_000_000, "lossless_only": true
    }))
    .unwrap();
    let kept = tempdir().unwrap();
    let report = run(&[clip], kept.path(), &keep, &Control::default(), |_| {});
    assert_eq!(report.status, Status::Completed, "{:?}", report.error);
    assert!(report.files[0].unchanged);
    assert_eq!(report.files[0].output_format, Format::Mov);
    assert_eq!(fs::read(&report.files[0].output_path).unwrap(), original);
}
