use crate::error::Error;
use crate::inventory::{VoiceInventory, normalize_voice_id};
use crate::model::{
    CandidateUtterance, MatchedUtterance, NormalizedText, RejectReason, RejectedUtterance,
    ScenarioTextEntry, SourceEventRef, SourceObservation, VoiceAsset,
};
use crate::speakers::{SpeakerAliases, SpeakerCatalog};
use crate::text::{NormalizationRules, normalize_text};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct JoinInput {
    pub inventory: VoiceInventory,
    pub entries: Vec<ScenarioTextEntry>,
    pub catalog: SpeakerCatalog,
    pub aliases: SpeakerAliases,
    /// Canonical source root is checked again by the diagnostics writer.
    pub source_root: PathBuf,
    /// A caller supplied digest of source and configuration inputs.
    pub fingerprint: String,
}

#[derive(Clone, Debug)]
pub struct JoinReport {
    pub matched: Vec<MatchedUtterance>,
    pub rejected: Vec<RejectedUtterance>,
    pub orphan_audio: Vec<VoiceAsset>,
    pub missing_audio: Vec<RejectedUtterance>,
    pub physical_audio_count: usize,
    pub source_root: PathBuf,
    pub fingerprint: String,
}

impl JoinReport {
    /// Counts physical OGG dispositions, independently of scenario reference count.
    pub fn accounted_physical_audio_count(&self) -> usize {
        self.matched.len()
            + self.orphan_audio.len()
            + self
                .rejected
                .iter()
                .filter(|row| row.source_ogg.is_some())
                .count()
    }
}

fn event_key(event: &SourceEventRef) -> (&str, usize, usize, usize) {
    (
        &event.source_scn,
        event.scenario_event_index,
        event.scene_index,
        event.scene_text_index,
    )
}

fn same_output_text(left: &NormalizedText, right: &NormalizedText) -> bool {
    left.display_text == right.display_text
        && left.spoken_text == right.spoken_text
        && left.normalization_version == right.normalization_version
        && left.quality_flags == right.quality_flags
}

fn observation(entry: &ScenarioTextEntry) -> SourceObservation {
    SourceObservation {
        source_event: entry.source_event.clone(),
        raw_japanese_text: entry.japanese_text.clone(),
        scenario_speaker_name: entry.speaker_name.clone(),
        display_speaker_name: entry.display_speaker_name.clone(),
        voices: entry.voices.clone(),
        appended_integer: entry.appended_integer,
    }
}

fn rejection(
    id: &str,
    audio: Option<&VoiceAsset>,
    events: &[&ScenarioTextEntry],
    reason: RejectReason,
    detail: String,
) -> RejectedUtterance {
    RejectedUtterance {
        voice_id: Some(id.to_owned()),
        source_ogg: audio.map(|asset| asset.source_ogg.clone()),
        source_events: events
            .iter()
            .map(|entry| entry.source_event.clone())
            .collect(),
        source_observations: events.iter().map(|entry| observation(entry)).collect(),
        stage: "join".into(),
        reason_code: reason,
        detail,
    }
}

/// Finds candidates only. A matched row is not an accepted row until Task 6 verifies its WAV.
pub fn join_candidates(input: JoinInput) -> JoinReport {
    let mut audio_by_id = BTreeMap::<String, Vec<VoiceAsset>>::new();
    for asset in input.inventory.assets {
        audio_by_id
            .entry(normalize_voice_id(&asset.voice_id))
            .or_default()
            .push(asset);
    }
    for assets in audio_by_id.values_mut() {
        assets.sort_by(|a, b| a.source_ogg.cmp(&b.source_ogg));
    }
    let physical_audio_count = audio_by_id.values().map(Vec::len).sum();
    let mut entries_by_id = BTreeMap::<String, Vec<ScenarioTextEntry>>::new();
    for entry in input.entries {
        for voice in &entry.voices {
            entries_by_id
                .entry(normalize_voice_id(&voice.voice))
                .or_default()
                .push(entry.clone());
        }
    }
    let mut matched = Vec::new();
    let mut rejected = Vec::new();
    let mut missing_audio = Vec::new();
    let mut orphan_audio = Vec::new();
    for (id, assets) in &audio_by_id {
        if !entries_by_id.contains_key(id) {
            orphan_audio.extend(assets.iter().cloned());
        }
    }
    for (id, mut entries) in entries_by_id {
        entries.sort_by(|a, b| event_key(&a.source_event).cmp(&event_key(&b.source_event)));
        let refs: Vec<_> = entries.iter().collect();
        let assets = audio_by_id.get(&id).map(Vec::as_slice).unwrap_or(&[]);
        let source_events: Vec<_> = entries.iter().map(|e| e.source_event.clone()).collect();
        let source_observations: Vec<_> = entries.iter().map(observation).collect();
        if assets.is_empty() {
            missing_audio.push(rejection(
                &id,
                None,
                &refs,
                RejectReason::MissingAudio,
                "no OGG has this normalized voice ID".into(),
            ));
        }
        let mut failure = if entries.iter().any(|e| {
            e.voices.len() != 1 || e.rejection_reason == Some(RejectReason::MultiVoiceEvent)
        }) {
            Some((
                RejectReason::MultiVoiceEvent,
                "text entry has multiple voice objects".to_owned(),
            ))
        } else if entries
            .iter()
            .any(|e| e.rejection_reason == Some(RejectReason::UnknownEventSchema))
        {
            Some((
                RejectReason::UnknownEventSchema,
                "unrecognized scenario event".to_owned(),
            ))
        } else if entries
            .iter()
            .any(|e| e.japanese_text.as_ref().is_none_or(|s| s.is_empty()))
        {
            Some((
                RejectReason::MissingJapaneseText,
                "Japanese text is absent".to_owned(),
            ))
        } else {
            None
        };
        let normalized: Vec<_> = if failure.is_none() {
            entries
                .iter()
                .map(|entry| {
                    normalize_text(
                        entry.japanese_text.as_deref().unwrap_or_default(),
                        &NormalizationRules::kanade(),
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        if failure.is_none() {
            failure = normalized.iter().find_map(|result| match result {
                Err(Error::UnknownMarkup(detail)) => {
                    Some((RejectReason::UnknownMarkup, detail.clone()))
                }
                Err(Error::MalformedMarkup(detail)) => {
                    Some((RejectReason::MalformedMarkup, detail.clone()))
                }
                Err(other) => Some((RejectReason::MalformedMarkup, other.to_string())),
                Ok(_) => None,
            });
        }
        if failure.is_none() {
            let first = normalized[0]
                .as_ref()
                .expect("normalization errors handled");
            if normalized.iter().skip(1).any(|result| {
                let other = result.as_ref().expect("normalization errors handled");
                !same_output_text(first, other)
            }) {
                failure = Some((
                    RejectReason::ConflictingText,
                    "repeated voice ID has different normalized Japanese text".to_owned(),
                ));
            }
        }
        if failure.is_none() {
            let speakers: Vec<_> = entries
                .iter()
                .map(|e| {
                    e.voices[0]
                        .name
                        .as_deref()
                        .and_then(|s| input.aliases.source_speaker(s))
                })
                .collect();
            if speakers.iter().any(Option::is_none) {
                failure = Some((
                    RejectReason::UnresolvedSpeaker,
                    "voice metadata speaker has no exact alias".to_owned(),
                ));
            } else if speakers.iter().any(|s| s != &speakers[0]) {
                failure = Some((
                    RejectReason::ConflictingSpeaker,
                    "repeated voice ID resolves to different speakers".to_owned(),
                ));
            } else if assets.len() > 1 {
                failure = Some((
                    RejectReason::DuplicateAudioId,
                    format!("{} physical OGGs share normalized ID", assets.len()),
                ));
            } else if assets.is_empty() {
                failure = Some((
                    RejectReason::MissingAudio,
                    "no OGG has this normalized voice ID".to_owned(),
                ));
            } else if speakers[0] != Some(assets[0].source_speaker.as_str()) {
                failure = Some((
                    RejectReason::ConflictingSpeaker,
                    "voice metadata speaker differs from OGG source directory".to_owned(),
                ));
            }
        }
        if let Some((reason, detail)) = failure {
            if assets.is_empty() {
                let row = rejection(&id, None, &refs, reason.clone(), detail);
                rejected.push(row);
            } else {
                for asset in assets {
                    rejected.push(rejection(
                        &id,
                        Some(asset),
                        &refs,
                        reason.clone(),
                        detail.clone(),
                    ));
                }
            }
            continue;
        }
        let entry = &entries[0];
        let audio = &assets[0];
        let speaker_id = input.catalog.speakers.get(&audio.source_speaker);
        let Some(speaker_id) = speaker_id else {
            rejected.push(rejection(
                &id,
                Some(audio),
                &refs,
                RejectReason::UnresolvedSpeaker,
                "source speaker absent from catalog".into(),
            ));
            continue;
        };
        let normalized = normalized
            .into_iter()
            .next()
            .expect("normalization attempted")
            .expect("failure handled");
        matched.push(MatchedUtterance {
            candidate: CandidateUtterance {
                source_event: entry.source_event.clone(),
                voice: entry.voices[0].clone(),
                speaker_name: entry.speaker_name.clone(),
                raw_text: entry.japanese_text.clone(),
            },
            source_events,
            source_observations,
            audio: audio.clone(),
            speaker_id: speaker_id.clone(),
            display_text: normalized.display_text,
            spoken_text: normalized.spoken_text,
            normalization_version: normalized.normalization_version,
            quality_flags: normalized.quality_flags,
        });
    }
    rejected.sort_by(|a, b| {
        (
            &a.voice_id,
            &a.source_ogg,
            &a.source_events[0].source_scn,
            a.source_events[0].scenario_event_index,
        )
            .cmp(&(
                &b.voice_id,
                &b.source_ogg,
                &b.source_events[0].source_scn,
                b.source_events[0].scenario_event_index,
            ))
    });
    missing_audio.sort_by(|a, b| a.voice_id.cmp(&b.voice_id));
    orphan_audio.sort_by(|a, b| a.source_ogg.cmp(&b.source_ogg));
    JoinReport {
        matched,
        rejected,
        orphan_audio,
        missing_audio,
        physical_audio_count,
        source_root: input.source_root,
        fingerprint: input.fingerprint,
    }
}
