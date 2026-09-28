//! Redaction & anonymisation configuration.
//!
//! When `ExtractionConfig::redaction` is `Some`, the redaction post-processor runs
//! as the Late stage of the pipeline and rewrites `content`, `formatted_content`,
//! every chunk's text, and the textual fields of `entities` / `summary` /
//! `translation` / `page_classifications` using the configured strategy. The
//! original text never appears in the returned `ExtractedDocument`.

use crate::Result;
use crate::types::redaction::{PiiCategory, RedactionStrategy};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

/// Configuration for the redaction post-processor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "alef-meta", alef(since = "1.0.0"))]
pub struct RedactionConfig {
    /// Categories to redact. Empty means "every category supported by the engine."
    #[serde(default)]
    #[cfg_attr(feature = "api", schema(value_type = Vec<PiiCategory>))]
    pub categories: HashSet<PiiCategory>,
    /// Strategy applied to every match.
    #[serde(default)]
    pub strategy: RedactionStrategy,
    /// Optional NER backend — required to redact PERSON / ORGANIZATION / LOCATION
    /// categories (the pure-Rust pattern engine only covers regex-detectable PII).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ner: Option<super::ner::NerConfig>,
    /// When `true`, chunk byte ranges are kept consistent with the rewritten content by
    /// adjusting `byte_start` / `byte_end` after replacement. When `false`, chunk byte
    /// ranges still refer to the *original* content offsets — useful when downstream
    /// consumers want to map findings back to the original document.
    #[serde(default = "default_preserve_offsets")]
    pub preserve_offsets: bool,
    /// Arbitrary user-supplied literal terms to redact.
    ///
    /// Each term is treated as a regex hit against the document, surfacing as
    /// `PiiCategory::Custom(label)` in [`RedactionFinding`](crate::types::redaction::RedactionFinding)
    /// where `label` is the per-term label (defaulting to the literal value itself).
    /// Case-insensitive by default; set [`RedactionTerm::case_sensitive`] for exact match.
    ///
    /// Use this when you need to redact tenant-specific tokens (employee IDs,
    /// project codes, internal product names) without writing a custom plugin.
    #[serde(default)]
    pub custom_terms: Vec<RedactionTerm>,
    /// Arbitrary user-supplied regex patterns to redact.
    ///
    /// Same surfacing semantics as [`custom_terms`](Self::custom_terms): each
    /// hit becomes a `PiiCategory::Custom(label)` finding. Patterns are validated
    /// at config-construction time via [`RedactionConfig::validate`].
    #[serde(default)]
    pub custom_patterns: Vec<RedactionPattern>,
    /// Findings produced by an external content-inspection engine (Presidio,
    /// AWS Comprehend, ...) over this document's extracted text.
    ///
    /// Each finding's literal value is redacted at every occurrence in every
    /// textual field, surfacing as `PiiCategory::Custom(label)`. Unlike
    /// [`ner`](Self::ner) labels, finding labels need no allowlist: the caller
    /// supplied them explicitly.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<ExternalRedactionFinding>,
    /// JSON array or JSON Lines file of findings, loaded when redaction runs
    /// and merged with [`findings`](Self::findings). Not supported on
    /// `wasm32`, which has no filesystem.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "api", schema(value_type = Option<String>))]
    pub findings_path: Option<PathBuf>,
    /// How a finding's `start` / `end` count into `content`. Only consulted
    /// for findings without `text`.
    #[serde(default)]
    pub findings_offset_encoding: RedactionOffsetEncoding,
}

/// One finding reported by an external content-inspection engine.
///
/// Unknown fields are ignored, so an engine's raw output can be passed as is.
/// Presidio's `entity_type` and AWS Comprehend's `Type`, `Text`,
/// `BeginOffset`, `EndOffset` and `Score` are accepted as aliases.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct ExternalRedactionFinding {
    /// Engine category, surfaced as `PiiCategory::Custom(label)`.
    #[serde(alias = "entity_type", alias = "Type")]
    pub label: String,
    /// Literal value to redact. When absent, it is read from `content` at
    /// `start..end` under [`RedactionConfig::findings_offset_encoding`].
    #[serde(default, alias = "Text", skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Start offset (inclusive) into `content`.
    #[serde(default, alias = "BeginOffset", skip_serializing_if = "Option::is_none")]
    pub start: Option<u32>,
    /// End offset (exclusive) into `content`.
    #[serde(default, alias = "EndOffset", skip_serializing_if = "Option::is_none")]
    pub end: Option<u32>,
    /// Engine confidence in `[0.0, 1.0]`. Validated, not used for filtering.
    #[serde(default, alias = "Score", skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

impl ExternalRedactionFinding {
    pub(crate) fn validate(&self, location: &str) -> Result<()> {
        let invalid = |reason: &str| Err(crate::XbergError::validation(format!("{location}: {reason}")));
        if self.label.trim().is_empty() {
            return invalid("label is empty");
        }
        if let (Some(start), Some(end)) = (self.start, self.end)
            && start >= end
        {
            return invalid(&format!("start {start} is not before end {end}"));
        }
        match &self.text {
            Some(text) if text.trim().is_empty() => return invalid("text is empty"),
            Some(_) => {}
            None if self.start.is_none() || self.end.is_none() => {
                return invalid("needs either text or both start and end");
            }
            None => {}
        }
        if let Some(score) = self.score
            && !(score.is_finite() && (0.0..=1.0).contains(&score))
        {
            return invalid(&format!("score must be between 0.0 and 1.0, got {score}"));
        }
        Ok(())
    }
}

/// Unit that an external finding's `start` / `end` offsets count in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RedactionOffsetEncoding {
    /// UTF-8 byte offsets.
    Utf8Bytes,
    /// Unicode scalar value (code point) offsets, as Presidio reports them.
    #[default]
    UnicodeCodePoints,
    /// UTF-16 code unit offsets.
    Utf16CodeUnits,
}

fn default_preserve_offsets() -> bool {
    true
}

fn default_case_sensitive() -> bool {
    false
}

/// One user-supplied literal term to redact.
///
/// Matched as a regex-escaped substring (so callers do not need to escape
/// metacharacters themselves). Case-insensitive by default — set
/// [`Self::case_sensitive`] to `true` for exact byte-match semantics.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct RedactionTerm {
    /// Custom category label surfaced in [`RedactionFinding::category`](crate::types::redaction::RedactionFinding::category).
    pub label: String,
    /// Literal value to match. Regex metacharacters are escaped automatically.
    pub value: String,
    /// When `true`, match the value as-is; otherwise match ASCII-case-insensitively.
    #[serde(default = "default_case_sensitive")]
    pub case_sensitive: bool,
}

impl RedactionTerm {
    /// Build a term whose label is the literal value itself (case-insensitive).
    pub fn literal(value: impl Into<String>) -> Self {
        let v = value.into();
        Self {
            label: v.clone(),
            value: v,
            case_sensitive: false,
        }
    }

    /// Build a term with a custom label.
    pub fn labeled(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            case_sensitive: false,
        }
    }
}

/// One user-supplied regex pattern to redact.
///
/// The pattern is compiled with the Rust `regex` crate (no look-around). Case
/// sensitivity is encoded in the pattern via the `(?i)` inline flag when
/// [`Self::case_sensitive`] is `false`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct RedactionPattern {
    /// Custom category label surfaced in [`RedactionFinding::category`](crate::types::redaction::RedactionFinding::category).
    pub label: String,
    /// Regex pattern (Rust `regex` crate dialect — no look-around).
    pub pattern: String,
    /// When `true`, match case-sensitively; otherwise prepend `(?i)` to the regex.
    #[serde(default = "default_case_sensitive")]
    pub case_sensitive: bool,
}

impl RedactionPattern {
    /// Build a pattern with the given label (case-insensitive by default).
    pub fn labeled(label: impl Into<String>, pattern: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            pattern: pattern.into(),
            case_sensitive: false,
        }
    }
}

impl Default for RedactionConfig {
    fn default() -> Self {
        Self {
            categories: HashSet::new(),
            strategy: RedactionStrategy::default(),
            ner: None,
            preserve_offsets: true,
            custom_terms: Vec::new(),
            custom_patterns: Vec::new(),
            findings: Vec::new(),
            findings_path: None,
            findings_offset_encoding: RedactionOffsetEncoding::default(),
        }
    }
}

impl RedactionConfig {
    /// Validate user-supplied terms and patterns at config-construction time.
    ///
    /// Compiles every [`RedactionPattern::pattern`] (with the case-insensitive
    /// inline flag where applicable) and returns the first compilation error so
    /// the caller can reject the config before the redaction pipeline runs.
    /// Pure terms (regex-escaped) cannot fail to compile, but the function
    /// still rejects empty values to avoid degenerate zero-length matches.
    /// Inline [`findings`](Self::findings) are checked for shape here; their
    /// offsets, and anything loaded from `findings_path`, are resolved when
    /// redaction runs.
    pub fn validate(&self) -> Result<()> {
        for term in &self.custom_terms {
            if term.value.is_empty() {
                return Err(crate::XbergError::validation(format!(
                    "RedactionConfig.custom_terms[{}]: value is empty",
                    term.label
                )));
            }
        }
        for pattern in &self.custom_patterns {
            if pattern.pattern.is_empty() {
                return Err(crate::XbergError::validation(format!(
                    "RedactionConfig.custom_patterns[{}]: pattern is empty",
                    pattern.label
                )));
            }
            let compiled = if pattern.case_sensitive {
                regex::Regex::new(&pattern.pattern)
            } else {
                regex::Regex::new(&format!("(?i){}", pattern.pattern))
            };
            if let Err(err) = compiled {
                return Err(crate::XbergError::validation(format!(
                    "RedactionConfig.custom_patterns[{}]: invalid regex: {err}",
                    pattern.label
                )));
            }
        }
        for (index, finding) in self.findings.iter().enumerate() {
            finding.validate(&format!("RedactionConfig.findings[{index}]"))?;
        }
        #[cfg(target_arch = "wasm32")]
        if self.findings_path.is_some() {
            return Err(crate::XbergError::validation(
                "RedactionConfig.findings_path is not supported on wasm32; pass findings inline".to_string(),
            ));
        }
        Ok(())
    }
}
