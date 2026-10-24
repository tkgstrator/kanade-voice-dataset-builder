use clap::Parser;
use kanade_tts::build::{BuildConfig, build_dataset, finalize_dataset};
use kanade_tts::cli::{Cli, Command};
use kanade_tts::inventory::inventory_voice_root;
use kanade_tts::paths::DatasetPaths;
use kanade_tts::probe::probe_audio;
use kanade_tts::readme::write_dataset_card;
use kanade_tts::review::{record_manual_review, review_required};
use kanade_tts::scenario::{
    extract_scenario_texts, load_psb_root, verify_locale_source, write_pilot_table,
};
use kanade_tts::speakers::build_speaker_catalog;
use kanade_tts::verify::{VerificationConfig, verify_dataset};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn main() {
    match Cli::parse().command {
        Command::Inspect { source, ffprobe } => {
            if let Err(error) = inspect(&source, &ffprobe) {
                eprintln!("inspect failed: {error}");
                std::process::exit(1);
            }
        }
        Command::Pilot {
            source,
            scenario,
            output,
        } => {
            if let Err(error) = pilot(&source, &scenario, &output) {
                eprintln!("pilot failed: {error}");
                std::process::exit(1);
            }
        }
        Command::Build {
            source,
            output,
            ffmpeg,
            ffprobe,
        } => {
            let config = BuildConfig {
                source_root: source,
                final_root: output,
                ffmpeg,
                ffprobe,
            };
            if let Err(error) = (|| {
                let built = build_dataset(&config)?;
                let checked = verify_dataset(
                    &built.work_root,
                    &config.source_root,
                    &VerificationConfig::default(),
                    &config.ffprobe,
                )?;
                write_dataset_card(&built.work_root, &checked)?;
                println!(
                    "verified work: {} accepted, {} rejected, {} orphan OGGs, {:.3} seconds; Card written to {}",
                    checked.accepted.len(),
                    checked.rejected_count,
                    checked.orphan_audio_count,
                    checked.duration_seconds,
                    built.work_root.join("README.md").display()
                );
                Ok::<_, kanade_tts::error::Error>(())
            })() {
                eprintln!("build failed: {error}");
                std::process::exit(1);
            }
        }
        Command::Review {
            source,
            output,
            ffprobe,
            utterance_id,
            decision,
            correct,
        } => {
            let config = BuildConfig {
                source_root: source,
                final_root: output,
                ffmpeg: PathBuf::new(),
                ffprobe,
            };
            let result = match (utterance_id, decision) {
                (Some(id), Some(decision)) => record_manual_review(&config, &id, decision, correct),
                (None, None) => review_required(&config),
                _ => unreachable!("clap requires utterance ID and decision together"),
            };
            match result {
                Ok(items) => {
                    let completed = items
                        .iter()
                        .filter(|item| !item.stale && item.decision.is_some())
                        .count();
                    let stale = items.iter().filter(|item| item.stale).count();
                    println!(
                        "manual review: {completed}/{} required samples decided; {stale} stale",
                        items.len()
                    );
                    for item in items {
                        println!(
                            "{}",
                            serde_json::to_string(&item).expect("review item serializes")
                        );
                    }
                }
                Err(error) => {
                    eprintln!("review failed: {error}");
                    std::process::exit(1);
                }
            }
        }
        Command::Verify {
            source,
            output,
            ffprobe,
        } => match verify_dataset(&output, &source, &VerificationConfig::default(), &ffprobe) {
            Ok(report) => println!(
                "verified: {} accepted, {} rejected, {} orphan OGGs, {} physical OGGs, {:.3} seconds",
                report.accepted.len(),
                report.rejected_count,
                report.orphan_audio_count,
                report.physical_audio_count,
                report.duration_seconds
            ),
            Err(error) => {
                eprintln!("verify failed: {error}");
                std::process::exit(1);
            }
        },
        Command::Finalize {
            source,
            output,
            ffprobe,
        } => {
            let config = BuildConfig {
                source_root: source,
                final_root: output,
                ffmpeg: PathBuf::new(),
                ffprobe,
            };
            if let Err(error) = finalize_dataset(&config) {
                eprintln!("finalize failed: {error}");
                std::process::exit(1);
            }
            println!("finalized {}", config.final_root.display());
        }
    }
}

fn pilot(source: &Path, scenario: &Path, output: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let paths = DatasetPaths::new(source, output)?;
    if scenario.components().count() != 1
        || scenario
            .extension()
            .is_none_or(|extension| extension != "scn")
    {
        return Err("scenario must be one .scn filename below the source scn directory".into());
    }
    verify_locale_source(&paths.source_root)?;
    let scenario_root = paths.source_root.join("scn");
    let scenario_path = scenario_root.join(scenario).canonicalize()?;
    if !scenario_path.starts_with(&scenario_root) {
        return Err("scenario resolves outside source scn directory".into());
    }
    let root = load_psb_root(&scenario_path)?;
    let entries = extract_scenario_texts(&root, &scenario_path)?;
    let inventory = inventory_voice_root(&paths.source_root.join("voice"))?;
    write_pilot_table(&entries, &inventory, &paths.output_root)?;
    println!(
        "{} scenario text events written to {}",
        entries.len(),
        paths.output_root.join("pilot.tsv").display()
    );
    Ok(())
}

fn inspect(source: &Path, ffprobe: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let inventory = inventory_voice_root(&source.join("voice"))?;
    let catalog = build_speaker_catalog(&inventory)?;
    let mut audio_probes = BTreeMap::new();
    for asset in &inventory.assets {
        let input = source.join("voice").join(&asset.source_ogg);
        audio_probes.insert(asset.voice_id.clone(), probe_audio(ffprobe, &input)?);
    }
    let mut formats = BTreeMap::<(String, u32, u16), usize>::new();
    for probe in audio_probes.values() {
        *formats
            .entry((probe.codec.clone(), probe.sample_rate, probe.channels))
            .or_default() += 1;
    }
    let scenario_root = source.join("scn");
    let mut scenario_count = 0;
    for entry in std::fs::read_dir(&scenario_root)? {
        if entry?
            .path()
            .extension()
            .is_some_and(|extension| extension == "scn")
        {
            scenario_count += 1;
        }
    }
    println!(
        "{} OGGs, {} sidecars, {} speakers, {} scenarios",
        inventory.assets.len(),
        inventory.sidecars.len(),
        catalog.speakers.len(),
        scenario_count
    );
    println!("{} audio files probed", audio_probes.len());
    for ((codec, sample_rate, channels), count) in formats {
        println!("{count} {codec} {sample_rate} Hz {channels} channel(s)");
    }
    Ok(())
}
