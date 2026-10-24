use crate::{
    error::{Error, Result},
    model::NormalizedText,
};
use unicode_normalization::UnicodeNormalization;

pub const NORMALIZATION_VERSION: &str = "kanade-text-v2";

#[derive(Clone, Debug, Default)]
pub struct NormalizationRules;

impl NormalizationRules {
    pub fn kanade() -> Self {
        Self
    }
}

fn next_char(value: &str, byte_index: usize) -> Option<(char, usize)> {
    value[byte_index..]
        .chars()
        .next()
        .map(|ch| (ch, byte_index + ch.len_utf8()))
}

fn push_line_break(display: &mut String, spoken: &mut String) {
    display.push('\n');
    if !spoken.is_empty() && !spoken.ends_with(char::is_whitespace) {
        spoken.push(' ');
    }
}

fn markup_end(value: &str, start: usize) -> Result<usize> {
    value[start..]
        .find(';')
        .map(|relative| start + relative + 1)
        .ok_or_else(|| Error::MalformedMarkup(value[start.saturating_sub(2)..].to_owned()))
}

fn remove_outer_japanese_quotes(value: &str) -> &str {
    value
        .strip_prefix('「')
        .and_then(|inner| inner.strip_suffix('」'))
        .unwrap_or(value)
}

pub fn normalize_text(raw: &str, _rules: &NormalizationRules) -> Result<NormalizedText> {
    let input = raw.nfc().collect::<String>();
    let mut display = String::new();
    let mut spoken = String::new();
    let mut index = 0;
    let mut emphasis_open = false;

    while index < input.len() {
        let rest = &input[index..];
        if rest.starts_with("%i1") {
            if emphasis_open {
                return Err(Error::MalformedMarkup("nested %i1".into()));
            }
            emphasis_open = true;
            index += 3;
            continue;
        }
        if rest.starts_with("%id") {
            if !emphasis_open {
                return Err(Error::MalformedMarkup("unmatched %id".into()));
            }
            emphasis_open = false;
            index += 3;
            continue;
        }
        if rest.starts_with("%f") {
            let end = markup_end(&input, index + 2)?;
            if end == index + 3 {
                return Err(Error::MalformedMarkup("empty %f control".into()));
            }
            index = end;
            continue;
        }
        if rest.starts_with("%n;") {
            push_line_break(&mut display, &mut spoken);
            index += 3;
            continue;
        }
        if rest.starts_with("%C")
            && rest[2..]
                .chars()
                .next()
                .is_none_or(|ch| !ch.is_ascii_alphabetic())
        {
            index += 2;
            continue;
        }
        if rest.starts_with('%') {
            let end = markup_end(&input, index + 1)?;
            let payload = &input[index + 1..end - 1];
            if payload.is_empty() || !payload.chars().all(|ch| ch.is_ascii_digit()) {
                return Err(Error::UnknownMarkup(input[index..end].to_owned()));
            }
            index = end;
            continue;
        }

        let Some((ch, next)) = next_char(&input, index) else {
            break;
        };
        match ch {
            '\\' => {
                let Some((escaped, after_escape)) = next_char(&input, next) else {
                    return Err(Error::MalformedMarkup("trailing escape".into()));
                };
                match escaped {
                    'n' => push_line_break(&mut display, &mut spoken),
                    '%' | '[' | ']' | '\\' | '#' | '&' | '$' => {
                        display.push(escaped);
                        spoken.push(escaped);
                    }
                    _ => {
                        return Err(Error::UnknownMarkup(format!("\\{escaped}")));
                    }
                }
                index = after_escape;
            }
            '\r' => {
                let after_cr = if input[next..].starts_with('\n') {
                    next + 1
                } else {
                    next
                };
                push_line_break(&mut display, &mut spoken);
                index = after_cr;
            }
            '\n' => {
                push_line_break(&mut display, &mut spoken);
                index = next;
            }
            '[' => {
                let closing = input[next..]
                    .find(']')
                    .map(|relative| next + relative)
                    .ok_or_else(|| Error::MalformedMarkup(input[index..].to_owned()))?;
                let contents = &input[next..closing];
                let (reading, base_len) = if let Some((reading, count)) = contents.split_once(',') {
                    if count.contains(',') || count.is_empty() {
                        return Err(Error::MalformedMarkup(contents.to_owned()));
                    }
                    let count = count
                        .parse::<usize>()
                        .map_err(|_| Error::MalformedMarkup(contents.to_owned()))?;
                    (reading, count.saturating_add(1))
                } else {
                    (contents, 1)
                };
                if reading.is_empty()
                    || reading
                        .chars()
                        .any(|ch| ch.is_control() || ch.is_whitespace() || "[]%=\\".contains(ch))
                {
                    return Err(Error::UnknownMarkup(contents.to_owned()));
                }
                let mut base_end = closing + 1;
                let mut base = String::new();
                for _ in 0..base_len {
                    let Some((base_char, next_base)) = next_char(&input, base_end) else {
                        return Err(Error::MalformedMarkup(contents.to_owned()));
                    };
                    if matches!(base_char, '[' | ']' | '%' | '\\' | '\r' | '\n') {
                        return Err(Error::MalformedMarkup(contents.to_owned()));
                    }
                    base.push(base_char);
                    base_end = next_base;
                }
                display.push_str(&base);
                spoken.push_str(reading);
                index = base_end;
            }
            ']' => return Err(Error::UnknownMarkup("]".into())),
            '#' | '&' | '$' => return Err(Error::UnknownMarkup(ch.to_string())),
            _ => {
                display.push(ch);
                spoken.push(ch);
                index = next;
            }
        }
    }
    if emphasis_open {
        return Err(Error::MalformedMarkup("unclosed %i1".into()));
    }
    if display.chars().all(char::is_whitespace) || spoken.chars().all(char::is_whitespace) {
        return Err(Error::MalformedMarkup("empty after controls".into()));
    }
    while spoken.ends_with(' ') {
        spoken.pop();
    }
    let display = remove_outer_japanese_quotes(&display).to_owned();
    let spoken = remove_outer_japanese_quotes(&spoken).to_owned();
    if display.is_empty() || spoken.is_empty() {
        return Err(Error::MalformedMarkup("empty after outer quotes".into()));
    }
    Ok(NormalizedText {
        raw_text: raw.to_owned(),
        display_text: display,
        spoken_text: spoken,
        quality_flags: Vec::new(),
        normalization_version: NORMALIZATION_VERSION.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normalize(raw: &str) -> NormalizedText {
        normalize_text(raw, &NormalizationRules::kanade()).unwrap()
    }

    #[test]
    fn only_one_complete_outer_japanese_quote_pair_is_removed() {
        for (raw, expected) in [
            ("「仮の文」", "仮の文"),
            ("仮の文", "仮の文"),
            ("前「中」後", "前「中」後"),
            ("「前「中」後」", "前「中」後"),
            ("「「仮」」", "「仮」"),
            ("「仮", "「仮"),
            ("仮」", "仮」"),
            ("「", "「"),
            ("」", "」"),
        ] {
            let normalized = normalize(raw);
            assert_eq!(normalized.raw_text, raw);
            assert_eq!(normalized.display_text, expected, "{raw}");
            assert_eq!(normalized.spoken_text, expected, "{raw}");
        }
        assert!(normalize_text("", &NormalizationRules::kanade()).is_err());
        assert!(normalize_text("「」", &NormalizationRules::kanade()).is_err());
    }

    #[test]
    fn raw_is_verbatim_while_plain_display_and_spoken_are_nfc() {
        let raw = "「カ\u{3099}ー」";
        let normalized = normalize(raw);
        assert_eq!(normalized.raw_text, raw);
        assert_eq!(normalized.display_text, "ガー");
        assert_eq!(normalized.spoken_text, "ガー");
        assert_eq!(normalized.normalization_version, NORMALIZATION_VERSION);
        assert!(normalized.quality_flags.is_empty());
    }

    #[test]
    fn paired_emphasis_and_font_controls_preserve_enclosed_text() {
        let normalized = normalize("「%i1甲%id乙%f$記号$;丙♪%fuser;」");
        assert_eq!(normalized.display_text, "甲乙丙♪");
        assert_eq!(normalized.spoken_text, "甲乙丙♪");
    }

    #[test]
    fn escaped_percent_and_brackets_are_literals() {
        let normalized = normalize(r"100\% \[注\]");
        assert_eq!(normalized.display_text, "100% [注]");
        assert_eq!(normalized.spoken_text, "100% [注]");
    }

    #[test]
    fn game_control_shaped_markers_reject_unescaped_and_preserve_escaped_literals() {
        for raw in [
            "#FF0000;本文",
            "&graph;本文",
            "$expression;本文",
            "#;本文",
            "&;本文",
            "$;本文",
            "#FF0000本文",
            "&graph本文",
            "$expression本文",
            "#FF 0000;本文",
            "&graph option;本文",
            "$x + 1;本文",
        ] {
            assert!(
                matches!(
                    normalize_text(raw, &NormalizationRules::kanade()),
                    Err(Error::UnknownMarkup(_))
                ),
                "{raw}"
            );
        }
        let escaped = normalize(r"\#FF0000; \&graph; \$expression;");
        assert_eq!(escaped.display_text, "#FF0000; &graph; $expression;");
        assert_eq!(escaped.spoken_text, "#FF0000; &graph; $expression;");
    }

    #[test]
    fn source_and_markup_newlines_have_explicit_display_and_spoken_forms() {
        let normalized = normalize("前\r\n中\\n後%n;終");
        assert_eq!(normalized.display_text, "前\n中\n後\n終");
        assert_eq!(normalized.spoken_text, "前 中 後 終");
    }

    #[test]
    fn observed_center_and_numeric_font_controls_are_non_spoken() {
        let normalized = normalize("%C%130;――仮の文？");
        assert_eq!(normalized.display_text, "――仮の文？");
        assert_eq!(normalized.spoken_text, "――仮の文？");
    }

    #[test]
    fn observed_ruby_keeps_orthography_for_display_and_reading_for_spoken() {
        let normalized = normalize("「[かん,1]漢字と[じ]字」");
        assert_eq!(normalized.display_text, "漢字と字");
        assert_eq!(normalized.spoken_text, "かんとじ");
    }

    #[test]
    fn empty_malformed_and_unknown_markup_fail_closed() {
        for raw in [
            "%i1%id",
            "%i1未完",
            "%id",
            "%fuser",
            "%q;本文",
            "%Color",
            "[よみ,2]一字",
            "[command name=x]字",
            "末尾[よみ]",
        ] {
            assert!(
                normalize_text(raw, &NormalizationRules::kanade()).is_err(),
                "{raw}"
            );
        }
    }
}
