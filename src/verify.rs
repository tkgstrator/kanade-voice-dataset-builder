use crate::audio::{CONVERSION_SETTINGS, sha256_file};
use crate::build::{
    DatasetReport, check_manual_review, is_regular, join_snapshot, publication_files, read_source,
    read_source_with_mode,
};
use crate::error::{Error, Result};
use crate::finder;
use crate::manifest::render_final_manifests;
use crate::model::{AcceptedUtterance, RejectedUtterance, SourceEventRef, VoiceAsset};
use crate::paths::DatasetPaths;
use crate::probe::{probe_audio, probe_canonical_wav};
use crate::scenario::ScenarioStructureMode;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path};
use walkdir::WalkDir;

#[derive(Clone, Debug, Default)]
pub struct VerificationConfig {
    pub expected_fingerprint: Option<String>,
}

#[derive(Clone, Debug)]
pub struct VerificationReport {
    pub fingerprint: String,
    pub accepted: Vec<AcceptedUtterance>,
    pub rejected_count: usize,
    pub orphan_audio_count: usize,
    pub physical_audio_count: usize,
    pub duration_seconds: f64,
}

fn fail(detail: impl Into<String>) -> Error {
    Error::Manifest(detail.into())
}
fn io(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        path: path.into(),
        source,
    }
}

fn jsonl<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    if !is_regular(path)? {
        return Err(fail(format!("missing manifest: {}", path.display())));
    }
    let content = fs::read_to_string(path).map_err(|source| io(path, source))?;
    let mut rows = Vec::new();
    for line in content.lines() {
        if line.is_empty() {
            return Err(fail("blank JSONL row"));
        }
        rows.push(serde_json::from_str(line).map_err(|error| fail(error.to_string()))?);
    }
    Ok(rows)
}

fn normal_relative(path: &Path) -> bool {
    path.components().next().is_some()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn event_key(event: &SourceEventRef) -> (&str, usize, usize, usize) {
    (
        &event.source_scn,
        event.scenario_event_index,
        event.scene_index,
        event.scene_text_index,
    )
}

fn check_events(
    events: &[SourceEventRef],
    seen: &mut BTreeSet<(String, usize, usize, usize)>,
) -> Result<()> {
    if events.is_empty() {
        return Err(fail("empty source event provenance"));
    }
    for event in events {
        if !normal_relative(Path::new(&event.source_scn)) || !event.source_scn.starts_with("scn/") {
            return Err(fail("absolute or escaping scenario provenance"));
        }
        let (path, event_index, scene_index, text_index) = event_key(event);
        if !seen.insert((path.to_owned(), event_index, scene_index, text_index)) {
            return Err(fail("duplicate SourceEventRef key"));
        }
    }
    Ok(())
}

fn check_publication(root: &Path) -> Result<()> {
    let path = root.join("publication-manifest.json");
    if root
        .join(".publication-manifest.json.tmp")
        .symlink_metadata()
        .is_ok()
    {
        return Err(fail("publication temporary file remains after sealing"));
    }
    #[derive(Deserialize, PartialEq, Eq)]
    struct FileEntry {
        path: String,
        size: u64,
        sha256: String,
    }
    let bytes = fs::read(&path).map_err(|source| io(&path, source))?;
    let declared: Vec<FileEntry> =
        serde_json::from_slice(&bytes).map_err(|error| fail(error.to_string()))?;
    let observed = publication_files(root)?;
    if declared.len() != observed.len() {
        return Err(fail("publication file count mismatch"));
    }
    for (row, actual) in declared.iter().zip(observed) {
        if !normal_relative(Path::new(&row.path))
            || (row.path.as_str(), row.size, row.sha256.as_str())
                != (actual.0.as_str(), actual.1, actual.2.as_str())
        {
            return Err(fail("publication path, size, or hash mismatch"));
        }
    }
    Ok(())
}

fn verify_rejected_csv(path: &Path, rows: &[RejectedUtterance]) -> Result<()> {
    if !is_regular(path)? {
        return Err(fail("rejected CSV missing"));
    }
    let mut reader = csv::Reader::from_path(path)?;
    if reader.headers()?.iter().collect::<Vec<_>>()
        != [
            "voice_id",
            "source_ogg",
            "source_events",
            "source_observations",
            "stage",
            "reason_code",
            "detail",
        ]
    {
        return Err(fail("rejected CSV header mismatch"));
    }
    let records = reader
        .records()
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if records.len() != rows.len() {
        return Err(fail("rejected CSV row count mismatch"));
    }
    for (record, row) in records.iter().zip(rows) {
        let events =
            serde_json::to_string(&row.source_events).map_err(|error| fail(error.to_string()))?;
        let observations = serde_json::to_string(&row.source_observations)
            .map_err(|error| fail(error.to_string()))?;
        let reason =
            serde_json::to_string(&row.reason_code).map_err(|error| fail(error.to_string()))?;
        let source = row
            .source_ogg
            .as_ref()
            .map_or_else(String::new, |path| path.to_string_lossy().into_owned());
        if record.iter().collect::<Vec<_>>()
            != [
                row.voice_id.as_deref().unwrap_or(""),
                &source,
                &events,
                &observations,
                &row.stage,
                reason.trim_matches('"'),
                &row.detail,
            ]
        {
            return Err(fail("rejected CSV/JSONL provenance mismatch"));
        }
    }
    Ok(())
}

fn check_metadata_directories(root: &Path) -> Result<()> {
    for (directory, allowed) in [
        ("manifests", &["accepted.jsonl"][..]),
        (
            "reports",
            &[
                "rejected.jsonl",
                "rejected.csv",
                "orphan-audio.jsonl",
                "missing-audio.jsonl",
                "manual-review.jsonl",
                "join-summary.json",
            ][..],
        ),
    ] {
        let path = root.join(directory);
        let meta = path
            .symlink_metadata()
            .map_err(|source| io(&path, source))?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(fail(format!("unsafe {directory} directory")));
        }
        for entry in fs::read_dir(&path).map_err(|source| io(&path, source))? {
            let entry = entry.map_err(|source| io(&path, source))?;
            let meta = entry
                .path()
                .symlink_metadata()
                .map_err(|source| io(&entry.path(), source))?;
            if !meta.is_file()
                || meta.file_type().is_symlink()
                || !allowed.contains(&entry.file_name().to_string_lossy().as_ref())
            {
                return Err(fail(format!(
                    "unexpected {directory} entry: {}",
                    entry.path().display()
                )));
            }
        }
    }
    Ok(())
}

pub fn verify_dataset(
    dataset_root: &Path,
    source_root: &Path,
    config: &VerificationConfig,
    ffprobe: &Path,
) -> Result<VerificationReport> {
    verify_dataset_with_mode(
        dataset_root,
        source_root,
        config,
        ffprobe,
        ScenarioStructureMode::Pinned,
    )
}

/// Synthetic source verification for integration tests. No CLI path selects this mode.
#[doc(hidden)]
pub fn verify_fixture_dataset_for_tests(
    dataset_root: &Path,
    source_root: &Path,
    config: &VerificationConfig,
    ffprobe: &Path,
) -> Result<VerificationReport> {
    verify_dataset_with_mode(
        dataset_root,
        source_root,
        config,
        ffprobe,
        ScenarioStructureMode::TestFixture,
    )
}

pub(crate) fn verify_dataset_with_mode(
    dataset_root: &Path,
    source_root: &Path,
    config: &VerificationConfig,
    ffprobe: &Path,
    mode: ScenarioStructureMode,
) -> Result<VerificationReport> {
    verify_dataset_inner(
        dataset_root,
        source_root,
        config,
        ffprobe,
        false,
        true,
        mode,
    )
}

pub(crate) fn verify_interrupted_work_with_mode(
    dataset_root: &Path,
    source_root: &Path,
    ffprobe: &Path,
    mode: ScenarioStructureMode,
) -> Result<VerificationReport> {
    verify_dataset_inner(
        dataset_root,
        source_root,
        &VerificationConfig::default(),
        ffprobe,
        true,
        true,
        mode,
    )
}

pub(crate) fn verify_strict_interrupted_work_with_mode(
    dataset_root: &Path,
    source_root: &Path,
    ffprobe: &Path,
    mode: ScenarioStructureMode,
) -> Result<VerificationReport> {
    verify_dataset_inner(
        dataset_root,
        source_root,
        &VerificationConfig::default(),
        ffprobe,
        true,
        false,
        mode,
    )
}

fn verify_dataset_inner(
    dataset_root: &Path,
    source_root: &Path,
    config: &VerificationConfig,
    ffprobe: &Path,
    interrupted_finalize: bool,
    allow_finder: bool,
    mode: ScenarioStructureMode,
) -> Result<VerificationReport> {
    let paths = DatasetPaths::new(source_root, dataset_root)?;
    let root = paths.output_root;
    if !root.is_dir() {
        return Err(fail("dataset root missing"));
    }
    if root
        .symlink_metadata()
        .map_err(|source| io(&root, source))?
        .file_type()
        .is_symlink()
    {
        return Err(fail("dataset root is a symlink"));
    }
    let work = root
        .file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.ends_with(".work"));
    let snapshot = match mode {
        ScenarioStructureMode::Pinned => read_source(&paths.source_root)?,
        ScenarioStructureMode::TestFixture => {
            read_source_with_mode(&paths.source_root, ScenarioStructureMode::TestFixture)?
        }
    };
    if config
        .expected_fingerprint
        .as_ref()
        .is_some_and(|expected| expected != &snapshot.fingerprint)
    {
        return Err(fail("source/config fingerprint drift"));
    }
    let source_assets = snapshot.inventory.assets.clone();
    let source_catalog = snapshot.catalog.speakers.clone();
    let expected = join_snapshot(snapshot);
    let fingerprint = expected.fingerprint.clone();
    let report_path = root.join("report.json");
    let report: DatasetReport =
        serde_json::from_slice(&fs::read(&report_path).map_err(|source| io(&report_path, source))?)
            .map_err(|error| fail(error.to_string()))?;
    if report.schema_version != "kanade-report-v1"
        || report.tool_version != env!("CARGO_PKG_VERSION")
        || report.source_config_fingerprint != fingerprint
        || report.conversion_settings != CONVERSION_SETTINGS
        || report.speakers != source_catalog
        || report.physical_audio_count != expected.physical_audio_count
        || report.normalization_version != "kanade-text-v2"
    {
        return Err(fail("report/source fingerprint or configuration mismatch"));
    }
    if work && root.join(".source-config-fingerprint.json").exists() {
        let marker = root.join(".source-config-fingerprint.json");
        if !is_regular(&marker)? {
            return Err(fail("unsafe work-root fingerprint marker"));
        }
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(&marker).map_err(|source| io(&marker, source))?)
                .map_err(|error| fail(error.to_string()))?;
        if value
            .get("source_config_fingerprint")
            .and_then(|v| v.as_str())
            != Some(&fingerprint)
        {
            return Err(fail("work-root fingerprint mismatch"));
        }
    } else if !work && root.join(".source-config-fingerprint.json").exists() {
        return Err(fail("work marker in final dataset"));
    } else if work && !root.join("dataset-complete.json").exists() && !interrupted_finalize {
        return Err(fail("unmarked incomplete work root"));
    }
    let marked_unsealed = allow_finder
        && work
        && root.join(".source-config-fingerprint.json").exists()
        && !root.join("dataset-complete.json").exists()
        && !root.join("publication-manifest.json").exists();
    for entry in fs::read_dir(&root).map_err(|source| io(&root, source))? {
        let entry = entry.map_err(|source| io(&root, source))?;
        let name = entry.file_name();
        if name == ".DS_Store" {
            finder::recognized_at(&root, &entry.path(), marked_unsealed)?;
            continue;
        }
        if work
            && interrupted_finalize
            && matches!(
                name.to_str(),
                Some(".dataset-complete.json.tmp" | ".publication-manifest.json.tmp")
            )
        {
            continue;
        }
        if !matches!(
            name.to_str(),
            Some(
                ".source-config-fingerprint.json"
                    | "metadata.csv"
                    | "speakers.json"
                    | "report.json"
                    | "README.md"
                    | "dataset-complete.json"
                    | "publication-manifest.json"
                    | "audio"
                    | "manifests"
                    | "reports"
            )
        ) {
            return Err(fail(format!(
                "unexpected dataset-root entry: {}",
                entry.path().display()
            )));
        }
    }
    check_metadata_directories(&root)?;
    let speakers_path = root.join("speakers.json");
    let speakers: BTreeMap<String, String> = serde_json::from_slice(
        &fs::read(&speakers_path).map_err(|source| io(&speakers_path, source))?,
    )
    .map_err(|error| fail(error.to_string()))?;
    if speakers != source_catalog {
        return Err(fail("speaker catalog mismatch"));
    }
    let accepted: Vec<AcceptedUtterance> = jsonl(&root.join("manifests/accepted.jsonl"))?;
    let rejected: Vec<RejectedUtterance> = jsonl(&root.join("reports/rejected.jsonl"))?;
    let orphan: Vec<VoiceAsset> = jsonl(&root.join("reports/orphan-audio.jsonl"))?;
    let missing: Vec<RejectedUtterance> = jsonl(&root.join("reports/missing-audio.jsonl"))?;
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct JoinSummary {
        physical_audio_count: usize,
        matched_count: usize,
        rejected_count: usize,
        orphan_audio_count: usize,
        missing_audio_count: usize,
    }
    let summary_path = root.join("reports/join-summary.json");
    let summary: JoinSummary = serde_json::from_slice(
        &fs::read(&summary_path).map_err(|source| io(&summary_path, source))?,
    )
    .map_err(|error| fail(error.to_string()))?;
    if summary.physical_audio_count != expected.physical_audio_count
        || summary.matched_count != expected.matched.len()
        || summary.rejected_count != expected.rejected.len()
        || summary.orphan_audio_count != expected.orphan_audio.len()
        || summary.missing_audio_count != expected.missing_audio.len()
    {
        return Err(fail("join summary differs from independent source join"));
    }
    if rejected != expected.rejected
        || orphan != expected.orphan_audio
        || missing != expected.missing_audio
    {
        return Err(fail(
            "rejected, orphan, or missing source provenance differs from independent join",
        ));
    }
    verify_rejected_csv(&root.join("reports/rejected.csv"), &rejected)?;
    if accepted.len() != expected.matched.len() {
        return Err(fail("accepted/join count mismatch"));
    }
    let by_source: BTreeMap<_, _> = source_assets
        .iter()
        .map(|asset| (&asset.source_ogg, asset))
        .collect();
    let by_match: BTreeMap<_, _> = expected
        .matched
        .iter()
        .map(|item| (&item.audio.source_ogg, item))
        .collect();
    let mut ids = BTreeSet::new();
    let mut wav_paths = BTreeSet::new();
    let mut source_paths = BTreeSet::new();
    let mut event_keys = BTreeSet::new();
    let mut duration = 0.0;
    for row in &accepted {
        if row.schema_version != "kanade-accepted-v1"
            || row.language != "ja"
            || row.spoken_text.trim().is_empty()
            || row.audio_path.components().count() != 3
            || !normal_relative(&row.audio_path)
            || !row.audio_path.starts_with("audio")
            || row.audio_path.extension().is_none_or(|ext| ext != "wav")
            || !normal_relative(&row.source_ogg)
            || row.source_ogg.components().count() != 2
            || !ids.insert(&row.utterance_id)
            || !wav_paths.insert(&row.audio_path)
            || !source_paths.insert(&row.source_ogg)
        {
            return Err(fail(
                "invalid accepted identity, transcript, or upload path",
            ));
        }
        check_events(&row.source_events, &mut event_keys)?;
        if row.primary_source_event != row.source_events[0]
            || row.source_scn != row.primary_source_event.source_scn
            || row.scenario_event_index != row.primary_source_event.scenario_event_index
            || row
                .source_observations
                .iter()
                .map(|o| &o.source_event)
                .collect::<Vec<_>>()
                != row.source_events.iter().collect::<Vec<_>>()
        {
            return Err(fail("accepted event provenance mismatch"));
        }
        let asset = by_source
            .get(&row.source_ogg)
            .ok_or_else(|| fail("accepted OGG not in physical inventory"))?;
        let joined = by_match
            .get(&row.source_ogg)
            .ok_or_else(|| fail("accepted OGG absent from independent join"))?;
        if row.voice_id != asset.voice_id
            || row.speaker_name != asset.source_speaker
            || row.speaker_id != joined.speaker_id
            || row.source_sha256 != asset.source_sha256
            || row.source_events != joined.source_events
            || row.source_observations != joined.source_observations
            || row.raw_text != joined.candidate.raw_text.as_deref().unwrap_or_default()
            || row.display_text != joined.display_text
            || row.spoken_text != joined.spoken_text
            || row.normalization_version != joined.normalization_version
            || row.quality_flags != joined.quality_flags
        {
            return Err(fail(
                "accepted text, speaker, or provenance differs from independent join",
            ));
        }
        let source = paths.source_root.join("voice").join(&row.source_ogg);
        if !is_regular(&source)?
            || source.canonicalize().map_err(|error| io(&source, error))? != source
            || sha256_file(&source)? != row.source_sha256
            || fs::metadata(&source)
                .map_err(|error| io(&source, error))?
                .len()
                != asset.byte_size
        {
            return Err(fail("physical source OGG identity mismatch"));
        }
        let wav = root.join(&row.audio_path);
        if !is_regular(&wav)?
            || wav.canonicalize().map_err(|error| io(&wav, error))? != wav
            || sha256_file(&wav)? != row.output_sha256
        {
            return Err(fail("accepted WAV missing or hash mismatch"));
        }
        let probe = probe_canonical_wav(ffprobe, &wav)?;
        let source_probe = probe_audio(ffprobe, &source)?;
        if probe.codec != "pcm_s16le"
            || probe.sample_rate != 44100
            || probe.channels != 1
            || row.output_sample_format != "s16"
            || row.output_sample_rate != 44100
            || row.output_channels != 1
            || (probe.duration_seconds - row.duration_seconds).abs() > 0.001
            || row.source_codec != source_probe.codec
            || row.source_sample_rate != source_probe.sample_rate
            || row.source_channels != source_probe.channels
        {
            return Err(fail("accepted audio probe mismatch"));
        }
        duration += row.duration_seconds;
    }
    for row in &rejected {
        let mut row_keys = BTreeSet::new();
        check_events(&row.source_events, &mut row_keys)?;
        if row_keys.iter().any(|key| event_keys.contains(key)) {
            return Err(fail("accepted and rejected provenance overlaps"));
        }
    }
    let rendered = render_final_manifests(&accepted)?;
    if fs::read(root.join("manifests/accepted.jsonl")).map_err(|error| io(&root, error))?
        != rendered.accepted_jsonl
        || fs::read(root.join("metadata.csv")).map_err(|error| io(&root, error))?
            != rendered.metadata_csv
    {
        return Err(fail("metadata CSV and accepted JSONL differ"));
    }
    if report.accepted_count != accepted.len()
        || report.rejected_count != rejected.len()
        || report.orphan_audio_count != orphan.len()
        || report.missing_audio_count != missing.len()
        || (report.duration_seconds - duration).abs() > 0.001
        || accepted.len()
            + orphan.len()
            + rejected.iter().filter(|r| r.source_ogg.is_some()).count()
            != report.physical_audio_count
    {
        return Err(fail("report accounting or duration mismatch"));
    }
    let audio_root = root.join("audio");
    if audio_root.exists() {
        for entry in WalkDir::new(&audio_root).follow_links(false) {
            let entry = entry?;
            if entry.file_type().is_symlink()
                || (!entry.file_type().is_file() && !entry.file_type().is_dir())
            {
                return Err(fail("unsafe audio output entry"));
            }
            if entry.file_type().is_file() {
                let rel = entry
                    .path()
                    .strip_prefix(&root)
                    .map_err(|_| fail("WAV escapes root"))?;
                if !wav_paths.contains(&rel.to_path_buf())
                    && !(work && rel.to_string_lossy().ends_with(".wav.state.json"))
                    && !finder::recognized_at(&root, entry.path(), marked_unsealed)?
                {
                    return Err(fail("unmanifested audio file"));
                }
            }
        }
    } else if !accepted.is_empty() {
        return Err(fail("audio directory missing"));
    }
    let completion = root.join("dataset-complete.json");
    let publication = root.join("publication-manifest.json");
    if !work || completion.exists() || publication.exists() {
        if !is_regular(&completion)? || (!interrupted_finalize && !is_regular(&publication)?) {
            return Err(fail("incomplete completion/publication pair"));
        }
        let marker: serde_json::Value =
            serde_json::from_slice(&fs::read(&completion).map_err(|error| io(&completion, error))?)
                .map_err(|error| fail(error.to_string()))?;
        if marker.get("schema_version").and_then(|v| v.as_str()) != Some("kanade-complete-v1")
            || marker
                .get("source_config_fingerprint")
                .and_then(|v| v.as_str())
                != Some(&fingerprint)
            || marker.get("accepted_count").and_then(|v| v.as_u64()) != Some(accepted.len() as u64)
        {
            return Err(fail("completion marker mismatch"));
        }
        if publication.exists() {
            check_publication(&root)?;
        }
        check_manual_review(&root, &accepted)?;
    }
    Ok(VerificationReport {
        fingerprint,
        accepted,
        rejected_count: rejected.len(),
        orphan_audio_count: orphan.len(),
        physical_audio_count: report.physical_audio_count,
        duration_seconds: duration,
    })
}

#[cfg(test)]
mod tests {
    use super::verify_rejected_csv;
    use crate::model::{RejectReason, RejectedUtterance};
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn rejected_csv_must_match_canonical_jsonl_provenance() {
        let path = std::env::temp_dir().join(format!("kanade-rejected-{}.csv", std::process::id()));
        let row = RejectedUtterance {
            voice_id: Some("kan_1".into()),
            source_ogg: Some(PathBuf::from("カナデ/KAN_1.ogg")),
            source_events: vec![],
            source_observations: vec![],
            stage: "join".into(),
            reason_code: RejectReason::InvalidAudio,
            detail: "bad source".into(),
        };
        fs::write(&path, b"voice_id,source_ogg,source_events,source_observations,stage,reason_code,detail\nkan_1,\xE3\x82\xAB\xE3\x83\x8A\xE3\x83\x87/KAN_1.ogg,[],[],join,invalid_audio,changed\n").unwrap();
        assert!(verify_rejected_csv(&path, &[row]).is_err());
        fs::remove_file(&path).unwrap();
    }
}
