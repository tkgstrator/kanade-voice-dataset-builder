use crate::error::{Error, Result};
use crate::manifest::atomic_write;
use crate::verify::VerificationReport;
use std::path::Path;

pub fn size_category(count: usize) -> &'static str {
    match count {
        0..=999 => "n<1K",
        1_000..=9_999 => "1K<n<10K",
        10_000..=99_999 => "10K<n<100K",
        100_000..=999_999 => "100K<n<1M",
        _ => "n>1M",
    }
}

/// Render only after a fresh source-aware verification has produced `report`.
pub fn render_dataset_card(report: &VerificationReport, template: &str) -> Result<String> {
    let count = report.accepted.len();
    let mut card = template.to_owned();
    for (key, value) in [
        ("{{SIZE_CATEGORY}}", size_category(count).to_owned()),
        ("{{ACCEPTED_COUNT}}", count.to_string()),
        ("{{REJECTED_COUNT}}", report.rejected_count.to_string()),
        ("{{ORPHAN_COUNT}}", report.orphan_audio_count.to_string()),
        (
            "{{PHYSICAL_COUNT}}",
            report.physical_audio_count.to_string(),
        ),
        (
            "{{DURATION_SECONDS}}",
            format!("{:.3}", report.duration_seconds),
        ),
        (
            "{{SPEAKER_COUNT}}",
            report
                .accepted
                .iter()
                .map(|row| row.speaker_id.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                .to_string(),
        ),
    ] {
        card = card.replace(key, &value);
    }
    if card.contains("{{") || card.contains("}}") {
        return Err(Error::Manifest(
            "unresolved dataset card placeholder".into(),
        ));
    }
    Ok(card)
}

pub fn write_dataset_card(work: &Path, report: &VerificationReport) -> Result<()> {
    let card = render_dataset_card(report, include_str!("../templates/README.md.tmpl"))?;
    atomic_write(&work.join("README.md"), card.as_bytes())
}
