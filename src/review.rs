use crate::build::{BuildConfig, is_regular, review_sample_ids, work_path};
use crate::error::{Error, Result};
use crate::manifest::atomic_write;
use crate::model::AcceptedUtterance;
use crate::readme::render_dataset_card;
use crate::scenario::ScenarioStructureMode;
use crate::verify::{VerificationConfig, VerificationReport, verify_dataset_with_mode};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ReviewDecision {
    Pass,
    Fail,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReviewItem {
    pub row: AcceptedUtterance,
    pub decision: Option<ReviewDecision>,
    pub stale: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManualReview {
    utterance_id: String,
    decision: ReviewDecision,
    review_evidence_sha256: String,
}

fn evidence_digests(accepted: &[AcceptedUtterance]) -> Result<BTreeMap<String, String>> {
    let mut digests = BTreeMap::new();
    for row in accepted {
        let bytes = serde_json::to_vec(row).map_err(|error| Error::Manifest(error.to_string()))?;
        let mut hash = Sha256::new();
        hash.update(b"kanade-manual-review-evidence-v1\0");
        hash.update(bytes);
        if digests
            .insert(row.utterance_id.clone(), format!("{:x}", hash.finalize()))
            .is_some()
        {
            return Err(Error::Manifest("duplicate accepted review identity".into()));
        }
    }
    Ok(digests)
}

fn required_ids(accepted: &[AcceptedUtterance]) -> BTreeSet<String> {
    let mut groups = BTreeMap::<String, Vec<String>>::new();
    for row in accepted {
        groups
            .entry(row.speaker_id.clone())
            .or_default()
            .push(row.utterance_id.clone());
    }
    review_sample_ids(&groups)
}

fn read_decisions(
    work: &Path,
    required: &BTreeSet<String>,
) -> Result<BTreeMap<String, ManualReview>> {
    let reports = work.join("reports");
    let metadata = reports.symlink_metadata().map_err(|source| Error::Io {
        path: reports.clone(),
        source,
    })?;
    let canonical = reports.canonicalize().map_err(|source| Error::Io {
        path: reports.clone(),
        source,
    })?;
    let expected = work
        .canonicalize()
        .map_err(|source| Error::Io {
            path: work.to_owned(),
            source,
        })?
        .join("reports");
    if !metadata.is_dir() || metadata.file_type().is_symlink() || canonical != expected {
        return Err(Error::Manifest(
            "unsafe manual-review reports directory".into(),
        ));
    }
    let path = work.join("reports/manual-review.jsonl");
    let temporary = work.join("reports/.manual-review.jsonl.tmp");
    if temporary.symlink_metadata().is_ok() {
        return Err(Error::Manifest("stale manual-review temporary file".into()));
    }
    if !is_regular(&path)? {
        return Ok(BTreeMap::new());
    }
    let content = fs::read_to_string(&path).map_err(|source| Error::Io {
        path: path.clone(),
        source,
    })?;
    let mut decisions = BTreeMap::new();
    for line in content.lines() {
        if line.is_empty() {
            return Err(Error::Manifest("blank manual-review row".into()));
        }
        let row: ManualReview = serde_json::from_str(line)
            .map_err(|error| Error::Manifest(format!("invalid manual-review row: {error}")))?;
        if !required.contains(&row.utterance_id) {
            return Err(Error::Manifest(
                "unexpected manual-review utterance ID".into(),
            ));
        }
        if row.review_evidence_sha256.len() != 64
            || !row
                .review_evidence_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(Error::Manifest(
                "invalid manual-review evidence digest".into(),
            ));
        }
        if decisions.insert(row.utterance_id.clone(), row).is_some() {
            return Err(Error::Manifest(
                "duplicate manual-review utterance ID".into(),
            ));
        }
    }
    Ok(decisions)
}

pub(crate) fn check_manual_review_rows(work: &Path, accepted: &[AcceptedUtterance]) -> Result<()> {
    let required = required_ids(accepted);
    let evidence = evidence_digests(accepted)?;
    let decisions = read_decisions(work, &required)?;
    if decisions.len() != required.len() {
        return Err(Error::Manifest("manual review coverage incomplete".into()));
    }
    if decisions
        .iter()
        .any(|(id, row)| evidence.get(id) != Some(&row.review_evidence_sha256))
    {
        return Err(Error::Manifest("stale manual-review evidence".into()));
    }
    if decisions
        .values()
        .any(|row| row.decision == ReviewDecision::Fail)
    {
        return Err(Error::Manifest(
            "manual review contains a fail decision".into(),
        ));
    }
    Ok(())
}

fn verified_work(
    config: &BuildConfig,
    mode: ScenarioStructureMode,
) -> Result<(std::path::PathBuf, VerificationReport)> {
    let work = work_path(&config.final_root)?;
    if config.final_root.symlink_metadata().is_ok() {
        return Err(Error::Manifest("final root already exists".into()));
    }
    if work
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        return Err(Error::Manifest("work root is a symlink".into()));
    }
    for name in ["dataset-complete.json", "publication-manifest.json"] {
        if work.join(name).symlink_metadata().is_ok() {
            return Err(Error::Manifest(
                "sealed work root cannot be reviewed".into(),
            ));
        }
    }
    let report = verify_dataset_with_mode(
        &work,
        &config.source_root,
        &VerificationConfig::default(),
        &config.ffprobe,
        mode,
    )?;
    let card = work.join("README.md");
    let expected = render_dataset_card(&report, include_str!("../templates/README.md.tmpl"))?;
    if !is_regular(&card)?
        || fs::read(&card).map_err(|source| Error::Io {
            path: card.clone(),
            source,
        })? != expected.as_bytes()
    {
        return Err(Error::Manifest(
            "dataset Card is missing or differs from freshly verified values".into(),
        ));
    }
    Ok((work, report))
}

pub fn review_required(config: &BuildConfig) -> Result<Vec<ReviewItem>> {
    review_required_with_mode(config, ScenarioStructureMode::Pinned)
}

/// Synthetic source review for integration tests. No CLI path selects this mode.
#[doc(hidden)]
pub fn review_fixture_required_for_tests(config: &BuildConfig) -> Result<Vec<ReviewItem>> {
    review_required_with_mode(config, ScenarioStructureMode::TestFixture)
}

fn review_required_with_mode(
    config: &BuildConfig,
    mode: ScenarioStructureMode,
) -> Result<Vec<ReviewItem>> {
    let (work, report) = verified_work(config, mode)?;
    let required = required_ids(&report.accepted);
    let evidence = evidence_digests(&report.accepted)?;
    let decisions = read_decisions(&work, &required)?;
    let mut items = report
        .accepted
        .into_iter()
        .filter(|row| required.contains(&row.utterance_id))
        .map(|row| {
            let previous = decisions.get(&row.utterance_id);
            let stale = previous.is_some_and(|entry| {
                evidence.get(&row.utterance_id) != Some(&entry.review_evidence_sha256)
            });
            ReviewItem {
                decision: previous.filter(|_| !stale).map(|entry| entry.decision),
                stale,
                row,
            }
        })
        .collect::<Vec<_>>();
    items.sort_by(|a, b| {
        (&a.row.speaker_id, &a.row.utterance_id).cmp(&(&b.row.speaker_id, &b.row.utterance_id))
    });
    Ok(items)
}

pub fn record_manual_review(
    config: &BuildConfig,
    utterance_id: &str,
    decision: ReviewDecision,
    correct: bool,
) -> Result<Vec<ReviewItem>> {
    record_manual_review_with_mode(
        config,
        utterance_id,
        decision,
        correct,
        ScenarioStructureMode::Pinned,
    )
}

/// Synthetic source review recording for integration tests. No CLI path selects this mode.
#[doc(hidden)]
pub fn record_fixture_manual_review_for_tests(
    config: &BuildConfig,
    utterance_id: &str,
    decision: ReviewDecision,
    correct: bool,
) -> Result<Vec<ReviewItem>> {
    record_manual_review_with_mode(
        config,
        utterance_id,
        decision,
        correct,
        ScenarioStructureMode::TestFixture,
    )
}

fn record_manual_review_with_mode(
    config: &BuildConfig,
    utterance_id: &str,
    decision: ReviewDecision,
    correct: bool,
    mode: ScenarioStructureMode,
) -> Result<Vec<ReviewItem>> {
    let (work, report) = verified_work(config, mode)?;
    let required = required_ids(&report.accepted);
    let evidence = evidence_digests(&report.accepted)?;
    if !required.contains(utterance_id) {
        return Err(Error::Manifest(
            "utterance ID is not a required review sample".into(),
        ));
    }
    let mut decisions = read_decisions(&work, &required)?;
    for (id, previous) in &decisions {
        if id != utterance_id && evidence.get(id) != Some(&previous.review_evidence_sha256) {
            return Err(Error::Manifest(format!(
                "unrelated stale manual-review evidence: {id}"
            )));
        }
    }
    if let Some(previous) = decisions.get(utterance_id) {
        let stale = evidence.get(utterance_id) != Some(&previous.review_evidence_sha256);
        if (stale || previous.decision != decision) && !correct {
            return Err(Error::Manifest(
                "stale evidence or decision correction requires --correct".into(),
            ));
        }
    }
    decisions.insert(
        utterance_id.to_owned(),
        ManualReview {
            utterance_id: utterance_id.to_owned(),
            decision,
            review_evidence_sha256: evidence
                .get(utterance_id)
                .expect("required row has evidence")
                .clone(),
        },
    );
    let mut bytes = Vec::new();
    for row in decisions.values() {
        serde_json::to_writer(&mut bytes, row)
            .map_err(|error| Error::Manifest(error.to_string()))?;
        bytes.push(b'\n');
    }
    atomic_write(&work.join("reports/manual-review.jsonl"), &bytes)?;
    review_required_with_mode(config, mode)
}
