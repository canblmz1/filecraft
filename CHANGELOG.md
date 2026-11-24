# Changelog

## 0.0.2

- Mixed batches re-encode opaque photos as JPEG above a 32 dB PSNR floor when the limit requires it. `min_psnr_db` now also applies to mixed batches.
- RGB ICC color profiles are carried over and verified instead of being refused. The raster limit is now 50 megapixels.
- Scanned PDFs: page images are located through the content stream, downsampled to 200, 150 or 120 dpi and stored as JPEG. Untouched objects are checked to decode identically.
- HEIC photos, including tile grids and Display P3 color, become upright sRGB JPEGs. EXIF and XMP are reported as removed.
- MOV and HEVC clips, rotated videos and HLG or PQ HDR become H.264/AAC MP4 at 1080p or 720p. Video metadata and extra tracks are dropped.
- One-click FFmpeg install on Windows x64 (desktop Engines page and `filecraft engine install`), pinned by size and SHA-256.
- The Windows installer embeds the WebView2 bootstrapper instead of the offline runtime, so it is a few MB instead of about 300 MB.
- A release workflow builds the Windows installer on GitHub Actions and signs it through SignPath once the project is configured.

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
