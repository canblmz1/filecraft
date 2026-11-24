# Release readiness toward 10,000 stars

Stars are an adoption signal. FileCraft's launch story is a verified size limit for one application package containing phone photos, scanned PDFs and phone videos.

| Priority | Current gap | Evidence to collect |
|---|---|---|
| 1 | The Windows installer is unsigned, and the one-click FFmpeg install has not yet been tried on fresh machines. Signing waits for SignPath approval, as described in [signing](SIGNING.md). | Fresh Windows 10 and 11 installs without WebView2 or FFmpeg, an engine install behind a proxy, and a signed release built by the release workflow. |
| 2 | Real phone files are covered by synthetic fixtures only. Photos are not resized, PDF downsampling skips JBIG2, CCITT, JPEG 2000 and CMYK images, and slow-motion video above 60 fps is refused. | Redistributable samples from current iPhones and Android phones, scanners and office multifunction printers, plus the reasons a goal was missed. |
| 3 | A passed byte check does not establish rendition fidelity. PDF lacks an independent renderer and text legibility check, and video lacks a perceptual metric. | PDF rendering and OCR comparisons, video quality evidence, and before/after previews in the app. |
| 4 | Crash recovery and worker isolation are incomplete. Windows has the first desktop smoke path. macOS and Linux need release verification and packages. | Cancellation and restart tests, bounded parser resources, signed platform builds, and a cross-platform smoke matrix. |
| 5 | A recorded 20 MB package demo and a synthetic input generator exist in `scripts/demo`. The project still needs a public adoption loop. | A tagged release, issue triage, and feedback from users with actual upload limits. |

Track completed mixed jobs, setup failures, and the reasons goals were missed during opt-in testing. Keep production processing and usage data local. Publish only aggregate results that participants have agreed to share.
