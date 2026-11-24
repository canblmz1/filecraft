use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, RwLock},
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::{
    Capability, Error, Format, Result,
    inspect::{Decoded, decode as decode_raster},
    recipe::MAX_IMAGE_BYTES,
};

pub(crate) const MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_SECONDS: f64 = 180.0;
const MAX_FRAMES: u64 = 10_800;
const MAX_LOG_BYTES: usize = 256 * 1024;

/// One encoding attempt: the shorter displayed side is limited to
/// `short_side` pixels and x264 runs at `crf`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Rung {
    pub short_side: u32,
    pub crf: u8,
}

pub(crate) const LADDER: [Rung; 3] = [
    Rung {
        short_side: 1080,
        crf: 23,
    },
    Rung {
        short_side: 1080,
        crf: 29,
    },
    Rung {
        short_side: 720,
        crf: 31,
    },
];

/// Relative quality loss used to rank candidates of the same clip.
pub(crate) fn penalty(crf: u8, size: (u32, u32)) -> f64 {
    let short = f64::from(size.0.min(size.1).max(1));
    f64::from(crf.saturating_sub(17)) + (1080.0 / short - 1.0).max(0.0) * 12.0
}

pub(crate) struct VideoTools {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    source: &'static str,
    version: String,
    zscale: bool,
    hevc: bool,
}

/// Where FileCraft found FFmpeg and what it can do.
#[derive(Debug, Clone, Serialize)]
pub struct EngineStatus {
    pub available: bool,
    pub source: Option<&'static str>,
    pub ffmpeg: Option<PathBuf>,
    pub version: Option<String>,
    pub hdr_tone_mapping: bool,
    pub heic: bool,
    pub managed_directory: Option<PathBuf>,
    pub installable: bool,
    pub install_bytes: Option<u64>,
}

static TOOLS: RwLock<Option<Option<Arc<VideoTools>>>> = RwLock::new(None);

pub(crate) fn tools() -> Option<Arc<VideoTools>> {
    if let Some(cached) = TOOLS.read().ok().and_then(|cache| cache.clone()) {
        return cached;
    }
    let found = locate().map(Arc::new);
    if let Ok(mut cache) = TOOLS.write() {
        *cache = Some(found.clone());
    }
    found
}

/// Forget the cached FFmpeg location so the next job searches again.
pub fn refresh_video_engine() {
    if let Ok(mut cache) = TOOLS.write() {
        *cache = None;
    }
}

pub fn video_engine() -> EngineStatus {
    let tools = tools();
    EngineStatus {
        available: tools.is_some(),
        source: tools.as_ref().map(|tools| tools.source),
        ffmpeg: tools.as_ref().map(|tools| tools.ffmpeg.clone()),
        version: tools.as_ref().map(|tools| tools.version.clone()),
        hdr_tone_mapping: tools.as_ref().is_some_and(|tools| tools.zscale),
        heic: tools.as_ref().is_some_and(|tools| tools.hevc),
        managed_directory: crate::engine::managed_dir(),
        installable: crate::engine::PACKAGE.is_some(),
        install_bytes: crate::engine::PACKAGE.map(|package| package.bytes),
    }
}

fn executable(directory: &Path, name: &str) -> PathBuf {
    directory.join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

fn locate() -> Option<VideoTools> {
    let mut candidates = Vec::new();
    if let Some(directory) = std::env::var_os("FILECRAFT_FFMPEG_DIR").map(PathBuf::from)
        && directory.is_absolute()
    {
        candidates.push((directory, "environment"));
    }
    if let Some(directory) = crate::engine::managed_bin() {
        candidates.push((directory, "managed"));
    }
    if let Some(search) = std::env::var_os("PATH") {
        candidates.extend(
            std::env::split_paths(&search)
                .filter(|directory| directory.is_absolute())
                .map(|directory| (directory, "path")),
        );
    }
    candidates.into_iter().find_map(|(directory, source)| {
        let ffmpeg = executable(&directory, "ffmpeg");
        let ffprobe = executable(&directory, "ffprobe");
        (ffmpeg.is_file() && ffprobe.is_file())
            .then(|| validate(ffmpeg, ffprobe, source))
            .flatten()
    })
}

fn listing(ffmpeg: &Path, argument: &str) -> Option<String> {
    let mut command = Command::new(ffmpeg);
    command.args(["-hide_banner", argument]);
    let output = execute(&mut command, true, Duration::from_secs(10), &|| Ok(())).ok()?;
    String::from_utf8(output).ok()
}

fn listed(listing: &str, name: &str) -> bool {
    listing
        .lines()
        .any(|line| line.split_whitespace().nth(1) == Some(name))
}

fn validate(ffmpeg: PathBuf, ffprobe: PathBuf, source: &'static str) -> Option<VideoTools> {
    let encoders = listing(&ffmpeg, "-encoders")?;
    if !["libx264", "aac", "png"]
        .iter()
        .all(|name| listed(&encoders, name))
    {
        return None;
    }
    let filters = listing(&ffmpeg, "-filters")?;
    let decoders = listing(&ffmpeg, "-decoders")?;
    let version = listing(&ffmpeg, "-version")?
        .lines()
        .next()?
        .trim()
        .to_owned();
    execute(
        Command::new(&ffprobe).arg("-version"),
        true,
        Duration::from_secs(10),
        &|| Ok(()),
    )
    .ok()?;
    Some(VideoTools {
        zscale: listed(&filters, "zscale") && listed(&filters, "tonemap"),
        // HEIF tile grids are assembled by FFmpeg 7.1 and later.
        hevc: listed(&decoders, "hevc") && at_least(&version, (7, 1)),
        ffmpeg,
        ffprobe,
        source,
        version,
    })
}

/// Compare an `ffmpeg version …` banner with a release. Snapshot builds count as new.
fn at_least(banner: &str, release: (u32, u32)) -> bool {
    let Some(version) = banner
        .strip_prefix("ffmpeg version ")
        .and_then(|rest| rest.split_whitespace().next())
    else {
        return false;
    };
    let version = version.trim_start_matches('n');
    if version.starts_with("N-") {
        return true;
    }
    let mut parts = version
        .split(|c: char| !c.is_ascii_digit())
        .map(|part| part.parse::<u32>().ok());
    match (parts.next().flatten(), parts.next().flatten()) {
        (Some(year), _) if year >= 2000 => true,
        (Some(major), minor) => (major, minor.unwrap_or(0)) >= release,
        _ => false,
    }
}

pub(crate) fn capability() -> Capability {
    Capability {
        interface_version: 1,
        id: "ffmpeg-video",
        version: env!("CARGO_PKG_VERSION"),
        license: "MIT (adapter); FFmpeg license depends on the build",
        input_formats: &[Format::Mp4, Format::Mov],
        output_formats: &[Format::Mp4],
        operations: &[
            "mixed_batch",
            "h264_mp4_compression",
            "hevc_mov_to_h264_mp4",
            "rotation_correction",
            "hdr_to_sdr_tone_mapping",
        ],
        constraints: &[
            "H.264 or HEVC, 8- or 10-bit 4:2:0, in MP4 or MOV",
            "Up to 4096 px wide, 60 fps and 180 seconds",
            "HLG and PQ HDR are tone-mapped to SDR, which needs FFmpeg with zscale",
            "Outputs H.264/AAC MP4 at up to 1080p; metadata, data and subtitle tracks are dropped",
        ],
        validation: &[
            "SHA-256",
            "stream type and codec",
            "rotation-corrected dimensions",
            "video frame count",
            "duration and audio channels",
            "complete decode",
        ],
        external_dependencies: &["ffmpeg (libx264, AAC, zscale)", "ffprobe"],
        max_pixels: 4096 * 2304,
        max_input_bytes: MAX_BYTES,
        cancellation: "terminates the active child process",
        available: tools().is_some(),
    }
}

pub(crate) fn heif_capability() -> Capability {
    Capability {
        interface_version: 1,
        id: "ffmpeg-heif",
        version: env!("CARGO_PKG_VERSION"),
        license: "MIT (adapter); FFmpeg license depends on the build",
        input_formats: &[Format::Heic],
        output_formats: &[Format::Jpeg, Format::Png, Format::Webp],
        operations: &[
            "decode_heic_tile_grids",
            "rotation_correction",
            "display_p3_to_srgb",
        ],
        constraints: &[
            "HEVC-coded HEIF stills, including tile grids",
            "SDR only; Display P3 and BT.2020 primaries are converted to sRGB unless an ICC profile is embedded",
        ],
        validation: &["decoded size", "SHA-256", "RGB PSNR of the encoded output"],
        external_dependencies: &["ffmpeg (HEVC decoder)", "ffprobe"],
        max_pixels: crate::recipe::MAX_PIXELS,
        max_input_bytes: MAX_IMAGE_BYTES,
        cancellation: "terminates the active child process",
        available: tools().is_some_and(|tools| tools.hevc),
    }
}

fn execute(
    command: &mut Command,
    capture: bool,
    timeout: Duration,
    check: &impl Fn() -> Result<()>,
) -> Result<Vec<u8>> {
    check()?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
        .env_remove("FFREPORT")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(if capture {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    let mut child = command.spawn()?;
    let reader = child.stdout.take().map(|stdout| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout
                .take(MAX_LOG_BYTES as u64 + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() > MAX_LOG_BYTES {
                return Err(std::io::Error::other("FFmpeg response exceeded its limit"));
            }
            Ok(bytes)
        })
    });
    let start = Instant::now();
    loop {
        let status = check().and_then(|()| {
            if start.elapsed() >= timeout {
                return Err(Error::Timeout);
            }
            Ok(child.try_wait()?)
        });
        match status {
            Ok(Some(result)) => {
                if !result.success() {
                    return Err(Error::Goal(format!(
                        "The video engine exited with {result}"
                    )));
                }
                return match reader {
                    Some(reader) => Ok(reader
                        .join()
                        .map_err(|_| std::io::Error::other("Video response reader stopped"))??),
                    None => Ok(Vec::new()),
                };
            }
            Ok(None) => thread::sleep(Duration::from_millis(30)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        }
    }
}

const LOCAL_INPUT: &[&str] = &[
    "-protocol_whitelist",
    "file",
    "-f",
    "mov",
    "-enable_drefs",
    "0",
    "-use_absolute_path",
    "0",
];
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);
const HDR_TRANSFERS: [&str; 2] = ["smpte2084", "arib-std-b67"];
const SDR_TRANSFERS: [&str; 6] = [
    "bt709",
    "smpte170m",
    "bt470bg",
    "iec61966-2-1",
    "bt2020-10",
    "unknown",
];
const VIDEO_SIDE_DATA: [&str; 5] = [
    "Display Matrix",
    "DOVI configuration record",
    "Mastering display metadata",
    "Content light level metadata",
    "Ambient viewing environment",
];
const TONE_MAP: &str = "zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv";

#[derive(Deserialize)]
struct Probe {
    streams: Vec<Stream>,
    #[serde(default)]
    chapters: Vec<serde_json::Value>,
    #[serde(default)]
    stream_groups: Vec<StreamGroup>,
}

#[derive(Deserialize)]
struct StreamGroup {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    components: Vec<GroupComponent>,
}

#[derive(Deserialize)]
struct GroupComponent {
    width: Option<u32>,
    height: Option<u32>,
}

#[derive(Deserialize)]
struct Stream {
    index: usize,
    codec_type: String,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    pix_fmt: Option<String>,
    sample_aspect_ratio: Option<String>,
    avg_frame_rate: Option<String>,
    field_order: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
    duration: Option<String>,
    start_time: Option<String>,
    nb_frames: Option<String>,
    channels: Option<u32>,
    sample_rate: Option<String>,
    disposition: Option<Disposition>,
    #[serde(default)]
    side_data_list: Vec<SideData>,
}

#[derive(Deserialize)]
struct Disposition {
    attached_pic: Option<u8>,
}

#[derive(Deserialize)]
struct SideData {
    side_data_type: Option<String>,
    rotation: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
struct Audio {
    channels: u32,
    sample_rate: u32,
    seconds: f64,
    offset: f64,
}

impl Audio {
    fn output_channels(self) -> u32 {
        self.channels.min(2)
    }

    fn output_rate(self) -> u32 {
        if matches!(self.sample_rate, 44_100 | 48_000) {
            self.sample_rate
        } else {
            48_000
        }
    }
}

pub(crate) struct VideoInfo {
    /// Displayed width and height after rotation.
    pub dimensions: (u32, u32),
    pub seconds: f64,
    pub hdr: bool,
    frames: u64,
    audio: Option<Audio>,
    compatible: bool,
    rotated: bool,
    video_index: usize,
    audio_index: Option<usize>,
}

impl VideoInfo {
    /// Even output size whose shorter side is at most `short_side`.
    pub(crate) fn target_size(&self, short_side: u32) -> (u32, u32) {
        let (width, height) = self.dimensions;
        let short = width.min(height);
        if short <= short_side {
            return (width, height);
        }
        let scale = f64::from(short_side) / f64::from(short);
        let even = |value: u32| (((f64::from(value) * scale) / 2.0).round() as u32 * 2).max(2);
        (even(width), even(height))
    }

    pub(crate) fn verify_encoded(&self, output: &Self, size: (u32, u32)) -> Result<()> {
        if output.dimensions != size
            || self.frames != output.frames
            || (self.seconds - output.seconds).abs() > 0.1
            || !output.compatible
            || output.rotated
            || !match (self.audio, output.audio) {
                (None, None) => true,
                (Some(input), Some(encoded)) => {
                    encoded.channels == input.output_channels()
                        && encoded.sample_rate == input.output_rate()
                        && (input.seconds - encoded.seconds).abs() <= 0.15
                        && (input.offset - encoded.offset).abs() <= 0.15
                }
                _ => false,
            }
        {
            return Err(Error::Goal("Encoded video changed its frame count, dimensions, duration, audio streams or expected codecs".into()));
        }
        Ok(())
    }
}

fn unsupported() -> Error {
    Error::Unsupported("Choose an H.264 or HEVC clip in MP4 or MOV with 4:2:0 color, up to 4096 px, 60 fps and 180 seconds, with a known duration and frame count".into())
}

fn threads() -> String {
    let cores = thread::available_parallelism().map_or(2, usize::from);
    (cores / 2).clamp(2, 8).to_string()
}

impl VideoTools {
    /// `groups` asks for stream groups, which only FFprobe 7 and later report.
    fn probe_streams(
        &self,
        path: &Path,
        groups: bool,
        check: &impl Fn() -> Result<()>,
    ) -> Result<Probe> {
        let mut command = Command::new(&self.ffprobe);
        command
            .args(["-v", "error"])
            .args(LOCAL_INPUT)
            .args(["-show_streams", "-show_chapters"]);
        if groups {
            command.arg("-show_stream_groups");
        }
        command.args(["-of", "json", "-i"]).arg(path);
        let response = execute(&mut command, true, PROBE_TIMEOUT, check)?;
        serde_json::from_slice(&response)
            .map_err(|_| Error::Invalid("FFprobe could not describe the media streams".into()))
    }

    pub(crate) fn probe(&self, path: &Path, check: &impl Fn() -> Result<()>) -> Result<VideoInfo> {
        let probe = self.probe_streams(path, false, check)?;
        let video = probe
            .streams
            .iter()
            .find(|stream| {
                stream.codec_type == "video"
                    && stream
                        .disposition
                        .as_ref()
                        .and_then(|value| value.attached_pic)
                        .unwrap_or(0)
                        == 0
            })
            .ok_or_else(unsupported)?;
        let audio = probe
            .streams
            .iter()
            .find(|stream| stream.codec_type == "audio");
        if !probe.chapters.is_empty() {
            return Err(unsupported());
        }
        let width = video.width.ok_or_else(unsupported)?;
        let height = video.height.ok_or_else(unsupported)?;
        let frames: u64 = number(video.nb_frames.as_deref()).ok_or_else(unsupported)?;
        let seconds = seconds(video.duration.as_deref()).ok_or_else(unsupported)?;
        let start = number::<f64>(video.start_time.as_deref())
            .filter(|value| value.is_finite())
            .ok_or_else(unsupported)?;
        let (num, den) = video
            .avg_frame_rate
            .as_deref()
            .and_then(|rate| rate.split_once('/'))
            .ok_or_else(unsupported)?;
        let fps = num.parse::<f64>().map_err(|_| unsupported())?
            / den.parse::<f64>().map_err(|_| unsupported())?;
        let mut rotation = 0.0;
        for side in &video.side_data_list {
            let kind = side.side_data_type.as_deref().unwrap_or_default();
            if !VIDEO_SIDE_DATA.contains(&kind) {
                return Err(unsupported());
            }
            if kind == "Display Matrix" {
                rotation = side.rotation.unwrap_or(0.0);
            }
        }
        let quarter_turns = (rotation / 90.0).round();
        if (rotation - quarter_turns * 90.0).abs() > 0.5 {
            return Err(unsupported());
        }
        let rotated = quarter_turns.rem_euclid(4.0) != 0.0;
        let sideways = quarter_turns.rem_euclid(2.0) != 0.0;
        let transfer = video.color_transfer.as_deref().unwrap_or("unknown");
        let hdr = HDR_TRANSFERS.contains(&transfer);
        if width == 0
            || height == 0
            || width % 2 != 0
            || height % 2 != 0
            || width.max(height) > 4096
            || width.min(height) > 2304
            || seconds > MAX_SECONDS
            || frames == 0
            || frames > MAX_FRAMES
            || !fps.is_finite()
            || !(1.0..=60.5).contains(&fps)
            || !matches!(video.codec_name.as_deref(), Some("h264" | "hevc"))
            || !matches!(
                video.pix_fmt.as_deref(),
                Some("yuv420p" | "yuvj420p" | "yuv420p10le")
            )
            || !matches!(
                video.sample_aspect_ratio.as_deref(),
                None | Some("1:1" | "0:1")
            )
            || !matches!(
                video.field_order.as_deref(),
                None | Some("progressive" | "unknown")
            )
            || !(hdr || SDR_TRANSFERS.contains(&transfer))
            || (!hdr && video.color_primaries.as_deref() == Some("bt2020"))
        {
            return Err(unsupported());
        }
        if hdr && !self.zscale {
            return Err(Error::Unsupported(
                "HDR video needs an FFmpeg build with zscale for tone mapping. Install the FileCraft video engine."
                    .into(),
            ));
        }
        let compatible = video.codec_name.as_deref() == Some("h264")
            && video.pix_fmt.as_deref() == Some("yuv420p")
            && !hdr
            && audio.is_none_or(|stream| stream.codec_name.as_deref() == Some("aac"));
        let audio_info = if let Some(stream) = audio {
            let channels = stream
                .channels
                .filter(|count| (1..=8).contains(count))
                .ok_or_else(unsupported)?;
            let sample_rate = number(stream.sample_rate.as_deref())
                .filter(|rate| (8_000..=96_000).contains(rate))
                .ok_or_else(unsupported)?;
            let time = self::seconds(stream.duration.as_deref()).ok_or_else(unsupported)?;
            let offset = number::<f64>(stream.start_time.as_deref())
                .filter(|value| value.is_finite())
                .ok_or_else(unsupported)?
                - start;
            if time > MAX_SECONDS + 0.5 || offset.abs() > MAX_SECONDS {
                return Err(unsupported());
            }
            Some(Audio {
                channels,
                sample_rate,
                seconds: time,
                offset,
            })
        } else {
            None
        };
        Ok(VideoInfo {
            dimensions: if sideways {
                (height, width)
            } else {
                (width, height)
            },
            seconds,
            hdr,
            frames,
            audio: audio_info,
            compatible,
            rotated,
            video_index: video.index,
            audio_index: audio.map(|stream| stream.index),
        })
    }

    pub(crate) fn decode(
        &self,
        path: &Path,
        info: &VideoInfo,
        check: &impl Fn() -> Result<()>,
    ) -> Result<()> {
        let mut command = self.input_command(path);
        command.arg("-map").arg(format!("0:{}", info.video_index));
        if let Some(index) = info.audio_index {
            command.arg("-map").arg(format!("0:{index}"));
        }
        command.args([
            "-fps_mode",
            "passthrough",
            "-progress",
            "pipe:1",
            "-stats_period",
            "10",
            "-f",
            "null",
            "-",
        ]);
        let progress = execute(&mut command, true, timeout(info), check)?;
        let progress = String::from_utf8_lossy(&progress);
        let frames = progress
            .lines()
            .filter_map(|line| line.strip_prefix("frame="))
            .next_back();
        if progress.lines().next_back() != Some("progress=end")
            || number::<u64>(frames) != Some(info.frames)
        {
            return Err(Error::Goal(
                "The decoded video frame count differs from the video track".into(),
            ));
        }
        Ok(())
    }

    /// Encode one rung to H.264/AAC MP4, stopping at `bytes`.
    pub(crate) fn encode(
        &self,
        input: &Path,
        output: &Path,
        info: &VideoInfo,
        rung: Rung,
        bytes: u64,
        check: &impl Fn() -> Result<()>,
    ) -> Result<(u32, u32)> {
        let size = info.target_size(rung.short_side);
        let mut filters = Vec::new();
        if info.hdr {
            filters.push(TONE_MAP.to_owned());
        }
        filters.push(format!(
            "scale={}:{}:flags=lanczos:out_range=tv,format=yuv420p",
            size.0, size.1
        ));
        let mut command = self.input_command(input);
        command.arg("-map").arg(format!("0:{}", info.video_index));
        if let Some(index) = info.audio_index {
            command.arg("-map").arg(format!("0:{index}"));
        }
        command
            .args([
                "-map_metadata",
                "-1",
                "-map_metadata:s:v",
                "-1",
                "-map_metadata:s:a",
                "-1",
                "-map_chapters",
                "-1",
                "-sn",
                "-dn",
                "-vf",
            ])
            .arg(filters.join(","))
            .args(["-c:v", "libx264", "-threads:v"])
            .arg(threads())
            .args(["-preset", "fast", "-crf"])
            .arg(rung.crf.to_string())
            .args([
                "-profile:v",
                "high",
                "-fps_mode",
                "passthrough",
                "-pix_fmt",
                "yuv420p",
                "-color_range",
                "tv",
            ]);
        if info.hdr {
            command.args([
                "-color_primaries",
                "bt709",
                "-color_trc",
                "bt709",
                "-colorspace",
                "bt709",
            ]);
        }
        if let Some(audio) = info.audio {
            command
                .args(["-c:a", "aac", "-b:a", "128k", "-ac"])
                .arg(audio.output_channels().to_string())
                .arg("-ar")
                .arg(audio.output_rate().to_string());
        }
        command
            .args(["-movflags", "+faststart", "-f", "mp4", "-fs"])
            .arg(bytes.to_string())
            .args(["-y"])
            .arg(output);
        execute(&mut command, false, timeout(info), check)?;
        Ok(size)
    }

    /// Decode a HEIC still, including tile grids and rotation, to sRGB pixels.
    pub(crate) fn decode_still(
        &self,
        path: &Path,
        workspace: &Path,
        check: &impl Fn() -> Result<()>,
    ) -> Result<Decoded> {
        if !self.hevc {
            return Err(Error::Unsupported(
                "This FFmpeg build cannot decode HEVC images".into(),
            ));
        }
        let probe = self.probe_streams(path, true, check)?;
        let first = probe
            .streams
            .iter()
            .find(|stream| stream.codec_type == "video")
            .ok_or_else(|| Error::Unsupported("The HEIC file has no image".into()))?;
        if first.codec_name.as_deref() != Some("hevc") {
            return Err(Error::Unsupported(
                "Only HEVC-coded HEIF images are supported".into(),
            ));
        }
        if HDR_TRANSFERS.contains(&first.color_transfer.as_deref().unwrap_or("unknown")) {
            return Err(Error::Unsupported(
                "HDR HEIF stills need a tone-mapping conversion".into(),
            ));
        }
        let grid = probe
            .stream_groups
            .iter()
            .find(|group| group.kind.as_deref() == Some("Tile Grid"))
            .and_then(|group| group.components.first())
            .and_then(|grid| Some((grid.width?, grid.height?)));
        let images = probe
            .streams
            .iter()
            .filter(|stream| stream.codec_type == "video")
            .count();
        if grid.is_none() && images > 1 {
            return Err(Error::Unsupported(
                "This HEIC layout is not supported by the installed FFmpeg".into(),
            ));
        }
        let expected = grid.or(first.width.zip(first.height));
        let output = tempfile::Builder::new()
            .prefix(".heic-")
            .suffix(".png")
            .tempfile_in(workspace)?;
        let mut command = self.input_command(path);
        command
            .args([
                "-frames:v",
                "1",
                "-update",
                "1",
                "-pix_fmt",
                "rgb24",
                "-c:v",
                "png",
                "-f",
                "image2",
                "-y",
            ])
            .arg(output.path());
        execute(&mut command, false, PROBE_TIMEOUT, check)?;
        let mut png = Vec::new();
        std::fs::File::open(output.path())?
            .take(MAX_IMAGE_BYTES + 1)
            .read_to_end(&mut png)?;
        if png.len() as u64 > MAX_IMAGE_BYTES {
            return Err(Error::Unsupported(
                "The decoded HEIC image is too large".into(),
            ));
        }
        let mut decoded = decode_raster(&png, Format::Png)?;
        let (width, height) = (decoded.image.width(), decoded.image.height());
        if expected.is_some_and(|size| size != (width, height) && size != (height, width)) {
            return Err(Error::Goal(
                "The decoded HEIC size differs from its image grid".into(),
            ));
        }
        if decoded.icc.is_none() {
            let matrix = match first.color_primaries.as_deref() {
                Some("smpte432") => Some(&P3_TO_SRGB),
                Some("bt2020") => Some(&BT2020_TO_SRGB),
                _ => None,
            };
            if let Some(matrix) = matrix {
                let mut rgb = decoded.image.into_rgb8();
                convert_primaries(&mut rgb, matrix);
                decoded.image = rgb.into();
            }
        }
        decoded.metadata = Vec::new();
        decoded.orientation_applied = false;
        Ok(decoded)
    }

    fn input_command(&self, input: &Path) -> Command {
        let mut command = Command::new(&self.ffmpeg);
        command
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-xerror",
                "-max_alloc",
                "536870912",
                "-filter_threads",
                "2",
                "-threads",
            ])
            .arg(threads())
            .args(LOCAL_INPUT)
            .arg("-i")
            .arg(input);
        command
    }
}

fn timeout(info: &VideoInfo) -> Duration {
    Duration::from_secs_f64(120.0 + info.seconds * 20.0)
}

type Primaries = [[f32; 3]; 3];
const P3_TO_SRGB: Primaries = [
    [1.224_940_1, -0.224_940_4, 0.0],
    [-0.042_056_9, 1.042_057_1, 0.0],
    [-0.019_637_6, -0.078_636_1, 1.098_273_5],
];
const BT2020_TO_SRGB: Primaries = [
    [1.660_491, -0.587_641_1, -0.072_849_9],
    [-0.124_550_5, 1.132_899_9, -0.008_349_4],
    [-0.018_150_8, -0.100_578_9, 1.118_729_7],
];

/// Convert sRGB-encoded samples between linear RGB primaries, clipping to the sRGB gamut.
fn convert_primaries(image: &mut image::RgbImage, matrix: &Primaries) {
    let linear: Vec<f32> = (0..=255)
        .map(|value| {
            let c = f32::from(value as u8) / 255.0;
            if c <= 0.040_45 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        })
        .collect();
    let encode = |c: f32| {
        let c = c.clamp(0.0, 1.0);
        let c = if c <= 0.003_130_8 {
            c * 12.92
        } else {
            1.055 * c.powf(1.0 / 2.4) - 0.055
        };
        (c * 255.0).round() as u8
    };
    for pixel in image.pixels_mut() {
        let [r, g, b] = pixel.0.map(|value| linear[usize::from(value)]);
        pixel.0 = matrix.map(|row| encode(row[0] * r + row[1] * g + row[2] * b));
    }
}

fn number<T: std::str::FromStr>(value: Option<&str>) -> Option<T> {
    value?.parse().ok()
}

fn seconds(value: Option<&str>) -> Option<f64> {
    number::<f64>(value).filter(|value| value.is_finite() && *value > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heif_grids_need_ffmpeg_7_1() {
        assert!(at_least(
            "ffmpeg version 8.1-full_build-www.gyan.dev Copyright",
            (7, 1)
        ));
        assert!(at_least("ffmpeg version n7.1.1 Copyright", (7, 1)));
        assert!(at_least(
            "ffmpeg version 2026-11-23-git-05c674c913-essentials",
            (7, 1)
        ));
        assert!(at_least("ffmpeg version N-118000-g1234abcd", (7, 1)));
        assert!(!at_least("ffmpeg version 6.1.1-3ubuntu5 Copyright", (7, 1)));
        assert!(!at_least("ffmpeg version 7.0.2", (7, 1)));
        assert!(!at_least("not ffmpeg", (7, 1)));
    }

    #[test]
    fn display_p3_red_is_clipped_into_srgb() {
        let mut image = image::RgbImage::from_pixel(1, 1, image::Rgb([180, 90, 90]));
        convert_primaries(&mut image, &P3_TO_SRGB);
        let [red, green, blue] = image.get_pixel(0, 0).0;
        assert!(
            (191..=195).contains(&red) && (81..=85).contains(&green) && (85..=89).contains(&blue)
        );
        let mut gray = image::RgbImage::from_pixel(1, 1, image::Rgb([128, 128, 128]));
        convert_primaries(&mut gray, &P3_TO_SRGB);
        assert_eq!(gray.get_pixel(0, 0).0, [128, 128, 128]);
    }
}
