use crate::error::{Error, Result};
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
