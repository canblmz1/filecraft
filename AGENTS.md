# Working on FileCraft

Keep application behavior in the Rust workspace so CLI and desktop share the same guarantees. The UI may present only registered capabilities. Preserve source paths and refuse goals that cannot be verified. Use synthetic fixtures for tests.

Run `cargo fmt --all -- --check`, `cargo test --locked --workspace`, `cargo clippy --locked --workspace --all-targets -- -D warnings` and `npm --prefix apps/desktop run build` for changes spanning the core and desktop. See the README for platform prerequisites.
