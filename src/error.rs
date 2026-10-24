use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("directory traversal failed: {0}")]
    Walk(#[from] walkdir::Error),
    #[error("invalid Unicode path: {0}")]
    InvalidUnicodePath(PathBuf),
    #[error("normalized speaker name collision: {first} and {second}")]
    SpeakerNameCollision { first: String, second: String },
    #[error("normalized voice ID {voice_id} collides: {first} and {second}")]
    VoiceIdCollision {
        voice_id: String,
        first: PathBuf,
        second: PathBuf,
    },
    #[error("audio probe failed for {input}: {detail}")]
    AudioProbe { input: PathBuf, detail: String },
    #[error("speaker ID collision: {0}")]
    SpeakerIdCollision(String),
    #[error("speaker alias configuration error: {0}")]
    SpeakerAliasConfig(String),
    #[error("unresolved speaker alias: {0}")]
    UnresolvedSpeaker(String),
    #[error("malformed inline markup: {0}")]
    MalformedMarkup(String),
    #[error("unknown inline markup: {0}")]
    UnknownMarkup(String),
    #[error("cannot resolve path {path}: {source}")]
    PathResolution {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("source root is not a directory: {0}")]
    SourceNotDirectory(PathBuf),
    #[error("output root {output} is inside source root {source_root}", output = output.display(), source_root = source_root.display())]
    OutputInsideSource {
        source_root: PathBuf,
        output: PathBuf,
    },
    #[error("PSB error at {path}: {detail}")]
    Psb { path: PathBuf, detail: String },
    #[error("scenario schema {code} at {path}")]
    ScenarioSchema { code: &'static str, path: String },
    #[error("locale mapping error: {0}")]
    LocaleMap(String),
    #[error("pilot output is a symlink: {0}")]
    UnsafePilotOutput(PathBuf),
    #[error("CSV error: {0}")]
    Csv(#[from] csv::Error),
    #[error("manifest error: {0}")]
    Manifest(String),
    #[error("audio conversion failed for {input}: {detail}")]
    Conversion { input: PathBuf, detail: String },
}
