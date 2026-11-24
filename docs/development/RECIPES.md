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
| `min_psnr_db` | JPEG-only minimum measured RGB PSNR in dB; equality satisfies all supported thresholds |
| `lossless_only` | When true, reject direct JPEG encoding and keep MP4 bytes unchanged in mixed mode. PNG/WebP encodes verify decoded pixel equality. |
| `max_seconds` | Cooperative processing deadline, 1–86,400 seconds; excludes queue waiting |

Input selection and the output folder are separate arguments. Raster re-encoding applies EXIF orientation and avoids copying recognized ancillary metadata. ICC input is refused when detected. In mixed mode, a smaller lossless WebP takes the place of the original raster encoding. When the original is smaller or equal it is copied byte-for-byte, retaining its metadata. PDF stays PDF and MP4 stays MP4. Mixed mode is a size goal and provides no metadata-removal guarantee. Example recipes live in [recipes](../../recipes/).
