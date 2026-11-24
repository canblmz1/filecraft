# Changelog

## 0.0.1

- Mixed batches of images, PDFs and eligible MP4 clips share a verified byte budget.
- Added lossless PDF stream optimization, optional FFmpeg video processing and an application package recipe.
- Reports distinguish transcoded files from exact-byte copies and record format-specific checks.
- Added a reproducible desktop demo with synthetic inputs.

## Image-only development preview

- Tauri desktop workbench and Rust CLI for static JPEG, PNG and WebP.
- Signature and SHA-256 inspection, raster header probes, and extension mismatch warnings.
- Verified per-file and aggregate byte goals, JSON recipe import/export, and structured reports.
- Asynchronous local execution, cooperative cancellation, and SQLite job history.
