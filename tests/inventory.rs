use clap::Parser;
use kanade_tts::cli::{Cli, Command};
use kanade_tts::inventory::{inventory_voice_root, normalize_voice_id};
#[cfg(unix)]
use kanade_tts::probe::probe_audio;
use kanade_tts::speakers::build_speaker_catalog;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_CASE: AtomicU64 = AtomicU64::new(0);

#[test]
fn inspect_cli_accepts_explicit_ffprobe_path() {
    let cli = Cli::try_parse_from([
        "kanade-tts",
        "inspect",
        "--source",
        "data",
        "--ffprobe",
        "/path/ffprobe",
    ])
    .unwrap();
    assert!(matches!(
        cli.command,
        Command::Inspect { source, ffprobe }
            if source == Path::new("data") && ffprobe == Path::new("/path/ffprobe")
    ));
}

struct CaseDir(PathBuf);

impl CaseDir {
    fn new() -> Self {
        let serial = NEXT_CASE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "kanade-tts-inventory-{}-{serial}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn join(&self, path: impl AsRef<Path>) -> PathBuf {
        self.0.join(path)
    }

    fn touch(&self, path: &str) {
        let path = self.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"audio fixture").unwrap();
    }
}

impl Drop for CaseDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn normalizes_nfc_and_ascii_case_only() {
    assert_eq!(normalize_voice_id("KAN_Cafe\u{301}_Ａ"), "kan_café_Ａ");
    assert_eq!(normalize_voice_id("ÄBC"), "Äbc");
}

#[test]
fn inventory_excludes_sidecars_and_preserves_nested_relative_paths() {
    let case = CaseDir::new();
    case.touch("カナデ/nested/KAN_002.ogg");
    case.touch("カナデ/KAN_001.ogg");
    case.touch("カナデ/KAN_001.ogg.sli");
    case.touch("カナデ/readme.txt");
    let inventory = inventory_voice_root(&case.0).unwrap();
    assert_eq!(inventory.assets.len(), 2);
    assert_eq!(inventory.sidecars.len(), 1);
    assert_eq!(inventory.speakers, vec!["カナデ"]);
    assert_eq!(
        inventory.assets[0].source_ogg,
        PathBuf::from("カナデ/KAN_001.ogg")
    );
    assert_eq!(inventory.assets[0].voice_id, "kan_001");
    assert_eq!(
        inventory.assets[1].source_ogg,
        PathBuf::from("カナデ/nested/KAN_002.ogg")
    );
}

#[test]
fn nfc_and_nfd_speaker_names_are_rejected_as_a_collision() {
    let case = CaseDir::new();
    case.touch("ガ/A_001.ogg");
    case.touch("カ\u{3099}/B_001.ogg");
    let count = std::fs::read_dir(&case.0).unwrap().count();
    let result = inventory_voice_root(&case.0);
    if count == 2 {
        assert!(result.is_err());
    } else {
        // Some macOS volumes resolve canonically equivalent directory names to one entry.
        assert_eq!(result.unwrap().assets[0].source_speaker, "ガ");
    }
}

#[test]
fn nfd_speaker_name_uses_its_dominant_prefix_in_catalog() {
    let case = CaseDir::new();
    case.touch("カ\u{3099}/ABC_001.ogg");
    let inventory = inventory_voice_root(&case.0).unwrap();
    let catalog = build_speaker_catalog(&inventory).unwrap();
    assert_eq!(catalog.speakers["ガ"], "abc");
}

#[test]
fn rejects_ascii_case_duplicate_voice_ids() {
    let case = CaseDir::new();
    case.touch("A/ABC_001.ogg");
    case.touch("B/abc_001.ogg");
    assert!(inventory_voice_root(&case.0).is_err());
}

#[test]
fn rejects_distinct_files_colliding_after_nfc_normalization() {
    let case = CaseDir::new();
    case.touch("A/Café_001.ogg");
    case.touch("B/Cafe\u{301}_001.ogg");
    assert!(inventory_voice_root(&case.0).is_err());
}

#[test]
fn inventory_and_catalog_order_do_not_depend_on_file_creation_order() {
    let case = CaseDir::new();
    case.touch("Zulu/ZUL_002.ogg");
    case.touch("Alpha/ALP_001.ogg");
    case.touch("Zulu/ZUL_001.ogg");
    let first = inventory_voice_root(&case.0).unwrap();
    let second = inventory_voice_root(&case.0).unwrap();
    assert_eq!(first, second);
    let catalog = build_speaker_catalog(&first).unwrap();
    assert_eq!(
        catalog.speakers.keys().cloned().collect::<Vec<_>>(),
        ["Alpha", "Zulu"]
    );
    assert_eq!(catalog.speakers["Alpha"], "alp");
    assert_eq!(catalog.speakers["Zulu"], "zul");
}

#[test]
fn catalog_hashes_colliding_dominant_prefixes_stably() {
    let case = CaseDir::new();
    case.touch("First/ABC_001.ogg");
    case.touch("Second/ABC_002.ogg");
    let inventory = inventory_voice_root(&case.0).unwrap();
    let catalog = build_speaker_catalog(&inventory).unwrap();
    let ids = catalog.speakers.values().cloned().collect::<BTreeSet<_>>();
    assert_eq!(ids.len(), 2);
    assert!(ids.iter().all(|id| id.starts_with("abc-") && id.is_ascii()));
    assert_eq!(catalog, build_speaker_catalog(&inventory).unwrap());
}

#[cfg(unix)]
#[test]
fn probe_parses_json_and_requires_one_audio_stream() {
    use std::os::unix::fs::PermissionsExt;
    let case = CaseDir::new();
    let script = case.join("fake ffprobe");
    std::fs::write(&script, "#!/bin/sh\nprintf '%s' '{\"streams\":[{\"codec_type\":\"audio\",\"codec_name\":\"vorbis\",\"sample_rate\":\"48000\",\"channels\":2}],\"format\":{\"duration\":\"1.25\"}}'\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let sample = case.join("input with spaces;still one argument.ogg");
    std::fs::write(&sample, b"fixture").unwrap();
    let probe = probe_audio(&script, &sample).unwrap();
    assert_eq!(probe.codec, "vorbis");
    assert_eq!(probe.sample_rate, 48_000);
    assert_eq!(probe.channels, 2);
    assert_eq!(probe.duration_seconds, 1.25);

    std::fs::write(&script, "#!/bin/sh\nprintf '%s' '{\"streams\":[{\"codec_type\":\"audio\"},{\"codec_type\":\"audio\"}],\"format\":{\"duration\":\"1\"}}'\n").unwrap();
    assert!(probe_audio(&script, &sample).is_err());
}

#[test]
#[ignore = "requires KANADE_SOURCE_ROOT pointing to lawfully extracted romfs/data"]
fn real_source_inventory_has_fixed_counts_and_distinct_known_speakers() {
    let root = std::env::var_os("KANADE_SOURCE_ROOT")
        .map(PathBuf::from)
        .expect("KANADE_SOURCE_ROOT must point to lawfully extracted romfs/data")
        .join("voice");
    let inventory = inventory_voice_root(&root).unwrap();
    assert_eq!(inventory.assets.len(), 2_809);
    assert_eq!(inventory.sidecars.len(), 322);
    assert_eq!(inventory.speakers.len(), 38);
    assert!(
        inventory
            .assets
            .iter()
            .all(|asset| asset.source_ogg.components().count() == 2)
    );
    let catalog = build_speaker_catalog(&inventory).unwrap();
    let known = ["カナデ", "伊織", "みのり", "system"];
    let ids = known.map(|name| catalog.speakers[name].clone());
    assert_eq!(ids.into_iter().collect::<BTreeSet<_>>().len(), 4);
}

#[cfg(unix)]
#[test]
fn inspect_reports_synthetic_counts_without_creating_an_output() {
    use std::os::unix::fs::PermissionsExt;
    let case = CaseDir::new();
    case.touch("data/voice/Speaker/ABC_001.ogg");
    case.touch("data/voice/Speaker/ABC_001.ogg.sli");
    case.touch("data/scn/example.scn");
    let ffprobe = case.join("ffprobe");
    std::fs::write(&ffprobe, "#!/bin/sh\nprintf '%s' '{\"streams\":[{\"codec_type\":\"audio\",\"codec_name\":\"vorbis\",\"sample_rate\":\"48000\",\"channels\":1}],\"format\":{\"duration\":\"1.0\"}}'\n").unwrap();
    std::fs::set_permissions(&ffprobe, std::fs::Permissions::from_mode(0o700)).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_kanade-tts"))
        .args(["inspect", "--source"])
        .arg(case.join("data"))
        .arg("--ffprobe")
        .arg(&ffprobe)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("1 OGG"), "{stdout}");
    assert!(stdout.contains("1 sidecar"), "{stdout}");
    assert!(stdout.contains("1 speaker"), "{stdout}");
    assert!(stdout.contains("1 scenario"), "{stdout}");
    assert!(!case.join("output").exists());
}
