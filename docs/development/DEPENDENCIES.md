# Runtime dependencies

The initial native raster adapter uses the JPEG, PNG and WebP features of the Rust [image crate](https://docs.rs/image/latest/image/), licensed MIT or Apache-2.0. Its JPEG encoder has an exposed quality setting, its WebP encoder is lossless, and its decoder interface exposes image metadata and dimensions. The lockfiles pin the resolved versions. The renderer bundles open-source Manrope and Space Grotesk fonts under the SIL Open Font License. SQLite is compiled into the desktop binary.

The Tauri bundle build generates third-party license notices from the enabled Cargo dependency tree and production npm dependencies. The notices are included in the bundled frontend assets. This inventory is generated per target. Windows uses Microsoft's [Evergreen WebView2 Runtime](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution), packaged as Microsoft's standalone installer for machines that need it. Runtime updates are managed by the host operating environment.

FFmpeg, libvips, qpdf, PDFium, Tesseract, LibreOffice and other external engines are candidates for later adapters. The base build does not include them. FFmpeg redistribution depends on the actual [build configuration and license](https://ffmpeg.org/legal.html). Review platform binaries, licensing, allowed protocols, and independent validation fixtures when enabling an engine.
