# Workbench architecture

Desktop React calls Tauri commands. A Rust queue limits desktop processing to one job at a time and moves CPU work off the UI thread. SQLite stores pending and terminal job records. Progress is in memory. The CLI and Tauri use the same core. The core registry describes the built-in raster adapter and the planner checks input and output compatibility through these declared capabilities.

1. Preflight checks input signatures and limits, recipe compatibility and output disk space.
2. The worker takes a bounded input snapshot, decodes it, checks supported metadata and normalizes EXIF orientation. Each candidate is encoded, written and synced in a uniquely named staging directory on the destination volume.
3. Verification reopens the staged bytes, checks format, size, dimensions, alpha and decoder metadata probes, then computes pixel equality or JPEG RGB mean squared error and SHA-256. PSNR is derived from mean squared error. A null PSNR alongside identical pixels stands for zero error.
4. JPEG planning keeps candidates that improve measured error for a given byte cost. Under a batch limit, start with the least expensive candidate per input. Spend the remaining bytes on the greatest relative error reduction per additional byte. This deterministic greedy selection is a bounded approximation. PNG and WebP currently produce one lossless candidate each.
5. Once the plan meets its limits, publish each verified candidate without replacing existing files. If publication stops after some files, the execution reports partial completion. Output hashes describe the staged bytes at publication. The app does not continuously monitor files afterward.

## Resource and recovery scope

Raster safety limits are 128 MiB input, 24 megapixels and 12,000 pixels per side. The worker stages candidates on disk and checks available space. A single file may use several image buffers, and disk capacity can change between the preflight and write. Cancellation and deadlines are cooperative at phase and measurement checkpoints. Individual codec calls may finish before accepting cancellation.

At startup, the desktop marks unfinished records from the previous session as failed and leaves previously published outputs intact. The OS may leave a staging directory behind after a crash or power loss. A retry creates new output names. Automatic resume, staging recovery, worker OS sandboxing, hard memory or time enforcement, pause, GPU and a full durable retry journal are future work.

Inspection fingerprints identify matching bytes at read time. A known signature and parseable header are not a comprehensive integrity guarantee. File paths are selected by the user and inherit the behavior of the OS, including sync or mounted volumes. SQLite records job outcomes and published output paths in the app data directory. The history view shows up to 100 recent jobs.
