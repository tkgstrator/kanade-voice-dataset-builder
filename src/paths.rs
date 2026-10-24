use crate::error::{Error, Result};
use serde::{Serialize, Serializer};
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatasetPaths {
    pub source_root: PathBuf,
    pub output_root: PathBuf,
}

impl DatasetPaths {
    pub fn new(source_root: &Path, output_root: &Path) -> Result<Self> {
        let source_root = source_root
            .canonicalize()
            .map_err(|source| Error::PathResolution {
                path: source_root.to_owned(),
                source,
            })?;
        if !source_root.is_dir() {
            return Err(Error::SourceNotDirectory(source_root));
        }

        let output_root =
            resolve_existing_components(output_root).map_err(|source| Error::PathResolution {
                path: output_root.to_owned(),
                source,
            })?;
        if output_root.starts_with(&source_root) {
            return Err(Error::OutputInsideSource {
                source_root,
                output: output_root,
            });
        }

        Ok(Self {
            source_root,
            output_root,
        })
    }
}

fn resolve_existing_components(path: &Path) -> std::io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => resolved.push(prefix.as_os_str()),
            Component::RootDir => resolved.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(name) => {
                resolved.push(name);
                match resolved.symlink_metadata() {
                    Ok(_) => resolved = resolved.canonicalize()?,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        }
    }
    Ok(resolved)
}

/// Stable spelling for relative paths stored in fingerprints and dataset files.
pub(crate) fn portable_relative(path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        let Component::Normal(name) = component else {
            return Err(Error::Manifest(format!(
                "non-relative dataset path: {}",
                path.display()
            )));
        };
        let name = name
            .to_str()
            .ok_or_else(|| Error::InvalidUnicodePath(path.to_owned()))?;
        if name.contains(['/', '\\']) {
            return Err(Error::Manifest(format!(
                "ambiguous dataset path: {}",
                path.display()
            )));
        }
        parts.push(name);
    }
    if parts.is_empty() {
        return Err(Error::Manifest("empty dataset path".into()));
    }
    Ok(parts.join("/"))
}

pub(crate) fn serialize_relative_path<S>(
    path: &Path,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: Serializer,
{
    portable_relative(path)
        .map_err(serde::ser::Error::custom)?
        .serialize(serializer)
}

pub(crate) fn serialize_optional_relative_path<S>(
    path: &Option<PathBuf>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: Serializer,
{
    path.as_ref()
        .map(|path| portable_relative(path))
        .transpose()
        .map_err(serde::ser::Error::custom)?
        .serialize(serializer)
}

#[cfg(test)]
mod portable_tests {
    use super::portable_relative;
    use crate::model::VoiceAsset;
    use std::path::{Path, PathBuf};

    #[test]
    fn serializes_relative_components_with_forward_slashes() {
        let path = PathBuf::from("audio").join("speaker").join("voice.wav");
        assert_eq!(portable_relative(&path).unwrap(), "audio/speaker/voice.wav");
    }

    #[test]
    fn rejects_escaping_and_ambiguous_relative_paths() {
        assert!(portable_relative(Path::new("../outside.wav")).is_err());
        assert!(portable_relative(Path::new("")).is_err());
        #[cfg(not(windows))]
        assert!(portable_relative(Path::new("audio\\speaker.wav")).is_err());
    }

    #[test]
    #[cfg(not(windows))]
    fn persisted_voice_paths_reject_ambiguous_separators() {
        let asset = VoiceAsset {
            source_ogg: PathBuf::from("speaker\\voice.ogg"),
            voice_id: "voice".into(),
            source_speaker: "speaker".into(),
            byte_size: 1,
            source_sha256: "hash".into(),
        };
        assert!(serde_json::to_string(&asset).is_err());
    }

    #[test]
    #[cfg(windows)]
    fn persisted_voice_paths_normalize_windows_separators() {
        let asset = VoiceAsset {
            source_ogg: PathBuf::from("speaker\\voice.ogg"),
            voice_id: "voice".into(),
            source_speaker: "speaker".into(),
            byte_size: 1,
            source_sha256: "hash".into(),
        };
        let serialized = serde_json::to_string(&asset).unwrap();
        assert!(serialized.contains("speaker/voice.ogg"));
    }
}
