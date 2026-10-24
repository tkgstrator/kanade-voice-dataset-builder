use crate::error::{Error, Result};
use crate::inventory::{VoiceInventory, normalize_voice_id};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpeakerCatalog {
    pub speakers: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpeakerAliases {
    aliases: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct AliasConfig {
    schema_version: String,
    aliases: Vec<AliasEntry>,
}

#[derive(Debug, Deserialize)]
struct AliasEntry {
    alias: String,
    source_speaker: String,
}

impl SpeakerAliases {
    pub fn from_json(input: &str, catalog: &SpeakerCatalog) -> Result<Self> {
        let config: AliasConfig = serde_json::from_str(input)
            .map_err(|error| Error::SpeakerAliasConfig(error.to_string()))?;
        if config.schema_version != "kanade-speaker-aliases-v1" {
            return Err(Error::SpeakerAliasConfig(
                "unsupported schema version".into(),
            ));
        }
        let mut aliases = BTreeMap::new();
        for entry in config.aliases {
            let alias = entry.alias.nfc().collect::<String>();
            let target = entry.source_speaker.nfc().collect::<String>();
            if alias.is_empty() {
                return Err(Error::SpeakerAliasConfig("empty alias".into()));
            }
            if !catalog.speakers.contains_key(&target) {
                return Err(Error::SpeakerAliasConfig(format!(
                    "alias {alias} targets unknown source speaker {target}"
                )));
            }
            if aliases.insert(alias.clone(), target).is_some() {
                return Err(Error::SpeakerAliasConfig(format!(
                    "duplicate normalized alias {alias}"
                )));
            }
        }
        Ok(Self { aliases })
    }

    pub fn kanade(catalog: &SpeakerCatalog) -> Result<Self> {
        Self::from_json(include_str!("../config/speaker-aliases.json"), catalog)
    }

    pub fn source_speaker(&self, alias: &str) -> Option<&str> {
        let alias = alias.nfc().collect::<String>();
        self.aliases.get(&alias).map(String::as_str)
    }
}

pub fn resolve_speaker(
    name: &str,
    aliases: &SpeakerAliases,
    catalog: &SpeakerCatalog,
) -> Result<crate::model::SpeakerId> {
    let normalized = name.nfc().collect::<String>();
    let source_speaker = aliases
        .aliases
        .get(&normalized)
        .ok_or_else(|| Error::UnresolvedSpeaker(name.to_owned()))?;
    catalog
        .speakers
        .get(source_speaker)
        .cloned()
        .ok_or_else(|| Error::UnresolvedSpeaker(name.to_owned()))
}

pub fn build_speaker_catalog(inventory: &VoiceInventory) -> Result<SpeakerCatalog> {
    let mut prefixes = BTreeMap::<String, BTreeMap<String, usize>>::new();
    for asset in &inventory.assets {
        let prefix = asset.voice_id.split('_').next().unwrap_or("");
        let ascii = prefix
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>();
        let prefix = if ascii.is_empty() {
            "speaker".into()
        } else {
            ascii
        };
        *prefixes
            .entry(asset.source_speaker.clone())
            .or_default()
            .entry(prefix)
            .or_default() += 1;
    }

    let mut base = BTreeMap::new();
    for speaker in &inventory.speakers {
        let counts = prefixes.get(speaker);
        let dominant = counts
            .and_then(|counts| {
                counts
                    .iter()
                    .max_by(|(left_prefix, left_count), (right_prefix, right_count)| {
                        left_count
                            .cmp(right_count)
                            .then_with(|| right_prefix.cmp(left_prefix))
                    })
                    .map(|(prefix, _)| prefix.clone())
            })
            .unwrap_or_else(|| "speaker".into());
        base.insert(speaker.clone(), dominant);
    }
    let mut occurrences = BTreeMap::<String, usize>::new();
    for prefix in base.values() {
        *occurrences.entry(prefix.clone()).or_default() += 1;
    }
    let mut speakers = BTreeMap::new();
    for (speaker, prefix) in base {
        let id = if occurrences[&prefix] == 1 {
            prefix
        } else {
            let digest = Sha256::digest(normalize_voice_id(&speaker).as_bytes());
            format!("{}-{}", prefix, &format!("{digest:x}")[..8])
        };
        speakers.insert(speaker, id);
    }
    if speakers.values().collect::<BTreeSet<_>>().len() != speakers.len() {
        return Err(Error::SpeakerIdCollision("short hash collision".into()));
    }
    Ok(SpeakerCatalog { speakers })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::inventory_voice_root;

    fn catalog(entries: &[(&str, &str)]) -> SpeakerCatalog {
        SpeakerCatalog {
            speakers: entries
                .iter()
                .map(|(name, id)| ((*name).to_owned(), (*id).to_owned()))
                .collect(),
        }
    }

    #[test]
    fn exact_nfc_alias_resolves_without_changing_identity() {
        let catalog = catalog(&[("ガ", "ga"), ("カナデ", "kan")]);
        let aliases = SpeakerAliases::from_json(
            r#"{"schema_version":"kanade-speaker-aliases-v1","aliases":[{"alias":"カ\u3099","source_speaker":"カ\u3099"},{"alias":"銀髪の少女","source_speaker":"カナデ"}]}"#,
            &catalog,
        )
        .unwrap();
        assert_eq!(resolve_speaker("ガ", &aliases, &catalog).unwrap(), "ga");
        assert_eq!(
            resolve_speaker("銀髪の少女", &aliases, &catalog).unwrap(),
            "kan"
        );
    }

    #[test]
    fn absent_substring_and_transliteration_aliases_are_not_inferred() {
        let catalog = catalog(&[("カナデ", "kan")]);
        let aliases = SpeakerAliases::from_json(
            r#"{"schema_version":"kanade-speaker-aliases-v1","aliases":[{"alias":"カナデ","source_speaker":"カナデ"},{"alias":"銀髪の少女","source_speaker":"カナデ"}]}"#,
            &catalog,
        )
        .unwrap();
        assert!(resolve_speaker("少女", &aliases, &catalog).is_err());
        assert!(resolve_speaker("Kanade", &aliases, &catalog).is_err());
        assert!(resolve_speaker(" カナデ ", &aliases, &catalog).is_err());
    }

    #[test]
    fn duplicate_alias_targets_and_unknown_targets_fail_loading() {
        let catalog = catalog(&[("カナデ", "kan"), ("伊織", "io")]);
        let duplicate = r#"{"schema_version":"kanade-speaker-aliases-v1","aliases":[{"alias":"少女","source_speaker":"カナデ"},{"alias":"少女","source_speaker":"伊織"}]}"#;
        assert!(SpeakerAliases::from_json(duplicate, &catalog).is_err());
        let unknown = r#"{"schema_version":"kanade-speaker-aliases-v1","aliases":[{"alias":"少女","source_speaker":"不存在"}]}"#;
        assert!(SpeakerAliases::from_json(unknown, &catalog).is_err());
    }

    #[test]
    #[ignore = "requires KANADE_SOURCE_ROOT pointing to lawfully extracted romfs/data"]
    fn frozen_aliases_cover_all_38_source_speakers_exactly() {
        let root = std::env::var_os("KANADE_SOURCE_ROOT")
            .map(std::path::PathBuf::from)
            .expect("KANADE_SOURCE_ROOT must point to lawfully extracted romfs/data")
            .join("voice");
        let inventory = inventory_voice_root(&root).unwrap();
        let catalog = build_speaker_catalog(&inventory).unwrap();
        let aliases = SpeakerAliases::kanade(&catalog).unwrap();
        assert_eq!(catalog.speakers.len(), 38);
        for (source_speaker, speaker_id) in &catalog.speakers {
            assert_eq!(
                resolve_speaker(source_speaker, &aliases, &catalog).unwrap(),
                *speaker_id,
                "missing exact self-alias for {source_speaker}"
            );
        }
        assert_eq!(aliases.source_speaker("銀髪の少女"), Some("カナデ"));
    }
}
