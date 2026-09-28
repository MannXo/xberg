//! End-to-end tests for `--redaction-findings` (xberg-io/xberg#1941).
//!
//! All PII in these tests is synthetic and built in-test.

#![cfg(feature = "redaction")]

use std::io::Write;
use std::process::{Command, Output, Stdio};

const DOCUMENT: &str = "Zarnak Quorlim moved to Quorlim City.";
const PRESIDIO: &str = r#"[{"entity_type": "PERSON", "start": 0, "end": 14, "score": 0.85}]"#;

fn xberg(args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_xberg"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("xberg must start");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("xberg must finish")
}

fn document() -> (tempfile::TempDir, String) {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("letter.txt");
    std::fs::write(&path, DOCUMENT).expect("write document");
    (directory, path.to_string_lossy().into_owned())
}

fn assert_redacted(output: &Output) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "xberg failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("[REDACTED] moved to Quorlim City."), "{stdout}");
    assert!(!stdout.contains("Zarnak"), "{stdout}");
}

#[test]
fn should_redact_findings_read_from_stdin() {
    let (_directory, path) = document();

    let output = xberg(
        &["extract", &path, "--no-config-discovery", "--redaction-findings", "-"],
        PRESIDIO,
    );

    assert_redacted(&output);
}

#[test]
fn should_redact_findings_read_from_a_file() {
    let (directory, path) = document();
    let findings = directory.path().join("findings.jsonl");
    std::fs::write(&findings, format!("{}\n", &PRESIDIO[1..PRESIDIO.len() - 1])).expect("write findings");

    let output = xberg(
        &[
            "extract",
            &path,
            "--no-config-discovery",
            "--redaction-findings",
            &findings.to_string_lossy(),
        ],
        "",
    );

    assert_redacted(&output);
}

#[test]
fn should_refuse_to_read_both_the_document_and_the_findings_from_stdin() {
    let output = xberg(
        &[
            "extract",
            "--stdin",
            "--no-config-discovery",
            "--redaction-findings",
            "-",
        ],
        DOCUMENT,
    );

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot both read from stdin"), "{stderr}");
}

#[test]
fn should_fail_on_a_finding_it_cannot_resolve() {
    let (_directory, path) = document();

    let output = xberg(
        &["extract", &path, "--no-config-discovery", "--redaction-findings", "-"],
        r#"[{"entity_type": "PERSON", "start": 0, "end": 400}]"#,
    );

    assert!(
        !output.status.success(),
        "an out-of-range span must fail the extraction"
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Zarnak"));
}
