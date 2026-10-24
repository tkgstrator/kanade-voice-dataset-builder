# KANADE voice dataset builder

This Rust command line tool inspects and prepares a local, auditable voice dataset from **your own lawfully obtained and extracted** KANADE `romfs/data` tree. The supported source is the Nintendo Switch release of [KANADE on the Japanese My Nintendo Store](https://store-jp.nintendo.com/item/software/D70010000112377) (software ID `D70010000112377`). Source fingerprints are intentionally fail-closed; other platforms, editions, or game builds are unsupported unless they are explicitly added and tested. It does not include decryption keys, game scripts, scenarios, audio, other game assets, or a generated dataset. The game and its content remain subject to the rights of their respective owners. Use and share any resulting dataset only where you have the necessary rights; this repository provides no permission to redistribute game content.

Prebuilt command-line binaries are produced for Linux amd64, Linux arm64, macOS arm64, and Windows amd64. `ffmpeg` and `ffprobe` must be installed separately and available on `PATH`; `inspect` and `build` require the audio tools, while `pilot` needs only the extracted source and an output location. Building from source requires Cargo and the Rust 1.98.1 toolchain pinned by this repository.

## Installation

Download the archive for your platform from [GitHub Releases](https://github.com/tkgstrator/kanade-voice-dataset-builder/releases):

| Platform | Release asset |
| --- | --- |
| Linux amd64 (`x86_64`) | `kanade-tts-linux-amd64.tar.gz` |
| Linux arm64 (`aarch64`) | `kanade-tts-linux-arm64.tar.gz` |
| macOS arm64 (Apple silicon) | `kanade-tts-macos-arm64.tar.gz` |
| Windows amd64 (`x86_64`) | `kanade-tts-windows-amd64.zip` |

Each release also includes `SHA256SUMS`. On Linux, set `archive` to the file you downloaded and run `grep -F "  $archive" SHA256SUMS | sha256sum -c -`. On macOS, compare `shasum -a 256 <archive>` with the matching line. On Windows, compare `(Get-FileHash <archive> -Algorithm SHA256).Hash` with the matching value in `SHA256SUMS`.

Extract the archive, then run `./kanade-tts --help` on Linux or macOS, or `.\kanade-tts.exe --help` in Windows PowerShell. You can move the executable to a directory on `PATH` or invoke it by its full path.

## Workflow

Choose a source directory whose children include `voice/`, `scn/`, and the runtime files checked by the KANADE fingerprint configuration. Put outputs outside the source tree. Replace the example paths below with your local paths.

The commands below run from a source checkout. When using a prebuilt archive, replace `cargo run --locked --` with the path to `kanade-tts` or `kanade-tts.exe`; the command names and arguments are identical.

```sh
SOURCE=/path/to/romfs/data
OUTPUT=/path/to/KANADE-v1

cargo run --locked -- inspect --source "$SOURCE" --ffprobe ffprobe
cargo run --locked -- pilot --source "$SOURCE" --scenario scn00_01.ks.scn --output /path/to/pilot-output
cargo run --locked -- build --source "$SOURCE" --output "$OUTPUT" --ffmpeg ffmpeg --ffprobe ffprobe
cargo run --locked -- review --source "$SOURCE" --output "$OUTPUT" --ffprobe ffprobe
cargo run --locked -- verify --source "$SOURCE" --output "$OUTPUT.work" --ffprobe ffprobe
cargo run --locked -- finalize --source "$SOURCE" --output "$OUTPUT" --ffprobe ffprobe
cargo run --locked -- verify --source "$SOURCE" --output "$OUTPUT" --ffprobe ffprobe
```

`build` writes and verifies a work directory next to the requested final output and writes a dataset Card there. `review` lists required samples and decisions. After checking a sample's evidence, record a decision with `--utterance-id ID --decision pass` or `--decision fail`; use `--correct` only to replace an existing decision. Re-run `review` and `verify` before `finalize`. `finalize` seals and promotes the work directory; the final `verify` checks the published local output. The tool rejects unrecognized source structures and changed fingerprinted runtime files.

## Command arguments

These signatures match `kanade-tts --help` and each subcommand's `--help`:

| Command | Required arguments | Optional arguments |
| --- | --- | --- |
| `inspect` | `--source <SOURCE> --ffprobe <FFPROBE>` | `--help` |
| `pilot` | `--source <SOURCE> --scenario <SCENARIO> --output <OUTPUT>` | `--help` |
| `build` | `--source <SOURCE> --output <OUTPUT> --ffmpeg <FFMPEG> --ffprobe <FFPROBE>` | `--help` |
| `review` | `--source <SOURCE> --output <OUTPUT> --ffprobe <FFPROBE>` | `--utterance-id <UTTERANCE_ID> --decision <pass|fail>` together; `--correct` requires an utterance ID; `--help` |
| `verify` | `--source <SOURCE> --output <OUTPUT> --ffprobe <FFPROBE>` | `--dataset` is an alias of `--output`; `--help` |
| `finalize` | `--source <SOURCE> --output <OUTPUT> --ffprobe <FFPROBE>` | `--help` |
| `help` | none | `help <COMMAND>` shows help for a subcommand |

The `--scenario` argument is a single `.scn` filename inside the source `scn/` directory. The `--output` argument for `build`, `review`, and `finalize` names the intended final root; `verify` accepts either the work root or the final root.

## Development

The Dev Container follows the Rust environment from [`qtmleap/devcontainers`](https://github.com/qtmleap/devcontainers/tree/master/examples/rust): Microsoft Ubuntu 24.04 base, the `vscode` user and workspace layout, persistent Cargo and target volumes, the shared Rust/agent/Docker tooling, lifecycle scripts, and editor extensions. This repository keeps Rust 1.98.1 pinned and adds FFmpeg for the dataset pipeline. Open the repository in the container and the locked Cargo dependencies are prefetched automatically.

Like the upstream environment, the container can receive local agent/API configuration, GitHub authentication, SSH/AWS configuration, and access to the host Docker service. Open only a trusted revision, and review changes to `.devcontainer/` before rebuilding or reopening it. Read-only mounts prevent writes to selected host directories but do not prevent the container from reading their contents.

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-features --workspace --locked
```

The default test suite creates synthetic scenarios and audio locally. Ignored real-corpus regressions require an explicit `KANADE_SOURCE_ROOT` pointing to your own lawfully extracted `romfs/data`; they are never needed for ordinary CI. See `THIRD_PARTY_NOTICES.md` for dependency and fixture provenance. This repository does not grant a license to the tool code.
