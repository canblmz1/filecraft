# Release readiness toward 10,000 stars

Stars are an adoption signal. FileCraft's launch story is a verified size limit for one application package containing multiple file types. This is a viable demo in 0.0.1, with measurable boundaries recorded in the output.

| Priority | Current gap | Evidence to collect |
|---|---|---|
| 1 | A new user needs FFmpeg and FFprobe on PATH for MP4. A signed, reproducible installer and a user-visible engine setup are still needed. | Fresh-machine installs, an offline launch, and a successful mixed batch without manual PATH editing. |
| 2 | Common scanned PDFs already compress their streams. Lossless rewriting may save little, and a short list of MP4 profiles excludes typical edge cases. | Public synthetic or redistributable fixtures for scanned PDFs, phone clips, already-compressed inputs, transparency, and exact budget failures. |
| 3 | A passed byte check does not establish rendition fidelity. PDF lacks an independent renderer and video lacks a perceptual metric. | PDF rendering and text comparisons, video quality evidence, and clear reasons when a goal is impossible. |
| 4 | Crash recovery and worker isolation are incomplete. Windows has the first desktop smoke path. macOS and Linux need release verification. | Cancellation and restart tests, bounded parser resources, signed platform builds, and a cross-platform smoke matrix. |
| 5 | A recorded 20 MB package demo and a synthetic input generator exist in `scripts/demo`. The project still needs a public adoption loop. | A tagged release, issue triage, and feedback from users with actual upload limits. |

Track completed mixed jobs, setup failures, and the reasons goals were missed during opt-in testing. Keep production processing and usage data local. Publish only aggregate results that participants have agreed to share.
