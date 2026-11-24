# Recipes v1

The schema is [recipe-v1.schema.json](../../schemas/recipe-v1.schema.json). A recipe is a single JSON object. The Rust core rejects unknown keys, unsupported format values, conflicting settings and imports larger than 64 KiB. Values below use decimal bytes, so 5 MB is 5,000,000 bytes. Missing or null optional fields mean no explicit constraint.

| Key | Semantics |
|---|---|
| `version` | Required integer, currently 1 |
| `output_format` | Required, one of `jpeg`, `png`, `webp`, `mixed` |
| `max_file_bytes` | Maximum verified size of each output |
| `max_batch_bytes` | Maximum sum of all planned outputs, required in mixed mode |
| `min_width`, `min_height` | Minimum oriented image and video pixel dimensions. PDF files are unaffected. |
| `jpeg_quality` | Preferred maximum JPEG encoder quality, 35–100; default 90 |
| `min_psnr_db` | Minimum measured RGB PSNR in dB for JPEG output, or for lossy photos and re-encoded PDF page images in mixed mode. Mixed mode defaults to 32 dB for photos and 30 dB for PDF page images. Equality satisfies all supported thresholds. |
| `lossless_only` | When true, reject direct JPEG encoding. In mixed mode, keep originals instead of re-encoding them lossily: photos become lossless WebP or byte copies, PDFs only receive lossless stream compression, and HEIC, MP4 and MOV files are copied unchanged. PNG/WebP encodes verify decoded pixel equality. |
| `max_seconds` | Cooperative processing deadline, 1–86,400 seconds; excludes queue waiting |

Input selection and the output folder are separate arguments. Raster re-encoding applies EXIF orientation and avoids copying recognized ancillary metadata. RGB ICC profiles are carried over byte-for-byte and verified in the output. Pixels are not converted between color spaces, except that HEIC photos tagged with Display P3 or BT.2020 primaries and no ICC profile are converted to sRGB.

When the inputs already fit, mixed mode avoids lossy steps, except that HEIC and MOV files are converted once at the highest quality for compatibility. Otherwise it plans verified candidates per file and spends the budget where it buys the most quality:

- Opaque photos: lossless WebP when smaller, the original bytes, and JPEG at qualities 90 to 50 above the PSNR floor. Transparent images stay lossless.
- HEIC photos: JPEG, because many upload forms reject HEIC.
- PDFs: lossless stream compression, plus page images downsampled to 200, 150 or 120 dpi and stored as JPEG at 82, 72 or 62. Signed, encrypted, incremental and linearized PDFs are never rewritten.
- Videos: H.264/AAC MP4 at 1080p CRF 23, 1080p CRF 29 and 720p CRF 31. MP4 originals are kept when they fit. MOV and HEVC clips are converted, HDR is tone-mapped to SDR, and rotation is applied to the pixels.

Mixed mode is a size goal. Re-encoded files drop EXIF, XMP, IPTC and video metadata such as GPS positions, but byte copies keep whatever the original contains. Example recipes live in [recipes](../../recipes/).
