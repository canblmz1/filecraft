# FileCraft

**The private workbench for any file.**

FileCraft is a free, MIT-licensed desktop application and CLI for local, goal-driven file processing. Set one total size limit for a package of images, PDFs and MP4 clips, then check the results before sharing.

## What you can do

- Inspect the detected content type, byte size and SHA-256. Supported raster headers also expose dimensions, animation, alpha channel, ICC and EXIF probes.
- Set a per-file or whole-batch size limit. With JPEG, try several encodings and select from verified candidates. PNG and WebP encode losslessly.
- Choose **Mixed batch** for static JPEG, PNG and WebP images, PDFs, and supported MP4 clips. Images are losslessly optimized as WebP when smaller. PDFs use lossless stream compression. MP4 uses verified H.264/AAC candidates when helpful. Byte-for-byte copies are reported explicitly. Video needs FFmpeg with libx264 and AAC plus FFprobe on PATH.
- Keep transparency and orientation-corrected pixel dimensions. Retrieve before/after sizes, checksums, validation results and JPEG RGB PSNR in a JSON report.
- Reuse native UI presets or import and export versioned JSON recipes. Desktop and CLI use the same Rust engine.
- Save verified output as a new file. Originals and existing output files are preserved.

## Run from source

Install Rust and Node.js 22, plus the platform [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/). Dependency downloads are required for the first build. The packaged application runs using locally installed or bundled runtime components and has no account, telemetry, remote font or update client.

```bash
npm --prefix apps/desktop ci
cd apps/desktop
npm run tauri dev
```

Use **Browse files**, choose a format or a quick recipe, and select **Run & verify**. Choose an output folder when prompted. Job reports remain in the local desktop history.

### CLI

From the repository root, with an existing output directory:

```bash
cargo run --locked -p filecraft-cli -- capabilities
cargo run --locked -p filecraft-cli -- inspect photo.png scan.jpg
cargo run --locked -p filecraft-cli -- run --recipe recipes/small-attachments.json --output output photo.png scan.jpg
cargo run --locked -p filecraft-cli -- run --recipe recipes/application-package.json --output output photo.png document.pdf clip.mp4
```

CLI output is JSON. Exit codes are 0 (completed), 1 (failed/partial), or 130 (cancelled). Ctrl+C requests cooperative cancellation. The processing engine uses complete bytes and the desktop displays decimal MB.

### Build and test

```bash
npm --prefix apps/desktop run build
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cd apps/desktop
npm run tauri build
```

On Windows systems with limited build memory, set `$env:CARGO_BUILD_JOBS = '1'` in PowerShell before running the Tauri build.

## Current boundaries

Raster conversion supports static 8-bit JPEG, PNG and WebP without embedded ICC profiles. Mixed MP4 supports eligible progressive SDR clips up to 1080p, 60 fps and 120 seconds. PDF optimization compresses eligible unfiltered streams and checks the parsed page order, objects and decompressed stream bytes. It can preserve already-optimized or protected PDFs as identical copies. It does not downsample scanned pages. Office formats, archives, perceptual video quality, independent PDF rendering and a portable no-setup video engine remain future work.

[Architecture](docs/architecture/WORKBENCH.md) · [Recipe format](docs/development/RECIPES.md) · [Dependencies](docs/development/DEPENDENCIES.md) · [Release readiness](docs/development/RELEASE_READINESS.md) · [Competitor research](docs/research/COMPETITORS.md) · [Contributing](CONTRIBUTING.md) · [Security scope](SECURITY.md)
