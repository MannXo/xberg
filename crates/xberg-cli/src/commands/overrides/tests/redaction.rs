use std::path::PathBuf;

use super::super::*;
use super::default_overrides;
use xberg::ExtractionConfig;
use xberg::core::config::redaction::{ExternalRedactionFinding, RedactionConfig};
use xberg::types::redaction::RedactionStrategy;

fn finding(label: &str, text: &str) -> ExternalRedactionFinding {
    ExternalRedactionFinding {
        label: label.to_string(),
        text: Some(text.to_string()),
        ..Default::default()
    }
}

#[test]
fn redaction_findings_path_enables_redaction() {
    let mut config = ExtractionConfig::default();
    let overrides = ExtractionOverrides {
        redaction_findings: Some(PathBuf::from("findings.json")),
        ..default_overrides()
    };
    overrides.apply(&mut config);
    let redaction = config.redaction.expect("the flag must enable redaction");
    assert_eq!(redaction.findings_path, Some(PathBuf::from("findings.json")));
    assert_eq!(redaction.strategy, RedactionStrategy::default());
}

#[test]
fn redaction_findings_path_keeps_the_configured_redaction_settings() {
    let mut config = ExtractionConfig {
        redaction: Some(RedactionConfig {
            strategy: RedactionStrategy::TokenReplace,
            ..RedactionConfig::default()
        }),
        ..Default::default()
    };
    let overrides = ExtractionOverrides {
        redaction_findings: Some(PathBuf::from("findings.jsonl")),
        ..default_overrides()
    };
    overrides.apply(&mut config);
    let redaction = config.redaction.expect("redaction");
    assert_eq!(redaction.strategy, RedactionStrategy::TokenReplace);
    assert_eq!(redaction.findings_path, Some(PathBuf::from("findings.jsonl")));
}

#[test]
fn redaction_findings_from_stdin_replace_the_configured_findings() {
    let mut config = ExtractionConfig {
        redaction: Some(RedactionConfig {
            findings: vec![finding("PERSON", "Blorp")],
            ..RedactionConfig::default()
        }),
        ..Default::default()
    };
    let overrides = ExtractionOverrides {
        redaction_findings: Some(PathBuf::from("-")),
        redaction_findings_stdin: Some(vec![finding("PERSON", "Zarnak Quorlim")]),
        ..default_overrides()
    };
    overrides.apply(&mut config);
    let redaction = config.redaction.expect("redaction");
    assert_eq!(redaction.findings, vec![finding("PERSON", "Zarnak Quorlim")]);
    assert_eq!(redaction.findings_path, None);
}

#[test]
fn unread_stdin_findings_pass_through_as_a_path_rather_than_nothing() {
    let mut config = ExtractionConfig::default();
    let overrides = ExtractionOverrides {
        redaction_findings: Some(PathBuf::from("-")),
        ..default_overrides()
    };
    overrides.apply(&mut config);
    assert_eq!(
        config.redaction.expect("redaction").findings_path,
        Some(PathBuf::from("-"))
    );
}

#[test]
fn stdin_findings_are_rejected_when_the_document_is_read_from_stdin() {
    let overrides = ExtractionOverrides {
        redaction_findings: Some(PathBuf::from("-")),
        ..default_overrides()
    };
    let error = overrides
        .read_redaction_findings_stdin(true)
        .expect_err("stdin cannot carry both the document and the findings");
    assert!(error.to_string().contains("--stdin"), "{error}");
}

#[test]
fn a_findings_path_does_not_touch_stdin() {
    let overrides = ExtractionOverrides {
        redaction_findings: Some(PathBuf::from("findings.json")),
        ..default_overrides()
    }
    .read_redaction_findings_stdin(true)
    .expect("a path needs no stdin");
    assert!(overrides.redaction_findings_stdin.is_none());
}
