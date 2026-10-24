use crate::error::{Error, Result};
use crate::model::VoiceAsset;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;
use walkdir::WalkDir;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceInventory {
    /// Paths are relative to the voice root.
    pub assets: Vec<VoiceAsset>,
    pub sidecars: Vec<PathBuf>,
    pub speakers: Vec<String>,
}

pub fn normalize_voice_id(value: &str) -> String {
    value
        .nfc()
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

pub fn inventory_voice_root(root: &Path) -> Result<VoiceInventory> {
    if !root.is_dir() {
        return Err(Error::SourceNotDirectory(root.to_owned()));
    }

    let mut names = BTreeMap::<String, String>::new();
    for entry in std::fs::read_dir(root).map_err(|source| Error::Io {
        path: root.to_owned(),
        source,
    })? {
        let entry = entry.map_err(|source| Error::Io {
            path: root.to_owned(),
            source,
        })?;
        if !entry
            .file_type()
            .map_err(|source| Error::Io {
                path: entry.path(),
                source,
            })?
            .is_dir()
        {
            continue;
        }
        let original = entry
            .file_name()
            .into_string()
            .map_err(|_| Error::InvalidUnicodePath(entry.path()))?;
        let normalized = normalize_voice_id(&original);
        if let Some(first) = names.insert(normalized, original.nfc().collect()) {
            return Err(Error::SpeakerNameCollision {
                first,
                second: original,
            });
        }
    }

    let mut assets = Vec::new();
    let mut sidecars = Vec::new();
    let mut ids = BTreeMap::<String, PathBuf>::new();
    let speakers = names
        .into_values()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    for entry in WalkDir::new(root).follow_links(false).sort_by_file_name() {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .expect("walk entry is below root")
            .to_owned();
        let filename = entry
            .file_name()
            .to_str()
            .ok_or_else(|| Error::InvalidUnicodePath(relative.clone()))?;
        if filename.ends_with(".ogg.sli") {
            sidecars.push(relative);
            continue;
        }
        let Some(stem) = filename.strip_suffix(".ogg") else {
            continue;
        };
        let speaker = relative
            .components()
            .next()
            .and_then(|component| component.as_os_str().to_str())
            .ok_or_else(|| Error::InvalidUnicodePath(relative.clone()))?
            .nfc()
            .collect();
        if relative.components().count() < 2 {
            continue;
        }
        let voice_id = normalize_voice_id(stem);
        if let Some(first) = ids.insert(voice_id.clone(), relative.clone()) {
            return Err(Error::VoiceIdCollision {
                voice_id,
                first,
                second: relative,
            });
        }
        let mut file = File::open(entry.path()).map_err(|source| Error::Io {
            path: relative.clone(),
            source,
        })?;
        let mut hash = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer).map_err(|source| Error::Io {
                path: relative.clone(),
                source,
            })?;
            if read == 0 {
                break;
            }
            hash.update(&buffer[..read]);
        }
        let byte_size = entry.metadata()?.len();
        assets.push(VoiceAsset {
            source_ogg: relative,
            voice_id,
            source_speaker: speaker,
            byte_size,
            source_sha256: format!("{:x}", hash.finalize()),
        });
    }
    assets.sort_by(|a, b| a.source_ogg.cmp(&b.source_ogg));
    sidecars.sort();
    Ok(VoiceInventory {
        assets,
        sidecars,
        speakers,
    })
}
