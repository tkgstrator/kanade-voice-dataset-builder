use crate::durable::{sync_directory, sync_existing_file};
use crate::error::{Error, Result};
use crate::model::{AcceptedUtterance, MatchedUtterance};
use crate::paths::DatasetPaths;
use crate::probe::{probe_audio, probe_canonical_wav};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

pub const CONVERSION_SETTINGS: &str = "mono-44100-pcm_s16le-v1";

#[derive(Clone, Debug)]
pub struct AudioConfig {
    pub source_root: PathBuf,
    pub work_root: PathBuf,
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct ResumeState {
    source_sha256: String,
    output_sha256: String,
    settings: String,
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(|source| Error::Io {
        path: path.into(),
        source,
    })?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let count = file.read(&mut buffer).map_err(|source| Error::Io {
            path: path.into(),
            source,
        })?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn normal_relative(path: &Path) -> bool {
    path.components().next().is_some()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn regular_or_absent(path: &Path) -> Result<bool> {
    match path.symlink_metadata() {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(Error::Manifest(format!(
            "unsafe file entry: {}",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(Error::Io {
            path: path.into(),
            source,
        }),
    }
}

fn ensure_directory(path: &Path) -> Result<()> {
    match path.symlink_metadata() {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(Error::Manifest(format!(
            "unsafe audio directory: {}",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|source| Error::Io {
                path: path.into(),
                source,
            })
        }
        Err(source) => Err(Error::Io {
            path: path.into(),
            source,
        }),
    }
}

fn verified_wav(ffprobe: &Path, path: &Path) -> Result<f64> {
    let probe = probe_canonical_wav(ffprobe, path)?;
    Ok(probe.duration_seconds)
}

fn state_path(wav: &Path) -> PathBuf {
    wav.with_extension("wav.state.json")
}

fn sync_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| Error::Io {
            path: path.into(),
            source,
        })?;
    let result = file
        .write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|source| Error::Io {
            path: path.into(),
            source,
        });
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

/// Converts one exact matched OGG. A row is returned only after independent output verification.
pub fn convert_one(config: &AudioConfig, item: &MatchedUtterance) -> Result<AcceptedUtterance> {
    if config
        .work_root
        .file_name()
        .and_then(|s| s.to_str())
        .is_none_or(|s| !s.ends_with(".work"))
    {
        return Err(Error::Manifest("conversion requires a .work root".into()));
    }
    let paths = DatasetPaths::new(&config.source_root, &config.work_root)?;
    if config
        .work_root
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        return Err(Error::Manifest("work root is a symlink".into()));
    }
    if !normal_relative(&item.audio.source_ogg)
        || item.audio.source_ogg.components().count() != 2
        || !safe_component(&item.speaker_id)
        || !safe_component(&item.audio.voice_id)
        || item
            .audio
            .source_ogg
            .extension()
            .is_none_or(|ext| ext != "ogg")
    {
        return Err(Error::Manifest("unsafe source or destination path".into()));
    }
    let source = paths.source_root.join("voice").join(&item.audio.source_ogg);
    if !regular_or_absent(&source)? {
        return Err(Error::Manifest("missing source OGG".into()));
    }
    let source_canonical = source.canonicalize().map_err(|source_error| Error::Io {
        path: source.clone(),
        source: source_error,
    })?;
    if !source_canonical.starts_with(paths.source_root.join("voice")) {
        return Err(Error::Manifest("source OGG escapes source root".into()));
    }
    let source_hash = sha256_file(&source)?;
    if source_hash != item.audio.source_sha256
        || fs::metadata(&source)
            .map_err(|source_error| Error::Io {
                path: source.clone(),
                source: source_error,
            })?
            .len()
            != item.audio.byte_size
    {
        return Err(Error::Manifest("source OGG identity changed".into()));
    }
    let source_probe = probe_audio(&config.ffprobe, &source)?;
    let relative = PathBuf::from("audio")
        .join(&item.speaker_id)
        .join(format!("{}.wav", item.audio.voice_id));
    let output = paths.output_root.join(&relative);
    let parent = output.parent().expect("WAV has parent");
    ensure_directory(&paths.output_root)?;
    ensure_directory(&paths.output_root.join("audio"))?;
    ensure_directory(parent)?;
    let temp_prefix = format!("{}.wav.", item.audio.voice_id);
    for entry in fs::read_dir(parent).map_err(|source| Error::Io {
        path: parent.into(),
        source,
    })? {
        let entry = entry.map_err(|source| Error::Io {
            path: parent.into(),
            source,
        })?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&temp_prefix) && name.ends_with(".tmp.wav") {
            return Err(Error::Manifest(format!(
                "leftover WAV temporary file: {}",
                entry.path().display()
            )));
        }
    }
    if parent.canonicalize().map_err(|source| Error::Io {
        path: parent.into(),
        source,
    })? != parent
    {
        return Err(Error::Manifest("audio directory is redirected".into()));
    }
    let state_file = state_path(&output);
    let mut reused = None;
    if regular_or_absent(&output)?
        && regular_or_absent(&state_file)?
        && let Ok(bytes) = fs::read(&state_file)
        && let Ok(state) = serde_json::from_slice::<ResumeState>(&bytes)
        && state.source_sha256 == source_hash
        && state.settings == CONVERSION_SETTINGS
        && let (Ok(duration), Ok(hash)) =
            (verified_wav(&config.ffprobe, &output), sha256_file(&output))
        && hash == state.output_sha256
    {
        reused = Some((duration, hash));
    }
    let (duration, output_hash) = if let Some(value) = reused {
        value
    } else {
        let temporary = output.with_extension(format!("wav.{}.tmp.wav", std::process::id()));
        let temp_state = state_file.with_extension("json.tmp");
        let mut created_state = false;
        if regular_or_absent(&temporary)? {
            return Err(Error::Manifest("leftover WAV temporary file".into()));
        }
        let result = (|| {
            let run = Command::new(&config.ffmpeg)
                .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i"])
                .arg(&source)
                .args([
                    "-map",
                    "0:a:0",
                    "-ac",
                    "1",
                    "-ar",
                    "44100",
                    "-c:a",
                    "pcm_s16le",
                    "-y",
                ])
                .arg(&temporary)
                .output()
                .map_err(|error| Error::Conversion {
                    input: source.clone(),
                    detail: error.to_string(),
                })?;
            if !run.status.success() {
                return Err(Error::Conversion {
                    input: source.clone(),
                    detail: String::from_utf8_lossy(&run.stderr).trim().into(),
                });
            }
            let duration = verified_wav(&config.ffprobe, &temporary)?;
            let hash = sha256_file(&temporary)?;
            sync_existing_file(&temporary).map_err(|source| Error::Io {
                path: temporary.clone(),
                source,
            })?;
            regular_or_absent(&output)?;
            fs::rename(&temporary, &output).map_err(|source| Error::Io {
                path: output.clone(),
                source,
            })?;
            let state = ResumeState {
                source_sha256: source_hash.clone(),
                output_sha256: hash.clone(),
                settings: CONVERSION_SETTINGS.into(),
            };
            if regular_or_absent(&temp_state)? {
                return Err(Error::Manifest("leftover state temporary file".into()));
            }
            sync_file(
                &temp_state,
                &serde_json::to_vec(&state).map_err(|error| Error::Manifest(error.to_string()))?,
            )?;
            created_state = true;
            regular_or_absent(&state_file)?;
            fs::rename(&temp_state, &state_file).map_err(|source| Error::Io {
                path: state_file.clone(),
                source,
            })?;
            sync_directory(parent).map_err(|source| Error::Io {
                path: parent.into(),
                source,
            })?;
            Ok((duration, hash))
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
            if created_state {
                let _ = fs::remove_file(&temp_state);
            }
        }
        result?
    };
    if sha256_file(&source)? != source_hash {
        return Err(Error::Manifest(
            "source OGG changed during conversion".into(),
        ));
    }
    let primary = item
        .source_events
        .first()
        .ok_or_else(|| Error::Manifest("matched utterance has no source event".into()))?
        .clone();
    Ok(AcceptedUtterance {
        schema_version: "kanade-accepted-v1".into(),
        utterance_id: format!("{}-{}", item.speaker_id, item.audio.voice_id),
        voice_id: item.audio.voice_id.clone(),
        speaker_id: item.speaker_id.clone(),
        speaker_name: item.audio.source_speaker.clone(),
        source_ogg: item.audio.source_ogg.clone(),
        source_scn: primary.source_scn.clone(),
        scenario_event_index: primary.scenario_event_index,
        primary_source_event: primary,
        source_events: item.source_events.clone(),
        source_observations: item.source_observations.clone(),
        audio_path: relative,
        raw_text: item
            .candidate
            .raw_text
            .clone()
            .ok_or_else(|| Error::Manifest("missing raw text".into()))?,
        display_text: item.display_text.clone(),
        spoken_text: item.spoken_text.clone(),
        language: "ja".into(),
        duration_seconds: duration,
        source_codec: source_probe.codec,
        source_sample_rate: source_probe.sample_rate,
        source_channels: source_probe.channels,
        output_sample_rate: 44100,
        output_channels: 1,
        output_sample_format: "s16".into(),
        source_sha256: source_hash,
        output_sha256: output_hash,
        normalization_version: item.normalization_version.clone(),
        quality_flags: item.quality_flags.clone(),
    })
}
