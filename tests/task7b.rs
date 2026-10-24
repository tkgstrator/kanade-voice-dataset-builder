use clap::Parser;
use kanade_tts::build::{BuildConfig, build_dataset, finalize_dataset};
use kanade_tts::cli::{Cli, Command};
use kanade_tts::readme::write_dataset_card;
use kanade_tts::readme::{render_dataset_card, size_category};
use kanade_tts::verify::VerificationReport;
use kanade_tts::verify::{VerificationConfig, verify_dataset};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as Process;

#[test]
fn card_front_matter_and_sections_derive_from_verified_count() {
    let report = VerificationReport {
        fingerprint: "pinned".into(),
        accepted: vec![],
        rejected_count: 49,
        orphan_audio_count: 164,
        physical_audio_count: 2809,
        duration_seconds: 12.5,
    };
    let card = render_dataset_card(&report, include_str!("../templates/README.md.tmpl")).unwrap();
    let yaml = card
        .strip_prefix("---\n")
        .unwrap()
        .split("\n---\n")
        .next()
        .unwrap();
    let meta: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(meta["pretty_name"], "KANADE Japanese Voice Dataset");
    assert_eq!(meta["license"], "other");
    assert_eq!(meta["language"][0], "ja");
    assert_eq!(meta["size_categories"][0], "n<1K");
    assert_eq!(size_category(2596), "1K<n<10K");
    for category in ["automatic-speech-recognition", "text-to-speech"] {
        assert!(
            meta["task_categories"]
                .as_sequence()
                .unwrap()
                .iter()
                .any(|v| v == category)
        );
    }
    for tag in ["audio", "voice", "speech", "KANADE"] {
        assert!(meta["tags"].as_sequence().unwrap().iter().any(|v| v == tag));
    }
    for section in [
        "Dataset Summary",
        "Directory Layout",
        "Loading",
        "Schema",
        "Intended Use",
        "Data Source & Extraction",
        "Limitations",
        "License & Copyright",
        "Citation",
    ] {
        assert!(card.contains(&format!("## {section}")), "missing {section}");
    }
    assert!(card.contains("©Frontwing / GOOD SMILE COMPANY"));
    assert!(card.contains("https://kanade.frontwing.co.jp/"));
    assert!(card.contains("https://www.nintendo.com/us/store/products/kanade-switch/"));
    assert!(card.contains("ultemica/kanade-voices"));
    assert!(card.contains("token=True"));
    assert!(card.contains("load_dataset(\"audiofolder\", data_dir=\"KANADE-v1\""));
    assert!(card.contains("sample[\"audio\"]"));
    assert!(!card.contains("sample[\"file_name\"]"));
    assert!(card.contains("audio/<speaker_id>/<voice_id>.wav"));
    assert!(!card.contains("audio/<speaker_id>/<utterance_id>.wav"));
    assert!(card.contains("| Rejected records | 49 |"));
    assert!(!card.contains("Rejected OGGs"));
    assert!(card.contains("Japanese audio only"));
    assert!(!card.contains(env!("CARGO_MANIFEST_DIR")));
    assert!(!card.contains("dev.keys"));
}

#[test]
fn cli_exposes_production_stages_and_rejects_inert_options() {
    let build = Cli::try_parse_from([
        "kanade-tts",
        "build",
        "--source",
        "/s",
        "--output",
        "/o/KANADE-v1",
        "--ffmpeg",
        "ffmpeg",
        "--ffprobe",
        "ffprobe",
    ])
    .unwrap();
    assert!(matches!(build.command, Command::Build { .. }));
    let verify = Cli::try_parse_from([
        "kanade-tts",
        "verify",
        "--source",
        "/s",
        "--dataset",
        "/o/KANADE-v1.work",
        "--ffprobe",
        "ffprobe",
    ])
    .unwrap();
    assert!(matches!(verify.command, Command::Verify { .. }));
    let review = Cli::try_parse_from([
        "kanade-tts",
        "review",
        "--source",
        "/s",
        "--output",
        "/o/KANADE-v1",
        "--ffprobe",
        "ffprobe",
        "--utterance-id",
        "kan_1",
        "--decision",
        "pass",
    ])
    .unwrap();
    assert!(matches!(review.command, Command::Review { .. }));
    assert!(
        Cli::try_parse_from([
            "kanade-tts",
            "build",
            "--source",
            "/s",
            "--output",
            "/o/KANADE-v1",
            "--ffmpeg",
            "ffmpeg",
            "--ffprobe",
            "ffprobe",
            "--dry-run"
        ])
        .is_err()
    );
    assert!(
        Cli::try_parse_from([
            "kanade-tts",
            "verify",
            "--source",
            "/s",
            "--output",
            "/o/KANADE-v1",
            "--ffprobe",
            "ffprobe",
            "--config",
            "/x"
        ])
        .is_err()
    );
    let help = Process::new(env!("CARGO_BIN_EXE_kanade-tts"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    let text = String::from_utf8(help.stdout).unwrap();
    for stage in ["build", "review", "verify", "finalize"] {
        assert!(text.contains(stage));
    }
    let invalid = Process::new(env!("CARGO_BIN_EXE_kanade-tts"))
        .args([
            "build",
            "--source",
            "/s",
            "--output",
            "/o/KANADE-v1",
            "--ffmpeg",
            "ffmpeg",
            "--ffprobe",
            "ffprobe",
            "--dry-run",
        ])
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
}

#[test]
#[ignore = "requires KANADE_SOURCE_ROOT pointing to lawfully extracted romfs/data"]
fn public_and_cli_verify_marked_work_sealed_work_and_final_root() {
    let root = std::env::temp_dir().join(format!("kanade-public-verify-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let source = root.join("source");
    let real = std::env::var_os("KANADE_SOURCE_ROOT")
        .map(PathBuf::from)
        .expect("KANADE_SOURCE_ROOT must point to lawfully extracted romfs/data");
    for name in [
        "main/default.tjs",
        "system/StorageData.tjs",
        "system/MainWindow.tjs",
        "sysscn/multilang.tjs",
    ] {
        let target = source.join(name);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(real.join(name), target).unwrap();
    }
    let scenarios = source.join("scn");
    fs::create_dir_all(&scenarios).unwrap();
    for entry in fs::read_dir(real.join("scn")).unwrap() {
        let entry = entry.unwrap();
        if entry.path().extension().is_some_and(|ext| ext == "scn") {
            fs::copy(entry.path(), scenarios.join(entry.file_name())).unwrap();
        }
    }
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
    let config = BuildConfig {
        source_root: source.clone(),
        final_root: root.join("KANADE-v1"),
        ffmpeg: PathBuf::from("ffmpeg"),
        ffprobe: PathBuf::from("ffprobe"),
    };
    let built = build_dataset(&config).unwrap();
    assert!(
        built
            .work_root
            .join(".source-config-fingerprint.json")
            .is_file()
    );
    let verified = verify_dataset(
        &built.work_root,
        &source,
        &VerificationConfig::default(),
        &config.ffprobe,
    )
    .unwrap();
    assert_eq!(verified.physical_audio_count, 0);
    write_dataset_card(&built.work_root, &verified).unwrap();
    let cli = |dataset: &Path| {
        Process::new(env!("CARGO_BIN_EXE_kanade-tts"))
            .args(["verify", "--source"])
            .arg(&source)
            .arg("--dataset")
            .arg(dataset)
            .args(["--ffprobe", "ffprobe"])
            .output()
            .unwrap()
    };
    let output = cli(&built.work_root);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::write(built.work_root.join("reports/manual-review.jsonl"), b"").unwrap();
    finalize_dataset(&config).unwrap();
    let sealed_work = root.join("KANADE-v1.work");
    fs::rename(&config.final_root, &sealed_work).unwrap();
    assert!(cli(&sealed_work).status.success());
    finalize_dataset(&config).unwrap();
    assert!(cli(&config.final_root).status.success());
    fs::remove_dir_all(root).unwrap();
}
