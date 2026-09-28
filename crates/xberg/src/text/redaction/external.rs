//! Findings supplied by an external content-inspection engine.
//!
//! An engine such as Presidio or AWS Comprehend reports what it found in the
//! extracted text. Each finding is resolved to its literal value and compiled
//! into the same word-boundary-anchored matcher NER mentions use, so it is
//! redacted at every occurrence in every field (xberg-io/xberg#1941).
//!
//! A finding that cannot be resolved fails the run instead of being skipped,
//! because a skipped finding is a value left unredacted.

use std::collections::{HashMap, HashSet};

use crate::Result;
use crate::XbergError;
use crate::core::config::redaction::{ExternalRedactionFinding, RedactionConfig, RedactionOffsetEncoding};
use crate::extractors::security::SecurityLimits;
use crate::types::redaction::PiiCategory;

use super::engine::literal_regex;

/// Parse findings from a JSON array or from JSON Lines, one finding per line.
#[cfg_attr(alef, alef(skip))]
pub fn parse_external_findings(text: &str) -> Result<Vec<ExternalRedactionFinding>> {
    if text.trim_start().starts_with('[') {
        return serde_json::from_str(text)
            .map_err(|err| XbergError::validation(format!("redaction findings: invalid JSON array: {err}")));
    }
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str(line).map_err(|err| {
                XbergError::validation(format!("redaction findings: invalid JSON on line {}: {err}", index + 1))
            })
        })
        .collect()
}

/// Resolve every inline and file-loaded finding to a `(category, matcher)` pair.
///
/// Inline findings must already have passed [`RedactionConfig::validate`].
pub(super) fn compile_external_findings(
    content: &str,
    config: &RedactionConfig,
    limits: &SecurityLimits,
) -> Result<Vec<(PiiCategory, regex::Regex)>> {
    let loaded = load_bounded_findings(config, limits)?;
    let inline_count = config.findings.len();
    let findings: Vec<&ExternalRedactionFinding> = config.findings.iter().chain(&loaded).collect();
    let mut wanted: Vec<usize> = findings
        .iter()
        .filter(|finding| finding.text.is_none())
        .flat_map(|finding| [finding.start, finding.end])
        .flatten()
        .map(|offset| offset as usize)
        .collect();
    let byte_offsets = byte_offsets(content, config.findings_offset_encoding, &mut wanted);

    let mut compiled: HashMap<&str, regex::Regex> = HashMap::new();
    let mut seen: HashSet<(PiiCategory, &str)> = HashSet::new();
    let mut out = Vec::new();
    for (index, finding) in findings.iter().enumerate() {
        let location = location(index, inline_count);
        let (literal, anchor) = resolve_literal(content, finding, &byte_offsets, config, &location)?;
        let regex = match compiled.get(literal) {
            Some(regex) => regex.clone(),
            None => {
                let Some(regex) = literal_regex(literal) else {
                    return Err(XbergError::validation(format!(
                        "{location}: text cannot be compiled into a matcher"
                    )));
                };
                compiled.insert(literal, regex.clone());
                regex
            }
        };
        // A span that cuts a word yields a literal the word-boundary matcher
        // never finds, which would leave the value unredacted. A mismatched
        // `findings_offset_encoding` is the usual cause.
        if let Some(anchor) = anchor
            && regex.find_at(content, anchor).map(|found| found.start()) != Some(anchor)
        {
            return Err(XbergError::validation(format!(
                "{location}: span {}..{} cuts a word under {}; check findings_offset_encoding",
                finding.start.unwrap_or_default(),
                finding.end.unwrap_or_default(),
                encoding_name(config.findings_offset_encoding)
            )));
        }
        let category = PiiCategory::Custom(finding.label.trim().to_string());
        if seen.insert((category.clone(), literal)) {
            out.push((category, regex));
        }
    }
    Ok(out)
}

/// Load `findings_path`, then enforce the count limit over inline and loaded
/// findings together and validate what was loaded.
fn load_bounded_findings(config: &RedactionConfig, limits: &SecurityLimits) -> Result<Vec<ExternalRedactionFinding>> {
    let loaded = match &config.findings_path {
        Some(path) => load_findings(path, limits)?,
        None => Vec::new(),
    };
    let inline_count = config.findings.len();
    let total = inline_count + loaded.len();
    if total > limits.max_redaction_findings {
        return Err(XbergError::validation(format!(
            "RedactionConfig: {total} findings exceed SecurityLimits.max_redaction_findings ({})",
            limits.max_redaction_findings
        )));
    }
    for (index, finding) in loaded.iter().enumerate() {
        finding.validate(&location(inline_count + index, inline_count))?;
    }
    Ok(loaded)
}

/// The trimmed literal a finding redacts, and for a span-derived one the byte
/// offset in `content` where it starts.
fn resolve_literal<'a>(
    content: &'a str,
    finding: &'a ExternalRedactionFinding,
    byte_offsets: &HashMap<usize, usize>,
    config: &RedactionConfig,
    location: &str,
) -> Result<(&'a str, Option<usize>)> {
    let (literal, anchor) = match (&finding.text, finding.start, finding.end) {
        (Some(text), _, _) => (text.trim(), None),
        (None, Some(start), Some(end)) => {
            let (Some(&byte_start), Some(&byte_end)) =
                (byte_offsets.get(&(start as usize)), byte_offsets.get(&(end as usize)))
            else {
                return Err(XbergError::validation(format!(
                    "{location}: span {start}..{end} does not fall on {} boundaries within content",
                    encoding_name(config.findings_offset_encoding)
                )));
            };
            let span = &content[byte_start..byte_end];
            let leading = span.len() - span.trim_start().len();
            (span.trim(), Some(byte_start + leading))
        }
        _ => {
            return Err(XbergError::validation(format!(
                "{location}: needs either text or both start and end"
            )));
        }
    };
    if literal.is_empty() {
        return Err(XbergError::validation(format!("{location}: resolves to blank text")));
    }
    Ok((literal, anchor))
}

fn location(index: usize, inline_count: usize) -> String {
    if index < inline_count {
        format!("RedactionConfig.findings[{index}]")
    } else {
        format!("RedactionConfig.findings_path entry {}", index - inline_count)
    }
}

fn encoding_name(encoding: RedactionOffsetEncoding) -> &'static str {
    match encoding {
        RedactionOffsetEncoding::Utf8Bytes => "utf8_bytes",
        RedactionOffsetEncoding::UnicodeCodePoints => "unicode_code_points",
        RedactionOffsetEncoding::Utf16CodeUnits => "utf16_code_units",
    }
}

/// Map each wanted offset, counted in `encoding` units, to a byte offset in
/// `content`. Offsets past the end or inside a character are left out.
fn byte_offsets(content: &str, encoding: RedactionOffsetEncoding, wanted: &mut Vec<usize>) -> HashMap<usize, usize> {
    wanted.sort_unstable();
    wanted.dedup();
    let unit_len: fn(char) -> usize = match encoding {
        RedactionOffsetEncoding::Utf8Bytes => char::len_utf8,
        RedactionOffsetEncoding::UnicodeCodePoints => |_| 1,
        RedactionOffsetEncoding::Utf16CodeUnits => char::len_utf16,
    };

    let mut map = HashMap::with_capacity(wanted.len());
    let mut pending = wanted.iter().copied().peekable();
    let mut units = 0;
    let positions = content.char_indices().map(Some).chain([None]);
    for position in positions {
        let byte = position.map_or(content.len(), |(byte, _)| byte);
        while let Some(&offset) = pending.peek() {
            if offset > units {
                break;
            }
            if offset == units {
                map.insert(offset, byte);
            }
            pending.next();
        }
        match (position, pending.peek()) {
            (Some((_, character)), Some(_)) => units += unit_len(character),
            _ => break,
        }
    }
    map
}

#[cfg(not(target_arch = "wasm32"))]
fn load_findings(path: &std::path::Path, limits: &SecurityLimits) -> Result<Vec<ExternalRedactionFinding>> {
    use std::io::Read;

    let unreadable = |err: std::io::Error| {
        XbergError::validation(format!("RedactionConfig.findings_path {}: {err}", path.display()))
    };
    let mut text = String::new();
    std::fs::File::open(path)
        .map_err(unreadable)?
        .take(limits.max_content_size as u64 + 1)
        .read_to_string(&mut text)
        .map_err(unreadable)?;
    if text.len() > limits.max_content_size {
        return Err(XbergError::validation(format!(
            "RedactionConfig.findings_path {} exceeds SecurityLimits.max_content_size ({} bytes)",
            path.display(),
            limits.max_content_size
        )));
    }
    parse_external_findings(&text)
}

#[cfg(target_arch = "wasm32")]
fn load_findings(_path: &std::path::Path, _limits: &SecurityLimits) -> Result<Vec<ExternalRedactionFinding>> {
    Err(XbergError::validation(
        "RedactionConfig.findings_path is not supported on wasm32; pass findings inline".to_string(),
    ))
}
