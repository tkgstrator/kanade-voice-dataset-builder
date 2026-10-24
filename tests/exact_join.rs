use kanade_tts::inventory::VoiceInventory;
use kanade_tts::join::{JoinInput, join_candidates};
use kanade_tts::manifest::{render_final_manifests, write_join_diagnostics};
use kanade_tts::model::{
    AcceptedUtterance, QualityFlag, RejectReason, ScenarioTextEntry, SourceEventRef,
    SourceObservation, VoiceAsset, VoiceMeta,
};
use kanade_tts::speakers::{SpeakerAliases, SpeakerCatalog};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Case(PathBuf);
impl Case {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "kanade-join-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn source(&self) -> PathBuf {
        let path = self.0.join("source");
        std::fs::create_dir_all(&path).unwrap();
        path
    }
    fn work(&self) -> PathBuf {
        self.0.join("KANADE-v1.work")
    }
}
impl Drop for Case {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn asset(id: &str, speaker: &str, path: &str) -> VoiceAsset {
    VoiceAsset {
        source_ogg: PathBuf::from(path),
        voice_id: id.into(),
        source_speaker: speaker.into(),
        byte_size: 3,
        source_sha256: "a".repeat(64),
    }
}
fn event(id: &str, speaker: &str, text: Option<&str>, index: usize) -> ScenarioTextEntry {
    ScenarioTextEntry {
        source_event: SourceEventRef {
            source_scn: "scn00.scn".into(),
            scenario_event_index: index,
            scene_index: 0,
            scene_text_index: index,
        },
        speaker_name: Some(speaker.into()),
        display_speaker_name: None,
        japanese_text: text.map(str::to_owned),
        voices: vec![VoiceMeta {
            voice: id.into(),
            name: Some(speaker.into()),
            pan: None,
            kind: None,
        }],
        appended_integer: None,
        rejection_reason: None,
    }
}

#[test]
fn exact_voice_name_resolves_when_display_speaker_is_absent_or_unknown() {
    let case = Case::new();
    for display in [None, Some("？？？"), Some("全員")] {
        let mut entry = event("KAN_1", "カナデ", Some("はい"), 0);
        entry.speaker_name = display.map(str::to_owned);
        entry.voices[0].name = Some("カナデ".into());
        let report = join_candidates(input(
            &case.source(),
            vec![asset("KAN_1", "カナデ", "カナデ/KAN_1.ogg")],
            vec![entry],
        ));
        assert_eq!(
            report.matched.len(),
            1,
            "{display:?}: {:?}",
            report.rejected
        );
        assert_eq!(
            report.matched[0].candidate.voice.name.as_deref(),
            Some("カナデ")
        );
    }
}

#[test]
fn voice_name_must_be_present_and_match_audio_speaker_exactly() {
    let case = Case::new();
    for voice_name in [None, Some("伊織"), Some("カナデ ")] {
        let mut entry = event("KAN_1", "カナデ", Some("はい"), 0);
        entry.voices[0].name = voice_name.map(str::to_owned);
        let report = join_candidates(input(
            &case.source(),
            vec![asset("KAN_1", "カナデ", "カナデ/KAN_1.ogg")],
            vec![entry],
        ));
        assert!(report.matched.is_empty(), "{voice_name:?}");
        assert!(matches!(
            report.rejected[0].reason_code,
            RejectReason::UnresolvedSpeaker | RejectReason::ConflictingSpeaker
        ));
    }
}
fn input(
    source_root: &Path,
    assets: Vec<VoiceAsset>,
    entries: Vec<ScenarioTextEntry>,
) -> JoinInput {
    let catalog = SpeakerCatalog {
        speakers: BTreeMap::from([
            ("カナデ".into(), "kan".into()),
            ("伊織".into(), "io".into()),
        ]),
    };
    let aliases = SpeakerAliases::from_json(r#"{"schema_version":"kanade-speaker-aliases-v1","aliases":[{"alias":"カナデ","source_speaker":"カナデ"},{"alias":"伊織","source_speaker":"伊織"}]}"#, &catalog).unwrap();
    JoinInput {
        inventory: VoiceInventory {
            assets,
            sidecars: vec![],
            speakers: vec!["カナデ".into(), "伊織".into()],
        },
        entries,
        catalog,
        aliases,
        source_root: source_root.to_owned(),
        fingerprint: "source-config-sha256".into(),
    }
}

#[test]
fn exact_pair_and_identical_repetitions_preserve_ordered_events() {
    let case = Case::new();
    let source = case.source();
    let report = join_candidates(input(
        &source,
        vec![asset("kan_1", "カナデ", "カナデ/KAN_1.ogg")],
        vec![
            event("KAN_1", "カナデ", Some("はい"), 2),
            event("kan_1", "カナデ", Some("はい"), 1),
        ],
    ));
    assert_eq!(report.matched.len(), 1);
    assert_eq!(
        report.matched[0]
            .source_events
            .iter()
            .map(|e| e.scenario_event_index)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(report.matched[0].spoken_text, "はい");
    assert!(report.rejected.is_empty());
    assert!(report.orphan_audio.is_empty());
}

#[test]
fn repeated_events_compare_normalized_text_and_choose_first_sorted_raw() {
    let case = Case::new();
    let report = join_candidates(input(
        &case.source(),
        vec![asset("kan_1", "カナデ", "カナデ/KAN_1.ogg")],
        vec![
            event("KAN_1", "カナデ", Some("はい"), 2),
            event("kan_1", "カナデ", Some("%Cはい"), 1),
        ],
    ));
    assert!(report.rejected.is_empty());
    assert_eq!(report.matched.len(), 1);
    assert_eq!(
        report.matched[0].candidate.raw_text.as_deref(),
        Some("%Cはい")
    );
    assert_eq!(report.matched[0].display_text, "はい");
    assert_eq!(report.matched[0].spoken_text, "はい");
    assert_eq!(report.matched[0].source_observations.len(), 2);
    assert_eq!(
        report.matched[0].source_observations[0]
            .raw_japanese_text
            .as_deref(),
        Some("%Cはい")
    );
    assert_eq!(
        report.matched[0].source_observations[1]
            .raw_japanese_text
            .as_deref(),
        Some("はい")
    );
    let nfc = join_candidates(input(
        &case.source(),
        vec![asset("kan_1", "カナデ", "カナデ/KAN_1.ogg")],
        vec![
            event("KAN_1", "カナデ", Some("ガ"), 2),
            event("kan_1", "カナデ", Some("カ\u{3099}"), 1),
        ],
    ));
    assert_eq!(nfc.matched.len(), 1);
    assert_eq!(nfc.matched[0].spoken_text, "ガ");
}

#[test]
fn markup_error_in_a_later_repeated_event_is_not_masked_as_text_conflict() {
    let case = Case::new();
    let report = join_candidates(input(
        &case.source(),
        vec![asset("kan_1", "カナデ", "カナデ/KAN_1.ogg")],
        vec![
            event("kan_1", "カナデ", Some("はい"), 1),
            event("kan_1", "カナデ", Some("%q;はい"), 2),
        ],
    ));
    assert!(report.matched.is_empty());
    assert_eq!(report.rejected[0].reason_code, RejectReason::UnknownMarkup);
}

#[test]
fn conflict_diagnostics_retain_every_raw_text_speaker_and_voice_observation() {
    let case = Case::new();
    let source = case.source();
    let mut first = event("kan_1", "カナデ", Some("はい"), 1);
    first.display_speaker_name = Some("奏".into());
    first.voices[0].name = Some("voice-one".into());
    first.voices[0].pan = Some(0.25);
    first.voices[0].kind = Some(2);
    let second = event("kan_1", "伊織", Some("いいえ"), 2);
    let audio = asset("kan_1", "カナデ", "カナデ/KAN_1.ogg");
    let report = join_candidates(input(
        &source,
        vec![audio.clone()],
        vec![second.clone(), first.clone()],
    ));
    assert_eq!(
        report.rejected[0].reason_code,
        RejectReason::ConflictingText
    );
    write_join_diagnostics(&report, &case.work()).unwrap();
    let jsonl = std::fs::read(case.work().join("reports/rejected.jsonl")).unwrap();
    let row: serde_json::Value = serde_json::from_slice(jsonl.trim_ascii()).unwrap();
    let observations = row["source_observations"].as_array().unwrap();
    assert_eq!(observations.len(), 2);
    assert_eq!(observations[0]["source_event"]["scenario_event_index"], 1);
    assert_eq!(observations[0]["raw_japanese_text"], "はい");
    assert_eq!(observations[0]["scenario_speaker_name"], "カナデ");
    assert_eq!(observations[0]["display_speaker_name"], "奏");
    assert_eq!(observations[0]["voices"][0]["name"], "voice-one");
    assert_eq!(observations[0]["voices"][0]["pan"], 0.25);
    assert_eq!(observations[0]["voices"][0]["type"], 2);
    assert_eq!(observations[1]["raw_japanese_text"], "いいえ");
    assert_eq!(observations[1]["scenario_speaker_name"], "伊織");
    let csv_before = std::fs::read(case.work().join("reports/rejected.csv")).unwrap();
    let csv_text = String::from_utf8_lossy(&csv_before);
    assert!(csv_text.contains("source_observations"));
    assert!(csv_text.contains("voice-one"));
    let shuffled = join_candidates(input(&source, vec![audio], vec![first, second]));
    write_join_diagnostics(&shuffled, &case.work()).unwrap();
    assert_eq!(
        jsonl,
        std::fs::read(case.work().join("reports/rejected.jsonl")).unwrap()
    );
    assert_eq!(
        csv_before,
        std::fs::read(case.work().join("reports/rejected.csv")).unwrap()
    );
}

#[test]
fn speaker_conflict_diagnostics_preserve_both_observed_speakers() {
    let case = Case::new();
    let report = join_candidates(input(
        &case.source(),
        vec![asset("kan_1", "カナデ", "カナデ/KAN_1.ogg")],
        vec![
            event("kan_1", "伊織", Some("はい"), 2),
            event("kan_1", "カナデ", Some("はい"), 1),
        ],
    ));
    assert_eq!(
        report.rejected[0].reason_code,
        RejectReason::ConflictingSpeaker
    );
    let row = serde_json::to_value(&report.rejected[0]).unwrap();
    assert_eq!(
        row["source_observations"][0]["scenario_speaker_name"],
        "カナデ"
    );
    assert_eq!(
        row["source_observations"][1]["scenario_speaker_name"],
        "伊織"
    );
}

#[test]
fn missing_and_orphan_audio_are_separate_and_every_physical_asset_is_accounted() {
    let case = Case::new();
    let source = case.source();
    let report = join_candidates(input(
        &source,
        vec![asset("orphan", "カナデ", "カナデ/orphan.ogg")],
        vec![event("missing", "カナデ", Some("はい"), 0)],
    ));
    assert_eq!(report.missing_audio.len(), 1);
    assert_eq!(
        report.missing_audio[0].reason_code,
        RejectReason::MissingAudio
    );
    assert_eq!(report.orphan_audio.len(), 1);
    assert_eq!(
        report.orphan_audio[0].source_ogg,
        PathBuf::from("カナデ/orphan.ogg")
    );
    assert_eq!(report.physical_audio_count, 1);
}

#[test]
fn missing_audio_accounting_survives_other_event_rejections() {
    let case = Case::new();
    let report = join_candidates(input(
        &case.source(),
        vec![],
        vec![event("missing", "カナデ", None, 0)],
    ));
    assert_eq!(
        report.rejected[0].reason_code,
        RejectReason::MissingJapaneseText
    );
    assert_eq!(report.missing_audio.len(), 1);
    assert_eq!(
        report.missing_audio[0].reason_code,
        RejectReason::MissingAudio
    );
}

#[test]
fn colliding_ids_reject_each_physical_ogg_without_overwriting() {
    let case = Case::new();
    let source = case.source();
    let report = join_candidates(input(
        &source,
        vec![
            asset("KAN_1", "カナデ", "カナデ/KAN_1.ogg"),
            asset("kan_1", "伊織", "伊織/kan_1.ogg"),
        ],
        vec![event("Kan_1", "カナデ", Some("はい"), 0)],
    ));
    assert_eq!(report.physical_audio_count, 2);
    assert_eq!(report.accounted_physical_audio_count(), 2);
    assert!(report.matched.is_empty());
    assert_eq!(
        report
            .rejected
            .iter()
            .filter(|r| r.reason_code == RejectReason::DuplicateAudioId)
            .count(),
        2
    );
    assert_eq!(
        report
            .rejected
            .iter()
            .filter_map(|r| r.source_ogg.as_ref())
            .count(),
        2
    );
}

#[test]
fn diagnostics_refuse_a_report_that_loses_a_physical_ogg() {
    let case = Case::new();
    let mut report = join_candidates(input(
        &case.source(),
        vec![asset("orphan", "カナデ", "カナデ/orphan.ogg")],
        vec![],
    ));
    report.orphan_audio.clear();
    assert!(write_join_diagnostics(&report, &case.work()).is_err());
    assert!(!case.work().exists());
}

#[test]
fn conflicts_and_invalid_events_never_become_matched() {
    let case = Case::new();
    let source = case.source();
    let cases = [
        (
            vec![
                event("kan_1", "カナデ", Some("はい"), 0),
                event("kan_1", "カナデ", Some("いいえ"), 1),
            ],
            RejectReason::ConflictingText,
        ),
        (
            vec![
                event("kan_1", "カナデ", Some("はい"), 0),
                event("kan_1", "伊織", Some("はい"), 1),
            ],
            RejectReason::ConflictingSpeaker,
        ),
        (
            vec![event("kan_1", "不明", Some("はい"), 0)],
            RejectReason::UnresolvedSpeaker,
        ),
        (
            vec![event("kan_1", "カナデ", None, 0)],
            RejectReason::MissingJapaneseText,
        ),
        (
            vec![event("kan_1", "カナデ", Some("%q;本文"), 0)],
            RejectReason::UnknownMarkup,
        ),
    ];
    for (entries, reason) in cases {
        let report = join_candidates(input(
            &source,
            vec![asset("kan_1", "カナデ", "カナデ/KAN_1.ogg")],
            entries,
        ));
        assert!(report.matched.is_empty());
        assert!(
            report.rejected.iter().any(|r| r.reason_code == reason),
            "{reason:?}"
        );
    }
    let mut multi = event("kan_1", "カナデ", Some("はい"), 0);
    multi.voices.push(multi.voices[0].clone());
    let report = join_candidates(input(
        &source,
        vec![asset("kan_1", "カナデ", "カナデ/KAN_1.ogg")],
        vec![multi],
    ));
    assert_eq!(
        report.rejected[0].reason_code,
        RejectReason::MultiVoiceEvent
    );
    let report = join_candidates(input(
        &source,
        vec![asset("kan_1", "カナデ", "カナデ/KAN_1.ogg")],
        vec![ScenarioTextEntry {
            voices: vec![],
            ..event("kan_1", "カナデ", Some("はい"), 0)
        }],
    ));
    assert!(report.matched.is_empty());
    assert_eq!(report.orphan_audio.len(), 1);
}

#[test]
fn diagnostics_are_deterministic_atomic_and_fingerprint_gated() {
    let case = Case::new();
    let source = case.source();
    let a = asset("kan_1", "カナデ", "カナデ/KAN_1.ogg");
    let b = asset("orphan", "伊織", "伊織/orphan.ogg");
    let report = join_candidates(input(
        &source,
        vec![a.clone(), b.clone()],
        vec![
            event("kan_1", "カナデ", Some("はい"), 1),
            event("missing", "カナデ", Some("はい"), 0),
        ],
    ));
    write_join_diagnostics(&report, &case.work()).unwrap();
    let first = std::fs::read(case.work().join("reports/rejected.jsonl")).unwrap();
    let first_csv = std::fs::read(case.work().join("reports/rejected.csv")).unwrap();
    let missing = std::fs::read(case.work().join("reports/missing-audio.jsonl")).unwrap();
    assert!(String::from_utf8_lossy(&missing).contains("\"reason_code\":\"missing_audio\""));
    let shuffled = join_candidates(input(
        &source,
        vec![b, a],
        vec![
            event("missing", "カナデ", Some("はい"), 0),
            event("kan_1", "カナデ", Some("はい"), 1),
        ],
    ));
    write_join_diagnostics(&shuffled, &case.work()).unwrap();
    assert_eq!(
        first,
        std::fs::read(case.work().join("reports/rejected.jsonl")).unwrap()
    );
    assert_eq!(
        first_csv,
        std::fs::read(case.work().join("reports/rejected.csv")).unwrap()
    );
    assert!(!String::from_utf8_lossy(&first).contains("\"stage\":\"\",\"reason_code\""));
    assert!(!case.work().join("metadata.csv").exists());
    assert!(!case.work().join("manifests/accepted.jsonl").exists());
    assert!(
        !std::fs::read_dir(case.work().join("reports"))
            .unwrap()
            .any(|p| p.unwrap().file_name().to_string_lossy().ends_with(".tmp"))
    );
    let mut drift = shuffled.clone();
    drift.fingerprint = "different".into();
    assert!(write_join_diagnostics(&drift, &case.work()).is_err());
    let unsafe_root = case.0.join("unexpected.work");
    std::fs::create_dir(&unsafe_root).unwrap();
    assert!(write_join_diagnostics(&shuffled, &unsafe_root).is_err());
    assert!(write_join_diagnostics(&shuffled, &source.join("nested.work")).is_err());
}

#[test]
fn diagnostics_work_root_is_sibling_of_final_output_even_when_source_is_elsewhere() {
    let case = Case::new();
    let source = case.source();
    let output_parent = case.0.join("tts-dataset");
    std::fs::create_dir(&output_parent).unwrap();
    let work = output_parent.join("KANADE-v1.work");
    let report = join_candidates(input(
        &source,
        vec![],
        vec![event("missing", "カナデ", Some("はい"), 0)],
    ));
    write_join_diagnostics(&report, &work).unwrap();
    assert!(work.join("reports/rejected.jsonl").is_file());
    assert!(!output_parent.join("KANADE-v1").exists());
}

#[test]
fn diagnostics_resume_with_known_later_stage_work_tree_entries() {
    let case = Case::new();
    let report = join_candidates(input(
        &case.source(),
        vec![],
        vec![event("missing", "カナデ", Some("はい"), 0)],
    ));
    let work = case.work();
    write_join_diagnostics(&report, &work).unwrap();
    std::fs::create_dir_all(work.join("audio/kan")).unwrap();
    std::fs::create_dir(work.join("manifests")).unwrap();
    let preserved = [
        "audio/kan/verified.wav",
        "manifests/accepted.jsonl",
        "reports/manual-review.jsonl",
        "metadata.csv",
        "speakers.json",
        "report.json",
        "README.md",
    ];
    for name in preserved {
        std::fs::write(work.join(name), name.as_bytes()).unwrap();
    }
    write_join_diagnostics(&report, &work).unwrap();
    for name in preserved {
        assert_eq!(std::fs::read(work.join(name)).unwrap(), name.as_bytes());
    }
}

#[test]
fn completion_or_publication_marker_blocks_diagnostics_before_any_report_mutation() {
    for marker in ["dataset-complete.json", "publication-manifest.json"] {
        let case = Case::new();
        let source = case.source();
        let initial = join_candidates(input(
            &source,
            vec![],
            vec![event("first", "カナデ", Some("はい"), 0)],
        ));
        let work = case.work();
        write_join_diagnostics(&initial, &work).unwrap();
        let names = [
            "rejected.jsonl",
            "rejected.csv",
            "orphan-audio.jsonl",
            "missing-audio.jsonl",
            "join-summary.json",
        ];
        let before: Vec<_> = names
            .iter()
            .map(|name| std::fs::read(work.join("reports").join(name)).unwrap())
            .collect();
        std::fs::write(work.join(marker), b"sealed").unwrap();
        let changed = join_candidates(input(
            &source,
            vec![],
            vec![event("second", "カナデ", Some("いいえ"), 0)],
        ));
        assert!(write_join_diagnostics(&changed, &work).is_err(), "{marker}");
        for (name, bytes) in names.iter().zip(&before) {
            assert_eq!(
                &std::fs::read(work.join("reports").join(name)).unwrap(),
                bytes,
                "{marker}: {name}"
            );
        }
        assert!(
            !std::fs::read_dir(work.join("reports"))
                .unwrap()
                .any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp"))
        );
    }
}

#[test]
fn completion_markers_block_even_when_symlinked_or_wrong_type() {
    for marker in ["dataset-complete.json", "publication-manifest.json"] {
        let case = Case::new();
        let report = join_candidates(input(&case.source(), vec![], vec![]));
        let work = case.work();
        write_join_diagnostics(&report, &work).unwrap();
        std::fs::create_dir(work.join(marker)).unwrap();
        assert!(
            write_join_diagnostics(&report, &work).is_err(),
            "directory {marker}"
        );
        std::fs::remove_dir(work.join(marker)).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(work.join("report.json"), work.join(marker)).unwrap();
            assert!(
                write_join_diagnostics(&report, &work).is_err(),
                "symlink {marker}"
            );
        }
    }
}

#[test]
fn diagnostics_resume_rejects_unknown_and_symlink_entries() {
    let case = Case::new();
    let report = join_candidates(input(&case.source(), vec![], vec![]));
    let work = case.work();
    write_join_diagnostics(&report, &work).unwrap();
    std::fs::write(work.join("unknown.json"), b"foreign").unwrap();
    assert!(write_join_diagnostics(&report, &work).is_err());
    std::fs::remove_file(work.join("unknown.json")).unwrap();
    std::fs::write(work.join("reports/foreign.jsonl"), b"foreign").unwrap();
    assert!(write_join_diagnostics(&report, &work).is_err());
    std::fs::remove_file(work.join("reports/foreign.jsonl")).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            work.join("reports/rejected.jsonl"),
            work.join("reports/manual-review.jsonl"),
        )
        .unwrap();
        assert!(write_join_diagnostics(&report, &work).is_err());
        std::fs::remove_file(work.join("reports/manual-review.jsonl")).unwrap();
        std::os::unix::fs::symlink(work.join("reports"), work.join("audio")).unwrap();
        assert!(write_join_diagnostics(&report, &work).is_err());
    }
}

#[test]
fn accepted_serializer_is_deterministic_and_keeps_rich_provenance_and_snake_case_flags() {
    let mut row = accepted("kan_1", 1);
    row.quality_flags = vec![QualityFlag::PossibleMusicOrSfx];
    let other = accepted("io_2", 2);
    let first = render_final_manifests(&[other.clone(), row.clone()]).unwrap();
    let second = render_final_manifests(&[row, other]).unwrap();
    assert_eq!(first, second);
    let csv = String::from_utf8(first.metadata_csv).unwrap();
    assert!(
        csv.lines()
            .next()
            .unwrap()
            .contains("file_name,transcription,speaker,speaker_id")
    );
    let jsonl = String::from_utf8(first.accepted_jsonl).unwrap();
    assert!(jsonl.contains("\"primary_source_event\""));
    assert!(jsonl.contains("\"source_events\""));
    assert!(jsonl.contains("\"source_observations\""));
    assert!(jsonl.contains("\"possible_music_or_sfx\""));
}

fn accepted(id: &str, index: usize) -> AcceptedUtterance {
    let source_event = SourceEventRef {
        source_scn: "scene.scn".into(),
        scenario_event_index: index,
        scene_index: 0,
        scene_text_index: index,
    };
    AcceptedUtterance {
        schema_version: "1".into(),
        utterance_id: id.into(),
        voice_id: id.into(),
        speaker_id: "kan".into(),
        speaker_name: "カナデ".into(),
        source_ogg: PathBuf::from(format!("カナデ/{id}.ogg")),
        source_scn: "scene.scn".into(),
        scenario_event_index: index,
        primary_source_event: source_event.clone(),
        source_events: vec![source_event],
        source_observations: vec![SourceObservation {
            source_event: SourceEventRef {
                source_scn: "scene.scn".into(),
                scenario_event_index: index,
                scene_index: 0,
                scene_text_index: index,
            },
            raw_japanese_text: Some("はい".into()),
            scenario_speaker_name: Some("カナデ".into()),
            display_speaker_name: None,
            voices: vec![VoiceMeta {
                voice: id.into(),
                name: None,
                pan: None,
                kind: None,
            }],
            appended_integer: None,
        }],
        audio_path: PathBuf::from(format!("audio/kan/{id}.wav")),
        raw_text: "はい".into(),
        display_text: "はい".into(),
        spoken_text: "はい".into(),
        language: "ja".into(),
        duration_seconds: 1.0,
        source_codec: "vorbis".into(),
        source_sample_rate: 44100,
        source_channels: 2,
        output_sample_rate: 44100,
        output_channels: 1,
        output_sample_format: "s16".into(),
        source_sha256: "a".repeat(64),
        output_sha256: "b".repeat(64),
        normalization_version: "kanade-text-v2".into(),
        quality_flags: vec![],
    }
}
