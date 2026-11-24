# Runtime dependencies

The native raster adapter uses the JPEG, PNG and WebP features of the Rust [image crate](https://docs.rs/image/latest/image/), licensed MIT or Apache-2.0. Its JPEG encoder has an exposed quality setting, its WebP encoder is lossless, and its decoder interface exposes image metadata and dimensions. The lockfiles pin the resolved versions. The renderer bundles open-source Manrope and Space Grotesk fonts under the SIL Open Font License. SQLite is compiled into the desktop binary.

The Tauri bundle build generates third-party license notices from the enabled Cargo dependency tree and production npm dependencies. The notices are included in the bundled frontend assets. This inventory is generated per target. Windows uses Microsoft's [Evergreen WebView2 Runtime](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution). Windows 11 includes it. The installer embeds Microsoft's small bootstrapper, which downloads the runtime only on machines that lack it. Runtime updates are managed by the host operating environment.

The built-in PDF adapter uses [lopdf](https://github.com/J-F-Liu/lopdf) under MIT and re-encodes page images with the image crate.

## Video engine

MP4, MOV and HEIC use FFmpeg and FFprobe as separate processes. FileCraft looks for them in this order:

1. the folder named by the `FILECRAFT_FFMPEG_DIR` environment variable;
2. the engine that FileCraft installed into its local data folder, `engines/ffmpeg` (on Windows `%LOCALAPPDATA%\org.filecraft.desktop`), shared by the desktop app and the CLI;
3. the process PATH.

FFmpeg must expose the libx264, AAC and PNG encoders. HDR tone mapping needs the zscale and tonemap filters. HEIC needs the HEVC decoder and FFmpeg 7.1 or later, which assembles HEIF tile grids.

On Windows x64, **Install** on the Engines page, or `filecraft engine install`, downloads the [FFmpeg 9.1 essentials build from gyan.dev](https://github.com/GyanD/codexffmpeg/releases/tag/9.1). The download is pinned by URL, byte size and SHA-256. Only `ffmpeg.exe`, `ffprobe.exe`, the license and the readme are extracted. FileCraft then runs its usual engine checks before it switches to the new engine, and keeps the previous engine if they fail. The installer uses [ureq](https://github.com/algesten/ureq) with rustls (MIT or Apache-2.0) and the [zip](https://github.com/zip-rs/zip2) crate (MIT). Nothing is downloaded unless someone asks for it.

That build is licensed GPL-3.0-or-later and links GPL libraries such as x264. FileCraft does not redistribute it. The person installing it downloads it from its publisher, and its license and source reference are kept next to the binaries. Redistributing FFmpeg yourself depends on the [build configuration and license](https://ffmpeg.org/legal.html).

The adapter passes an explicit local file protocol allowlist and disables external MOV data references. It does not run FFmpeg in an OS sandbox. [FFmpeg protocols](https://ffmpeg.org/ffmpeg-protocols.html)
