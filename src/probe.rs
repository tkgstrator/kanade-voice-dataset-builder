use crate::error::{Error, Result};
use serde::Deserialize;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::Command;

#[derive(Clone, Debug, PartialEq)]
pub struct AudioProbe {
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub duration_seconds: f64,
    pub format_name: Option<String>,
}

#[derive(Deserialize)]
struct ProbeJson {
    streams: Vec<StreamJson>,
    format: FormatJson,
}

#[derive(Deserialize)]
struct StreamJson {
    codec_type: Option<String>,
    codec_name: Option<String>,
    sample_rate: Option<String>,
    channels: Option<u16>,
}

#[derive(Deserialize)]
struct FormatJson {
    duration: Option<String>,
    format_name: Option<String>,
}

pub fn probe_audio(ffprobe: &Path, input: &Path) -> Result<AudioProbe> {
    let output = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_type,codec_name,sample_rate,channels:format=duration,format_name",
            "-of",
            "json",
            "--",
        ])
        .arg(input)
        .output()
        .map_err(|error| Error::AudioProbe {
            input: input.to_owned(),
            detail: error.to_string(),
        })?;
    if !output.status.success() {
        return Err(Error::AudioProbe {
            input: input.to_owned(),
            detail: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    let parsed: ProbeJson =
        serde_json::from_slice(&output.stdout).map_err(|error| Error::AudioProbe {
            input: input.to_owned(),
            detail: error.to_string(),
        })?;
    let mut audio = parsed
        .streams
        .into_iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("audio"));
    let stream = audio.next().ok_or_else(|| Error::AudioProbe {
        input: input.to_owned(),
        detail: "expected exactly one audio stream; found none".into(),
    })?;
    if audio.next().is_some() {
        return Err(Error::AudioProbe {
            input: input.to_owned(),
            detail: "expected exactly one audio stream; found multiple".into(),
        });
    }
    let invalid = |detail: &str| Error::AudioProbe {
        input: input.to_owned(),
        detail: detail.into(),
    };
    let codec = stream
        .codec_name
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid("missing codec name"))?;
    let sample_rate = stream
        .sample_rate
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid("invalid sample rate"))?;
    let channels = stream
        .channels
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid("invalid channel count"))?;
    let duration_seconds = parsed
        .format
        .duration
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or_else(|| invalid("invalid duration"))?;
    Ok(AudioProbe {
        codec,
        sample_rate,
        channels,
        duration_seconds,
        format_name: parsed.format.format_name,
    })
}

pub fn probe_canonical_wav(ffprobe: &Path, input: &Path) -> Result<AudioProbe> {
    let probe = probe_audio(ffprobe, input)?;
    let mut header = [0_u8; 12];
    File::open(input)
        .and_then(|mut file| file.read_exact(&mut header))
        .map_err(|error| Error::AudioProbe {
            input: input.into(),
            detail: error.to_string(),
        })?;
    if probe.codec != "pcm_s16le"
        || probe.sample_rate != 44100
        || probe.channels != 1
        || probe.format_name.as_deref() != Some("wav")
        || &header[..4] != b"RIFF"
        || &header[8..] != b"WAVE"
    {
        return Err(Error::AudioProbe {
            input: input.into(),
            detail: "not a canonical PCM WAV container".into(),
        });
    }
    Ok(probe)
}
