# GitHub Release Design

## Intent

Turn the existing manual `deployment.yaml` packaging workflow into the permanent release path for the Rust CLI. A successful run from the default branch must publish the two verified native packages and their checksums as GitHub Release assets.

## Goals

- Derive the release tag from the `kanade-tts` version in `Cargo.toml`.
- Build and smoke-test Linux x86_64 and macOS arm64 binaries at one immutable commit.
- Publish exactly two tarballs plus `SHA256SUMS` under `v<package-version>`.
- Keep build jobs read-only and grant `contents: write` only to the publishing job.
- Refuse non-default-branch runs, existing tags, and existing releases instead of overwriting them.
- Verify the draft's assets before publishing it.

## Non-goals

- Cross-compiling additional targets.
- Code signing or notarizing the macOS binary.
- Replacing Cargo's version with a separately maintained workflow version.
- Automatically deleting or overwriting a partial draft after a failed run.

## Approach

The workflow has three stages: prepare, package, and release. Prepare validates the selected ref, derives and validates the SemVer tag, and fails closed if that tag already exists. The package matrix checks out the prepared commit and retains the existing build, smoke-test, and archive allowlist. With its write-scoped token, release first checks for any matching published or draft release, downloads the two exact current-run artifacts, rechecks both archives, generates and verifies `SHA256SUMS`, creates the tag at the prepared commit, creates a draft release, verifies the draft asset names, sizes, and SHA-256 digests, and only then publishes it.

Alternatives considered were keeping the seven-day Actions artifacts only, or creating a release directly without a draft. The former is not a permanent release; the latter exposes a greater partial-publication window. A tag supplied as free-form dispatch input was also rejected because the Cargo package version is the authoritative version and requires no shell-facing user input.

## Failure and Recovery

Concurrency is serialized without cancellation so a second dispatch cannot interrupt publication. Existing tags or releases are never moved or clobbered. A failure after tag or draft creation remains visible for deliberate inspection; the workflow reports the recovery state instead of silently deleting remote state.

