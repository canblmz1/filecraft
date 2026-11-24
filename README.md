# FileCraft

**The private workbench for any file.**

FileCraft is a free, MIT-licensed desktop application and CLI for local, goal-driven file processing. Version 0.1 starts with static JPEG, PNG and WebP images.

## What you can do

- Inspect the detected content type, byte size and SHA-256. Supported raster headers also expose dimensions, animation, alpha channel, ICC and EXIF probes.
- Set a per-file or whole-batch size limit. With JPEG, try several encodings and select from verified candidates. PNG and WebP encode losslessly.
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

Conversion is limited to static 8-bit JPEG, PNG and WebP. Animated images, embedded ICC profiles and higher bit depths require additional engines or color handling. Some other families can be recognized by signature for inspection. Media, PDF, documents, archives and cross-format batch optimization remain on the roadmap. Candidate search is bounded and measures RGB PSNR for JPEG. Perceptual quality, color appearance and the absence of private information visible in pixels require separate checks.

[Architecture](docs/architecture/WORKBENCH.md) · [Recipe format](docs/development/RECIPES.md) · [Dependencies](docs/development/DEPENDENCIES.md) · [Competitor research](docs/research/COMPETITORS.md) · [Contributing](CONTRIBUTING.md) · [Security scope](SECURITY.md)
