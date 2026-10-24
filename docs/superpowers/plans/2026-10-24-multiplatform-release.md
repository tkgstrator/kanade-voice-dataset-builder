# Multiplatform Release Implementation Plan

1. Add failing portability tests for no-replace promotion and canonical persisted paths.
2. Add cross-platform durability helpers, Windows `MoveFileExW` promotion, and portable
   relative-path serialization; run format, Clippy, and all synthetic tests.
3. Align `.devcontainer`, Cargo configuration, editor settings, and Docker ignore behavior
   with `qtmleap/devcontainers/examples/rust`, retaining the pinned toolchain and adding
   FFmpeg for this project.
4. Expand Integration to Linux amd64, Linux arm64, macOS arm64, and Windows amd64; install
   and report FFmpeg/FFprobe on every runner and run all existing quality gates.
5. Expand Deployment to four explicit native package jobs. Package Windows as ZIP and the
   Unix targets as tar.gz archives.
6. Extend release validation to an exact four-archive allowlist, safe archive member types,
   native executable headers, four checksums, and an exact five-asset draft and publication.
7. Bump the crate to `0.1.1` and update README support, download, verification, and
   Dev Container documentation while preserving the Nintendo Store target statement.
8. Validate locally, push cohesive Conventional Commits through guarded Git, wait for the
   four-platform Integration run, manually dispatch Deployment from the default branch,
   and independently verify the tag, release, asset names, sizes, checksums, and formats.
