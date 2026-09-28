//! Tests for xberg-io/xberg#1941: redaction driven by the findings an external
//! content-inspection engine (Presidio, AWS Comprehend) reported over the
//! extracted text.
//!
//! All PII in these tests is synthetic and built in-test.

#![cfg(feature = "redaction")]

use std::borrow::Cow;
use std::collections::HashSet;

use xberg::ExtractionConfig;
use xberg::core::config::redaction::{ExternalRedactionFinding, RedactionConfig, RedactionOffsetEncoding};
use xberg::extractors::security::SecurityLimits;
use xberg::plugins::PostProcessor;
use xberg::plugins::processor::builtin::redaction::RedactionProcessor;
use xberg::text::redaction::{parse_external_findings, redact_with_entities};
use xberg::types::redaction::PiiCategory;
use xberg::types::tables::Table;
use xberg::types::{Chunk, ChunkMetadata, ChunkType, ExtractedDocument};

const MASK: &str = "[REDACTED]";

fn document(content: &str) -> ExtractedDocument {
    let mut document = ExtractedDocument::default();
    document.content = content.to_string();
    document.mime_type = Cow::Borrowed("text/plain");
    document
}

fn with_findings(findings: Vec<ExternalRedactionFinding>) -> RedactionConfig {
    RedactionConfig {
        findings,
        ..RedactionConfig::default()
    }
}

fn text_finding(label: &str, text: &str) -> ExternalRedactionFinding {
    ExternalRedactionFinding {
        label: label.to_string(),
        text: Some(text.to_string()),
        ..Default::default()
    }
}

fn span_finding(label: &str, start: u32, end: u32) -> ExternalRedactionFinding {
    ExternalRedactionFinding {
        label: label.to_string(),
        start: Some(start),
        end: Some(end),
        ..Default::default()
    }
}

fn categories(document: &ExtractedDocument) -> HashSet<PiiCategory> {
    let report = document.redaction_report.as_ref().expect("report must be attached");
    report.findings.iter().map(|finding| finding.category.clone()).collect()
}

fn chunk(content: &str) -> Chunk {
    Chunk {
        content: content.to_string(),
        chunk_type: ChunkType::Unknown,
        embedding: None,
        sparse_embedding: None,
        late_interaction: None,
        metadata: ChunkMetadata {
            byte_start: 0,
            byte_end: content.len(),
            token_count: None,
            chunk_index: 0,
            total_chunks: 1,
            first_page: None,
            last_page: None,
            heading_context: None,
            heading_path: Vec::new(),
            image_indices: Vec::new(),
            node_ids: Vec::new(),
            page_spans: Vec::new(),
            classifications: Vec::new(),
        },
    }
}

fn run_processor(document: &mut ExtractedDocument, config: &ExtractionConfig) -> xberg::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(RedactionProcessor.process(document, config))
}

#[test]
fn should_redact_raw_presidio_output_by_deriving_text_from_code_point_offsets() {
    // The accented name puts code-point and byte offsets out of step, so a
    // byte-offset reading would slice the wrong text.
    let content = "Zoë Quorlim called (415) 555-0132 on Monday.";
    let mut document = document(content);
    let presidio = r#"[
        {"entity_type": "PERSON", "start": 0, "end": 11, "score": 0.85,
         "analysis_explanation": null, "recognition_metadata": {"recognizer_name": "SpacyRecognizer"}},
        {"entity_type": "PHONE_NUMBER", "start": 19, "end": 33, "score": 0.75}
    ]"#;
    let findings = parse_external_findings(presidio).expect("Presidio output must parse");
    // Keep the built-in phone detector from claiming the span first.
    let config = RedactionConfig {
        categories: HashSet::from([PiiCategory::Email]),
        ..with_findings(findings)
    };

    redact_with_entities(&mut document, &config, &[]).expect("redaction must succeed");

    assert_eq!(document.content, format!("{MASK} called {MASK} on Monday."));
    assert_eq!(
        categories(&document),
        HashSet::from([
            PiiCategory::Custom("PERSON".to_string()),
            PiiCategory::Custom("PHONE_NUMBER".to_string()),
        ])
    );
}

#[test]
fn should_accept_aws_comprehend_field_names() {
    let content = "Hello Zarnak Quorlim, card 4111 1111 1111 1111 is on file.";
    let comprehend = r#"[
        {"Score": 0.9999, "Type": "NAME", "BeginOffset": 6, "EndOffset": 20},
        {"Score": 0.8905, "Type": "CREDIT_DEBIT_NUMBER", "BeginOffset": 27, "EndOffset": 46}
    ]"#;
    let mut document = document(content);
    let findings = parse_external_findings(comprehend).expect("Comprehend output must parse");
    assert_eq!(findings[0].label, "NAME");
    assert_eq!((findings[0].start, findings[0].end), (Some(6), Some(20)));

    redact_with_entities(&mut document, &with_findings(findings), &[]).expect("redaction must succeed");

    assert_eq!(document.content, format!("Hello {MASK}, card {MASK} is on file."));
}

#[test]
fn should_fail_when_a_span_cuts_a_word_under_the_configured_encoding() {
    // Presidio's code-point span for the name, read as UTF-8 bytes: the
    // two-byte "ë" shifts the end one byte short, mid-word.
    let content = "Zoë Quorlim called.";
    let mut document = document(content);
    let config = RedactionConfig {
        findings_offset_encoding: RedactionOffsetEncoding::Utf8Bytes,
        ..with_findings(vec![span_finding("PERSON", 0, 11)])
    };

    let error = redact_with_entities(&mut document, &config, &[]).expect_err("a word-cutting span must fail");

    assert!(error.to_string().contains("findings_offset_encoding"), "{error}");
    assert!(
        !error.to_string().contains("Quorl"),
        "the error must not echo document text: {error}"
    );
    assert_eq!(document.content, content);
}

#[test]
fn should_redact_every_occurrence_in_every_text_field() {
    let name = "Zarnak Quorlim";
    let mut document = document(&format!("{name} signed. Witness: {name}."));
    document.formatted_content = Some(format!("# {name}\n\nSigned by {name}."));
    document.chunks = Some(vec![chunk(&format!("{name} signed."))]);
    document.tables = vec![Table {
        cells: vec![vec!["Signatory".into(), name.into()]],
        markdown: format!("| Signatory | {name} |"),
        page_number: 1,
        ..Default::default()
    }];
    document.metadata.subject = Some(format!("Agreement with {name}"));

    redact_with_entities(&mut document, &with_findings(vec![text_finding("PERSON", name)]), &[])
        .expect("redaction must succeed");

    assert_eq!(document.content, format!("{MASK} signed. Witness: {MASK}."));
    assert_eq!(
        document.formatted_content.as_deref(),
        Some(format!("# {MASK}\n\nSigned by {MASK}.").as_str())
    );
    let chunks = document.chunks.as_ref().expect("chunks");
    assert_eq!(chunks[0].content, format!("{MASK} signed."));
    assert_eq!(document.tables[0].cells[0][1], MASK);
    assert_eq!(document.tables[0].markdown, format!("| Signatory | {MASK} |"));
    assert_eq!(document.metadata.subject, Some(format!("Agreement with {MASK}")));
    assert_eq!(
        categories(&document),
        HashSet::from([PiiCategory::Custom("PERSON".to_string())])
    );
}

#[test]
fn should_redact_a_finding_label_without_allowlisting_it() {
    // A category filter narrows the built-in detectors; it must not drop a
    // finding the caller supplied explicitly.
    let mut document = document("Zarnak Quorlim wrote to alice@example.com.");
    let config = RedactionConfig {
        categories: HashSet::from([PiiCategory::Email]),
        ..with_findings(vec![text_finding("PERSON", "Zarnak Quorlim")])
    };

    redact_with_entities(&mut document, &config, &[]).expect("redaction must succeed");

    assert_eq!(document.content, format!("{MASK} wrote to {MASK}."));
}

#[test]
fn should_match_a_finding_as_a_whole_word_and_case_sensitively() {
    let mut document = document("Ann filed the Annual report; ann and ANN did not.");

    redact_with_entities(&mut document, &with_findings(vec![text_finding("PERSON", "Ann")]), &[])
        .expect("redaction must succeed");

    assert_eq!(
        document.content,
        format!("{MASK} filed the Annual report; ann and ANN did not.")
    );
}

#[test]
fn should_derive_text_from_utf8_and_utf16_offsets() {
    // U+1F600 is four UTF-8 bytes and two UTF-16 code units.
    let content = "\u{1F600} Zarnak smiled.";
    for (encoding, start, end) in [
        (RedactionOffsetEncoding::Utf8Bytes, 5, 11),
        (RedactionOffsetEncoding::Utf16CodeUnits, 3, 9),
        (RedactionOffsetEncoding::UnicodeCodePoints, 2, 8),
    ] {
        let mut document = document(content);
        let config = RedactionConfig {
            findings_offset_encoding: encoding,
            ..with_findings(vec![span_finding("PERSON", start, end)])
        };

        redact_with_entities(&mut document, &config, &[]).expect("redaction must succeed");

        assert_eq!(document.content, format!("\u{1F600} {MASK} smiled."), "{encoding:?}");
    }
}

#[test]
fn should_fail_rather_than_skip_a_span_it_cannot_resolve() {
    let content = "\u{1F600} Zarnak smiled.";
    for (encoding, start, end) in [
        // Inside the surrogate pair.
        (RedactionOffsetEncoding::Utf16CodeUnits, 1, 9),
        // Inside the four-byte sequence.
        (RedactionOffsetEncoding::Utf8Bytes, 2, 11),
        // Past the end of content.
        (RedactionOffsetEncoding::UnicodeCodePoints, 2, 400),
    ] {
        let mut document = document(content);
        let config = RedactionConfig {
            findings_offset_encoding: encoding,
            ..with_findings(vec![span_finding("PERSON", start, end)])
        };

        let error = redact_with_entities(&mut document, &config, &[]).expect_err("an unresolvable span must fail");

        assert!(error.to_string().contains("RedactionConfig.findings[0]"), "{error}");
        assert_eq!(document.content, content, "{encoding:?}");
    }
}

#[test]
fn should_reject_malformed_findings_at_validation() {
    let cases = [
        text_finding(" ", "Zarnak"),
        text_finding("PERSON", "  "),
        ExternalRedactionFinding {
            label: "PERSON".to_string(),
            start: Some(4),
            ..Default::default()
        },
        span_finding("PERSON", 8, 8),
        ExternalRedactionFinding {
            score: Some(1.5),
            ..text_finding("PERSON", "Zarnak")
        },
    ];
    for finding in cases {
        let config = with_findings(vec![finding.clone()]);
        assert!(config.validate().is_err(), "must be rejected: {finding:?}");
    }
}

#[test]
fn should_read_findings_from_extraction_config_json() {
    let config: ExtractionConfig = serde_json::from_str(
        r#"{"redaction": {"findings": [
            {"entity_type": "PERSON", "text": "Zarnak Quorlim", "score": 0.92, "recognition_metadata": {}}
        ]}}"#,
    )
    .expect("findings with vendor extras must parse");
    let redaction = config.redaction.expect("redaction");
    assert_eq!(
        redaction.findings,
        vec![ExternalRedactionFinding {
            score: Some(0.92),
            ..text_finding("PERSON", "Zarnak Quorlim")
        }]
    );

    let typo = serde_json::from_str::<ExtractionConfig>(r#"{"redaction": {"findingz": []}}"#);
    assert!(typo.is_err(), "RedactionConfig itself must still reject unknown keys");
}

#[test]
fn should_load_findings_from_json_and_json_lines_files() {
    let directory = tempfile::tempdir().expect("tempdir");
    let array = directory.path().join("findings.json");
    std::fs::write(&array, r#"[{"entity_type": "PERSON", "start": 0, "end": 14}]"#).expect("write");
    let lines = directory.path().join("findings.jsonl");
    std::fs::write(
        &lines,
        "{\"entity_type\": \"PERSON\", \"start\": 0, \"end\": 14}\n\n{\"entity_type\": \"CITY\", \"text\": \"Quorlim City\"}\n",
    )
    .expect("write");

    for (path, expected) in [
        (array, "[REDACTED] moved to Quorlim City."),
        (lines, "[REDACTED] moved to [REDACTED]."),
    ] {
        let mut document = document("Zarnak Quorlim moved to Quorlim City.");
        let config = RedactionConfig {
            findings_path: Some(path.clone()),
            ..RedactionConfig::default()
        };

        redact_with_entities(&mut document, &config, &[]).expect("redaction must succeed");

        assert_eq!(document.content, expected, "{}", path.display());
    }
}

#[test]
fn should_report_the_line_of_a_malformed_json_lines_entry() {
    let error = parse_external_findings("{\"entity_type\": \"PERSON\", \"text\": \"Zarnak\"}\nnot json\n")
        .expect_err("line 2 is not JSON");
    assert!(error.to_string().contains("line 2"), "{error}");
}

#[test]
fn should_validate_findings_loaded_from_a_file() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("findings.json");
    std::fs::write(&path, r#"[{"entity_type": "PERSON"}]"#).expect("write");
    let mut document = document("Zarnak Quorlim.");
    let config = RedactionConfig {
        findings_path: Some(path),
        ..RedactionConfig::default()
    };

    let error =
        redact_with_entities(&mut document, &config, &[]).expect_err("a finding with no text or span must fail");

    assert!(error.to_string().contains("findings_path entry 0"), "{error}");
}

#[test]
fn should_reject_more_findings_than_the_security_limit_allows() {
    let findings = vec![
        text_finding("PERSON", "Zarnak"),
        text_finding("PERSON", "Quorlim"),
        text_finding("PERSON", "Blorp"),
    ];
    let config = ExtractionConfig {
        redaction: Some(with_findings(findings)),
        security_limits: Some(SecurityLimits {
            max_redaction_findings: 2,
            ..SecurityLimits::default()
        }),
        ..Default::default()
    };
    let mut document = document("Zarnak Quorlim met Blorp.");

    let error = run_processor(&mut document, &config).expect_err("three findings exceed a limit of two");

    assert!(error.to_string().contains("max_redaction_findings"), "{error}");
    assert_eq!(document.content, "Zarnak Quorlim met Blorp.");
}

#[test]
fn should_redact_through_the_post_processor_within_the_limit() {
    let config = ExtractionConfig {
        redaction: Some(with_findings(vec![text_finding("PERSON", "Zarnak Quorlim")])),
        ..Default::default()
    };
    let mut document = document("Zarnak Quorlim met Blorp.");

    run_processor(&mut document, &config).expect("redaction must succeed");

    assert_eq!(document.content, format!("{MASK} met Blorp."));
}
