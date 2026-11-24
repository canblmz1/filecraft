# Recipes v1

The schema is [recipe-v1.schema.json](../../schemas/recipe-v1.schema.json). A recipe is a single JSON object. The Rust core rejects unknown keys, unsupported format values, conflicting settings and imports larger than 64 KiB. Values below use decimal bytes, so 5 MB is 5,000,000 bytes. Missing or null optional fields mean no explicit constraint.

| Key | Semantics |
|---|---|
| `version` | Required integer, currently 1 |
| `output_format` | Required, one of `jpeg`, `png`, `webp` |
| `max_file_bytes` | Maximum verified size of each output |
| `max_batch_bytes` | Maximum sum of all planned outputs |
| `min_width`, `min_height` | Minimum oriented pixel dimensions; images are neither resized nor upscaled |
| `jpeg_quality` | Preferred maximum JPEG encoder quality, 35–100; default 90 |
| `min_psnr_db` | JPEG-only minimum measured RGB PSNR in dB; equality satisfies all supported thresholds |
| `lossless_only` | Reject JPEG output when true; PNG/WebP always verify exact decoded pixel equality |
| `max_seconds` | Cooperative processing deadline, 1–86,400 seconds; excludes queue waiting |

Input selection and the output folder are separate arguments. The engine preserves transparency or refuses the operation. Output is re-encoded without copying ancillary metadata. EXIF orientation is applied before encoding. ICC input is refused when detected. The output report records recognized EXIF, XMP, and IPTC changes separately from checks that need other tools. Example recipes live in [recipes](../../recipes/).
