use crate::review::ReviewDecision;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "kanade-tts", about = "Prepare an auditable KANADE TTS dataset")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Inspect {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        ffprobe: PathBuf,
    },
    Pilot {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        scenario: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    Build {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        ffmpeg: PathBuf,
        #[arg(long)]
        ffprobe: PathBuf,
    },
    Review {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        ffprobe: PathBuf,
        #[arg(long, requires = "decision")]
        utterance_id: Option<String>,
        #[arg(long, requires = "utterance_id")]
        decision: Option<ReviewDecision>,
        #[arg(long, requires = "utterance_id")]
        correct: bool,
    },
    Finalize {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        ffprobe: PathBuf,
    },
    Verify {
        #[arg(long)]
        source: PathBuf,
        #[arg(long, alias = "dataset")]
        output: PathBuf,
        #[arg(long)]
        ffprobe: PathBuf,
    },
}
