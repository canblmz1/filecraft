# Opportunities and implementation plan

1. **Make the target explicit.** Filee already supports video size budgets, presets, routed conversion and safety checks. CloudConvert and FileFlows already orchestrate jobs. Offer a workflow where the user sets per-file and total limits and can see exactly which checks passed. Image batches are the first test of that approach.
2. **Report evidence per result.** Combine inspiration from MediaInfo and Squoosh with dimensions, achieved size, an actual decode, checksum and a lossless pixel comparison or JPEG PSNR. Expose budget failure before creating user-facing output.
3. **Re-use the exact same core in desktop and CLI.** Store versioned declarative recipes with closed fields, explicit constraints and no scripting. Keep source filenames and paths outside recipes. Make format detection independent of extensions.
4. **Keep the base engine small.** Use the Rust image crate with JPEG, PNG and WebP enabled. Constrain this release to static 8-bit inputs and protect transparency, dimensions and orientation. WebP and PNG initially use lossless encoding. JPEG uses bounded, measured alternatives. For a batch, select candidates from actual encoded sizes with a shared budget and a quality preference.
5. **Expand against real fixtures.** Next, isolate FFmpeg/FFprobe workers and validate video stream count, duration, compatibility and size. Review build-specific FFmpeg licenses before redistribution. Evaluate qpdf for PDF structure, PDFium for independent render checks, and OCRmyPDF/Tesseract for OCR. Evaluate safe archive extraction and document conversion independently. Merge media families into one budget after each has clear preservation checks.

## Release gates beyond the initial foundation

Distribute signed packages after Windows, macOS and Linux CI and offline install verification. Add worker isolation and crash recovery before broadening the set of untrusted formats. Build PDF redaction around removal verification and separate visual signatures from digital signatures. Collect reproducible timings, peak memory and fixture results before making performance claims.
