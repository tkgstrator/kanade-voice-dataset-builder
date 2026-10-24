use crate::build::promote_noreplace;
use crate::durable::sync_directory;
use crate::error::{Error, Result};
use crate::paths::portable_relative;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

// A conservative envelope for standard Finder Bud1 stores. A larger or unusual
// store is left untouched for explicit inspection instead of being published.
const MIN_BYTES: u64 = 6_148;
const MAX_BYTES: u64 = 1_048_576;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FinderEvidence {
    pub size: u64,
    pub sha256: String,
}

fn io(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        path: path.to_owned(),
        source,
    }
}

pub(crate) fn inspect(path: &Path) -> Result<FinderEvidence> {
    if path.file_name().is_none_or(|name| name != ".DS_Store") {
        return Err(Error::Manifest("not a Finder sidecar basename".into()));
    }
    let metadata = path.symlink_metadata().map_err(|source| io(path, source))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(Error::Manifest(format!(
            "unsafe Finder sidecar: {}",
            path.display()
        )));
    }
    let size = metadata.len();
    if !(MIN_BYTES..=MAX_BYTES).contains(&size) || (size - 4) % 2_048 != 0 {
        return Err(Error::Manifest(format!(
            "unsupported Finder sidecar size: {}",
            path.display()
        )));
    }
    let bytes = fs::read(path).map_err(|source| io(path, source))?;
    if bytes.len() as u64 != size {
        return Err(Error::Manifest(
            "Finder sidecar changed while reading".into(),
        ));
    }
    let store = ds_parser::parse(&bytes)
        .map_err(|error| Error::Manifest(format!("invalid Finder sidecar structure: {error}")))?;
    if !store.warnings.is_empty() {
        return Err(Error::Manifest(
            "noncanonical Finder sidecar structure".into(),
        ));
    }
    Ok(FinderEvidence {
        size,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
    })
}

pub(crate) fn recognized_at(root: &Path, path: &Path, marked_unsealed: bool) -> Result<bool> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| Error::Manifest("Finder sidecar outside work root".into()))?;
    if !matches!(
        portable_relative(relative)?.as_str(),
        ".DS_Store" | "audio/.DS_Store"
    ) {
        return Ok(false);
    }
    if !marked_unsealed {
        return Err(Error::Manifest(
            "Finder sidecar in sealed or unmarked tree".into(),
        ));
    }
    inspect(path)?;
    Ok(true)
}

pub(crate) fn locations(root: &Path) -> [PathBuf; 2] {
    [root.join(".DS_Store"), root.join("audio/.DS_Store")]
}

pub(crate) fn ensure_absent(root: &Path) -> Result<()> {
    for path in locations(root) {
        match path.symlink_metadata() {
            Ok(_) => {
                return Err(Error::Manifest(format!(
                    "Finder sidecar reappeared before promotion: {}",
                    path.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(io(&path, source)),
        }
    }
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct QuarantinePlan {
    schema_version: String,
    source_config_fingerprint: String,
    files: BTreeMap<String, FinderEvidence>,
}

fn existing_file(path: &Path) -> Result<bool> {
    match path.symlink_metadata() {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(Error::Manifest(format!(
            "unsafe quarantine file: {}",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(io(path, source)),
    }
}

fn ensure_directory(path: &Path) -> Result<()> {
    match path.symlink_metadata() {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(Error::Manifest(format!(
            "unsafe quarantine directory: {}",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|source| io(path, source))
        }
        Err(source) => Err(io(path, source)),
    }
}

fn write_plan_once(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_file_name(".KANADE-v1.finder-quarantine.json.tmp");
    if existing_file(&temporary)? {
        if fs::read(&temporary).map_err(|source| io(&temporary, source))? != bytes {
            return Err(Error::Manifest(
                "stale Finder quarantine plan differs".into(),
            ));
        }
    } else {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|source| io(&temporary, source))?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|source| io(&temporary, source))?;
    }
    promote_noreplace(&temporary, path)?;
    sync_directory(path.parent().expect("quarantine plan has parent"))
        .map_err(|source| io(path, source))
}

/// Move only reviewed Finder sidecars from an owned work tree to its deterministic
/// sibling. The plan is written first, so a partially moved set can resume.
pub(crate) fn quarantine(work: &Path, fingerprint: &str) -> Result<()> {
    let parent = work
        .parent()
        .ok_or_else(|| Error::Manifest("work root has no parent".into()))?;
    let directory = parent.join("KANADE-v1.finder-quarantine");
    let plan_path = parent.join("KANADE-v1.finder-quarantine.json");
    let plan_temporary = parent.join(".KANADE-v1.finder-quarantine.json.tmp");
    let mut source_evidence = BTreeMap::new();
    for source in locations(work) {
        if existing_file(&source)? {
            let relative = source
                .strip_prefix(work)
                .map_err(|_| Error::Manifest("sidecar escapes work".into()))?;
            let relative = portable_relative(relative)?;
            source_evidence.insert(relative, inspect(&source)?);
        }
    }
    let plan = if existing_file(&plan_path)? {
        if existing_file(&plan_temporary)? {
            return Err(Error::Manifest(
                "ambiguous Finder quarantine plan and temporary plan".into(),
            ));
        }
        let bytes = fs::read(&plan_path).map_err(|source| io(&plan_path, source))?;
        serde_json::from_slice::<QuarantinePlan>(&bytes)
            .map_err(|error| Error::Manifest(format!("invalid Finder quarantine plan: {error}")))?
    } else {
        match directory.symlink_metadata() {
            Ok(_) => {
                return Err(Error::Manifest(
                    "occupied Finder quarantine without a plan".into(),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(io(&directory, source)),
        }
        if source_evidence.is_empty() {
            return Ok(());
        }
        let plan = QuarantinePlan {
            schema_version: "kanade-finder-quarantine-v1".into(),
            source_config_fingerprint: fingerprint.into(),
            files: source_evidence.clone(),
        };
        let bytes =
            serde_json::to_vec_pretty(&plan).map_err(|error| Error::Manifest(error.to_string()))?;
        write_plan_once(&plan_path, &bytes)?;
        plan
    };
    if plan.schema_version != "kanade-finder-quarantine-v1"
        || plan.source_config_fingerprint != fingerprint
        || plan.files.is_empty()
        || plan
            .files
            .keys()
            .any(|name| !matches!(name.as_str(), ".DS_Store" | "audio/.DS_Store"))
    {
        return Err(Error::Manifest(
            "Finder quarantine plan does not match work".into(),
        ));
    }
    if source_evidence
        .keys()
        .any(|name| !plan.files.contains_key(name))
    {
        return Err(Error::Manifest(
            "new Finder sidecar after quarantine plan".into(),
        ));
    }
    ensure_directory(&directory)?;
    if plan.files.contains_key("audio/.DS_Store") {
        ensure_directory(&directory.join("audio"))?;
    }
    for entry in WalkDir::new(&directory).follow_links(false) {
        let entry = entry?;
        if entry.path() == directory {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(&directory)
            .map_err(|_| Error::Manifest("quarantine path escape".into()))?;
        if entry.file_type().is_dir() && relative == Path::new("audio") {
            continue;
        }
        if !entry.file_type().is_file()
            || !plan
                .files
                .contains_key(portable_relative(relative)?.as_str())
        {
            return Err(Error::Manifest("unexpected Finder quarantine entry".into()));
        }
    }
    for (name, expected) in &plan.files {
        let source = work.join(name);
        let destination = directory.join(name);
        let source_exists = existing_file(&source)?;
        let destination_exists = existing_file(&destination)?;
        if source_exists && destination_exists {
            return Err(Error::Manifest(format!(
                "Finder sidecar recreated or quarantine occupied: {name}"
            )));
        }
        if source_exists {
            if &inspect(&source)? != expected {
                return Err(Error::Manifest(format!(
                    "Finder sidecar changed before quarantine: {name}"
                )));
            }
            promote_noreplace(&source, &destination)?;
            if &inspect(&destination)? != expected {
                return Err(Error::Manifest(format!(
                    "Finder sidecar changed during quarantine: {name}"
                )));
            }
        } else if destination_exists {
            if &inspect(&destination)? != expected {
                return Err(Error::Manifest(format!(
                    "Finder quarantine bytes changed: {name}"
                )));
            }
        } else {
            return Err(Error::Manifest(format!(
                "Finder sidecar missing from work and quarantine: {name}"
            )));
        }
    }
    let audio_directory = directory.join("audio");
    if audio_directory.exists() {
        sync_directory(&audio_directory).map_err(|source| io(&audio_directory, source))?;
    }
    sync_directory(&directory).map_err(|source| io(&directory, source))?;
    sync_directory(parent).map_err(|source| io(parent, source))?;
    Ok(())
}
