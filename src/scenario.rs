use crate::{
    error::{Error, Result},
    inventory::{VoiceInventory, normalize_voice_id},
    model::{RejectReason, ScenarioTextEntry, SourceEventRef, VoiceMeta, quality_flags_cell},
    paths::portable_relative,
    speakers::{SpeakerAliases, build_speaker_catalog, resolve_speaker},
    text::{NormalizationRules, normalize_text},
};
use emote_psb::{
    psb::read::PsbFile,
    value::{PsbValue, number::PsbNumber},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::BufReader,
    path::Path,
};

#[derive(Clone, Debug, Deserialize)]
struct VersionedLocaleMap {
    schema_version: String,
    source_files: Vec<LocaleSourceFile>,
    base_tag: String,
    runtime_base_index: usize,
    tags: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct LocaleSourceFile {
    path: String,
    sha256: String,
}

const LOCALE_SOURCES: [(&str, &str); 4] = [
    (
        "main/default.tjs",
        "0b82ffaf96aa3f3bd37fbb9434d98a31068219fa102f89c6b8b10cbb7f912a72",
    ),
    (
        "system/StorageData.tjs",
        "4bbd09c79610557afbd6bc14f65a3c2d052843779f355e2416a0518c2c5a5b98",
    ),
    (
        "system/MainWindow.tjs",
        "2753db2340a6058e666fcc5452124169ef8ec9edb69e7142e16e6587a4ce4466",
    ),
    (
        "sysscn/multilang.tjs",
        "79dde5c441a03b8981c1ad5c241b216298093a4ed662f99dfa20800c156fb035",
    ),
];

#[derive(Clone, Debug)]
pub struct LocaleMap {
    tags: Vec<String>,
    base_tag: String,
    runtime_base_index: usize,
}

impl LocaleMap {
    pub fn from_tags<I, S>(tags: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let tags: Vec<String> = tags
            .into_iter()
            .map(|tag| tag.as_ref().to_owned())
            .collect();
        if tags.iter().filter(|tag| tag.as_str() == "jp").count() != 1 {
            return Err(Error::LocaleMap(
                "expected exactly one jp declaration".into(),
            ));
        }
        if tags.first().is_none_or(|tag| tag != "jp") {
            return Err(Error::LocaleMap(
                "jp must be the unsuffixed base locale".into(),
            ));
        }
        let distinct: BTreeSet<&str> = tags.iter().map(String::as_str).collect();
        if distinct.len() != tags.len() {
            return Err(Error::LocaleMap("duplicate locale declaration".into()));
        }
        if tags.len() != 4 || distinct != BTreeSet::from(["jp", "en", "cn", "tw"]) {
            return Err(Error::LocaleMap("unsupported locale declaration".into()));
        }
        Ok(Self {
            tags,
            base_tag: "jp".into(),
            runtime_base_index: 0,
        })
    }

    pub fn kanade() -> Result<Self> {
        let versioned: VersionedLocaleMap =
            serde_json::from_str(include_str!("../config/locale-map.json"))
                .map_err(|error| Error::LocaleMap(error.to_string()))?;
        if versioned.schema_version != "kanade-scenario-locale-v2"
            || versioned.base_tag != "jp"
            || versioned.runtime_base_index != 0
            || versioned.source_files.len() != LOCALE_SOURCES.len()
            || versioned
                .source_files
                .iter()
                .zip(LOCALE_SOURCES)
                .any(|(actual, expected)| actual.path != expected.0 || actual.sha256 != expected.1)
            || versioned.tags != ["jp", "en", "cn", "tw"]
        {
            return Err(Error::LocaleMap(
                "unrecognized locale map version or source".into(),
            ));
        }
        let mut map = Self::from_tags(versioned.tags)?;
        map.base_tag = versioned.base_tag;
        map.runtime_base_index = versioned.runtime_base_index;
        Ok(map)
    }
}

pub fn verify_locale_source(source_root: &Path) -> Result<()> {
    LocaleMap::kanade()?;
    let versioned: VersionedLocaleMap =
        serde_json::from_str(include_str!("../config/locale-map.json"))
            .map_err(|error| Error::LocaleMap(error.to_string()))?;
    for source_file in versioned.source_files {
        let path = source_root.join(&source_file.path);
        let bytes = std::fs::read(&path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        let actual = format!("{:x}", Sha256::digest(bytes));
        if actual != source_file.sha256 {
            return Err(Error::LocaleMap(format!(
                "locale runtime fingerprint mismatch at {}",
                path.display()
            )));
        }
    }
    Ok(())
}

pub fn load_psb_root(path: &Path) -> Result<PsbValue> {
    let file = File::open(path).map_err(|source| Error::Io {
        path: path.to_owned(),
        source,
    })?;
    let mut psb = PsbFile::open(BufReader::new(file)).map_err(|error| Error::Psb {
        path: path.to_owned(),
        detail: error.to_string(),
    })?;
    if psb.version != 3 || psb.encrypted {
        return Err(Error::Psb {
            path: path.to_owned(),
            detail: format!("unsupported PSB version {} or encryption", psb.version),
        });
    }
    psb.deserialize_root().map_err(|error| Error::Psb {
        path: path.to_owned(),
        detail: error.to_string(),
    })
}

fn schema(code: &'static str, path: impl Into<String>) -> Error {
    Error::ScenarioSchema {
        code,
        path: path.into(),
    }
}
fn object<'a>(
    value: &'a PsbValue,
    path: &str,
) -> Result<&'a std::collections::HashMap<smol_str::SmolStr, PsbValue>> {
    match value {
        PsbValue::Object(value) => Ok(value),
        _ => Err(schema("unknown_event_schema", path)),
    }
}
fn list<'a>(value: &'a PsbValue, path: &str) -> Result<&'a [PsbValue]> {
    match value {
        PsbValue::List(value) => Ok(value),
        _ => Err(schema("unknown_event_schema", path)),
    }
}
fn string<'a>(value: &'a PsbValue, path: &str) -> Result<&'a str> {
    match value {
        PsbValue::String(value) => Ok(value.as_str()),
        _ => Err(schema("unknown_event_schema", path)),
    }
}
fn optional_string(value: &PsbValue, path: &str) -> Result<Option<String>> {
    match value {
        PsbValue::Null => Ok(None),
        PsbValue::String(value) => Ok(Some(value.to_string())),
        _ => Err(schema("unknown_event_schema", path)),
    }
}
fn numeric(value: &PsbValue, path: &str) -> Result<f64> {
    match value {
        PsbValue::Number(PsbNumber::Integer(value)) => Ok(*value as f64),
        PsbValue::Number(PsbNumber::Double(value)) => Ok(*value),
        PsbValue::Number(PsbNumber::Float(value)) => Ok(f64::from(*value)),
        _ => Err(schema("unknown_event_schema", path)),
    }
}

type PsbObject = std::collections::HashMap<smol_str::SmolStr, PsbValue>;

enum SceneKind<'a> {
    Text {
        texts: &'a [PsbValue],
        line_count: usize,
        label: &'a str,
        branch: bool,
    },
    EndTransition,
    BranchTransition {
        label: &'a str,
        line_count: usize,
        mapped: bool,
    },
    ChoiceTransition {
        choices: usize,
    },
}

fn exact_keys(fields: &PsbObject, keys: &[&str]) -> bool {
    fields.len() == keys.len() && keys.iter().all(|key| fields.contains_key(*key))
}

fn integer(value: &PsbValue) -> Option<i64> {
    if let PsbValue::Number(PsbNumber::Integer(value)) = value {
        Some(*value)
    } else {
        None
    }
}

fn event_metadata(value: &PsbValue, path: &str) -> Result<()> {
    let fields = object(value, path)?;
    if !matches!(fields.get("data"), Some(PsbValue::List(_)))
        || !matches!(fields.get("env"), Some(PsbValue::Object(_)))
        || fields
            .keys()
            .any(|key| !["data", "env", "meswintype", "loopVoiceList"].contains(&key.as_str()))
        || fields
            .get("meswintype")
            .is_some_and(|value| !matches!(value, PsbValue::String(_)))
        || fields
            .get("loopVoiceList")
            .is_some_and(|value| !matches!(value, PsbValue::List(_)))
    {
        return Err(schema("unknown_event_schema", path));
    }
    Ok(())
}

fn structural_token(value: &PsbValue, hash: &mut Sha256, path: &str) -> Result<()> {
    match value {
        PsbValue::Null => hash.update(b"null"),
        PsbValue::String(_) => hash.update(b"string"),
        PsbValue::Number(PsbNumber::Integer(_)) => hash.update(b"integer"),
        PsbValue::Number(PsbNumber::Float(_)) => hash.update(b"float"),
        PsbValue::Number(PsbNumber::Double(_)) => hash.update(b"double"),
        PsbValue::List(items) => {
            hash.update(b"list");
            hash.update((items.len() as u64).to_le_bytes());
            for (index, item) in items.iter().enumerate() {
                structural_token(item, hash, &format!("{path}[{index}]"))?;
            }
            hash.update(b"end-list");
        }
        PsbValue::Object(fields) => {
            hash.update(b"object");
            hash.update((fields.len() as u64).to_le_bytes());
            let mut keys: Vec<_> = fields.keys().collect();
            keys.sort();
            for key in keys {
                hash.update((key.len() as u64).to_le_bytes());
                hash.update(key.as_bytes());
                let value = &fields[key];
                if key.as_str() == "env" {
                    if !matches!(value, PsbValue::Object(_)) {
                        return Err(schema("unknown_event_schema", format!("{path}.{key}")));
                    }
                    // Runtime environment state is intentionally open. The
                    // containing object's keys and env's object type remain
                    // part of the structural fingerprint.
                    hash.update(b"open-env-object");
                } else {
                    structural_token(value, hash, &format!("{path}.{key}"))?;
                }
            }
            hash.update(b"end-object");
        }
        _ => return Err(schema("unknown_event_schema", path)),
    }
    Ok(())
}

fn line_structure_digest(lines: &[PsbValue], path: &str) -> Result<String> {
    let mut hash = Sha256::new();
    hash.update(b"kanade-scene-line-structure-v1\0");
    hash.update((lines.len() as u64).to_le_bytes());
    for (index, line) in lines.iter().enumerate() {
        structural_token(line, &mut hash, &format!("{path}[{index}]"))?;
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn line_marker(value: &PsbValue) -> bool {
    let PsbValue::List(fields) = value else {
        return false;
    };
    fields.len() == 5
        && integer(&fields[0]).is_some()
        && matches!(fields[1], PsbValue::Object(_))
        && matches!(fields[2], PsbValue::Null)
        && integer(&fields[3]).is_some()
        && integer(&fields[4]).is_some()
}

fn synthetic_fixture_lines(lines: &[PsbValue]) -> bool {
    if lines.len() != 64
        || !lines.first().is_some_and(line_marker)
        || !lines.last().is_some_and(line_marker)
    {
        return false;
    }
    let marker_metadata = |value: &PsbValue| match value {
        PsbValue::List(fields) => match &fields[1] {
            PsbValue::Object(metadata) => {
                exact_keys(metadata, &["data", "env"])
                    && matches!(metadata.get("data"), Some(PsbValue::List(data)) if data.is_empty())
                    && matches!(metadata.get("env"), Some(PsbValue::Object(env)) if env.is_empty())
            }
            _ => false,
        },
        _ => false,
    };
    if !marker_metadata(&lines[0]) || !marker_metadata(&lines[63]) {
        return false;
    }
    let mut in_control_lines = false;
    for line in &lines[1..63] {
        match line {
            PsbValue::Number(PsbNumber::Integer(_)) if !in_control_lines => {}
            PsbValue::List(items)
                if items.len() == 1 && matches!(items[0], PsbValue::String(_)) =>
            {
                in_control_lines = true;
            }
            _ => return false,
        }
    }
    true
}

#[derive(Deserialize)]
struct LineStructureFingerprints {
    schema_version: String,
    scenes: BTreeMap<String, Vec<String>>,
}

const REVIEWED_LINE_CONFIG_SHA256: &str =
    "372270ff916580802e72c75844260846055b4d7609b1a4e712b5d26173092182";
const REVIEWED_SCENES: [(&str, usize); 43] = [
    ("cd01.ks.scn", 2),
    ("cd02.ks.scn", 2),
    ("cd03.ks.scn", 2),
    ("cd04.ks.scn", 2),
    ("cd05.ks.scn", 2),
    ("cd06.ks.scn", 2),
    ("cd07.ks.scn", 2),
    ("scn00_01.ks.scn", 2),
    ("scn00_02.ks.scn", 2),
    ("scn00_03.ks.scn", 2),
    ("scn00_04.ks.scn", 2),
    ("scn00_05.ks.scn", 2),
    ("scn00_06.ks.scn", 2),
    ("scn00_07.ks.scn", 2),
    ("scn00_08.ks.scn", 2),
    ("scn01_01.ks.scn", 2),
    ("scn01_02.ks.scn", 2),
    ("scn01_03.ks.scn", 2),
    ("scn01_04.ks.scn", 2),
    ("scn01_05.ks.scn", 2),
    ("scn01_06.ks.scn", 2),
    ("scn01_07.ks.scn", 2),
    ("scn01_08.ks.scn", 2),
    ("scn02_01.ks.scn", 2),
    ("scn02_02.ks.scn", 2),
    ("scn02_03.ks.scn", 2),
    ("scn02_04.ks.scn", 2),
    ("scn02_05.ks.scn", 2),
    ("scn02_06.ks.scn", 2),
    ("scn03_01.ks.scn", 2),
    ("scn03_02.ks.scn", 2),
    ("scn03_03.ks.scn", 2),
    ("scn03_04.ks.scn", 2),
    ("scn04_01.ks.scn", 2),
    ("scn04_02.ks.scn", 3),
    ("scn05_01.ks.scn", 2),
    ("scn05_02.ks.scn", 2),
    ("scn05_03.ks.scn", 2),
    ("scn05_04.ks.scn", 4),
    ("scn06_01.ks.scn", 2),
    ("scn06_02.ks.scn", 2),
    ("scn06_03.ks.scn", 2),
    ("scn06_04.ks.scn", 2),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ScenarioStructureMode {
    Pinned,
    TestFixture,
}

pub(crate) fn reviewed_scenario_names() -> impl Iterator<Item = &'static str> {
    REVIEWED_SCENES.iter().map(|(name, _)| *name)
}

fn line_structure_config(path: &str) -> Result<LineStructureFingerprints> {
    line_structure_config_from_bytes(
        include_bytes!("../config/line-structure-fingerprints.json"),
        path,
    )
}

fn line_structure_config_from_bytes(bytes: &[u8], path: &str) -> Result<LineStructureFingerprints> {
    if format!("{:x}", Sha256::digest(bytes)) != REVIEWED_LINE_CONFIG_SHA256 {
        return Err(schema("unknown_event_schema", path));
    }
    let config: LineStructureFingerprints =
        serde_json::from_slice(bytes).map_err(|_| schema("unknown_event_schema", path))?;
    if !valid_line_structure_config(&config) {
        return Err(schema("unknown_event_schema", path));
    }
    Ok(config)
}

fn valid_line_structure_config(config: &LineStructureFingerprints) -> bool {
    config.schema_version == "kanade-scene-line-structure-v1"
        && config.scenes.len() == REVIEWED_SCENES.len()
        && REVIEWED_SCENES.iter().all(|(name, count)| {
            config.scenes.get(*name).is_some_and(|hashes| {
                hashes.len() == *count
                    && hashes.iter().all(|hash| {
                        hash.len() == 64
                            && hash
                                .bytes()
                                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                    })
                    && hashes.iter().collect::<BTreeSet<_>>().len() == hashes.len()
            })
        })
}

fn validate_scene_lines(
    lines: &[PsbValue],
    path: &str,
    source: &Path,
    scene_index: usize,
    mode: ScenarioStructureMode,
) -> Result<()> {
    if !lines.first().is_some_and(line_marker) {
        return Err(schema("unknown_event_schema", format!("{path}[0]")));
    }
    if !lines.last().is_some_and(line_marker) {
        return Err(schema(
            "unknown_event_schema",
            format!("{path}[{}]", lines.len() - 1),
        ));
    }
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| schema("unknown_event_schema", path))?;
    if mode == ScenarioStructureMode::TestFixture
        && matches!(name, "fixture.ks.scn" | "pilot.ks.scn")
    {
        if synthetic_fixture_lines(lines) {
            return Ok(());
        }
        return Err(schema("unknown_event_schema", path));
    }
    let config = line_structure_config(path)?;
    let expected = config
        .scenes
        .get(name)
        .and_then(|hashes| hashes.get(scene_index))
        .ok_or_else(|| schema("unknown_event_schema", path))?;
    let actual = line_structure_digest(lines, path)?;
    if &actual != expected {
        return Err(schema("unknown_event_schema", path));
    }
    Ok(())
}
fn classify_scene<'a>(
    scene: &'a PsbObject,
    path: &str,
    source: &Path,
    scene_index: usize,
    mode: ScenarioStructureMode,
) -> Result<SceneKind<'a>> {
    let label = scene
        .get("label")
        .and_then(|value| match value {
            PsbValue::String(value) => Some(value.as_str()),
            _ => None,
        })
        .ok_or_else(|| schema("unknown_event_schema", format!("{path}.label")))?;
    let is_text = scene.contains_key("texts");
    let is_choice = scene.contains_key("selects") || scene.contains_key("selectInfo");
    let common = [
        "files",
        "firstLine",
        "label",
        "lines",
        "spCount",
        "title",
        "version",
    ];
    let mut keys = common.to_vec();
    if is_choice {
        keys.extend(["selectInfo", "selects"]);
    } else {
        keys.push("nexts");
        if is_text {
            keys.push("texts");
        }
    }
    if !exact_keys(scene, &keys)
        || !matches!(scene.get("files"), Some(PsbValue::List(_)))
        || !scene
            .get("firstLine")
            .is_some_and(|value| integer(value).is_some())
        || !scene
            .get("spCount")
            .is_some_and(|value| integer(value).is_some())
        || !scene
            .get("version")
            .is_some_and(|value| numeric(value, path).is_ok())
        || !scene.get("title").is_some_and(|value| match value {
            PsbValue::String(_) => true,
            PsbValue::List(values) => {
                values.len() == 4
                    && values
                        .iter()
                        .all(|value| matches!(value, PsbValue::String(_)))
            }
            _ => false,
        })
    {
        return Err(schema("unknown_event_schema", path));
    }
    let files = list(&scene["files"], &format!("{path}.files"))?;
    for (index, file) in files.iter().enumerate() {
        let file_path = format!("{path}.files[{index}]");
        let fields = list(file, &file_path)?;
        if fields.len() != 2
            || !matches!(fields[0], PsbValue::String(_))
            || integer(&fields[1]).is_none()
        {
            return Err(schema("unknown_event_schema", file_path));
        }
    }
    let lines = list(&scene["lines"], &format!("{path}.lines"))?;
    validate_scene_lines(lines, &format!("{path}.lines"), source, scene_index, mode)?;
    let Some(PsbValue::List(first)) = lines.first() else {
        return Err(schema("unknown_event_schema", format!("{path}.lines[0]")));
    };
    if first.len() != 5
        || integer(&first[0]).is_none()
        || !matches!(first[2], PsbValue::Null)
        || integer(&first[3]).is_none()
        || integer(&first[4]).is_none()
    {
        return Err(schema("unknown_event_schema", format!("{path}.lines[0]")));
    }
    event_metadata(&first[1], &format!("{path}.lines[0][1]"))?;
    if is_choice {
        if is_text || !label.starts_with("*dummyselect") || !files.is_empty() {
            return Err(schema("unknown_event_schema", path));
        }
        let info = object(&scene["selectInfo"], &format!("{path}.selectInfo"))?;
        if !exact_keys(info, &["_line"]) || integer(&info["_line"]).is_none() {
            return Err(schema("unknown_event_schema", format!("{path}.selectInfo")));
        }
        let selects = list(&scene["selects"], &format!("{path}.selects"))?;
        if selects.is_empty() {
            return Err(schema("unknown_event_schema", format!("{path}.selects")));
        }
        for (index, select) in selects.iter().enumerate() {
            let p = format!("{path}.selects[{index}]");
            let fields = object(select, &p)?;
            if !exact_keys(
                fields,
                &["language", "render", "selidx", "storage", "target", "text"],
            ) || !matches!(fields.get("language"),Some(PsbValue::List(values)) if values.len()==4)
                || !fields.get("render").is_some_and(|v| integer(v).is_some())
                || !fields.get("selidx").is_some_and(|v| integer(v).is_some())
                || ["storage", "target", "text"]
                    .iter()
                    .any(|key| !matches!(fields.get(*key), Some(PsbValue::String(_))))
            {
                return Err(schema("unknown_event_schema", p));
            }
            let localized = list(&fields["language"], &format!("{p}.language"))?;
            for (language_index, value) in localized.iter().enumerate() {
                let language_path = format!("{p}.language[{language_index}]");
                if language_index == 0 {
                    if !matches!(value, PsbValue::Null) {
                        return Err(schema("unknown_event_schema", language_path));
                    }
                } else {
                    let language = object(value, &language_path)?;
                    if !exact_keys(
                        language,
                        &["searchtext", "speechtext", "text", "textlength"],
                    ) || ["searchtext", "speechtext", "text"]
                        .iter()
                        .any(|key| !matches!(language.get(*key), Some(PsbValue::String(_))))
                        || integer(&language["textlength"]).is_none()
                    {
                        return Err(schema("unknown_event_schema", language_path));
                    }
                }
            }
        }
        return Ok(SceneKind::ChoiceTransition {
            choices: selects.len(),
        });
    }
    let nexts = list(&scene["nexts"], &format!("{path}.nexts"))?;
    if nexts.is_empty() {
        return Err(schema("unknown_event_schema", format!("{path}.nexts")));
    }
    for (index, next) in nexts.iter().enumerate() {
        let p = format!("{path}.nexts[{index}]");
        let fields = object(next, &p)?;
        if !(exact_keys(fields, &["type", "storage", "target"])
            || exact_keys(fields, &["type", "storage", "target", "eval"]))
            || !fields.get("type").is_some_and(|v| integer(v).is_some())
            || ["storage", "target"]
                .iter()
                .any(|key| !matches!(fields.get(*key), Some(PsbValue::String(_))))
            || fields
                .get("eval")
                .is_some_and(|v| !matches!(v, PsbValue::String(_)))
        {
            return Err(schema("unknown_event_schema", p));
        }
    }
    if is_text {
        if !(label == "*start" || label.starts_with("*root")) {
            return Err(schema("unknown_event_schema", path));
        }
        let texts = list(&scene["texts"], &format!("{path}.texts"))?;
        if lines
            .iter()
            .filter(|line| matches!(line, PsbValue::Number(PsbNumber::Integer(_))))
            .count()
            != texts.len()
        {
            return Err(schema("unknown_event_schema", format!("{path}.lines")));
        }
        Ok(SceneKind::Text {
            texts,
            line_count: lines.len(),
            label,
            branch: label.starts_with("*root"),
        })
    } else if label == "*end" && files.is_empty() {
        Ok(SceneKind::EndTransition)
    } else if label.starts_with("*root") {
        Ok(SceneKind::BranchTransition {
            label,
            line_count: lines.len(),
            mapped: !files.is_empty(),
        })
    } else {
        Err(schema("unknown_event_schema", path))
    }
}

pub fn extract_scenario_texts(root: &PsbValue, source: &Path) -> Result<Vec<ScenarioTextEntry>> {
    let map = LocaleMap::kanade()?;
    extract_scenario_texts_with_map(root, source, &map)
}

/// Synthetic scenario parser for integration tests. No CLI path selects this mode.
#[doc(hidden)]
pub fn extract_fixture_scenario_texts(
    root: &PsbValue,
    source: &Path,
) -> Result<Vec<ScenarioTextEntry>> {
    let map = LocaleMap::kanade()?;
    extract_fixture_scenario_texts_with_map(root, source, &map)
}

/// Synthetic scenario parser for integration tests. No CLI path selects this mode.
#[doc(hidden)]
pub fn extract_fixture_scenario_texts_with_map(
    root: &PsbValue,
    source: &Path,
    map: &LocaleMap,
) -> Result<Vec<ScenarioTextEntry>> {
    extract_scenario_texts_inner(root, source, map, ScenarioStructureMode::TestFixture)
}

pub fn extract_scenario_texts_with_map(
    root: &PsbValue,
    source: &Path,
    map: &LocaleMap,
) -> Result<Vec<ScenarioTextEntry>> {
    extract_scenario_texts_inner(root, source, map, ScenarioStructureMode::Pinned)
}

fn extract_scenario_texts_inner(
    root: &PsbValue,
    source: &Path,
    map: &LocaleMap,
    mode: ScenarioStructureMode,
) -> Result<Vec<ScenarioTextEntry>> {
    let source_scn = if source.is_absolute() {
        source.to_string_lossy().into_owned()
    } else {
        portable_relative(source)?
    };
    let root = object(root, "root")?;
    let name = string(
        root.get("name")
            .ok_or_else(|| schema("missing_name", "root.name"))?,
        "root.name",
    )?;
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || source.file_name().and_then(|file| file.to_str()) != Some(&format!("{name}.scn"))
    {
        return Err(schema("invalid_name", "root.name"));
    }
    let languages = list(
        root.get("languages")
            .ok_or_else(|| schema("missing_languages", "root.languages"))?,
        "root.languages",
    )?;
    if languages.len() + 1 != map.tags.len() {
        return Err(schema("locale_cardinality", "root.languages"));
    }
    let declared: Vec<&str> = languages
        .iter()
        .enumerate()
        .map(|(index, value)| string(value, &format!("root.languages[{index}]")))
        .collect::<Result<_>>()?;
    let expected: BTreeSet<&str> = map.tags.iter().skip(1).map(String::as_str).collect();
    let actual: BTreeSet<&str> = declared.iter().copied().collect();
    if declared.contains(&"jp") || actual != expected || actual.len() != declared.len() {
        return Err(schema("ambiguous_locale_declaration", "root.languages"));
    }
    // StorageData.convertSceneText inserts the base record at the runtime base
    // index, then preserves the translated records in root.languages order.
    let mut localized_tags = declared.clone();
    if map.runtime_base_index > localized_tags.len() {
        return Err(schema("ambiguous_locale_declaration", "root.languages"));
    }
    localized_tags.insert(map.runtime_base_index, map.base_tag.as_str());
    let japanese_index = localized_tags
        .iter()
        .position(|tag| *tag == "jp")
        .ok_or_else(|| schema("ambiguous_locale_declaration", "root.languages"))?;
    let llmap = list(
        root.get("llmap")
            .ok_or_else(|| schema("missing_llmap", "root.llmap"))?,
        "root.llmap",
    )?;
    if llmap.len() != declared.len() {
        return Err(schema("locale_cardinality", "root.llmap"));
    }
    let stem = name
        .strip_suffix(".ks")
        .ok_or_else(|| schema("invalid_name", "root.name"))?;
    for (index, (entry, lang)) in llmap.iter().zip(declared.iter()).enumerate() {
        let path = format!("root.llmap[{index}]");
        let entry = object(entry, &path)?;
        let actual_name = string(
            entry
                .get("name")
                .ok_or_else(|| schema("missing_llmap_name", &path))?,
            &path,
        )?;
        if actual_name != format!("{stem}_{lang}.ks") {
            return Err(schema("ambiguous_locale_declaration", path));
        }
    }
    let scenes = list(
        root.get("scenes")
            .ok_or_else(|| schema("missing_scenes", "root.scenes"))?,
        "root.scenes",
    )?;
    let source_name = source.file_name().and_then(|name| name.to_str());
    if !(mode == ScenarioStructureMode::TestFixture
        && matches!(source_name, Some("fixture.ks.scn" | "pilot.ks.scn")))
    {
        let config = line_structure_config("root.scenes")?;
        if source_name
            .and_then(|name| config.scenes.get(name))
            .is_none_or(|hashes| hashes.len() != scenes.len())
        {
            return Err(schema("unknown_event_schema", "root.scenes"));
        }
    }
    let mut choice_count = None;
    for (scene_index, scene) in scenes.iter().enumerate() {
        let path = format!("root.scenes[{scene_index}]");
        if let SceneKind::ChoiceTransition { choices } =
            classify_scene(object(scene, &path)?, &path, source, scene_index, mode)?
            && choice_count.replace(choices).is_some()
        {
            return Err(schema("unknown_event_schema", path));
        }
    }
    let choice_count = choice_count.unwrap_or(0);
    // Every scene has passed full line and control-payload validation before
    // line counts are used as locale-map bounds.
    let total_line_count: usize = scenes
        .iter()
        .map(|scene| match scene {
            PsbValue::Object(fields) => match fields.get("lines") {
                Some(PsbValue::List(lines)) => lines.len(),
                _ => unreachable!("validated scene lines"),
            },
            _ => unreachable!("validated scene object"),
        })
        .sum();
    let mut map_specs = BTreeMap::<String, (usize, usize)>::new();
    for (scene_index, scene) in scenes.iter().enumerate() {
        let path = format!("root.scenes[{scene_index}]");
        let scene = object(scene, &path)?;
        let spec = match classify_scene(scene, &path, source, scene_index, mode)? {
            SceneKind::Text {
                texts,
                line_count,
                label,
                branch,
            } => Some((
                label,
                texts.len() + if branch { choice_count } else { 0 },
                if branch { total_line_count } else { line_count },
            )),
            SceneKind::BranchTransition {
                label,
                line_count,
                mapped: true,
            } => Some((label, choice_count, total_line_count.max(line_count))),
            SceneKind::EndTransition
            | SceneKind::ChoiceTransition { .. }
            | SceneKind::BranchTransition { mapped: false, .. } => None,
        };
        if spec.is_some_and(|(label, _, _)| label.starts_with("*root")) && choice_count == 0 {
            return Err(schema("unknown_event_schema", path));
        }
        if let Some((label, count, range)) = spec
            && map_specs.insert(label.to_owned(), (count, range)).is_some()
        {
            return Err(schema("ambiguous_locale_declaration", path));
        }
    }
    for (locale_index, translation) in llmap.iter().enumerate() {
        let path = format!("root.llmap[{locale_index}]");
        let translation = object(translation, &path)?;
        if translation.len() != map_specs.len() + 1
            || translation
                .keys()
                .any(|key| key != "name" && !map_specs.contains_key(key.as_str()))
        {
            return Err(schema("ambiguous_locale_declaration", path));
        }
        for (label, (count, range)) in &map_specs {
            let map_path = format!("{path}.{label}");
            let indices = list(
                translation
                    .get(label.as_str())
                    .ok_or_else(|| schema("ambiguous_locale_declaration", &map_path))?,
                &map_path,
            )?;
            if indices.len() != *count {
                return Err(schema("ambiguous_locale_declaration", map_path));
            }
            let mut previous = None;
            for value in indices {
                let line = integer(value)
                    .ok_or_else(|| schema("ambiguous_locale_declaration", &map_path))?;
                if line < 0
                    || !usize::try_from(line).is_ok_and(|line| line < *range)
                    || previous.is_some_and(|previous| line <= previous)
                {
                    return Err(schema("ambiguous_locale_declaration", &map_path));
                }
                previous = Some(line);
            }
        }
    }
    let mut entries = Vec::new();
    for (scene_index, scene) in scenes.iter().enumerate() {
        let scene_path = format!("root.scenes[{scene_index}]");
        let scene = object(scene, &scene_path)?;
        let SceneKind::Text { texts, .. } =
            classify_scene(scene, &scene_path, source, scene_index, mode)?
        else {
            continue;
        };
        for (scene_text_index, event) in texts.iter().enumerate() {
            let path = format!("{scene_path}.texts[{scene_text_index}]");
            let slots = list(event, &path)?;
            if !matches!(slots.len(), 5 | 6) || numeric(&slots[3], &format!("{path}[3]")).is_err() {
                return Err(schema("unknown_event_schema", path));
            }
            event_metadata(&slots[4], &format!("{path}[4]"))?;
            let appended_integer = if slots.len() == 6 {
                Some(
                    integer(&slots[5])
                        .ok_or_else(|| schema("unknown_event_schema", format!("{path}[5]")))?,
                )
            } else {
                None
            };
            let speaker_name = optional_string(&slots[0], &format!("{path}[0]"))?;
            let locales = list(&slots[1], &format!("{path}[1]"))?;
            if locales.len() != map.tags.len() {
                return Err(schema("locale_cardinality", format!("{path}[1]")));
            }
            let mut japanese_text = None;
            let mut display_speaker_name = None;
            for (index, locale) in locales.iter().enumerate() {
                let locale_path = format!("{path}[1][{index}]");
                let fields = list(locale, &locale_path)?;
                if !matches!(fields.len(), 3 | 5)
                    || !matches!(fields[2], PsbValue::Number(_))
                    || fields[3..]
                        .iter()
                        .any(|value| !matches!(value, PsbValue::String(_)))
                {
                    return Err(schema("unknown_event_schema", locale_path));
                }
                let display = optional_string(&fields[0], &format!("{locale_path}[0]"))?;
                let text = optional_string(&fields[1], &format!("{locale_path}[1]"))?;
                if index == japanese_index {
                    display_speaker_name = display;
                    japanese_text = text;
                }
            }
            let voices = match &slots[2] {
                PsbValue::Null => Vec::new(),
                PsbValue::List(values) => values
                    .iter()
                    .enumerate()
                    .map(|(index, voice)| {
                        let voice_path = format!("{path}[2][{index}]");
                        let fields = object(voice, &voice_path)
                            .map_err(|_| schema("malformed_voice_object", &voice_path))?;
                        if fields.len() != 4
                            || !["name", "pan", "type", "voice"]
                                .iter()
                                .all(|key| fields.contains_key(*key))
                        {
                            return Err(schema("malformed_voice_object", &voice_path));
                        }
                        let id = fields
                            .get("voice")
                            .and_then(|value| match value {
                                PsbValue::String(id) if !id.is_empty() => Some(id.to_string()),
                                _ => None,
                            })
                            .ok_or_else(|| schema("malformed_voice_object", &voice_path))?;
                        let name = match fields.get("name") {
                            Some(PsbValue::String(name)) => Some(name.to_string()),
                            _ => return Err(schema("malformed_voice_object", &voice_path)),
                        };
                        let pan = Some(
                            numeric(&fields["pan"], &voice_path)
                                .map_err(|_| schema("malformed_voice_object", &voice_path))?,
                        );
                        let kind = match fields.get("type") {
                            Some(PsbValue::Number(PsbNumber::Integer(value))) => Some(*value),
                            _ => return Err(schema("malformed_voice_object", &voice_path)),
                        };
                        Ok(VoiceMeta {
                            voice: id,
                            name,
                            pan,
                            kind,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
                _ => return Err(schema("unknown_event_schema", format!("{path}[2]"))),
            };
            let rejection_reason = if voices.len() > 1 {
                Some(RejectReason::MultiVoiceEvent)
            } else if !voices.is_empty() && japanese_text.as_ref().is_none_or(String::is_empty) {
                Some(RejectReason::MissingJapaneseText)
            } else {
                None
            };
            entries.push(ScenarioTextEntry {
                source_event: SourceEventRef {
                    source_scn: source_scn.clone(),
                    scenario_event_index: entries.len(),
                    scene_index,
                    scene_text_index,
                },
                speaker_name,
                display_speaker_name,
                japanese_text,
                voices,
                appended_integer,
                rejection_reason,
            });
        }
    }
    if entries.is_empty() {
        return Err(schema("missing_texts", "root.scenes[].texts"));
    }
    Ok(entries)
}

pub fn write_pilot_table(
    entries: &[ScenarioTextEntry],
    inventory: &VoiceInventory,
    out: &Path,
) -> Result<()> {
    let mut assets = BTreeMap::new();
    for asset in &inventory.assets {
        let voice_id = normalize_voice_id(&asset.voice_id);
        if let Some(first) = assets.insert(voice_id.clone(), asset) {
            return Err(Error::VoiceIdCollision {
                voice_id,
                first: first.source_ogg.clone(),
                second: asset.source_ogg.clone(),
            });
        }
    }
    std::fs::create_dir_all(out).map_err(|source| Error::Io {
        path: out.to_owned(),
        source,
    })?;
    let path = out.join("pilot.tsv");
    match path.symlink_metadata() {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(Error::UnsafePilotOutput(path));
        }
        Ok(_) => {}
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => return Err(Error::Io { path, source }),
    }
    let catalog = build_speaker_catalog(inventory)?;
    let aliases = SpeakerAliases::kanade(&catalog)?;
    let normalization_rules = NormalizationRules::kanade();
    let file = File::create(&path).map_err(|source| Error::Io {
        path: path.clone(),
        source,
    })?;
    let mut writer = csv::WriterBuilder::new().delimiter(b'\t').from_writer(file);
    writer.write_record([
        "source_scn",
        "scene_index",
        "scene_text_index",
        "scenario_event_index",
        "scenario_speaker",
        "voice_meta_name",
        "source_speaker",
        "speaker_id",
        "display_speaker",
        "japanese_raw_text",
        "japanese_display_text",
        "japanese_spoken_text",
        "normalization_version",
        "quality_flags",
        "voice_id",
        "source_ogg_match_count",
        "source_ogg",
        "rejection_reason",
    ])?;
    for entry in entries {
        let voices: Vec<Option<&VoiceMeta>> = if entry.voices.is_empty() {
            vec![None]
        } else {
            entry.voices.iter().map(Some).collect()
        };
        for voice in voices {
            let voice_id = voice.map(|voice| voice.voice.as_str());
            let matches = voice_id.and_then(|id| assets.get(&normalize_voice_id(id)));
            let voice_name = voice.and_then(|voice| voice.name.as_deref());
            let speaker = voice_name.map(|name| resolve_speaker(name, &aliases, &catalog));
            let speaker_conflict = voice_name
                .and_then(|name| aliases.source_speaker(name))
                .zip(matches)
                .is_some_and(|(canonical, asset)| canonical != asset.source_speaker);
            let normalization = entry
                .japanese_text
                .as_deref()
                .map(|raw| normalize_text(raw, &normalization_rules));
            let reason = match (
                &entry.rejection_reason,
                voice_id,
                matches,
                &speaker,
                &normalization,
            ) {
                (Some(RejectReason::MultiVoiceEvent), _, _, _, _) => "multi_voice_event",
                (Some(RejectReason::MissingJapaneseText), _, _, _, _) => "missing_japanese_text",
                (Some(_), _, _, _, _) => "unknown_event_schema",
                (None, Some(_), _, Some(Err(Error::UnresolvedSpeaker(_))), _) => {
                    "unresolved_speaker"
                }
                (None, Some(_), _, None, _) => "unresolved_speaker",
                (None, _, _, _, Some(Err(Error::MalformedMarkup(_)))) => "malformed_markup",
                (None, _, _, _, Some(Err(Error::UnknownMarkup(_)))) => "unknown_markup",
                (None, None, _, _, _) => "unvoiced_event",
                (None, Some(_), None, _, _) => "missing_audio",
                (None, Some(_), Some(_), Some(Ok(_)), _) if speaker_conflict => {
                    "conflicting_speaker"
                }
                _ => "",
            };
            let speaker_id = speaker
                .as_ref()
                .and_then(|speaker| speaker.as_ref().ok())
                .map(String::as_str)
                .unwrap_or("");
            let normalized = normalization
                .as_ref()
                .and_then(|normalized| normalized.as_ref().ok());
            let quality_flags = normalized
                .map(|normalized| quality_flags_cell(&normalized.quality_flags))
                .unwrap_or_default();
            let source_ogg = matches
                .map(|asset| portable_relative(&asset.source_ogg))
                .transpose()?
                .unwrap_or_default();
            writer.write_record([
                entry.source_event.source_scn.as_str(),
                &entry.source_event.scene_index.to_string(),
                &entry.source_event.scene_text_index.to_string(),
                &entry.source_event.scenario_event_index.to_string(),
                entry.speaker_name.as_deref().unwrap_or(""),
                voice_name.unwrap_or(""),
                matches.map_or("", |asset| asset.source_speaker.as_str()),
                speaker_id,
                entry.display_speaker_name.as_deref().unwrap_or(""),
                entry.japanese_text.as_deref().unwrap_or(""),
                normalized.map_or("", |text| text.display_text.as_str()),
                normalized.map_or("", |text| text.spoken_text.as_str()),
                normalized.map_or("", |text| text.normalization_version.as_str()),
                &quality_flags,
                voice_id.unwrap_or(""),
                &usize::from(matches.is_some()).to_string(),
                &source_ogg,
                reason,
            ])?;
        }
    }
    writer.flush().map_err(|source| Error::Io { path, source })
}

#[cfg(test)]
mod tests {
    use super::{
        LineStructureFingerprints, line_structure_config_from_bytes, valid_line_structure_config,
    };

    #[test]
    fn reviewed_structure_config_requires_exact_names_lengths_hashes_and_index_order() {
        let bytes = include_bytes!("../config/line-structure-fingerprints.json");
        let mut config: LineStructureFingerprints = serde_json::from_slice(bytes).unwrap();
        assert!(valid_line_structure_config(&config));
        assert!(line_structure_config_from_bytes(bytes, "lines").is_ok());

        config
            .scenes
            .get_mut("cd01.ks.scn")
            .unwrap()
            .push("0".repeat(64));
        assert!(!valid_line_structure_config(&config));
        config.scenes.get_mut("cd01.ks.scn").unwrap().pop();
        let old = config.scenes.remove("cd01.ks.scn").unwrap();
        assert!(!valid_line_structure_config(&config));
        config.scenes.insert("unreviewed.ks.scn".into(), old);
        assert!(!valid_line_structure_config(&config));
        let old = config.scenes.remove("unreviewed.ks.scn").unwrap();
        config.scenes.insert("cd01.ks.scn".into(), old);
        config.scenes.get_mut("cd01.ks.scn").unwrap()[0] = "A".repeat(64);
        assert!(!valid_line_structure_config(&config));
        let original: LineStructureFingerprints = serde_json::from_slice(bytes).unwrap();
        config.scenes.get_mut("cd01.ks.scn").unwrap()[0] =
            original.scenes["cd01.ks.scn"][1].clone();
        assert!(!valid_line_structure_config(&config));

        let mut reordered: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        reordered["scenes"]["cd01.ks.scn"]
            .as_array_mut()
            .unwrap()
            .swap(0, 1);
        let reordered = serde_json::to_vec(&reordered).unwrap();
        assert!(line_structure_config_from_bytes(&reordered, "lines").is_err());
        let mut padding = bytes.to_vec();
        padding.push(b' ');
        assert!(line_structure_config_from_bytes(&padding, "lines").is_err());
    }
}
