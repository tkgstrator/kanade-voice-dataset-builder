use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceEventRef {
    pub source_scn: String,
    pub scenario_event_index: usize,
    #[serde(default)]
    pub scene_index: usize,
    #[serde(default)]
    pub scene_text_index: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VoiceMeta {
    pub voice: String,
    pub name: Option<String>,
    pub pan: Option<f64>,
    #[serde(rename = "type")]
    pub kind: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScenarioTextEntry {
    pub source_event: SourceEventRef,
    pub speaker_name: Option<String>,
    pub display_speaker_name: Option<String>,
    pub japanese_text: Option<String>,
    pub voices: Vec<VoiceMeta>,
    #[serde(default)]
    pub appended_integer: Option<i64>,
    pub rejection_reason: Option<RejectReason>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceObservation {
    pub source_event: SourceEventRef,
    pub raw_japanese_text: Option<String>,
    pub scenario_speaker_name: Option<String>,
    pub display_speaker_name: Option<String>,
    pub voices: Vec<VoiceMeta>,
    #[serde(default)]
    pub appended_integer: Option<i64>,
}

pub type SpeakerId = String;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizedText {
    pub raw_text: String,
    pub display_text: String,
    pub spoken_text: String,
    pub quality_flags: Vec<QualityFlag>,
    pub normalization_version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceAsset {
    pub source_ogg: PathBuf,
    pub voice_id: String,
    pub source_speaker: String,
    pub byte_size: u64,
    pub source_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CandidateUtterance {
    pub source_event: SourceEventRef,
    pub voice: VoiceMeta,
    pub speaker_name: Option<String>,
    pub raw_text: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MatchedUtterance {
    pub candidate: CandidateUtterance,
    pub source_events: Vec<SourceEventRef>,
    pub source_observations: Vec<SourceObservation>,
    pub audio: VoiceAsset,
    pub speaker_id: String,
    pub display_text: String,
    pub spoken_text: String,
    pub normalization_version: String,
    pub quality_flags: Vec<QualityFlag>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AcceptedUtterance {
    pub schema_version: String,
    pub utterance_id: String,
    pub voice_id: String,
    pub speaker_id: String,
    pub speaker_name: String,
    pub source_ogg: PathBuf,
    pub source_scn: String,
    pub scenario_event_index: usize,
    pub primary_source_event: SourceEventRef,
    pub source_events: Vec<SourceEventRef>,
    pub source_observations: Vec<SourceObservation>,
    pub audio_path: PathBuf,
    pub raw_text: String,
    pub display_text: String,
    pub spoken_text: String,
    pub language: String,
    pub duration_seconds: f64,
    pub source_codec: String,
    pub source_sample_rate: u32,
    pub source_channels: u16,
    pub output_sample_rate: u32,
    pub output_channels: u16,
    pub output_sample_format: String,
    pub source_sha256: String,
    pub output_sha256: String,
    pub normalization_version: String,
    pub quality_flags: Vec<QualityFlag>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RejectedUtterance {
    pub voice_id: Option<String>,
    pub source_ogg: Option<PathBuf>,
    pub source_events: Vec<SourceEventRef>,
    pub source_observations: Vec<SourceObservation>,
    pub stage: String,
    pub reason_code: RejectReason,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityFlag {
    Singing,
    NonVerbalVocalization,
    VeryShortDuration,
    VeryLongDuration,
    PossibleMusicOrSfx,
    SystemPrompt,
    UnknownMarkup,
}

pub fn quality_flags_cell(flags: &[QualityFlag]) -> String {
    flags
        .iter()
        .map(|flag| match flag {
            QualityFlag::Singing => "singing",
            QualityFlag::NonVerbalVocalization => "non_verbal_vocalization",
            QualityFlag::VeryShortDuration => "very_short_duration",
            QualityFlag::VeryLongDuration => "very_long_duration",
            QualityFlag::PossibleMusicOrSfx => "possible_music_or_sfx",
            QualityFlag::SystemPrompt => "system_prompt",
            QualityFlag::UnknownMarkup => "unknown_markup",
        })
        .collect::<Vec<_>>()
        .join(";")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    MultiVoiceEvent,
    MissingJapaneseText,
    MissingAudio,
    DuplicateAudioId,
    OrphanAudio,
    ConflictingText,
    ConflictingSpeaker,
    UnresolvedSpeaker,
    UnknownEventSchema,
    UnknownMarkup,
    MalformedMarkup,
    InvalidAudio,
    ConversionFailed,
}
