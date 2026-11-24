use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::OnceLock,
    thread,
    time::{Duration, Instant},
};

use serde::Deserialize;

use crate::{Capability, Error, Format, Result};

pub(crate) const MAX_BYTES: u64 = 512 * 1024 * 1024;
pub(crate) const QUALITY: [u8; 3] = [23, 29, 35];
const MAX_LOG_BYTES: usize = 256 * 1024;

pub(crate) struct VideoTools {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
}

pub(crate) fn tools() -> Option<&'static VideoTools> {
    static TOOLS: OnceLock<Option<VideoTools>> = OnceLock::new();
    TOOLS
        .get_or_init(|| {
            let ffmpeg = find("ffmpeg")?;
            let ffprobe = find("ffprobe")?;
            let mut command = Command::new(&ffmpeg);
            command.args(["-hide_banner", "-encoders"]);
            let encoders = execute(&mut command, true, Duration::from_secs(5), &|| Ok(())).ok()?;
            let encoders = String::from_utf8(encoders).ok()?;
            if !["libx264", "aac"].iter().all(|name| {
                encoders
                    .lines()
                    .any(|line| line.split_whitespace().nth(1) == Some(*name))
            }) {
                return None;
            }
            execute(
                Command::new(&ffprobe).arg("-version"),
                true,
                Duration::from_secs(5),
                &|| Ok(()),
            )
            .ok()?;
            Some(VideoTools { ffmpeg, ffprobe })
        })
        .as_ref()
}

fn find(name: &str) -> Option<PathBuf> {
    let executable = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let search = std::env::var_os("PATH")?;
    std::env::split_paths(&search)
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(&executable))
        .find(|path| path.is_file())
}

pub(crate) fn capability() -> Capability {
    Capability {
        interface_version: 1,
        id: "ffmpeg-video",
        version: env!("CARGO_PKG_VERSION"),
        license: "MIT (adapter); FFmpeg license depends on the installed build",
        input_formats: &[Format::Mp4],
        output_formats: &[Format::Mp4],
        operations: &["mixed_batch", "h264_mp4_compression"],
        constraints: &[
            "8-bit SDR MP4, one video and optional mono or stereo audio",
            "Up to 1080p, 60 fps point rate and 120 seconds",
            "No rotation, subtitles, chapters or extra tracks",
        ],
        validation: &[
            "SHA-256",
            "stream count, type and codec",
            "dimensions",
            "video frame count",
            "duration and audio channels",
            "complete decode",
        ],
        external_dependencies: &["ffmpeg (libx264 and AAC)", "ffprobe"],
        max_pixels: 1920 * 1080,
        max_input_bytes: MAX_BYTES,
        cancellation: "terminates the active child process",
        available: tools().is_some(),
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

const LOCAL_MP4: &[&str] = &[
    "-protocol_whitelist",
    "file",
    "-f",
    "mov",
    "-enable_drefs",
    "0",
    "-use_absolute_path",
    "0",
];
const TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Deserialize)]
struct Probe {
    streams: Vec<Stream>,
    chapters: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct Stream {
    codec_type: String,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    pix_fmt: Option<String>,
    sample_aspect_ratio: Option<String>,
    avg_frame_rate: Option<String>,
    field_order: Option<String>,
    color_transfer: Option<String>,
    duration: Option<String>,
    start_time: Option<String>,
    nb_frames: Option<String>,
    channels: Option<u32>,
    sample_rate: Option<String>,
    disposition: Option<Disposition>,
    #[serde(default)]
    side_data_list: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct Disposition {
    attached_pic: Option<u8>,
}

pub(crate) struct VideoInfo {
    pub dimensions: (u32, u32),
    pub seconds: f64,
    frames: u64,
    audio: Option<(u32, u32, f64, f64)>,
    compatible: bool,
}

impl VideoInfo {
    pub(crate) fn verify_encoded(&self, output: &Self) -> Result<()> {
        if self.dimensions != output.dimensions
            || self.frames != output.frames
            || (self.seconds - output.seconds).abs() > 0.05
            || !output.compatible
            || !match (self.audio, output.audio) {
                (None, None) => true,
                (Some((a, r, t, o)), Some((b, s, u, p))) => {
                    a == b && r == s && (t - u).abs() <= 0.1 && (o - p).abs() <= 0.1
                }
                _ => false,
            }
        {
            return Err(Error::Goal("Encoded MP4 changed its frame count, dimensions, duration, audio streams or expected codecs".into()));
        }
        Ok(())
    }
}

fn unsupported() -> Error {
    Error::Unsupported("Choose an SDR MP4 with one progressive video, optional mono or stereo audio, a known duration and frame count, and no extra tracks or rotation".into())
}

impl VideoTools {
    pub(crate) fn probe(&self, path: &Path, check: &impl Fn() -> Result<()>) -> Result<VideoInfo> {
        let mut command = Command::new(&self.ffprobe);
        command
            .args(["-v", "error"])
            .args(LOCAL_MP4)
            .args(["-show_streams", "-show_chapters", "-of", "json", "-i"])
            .arg(path);
        let response = execute(&mut command, true, TIMEOUT, check)?;
        let probe: Probe = serde_json::from_slice(&response)
            .map_err(|_| Error::Invalid("FFprobe could not describe the MP4 streams".into()))?;
        let video = probe
            .streams
            .iter()
            .find(|stream| stream.codec_type == "video")
            .ok_or_else(unsupported)?;
        let audio = probe
            .streams
            .iter()
            .find(|stream| stream.codec_type == "audio");
        if probe.streams.len() != 1 + usize::from(audio.is_some()) || !probe.chapters.is_empty() {
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
        if width == 0
            || height == 0
            || width % 2 != 0
            || height % 2 != 0
            || width.max(height) > 1920
            || width.min(height) > 1080
            || seconds > 120.0
            || frames == 0
            || frames > 7200
            || !fps.is_finite()
            || !(1.0..=60.0).contains(&fps)
            || video.pix_fmt.as_deref() != Some("yuv420p")
            || !matches!(video.sample_aspect_ratio.as_deref(), None | Some("1:1"))
            || !matches!(
                video.field_order.as_deref(),
                None | Some("progressive" | "unknown")
            )
            || !matches!(
                video.color_transfer.as_deref(),
                None | Some("bt709" | "smpte170m" | "bt470bg" | "iec61966-2-1" | "unknown")
            )
            || !video.side_data_list.is_empty()
            || video
                .disposition
                .as_ref()
                .and_then(|value| value.attached_pic)
                .unwrap_or(0)
                != 0
        {
            return Err(unsupported());
        }
        let mut compatible = video.codec_name.as_deref() == Some("h264");
        let audio = if let Some(stream) = audio {
            compatible &= stream.codec_name.as_deref() == Some("aac");
            let channels = stream
                .channels
                .filter(|count| (1..=2).contains(count))
                .ok_or_else(unsupported)?;
            let sample_rate = number(stream.sample_rate.as_deref())
                .filter(|rate| matches!(rate, 44100 | 48000))
                .ok_or_else(unsupported)?;
            let time = self::seconds(stream.duration.as_deref()).ok_or_else(unsupported)?;
            let start_offset = number::<f64>(stream.start_time.as_deref())
                .filter(|value| value.is_finite())
                .ok_or_else(unsupported)?
                - start;
            if time > 120.1 || start_offset.abs() > 120.0 {
                return Err(unsupported());
            }
            Some((channels, sample_rate, time, start_offset))
        } else {
            None
        };
        Ok(VideoInfo {
            dimensions: (width, height),
            seconds,
            frames,
            audio,
            compatible,
        })
    }

    pub(crate) fn decode(
        &self,
        path: &Path,
        info: &VideoInfo,
        check: &impl Fn() -> Result<()>,
    ) -> Result<()> {
        let mut command = self.input_command(path);
        command.args([
            "-map",
            "0:v:0",
            "-map",
            "0:a:0?",
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
        let progress = execute(&mut command, true, TIMEOUT, check)?;
        let progress = String::from_utf8_lossy(&progress);
        let frames = progress
            .lines()
            .filter_map(|line| line.strip_prefix("frame="))
            .next_back();
        if progress.lines().next_back() != Some("progress=end")
            || number::<u64>(frames) != Some(info.frames)
        {
            return Err(Error::Goal(
                "The decoded video frame count differs from the MP4 track".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn encode(
        &self,
        input: &Path,
        output: &Path,
        crf: u8,
        bytes: u64,
        check: &impl Fn() -> Result<()>,
    ) -> Result<()> {
        let mut command = self.input_command(input);
        command
            .args([
                "-map",
                "0:v:0",
                "-map",
                "0:a:0?",
                "-map_metadata",
                "-1",
                "-map_chapters",
                "-1",
                "-c:v",
                "libx264",
                "-threads:v",
                "2",
                "-preset",
                "fast",
                "-crf",
            ])
            .arg(crf.to_string())
            .args([
                "-fps_mode",
                "passthrough",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-b:a",
                "128k",
                "-f",
                "mp4",
                "-fs",
            ])
            .arg(bytes.to_string())
            .args(["-y"])
            .arg(output);
        execute(&mut command, false, TIMEOUT, check)?;
        Ok(())
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
                "134217728",
                "-filter_threads",
                "2",
                "-threads",
                "2",
                "-noautorotate",
            ])
            .args(LOCAL_MP4)
            .arg("-i")
            .arg(input);
        command
    }
}

fn number<T: std::str::FromStr>(value: Option<&str>) -> Option<T> {
    value?.parse().ok()
}
fn seconds(value: Option<&str>) -> Option<f64> {
    number::<f64>(value).filter(|value| value.is_finite() && *value > 0.0)
}
