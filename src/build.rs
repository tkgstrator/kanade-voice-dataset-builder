use crate::audio::{AudioConfig, CONVERSION_SETTINGS, convert_one, sha256_file};
use crate::error::{Error, Result};
use crate::finder;
use crate::inventory::{VoiceInventory, inventory_voice_root};
use crate::join::{JoinInput, join_candidates};
use crate::manifest::{write_final_manifests, write_join_diagnostics};
use crate::model::{AcceptedUtterance, RejectedUtterance, ScenarioTextEntry, VoiceAsset};
use crate::paths::DatasetPaths;
use crate::readme::render_dataset_card;
use crate::review::check_manual_review_rows;
use crate::scenario::{
    ScenarioStructureMode, extract_fixture_scenario_texts, extract_scenario_texts, load_psb_root,
    reviewed_scenario_names, verify_locale_source,
};
use crate::speakers::{SpeakerAliases, SpeakerCatalog, build_speaker_catalog};
use crate::verify::{
    VerificationConfig, verify_dataset_with_mode, verify_interrupted_work_with_mode,
    verify_strict_interrupted_work_with_mode,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::{ffi::CString, os::unix::ffi::OsStrExt};
use walkdir::WalkDir;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn renamex_np(
        from: *const std::ffi::c_char,
        to: *const std::ffi::c_char,
        flags: u32,
    ) -> std::ffi::c_int;
}

#[cfg(target_os = "linux")]
unsafe extern "C" {
    fn renameat2(
        old_dirfd: std::ffi::c_int,
        old_path: *const std::ffi::c_char,
        new_dirfd: std::ffi::c_int,
        new_path: *const std::ffi::c_char,
        flags: u32,
    ) -> std::ffi::c_int;
}

#[derive(Clone, Debug)]
pub struct BuildConfig {
    pub source_root: PathBuf,
    pub final_root: PathBuf,
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

#[derive(Clone, Debug)]
pub struct BuildReport {
    pub source_root: PathBuf,
    pub work_root: PathBuf,
    pub ffprobe: PathBuf,
    pub fingerprint: String,
    pub accepted: Vec<AcceptedUtterance>,
    pub rejected: Vec<RejectedUtterance>,
    pub orphan_audio: Vec<VoiceAsset>,
    pub missing_audio: Vec<RejectedUtterance>,
    pub speakers: BTreeMap<String, String>,
    pub physical_audio_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct DatasetReport {
    pub schema_version: String,
    pub tool_version: String,
    pub source_config_fingerprint: String,
    pub conversion_settings: String,
    pub normalization_version: String,
    pub physical_audio_count: usize,
    pub accepted_count: usize,
    pub rejected_count: usize,
    pub orphan_audio_count: usize,
    pub missing_audio_count: usize,
    pub duration_seconds: f64,
    pub speakers: BTreeMap<String, String>,
}

pub(crate) struct SourceSnapshot {
    pub inventory: VoiceInventory,
    pub entries: Vec<ScenarioTextEntry>,
    pub catalog: SpeakerCatalog,
    pub fingerprint: String,
    pub root: PathBuf,
}

fn io(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        path: path.into(),
        source,
    }
}

pub(crate) fn promote_noreplace(source: &Path, destination: &Path) -> Result<()> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::os::unix::fs::MetadataExt;
        let source_device = fs::metadata(source)
            .map_err(|error| io(source, error))?
            .dev();
        let parent = destination
            .parent()
            .ok_or_else(|| Error::Manifest("destination has no parent".into()))?;
        let destination_device = fs::metadata(parent)
            .map_err(|error| io(parent, error))?
            .dev();
        if source_device != destination_device {
            return Err(Error::Manifest(
                "no-replace promotion crosses filesystems".into(),
            ));
        }
        let from = CString::new(source.as_os_str().as_bytes())
            .map_err(|_| Error::Manifest("NUL in work path".into()))?;
        let to = CString::new(destination.as_os_str().as_bytes())
            .map_err(|_| Error::Manifest("NUL in final path".into()))?;
        #[cfg(target_os = "macos")]
        // RENAME_EXCL is 0x00000004 in the macOS SDK sys/stdio.h.
        let result = unsafe { renamex_np(from.as_ptr(), to.as_ptr(), 0x0000_0004) };
        #[cfg(target_os = "linux")]
        // RENAME_NOREPLACE is 1; AT_FDCWD is -100.
        let result = unsafe { renameat2(-100, from.as_ptr(), -100, to.as_ptr(), 1) };
        if result != 0 {
            return Err(io(destination, std::io::Error::last_os_error()));
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (source, destination);
        Err(Error::Manifest(
            "atomic no-replace directory promotion is unavailable".into(),
        ))
    }
}

pub(crate) fn is_regular(path: &Path) -> Result<bool> {
    match path.symlink_metadata() {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(Error::Manifest(format!("unsafe file: {}", path.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(io(path, source)),
    }
}

pub(crate) fn read_source(source_root: &Path) -> Result<SourceSnapshot> {
    read_source_with_mode(source_root, ScenarioStructureMode::Pinned)
}

pub(crate) fn read_source_with_mode(
    source_root: &Path,
    mode: ScenarioStructureMode,
) -> Result<SourceSnapshot> {
    let root = source_root
        .canonicalize()
        .map_err(|source| io(source_root, source))?;
    if mode == ScenarioStructureMode::Pinned {
        verify_locale_source(&root)?;
    }
    let voice_root = root.join("voice");
    for entry in WalkDir::new(&voice_root).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_symlink()
            || (!entry.file_type().is_file() && !entry.file_type().is_dir())
        {
            return Err(Error::Manifest(format!(
                "unsafe voice source entry: {}",
                entry.path().display()
            )));
        }
    }
    let inventory = inventory_voice_root(&voice_root)?;
    let catalog = build_speaker_catalog(&inventory)?;
    SpeakerAliases::kanade(&catalog)?;
    let scenario_root = root.join("scn");
    let scenario_metadata = scenario_root
        .symlink_metadata()
        .map_err(|source| io(&scenario_root, source))?;
    if !scenario_metadata.is_dir()
        || scenario_metadata.file_type().is_symlink()
        || scenario_root
            .canonicalize()
            .map_err(|source| io(&scenario_root, source))?
            != scenario_root
    {
        return Err(Error::Manifest("unsafe scenario root".into()));
    }
    let mut scenarios = Vec::new();
    for entry in fs::read_dir(&scenario_root).map_err(|source| io(&scenario_root, source))? {
        let entry = entry.map_err(|source| io(&scenario_root, source))?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "scn") {
            if !is_regular(&path)? {
                return Err(Error::Manifest(format!(
                    "missing scenario: {}",
                    path.display()
                )));
            }
            if !path
                .canonicalize()
                .map_err(|source| io(&path, source))?
                .starts_with(&scenario_root)
            {
                return Err(Error::Manifest("scenario PSB escapes source root".into()));
            }
            scenarios.push(path);
        }
    }
    scenarios.sort();
    if scenarios.is_empty() {
        return Err(Error::Manifest("no scenario PSB files".into()));
    }
    let observed: BTreeSet<_> = scenarios
        .iter()
        .filter_map(|path| path.file_name().and_then(|name| name.to_str()))
        .collect();
    let expected: BTreeSet<_> = match mode {
        ScenarioStructureMode::Pinned => reviewed_scenario_names().collect(),
        ScenarioStructureMode::TestFixture => ["pilot.ks.scn"].into_iter().collect(),
    };
    if observed != expected {
        return Err(Error::Manifest("unrecognized scenario source set".into()));
    }
    let mut entries = Vec::new();
    let mut hash = source_config_hasher(
        &root,
        include_bytes!("../config/line-structure-fingerprints.json"),
    );
    for asset in &inventory.assets {
        hash.update(asset.source_ogg.to_string_lossy().as_bytes());
        hash.update(asset.byte_size.to_le_bytes());
        hash.update(asset.source_sha256.as_bytes());
    }
    for sidecar in &inventory.sidecars {
        hash.update(sidecar.to_string_lossy().as_bytes());
        hash.update(sha256_file(&voice_root.join(sidecar))?.as_bytes());
    }
    for path in scenarios {
        let relative = path
            .strip_prefix(&root)
            .map_err(|_| Error::Manifest("scenario path escapes source".into()))?;
        hash.update(relative.to_string_lossy().as_bytes());
        hash.update(sha256_file(&path)?.as_bytes());
        let psb = load_psb_root(&path)?;
        entries.extend(
            (match mode {
                ScenarioStructureMode::Pinned => extract_scenario_texts(&psb, relative),
                ScenarioStructureMode::TestFixture => {
                    extract_fixture_scenario_texts(&psb, relative)
                }
            })
            .map_err(|error| match error {
                Error::ScenarioSchema { code, path } => Error::ScenarioSchema {
                    code,
                    path: format!("{}:{path}", relative.display()),
                },
                other => other,
            })?,
        );
    }
    Ok(SourceSnapshot {
        inventory,
        entries,
        catalog,
        fingerprint: format!("{:x}", hash.finalize()),
        root,
    })
}

fn source_config_hasher(root: &Path, line_structure_config: &[u8]) -> Sha256 {
    let mut hash = Sha256::new();
    hash.update(b"kanade-source-config-v1\0");
    hash.update(root.to_string_lossy().as_bytes());
    hash.update(CONVERSION_SETTINGS.as_bytes());
    hash.update(include_bytes!("../config/locale-map.json"));
    hash.update(include_bytes!("../config/speaker-aliases.json"));
    hash.update(line_structure_config);
    hash
}

pub(crate) fn join_snapshot(snapshot: SourceSnapshot) -> crate::join::JoinReport {
    let aliases =
        SpeakerAliases::kanade(&snapshot.catalog).expect("source snapshot validated aliases");
    join_candidates(JoinInput {
        inventory: snapshot.inventory,
        entries: snapshot.entries,
        catalog: snapshot.catalog,
        aliases,
        source_root: snapshot.root,
        fingerprint: snapshot.fingerprint,
    })
}

pub(crate) fn work_path(final_root: &Path) -> Result<PathBuf> {
    let name = final_root
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| Error::Manifest("missing final root name".into()))?;
    if name != "KANADE-v1" {
        return Err(Error::Manifest(
            "only KANADE-v1 output version is supported".into(),
        ));
    }
    Ok(final_root.with_file_name(format!("{name}.work")))
}

pub fn build_dataset(config: &BuildConfig) -> Result<BuildReport> {
    build_dataset_inner(config, ScenarioStructureMode::Pinned)
}

/// Synthetic source build for integration tests. No CLI path selects this mode.
#[doc(hidden)]
pub fn build_fixture_dataset_for_tests(config: &BuildConfig) -> Result<BuildReport> {
    build_dataset_inner(config, ScenarioStructureMode::TestFixture)
}

fn build_dataset_inner(config: &BuildConfig, mode: ScenarioStructureMode) -> Result<BuildReport> {
    let work = work_path(&config.final_root)?;
    if work
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        return Err(Error::Manifest("work root is a symlink".into()));
    }
    let paths = DatasetPaths::new(&config.source_root, &work)?;
    if config.final_root.symlink_metadata().is_ok() {
        return Err(Error::Manifest("final root already exists".into()));
    }
    if !paths.output_root.parent().is_some_and(Path::is_dir) {
        return Err(Error::Manifest("output parent does not exist".into()));
    }
    let snapshot = match mode {
        ScenarioStructureMode::Pinned => read_source(&paths.source_root)?,
        ScenarioStructureMode::TestFixture => {
            read_source_with_mode(&paths.source_root, ScenarioStructureMode::TestFixture)?
        }
    };
    let speakers = snapshot.catalog.speakers.clone();
    let join = join_snapshot(snapshot);
    write_join_diagnostics(&join, &paths.output_root)?;
    let audio_config = AudioConfig {
        source_root: paths.source_root.clone(),
        work_root: paths.output_root.clone(),
        ffmpeg: config.ffmpeg.clone(),
        ffprobe: config.ffprobe.clone(),
    };
    let mut accepted = Vec::new();
    for item in &join.matched {
        accepted.push(convert_one(&audio_config, item)?);
    }
    accepted.sort_by(|a, b| a.utterance_id.cmp(&b.utterance_id));
    let report = BuildReport {
        source_root: paths.source_root,
        work_root: paths.output_root.clone(),
        ffprobe: config.ffprobe.clone(),
        fingerprint: join.fingerprint,
        accepted,
        rejected: join.rejected,
        orphan_audio: join.orphan_audio,
        missing_audio: join.missing_audio,
        speakers,
        physical_audio_count: join.physical_audio_count,
    };
    write_final_manifests(&report, &paths.output_root)?;
    Ok(report)
}

/// Deterministic per-speaker sample used by review and finalization.
pub fn review_sample_ids(groups: &BTreeMap<String, Vec<String>>) -> BTreeSet<String> {
    let mut required = BTreeSet::new();
    for ids in groups.values() {
        let mut ids = ids.clone();
        ids.sort();
        if ids.len() < 5 {
            required.extend(ids);
            continue;
        }
        required.insert(ids[0].clone());
        required.insert(ids[ids.len() / 2].clone());
        required.insert(ids[ids.len() - 1].clone());
        let mut candidates: Vec<_> = ids
            .into_iter()
            .filter(|id| !required.contains(id))
            .map(|id| (format!("{:x}", Sha256::digest(id.as_bytes())), id))
            .collect();
        candidates.sort();
        required.extend(candidates.into_iter().take(2).map(|(_, id)| id));
    }
    required
}

pub(crate) fn check_manual_review(work: &Path, accepted: &[AcceptedUtterance]) -> Result<()> {
    check_manual_review_rows(work, accepted)
}

#[derive(Serialize)]
struct Completion<'a> {
    schema_version: &'static str,
    source_config_fingerprint: &'a str,
    accepted_count: usize,
}

#[derive(Serialize)]
struct PublicationFile {
    path: String,
    size: u64,
    sha256: String,
}

fn allowed_file(relative: &Path) -> bool {
    if relative
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return false;
    }
    let text = relative.to_string_lossy();
    matches!(
        text.as_ref(),
        "metadata.csv"
            | "speakers.json"
            | "report.json"
            | "README.md"
            | "dataset-complete.json"
            | "manifests/accepted.jsonl"
            | "reports/rejected.jsonl"
            | "reports/rejected.csv"
            | "reports/orphan-audio.jsonl"
            | "reports/missing-audio.jsonl"
            | "reports/manual-review.jsonl"
            | "reports/join-summary.json"
    ) || (relative.components().count() == 3
        && text.starts_with("audio/")
        && text.ends_with(".wav"))
}

pub(crate) fn publication_files(root: &Path) -> Result<Vec<(String, u64, String)>> {
    let mut files = Vec::new();
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        if entry.path() == root {
            continue;
        }
        if entry.file_type().is_symlink()
            || (!entry.file_type().is_file() && !entry.file_type().is_dir())
        {
            return Err(Error::Manifest(format!(
                "unsafe publication entry: {}",
                entry.path().display()
            )));
        }
        if entry.file_type().is_dir() {
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(|_| Error::Manifest("publication escape".into()))?;
            let text = relative.to_string_lossy();
            let allowed = matches!(text.as_ref(), "audio" | "manifests" | "reports")
                || (relative.components().count() == 2 && text.starts_with("audio/"));
            if !allowed {
                return Err(Error::Manifest(format!(
                    "unexpected publication directory: {}",
                    relative.display()
                )));
            }
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(root)
            .map_err(|_| Error::Manifest("publication escape".into()))?;
        if rel == Path::new("publication-manifest.json")
            || rel == Path::new(".publication-manifest.json.tmp")
        {
            continue;
        }
        if !allowed_file(rel) {
            return Err(Error::Manifest(format!(
                "unexpected publication file: {}",
                rel.display()
            )));
        }
        files.push((
            rel.to_string_lossy().into_owned(),
            entry.metadata()?.len(),
            sha256_file(entry.path())?,
        ));
    }
    files.sort();
    for required in [
        "metadata.csv",
        "speakers.json",
        "report.json",
        "README.md",
        "dataset-complete.json",
        "manifests/accepted.jsonl",
        "reports/rejected.jsonl",
        "reports/rejected.csv",
        "reports/orphan-audio.jsonl",
        "reports/missing-audio.jsonl",
        "reports/manual-review.jsonl",
        "reports/join-summary.json",
    ] {
        if !files.iter().any(|(path, _, _)| path == required) {
            return Err(Error::Manifest(format!(
                "required publication file missing: {required}"
            )));
        }
    }
    Ok(files)
}

fn write_seal_once(path: &Path, bytes: &[u8]) -> Result<()> {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| Error::Manifest("seal path has no filename".into()))?;
    let temporary = path.with_file_name(format!(".{name}.tmp"));
    if is_regular(path)? {
        if fs::read(path).map_err(|source| io(path, source))? != bytes {
            return Err(Error::Manifest(
                "existing seal differs from expected bytes".into(),
            ));
        }
        if is_regular(&temporary)? {
            if fs::read(&temporary).map_err(|source| io(&temporary, source))? != bytes {
                return Err(Error::Manifest("stale seal temporary file differs".into()));
            }
            fs::remove_file(&temporary).map_err(|source| io(&temporary, source))?;
        }
        return Ok(());
    }
    if is_regular(&temporary)? {
        if fs::read(&temporary).map_err(|source| io(&temporary, source))? != bytes {
            return Err(Error::Manifest("stale seal temporary file differs".into()));
        }
    } else {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|source| io(&temporary, source))?;
        let result = file
            .write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|source| io(&temporary, source));
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
    }
    File::open(&temporary)
        .and_then(|file| file.sync_all())
        .map_err(|source| io(&temporary, source))?;
    fs::rename(&temporary, path).map_err(|source| io(path, source))?;
    File::open(path.parent().expect("seal has parent"))
        .and_then(|file| file.sync_all())
        .map_err(|source| io(path, source))?;
    Ok(())
}

pub fn finalize_dataset(config: &BuildConfig) -> Result<()> {
    finalize_dataset_inner(config, ScenarioStructureMode::Pinned)
}

/// Synthetic source finalization for integration tests. No CLI path selects this mode.
#[doc(hidden)]
pub fn finalize_fixture_dataset_for_tests(config: &BuildConfig) -> Result<()> {
    finalize_dataset_inner(config, ScenarioStructureMode::TestFixture)
}

fn finalize_dataset_inner(config: &BuildConfig, mode: ScenarioStructureMode) -> Result<()> {
    let work = work_path(&config.final_root)?;
    if work
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        return Err(Error::Manifest("work root is a symlink".into()));
    }
    let paths = DatasetPaths::new(&config.source_root, &work)?;
    if config.final_root.symlink_metadata().is_ok() {
        return Err(Error::Manifest("final root already exists".into()));
    }
    if !work.is_dir() {
        return Err(Error::Manifest("missing work root".into()));
    }
    let publication_path = work.join("publication-manifest.json");
    let sealed = is_regular(&publication_path)?;
    let verification = if sealed {
        verify_dataset_with_mode(
            &work,
            &paths.source_root,
            &VerificationConfig::default(),
            &config.ffprobe,
            mode,
        )?
    } else {
        verify_interrupted_work_with_mode(&work, &paths.source_root, &config.ffprobe, mode)?
    };
    check_manual_review(&work, &verification.accepted)?;
    let card = work.join("README.md");
    let expected_card =
        render_dataset_card(&verification, include_str!("../templates/README.md.tmpl"))?;
    if !is_regular(&card)?
        || fs::read(&card).map_err(|source| io(&card, source))? != expected_card.as_bytes()
    {
        return Err(Error::Manifest(
            "dataset Card differs from freshly verified values".into(),
        ));
    }
    if !sealed {
        finder::quarantine(&paths.output_root, &verification.fingerprint)?;
        verify_strict_interrupted_work_with_mode(&work, &paths.source_root, &config.ffprobe, mode)?;
        for row in &verification.accepted {
            let state = work.join(&row.audio_path).with_extension("wav.state.json");
            if is_regular(&state)? {
                fs::remove_file(&state).map_err(|source| io(&state, source))?;
            }
        }
        let work_marker = work.join(".source-config-fingerprint.json");
        if is_regular(&work_marker)? {
            fs::remove_file(&work_marker).map_err(|source| io(&work_marker, source))?;
        }
        let completion = Completion {
            schema_version: "kanade-complete-v1",
            source_config_fingerprint: &verification.fingerprint,
            accepted_count: verification.accepted.len(),
        };
        let completion_path = work.join("dataset-complete.json");
        write_seal_once(
            &completion_path,
            &serde_json::to_vec_pretty(&completion).map_err(|e| Error::Manifest(e.to_string()))?,
        )?;
        let files = publication_files(&work)?;
        let manifest: Vec<_> = files
            .into_iter()
            .map(|(path, size, sha256)| PublicationFile { path, size, sha256 })
            .collect();
        write_seal_once(
            &publication_path,
            &serde_json::to_vec_pretty(&manifest).map_err(|e| Error::Manifest(e.to_string()))?,
        )?;
        verify_dataset_with_mode(
            &work,
            &paths.source_root,
            &VerificationConfig::default(),
            &config.ffprobe,
            mode,
        )?;
    }
    for entry in WalkDir::new(&work).follow_links(false).contents_first(true) {
        let entry = entry?;
        File::open(entry.path())
            .and_then(|file| file.sync_all())
            .map_err(|source| io(entry.path(), source))?;
    }
    finder::ensure_absent(&work)?;
    promote_noreplace(&work, &config.final_root)?;
    File::open(config.final_root.parent().expect("final root has parent"))
        .and_then(|file| file.sync_all())
        .map_err(|source| io(&config.final_root, source))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        promote_noreplace, publication_files, read_source, review_sample_ids, source_config_hasher,
    };
    use crate::join::JoinReport;
    use crate::manifest::write_join_diagnostics;
    use sha2::Digest;
    use std::collections::BTreeMap;
    use std::fs;

    #[test]
    fn line_structure_config_drift_changes_source_fingerprint() {
        let root = std::path::Path::new("/source");
        let original = include_bytes!("../config/line-structure-fingerprints.json");
        let mut changed = original.to_vec();
        changed.push(b' ');
        let original_fingerprint = source_config_hasher(root, original).finalize();
        let changed_fingerprint = source_config_hasher(root, &changed).finalize();
        assert_ne!(original_fingerprint, changed_fingerprint);

        let base = std::env::temp_dir().join(format!("kanade-config-drift-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let source = base.join("source");
        let work = base.join("KANADE-v1.work");
        fs::create_dir_all(&source).unwrap();
        let mut report = JoinReport {
            matched: vec![],
            rejected: vec![],
            orphan_audio: vec![],
            missing_audio: vec![],
            physical_audio_count: 0,
            source_root: source,
            fingerprint: format!("{original_fingerprint:x}"),
        };
        write_join_diagnostics(&report, &work).unwrap();
        report.fingerprint = format!("{changed_fingerprint:x}");
        let error = write_join_diagnostics(&report, &work).unwrap_err();
        assert!(error.to_string().contains("fingerprint drift"), "{error}");
        fs::remove_dir_all(base).unwrap();
    }

    fn populate_required(root: &std::path::Path) {
        for name in [
            "metadata.csv",
            "speakers.json",
            "report.json",
            "README.md",
            "dataset-complete.json",
            "manifests/accepted.jsonl",
            "reports/rejected.jsonl",
            "reports/rejected.csv",
            "reports/orphan-audio.jsonl",
            "reports/missing-audio.jsonl",
            "reports/manual-review.jsonl",
            "reports/join-summary.json",
        ] {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"fixture").unwrap();
        }
    }

    #[test]
    fn publication_inventory_rejects_unexpected_empty_directories() {
        let root = std::env::temp_dir().join(format!("kanade-publication-{}", std::process::id()));
        if root.exists() {
            fs::remove_dir_all(&root).unwrap();
        }
        populate_required(&root);
        fs::create_dir_all(root.join("unexpected/empty")).unwrap();
        let result = publication_files(&root);
        fs::remove_dir_all(&root).unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn publication_inventory_rejects_missing_required_metadata() {
        let root =
            std::env::temp_dir().join(format!("kanade-publication-missing-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let result = publication_files(&root);
        fs::remove_dir_all(&root).unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn review_sample_covers_all_small_speakers_and_five_spread_large_speakers() {
        let groups = BTreeMap::from([
            (
                "small".to_owned(),
                (0..4).map(|i| format!("small-{i}")).collect(),
            ),
            (
                "large".to_owned(),
                (0..9).map(|i| format!("large-{i}")).collect(),
            ),
        ]);
        let chosen = review_sample_ids(&groups);
        assert_eq!(chosen, review_sample_ids(&groups));
        for i in 0..4 {
            assert!(chosen.contains(&format!("small-{i}")));
        }
        assert_eq!(
            chosen.iter().filter(|id| id.starts_with("large-")).count(),
            5
        );
        for id in ["large-0", "large-4", "large-8"] {
            assert!(chosen.contains(id));
        }
    }

    #[test]
    #[ignore = "requires KANADE_SOURCE_ROOT pointing to lawfully extracted romfs/data"]
    fn source_parser_handles_every_real_scenario() {
        let source = std::env::var_os("KANADE_SOURCE_ROOT")
            .map(std::path::PathBuf::from)
            .expect("KANADE_SOURCE_ROOT must point to lawfully extracted romfs/data");
        read_source(&source).unwrap();
    }

    #[test]
    fn atomic_promotion_never_replaces_existing_empty_or_nonempty_final_directory() {
        let base = std::env::temp_dir().join(format!("kanade-no-replace-{}", std::process::id()));
        fs::create_dir(&base).unwrap();
        for occupied in [false, true] {
            let source = base.join(if occupied { "work-full" } else { "work-empty" });
            let destination = base.join(if occupied {
                "final-full"
            } else {
                "final-empty"
            });
            fs::create_dir(&source).unwrap();
            fs::write(source.join("payload"), b"work").unwrap();
            fs::create_dir(&destination).unwrap();
            if occupied {
                fs::write(destination.join("foreign"), b"foreign").unwrap();
            }
            assert!(promote_noreplace(&source, &destination).is_err());
            assert_eq!(fs::read(source.join("payload")).unwrap(), b"work");
            assert!(destination.is_dir());
            if occupied {
                assert_eq!(fs::read(destination.join("foreign")).unwrap(), b"foreign");
            }
        }
        fs::remove_dir_all(base).unwrap();
    }
}
