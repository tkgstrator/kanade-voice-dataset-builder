# KANADE voice dataset builder

This Rust command line tool inspects and prepares a local, auditable voice dataset from **your own lawfully obtained and extracted** KANADE `romfs/data` tree. The supported source is the Nintendo Switch release of [KANADE on the Japanese My Nintendo Store](https://store-jp.nintendo.com/item/software/D70010000112377) (software ID `D70010000112377`). Source fingerprints are intentionally fail-closed; other platforms, editions, or game builds are unsupported unless they are explicitly added and tested. It does not include decryption keys, game scripts, scenarios, audio, other game assets, or a generated dataset. The game and its content remain subject to the rights of their respective owners. Use and share any resulting dataset only where you have the necessary rights; this repository provides no permission to redistribute game content.

The supported runtime platforms are macOS and Linux. Windows is not a supported runtime. Install Rust 1.98.1 (the repository toolchain file pins it), Cargo, `ffmpeg`, and `ffprobe`. The devcontainer supplies these tools on Ubuntu 24.04. `inspect` and `build` require working audio tools; `pilot` needs only the extracted source and an output location.

## Workflow

Choose a source directory whose children include `voice/`, `scn/`, and the runtime files checked by the KANADE fingerprint configuration. Put outputs outside the source tree. Replace the example paths below with your local paths.

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

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-features --workspace --locked
```

The default test suite creates synthetic scenarios and audio locally. Ignored real-corpus regressions require an explicit `KANADE_SOURCE_ROOT` pointing to your own lawfully extracted `romfs/data`; they are never needed for ordinary CI. See `THIRD_PARTY_NOTICES.md` for dependency and fixture provenance. This repository does not grant a license to the tool code.
