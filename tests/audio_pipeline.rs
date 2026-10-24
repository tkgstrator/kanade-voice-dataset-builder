use base64::Engine;
use clap::Parser;
use emote_psb::{psb::write::PsbWriter, value::PsbValue};
use kanade_tts::audio::{AudioConfig, convert_one};
use kanade_tts::build::{
    BuildConfig, build_fixture_dataset_for_tests as build_dataset,
    finalize_fixture_dataset_for_tests as finalize_dataset,
};
use kanade_tts::cli::Cli;
use kanade_tts::error::Error;
use kanade_tts::model::{
    CandidateUtterance, MatchedUtterance, SourceEventRef, SourceObservation, VoiceAsset, VoiceMeta,
};
use kanade_tts::probe::probe_audio;
use kanade_tts::readme::write_dataset_card;
use kanade_tts::review::{
    ReviewDecision, record_fixture_manual_review_for_tests as record_manual_review,
    review_fixture_required_for_tests as review_required,
};
use kanade_tts::verify::{VerificationConfig, verify_fixture_dataset_for_tests as verify_dataset};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Case(PathBuf);
impl Case {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "kanade-audio-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn source(&self, channels: u8) -> PathBuf {
        let input = self.0.join("source/voice/カナデ/KAN_1.ogg");
        fs::create_dir_all(input.parent().unwrap()).unwrap();
        let status = Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
            ])
            .arg("sine=frequency=440:duration=0.2")
            .args([
                "-ac",
                &channels.to_string(),
                "-c:a",
                "vorbis",
                "-strict",
                "-2",
                "-y",
            ])
            .arg(&input)
            .status()
            .unwrap();
        assert!(status.success());
        input
    }
    fn config(&self) -> AudioConfig {
        AudioConfig {
            source_root: self.0.join("source"),
            work_root: self.0.join("KANADE-v1.work"),
            ffmpeg: PathBuf::from("ffmpeg"),
            ffprobe: PathBuf::from("ffprobe"),
        }
    }
    fn dataset(&self, multi_voice: bool) -> BuildConfig {
        let source_root = self.0.join("source");
        let first = self.source(2);
        if multi_voice {
            fs::copy(&first, first.with_file_name("KAN_2.ogg")).unwrap();
        }
        let aliases: serde_json::Value =
            serde_json::from_str(include_str!("../config/speaker-aliases.json")).unwrap();
        for row in aliases["aliases"].as_array().unwrap() {
            let speaker = row["source_speaker"].as_str().unwrap();
            fs::create_dir_all(source_root.join("voice").join(speaker)).unwrap();
        }
        let scenario = source_root.join("scn/pilot.ks.scn");
        fs::create_dir_all(scenario.parent().unwrap()).unwrap();
        let voices = if multi_voice {
            vec![psb_voice("KAN_1"), psb_voice("KAN_2")]
        } else {
            vec![psb_voice("KAN_1")]
        };
        PsbWriter::new(
            3,
            false,
            &psb_root(voices),
            fs::File::create(scenario).unwrap(),
        )
        .unwrap()
        .finish()
        .unwrap();
        BuildConfig {
            source_root,
            final_root: self.0.join("KANADE-v1"),
            ffmpeg: PathBuf::from("ffmpeg"),
            ffprobe: PathBuf::from("ffprobe"),
        }
    }
}

fn psb_string(value: &str) -> PsbValue {
    PsbValue::String(value.into())
}
fn psb_number(value: i64) -> PsbValue {
    PsbValue::Number(value.into())
}
fn psb_object(fields: Vec<(&str, PsbValue)>) -> PsbValue {
    PsbValue::Object(
        fields
            .into_iter()
            .map(|(name, value)| (name.into(), value))
            .collect(),
    )
}
fn psb_list(values: Vec<PsbValue>) -> PsbValue {
    PsbValue::List(values)
}
fn psb_voice(id: &str) -> PsbValue {
    psb_object(vec![
        ("name", psb_string("カナデ")),
        ("pan", psb_number(0)),
        ("type", psb_number(0)),
        ("voice", psb_string(id)),
    ])
}
fn psb_root(voices: Vec<PsbValue>) -> PsbValue {
    let localization = |speaker: &str, text: &str| {
        psb_list(vec![psb_string(speaker), psb_string(text), psb_number(0)])
    };
    let event = psb_list(vec![
        psb_string("カナデ"),
        psb_list(vec![
            localization("少女", "「こんにちは」"),
            localization("Girl", "Hello"),
            localization("少女", "你好"),
            localization("少女", "你好啊"),
        ]),
        psb_list(voices),
        psb_number(0),
        psb_object(vec![
            ("data", psb_list(vec![])),
            ("env", psb_object(vec![])),
        ]),
    ]);
    let languages = ["en", "cn", "tw"];
    let marker = psb_list(vec![
        psb_number(0),
        psb_object(vec![
            ("data", psb_list(vec![])),
            ("env", psb_object(vec![])),
        ]),
        PsbValue::Null,
        psb_number(0),
        psb_number(0),
    ]);
    let lines = std::iter::once(marker.clone())
        .chain(std::iter::once(psb_number(0)))
        .chain((0..61).map(|_| psb_list(vec![psb_string("control")])))
        .chain(std::iter::once(marker))
        .collect();
    psb_object(vec![
        ("name", psb_string("pilot.ks")),
        (
            "languages",
            psb_list(languages.iter().map(|name| psb_string(name)).collect()),
        ),
        (
            "llmap",
            psb_list(
                languages
                    .iter()
                    .map(|name| {
                        psb_object(vec![
                            ("name", psb_string(&format!("pilot_{name}.ks"))),
                            ("*start", psb_list(vec![psb_number(0)])),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "scenes",
            psb_list(vec![psb_object(vec![
                ("files", psb_list(vec![])),
                ("firstLine", psb_number(0)),
                ("label", psb_string("*start")),
                ("lines", psb_list(lines)),
                (
                    "nexts",
                    psb_list(vec![psb_object(vec![
                        ("type", psb_number(0)),
                        ("storage", psb_string("next.ks")),
                        ("target", psb_string("*start")),
                    ])]),
                ),
                ("spCount", psb_number(1)),
                ("texts", psb_list(vec![event])),
                ("title", psb_list(vec![psb_string(""); 4])),
                ("version", psb_number(1)),
            ])]),
        ),
    ])
}
impl Drop for Case {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn digest(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

// Finder-written fixture from dmgbuild/ds_store tests (MIT license), encoded as text
// so the fixture remains reviewable through ordinary source patches.
fn finder_store() -> Vec<u8> {
    let encoded = include_str!("fixtures/finder-macos.ds_store.b64")
        .lines()
        .collect::<String>();
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap()
}

fn matched(input: &Path) -> MatchedUtterance {
    let event = SourceEventRef {
        source_scn: "scn/example.scn".into(),
        scenario_event_index: 0,
        scene_index: 0,
        scene_text_index: 0,
    };
    let voice = VoiceMeta {
        voice: "KAN_1".into(),
        name: None,
        pan: None,
        kind: None,
    };
    MatchedUtterance {
        candidate: CandidateUtterance {
            source_event: event.clone(),
            voice: voice.clone(),
            speaker_name: Some("カナデ".into()),
            raw_text: Some("はい".into()),
        },
        source_events: vec![event.clone()],
        source_observations: vec![SourceObservation {
            source_event: event,
            raw_japanese_text: Some("はい".into()),
            scenario_speaker_name: Some("カナデ".into()),
            display_speaker_name: None,
            voices: vec![voice],
            appended_integer: None,
        }],
        audio: VoiceAsset {
            source_ogg: PathBuf::from("カナデ/KAN_1.ogg"),
            voice_id: "kan_1".into(),
            source_speaker: "カナデ".into(),
            byte_size: fs::metadata(input).unwrap().len(),
            source_sha256: digest(input),
        },
        speaker_id: "kan".into(),
        display_text: "はい".into(),
        spoken_text: "はい".into(),
        normalization_version: "kanade-text-v2".into(),
        quality_flags: vec![],
    }
}

#[test]
fn converts_stereo_ogg_to_verified_mono_pcm_without_touching_source() {
    let case = Case::new();
    let source = case.source(2);
    let before = digest(&source);
    let row = convert_one(&case.config(), &matched(&source)).unwrap();
    let output = case.config().work_root.join(&row.audio_path);
    let probe = probe_audio(Path::new("ffprobe"), &output).unwrap();
    assert_eq!(
        (probe.codec.as_str(), probe.sample_rate, probe.channels),
        ("pcm_s16le", 44100, 1)
    );
    assert_eq!(row.output_sha256, digest(&output));
    assert_eq!(before, digest(&source));
    assert_eq!(row.source_sha256, before);
    assert!(row.duration_seconds > 0.0);
}

#[test]
fn damaged_wav_is_replaced_and_matching_wav_is_reused() {
    let case = Case::new();
    let source = case.source(2);
    let item = matched(&source);
    let first = convert_one(&case.config(), &item).unwrap();
    let output = case.config().work_root.join(&first.audio_path);
    let bytes = fs::read(&output).unwrap();
    let second = convert_one(&case.config(), &item).unwrap();
    assert_eq!(first.output_sha256, second.output_sha256);
    fs::write(&output, &bytes[..16]).unwrap();
    let repaired = convert_one(&case.config(), &item).unwrap();
    assert_eq!(repaired.output_sha256, first.output_sha256);
    assert_eq!(fs::read(&output).unwrap(), bytes);
}

#[test]
fn incomplete_work_cannot_verify_or_promote_and_never_creates_final() {
    let case = Case::new();
    case.source(2);
    let config = BuildConfig {
        source_root: case.config().source_root,
        final_root: case.0.join("KANADE-v1"),
        ffmpeg: PathBuf::from("ffmpeg"),
        ffprobe: PathBuf::from("ffprobe"),
    };
    fs::create_dir(&case.config().work_root).unwrap();
    assert!(
        verify_dataset(
            &case.config().work_root,
            &config.source_root,
            &VerificationConfig::default(),
            Path::new("ffprobe")
        )
        .is_err()
    );
    assert!(finalize_dataset(&config).is_err());
    assert!(!config.final_root.exists());
    assert!(build_dataset(&config).is_err());
    assert!(!config.final_root.exists());
}

#[test]
fn changed_source_hash_and_unsafe_destination_fail_closed() {
    let case = Case::new();
    let source = case.source(2);
    let item = matched(&source);
    fs::write(&source, b"changed").unwrap();
    assert!(convert_one(&case.config(), &item).is_err());
    assert!(!case.config().work_root.join("audio/kan/kan_1.wav").exists());
    let mut unsafe_item = item;
    unsafe_item.speaker_id = "../escape".into();
    assert!(convert_one(&case.config(), &unsafe_item).is_err());
}

#[test]
fn build_cli_accepts_explicit_audio_tools() {
    assert!(
        Cli::try_parse_from([
            "kanade-tts",
            "build",
            "--source",
            "/source",
            "--output",
            "/output/KANADE-v1",
            "--ffmpeg",
            "/usr/bin/ffmpeg",
            "--ffprobe",
            "/usr/bin/ffprobe",
        ])
        .is_ok()
    );
}

#[test]
fn leftover_temporary_wav_blocks_resume_without_promotion() {
    let case = Case::new();
    let source = case.source(2);
    let directory = case.config().work_root.join("audio/kan");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("kan_1.wav.999999.tmp.wav"), b"partial").unwrap();
    assert!(convert_one(&case.config(), &matched(&source)).is_err());
    assert!(!directory.join("kan_1.wav").exists());
}

#[cfg(unix)]
#[test]
fn audio_symlink_escape_does_not_create_external_destination() {
    let case = Case::new();
    let source = case.source(2);
    let outside = case.0.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::create_dir(&case.config().work_root).unwrap();
    std::os::unix::fs::symlink(&outside, case.config().work_root.join("audio")).unwrap();
    assert!(convert_one(&case.config(), &matched(&source)).is_err());
    assert!(!outside.join("kan").exists());
}

#[test]
fn ffmpeg_failure_is_typed_and_leaves_no_accepted_wav() {
    let case = Case::new();
    let source = case.source(2);
    let mut config = case.config();
    config.ffmpeg = case.0.join("missing-ffmpeg");
    let result = convert_one(&config, &matched(&source));
    assert!(matches!(result, Err(Error::Conversion { .. })));
    assert!(!config.work_root.join("audio/kan/kan_1.wav").exists());
}

#[test]
fn isolated_build_verify_review_finalize_and_final_verify() {
    let case = Case::new();
    let config = case.dataset(false);
    let report = build_dataset(&config).unwrap();
    assert_eq!(report.accepted.len(), 1);
    assert!(!config.final_root.exists());
    let work = case.config().work_root;
    let checked = verify_dataset(
        &work,
        &config.source_root,
        &VerificationConfig::default(),
        &config.ffprobe,
    )
    .unwrap();
    assert_eq!(checked.accepted.len(), 1);
    write_dataset_card(&work, &checked).unwrap();
    record_manual_review(
        &config,
        &report.accepted[0].utterance_id,
        ReviewDecision::Pass,
        false,
    )
    .unwrap();
    fs::write(work.join("README.md"), b"# fixture\n").unwrap();
    assert!(finalize_dataset(&config).is_err());
    write_dataset_card(&work, &checked).unwrap();
    finalize_dataset(&config).unwrap();
    assert!(!work.exists());
    assert!(config.final_root.is_dir());
    let final_check = verify_dataset(
        &config.final_root,
        &config.source_root,
        &VerificationConfig::default(),
        &config.ffprobe,
    )
    .unwrap();
    assert_eq!(final_check.accepted.len(), 1);
}

#[test]
fn review_records_real_decisions_and_requires_explicit_correction() {
    let case = Case::new();
    let config = case.dataset(false);
    build_dataset(&config).unwrap();
    let work = case.config().work_root;
    assert!(review_required(&config).is_err());
    let checked = verify_dataset(
        &work,
        &config.source_root,
        &VerificationConfig::default(),
        &config.ffprobe,
    )
    .unwrap();
    write_dataset_card(&work, &checked).unwrap();
    let rows = review_required(&config).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].row.audio_path, PathBuf::from("audio/kan/kan_1.wav"));
    assert_eq!(rows[0].decision, None);
    assert!(rows[0].row.duration_seconds > 0.0);
    let id = rows[0].row.utterance_id.clone();
    assert!(record_manual_review(&config, "not-a-sample", ReviewDecision::Pass, false).is_err());
    record_manual_review(&config, &id, ReviewDecision::Fail, false).unwrap();
    assert_eq!(
        review_required(&config).unwrap()[0].decision,
        Some(ReviewDecision::Fail)
    );
    assert!(record_manual_review(&config, &id, ReviewDecision::Pass, false).is_err());
    assert!(finalize_dataset(&config).is_err());
    record_manual_review(&config, &id, ReviewDecision::Pass, true).unwrap();
    assert_eq!(
        review_required(&config).unwrap()[0].decision,
        Some(ReviewDecision::Pass)
    );
    finalize_dataset(&config).unwrap();
    assert!(record_manual_review(&config, &id, ReviewDecision::Fail, true).is_err());
}

#[test]
fn review_rejects_duplicate_unexpected_and_stale_temporary_records() {
    let case = Case::new();
    let config = case.dataset(false);
    build_dataset(&config).unwrap();
    let work = case.config().work_root;
    let checked = verify_dataset(
        &work,
        &config.source_root,
        &VerificationConfig::default(),
        &config.ffprobe,
    )
    .unwrap();
    write_dataset_card(&work, &checked).unwrap();
    let rows = review_required(&config).unwrap();
    let id = &rows[0].row.utterance_id;
    let file = case.config().work_root.join("reports/manual-review.jsonl");
    record_manual_review(&config, id, ReviewDecision::Pass, false).unwrap();
    let row = fs::read_to_string(&file).unwrap();
    fs::write(&file, format!("{row}{row}")).unwrap();
    assert!(review_required(&config).is_err());
    let mut changed: serde_json::Value = serde_json::from_str(row.trim()).unwrap();
    changed["utterance_id"] = "unexpected".into();
    fs::write(&file, format!("{changed}\n")).unwrap();
    assert!(review_required(&config).is_err());
    changed["utterance_id"] = id.as_str().into();
    changed["decision"] = "maybe".into();
    fs::write(&file, format!("{changed}\n")).unwrap();
    assert!(review_required(&config).is_err());
    fs::remove_file(&file).unwrap();
    let temp = case
        .config()
        .work_root
        .join("reports/.manual-review.jsonl.tmp");
    fs::write(&temp, b"stale").unwrap();
    assert!(record_manual_review(&config, id, ReviewDecision::Pass, false).is_err());
    assert!(!file.exists());
    fs::remove_file(&temp).unwrap();
    #[cfg(unix)]
    {
        let outside = case.0.join("outside.jsonl");
        fs::write(&outside, b"untouched").unwrap();
        std::os::unix::fs::symlink(&outside, &file).unwrap();
        assert!(record_manual_review(&config, id, ReviewDecision::Pass, false).is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"untouched");
    }
}

#[test]
fn review_pass_is_invalidated_when_verified_wav_evidence_changes() {
    let case = Case::new();
    let config = case.dataset(false);
    build_dataset(&config).unwrap();
    let work = case.config().work_root;
    let checked = verify_dataset(
        &work,
        &config.source_root,
        &VerificationConfig::default(),
        &config.ffprobe,
    )
    .unwrap();
    write_dataset_card(&work, &checked).unwrap();
    let mut row = checked.accepted[0].clone();
    record_manual_review(&config, &row.utterance_id, ReviewDecision::Pass, false).unwrap();
    let original_hash = row.output_sha256.clone();
    let wav = work.join(&row.audio_path);
    assert!(
        Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=880:duration=0.2",
                "-ac",
                "1",
                "-ar",
                "44100",
                "-c:a",
                "pcm_s16le",
                "-f",
                "wav",
                "-y"
            ])
            .arg(&wav)
            .status()
            .unwrap()
            .success()
    );
    row.output_sha256 = digest(&wav);
    row.duration_seconds = probe_audio(&config.ffprobe, &wav).unwrap().duration_seconds;
    assert_ne!(row.output_sha256, original_hash);
    fs::write(
        work.join("manifests/accepted.jsonl"),
        format!("{}\n", serde_json::to_string(&row).unwrap()),
    )
    .unwrap();
    let report_file = work.join("report.json");
    let mut report: serde_json::Value =
        serde_json::from_slice(&fs::read(&report_file).unwrap()).unwrap();
    report["duration_seconds"] = row.duration_seconds.into();
    fs::write(&report_file, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    let refreshed = verify_dataset(
        &work,
        &config.source_root,
        &VerificationConfig::default(),
        &config.ffprobe,
    )
    .unwrap();
    write_dataset_card(&work, &refreshed).unwrap();
    let stale = review_required(&config).unwrap();
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].decision, None);
    assert_eq!(serde_json::to_value(&stale[0]).unwrap()["stale"], true);
    assert!(finalize_dataset(&config).is_err());
    let review_file = work.join("reports/manual-review.jsonl");
    let old_review = fs::read(&review_file).unwrap();
    let error =
        record_manual_review(&config, &row.utterance_id, ReviewDecision::Pass, false).unwrap_err();
    assert!(error.to_string().contains("--correct"), "{error}");
    assert_eq!(fs::read(&review_file).unwrap(), old_review);
    record_manual_review(&config, &row.utterance_id, ReviewDecision::Pass, true).unwrap();
    assert_ne!(fs::read(&review_file).unwrap(), old_review);
    let current = review_required(&config).unwrap();
    assert_eq!(current[0].decision, Some(ReviewDecision::Pass));
    assert_eq!(serde_json::to_value(&current[0]).unwrap()["stale"], false);
    finalize_dataset(&config).unwrap();
    verify_dataset(
        &config.final_root,
        &config.source_root,
        &VerificationConfig::default(),
        &config.ffprobe,
    )
    .unwrap();
}

#[test]
fn finder_store_at_work_root_does_not_block_matching_resume() {
    let case = Case::new();
    let config = case.dataset(false);
    let report = build_dataset(&config).unwrap();
    let wav = report.work_root.join(&report.accepted[0].audio_path);
    let original_wav = digest(&wav);
    let original_modified = fs::metadata(&wav).unwrap().modified().unwrap();
    #[cfg(unix)]
    let original_inode = std::os::unix::fs::MetadataExt::ino(&fs::metadata(&wav).unwrap());
    let sidecar = report.work_root.join(".DS_Store");
    fs::write(&sidecar, finder_store()).unwrap();
    let resumed = build_dataset(&config).unwrap();
    assert_eq!(resumed.accepted.len(), 1);
    assert_eq!(digest(&wav), original_wav);
    assert_eq!(
        fs::metadata(&wav).unwrap().modified().unwrap(),
        original_modified
    );
    #[cfg(unix)]
    assert_eq!(
        std::os::unix::fs::MetadataExt::ino(&fs::metadata(&wav).unwrap()),
        original_inode
    );
    assert!(sidecar.is_file());
}

#[test]
fn work_verify_tolerates_finder_store_without_mutating_it() {
    let case = Case::new();
    let config = case.dataset(false);
    let report = build_dataset(&config).unwrap();
    let sidecar = report.work_root.join("audio/.DS_Store");
    fs::write(&sidecar, finder_store()).unwrap();
    let original_hash = digest(&sidecar);
    let original_modified = fs::metadata(&sidecar).unwrap().modified().unwrap();
    let checked = verify_dataset(
        &report.work_root,
        &config.source_root,
        &VerificationConfig::default(),
        &config.ffprobe,
    )
    .unwrap();
    assert_eq!(checked.accepted.len(), 1);
    assert_eq!(digest(&sidecar), original_hash);
    assert_eq!(
        fs::metadata(&sidecar).unwrap().modified().unwrap(),
        original_modified
    );
}

#[test]
fn finalize_quarantines_finder_sidecars_outside_publication_root() {
    let case = Case::new();
    let config = reviewed_fixture(&case);
    let work = case.config().work_root;
    let bytes = finder_store();
    let expected_hash = format!("{:x}", Sha256::digest(&bytes));
    for name in [".DS_Store", "audio/.DS_Store"] {
        fs::write(work.join(name), &bytes).unwrap();
    }
    finalize_dataset(&config).unwrap();
    let quarantine = config
        .final_root
        .with_file_name("KANADE-v1.finder-quarantine");
    for name in [".DS_Store", "audio/.DS_Store"] {
        assert!(!config.final_root.join(name).exists());
        assert_eq!(fs::read(quarantine.join(name)).unwrap(), bytes);
        assert_eq!(digest(&quarantine.join(name)), expected_hash);
    }
    assert!(
        config
            .final_root
            .with_file_name("KANADE-v1.finder-quarantine.json")
            .is_file()
    );
    verify_dataset(
        &config.final_root,
        &config.source_root,
        &VerificationConfig::default(),
        &config.ffprobe,
    )
    .unwrap();
    let manifest = fs::read_to_string(config.final_root.join("publication-manifest.json")).unwrap();
    assert!(!manifest.contains(".DS_Store"));
}

#[test]
fn finder_store_in_unrecognized_report_directory_fails_before_sealing() {
    let case = Case::new();
    let config = reviewed_fixture(&case);
    let work = case.config().work_root;
    fs::write(work.join("reports/.DS_Store"), finder_store()).unwrap();
    assert!(
        verify_dataset(
            &work,
            &config.source_root,
            &VerificationConfig::default(),
            &config.ffprobe
        )
        .is_err()
    );
    assert!(finalize_dataset(&config).is_err());
    assert!(!work.join("dataset-complete.json").exists());
    assert!(!work.join("publication-manifest.json").exists());
    assert!(!config.final_root.exists());
}

#[test]
fn malformed_and_oversized_finder_candidates_fail_closed() {
    let valid = finder_store();
    let mut header_only = vec![0_u8; valid.len()];
    header_only[..8].copy_from_slice(&valid[..8]);
    let cases = [
        ("header-only", header_only),
        ("truncated", valid[..valid.len() - 1].to_vec()),
        ("oversized", vec![0_u8; 1_048_577]),
    ];
    for (name, bytes) in cases {
        let case = Case::new();
        let config = reviewed_fixture(&case);
        let work = case.config().work_root;
        let wav = work.join("audio/kan/kan_1.wav");
        let original_wav = digest(&wav);
        fs::write(work.join("audio/.DS_Store"), bytes).unwrap();
        assert!(
            verify_dataset(
                &work,
                &config.source_root,
                &VerificationConfig::default(),
                &config.ffprobe
            )
            .is_err(),
            "{name}"
        );
        assert!(finalize_dataset(&config).is_err(), "{name}");
        assert!(!work.join("dataset-complete.json").exists(), "{name}");
        assert!(!config.final_root.exists(), "{name}");
        assert_eq!(digest(&wav), original_wav, "{name}");
    }
}

#[test]
fn fake_root_finder_store_does_not_enter_resume_path() {
    let case = Case::new();
    let config = case.dataset(false);
    let report = build_dataset(&config).unwrap();
    let wav = report.work_root.join(&report.accepted[0].audio_path);
    let original_wav = digest(&wav);
    let mut fake = vec![0_u8; 6_148];
    fake[..8].copy_from_slice(&finder_store()[..8]);
    fs::write(report.work_root.join(".DS_Store"), fake).unwrap();
    assert!(build_dataset(&config).is_err());
    assert_eq!(digest(&wav), original_wav);
    assert!(!config.final_root.exists());
}

#[cfg(unix)]
#[test]
fn symlink_and_fifo_finder_candidates_fail_closed() {
    for special in ["symlink", "fifo"] {
        let case = Case::new();
        let config = reviewed_fixture(&case);
        let work = case.config().work_root;
        let sidecar = work.join("audio/.DS_Store");
        if special == "symlink" {
            let outside = case.0.join("outside-store");
            fs::write(&outside, finder_store()).unwrap();
            std::os::unix::fs::symlink(outside, &sidecar).unwrap();
        } else {
            assert!(
                Command::new("mkfifo")
                    .arg(&sidecar)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        assert!(
            verify_dataset(
                &work,
                &config.source_root,
                &VerificationConfig::default(),
                &config.ffprobe
            )
            .is_err(),
            "{special}"
        );
        assert!(finalize_dataset(&config).is_err(), "{special}");
        assert!(!work.join("dataset-complete.json").exists(), "{special}");
    }
}

#[test]
fn unknown_file_and_occupied_quarantine_never_get_sealed() {
    let extra = Case::new();
    let extra_config = reviewed_fixture(&extra);
    let extra_work = extra.config().work_root;
    fs::write(extra_work.join("unexpected.bin"), b"foreign").unwrap();
    assert!(
        verify_dataset(
            &extra_work,
            &extra_config.source_root,
            &VerificationConfig::default(),
            &extra_config.ffprobe
        )
        .is_err()
    );
    assert!(finalize_dataset(&extra_config).is_err());
    assert!(!extra_work.join("dataset-complete.json").exists());

    let occupied = Case::new();
    let occupied_config = reviewed_fixture(&occupied);
    let work = occupied.config().work_root;
    fs::write(work.join(".DS_Store"), finder_store()).unwrap();
    let quarantine = occupied_config
        .final_root
        .with_file_name("KANADE-v1.finder-quarantine");
    fs::create_dir(&quarantine).unwrap();
    fs::write(quarantine.join(".DS_Store"), finder_store()).unwrap();
    assert!(finalize_dataset(&occupied_config).is_err());
    assert!(work.join(".DS_Store").is_file());
    assert!(!work.join("dataset-complete.json").exists());
    assert!(!occupied_config.final_root.exists());
}

#[test]
fn sealed_work_and_final_root_reject_new_finder_sidecar() {
    let case = Case::new();
    let config = reviewed_fixture(&case);
    finalize_dataset(&config).unwrap();
    let final_sidecar = config.final_root.join(".DS_Store");
    fs::write(&final_sidecar, finder_store()).unwrap();
    assert!(
        verify_dataset(
            &config.final_root,
            &config.source_root,
            &VerificationConfig::default(),
            &config.ffprobe
        )
        .is_err()
    );
    fs::remove_file(&final_sidecar).unwrap();
    let work = case.config().work_root;
    fs::rename(&config.final_root, &work).unwrap();
    fs::write(work.join("audio/.DS_Store"), finder_store()).unwrap();
    assert!(
        verify_dataset(
            &work,
            &config.source_root,
            &VerificationConfig::default(),
            &config.ffprobe
        )
        .is_err()
    );
    assert!(finalize_dataset(&config).is_err());
    assert!(!config.final_root.exists());
}

#[test]
fn interrupted_quarantine_resumes_and_recreated_sidecar_fails_once() {
    let case = Case::new();
    let config = reviewed_fixture(&case);
    let work = case.config().work_root;
    let bytes = finder_store();
    fs::write(work.join(".DS_Store"), &bytes).unwrap();
    fs::write(work.join("audio/.DS_Store"), &bytes).unwrap();
    let stale_seal_temp = work.join(".dataset-complete.json.tmp");
    fs::write(&stale_seal_temp, b"foreign seal bytes").unwrap();
    assert!(finalize_dataset(&config).is_err());
    let quarantine = config
        .final_root
        .with_file_name("KANADE-v1.finder-quarantine");
    assert_eq!(fs::read(quarantine.join(".DS_Store")).unwrap(), bytes);
    assert_eq!(fs::read(quarantine.join("audio/.DS_Store")).unwrap(), bytes);
    assert!(!work.join("dataset-complete.json").exists());
    assert!(!config.final_root.exists());

    fs::remove_file(&stale_seal_temp).unwrap();
    fs::write(work.join(".DS_Store"), &bytes).unwrap();
    let error = finalize_dataset(&config).unwrap_err();
    assert!(error.to_string().contains("Finder sidecar"), "{error}");
    assert!(!work.join("dataset-complete.json").exists());
    fs::remove_file(work.join(".DS_Store")).unwrap();
    finalize_dataset(&config).unwrap();
    assert!(!work.exists());
    assert!(config.final_root.is_dir());
    verify_dataset(
        &config.final_root,
        &config.source_root,
        &VerificationConfig::default(),
        &config.ffprobe,
    )
    .unwrap();
}

#[test]
fn changed_finder_store_after_quarantine_plan_is_not_moved() {
    let case = Case::new();
    let config = reviewed_fixture(&case);
    let work = case.config().work_root;
    let sidecar = work.join(".DS_Store");
    let original = finder_store();
    fs::write(&sidecar, &original).unwrap();
    let checked = verify_dataset(
        &work,
        &config.source_root,
        &VerificationConfig::default(),
        &config.ffprobe,
    )
    .unwrap();
    let plan_path = config
        .final_root
        .with_file_name("KANADE-v1.finder-quarantine.json");
    let plan = serde_json::json!({
        "schema_version": "kanade-finder-quarantine-v1",
        "source_config_fingerprint": checked.fingerprint,
        "files": {".DS_Store": {"size": original.len(), "sha256": digest(&sidecar)}}
    });
    fs::write(&plan_path, serde_json::to_vec_pretty(&plan).unwrap()).unwrap();
    let mut changed = original.clone();
    changed[20] ^= 1;
    fs::write(&sidecar, &changed).unwrap();
    verify_dataset(
        &work,
        &config.source_root,
        &VerificationConfig::default(),
        &config.ffprobe,
    )
    .unwrap();
    let error = finalize_dataset(&config).unwrap_err();
    assert!(
        error.to_string().contains("changed before quarantine"),
        "{error}"
    );
    assert_eq!(fs::read(&sidecar).unwrap(), changed);
    assert!(!work.join("dataset-complete.json").exists());
    assert!(!config.final_root.exists());
}

#[test]
fn occupied_planned_quarantine_destination_is_not_overwritten() {
    let case = Case::new();
    let config = reviewed_fixture(&case);
    let work = case.config().work_root;
    let sidecar = work.join(".DS_Store");
    let bytes = finder_store();
    fs::write(&sidecar, &bytes).unwrap();
    let checked = verify_dataset(
        &work,
        &config.source_root,
        &VerificationConfig::default(),
        &config.ffprobe,
    )
    .unwrap();
    let plan_path = config
        .final_root
        .with_file_name("KANADE-v1.finder-quarantine.json");
    let plan = serde_json::json!({
        "schema_version": "kanade-finder-quarantine-v1",
        "source_config_fingerprint": checked.fingerprint,
        "files": {".DS_Store": {"size": bytes.len(), "sha256": digest(&sidecar)}}
    });
    fs::write(&plan_path, serde_json::to_vec_pretty(&plan).unwrap()).unwrap();
    let quarantine = config
        .final_root
        .with_file_name("KANADE-v1.finder-quarantine");
    fs::create_dir(&quarantine).unwrap();
    fs::write(quarantine.join(".DS_Store"), b"occupied").unwrap();
    assert!(finalize_dataset(&config).is_err());
    assert_eq!(fs::read(quarantine.join(".DS_Store")).unwrap(), b"occupied");
    assert_eq!(fs::read(&sidecar).unwrap(), bytes);
    assert!(!work.join("dataset-complete.json").exists());
}

#[test]
fn existing_quarantine_plan_with_stale_temporary_is_ambiguous() {
    let case = Case::new();
    let config = reviewed_fixture(&case);
    let work = case.config().work_root;
    fs::write(work.join(".DS_Store"), finder_store()).unwrap();
    let stale_seal_temp = work.join(".dataset-complete.json.tmp");
    fs::write(&stale_seal_temp, b"foreign seal bytes").unwrap();
    assert!(finalize_dataset(&config).is_err());
    fs::remove_file(&stale_seal_temp).unwrap();

    let plan_temp = config
        .final_root
        .with_file_name(".KANADE-v1.finder-quarantine.json.tmp");
    fs::write(&plan_temp, b"stale plan bytes").unwrap();
    let error = finalize_dataset(&config).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("ambiguous Finder quarantine plan")
    );
    assert_eq!(fs::read(&plan_temp).unwrap(), b"stale plan bytes");
    assert!(!config.final_root.exists());
}

#[test]
fn normal_build_rejects_fixture_source_before_creating_work_root() {
    let case = Case::new();
    let config = case.dataset(false);
    let error = kanade_tts::build::build_dataset(&config).unwrap_err();
    assert!(error.to_string().contains("default.tjs"), "{error}");
    assert!(!case.config().work_root.exists());
}

#[test]
fn normal_verify_and_finalize_reject_fixture_source_even_after_fixture_build() {
    let case = Case::new();
    let config = case.dataset(false);
    let report = build_dataset(&config).unwrap();
    let work = case.config().work_root;
    let verify_error = kanade_tts::verify::verify_dataset(
        &work,
        &config.source_root,
        &VerificationConfig::default(),
        &config.ffprobe,
    )
    .unwrap_err();
    assert!(verify_error.to_string().contains("default.tjs"));
    fs::write(work.join("README.md"), b"# fixture\n").unwrap();
    fs::write(
        work.join("reports/manual-review.jsonl"),
        format!(
            "{}\n",
            serde_json::json!({"utterance_id": report.accepted[0].utterance_id, "decision": "pass"})
        ),
    )
    .unwrap();
    let finalize_error = kanade_tts::build::finalize_dataset(&config).unwrap_err();
    assert!(finalize_error.to_string().contains("default.tjs"));
    assert!(work.is_dir());
    assert!(!config.final_root.exists());
}

#[test]
fn one_multi_voice_event_can_account_for_two_rejected_oggs() {
    let case = Case::new();
    let config = case.dataset(true);
    let report = build_dataset(&config).unwrap();
    assert_eq!(report.accepted.len(), 0);
    assert_eq!(report.rejected.len(), 2);
    let verified = verify_dataset(
        &case.config().work_root,
        &config.source_root,
        &VerificationConfig::default(),
        &config.ffprobe,
    )
    .unwrap();
    assert_eq!(verified.rejected_count, 2);
}

#[test]
fn duplicate_accepted_source_event_key_still_fails_closed() {
    let case = Case::new();
    let config = case.dataset(false);
    build_dataset(&config).unwrap();
    let manifest = case.config().work_root.join("manifests/accepted.jsonl");
    let row = fs::read_to_string(&manifest).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(row.trim()).unwrap();
    let duplicate = value["source_events"][0].clone();
    value["source_events"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    fs::write(&manifest, format!("{}\n", value)).unwrap();
    let error = verify_dataset(
        &case.config().work_root,
        &config.source_root,
        &VerificationConfig::default(),
        &config.ffprobe,
    )
    .unwrap_err();
    assert!(error.to_string().contains("duplicate SourceEventRef"));
}

#[test]
fn pcm_in_caf_container_is_not_reused_as_a_wav() {
    let case = Case::new();
    let source = case.source(2);
    let config = case.config();
    let item = matched(&source);
    let row = convert_one(&config, &item).unwrap();
    let output = config.work_root.join(&row.audio_path);
    let status = Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(&source)
        .args([
            "-ac",
            "1",
            "-ar",
            "44100",
            "-c:a",
            "pcm_s16le",
            "-f",
            "caf",
            "-y",
        ])
        .arg(&output)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(&fs::read(&output).unwrap()[..4], b"caff");
    let state = output.with_extension("wav.state.json");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&state).unwrap()).unwrap();
    value["output_sha256"] = digest(&output).into();
    fs::write(state, serde_json::to_vec(&value).unwrap()).unwrap();
    let repaired = convert_one(&config, &item).unwrap();
    assert_eq!(repaired.output_sha256, digest(&output));
    assert_eq!(&fs::read(&output).unwrap()[..4], b"RIFF");
}

#[test]
fn failed_state_write_cleans_only_its_own_temp() {
    let case = Case::new();
    let source = case.source(2);
    let config = case.config();
    let output = config.work_root.join("audio/kan/kan_1.wav");
    fs::create_dir_all(output.parent().unwrap()).unwrap();
    let foreign_state = output.with_extension("wav.state.json");
    fs::create_dir(&foreign_state).unwrap();
    assert!(convert_one(&config, &matched(&source)).is_err());
    assert!(foreign_state.is_dir());
    assert!(!foreign_state.with_extension("json.tmp").exists());
    let other = Case::new();
    let other_source = other.source(2);
    let other_config = other.config();
    let other_output = other_config.work_root.join("audio/kan/kan_1.wav");
    fs::create_dir_all(other_output.parent().unwrap()).unwrap();
    let foreign_temp = other_output
        .with_extension("wav.state.json")
        .with_extension("json.tmp");
    fs::write(&foreign_temp, b"foreign").unwrap();
    assert!(convert_one(&other_config, &matched(&other_source)).is_err());
    assert_eq!(fs::read(&foreign_temp).unwrap(), b"foreign");
}

#[cfg(unix)]
#[test]
fn scenario_root_symlink_is_rejected_before_build_output() {
    let case = Case::new();
    let config = case.dataset(false);
    let original = config.source_root.join("scn");
    let outside = case.0.join("outside-scn");
    fs::rename(&original, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, &original).unwrap();
    assert!(build_dataset(&config).is_err());
    assert!(!case.config().work_root.exists());
}

#[cfg(unix)]
#[test]
fn scenario_psb_escape_and_special_root_are_rejected() {
    let escaped = Case::new();
    let config = escaped.dataset(false);
    let psb = config.source_root.join("scn/pilot.ks.scn");
    let outside = escaped.0.join("outside.ks.scn");
    fs::rename(&psb, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, &psb).unwrap();
    assert!(build_dataset(&config).is_err());
    assert!(!escaped.config().work_root.exists());

    let special = Case::new();
    let special_config = special.dataset(false);
    fs::remove_dir_all(special_config.source_root.join("scn")).unwrap();
    fs::write(special_config.source_root.join("scn"), b"not a directory").unwrap();
    assert!(build_dataset(&special_config).is_err());
    assert!(!special.config().work_root.exists());
}

#[test]
fn independent_verifier_rejects_changed_join_summary() {
    let case = Case::new();
    let config = case.dataset(false);
    build_dataset(&config).unwrap();
    let summary = case.config().work_root.join("reports/join-summary.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&summary).unwrap()).unwrap();
    value["physical_audio_count"] = 999.into();
    fs::write(&summary, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(
        verify_dataset(
            &case.config().work_root,
            &config.source_root,
            &VerificationConfig::default(),
            &config.ffprobe
        )
        .is_err()
    );
}

#[test]
fn report_carries_and_verifier_checks_tool_version() {
    let case = Case::new();
    let config = case.dataset(false);
    build_dataset(&config).unwrap();
    let report_path = case.config().work_root.join("report.json");
    let mut report: serde_json::Value =
        serde_json::from_slice(&fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report["tool_version"], env!("CARGO_PKG_VERSION"));
    report["tool_version"] = "wrong".into();
    fs::write(&report_path, serde_json::to_vec(&report).unwrap()).unwrap();
    assert!(
        verify_dataset(
            &case.config().work_root,
            &config.source_root,
            &VerificationConfig::default(),
            &config.ffprobe
        )
        .is_err()
    );
}

fn reviewed_fixture(case: &Case) -> BuildConfig {
    let config = case.dataset(false);
    let report = build_dataset(&config).unwrap();
    let work = case.config().work_root;
    let checked = verify_dataset(
        &work,
        &config.source_root,
        &VerificationConfig::default(),
        &config.ffprobe,
    )
    .unwrap();
    write_dataset_card(&work, &checked).unwrap();
    record_manual_review(
        &config,
        &report.accepted[0].utterance_id,
        ReviewDecision::Pass,
        false,
    )
    .unwrap();
    config
}

#[test]
fn interrupted_finalization_phases_resume_without_rebuilding() {
    for phase in [
        "state-cleaned",
        "partly-cleaned",
        "cleaned",
        "completion-only",
        "completion-temp",
        "publication-temp",
        "sealed",
    ] {
        let case = Case::new();
        let config = reviewed_fixture(&case);
        let work = case.config().work_root;
        if phase == "state-cleaned" {
            let state = work.join("audio/kan/kan_1.wav.state.json");
            fs::remove_file(state).unwrap();
        } else if phase == "partly-cleaned" {
            fs::remove_file(work.join(".source-config-fingerprint.json")).unwrap();
        } else {
            finalize_dataset(&config).unwrap();
            fs::rename(&config.final_root, &work).unwrap();
            if phase == "cleaned" {
                fs::remove_file(work.join("dataset-complete.json")).unwrap();
                fs::remove_file(work.join("publication-manifest.json")).unwrap();
            } else if phase == "completion-only" {
                fs::remove_file(work.join("publication-manifest.json")).unwrap();
            } else if phase == "completion-temp" {
                fs::remove_file(work.join("publication-manifest.json")).unwrap();
                fs::rename(
                    work.join("dataset-complete.json"),
                    work.join(".dataset-complete.json.tmp"),
                )
                .unwrap();
            } else if phase == "publication-temp" {
                fs::rename(
                    work.join("publication-manifest.json"),
                    work.join(".publication-manifest.json.tmp"),
                )
                .unwrap();
            }
        }
        assert!(!config.final_root.exists());
        let accepted_before = digest(&work.join("manifests/accepted.jsonl"));
        finalize_dataset(&config).unwrap_or_else(|error| panic!("phase {phase}: {error}"));
        assert_eq!(
            digest(&config.final_root.join("manifests/accepted.jsonl")),
            accepted_before
        );
        verify_dataset(
            &config.final_root,
            &config.source_root,
            &VerificationConfig::default(),
            &config.ffprobe,
        )
        .unwrap();
    }
}

#[test]
fn sealed_retry_does_not_rewrite_hashed_content() {
    let case = Case::new();
    let config = reviewed_fixture(&case);
    finalize_dataset(&config).unwrap();
    let work = case.config().work_root;
    fs::rename(&config.final_root, &work).unwrap();
    let completion = digest(&work.join("dataset-complete.json"));
    let publication = digest(&work.join("publication-manifest.json"));
    finalize_dataset(&config).unwrap();
    assert_eq!(
        digest(&config.final_root.join("dataset-complete.json")),
        completion
    );
    assert_eq!(
        digest(&config.final_root.join("publication-manifest.json")),
        publication
    );
}
