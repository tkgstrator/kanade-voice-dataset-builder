# GitHub Release Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Publish the Rust CLI's Linux and macOS packages as a verified, versioned GitHub Release from `deployment.yaml`.

**Architecture:** A prepare job validates the default-branch commit and derives the Cargo version. Existing native package jobs build that exact commit. A least-privilege release job verifies the downloaded archives, creates checksums and a fixed tag, validates a draft release, and publishes it.

**Tech Stack:** GitHub Actions, Cargo, GitHub CLI, POSIX shell, `actionlint`

**Spec:** `docs/superpowers/specs/2026-10-24-github-release-design.md`

## Global Constraints

- Run only from the repository default branch.
- Derive `v0.1.0` from the checked-out `kanade-tts` Cargo package; do not duplicate the version in YAML.
- Release exactly Linux x86_64 and macOS arm64 tarballs plus `SHA256SUMS`.
- Pin every third-party action to a full commit SHA.
- Give only the release job `contents: write`; never overwrite a tag, release, or asset.
- Keep in-progress release runs serialized and never cancel one automatically.

## Review Focus

- A dispatch from a branch or tag other than the default branch must fail before building.
- An existing tag or release must fail closed without mutation.
- Missing, extra, renamed, or malformed archives must prevent release creation.
- A release must never be published before all three assets are verified.
- A partial tag or draft left by an API failure must remain inspectable and must not be silently clobbered.

---

### Task 1: Versioned release workflow

**Files:**
- Modify: `.github/workflows/deployment.yaml`

**Interfaces:**
- Consumes: `Cargo.toml` package name/version, the workflow dispatch commit, two matrix artifacts
- Produces: tag `v<version>`, a published GitHub Release, two tarballs, and `SHA256SUMS`

- [ ] **Step 1: Run the structural contract against the existing workflow**

Expected: FAIL because the `prepare` and `release` jobs do not exist.

- [ ] **Step 2: Add prepare, immutable package checkout, and release jobs**

Use the design's branch, version, permission, archive, checksum, tag, draft, and publication constraints.

- [ ] **Step 3: Run the structural contract and `actionlint` with its documented queue-schema exclusion**

Run `actionlint -ignore 'unexpected key "queue" for "concurrency" section'` because actionlint 1.7.12 predates GitHub's documented `concurrency.queue` key. Expected: the contract and all remaining actionlint checks pass with no warnings.

- [ ] **Step 4: Run the Rust verification suite**

Run: `cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-targets --all-features`

Expected: formatting and Clippy succeed; all non-ignored tests pass.

- [ ] **Step 5: Commit and push the workflow change through guarded Git**

Commit message: `ci: publish versioned release assets`

### Task 2: Live release and artifact audit

**Files:**
- No repository files

**Interfaces:**
- Consumes: pushed `master` workflow and its run ID
- Produces: verified GitHub Release `v0.1.0`

- [ ] **Step 1: Dispatch `deployment.yaml` on `master` and wait for completion**

Expected: prepare, Linux package, macOS package, and release jobs all succeed.

- [ ] **Step 2: Verify remote tag and release metadata**

Expected: `v0.1.0` resolves to the run's immutable head SHA; release is published, not a draft or prerelease.

- [ ] **Step 3: Download and verify release assets**

Expected: exactly the two tarballs and `SHA256SUMS`; `sha256sum -c SHA256SUMS` succeeds; each archive contains only its directory, executable, README, and notices file.

- [ ] **Step 4: Confirm repository and workflow state**

Expected: remote `master` equals local `HEAD`, the worktree is clean, and the Integration workflow for the release commit succeeds.

