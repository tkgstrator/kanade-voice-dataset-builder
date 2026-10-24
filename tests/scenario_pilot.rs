use emote_psb::{psb::write::PsbWriter, value::PsbValue};
use kanade_tts::{
    inventory::{VoiceInventory, inventory_voice_root},
    join::{JoinInput, join_candidates},
    model::{QualityFlag, RejectReason, VoiceAsset, quality_flags_cell},
    scenario::{
        LocaleMap, extract_fixture_scenario_texts as extract_scenario_texts,
        extract_fixture_scenario_texts_with_map as extract_scenario_texts_with_map, load_psb_root,
        verify_locale_source, write_pilot_table,
    },
    speakers::{SpeakerAliases, build_speaker_catalog},
};
use std::{
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_FILE: AtomicUsize = AtomicUsize::new(0);

#[test]
fn quality_flag_cells_use_manifest_snake_case_for_nonempty_flags() {
    assert_eq!(
        quality_flags_cell(&[
            QualityFlag::PossibleMusicOrSfx,
            QualityFlag::VeryShortDuration
        ]),
        "possible_music_or_sfx;very_short_duration"
    );
    assert_eq!(quality_flags_cell(&[]), "");
    for flag in [
        QualityFlag::Singing,
        QualityFlag::NonVerbalVocalization,
        QualityFlag::VeryShortDuration,
        QualityFlag::VeryLongDuration,
        QualityFlag::PossibleMusicOrSfx,
        QualityFlag::SystemPrompt,
        QualityFlag::UnknownMarkup,
    ] {
        assert_eq!(
            format!("\"{}\"", quality_flags_cell(std::slice::from_ref(&flag))),
            serde_json::to_string(&flag).unwrap()
        );
    }
}

fn string(value: &str) -> PsbValue {
    PsbValue::String(value.into())
}
fn number(value: i64) -> PsbValue {
    PsbValue::Number(value.into())
}
fn object(fields: Vec<(&str, PsbValue)>) -> PsbValue {
    PsbValue::Object(
        fields
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect(),
    )
}
fn list(values: Vec<PsbValue>) -> PsbValue {
    PsbValue::List(values)
}

fn localized(speaker: &str, text: &str) -> PsbValue {
    list(vec![string(speaker), string(text), number(0)])
}
fn voice(id: &str) -> PsbValue {
    object(vec![
        ("name", string("カナデ")),
        ("pan", number(0)),
        ("type", number(0)),
        ("voice", string(id)),
    ])
}
fn event(localizations: Vec<PsbValue>, voices: PsbValue) -> PsbValue {
    list(vec![
        string("カナデ"),
        list(localizations),
        voices,
        number(0),
        object(vec![("data", list(vec![])), ("env", object(vec![]))]),
    ])
}
fn normal_event() -> PsbValue {
    event(
        vec![
            localized("少女", "「こんにちは」"),
            localized("Girl", "Hello"),
            localized("少女", "你好"),
            localized("少女", "你好啊"),
        ],
        list(vec![voice("KAN_scn00_01_001")]),
    )
}
fn root(languages: &[&str], events: Vec<PsbValue>) -> PsbValue {
    let event_count = events.len();
    let marker = list(vec![
        number(0),
        object(vec![("data", list(vec![])), ("env", object(vec![]))]),
        PsbValue::Null,
        number(0),
        number(0),
    ]);
    let lines = std::iter::once(marker.clone())
        .chain((0..event_count).map(|_| number(0)))
        .chain((0..(62 - event_count)).map(|_| list(vec![string("control")])))
        .chain(std::iter::once(marker))
        .collect();
    object(vec![
        ("name", string("fixture.ks")),
        (
            "languages",
            list(languages.iter().map(|lang| string(lang)).collect()),
        ),
        (
            "llmap",
            list(
                languages
                    .iter()
                    .map(|lang| {
                        object(vec![
                            ("name", string(&format!("fixture_{lang}.ks"))),
                            (
                                "*start",
                                list((0..event_count).map(|index| number(index as i64)).collect()),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
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
                ("texts", list(events)),
                ("title", list(vec![string(""); 4])),
                ("version", number(1)),
            ])]),
        ),
    ])
}
fn source() -> &'static Path {
    Path::new("fixture.ks.scn")
}

fn source_root() -> std::path::PathBuf {
    std::env::var_os("KANADE_SOURCE_ROOT")
        .map(std::path::PathBuf::from)
        .expect("KANADE_SOURCE_ROOT must point to lawfully extracted romfs/data")
}

fn set_line_maps(root: &mut PsbValue, values: &[i64]) {
    let PsbValue::Object(fields) = root else {
        unreachable!()
    };
    let Some(PsbValue::List(llmap)) = fields.get_mut("llmap") else {
        unreachable!()
    };
    for map in llmap {
        let PsbValue::Object(fields) = map else {
            unreachable!()
        };
        fields.insert(
            "*start".into(),
            list(values.iter().copied().map(number).collect()),
        );
    }
}

fn scene_lines(root: &mut PsbValue, scene_index: usize) -> &mut Vec<PsbValue> {
    let PsbValue::Object(root) = root else {
        unreachable!()
    };
    let PsbValue::List(scenes) = root.get_mut("scenes").unwrap() else {
        unreachable!()
    };
    let PsbValue::Object(scene) = &mut scenes[scene_index] else {
        unreachable!()
    };
    let PsbValue::List(lines) = scene.get_mut("lines").unwrap() else {
        unreachable!()
    };
    lines
}

fn line_mutation_error(root: &PsbValue, path: &Path) -> String {
    match extract_scenario_texts(root, path) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("unsupported line structure was accepted"),
    }
}

#[test]
fn normal_parser_rejects_fixture_named_extra_scenario() {
    let fixture = root(&["en", "cn", "tw"], vec![normal_event()]);
    let error = kanade_tts::scenario::extract_scenario_texts(&fixture, source()).unwrap_err();
    assert!(
        error.to_string().contains("unknown_event_schema"),
        "{error}"
    );
}

#[test]
#[ignore = "requires KANADE_SOURCE_ROOT pointing to lawfully extracted romfs/data"]
fn pinned_real_file_rejects_missing_trailing_scene_index() {
    let path = source_root().join("scn/scn00_01.ks.scn");
    let mut root = load_psb_root(&path).unwrap();
    let PsbValue::Object(fields) = &mut root else {
        unreachable!()
    };
    let PsbValue::List(scenes) = fields.get_mut("scenes").unwrap() else {
        unreachable!()
    };
    scenes.pop();
    let error = match kanade_tts::scenario::extract_scenario_texts(&root, &path) {
        Ok(_) => panic!("missing reviewed scene index was accepted"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("root.scenes"), "{error}");
}

#[test]
#[ignore = "requires KANADE_SOURCE_ROOT pointing to lawfully extracted romfs/data"]
fn scalar_and_one_element_list_lines_are_distinct() {
    let path = source_root().join("scn/cd01.ks.scn");
    let mut root = load_psb_root(&path).unwrap();
    let lines = scene_lines(&mut root, 0);
    let index = lines
        .iter()
        .position(|line| matches!(line, PsbValue::String(_)))
        .unwrap();
    let old = std::mem::replace(&mut lines[index], PsbValue::Null);
    lines[index] = list(vec![old]);
    assert!(line_mutation_error(&root, &path).contains("lines"));
}

#[test]
#[ignore = "requires KANADE_SOURCE_ROOT pointing to lawfully extracted romfs/data"]
fn nested_list_arity_and_member_positions_are_exact() {
    let path = source_root().join("scn/cd01.ks.scn");
    let original = load_psb_root(&path).unwrap();
    let mut longer = original.clone();
    let PsbValue::List(line) = &mut scene_lines(&mut longer, 0)[1] else {
        unreachable!()
    };
    let PsbValue::List(children) = &mut line[2] else {
        unreachable!()
    };
    children.push(PsbValue::Null);
    assert!(line_mutation_error(&longer, &path).contains("lines"));

    let mut reordered = original;
    let PsbValue::List(line) = &mut scene_lines(&mut reordered, 0)[1] else {
        unreachable!()
    };
    let PsbValue::List(children) = &mut line[2] else {
        unreachable!()
    };
    let first = children
        .iter()
        .position(|value| matches!(value, PsbValue::List(_)))
        .unwrap();
    let second = children
        .iter()
        .position(|value| matches!(value, PsbValue::Object(_)))
        .unwrap();
    children.swap(first, second);
    assert!(line_mutation_error(&reordered, &path).contains("lines"));
}

#[test]
#[ignore = "requires KANADE_SOURCE_ROOT pointing to lawfully extracted romfs/data"]
fn inner_data_records_reject_empty_and_malformed_lists() {
    let path = source_root().join("scn/cd01.ks.scn");
    let original = load_psb_root(&path).unwrap();
    for item in [list(vec![]), list(vec![PsbValue::Null])] {
        let mut root = original.clone();
        let PsbValue::List(marker) = &mut scene_lines(&mut root, 1)[0] else {
            unreachable!()
        };
        let PsbValue::Object(metadata) = &mut marker[1] else {
            unreachable!()
        };
        let PsbValue::List(data) = metadata.get_mut("data").unwrap() else {
            unreachable!()
        };
        data[0] = item;
        assert!(line_mutation_error(&root, &path).contains("lines"));
    }
}

#[test]
#[ignore = "requires KANADE_SOURCE_ROOT pointing to lawfully extracted romfs/data"]
fn individually_known_keys_cannot_form_unobserved_object_schema() {
    let path = source_root().join("scn/cd01.ks.scn");
    let mut root = load_psb_root(&path).unwrap();
    let PsbValue::List(line) = &mut scene_lines(&mut root, 0)[1] else {
        unreachable!()
    };
    let PsbValue::Object(fields) = &mut line[4] else {
        unreachable!()
    };
    fields.insert("wait".into(), number(0));
    assert!(line_mutation_error(&root, &path).contains("lines"));
}

#[test]
#[ignore = "requires KANADE_SOURCE_ROOT pointing to lawfully extracted romfs/data"]
fn line_environment_contents_remain_open_with_exact_containing_schema() {
    let path = source_root().join("scn/cd01.ks.scn");
    let mut root = load_psb_root(&path).unwrap();
    let PsbValue::List(marker) = &mut scene_lines(&mut root, 0)[0] else {
        unreachable!()
    };
    let PsbValue::Object(metadata) = &mut marker[1] else {
        unreachable!()
    };
    let PsbValue::Object(env) = metadata.get_mut("env").unwrap() else {
        unreachable!()
    };
    env.insert(
        "runtimeField".into(),
        list(vec![number(1), string("value")]),
    );
    assert_eq!(extract_scenario_texts(&root, &path).unwrap().len(), 61);
}

#[test]
fn runtime_locale_mapping_config_fingerprints_all_four_game_scripts() {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../config/locale-map.json")).unwrap();
    assert_eq!(config["schema_version"], "kanade-scenario-locale-v2");
    assert_eq!(config["runtime_base_index"], 0);
    let files = config["source_files"].as_array().unwrap();
    assert_eq!(files.len(), 4);
    for (index, (path, hash)) in [
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
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(files[index]["path"], *path);
        assert_eq!(files[index]["sha256"], *hash);
    }
}

#[test]
#[ignore = "requires KANADE_SOURCE_ROOT pointing to lawfully extracted romfs/data"]
fn locale_source_check_detects_runtime_script_change() {
    let input = source_root();
    let scratch = std::env::temp_dir().join(format!(
        "kanade-tts-runtime-{}-{}",
        std::process::id(),
        NEXT_FILE.fetch_add(1, Ordering::Relaxed)
    ));
    for relative in [
        "main/default.tjs",
        "system/StorageData.tjs",
        "system/MainWindow.tjs",
        "sysscn/multilang.tjs",
    ] {
        let dst = scratch.join(relative);
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        std::fs::copy(input.join(relative), &dst).unwrap();
    }
    assert!(verify_locale_source(&scratch).is_ok());
    std::fs::write(scratch.join("system/StorageData.tjs"), b"altered runtime").unwrap();
    let result = verify_locale_source(&scratch);
    std::fs::remove_dir_all(&scratch).unwrap();
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("fingerprint mismatch")
    );
}

#[test]
fn synthetic_psb_v3_round_trip_preserves_co_located_japanese_voice_and_indices() {
    let path = std::env::temp_dir().join(format!(
        "kanade-tts-scenario-{}-{}.psb",
        std::process::id(),
        NEXT_FILE.fetch_add(1, Ordering::Relaxed)
    ));
    let original = root(&["en", "cn", "tw"], vec![normal_event()]);
    PsbWriter::new(3, false, &original, std::fs::File::create(&path).unwrap())
        .unwrap()
        .finish()
        .unwrap();
    let decoded = load_psb_root(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let entries = extract_scenario_texts(&decoded, source()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].source_event.scenario_event_index, 0);
    assert_eq!(entries[0].source_event.scene_index, 0);
    assert_eq!(entries[0].source_event.scene_text_index, 0);
    assert_eq!(entries[0].speaker_name.as_deref(), Some("カナデ"));
    assert_eq!(entries[0].japanese_text.as_deref(), Some("「こんにちは」"));
    assert_eq!(entries[0].voices[0].voice, "KAN_scn00_01_001");
    assert_eq!(entries[0].rejection_reason, None);
}

#[test]
fn reordered_declared_translations_preserve_japanese_by_locale_identity() {
    let reordered = root(
        &["tw", "en", "cn"],
        vec![event(
            vec![
                localized("少女", "「こんにちは」"),
                localized("少女", "你好啊"),
                localized("Girl", "Hello"),
                localized("少女", "你好"),
            ],
            list(vec![voice("KAN_scn00_01_001")]),
        )],
    );
    let entries = extract_scenario_texts(&reordered, source()).unwrap();
    assert_eq!(entries[0].japanese_text.as_deref(), Some("「こんにちは」"));
}

#[test]
fn japanese_looking_translation_never_overrides_the_jp_tag() {
    let scenario = root(
        &["en", "cn", "tw"],
        vec![event(
            vec![
                localized("Narrator", "Plain ASCII base"),
                localized("Girl", "「日本語らしいが英語スロット」"),
                localized("少女", "你好"),
                localized("少女", "你好啊"),
            ],
            list(vec![voice("KAN_scn00_01_001")]),
        )],
    );
    let entry = &extract_scenario_texts(&scenario, source()).unwrap()[0];
    assert_eq!(entry.japanese_text.as_deref(), Some("Plain ASCII base"));
    assert_eq!(entry.display_speaker_name.as_deref(), Some("Narrator"));
}

#[test]
fn locale_map_rejects_missing_or_duplicate_japanese_declarations() {
    assert!(LocaleMap::from_tags(["en", "cn", "tw"]).is_err());
    assert!(LocaleMap::from_tags(["jp", "en", "jp", "tw"]).is_err());
}

#[test]
fn localized_cardinality_and_declaration_errors_are_stable() {
    let no_japanese = LocaleMap::from_tags(["en", "cn", "tw", "fr"]);
    assert!(no_japanese.is_err());
    let short = root(
        &["en", "cn", "tw"],
        vec![event(
            vec![
                localized("少女", "「こんにちは」"),
                localized("Girl", "Hello"),
            ],
            list(vec![voice("KAN_scn00_01_001")]),
        )],
    );
    assert!(
        extract_scenario_texts(&short, source())
            .unwrap_err()
            .to_string()
            .contains("locale_cardinality")
    );
    let non_japanese_looking_japanese = root(
        &["en", "cn", "tw"],
        vec![event(
            vec![
                localized("Girl", "Hello"),
                localized("少女", "「こんにちは」"),
                localized("少女", "你好"),
                localized("少女", "你好啊"),
            ],
            list(vec![voice("KAN_scn00_01_001")]),
        )],
    );
    let entries = extract_scenario_texts(&non_japanese_looking_japanese, source()).unwrap();
    assert_eq!(entries[0].japanese_text.as_deref(), Some("Hello"));
    let other_map = LocaleMap::from_tags(["jp", "tw", "en", "cn"]).unwrap();
    assert!(extract_scenario_texts_with_map(&short, source(), &other_map).is_err());
}

#[test]
fn translation_line_maps_must_match_text_count_but_may_differ() {
    let PsbValue::Object(mut fields) = root(&["en", "cn", "tw"], vec![normal_event()]) else {
        unreachable!()
    };
    let Some(PsbValue::List(llmap)) = fields.get_mut("llmap") else {
        unreachable!()
    };
    let PsbValue::Object(first) = &mut llmap[0] else {
        unreachable!()
    };
    first.insert("*start".into(), list(vec![number(0), number(1)]));
    let error = extract_scenario_texts(&PsbValue::Object(fields), source()).unwrap_err();
    assert!(error.to_string().contains("ambiguous_locale_declaration"));

    let PsbValue::Object(mut fields) = root(&["en", "cn", "tw"], vec![normal_event()]) else {
        unreachable!()
    };
    let Some(PsbValue::List(llmap)) = fields.get_mut("llmap") else {
        unreachable!()
    };
    let PsbValue::Object(second) = &mut llmap[1] else {
        unreachable!()
    };
    second.insert("*start".into(), list(vec![number(1)]));
    let entries = extract_scenario_texts(&PsbValue::Object(fields), source()).unwrap();
    assert_eq!(entries.len(), 1);
}

#[test]
fn independently_valid_translation_line_maps_may_differ() {
    let PsbValue::Object(mut fields) = root(&["en", "cn", "tw"], vec![normal_event()]) else {
        unreachable!()
    };
    let Some(PsbValue::List(llmap)) = fields.get_mut("llmap") else {
        unreachable!()
    };
    let PsbValue::Object(third) = &mut llmap[2] else {
        unreachable!()
    };
    third.insert("*start".into(), list(vec![number(1)]));
    let entries = extract_scenario_texts(&PsbValue::Object(fields), source()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].japanese_text.as_deref(), Some("「こんにちは」"));
}

#[test]
#[ignore = "requires KANADE_SOURCE_ROOT pointing to lawfully extracted romfs/data"]
fn real_branch_choice_and_six_slot_variants_preserve_source_indices() {
    for (file, expected) in [
        ("scn04_02.ks.scn", 132),
        ("scn05_04.ks.scn", 103),
        ("scn01_02.ks.scn", 0),
    ] {
        let path = source_root().join("scn").join(file);
        let root = load_psb_root(&path).unwrap();
        let entries = extract_scenario_texts(&root, &path).unwrap();
        if expected != 0 {
            assert_eq!(entries.len(), expected);
        }
        for (index, entry) in entries.iter().enumerate() {
            assert_eq!(entry.source_event.scenario_event_index, index);
        }
        if file == "scn05_04.ks.scn" {
            assert_eq!(
                (
                    entries[102].source_event.scene_index,
                    entries[102].source_event.scene_text_index
                ),
                (1, 0)
            );
            assert!(entries[102].appended_integer.is_some());
        }
        if file == "scn01_02.ks.scn" {
            assert_eq!(
                (
                    entries[44].source_event.scene_index,
                    entries[44].source_event.scene_text_index
                ),
                (0, 44)
            );
            assert!(entries[44].appended_integer.is_some());
            assert!(entries[45].appended_integer.is_some());
        }
    }
}

#[test]
fn six_slot_integer_and_metadata_key_types_are_typed_and_fail_closed() {
    let PsbValue::List(mut slots) = normal_event() else {
        unreachable!()
    };
    slots.push(number(27));
    let entries = extract_scenario_texts(
        &root(&["en", "cn", "tw"], vec![list(slots.clone())]),
        source(),
    )
    .unwrap();
    assert_eq!(entries[0].appended_integer, Some(27));
    slots[5] = string("27");
    assert!(
        extract_scenario_texts(
            &root(&["en", "cn", "tw"], vec![list(slots.clone())]),
            source()
        )
        .unwrap_err()
        .to_string()
        .contains("root.scenes[0].texts[0][5]")
    );
    slots.pop();
    for (key, value) in [
        ("unknown", number(1)),
        ("meswintype", number(1)),
        ("loopVoiceList", string("bad")),
    ] {
        let PsbValue::Object(meta) = &mut slots[4] else {
            unreachable!()
        };
        meta.insert(key.into(), value);
        let error = extract_scenario_texts(
            &root(&["en", "cn", "tw"], vec![list(slots.clone())]),
            source(),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("root.scenes[0].texts[0][4]"),
            "{error}"
        );
        let PsbValue::Object(meta) = &mut slots[4] else {
            unreachable!()
        };
        meta.remove(key);
    }
}

#[test]
#[ignore = "requires KANADE_SOURCE_ROOT pointing to lawfully extracted romfs/data"]
fn branch_label_maps_validate_each_locale_and_reject_unknown_labels() {
    let path = source_root().join("scn/scn05_04.ks.scn");
    let original = load_psb_root(&path).unwrap();
    for replacement in [
        list(vec![number(0), number(1)]),
        list(vec![number(-1), number(2), number(3)]),
        list(vec![number(0), number(0), number(1)]),
    ] {
        let mut root = original.clone();
        let PsbValue::Object(fields) = &mut root else {
            unreachable!()
        };
        let PsbValue::List(maps) = fields.get_mut("llmap").unwrap() else {
            unreachable!()
        };
        let PsbValue::Object(first) = &mut maps[0] else {
            unreachable!()
        };
        first.insert("*root2".into(), replacement);
        let error = extract_scenario_texts(&root, &path).unwrap_err();
        assert!(
            error.to_string().contains("root.llmap[0].*root2"),
            "{error}"
        );
    }
    let mut root = original;
    let PsbValue::Object(fields) = &mut root else {
        unreachable!()
    };
    let PsbValue::List(maps) = fields.get_mut("llmap").unwrap() else {
        unreachable!()
    };
    let PsbValue::Object(first) = &mut maps[0] else {
        unreachable!()
    };
    first.insert("*unexpected".into(), list(vec![number(1)]));
    assert!(
        extract_scenario_texts(&root, &path)
            .unwrap_err()
            .to_string()
            .contains("root.llmap[0]")
    );
}

#[test]
#[ignore = "requires KANADE_SOURCE_ROOT pointing to lawfully extracted romfs/data"]
fn malformed_trailing_lines_and_range_inflation_fail_at_the_line_index() {
    let path = source_root().join("scn/cd01.ks.scn");
    let mut real = load_psb_root(&path).unwrap();
    let PsbValue::Object(root_fields) = &mut real else {
        unreachable!()
    };
    let PsbValue::List(scenes) = root_fields.get_mut("scenes").unwrap() else {
        unreachable!()
    };
    let PsbValue::Object(end) = &mut scenes[1] else {
        unreachable!()
    };
    let PsbValue::List(lines) = end.get_mut("lines").unwrap() else {
        unreachable!()
    };
    lines.push(string("malformed trailing control line"));
    let error = match extract_scenario_texts(&real, &path) {
        Err(error) => error,
        Ok(_) => panic!("malformed trailing line must fail"),
    };
    assert!(
        error.to_string().contains("root.scenes[1].lines[1]"),
        "{error}"
    );

    let mut inflated = root(&["en", "cn", "tw"], vec![normal_event()]);
    let PsbValue::Object(root) = &mut inflated else {
        unreachable!()
    };
    let PsbValue::List(maps) = root.get_mut("llmap").unwrap() else {
        unreachable!()
    };
    let PsbValue::Object(first) = &mut maps[0] else {
        unreachable!()
    };
    first.insert("*start".into(), list(vec![number(64)]));
    let PsbValue::List(scenes) = root.get_mut("scenes").unwrap() else {
        unreachable!()
    };
    let PsbValue::Object(scene) = &mut scenes[0] else {
        unreachable!()
    };
    let PsbValue::List(lines) = scene.get_mut("lines").unwrap() else {
        unreachable!()
    };
    lines.push(string("inflation"));
    let error = match extract_scenario_texts(&inflated, source()) {
        Err(error) => error,
        Ok(_) => panic!("range inflation must fail"),
    };
    assert!(
        error.to_string().contains("root.scenes[0].lines[64]"),
        "{error}"
    );
}

#[test]
#[ignore = "requires KANADE_SOURCE_ROOT pointing to lawfully extracted romfs/data"]
fn nested_line_metadata_rejects_unknown_keys_and_non_list_data_items() {
    let path = source_root().join("scn/cd01.ks.scn");
    let original = load_psb_root(&path).unwrap();
    let mut unknown = original.clone();
    let PsbValue::Object(root) = &mut unknown else {
        unreachable!()
    };
    let PsbValue::List(scenes) = root.get_mut("scenes").unwrap() else {
        unreachable!()
    };
    let PsbValue::Object(scene) = &mut scenes[0] else {
        unreachable!()
    };
    let PsbValue::List(lines) = scene.get_mut("lines").unwrap() else {
        unreachable!()
    };
    let PsbValue::List(line) = &mut lines[1] else {
        unreachable!()
    };
    let PsbValue::Object(metadata) = &mut line[4] else {
        unreachable!()
    };
    metadata.insert("unknownControl".into(), number(7));
    let error = match extract_scenario_texts(&unknown, &path) {
        Err(error) => error,
        Ok(_) => panic!("unknown nested line key must fail"),
    };
    assert!(
        error.to_string().contains("root.scenes[0].lines"),
        "{error}"
    );

    let mut bad_data = original;
    let PsbValue::Object(root) = &mut bad_data else {
        unreachable!()
    };
    let PsbValue::List(scenes) = root.get_mut("scenes").unwrap() else {
        unreachable!()
    };
    let PsbValue::Object(end) = &mut scenes[1] else {
        unreachable!()
    };
    let PsbValue::List(lines) = end.get_mut("lines").unwrap() else {
        unreachable!()
    };
    let PsbValue::List(marker) = &mut lines[0] else {
        unreachable!()
    };
    let PsbValue::Object(metadata) = &mut marker[1] else {
        unreachable!()
    };
    let PsbValue::List(data) = metadata.get_mut("data").unwrap() else {
        unreachable!()
    };
    data[0] = number(7);
    let error = match extract_scenario_texts(&bad_data, &path) {
        Err(error) => error,
        Ok(_) => panic!("non-list line data item must fail"),
    };
    assert!(
        error.to_string().contains("root.scenes[1].lines"),
        "{error}"
    );
}

#[test]
#[ignore = "requires KANADE_SOURCE_ROOT pointing to lawfully extracted romfs/data"]
fn malformed_control_files_and_choice_language_fail_at_nested_indices() {
    let path = source_root().join("scn/scn04_02.ks.scn");
    let original = load_psb_root(&path).unwrap();
    let mut bad_file = original.clone();
    let PsbValue::Object(root) = &mut bad_file else {
        unreachable!()
    };
    let PsbValue::List(scenes) = root.get_mut("scenes").unwrap() else {
        unreachable!()
    };
    let PsbValue::Object(choice) = &mut scenes[2] else {
        unreachable!()
    };
    let PsbValue::List(files) = choice.get_mut("files").unwrap() else {
        unreachable!()
    };
    files.push(number(7));
    let error = match extract_scenario_texts(&bad_file, &path) {
        Err(error) => error,
        Ok(_) => panic!("malformed file must fail"),
    };
    assert!(
        error.to_string().contains("root.scenes[2].files[0]"),
        "{error}"
    );

    let mut bad_language = original;
    let PsbValue::Object(root) = &mut bad_language else {
        unreachable!()
    };
    let PsbValue::List(scenes) = root.get_mut("scenes").unwrap() else {
        unreachable!()
    };
    let PsbValue::Object(choice) = &mut scenes[2] else {
        unreachable!()
    };
    let PsbValue::List(selects) = choice.get_mut("selects").unwrap() else {
        unreachable!()
    };
    let PsbValue::Object(first) = &mut selects[0] else {
        unreachable!()
    };
    let PsbValue::List(language) = first.get_mut("language").unwrap() else {
        unreachable!()
    };
    language[1] = number(7);
    let error = match extract_scenario_texts(&bad_language, &path) {
        Err(error) => error,
        Ok(_) => panic!("malformed choice language must fail"),
    };
    assert!(
        error
            .to_string()
            .contains("root.scenes[2].selects[0].language[1]"),
        "{error}"
    );
}

#[test]
#[ignore = "requires KANADE_SOURCE_ROOT pointing to lawfully extracted romfs/data"]
fn all_real_scenarios_parse_with_audited_event_accounting() {
    let mut files: Vec<_> = std::fs::read_dir(source_root().join("scn"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "scn"))
        .collect();
    files.sort();
    assert_eq!(files.len(), 43);
    let mut texts = 0;
    let mut refs = 0;
    let mut zero = 0;
    let mut multi = 0;
    let mut pairs = std::collections::BTreeSet::new();
    let mut ids = std::collections::BTreeMap::<String, usize>::new();
    for path in files {
        let entries =
            kanade_tts::scenario::extract_scenario_texts(&load_psb_root(&path).unwrap(), &path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let name = path.file_name().unwrap().to_str().unwrap();
        let first = &entries[0].source_event;
        assert_eq!(
            (
                first.scene_index,
                first.scene_text_index,
                first.scenario_event_index
            ),
            (0, 0, 0),
            "{name}"
        );
        assert_eq!(first.source_scn, path.to_string_lossy(), "{name}");
        let last = &entries.last().unwrap().source_event;
        if name == "scn05_04.ks.scn" {
            assert_eq!(
                (
                    entries.len(),
                    last.scene_index,
                    last.scene_text_index,
                    last.scenario_event_index
                ),
                (103, 1, 0, 102)
            );
        } else {
            assert_eq!(
                (
                    last.scene_index,
                    last.scene_text_index,
                    last.scenario_event_index
                ),
                (0, entries.len() - 1, entries.len() - 1),
                "{name}"
            );
        }
        for (filename, count) in [
            ("cd01.ks.scn", 61),
            ("scn00_01.ks.scn", 19),
            ("scn04_02.ks.scn", 132),
            ("scn06_04.ks.scn", 105),
        ] {
            if name == filename {
                assert_eq!(entries.len(), count, "{name}");
            }
        }
        for (index, entry) in entries.iter().enumerate() {
            assert_eq!(
                entry.source_event.scenario_event_index,
                index,
                "{}",
                path.display()
            );
            refs += entry.voices.len();
            zero += usize::from(entry.voices.is_empty());
            multi += usize::from(entry.voices.len() > 1);
            for voice in &entry.voices {
                assert!(voice.name.as_ref().is_some_and(|name| !name.is_empty()));
                pairs.insert((entry.speaker_name.clone(), voice.name.clone()));
                *ids.entry(voice.voice.clone()).or_default() += 1;
            }
        }
        texts += entries.len();
    }
    assert_eq!((texts, refs, zero, multi), (6412, 2650, 3798, 12));
    assert_eq!(pairs.len(), 62);
    assert_eq!(ids.len(), 2645);
    assert_eq!(ids.values().filter(|count| **count == 2).count(), 5);
    assert!(ids.values().all(|count| *count <= 2));
}

#[test]
#[ignore = "requires KANADE_SOURCE_ROOT pointing to lawfully extracted romfs/data"]
fn real_exact_join_preserves_repeated_provenance_and_rejects_multi_voice() {
    let source = source_root();
    let inventory = inventory_voice_root(&source.join("voice")).unwrap();
    assert_eq!(inventory.assets.len(), 2809);
    let catalog = build_speaker_catalog(&inventory).unwrap();
    let aliases = SpeakerAliases::kanade(&catalog).unwrap();
    let mut paths: Vec<_> = std::fs::read_dir(source.join("scn"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "scn"))
        .collect();
    paths.sort();
    let mut entries = Vec::new();
    for path in paths {
        entries.extend(
            kanade_tts::scenario::extract_scenario_texts(&load_psb_root(&path).unwrap(), &path)
                .unwrap(),
        );
    }
    let report = join_candidates(JoinInput {
        inventory: inventory.clone(),
        entries,
        catalog,
        aliases,
        source_root: source,
        fingerprint: "read-only-preflight".into(),
    });
    assert_eq!(
        report.accounted_physical_audio_count(),
        inventory.assets.len()
    );
    assert_eq!(
        (
            report.matched.len(),
            report.rejected.len(),
            report.orphan_audio.len(),
            report.missing_audio.len()
        ),
        (2596, 49, 164, 0)
    );
    assert!(
        report
            .rejected
            .iter()
            .any(|row| row.reason_code == RejectReason::MultiVoiceEvent)
    );
    assert!(
        report
            .rejected
            .iter()
            .all(|row| row.reason_code != RejectReason::UnknownEventSchema)
    );
    let repeated = report
        .matched
        .iter()
        .filter(|row| row.source_events.len() == 2)
        .count();
    let mut reasons = std::collections::BTreeMap::<String, usize>::new();
    for row in &report.rejected {
        *reasons.entry(format!("{:?}", row.reason_code)).or_default() += 1;
    }
    eprintln!(
        "join preflight: matched={} rejected={} orphan={} missing={} repeated_matched={repeated}",
        report.matched.len(),
        report.rejected.len(),
        report.orphan_audio.len(),
        report.missing_audio.len()
    );
    eprintln!("join rejection reasons: {reasons:?}");
    assert!(report.matched.iter().any(|row| {
        row.source_observations
            .iter()
            .any(|observation| observation.scenario_speaker_name.is_none())
    }));
    assert!(report.matched.iter().any(|row| {
        row.source_observations
            .iter()
            .any(|observation| observation.appended_integer.is_some())
    }));
    assert_eq!(repeated, 4);
    assert_eq!(reasons.get("MultiVoiceEvent"), Some(&48));
    assert_eq!(reasons.get("ConflictingText"), Some(&1));
    assert_eq!(
        report
            .rejected
            .iter()
            .find(|row| row.reason_code == RejectReason::ConflictingText)
            .unwrap()
            .source_events
            .len(),
        2
    );
}

#[test]
fn line_maps_reject_nonmonotonic_negative_and_out_of_range_indices() {
    for indices in [&[4, 3][..], &[-1, 2][..], &[0, 65][..]] {
        let mut scenario = root(&["en", "cn", "tw"], vec![normal_event(), normal_event()]);
        set_line_maps(&mut scenario, indices);
        let error = extract_scenario_texts(&scenario, source()).unwrap_err();
        assert!(
            error.to_string().contains("ambiguous_locale_declaration"),
            "{error}"
        );
    }
}

#[test]
#[ignore = "requires KANADE_SOURCE_ROOT pointing to lawfully extracted romfs/data"]
fn only_the_observed_non_text_end_scene_may_be_skipped() {
    let path = source_root().join("scn/scn00_01.ks.scn");
    let mut root = load_psb_root(&path).unwrap();
    let PsbValue::Object(fields) = &mut root else {
        unreachable!()
    };
    let Some(PsbValue::List(scenes)) = fields.get_mut("scenes") else {
        unreachable!()
    };
    let PsbValue::Object(end) = &mut scenes[1] else {
        unreachable!()
    };
    assert_eq!(end.len(), 8);
    assert!(matches!(end.get("label"), Some(PsbValue::String(value)) if value == "*end"));
    assert!(matches!(end.get("files"), Some(PsbValue::List(value)) if value.is_empty()));
    assert!(matches!(end.get("lines"), Some(PsbValue::List(value)) if value.len() == 1));
    assert!(matches!(end.get("nexts"), Some(PsbValue::List(value)) if value.len() == 1));
    assert!(matches!(end.get("title"), Some(PsbValue::List(value)) if value.len() == 4));
    assert_eq!(extract_scenario_texts(&root, &path).unwrap().len(), 19);
    let PsbValue::Object(fields) = &mut root else {
        unreachable!()
    };
    let Some(PsbValue::List(scenes)) = fields.get_mut("scenes") else {
        unreachable!()
    };
    let PsbValue::Object(end) = &mut scenes[1] else {
        unreachable!()
    };
    end.insert("unknown".into(), number(7));
    let error = extract_scenario_texts(&root, &path).unwrap_err();
    assert!(error.to_string().contains("unknown_event_schema"));
}

#[test]
fn missing_scenes_texts_and_unknown_event_shapes_are_rejected() {
    let PsbValue::Object(mut fields) = root(&["en", "cn", "tw"], vec![]) else {
        unreachable!()
    };
    fields.remove("scenes");
    let missing_scenes = PsbValue::Object(fields);
    assert!(
        extract_scenario_texts(&missing_scenes, source())
            .unwrap_err()
            .to_string()
            .contains("missing_scenes")
    );
    let missing_texts = root(&["en", "cn", "tw"], vec![]);
    let PsbValue::Object(mut fields) = missing_texts else {
        unreachable!()
    };
    fields.insert(
        "scenes".into(),
        list(vec![object(vec![("label", string("*start"))])]),
    );
    assert!(
        extract_scenario_texts(&PsbValue::Object(fields), source())
            .unwrap_err()
            .to_string()
            .contains("unknown_event_schema")
    );
    let unknown = root(&["en", "cn", "tw"], vec![string("unrelated Japanese text")]);
    assert!(
        extract_scenario_texts(&unknown, source())
            .unwrap_err()
            .to_string()
            .contains("unknown_event_schema")
    );
}

#[test]
fn null_empty_multi_and_malformed_voices_have_stable_results() {
    for voices in [PsbValue::Null, list(vec![])] {
        let entries = extract_scenario_texts(
            &root(
                &["en", "cn", "tw"],
                vec![event(
                    vec![
                        localized("少女", "「こんにちは」"),
                        localized("Girl", "Hello"),
                        localized("少女", "你好"),
                        localized("少女", "你好啊"),
                    ],
                    voices,
                )],
            ),
            source(),
        )
        .unwrap();
        assert!(entries[0].voices.is_empty());
        assert_eq!(entries[0].rejection_reason, None);
    }
    let multi = root(
        &["en", "cn", "tw"],
        vec![event(
            vec![
                localized("少女", "「こんにちは」"),
                localized("Girl", "Hello"),
                localized("少女", "你好"),
                localized("少女", "你好啊"),
            ],
            list(vec![voice("A"), voice("B")]),
        )],
    );
    assert_eq!(
        extract_scenario_texts(&multi, source()).unwrap()[0].rejection_reason,
        Some(RejectReason::MultiVoiceEvent)
    );
    let malformed = root(
        &["en", "cn", "tw"],
        vec![event(
            vec![
                localized("少女", "「こんにちは」"),
                localized("Girl", "Hello"),
                localized("少女", "你好"),
                localized("少女", "你好啊"),
            ],
            list(vec![object(vec![("voice", number(1))])]),
        )],
    );
    assert!(
        extract_scenario_texts(&malformed, source())
            .unwrap_err()
            .to_string()
            .contains("malformed_voice_object")
    );
}

#[test]
#[ignore = "requires KANADE_SOURCE_ROOT pointing to lawfully extracted romfs/data"]
fn real_pilot_psb_has_structurally_co_located_first_and_last_voice() {
    let path = source_root().join("scn/scn00_01.ks.scn");
    let root = load_psb_root(&path).unwrap();
    let entries = extract_scenario_texts(&root, &path).unwrap();
    assert_eq!(entries.len(), 19);
    assert_eq!(entries[0].voices[0].voice, "KAN_scn00_01_001");
    assert_eq!(entries[18].voices[0].voice, "KAN_scn00_01_014");
    assert_eq!(entries[18].source_event.scenario_event_index, 18);
    for entry in entries.iter().filter(|entry| !entry.voices.is_empty()) {
        assert!(
            entry
                .speaker_name
                .as_ref()
                .is_some_and(|name| !name.is_empty())
        );
        assert!(
            entry
                .japanese_text
                .as_ref()
                .is_some_and(|text| !text.is_empty())
        );
    }
}

#[test]
#[ignore = "requires KANADE_SOURCE_ROOT pointing to lawfully extracted romfs/data"]
fn pilot_command_writes_auditable_tsv_without_audio() {
    let output = std::env::temp_dir().join(format!(
        "kanade-tts-pilot-{}-{}",
        std::process::id(),
        NEXT_FILE.fetch_add(1, Ordering::Relaxed)
    ));
    let source = source_root();
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_kanade-tts"))
        .args(["pilot", "--scenario", "scn00_01.ks.scn", "--output"])
        .arg("--source")
        .arg(&source)
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let table = std::fs::read_to_string(output.join("pilot.tsv")).unwrap();
    assert!(table.contains("KAN_scn00_01_001"));
    assert!(table.contains("KAN_scn00_01_014"));
    assert!(table.contains("japanese_raw_text"));
    assert!(table.contains("japanese_display_text"));
    assert!(table.contains("japanese_spoken_text"));
    assert!(table.contains("normalization_version"));
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(b'\t')
        .from_reader(table.as_bytes());
    let rows: Vec<csv::StringRecord> = reader.records().map(Result::unwrap).collect();
    assert_eq!(rows.len(), 19);
    let headers = reader.headers().unwrap().clone();
    let column = |name: &str| headers.iter().position(|header| header == name).unwrap();
    assert_eq!(
        rows.iter()
            .filter(|row| row.get(column("rejection_reason")) == Some("unvoiced_event"))
            .count(),
        5
    );
    assert_eq!(
        rows.iter()
            .filter(|row| {
                row.get(column("voice_id")).is_some_and(|id| !id.is_empty())
                    && row.get(column("source_ogg_match_count")) == Some("1")
            })
            .count(),
        14
    );
    assert_eq!(rows[0].get(column("speaker_id")), Some("kan"));
    for row in [&rows[0], &rows[16], &rows[18]] {
        assert!(
            row.get(column("japanese_raw_text"))
                .is_some_and(|text| !text.is_empty())
        );
        assert!(
            row.get(column("japanese_display_text"))
                .is_some_and(|text| !text.is_empty())
        );
        assert!(
            row.get(column("japanese_spoken_text"))
                .is_some_and(|text| !text.is_empty())
        );
    }
    assert_eq!(std::fs::read_dir(&output).unwrap().count(), 1);
    std::fs::remove_dir_all(output).unwrap();
}

#[cfg(unix)]
#[test]
fn pilot_table_rejects_an_existing_symlink_destination() {
    let base = std::env::temp_dir().join(format!(
        "kanade-tts-pilot-link-{}-{}",
        std::process::id(),
        NEXT_FILE.fetch_add(1, Ordering::Relaxed)
    ));
    let out = base.join("output");
    std::fs::create_dir_all(&out).unwrap();
    let victim = base.join("victim.txt");
    std::fs::write(&victim, "keep").unwrap();
    std::os::unix::fs::symlink(&victim, out.join("pilot.tsv")).unwrap();
    let inventory = VoiceInventory {
        assets: vec![],
        sidecars: vec![],
        speakers: vec![],
    };
    assert!(write_pilot_table(&[], &inventory, &out).is_err());
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn pilot_table_fails_closed_for_duplicate_inventory_ids() {
    let entry =
        extract_scenario_texts(&root(&["en", "cn", "tw"], vec![normal_event()]), source()).unwrap();
    let inventory = VoiceInventory {
        assets: ["one/KAN_scn00_01_001.ogg", "two/KAN_scn00_01_001.ogg"]
            .into_iter()
            .map(|path| VoiceAsset {
                source_ogg: path.into(),
                voice_id: "kan_scn00_01_001".into(),
                source_speaker: "カナデ".into(),
                byte_size: 1,
                source_sha256: "stub".into(),
            })
            .collect(),
        sidecars: vec![],
        speakers: vec!["カナデ".into()],
    };
    let output = std::env::temp_dir().join(format!(
        "kanade-tts-duplicate-{}-{}",
        std::process::id(),
        NEXT_FILE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = write_pilot_table(&entry, &inventory, &output);
    if output.exists() {
        std::fs::remove_dir_all(&output).unwrap();
    }
    assert!(result.unwrap_err().to_string().contains("collides"));
}

#[test]
fn pilot_flags_exact_voice_speaker_and_physical_directory_conflict() {
    let entries =
        extract_scenario_texts(&root(&["en", "cn", "tw"], vec![normal_event()]), source()).unwrap();
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../config/speaker-aliases.json")).unwrap();
    let speakers: Vec<String> = config["aliases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|alias| alias["source_speaker"].as_str().unwrap().to_owned())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let inventory = VoiceInventory {
        assets: vec![VoiceAsset {
            source_ogg: "伊織/KAN_scn00_01_001.ogg".into(),
            voice_id: "KAN_scn00_01_001".into(),
            source_speaker: "伊織".into(),
            byte_size: 1,
            source_sha256: "stub".into(),
        }],
        sidecars: vec![],
        speakers,
    };
    let output = std::env::temp_dir().join(format!(
        "kanade-tts-pilot-conflict-{}-{}",
        std::process::id(),
        NEXT_FILE.fetch_add(1, Ordering::Relaxed)
    ));
    write_pilot_table(&entries, &inventory, &output).unwrap();
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(b'\t')
        .from_path(output.join("pilot.tsv"))
        .unwrap();
    let headers = reader.headers().unwrap().clone();
    let column = |name: &str| headers.iter().position(|header| header == name).unwrap();
    let row = reader.records().next().unwrap().unwrap();
    assert_eq!(row.get(column("scenario_speaker")), Some("カナデ"));
    assert_eq!(row.get(column("display_speaker")), Some("少女"));
    assert_eq!(row.get(column("voice_meta_name")), Some("カナデ"));
    assert_eq!(row.get(column("source_speaker")), Some("伊織"));
    assert_eq!(
        row.get(column("rejection_reason")),
        Some("conflicting_speaker")
    );
    std::fs::remove_dir_all(output).unwrap();
}

#[test]
#[ignore = "requires KANADE_SOURCE_ROOT pointing to lawfully extracted romfs/data"]
fn voiced_row_without_scenario_speaker_uses_exact_voice_metadata_name() {
    let scenario_path = source_root().join("scn/scn00_01.ks.scn");
    let root = load_psb_root(&scenario_path).unwrap();
    let mut entries = extract_scenario_texts(&root, &scenario_path).unwrap();
    entries[0].speaker_name = None;
    let inventory = inventory_voice_root(&source_root().join("voice")).unwrap();
    let output = std::env::temp_dir().join(format!(
        "kanade-tts-missing-speaker-{}-{}",
        std::process::id(),
        NEXT_FILE.fetch_add(1, Ordering::Relaxed)
    ));
    write_pilot_table(&entries[..1], &inventory, &output).unwrap();
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(b'\t')
        .from_path(output.join("pilot.tsv"))
        .unwrap();
    let headers = reader.headers().unwrap().clone();
    let column = |name: &str| headers.iter().position(|header| header == name).unwrap();
    let row = reader.records().next().unwrap().unwrap();
    assert_eq!(row.get(column("voice_id")), Some("KAN_scn00_01_001"));
    assert_eq!(row.get(column("speaker_id")), Some("kan"));
    assert_eq!(row.get(column("rejection_reason")), Some(""));
    assert_eq!(row.get(column("display_speaker")), Some("銀髪の少女"));
    std::fs::remove_dir_all(output).unwrap();
}
