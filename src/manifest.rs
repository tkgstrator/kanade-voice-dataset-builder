use crate::audio::sha256_file;
use crate::build::{BuildReport, DatasetReport};
use crate::durable::sync_directory;
use crate::error::{Error, Result};
use crate::finder;
use crate::join::JoinReport;
use crate::model::{AcceptedUtterance, quality_flags_cell};
use crate::paths::{DatasetPaths, portable_relative};
use crate::probe::probe_canonical_wav;
use serde::Serialize;
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path};

#[derive(Debug, PartialEq, Eq)]
pub struct FinalManifests {
    pub accepted_jsonl: Vec<u8>,
    pub metadata_csv: Vec<u8>,
}

fn jsonl<T: Serialize>(rows: impl IntoIterator<Item = T>) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    for row in rows {
        serde_json::to_writer(&mut bytes, &row).map_err(|e| Error::Manifest(e.to_string()))?;
        bytes.push(b'\n');
    }
    Ok(bytes)
}

/// Serializes only already verified accepted rows. This function does not write final metadata.
pub fn render_final_manifests(rows: &[AcceptedUtterance]) -> Result<FinalManifests> {
    let mut rows: Vec<_> = rows.iter().collect();
    rows.sort_by(|a, b| (&a.utterance_id, &a.audio_path).cmp(&(&b.utterance_id, &b.audio_path)));
    let mut ids = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for row in &rows {
        if !ids.insert(&row.utterance_id) || !paths.insert(&row.audio_path) {
            return Err(Error::Manifest(
                "duplicate accepted utterance or audio path".into(),
            ));
        }
        if row
            .audio_path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
            || !row.audio_path.starts_with("audio")
            || row.audio_path.extension().is_none_or(|e| e != "wav")
        {
            return Err(Error::Manifest(
                "accepted audio_path must be a relative audio WAV".into(),
            ));
        }
    }
    let accepted_jsonl = jsonl(rows.iter().copied())?;
    let mut writer = csv::Writer::from_writer(Vec::new());
    writer.write_record([
        "file_name",
        "transcription",
        "speaker",
        "speaker_id",
        "utterance_id",
        "voice_id",
        "source_scn",
        "scenario_event_index",
        "display_text",
        "quality_flags",
    ])?;
    for row in rows {
        let flags = quality_flags_cell(&row.quality_flags);
        let file_name = portable_relative(&row.audio_path)?;
        writer.write_record([
            &file_name,
            &row.spoken_text,
            &row.speaker_name,
            &row.speaker_id,
            &row.utterance_id,
            &row.voice_id,
            &row.source_scn,
            &row.scenario_event_index.to_string(),
            &row.display_text,
            &flags,
        ])?;
    }
    let metadata_csv = writer
        .into_inner()
        .map_err(|e| Error::Manifest(e.to_string()))?;
    Ok(FinalManifests {
        accepted_jsonl,
        metadata_csv,
    })
}

fn io(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        path: path.to_owned(),
        source,
    }
}

fn safe_existing(path: &Path, directory: bool) -> Result<bool> {
    match path.symlink_metadata() {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(Error::Manifest(format!(
            "symlink in work root: {}",
            path.display()
        ))),
        Ok(metadata) if metadata.is_dir() == directory => Ok(true),
        Ok(_) => Err(Error::Manifest(format!(
            "unexpected work-root entry: {}",
            path.display()
        ))),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(io(path, source)),
    }
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let name = path
        .file_name()
        .ok_or_else(|| Error::Manifest("missing output filename".into()))?
        .to_string_lossy();
    let temporary = path.with_file_name(format!(".{name}.tmp"));
    if safe_existing(&temporary, false)? {
        return Err(Error::Manifest(format!(
            "stale temporary file: {}",
            temporary.display()
        )));
    }
    safe_existing(path, false)?;
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| io(&temporary, e))?;
        file.write_all(bytes).map_err(|e| io(&temporary, e))?;
        file.flush().map_err(|e| io(&temporary, e))?;
        file.sync_all().map_err(|e| io(&temporary, e))?;
        fs::rename(&temporary, path).map_err(|e| io(path, e))?;
        sync_directory(path.parent().expect("output has parent"))
            .map_err(|e| io(path.parent().expect("output has parent"), e))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Writes final metadata from rows whose on-disk WAVs and source bytes are verified again.
pub fn write_final_manifests(report: &BuildReport, root: &Path) -> Result<()> {
    if root != report.work_root
        || !root.is_dir()
        || root
            .file_name()
            .and_then(|s| s.to_str())
            .is_none_or(|s| !s.ends_with(".work"))
    {
        return Err(Error::Manifest(
            "final manifests require the matching work root".into(),
        ));
    }
    for name in ["dataset-complete.json", "publication-manifest.json"] {
        if root.join(name).symlink_metadata().is_ok() {
            return Err(Error::Manifest("sealed work root".into()));
        }
    }
    let mut source_paths = BTreeSet::new();
    for row in &report.accepted {
        if row.audio_path.components().count() != 3
            || !row.audio_path.starts_with("audio")
            || !row
                .audio_path
                .components()
                .all(|c| matches!(c, Component::Normal(_)))
            || row.source_ogg.components().count() != 2
            || !row
                .source_ogg
                .components()
                .all(|c| matches!(c, Component::Normal(_)))
            || !source_paths.insert(&row.source_ogg)
        {
            return Err(Error::Manifest(
                "unsafe or duplicate accepted physical path".into(),
            ));
        }
        let wav = root.join(&row.audio_path);
        if !safe_existing(&wav, false)? || sha256_file(&wav)? != row.output_sha256 {
            return Err(Error::Manifest("accepted WAV hash mismatch".into()));
        }
        let probe = probe_canonical_wav(&report.ffprobe, &wav)?;
        if probe.codec != "pcm_s16le"
            || probe.channels != 1
            || probe.sample_rate != 44100
            || (probe.duration_seconds - row.duration_seconds).abs() > 0.001
        {
            return Err(Error::Manifest("accepted WAV probe mismatch".into()));
        }
        let source = report.source_root.join("voice").join(&row.source_ogg);
        if !safe_existing(&source, false)? || sha256_file(&source)? != row.source_sha256 {
            return Err(Error::Manifest("accepted source hash mismatch".into()));
        }
    }
    let rendered = render_final_manifests(&report.accepted)?;
    let manifests = root.join("manifests");
    if !safe_existing(&manifests, true)? {
        fs::create_dir(&manifests).map_err(|e| io(&manifests, e))?;
    }
    atomic_write(&manifests.join("accepted.jsonl"), &rendered.accepted_jsonl)?;
    atomic_write(&root.join("metadata.csv"), &rendered.metadata_csv)?;
    let speakers =
        serde_json::to_vec_pretty(&report.speakers).map_err(|e| Error::Manifest(e.to_string()))?;
    atomic_write(&root.join("speakers.json"), &speakers)?;
    let dataset_report = DatasetReport {
        schema_version: "kanade-report-v1".into(),
        tool_version: env!("CARGO_PKG_VERSION").into(),
        source_config_fingerprint: report.fingerprint.clone(),
        conversion_settings: crate::audio::CONVERSION_SETTINGS.into(),
        normalization_version: "kanade-text-v2".into(),
        physical_audio_count: report.physical_audio_count,
        accepted_count: report.accepted.len(),
        rejected_count: report.rejected.len(),
        orphan_audio_count: report.orphan_audio.len(),
        missing_audio_count: report.missing_audio.len(),
        duration_seconds: report.accepted.iter().map(|row| row.duration_seconds).sum(),
        speakers: report.speakers.clone(),
    };
    let bytes =
        serde_json::to_vec_pretty(&dataset_report).map_err(|e| Error::Manifest(e.to_string()))?;
    atomic_write(&root.join("report.json"), &bytes)
}

fn validate_work_root(report: &JoinReport, root: &Path) -> Result<()> {
    if root
        .file_name()
        .and_then(|s| s.to_str())
        .is_none_or(|s| !s.ends_with(".work"))
    {
        return Err(Error::Manifest(
            "diagnostics require a sibling .work root".into(),
        ));
    }
    if report.fingerprint.is_empty() {
        return Err(Error::Manifest("empty source/config fingerprint".into()));
    }
    DatasetPaths::new(&report.source_root, root)?;
    if safe_existing(root, true)? {
        for name in ["dataset-complete.json", "publication-manifest.json"] {
            let path = root.join(name);
            match path.symlink_metadata() {
                Ok(_) => {
                    return Err(Error::Manifest(format!(
                        "diagnostics cannot mutate a sealed work root: {}",
                        path.display()
                    )));
                }
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => return Err(io(&path, source)),
            }
        }
        let marker = root.join(".source-config-fingerprint.json");
        if !safe_existing(&marker, false)? {
            return Err(Error::Manifest("unrecognized existing work root".into()));
        }
        let marker_bytes = fs::read(&marker).map_err(|e| io(&marker, e))?;
        let old: Fingerprint =
            serde_json::from_slice(&marker_bytes).map_err(|e| Error::Manifest(e.to_string()))?;
        if old.source_config_fingerprint != report.fingerprint {
            return Err(Error::Manifest("source/config fingerprint drift".into()));
        }
        for entry in fs::read_dir(root).map_err(|e| io(root, e))? {
            let entry = entry.map_err(|e| io(root, e))?;
            let name = entry.file_name();
            if name == ".DS_Store" {
                finder::recognized_at(root, &entry.path(), true)?;
                continue;
            }
            let expected_directory = match name.to_str() {
                Some(
                    ".source-config-fingerprint.json"
                    | "metadata.csv"
                    | "speakers.json"
                    | "report.json"
                    | "README.md",
                ) => Some(false),
                Some("audio" | "manifests" | "reports") => Some(true),
                _ => None,
            };
            let Some(expected_directory) = expected_directory else {
                return Err(Error::Manifest(format!(
                    "unexpected existing work-root entry: {}",
                    entry.path().display()
                )));
            };
            safe_existing(&entry.path(), expected_directory)?;
        }
        let reports = root.join("reports");
        if safe_existing(&reports, true)? {
            for entry in fs::read_dir(&reports).map_err(|e| io(&reports, e))? {
                let entry = entry.map_err(|e| io(&reports, e))?;
                if ![
                    "rejected.jsonl",
                    "rejected.csv",
                    "orphan-audio.jsonl",
                    "missing-audio.jsonl",
                    "manual-review.jsonl",
                    "join-summary.json",
                ]
                .contains(&entry.file_name().to_string_lossy().as_ref())
                    || !safe_existing(&entry.path(), false)?
                {
                    return Err(Error::Manifest(format!(
                        "unexpected diagnostics entry: {}",
                        entry.path().display()
                    )));
                }
            }
        }
    } else {
        fs::create_dir(root).map_err(|e| io(root, e))?;
        let marker = root.join(".source-config-fingerprint.json");
        let bytes = serde_json::to_vec(&Fingerprint {
            source_config_fingerprint: &report.fingerprint,
        })
        .map_err(|e| Error::Manifest(e.to_string()))?;
        atomic_write(&marker, &bytes)?;
    }
    Ok(())
}

#[derive(Serialize, serde::Deserialize)]
struct Fingerprint<'a> {
    source_config_fingerprint: &'a str,
}

/// Writes only diagnostics. A partial work root cannot be treated as a complete dataset.
pub fn write_join_diagnostics(report: &JoinReport, root: &Path) -> Result<()> {
    if report.accounted_physical_audio_count() != report.physical_audio_count {
        return Err(Error::Manifest("physical OGG accounting mismatch".into()));
    }
    validate_work_root(report, root)?;
    let dir = root.join("reports");
    if !safe_existing(&dir, true)? {
        fs::create_dir(&dir).map_err(|e| io(&dir, e))?;
    }
    let rejected_jsonl = jsonl(&report.rejected)?;
    let orphan_jsonl = jsonl(&report.orphan_audio)?;
    let missing_jsonl = jsonl(&report.missing_audio)?;
    let mut csv_writer = csv::Writer::from_writer(Vec::new());
    csv_writer.write_record([
        "voice_id",
        "source_ogg",
        "source_events",
        "source_observations",
        "stage",
        "reason_code",
        "detail",
    ])?;
    for row in &report.rejected {
        let events = serde_json::to_string(&row.source_events)
            .map_err(|e| Error::Manifest(e.to_string()))?;
        let observations = serde_json::to_string(&row.source_observations)
            .map_err(|e| Error::Manifest(e.to_string()))?;
        let reason =
            serde_json::to_string(&row.reason_code).map_err(|e| Error::Manifest(e.to_string()))?;
        let source_ogg = row
            .source_ogg
            .as_ref()
            .map(|path| portable_relative(path))
            .transpose()?
            .unwrap_or_default();
        csv_writer.write_record([
            row.voice_id.as_deref().unwrap_or(""),
            &source_ogg,
            &events,
            &observations,
            &row.stage,
            reason.trim_matches('"'),
            &row.detail,
        ])?;
    }
    let rejected_csv = csv_writer
        .into_inner()
        .map_err(|e| Error::Manifest(e.to_string()))?;
    #[derive(Serialize)]
    struct Summary {
        physical_audio_count: usize,
        matched_count: usize,
        rejected_count: usize,
        orphan_audio_count: usize,
        missing_audio_count: usize,
    }
    let summary = serde_json::to_vec(&Summary {
        physical_audio_count: report.physical_audio_count,
        matched_count: report.matched.len(),
        rejected_count: report.rejected.len(),
        orphan_audio_count: report.orphan_audio.len(),
        missing_audio_count: report.missing_audio.len(),
    })
    .map_err(|e| Error::Manifest(e.to_string()))?;
    for (name, bytes) in [
        ("rejected.jsonl", rejected_jsonl),
        ("rejected.csv", rejected_csv),
        ("orphan-audio.jsonl", orphan_jsonl),
        ("missing-audio.jsonl", missing_jsonl),
        ("join-summary.json", summary),
    ] {
        atomic_write(&dir.join(name), &bytes)?;
    }
    Ok(())
}
