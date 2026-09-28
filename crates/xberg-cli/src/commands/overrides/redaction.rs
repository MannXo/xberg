//! `--redaction-findings` override.

use std::path::Path;

use anyhow::{Context, Result, bail};
use xberg::ExtractionConfig;
use xberg::core::config::redaction::RedactionConfig;

use super::ExtractionOverrides;

impl ExtractionOverrides {
    /// Read the findings for `--redaction-findings -` from stdin. Call once,
    /// before `apply()`. `document_from_stdin` is whether the document itself
    /// is read from stdin, which cannot be shared.
    pub fn read_redaction_findings_stdin(mut self, document_from_stdin: bool) -> Result<Self> {
        if self.redaction_findings.as_deref() != Some(Path::new("-")) {
            return Ok(self);
        }
        if document_from_stdin {
            bail!("--redaction-findings - and --stdin cannot both read from stdin");
        }
        let text = std::io::read_to_string(std::io::stdin()).context("Failed to read redaction findings from stdin")?;
        let findings = xberg::text::redaction::parse_external_findings(&text)
            .context("Failed to parse redaction findings from stdin")?;
        self.redaction_findings_stdin = Some(findings);
        Ok(self)
    }

    pub(super) fn apply_redaction(&self, config: &mut ExtractionConfig) {
        let Some(path) = &self.redaction_findings else {
            return;
        };
        let redaction = config.redaction.get_or_insert_with(RedactionConfig::default);
        match &self.redaction_findings_stdin {
            Some(findings) => redaction.findings = findings.clone(),
            // `-` that was never read passes through as a path, which fails at
            // extraction instead of silently redacting nothing.
            None => redaction.findings_path = Some(path.clone()),
        }
    }
}
