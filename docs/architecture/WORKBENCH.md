# Workbench architecture

Desktop React calls Tauri commands. A Rust queue limits desktop processing to one job at a time and moves CPU work off the UI thread. SQLite stores pending and terminal job records. Progress is in memory. The CLI and Tauri use the same core. The registry describes built-in raster and PDF adapters, plus an optional local FFmpeg adapter for MP4.

1. Preflight checks input signatures and limits, recipe compatibility and output disk space.
2. The worker takes a bounded input snapshot. Each candidate is written and synced in a uniquely named staging directory on the destination volume.
3. Raster candidates are decoded again and checked for dimensions, metadata, alpha and pixel equality or JPEG PSNR. PDF candidates verify page order, parsed objects and trailer, and decompressed bytes of changed streams. Video candidates verify codecs, dimensions, decoded frame count, durations, audio channels and sample rate, relative start offset, and full decode. The report includes byte sizes, SHA-256 and unchecked properties.
4. Begin with the smallest verified candidate for each input. Spend the remaining budget to select higher-fidelity candidates, preferring original MP4 bytes if they fit. H.264 candidate ordering uses the encoder's CRF setting as a preference, without treating it as a measured quality score. For direct JPEG batches the ranking uses measured RGB error. The bounded greedy search does not model perceptual quality.
5. Once the plan meets its limits, publish each verified candidate without replacing existing files. If publication stops after some files, the execution reports partial completion. Output hashes describe the staged bytes at publication. The app does not continuously monitor files afterward.

## Resource and recovery scope

Raster limits are 128 MiB input, 24 megapixels and 12,000 pixels per side. PDF limits are 32 MiB input and 16 MiB per eagerly decompressed PDF stream. MP4 limits are 512 MiB input, video up to 1080p and 120 seconds, up to 60 average fps, and a bounded encoder ladder. Cancellation is cooperative in native codecs and stops the active FFmpeg or FFprobe child process. Individual native calls may finish before accepting cancellation.

At startup, the desktop marks unfinished records from the previous session as failed and leaves previously published outputs intact. The OS may leave a staging directory behind after a crash or power loss. A retry creates new output names. Automatic resume, staging recovery, worker OS sandboxing, hard memory or time enforcement, pause, GPU and a full durable retry journal are future work.

Inspection fingerprints identify matching bytes at read time. A known signature and parseable header are not a comprehensive integrity guarantee. File paths are selected by the user and inherit the behavior of the OS, including sync or mounted volumes. SQLite records job outcomes and published output paths in the app data directory. The history view shows up to 100 recent jobs.
