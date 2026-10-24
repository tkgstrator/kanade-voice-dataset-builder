# Multiplatform Release Design

## Goal

Publish a verified `v0.1.1` release with native binaries for Linux amd64, Linux arm64,
macOS arm64, and Windows amd64, and align the development container with the Rust
environment in `qtmleap/devcontainers`.

## Runtime support

Windows is a supported runtime only after the complete synthetic dataset workflow passes
on `windows-2025`. The implementation must preserve the existing no-replace finalization
guarantee, use writable handles when flushing generated files on Windows, avoid unsupported
directory flushes there, and serialize persisted relative paths with `/` on every platform.

The Windows promotion primitive is `MoveFileExW` with `MOVEFILE_WRITE_THROUGH` and without
`MOVEFILE_REPLACE_EXISTING` or `MOVEFILE_COPY_ALLOWED`. This keeps the operation on one
volume and refuses an occupied destination.

Windows flushes generated files through writable handles. Rust does not expose the Unix
directory `fsync` behavior on Windows, so directory sync there is an existence/type check;
functional atomic promotion is required, while power-loss persistence of directory entries
is not claimed to be equivalent to Unix.

## CI and release artifacts

Integration runs the full format, Clippy, and synthetic-test gates on these native runners:

- `ubuntu-24.04` for Linux amd64
- `ubuntu-24.04-arm` for Linux arm64
- `macos-15` for macOS arm64
- `windows-2025` for Windows amd64

Deployment builds and starts each binary on the same native runner. The immutable release
asset set is:

- `kanade-tts-linux-amd64.tar.gz`
- `kanade-tts-linux-arm64.tar.gz`
- `kanade-tts-macos-arm64.tar.gz`
- `kanade-tts-windows-amd64.zip`
- `SHA256SUMS`

Each archive has one root directory and contains only the executable, `README.md`, and
`THIRD_PARTY_NOTICES.md`. The release job rejects extra artifacts, duplicate or unsafe
members, links and non-regular members, an encrypted ZIP, an unexpected executable format,
or a digest mismatch before creating the tag or draft release.

## Dev Container

The development container follows `qtmleap/devcontainers`' Rust example: the Microsoft
Ubuntu 24.04 dev-container base, `vscode` user and workspace layout, named Cargo and target
volumes, common utilities, GitHub CLI, guarded Git, Rust, Claude Code, Codex CLI, Bun,
Docker-outside-of-Docker, Act, direnv, the upstream lifecycle scripts, and the upstream
VS Code extension/settings set. The repository keeps its pinned Rust toolchain and adds
FFmpeg as a project requirement. Sensitive host mounts stay identical to the upstream
contract and remain read-only where upstream marks them read-only. Because those mounts and
Docker access expose powerful host capabilities to the container, the README must state that
the Dev Container is for trusted revisions and that read-only mounts still permit reads.

The container definition must build on both amd64 and arm64 hosts. Validation uses a
configuration parse, Compose render, and a real Dev Container build when the local Docker
daemon and Dev Container CLI are available.

## Documentation

The GitHub README retains the exact Japanese My Nintendo Store target and software ID,
lists the four supported binary targets, links the GitHub Releases page, explains archive
verification with `SHA256SUMS`, and notes that FFmpeg and FFprobe remain runtime dependencies.

## Release safety

`v0.1.0` remains untouched. Cargo is advanced to `0.1.1`, a fresh tag-absence check runs
before deployment, and the manual workflow refuses non-default branches and existing tags.
The release is published only after Integration succeeds for the release commit.

The existing fail-closed recovery policy remains deliberate: a failure after tag or draft
creation leaves that remote state visible for inspection, and a retry refuses to overwrite
it. Recovery requires an explicit human decision after checking the tag target and draft;
the workflow never deletes or reuses an existing remote tag automatically.
