use clap::Parser;
use kanade_tts::cli::Cli;
use kanade_tts::error::Error;
use kanade_tts::model::SourceEventRef;
use kanade_tts::paths::DatasetPaths;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_CASE: AtomicU64 = AtomicU64::new(0);

struct CaseDir(PathBuf);

impl CaseDir {
    fn new() -> Self {
        let serial = NEXT_CASE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "kanade-tts-path-safety-{}-{serial}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn join(&self, path: impl AsRef<Path>) -> PathBuf {
        self.0.join(path)
    }
}

impl Drop for CaseDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn rejects_output_inside_source() {
    let case = CaseDir::new();
    let source = case.join("source");
    std::fs::create_dir(&source).unwrap();

    let result = DatasetPaths::new(&source, &source.join("uncreated/dataset"));
    assert!(matches!(result, Err(Error::OutputInsideSource { .. })));
}

#[test]
fn rejects_same_source_and_output() {
    let case = CaseDir::new();
    let source = case.join("source");
    std::fs::create_dir(&source).unwrap();

    let result = DatasetPaths::new(&source, &source);
    assert!(matches!(result, Err(Error::OutputInsideSource { .. })));
}

#[test]
fn accepts_sibling_output() {
    let case = CaseDir::new();
    let source = case.join("source");
    std::fs::create_dir(&source).unwrap();
    let output = case.join("output/uncreated/dataset");

    let paths = DatasetPaths::new(&source, &output).unwrap();
    assert_eq!(paths.source_root, source.canonicalize().unwrap());
    assert_eq!(
        paths.output_root,
        case.0
            .canonicalize()
            .unwrap()
            .join("output/uncreated/dataset")
    );
    assert!(!output.exists());
}

#[test]
fn rejects_output_with_dot_dot_back_into_source() {
    let case = CaseDir::new();
    let source = case.join("source");
    std::fs::create_dir(&source).unwrap();

    let result = DatasetPaths::new(&source, &source.join("uncreated/../dataset"));
    assert!(matches!(result, Err(Error::OutputInsideSource { .. })));
}

#[cfg(unix)]
#[test]
fn rejects_output_through_existing_symlink() {
    let case = CaseDir::new();
    let source = case.join("source");
    std::fs::create_dir(&source).unwrap();
    std::os::unix::fs::symlink(&source, case.join("alias")).unwrap();

    let result = DatasetPaths::new(&source, &case.join("alias/uncreated/dataset"));
    assert!(matches!(result, Err(Error::OutputInsideSource { .. })));
}

#[test]
fn source_event_round_trips_json() {
    let source_event = SourceEventRef {
        source_scn: "scn/scn00_01.ks.scn".into(),
        scenario_event_index: 14,
        scene_index: 0,
        scene_text_index: 14,
    };
    let serialized = serde_json::to_string(&source_event).unwrap();
    assert_eq!(
        serialized,
        r#"{"source_scn":"scn/scn00_01.ks.scn","scenario_event_index":14,"scene_index":0,"scene_text_index":14}"#
    );
    let decoded: SourceEventRef = serde_json::from_str(&serialized).unwrap();
    assert_eq!(decoded, source_event);
}

#[test]
fn cli_exposes_all_rust_stages() {
    for stage in ["inspect", "pilot", "build", "review", "finalize", "verify"] {
        let parsed = Cli::try_parse_from(["kanade-tts", stage, "--help"]);
        assert!(parsed.is_err(), "{stage} --help should request help");
        assert_eq!(
            parsed.unwrap_err().kind(),
            clap::error::ErrorKind::DisplayHelp
        );
    }
}
