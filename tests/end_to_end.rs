use emote_psb::{psb::write::PsbWriter, value::PsbValue};
use kanade_tts::audio::sha256_file;
use kanade_tts::build::{
    BuildConfig, build_fixture_dataset_for_tests, finalize_fixture_dataset_for_tests,
};
use kanade_tts::readme::write_dataset_card;
use kanade_tts::review::{
    ReviewDecision, record_fixture_manual_review_for_tests, review_fixture_required_for_tests,
};
use kanade_tts::verify::{VerificationConfig, verify_fixture_dataset_for_tests};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use walkdir::WalkDir;

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "kanade-e2e-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn source(&self) -> PathBuf {
        let source = self.0.join("source");
        let ogg = source.join("voice/カナデ/KAN_1.ogg");
        fs::create_dir_all(ogg.parent().unwrap()).unwrap();
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
                    "sine=frequency=440:duration=0.2",
                    "-ac",
                    "2",
                    "-c:a",
                    "vorbis",
                    "-strict",
                    "-2",
                    "-y"
                ])
                .arg(&ogg)
                .status()
                .unwrap()
                .success()
        );
        fs::copy(&ogg, ogg.with_file_name("KAN_2.ogg")).unwrap();
        let aliases: serde_json::Value =
            serde_json::from_str(include_str!("../config/speaker-aliases.json")).unwrap();
        for row in aliases["aliases"].as_array().unwrap() {
            fs::create_dir_all(
                source
                    .join("voice")
                    .join(row["source_speaker"].as_str().unwrap()),
            )
            .unwrap();
        }
        let scn = source.join("scn/pilot.ks.scn");
        fs::create_dir_all(scn.parent().unwrap()).unwrap();
        PsbWriter::new(3, false, &fixture_psb(), fs::File::create(scn).unwrap())
            .unwrap()
            .finish()
            .unwrap();
        source
    }
    fn config(&self, source: &Path, name: &str) -> BuildConfig {
        let parent = self.0.join(name);
        fs::create_dir(&parent).unwrap();
        BuildConfig {
            source_root: source.to_owned(),
            final_root: parent.join("KANADE-v1"),
            ffmpeg: "ffmpeg".into(),
            ffprobe: "ffprobe".into(),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn string(s: &str) -> PsbValue {
    PsbValue::String(s.into())
}
fn number(n: i64) -> PsbValue {
    PsbValue::Number(n.into())
}
fn list(v: Vec<PsbValue>) -> PsbValue {
    PsbValue::List(v)
}
fn object(v: Vec<(&str, PsbValue)>) -> PsbValue {
    PsbValue::Object(v.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
fn fixture_psb() -> PsbValue {
    let language = |speaker: &str, text: &str| list(vec![string(speaker), string(text), number(0)]);
    let event = list(vec![
        string("カナデ"),
        list(vec![
            language("少女", "「こんにちは」"),
            language("Girl", "Hello"),
            language("少女", "你好"),
            language("少女", "你好啊"),
        ]),
        list(vec![object(vec![
            ("name", string("カナデ")),
            ("pan", number(0)),
            ("type", number(0)),
            ("voice", string("KAN_1")),
        ])]),
        number(0),
        object(vec![("data", list(vec![])), ("env", object(vec![]))]),
    ]);
    let marker = list(vec![
        number(0),
        object(vec![("data", list(vec![])), ("env", object(vec![]))]),
        PsbValue::Null,
        number(0),
        number(0),
    ]);
    let lines = std::iter::once(marker.clone())
        .chain(std::iter::once(number(0)))
        .chain((0..61).map(|_| list(vec![string("control")])))
        .chain(std::iter::once(marker))
        .collect();
    let maps = ["en", "cn", "tw"]
        .iter()
        .map(|name| {
            object(vec![
                ("name", string(&format!("pilot_{name}.ks"))),
                ("*start", list(vec![number(0)])),
            ])
        })
        .collect();
    object(vec![
        ("name", string("pilot.ks")),
        (
            "languages",
            list(["en", "cn", "tw"].iter().map(|s| string(s)).collect()),
        ),
        ("llmap", list(maps)),
        (
            "scenes",
            list(vec![object(vec![
                ("files", list(vec![])),
                ("firstLine", number(0)),
                ("label", string("*start")),
                ("lines", list(lines)),
                (
                    "nexts",
                    list(vec![object(vec![
                        ("type", number(0)),
                        ("storage", string("next.ks")),
                        ("target", string("*start")),
                    ])]),
                ),
                ("spCount", number(1)),
                ("texts", list(vec![event])),
                ("title", list(vec![string(""); 4])),
                ("version", number(1)),
            ])]),
        ),
    ])
}

#[test]
fn two_outputs_are_deterministic_and_review_gates_finalization() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let source_hashes = WalkDir::new(&source)
        .into_iter()
        .map(Result::unwrap)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| {
            let path = entry.path().to_owned();
            let digest = sha256_file(&path).unwrap();
            let mut permissions = fs::metadata(&path).unwrap().permissions();
            permissions.set_readonly(true);
            fs::set_permissions(&path, permissions).unwrap();
            (path, digest)
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let source_oggs = WalkDir::new(source.join("voice"))
        .into_iter()
        .map(Result::unwrap)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "ogg"))
        .count();
    assert_eq!(source_oggs, 2);
    let left = fixture.config(&source, "left");
    let right = fixture.config(&source, "right");
    let built_left = build_fixture_dataset_for_tests(&left).unwrap();
    let built_right = build_fixture_dataset_for_tests(&right).unwrap();
    for (config, built) in [(&left, &built_left), (&right, &built_right)] {
        let checked = verify_fixture_dataset_for_tests(
            &built.work_root,
            &source,
            &VerificationConfig::default(),
            &config.ffprobe,
        )
        .unwrap();
        assert_eq!(
            (
                checked.accepted.len(),
                checked.orphan_audio_count,
                checked.physical_audio_count
            ),
            (1, 1, source_oggs)
        );
        for row in &checked.accepted {
            assert!(built.work_root.join(&row.audio_path).is_file());
            assert_eq!(row.spoken_text, "こんにちは");
            assert_eq!(row.display_text, "こんにちは");
            assert!(source.join("voice").join(&row.source_ogg).is_file());
        }
        let mut metadata = csv::Reader::from_path(built.work_root.join("metadata.csv")).unwrap();
        let headers = metadata.headers().unwrap().clone();
        let row = metadata.records().next().unwrap().unwrap();
        let value = |name: &str| row.get(headers.iter().position(|header| header == name).unwrap());
        assert_eq!(value("transcription"), Some("こんにちは"));
        assert_eq!(value("display_text"), Some("こんにちは"));
        write_dataset_card(&built.work_root, &checked).unwrap();
        assert!(finalize_fixture_dataset_for_tests(config).is_err());
    }
    let intended = [
        "README.md",
        "metadata.csv",
        "speakers.json",
        "report.json",
        "manifests/accepted.jsonl",
        "reports/rejected.jsonl",
        "reports/rejected.csv",
        "reports/orphan-audio.jsonl",
        "reports/missing-audio.jsonl",
        "reports/join-summary.json",
        "audio/kan/kan_1.wav",
    ];
    for name in intended {
        assert_eq!(
            fs::read(built_left.work_root.join(name)).unwrap(),
            fs::read(built_right.work_root.join(name)).unwrap(),
            "different {name}"
        );
    }
    build_fixture_dataset_for_tests(&left).unwrap();
    let rows = review_fixture_required_for_tests(&left).unwrap();
    assert_eq!(rows.len(), 1);
    let id = &rows[0].row.utterance_id;
    record_fixture_manual_review_for_tests(&left, id, ReviewDecision::Fail, false).unwrap();
    assert!(finalize_fixture_dataset_for_tests(&left).is_err());
    record_fixture_manual_review_for_tests(&left, id, ReviewDecision::Pass, true).unwrap();
    record_fixture_manual_review_for_tests(&right, id, ReviewDecision::Pass, false).unwrap();
    finalize_fixture_dataset_for_tests(&left).unwrap();
    finalize_fixture_dataset_for_tests(&right).unwrap();
    assert!(
        verify_fixture_dataset_for_tests(
            &left.final_root,
            &source,
            &VerificationConfig::default(),
            &left.ffprobe
        )
        .is_ok()
    );
    let left_files = WalkDir::new(&left.final_root)
        .into_iter()
        .map(Result::unwrap)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| {
            entry
                .path()
                .strip_prefix(&left.final_root)
                .unwrap()
                .to_path_buf()
        })
        .collect::<std::collections::BTreeSet<_>>();
    let right_files = WalkDir::new(&right.final_root)
        .into_iter()
        .map(Result::unwrap)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| {
            entry
                .path()
                .strip_prefix(&right.final_root)
                .unwrap()
                .to_path_buf()
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(left_files, right_files);
    for name in left_files {
        assert_eq!(
            fs::read(left.final_root.join(&name)).unwrap(),
            fs::read(right.final_root.join(&name)).unwrap(),
            "different finalized {}",
            name.display()
        );
    }
    for (path, digest) in source_hashes {
        assert_eq!(
            sha256_file(&path).unwrap(),
            digest,
            "source changed: {}",
            path.display()
        );
    }
    assert!(build_fixture_dataset_for_tests(&left).is_err());
    assert!(review_fixture_required_for_tests(&left).is_err());
    assert!(finalize_fixture_dataset_for_tests(&left).is_err());
}
