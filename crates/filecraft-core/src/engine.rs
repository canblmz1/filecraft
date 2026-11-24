//! External engines that FileCraft can install into its own data folder.
//!
//! FFmpeg is only downloaded when a person asks for it. The archive is pinned
//! by URL, size and SHA-256, and only the listed files are extracted.

use std::{path::PathBuf, sync::RwLock};

#[derive(Debug, Clone, Copy)]
pub(crate) struct Package {
    pub version: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub bytes: u64,
    /// Archive entries to extract and where to place them in the engine folder.
    pub files: &'static [(&'static str, &'static str)],
    pub license: &'static str,
    pub source: &'static str,
}

#[cfg(all(windows, target_arch = "x86_64"))]
pub(crate) const PACKAGE: Option<Package> = Some(Package {
    version: "9.1 essentials (gyan.dev)",
    url: "https://github.com/GyanD/codexffmpeg/releases/download/9.1/ffmpeg-9.1-essentials_build.zip",
    sha256: "b9bbbb3acece000cbeec994f856649354e52c773807841e78908374b24d658fd",
    bytes: 119_467_219,
    files: &[
        (
            "ffmpeg-9.1-essentials_build/bin/ffmpeg.exe",
            "bin/ffmpeg.exe",
        ),
        (
            "ffmpeg-9.1-essentials_build/bin/ffprobe.exe",
            "bin/ffprobe.exe",
        ),
        ("ffmpeg-9.1-essentials_build/LICENSE", "LICENSE.txt"),
        ("ffmpeg-9.1-essentials_build/README.txt", "README.txt"),
    ],
    license: "GPL-3.0-or-later",
    source: "https://github.com/FFmpeg/FFmpeg/commit/74e4393595",
});

#[cfg(not(all(windows, target_arch = "x86_64")))]
pub(crate) const PACKAGE: Option<Package> = None;

static DATA_DIR: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Use `path` for FileCraft-managed engines instead of the platform default.
pub fn set_data_dir(path: PathBuf) {
    if let Ok(mut directory) = DATA_DIR.write() {
        *directory = Some(path);
    }
    crate::video::refresh_video_engine();
}

/// The desktop app's local data folder, which the CLI shares.
fn default_data_dir() -> Option<PathBuf> {
    const IDENTIFIER: &str = "org.filecraft.desktop";
    let absolute =
        |value: std::ffi::OsString| Some(PathBuf::from(value)).filter(|path| path.is_absolute());
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").and_then(absolute);
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .and_then(absolute)
        .map(|home| home.join("Library/Application Support"));
    #[cfg(all(unix, not(target_os = "macos")))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .and_then(absolute)
        .or_else(|| {
            std::env::var_os("HOME")
                .and_then(absolute)
                .map(|home| home.join(".local/share"))
        });
    base.map(|base| base.join(IDENTIFIER))
}

pub(crate) fn data_dir() -> Option<PathBuf> {
    DATA_DIR
        .read()
        .ok()
        .and_then(|directory| directory.clone())
        .or_else(default_data_dir)
}

pub(crate) fn managed_dir() -> Option<PathBuf> {
    data_dir().map(|directory| directory.join("engines").join("ffmpeg"))
}

pub(crate) fn managed_bin() -> Option<PathBuf> {
    let directory = managed_dir()?;
    directory
        .join("manifest.json")
        .is_file()
        .then(|| directory.join("bin"))
}

#[cfg(feature = "installer")]
pub use install::{InstallProgress, InstallStage, install_video_engine};

#[cfg(feature = "installer")]
mod install {
    use std::{
        fs::{self, File},
        io::{BufReader, Read, Write},
        path::Path,
        time::Duration,
    };

    use serde::Serialize;
    use sha2::{Digest, Sha256};

    use super::{PACKAGE, Package, data_dir};
    use crate::{Control, EngineStatus, Error, Result, video};

    const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;

    #[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum InstallStage {
        Downloading,
        Verifying,
        Extracting,
        Testing,
    }

    #[derive(Debug, Clone, Copy, Serialize)]
    pub struct InstallProgress {
        pub stage: InstallStage,
        pub received_bytes: u64,
        pub total_bytes: u64,
    }

    /// Download, verify and activate the pinned FFmpeg build for this platform.
    pub fn install_video_engine(
        control: &Control,
        mut progress: impl FnMut(InstallProgress),
    ) -> Result<EngineStatus> {
        let package = PACKAGE.ok_or_else(|| {
            Error::Unsupported(
                "FileCraft installs FFmpeg on Windows x64. On macOS run `brew install ffmpeg`; on Linux install ffmpeg with your package manager."
                    .into(),
            )
        })?;
        let root = data_dir()
            .ok_or_else(|| Error::Invalid("FileCraft could not find its local data folder".into()))?
            .join("engines");
        fs::create_dir_all(&root)?;
        let check = || control.check(None);
        let mut report = |stage, received_bytes| {
            progress(InstallProgress {
                stage,
                received_bytes,
                total_bytes: package.bytes,
            })
        };
        let mut archive = tempfile::Builder::new()
            .prefix(".ffmpeg-")
            .suffix(".zip")
            .tempfile_in(&root)?;
        download(package, archive.as_file_mut(), &check, &mut report)?;
        report(InstallStage::Extracting, package.bytes);
        let staging = tempfile::Builder::new()
            .prefix(".ffmpeg-staging-")
            .tempdir_in(&root)?;
        extract(package, archive.path(), staging.path(), &check)?;
        drop(archive);
        let manifest = serde_json::json!({
            "name": "FFmpeg",
            "version": package.version,
            "url": package.url,
            "sha256": package.sha256,
            "license": package.license,
            "source": package.source,
        });
        fs::write(
            staging.path().join("manifest.json"),
            serde_json::to_vec_pretty(&manifest)?,
        )?;
        check()?;
        report(InstallStage::Testing, package.bytes);
        let target = root.join("ffmpeg");
        let backup = root.join(".ffmpeg-previous");
        if backup.exists() {
            fs::remove_dir_all(&backup)?;
        }
        let had_previous = target.exists();
        if had_previous {
            fs::rename(&target, &backup)?;
        }
        let staged = staging.keep();
        let restore = |error: Error| {
            let _ = fs::remove_dir_all(&target);
            if had_previous {
                let _ = fs::rename(&backup, &target);
            }
            let _ = fs::remove_dir_all(&staged);
            video::refresh_video_engine();
            error
        };
        fs::rename(&staged, &target).map_err(|error| restore(error.into()))?;
        video::refresh_video_engine();
        let status = video::video_engine();
        if status.source != Some("managed") {
            return Err(restore(Error::Goal(
                "The downloaded FFmpeg did not pass FileCraft's engine checks".into(),
            )));
        }
        if had_previous {
            let _ = fs::remove_dir_all(&backup);
        }
        Ok(status)
    }

    fn download(
        package: Package,
        file: &mut File,
        check: &impl Fn() -> Result<()>,
        report: &mut impl FnMut(InstallStage, u64),
    ) -> Result<()> {
        let failed = |error: ureq::Error| {
            Error::Io(std::io::Error::other(format!("Download failed: {error}")))
        };
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .user_agent(concat!("FileCraft/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        let mut response = agent.get(package.url).call().map_err(failed)?;
        let mut reader = response
            .body_mut()
            .with_config()
            .limit(package.bytes + 1)
            .reader();
        let mut hasher = Sha256::new();
        let mut buffer = vec![0_u8; 256 * 1024];
        let mut received = 0_u64;
        let mut reported = 0_u64;
        report(InstallStage::Downloading, 0);
        loop {
            check()?;
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            received += count as u64;
            if received > package.bytes {
                return Err(Error::Goal(
                    "The download is larger than the pinned FFmpeg archive".into(),
                ));
            }
            hasher.update(&buffer[..count]);
            file.write_all(&buffer[..count])?;
            if received - reported >= 1024 * 1024 {
                reported = received;
                report(InstallStage::Downloading, received);
            }
        }
        report(InstallStage::Verifying, received);
        file.sync_all()?;
        let digest = format!("{:x}", hasher.finalize());
        if received != package.bytes || digest != package.sha256 {
            return Err(Error::Goal(
                "The downloaded FFmpeg archive does not match its pinned size and SHA-256".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn extract(
        package: Package,
        archive: &Path,
        target: &Path,
        check: &impl Fn() -> Result<()>,
    ) -> Result<()> {
        let invalid = |error: zip::result::ZipError| {
            Error::Invalid(format!("The FFmpeg archive could not be read: {error}"))
        };
        let mut zip =
            zip::ZipArchive::new(BufReader::new(File::open(archive)?)).map_err(invalid)?;
        for (entry, destination) in package.files {
            check()?;
            let mut file = zip.by_name(entry).map_err(invalid)?;
            if !file.is_file() || file.size() > MAX_FILE_BYTES {
                return Err(Error::Invalid(format!("Unexpected archive entry {entry}")));
            }
            let path = target.join(destination);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut output = File::create(&path)?;
            let copied = std::io::copy(&mut (&mut file).take(MAX_FILE_BYTES + 1), &mut output)?;
            if copied != file.size() {
                return Err(Error::Invalid(format!(
                    "Archive entry {entry} is truncated"
                )));
            }
            output.sync_all()?;
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use std::io::Write;

        use super::*;

        const TEST_PACKAGE: Package = Package {
            version: "test",
            url: "https://invalid.example/ffmpeg.zip",
            sha256: "",
            bytes: 0,
            files: &[
                ("pack/bin/ffmpeg", "bin/ffmpeg"),
                ("pack/LICENSE", "LICENSE.txt"),
            ],
            license: "test",
            source: "test",
        };

        fn archive(path: &Path, entries: &[(&str, &[u8])]) {
            let mut writer = zip::ZipWriter::new(File::create(path).unwrap());
            for (name, bytes) in entries {
                writer
                    .start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(bytes).unwrap();
            }
            writer.finish().unwrap();
        }

        #[test]
        fn extracts_only_listed_entries() {
            let directory = tempfile::tempdir().unwrap();
            let zip = directory.path().join("engine.zip");
            archive(
                &zip,
                &[
                    ("pack/bin/ffmpeg", b"binary"),
                    ("pack/LICENSE", b"license"),
                    ("../escape", b"no"),
                    ("pack/bin/other", b"no"),
                ],
            );
            let target = directory.path().join("engine");
            extract(TEST_PACKAGE, &zip, &target, &|| Ok(())).unwrap();
            assert_eq!(fs::read(target.join("bin/ffmpeg")).unwrap(), b"binary");
            assert_eq!(fs::read(target.join("LICENSE.txt")).unwrap(), b"license");
            assert!(!target.join("bin/other").exists());
            assert!(!directory.path().join("escape").exists());
        }

        #[test]
        fn missing_entries_are_rejected() {
            let directory = tempfile::tempdir().unwrap();
            let zip = directory.path().join("engine.zip");
            archive(&zip, &[("pack/bin/ffmpeg", b"binary")]);
            let error = extract(
                TEST_PACKAGE,
                &zip,
                &directory.path().join("engine"),
                &|| Ok(()),
            )
            .unwrap_err();
            assert_eq!(error.code(), "invalid_input");
        }
    }
}
