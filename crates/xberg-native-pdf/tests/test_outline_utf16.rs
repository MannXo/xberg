//! Reproduction for UTF-16BE bookmark titles (the form hyperref emits
//! by default). `/Title` may be a UTF-16BE string with a `FEFF` BOM,
//! expressed either as a literal `(...)` or hex `<...>` string.

// ~keep: test/bench binaries print by design; org logging policy exempts tests
#![allow(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)]

use xberg_native_pdf::document::PdfDocument;
use xberg_native_pdf::outline::Destination;

/// Build a minimal one-page PDF whose outline has two bookmarks, both
/// titled "Proof of Lemma 1" encoded as UTF-16BE with a BOM — one as a
/// literal string, one as a hex string. xref offsets are computed so
/// the file parses without reconstruction.
fn build_pdf(title_literal: &[u8], title_hex: &str) -> Vec<u8> {
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /Outlines 4 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_vec(),
        b"<< /Type /Outlines /First 5 0 R /Last 6 0 R /Count 2 >>".to_vec(),
    ];

    let mut item1 = Vec::new();
    item1.extend_from_slice(b"<< /Title (");
    item1.extend_from_slice(title_literal);
    item1.extend_from_slice(b") /Parent 4 0 R /Next 6 0 R /Dest [3 0 R /Fit] >>");
    objects.push(item1);

    let mut item2 = Vec::new();
    item2.extend_from_slice(b"<< /Title <");
    item2.extend_from_slice(title_hex.as_bytes());
    item2.extend_from_slice(b"> /Parent 4 0 R /Prev 5 0 R /Dest [3 0 R /Fit] >>");
    objects.push(item2);

    let mut pdf = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.7\n");
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(b"\nendobj\n");
    }

    let xref_offset = pdf.len();
    let n = objects.len() + 1;
    pdf.extend_from_slice(format!("xref\n0 {}\n", n).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for off in &offsets {
        pdf.extend_from_slice(format!("{:010} 00000 n \n", off).as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF",
            n, xref_offset
        )
        .as_bytes(),
    );
    pdf
}

#[test]
fn utf16be_bookmark_titles_decode() {
    let title = "Proof of Lemma 1";
    let mut utf16be = vec![0xFEu8, 0xFF];
    for u in title.encode_utf16() {
        utf16be.extend_from_slice(&u.to_be_bytes());
    }
    let hex: String = utf16be.iter().map(|b| format!("{:02X}", b)).collect();

    let pdf = build_pdf(&utf16be, &hex);
    let doc = PdfDocument::from_bytes(pdf).expect("open");
    let outline = doc.get_outline().expect("get_outline").expect("some");

    eprintln!("literal title = {:?}", outline[0].title);
    eprintln!("hex     title = {:?}", outline[1].title);

    assert_eq!(outline[0].title, title, "literal UTF-16BE title");
    assert_eq!(outline[1].title, title, "hex UTF-16BE title");
}

/// `/Title` given as an indirect reference to a UTF-16BE string must
/// also resolve and decode (rather than falling back to "(No Title)").
#[test]
fn utf16be_bookmark_title_indirect_reference() {
    let title = "Proof of Lemma 1";
    let mut utf16be = vec![0xFEu8, 0xFF];
    for u in title.encode_utf16() {
        utf16be.extend_from_slice(&u.to_be_bytes());
    }

    let mut pdf = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.7\n");
    let mut bodies: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /Outlines 4 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_vec(),
        b"<< /Type /Outlines /First 5 0 R /Last 5 0 R /Count 1 >>".to_vec(),
        b"<< /Title 6 0 R /Parent 4 0 R /Dest [3 0 R /Fit] >>".to_vec(),
    ];
    let mut str_obj = Vec::new();
    str_obj.extend_from_slice(b"(");
    str_obj.extend_from_slice(&utf16be);
    str_obj.extend_from_slice(b")");
    bodies.push(str_obj);

    let mut offsets = Vec::new();
    for (i, body) in bodies.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref_offset = pdf.len();
    let n = bodies.len() + 1;
    pdf.extend_from_slice(format!("xref\n0 {}\n", n).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for off in &offsets {
        pdf.extend_from_slice(format!("{:010} 00000 n \n", off).as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF",
            n, xref_offset
        )
        .as_bytes(),
    );

    let doc = PdfDocument::from_bytes(pdf).expect("open");
    let outline = doc.get_outline().expect("get_outline").expect("some");
    assert_eq!(outline[0].title, title, "indirect-reference UTF-16BE title");
}

/// GH #1589: a `/Dest` byte string that names an entry in the
/// `/Names` -> `/Dests` name tree (ISO 32000-1 §7.9.6) is a *byte*
/// string, ordered and compared by byte — not a text string. A
/// UTF-16BE-with-BOM key, what Adobe Distiller writes, must resolve
/// to its page exactly like an ASCII key.
///
/// Before the fix, `resolve_destination`'s `Object::String` arm
/// decoded the `/Dest` bytes with `String::from_utf8_lossy` and
/// looked *that* string up, mangling the `FE FF` BOM into two U+FFFD
/// replacement characters (`EF BF BD` each in UTF-8) before the
/// lookup ever ran. The tree is keyed by the raw `FE FF 00 41 00 33`
/// bytes, so the mangled `EF BF BD EF BF BD 00 41 00 33` search could
/// never match, and the bookmark resolved to
/// `Destination::Named("\u{FFFD}\u{FFFD}\0A\03")` with no page rather
/// than `Destination::PageIndex(0)`.
#[test]
fn utf16be_named_destination_resolves_to_page() {
    // UTF-16BE-with-BOM encoding of the name "A3".
    let key: &[u8] = &[0xFE, 0xFF, 0x00, 0x41, 0x00, 0x33];
    let key_hex: String = key.iter().map(|b| format!("{:02X}", b)).collect();

    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /Outlines 4 0 R /Names 6 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_vec(),
        b"<< /Type /Outlines /First 5 0 R /Last 5 0 R /Count 1 >>".to_vec(),
        format!("<< /Title (Section A3) /Parent 4 0 R /Dest <{key_hex}> >>").into_bytes(),
        b"<< /Dests 7 0 R >>".to_vec(),
        format!("<< /Names [<{key_hex}> [3 0 R /Fit]] >>").into_bytes(),
    ];

    let mut pdf = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.7\n");
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(b"\nendobj\n");
    }

    let xref_offset = pdf.len();
    let n = objects.len() + 1;
    pdf.extend_from_slice(format!("xref\n0 {}\n", n).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for off in &offsets {
        pdf.extend_from_slice(format!("{:010} 00000 n \n", off).as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF",
            n, xref_offset
        )
        .as_bytes(),
    );

    let doc = PdfDocument::from_bytes(pdf).expect("open");
    let outline = doc.get_outline().expect("get_outline").expect("some");

    assert_eq!(outline.len(), 1);
    assert_eq!(outline[0].title, "Section A3");
    assert!(
        matches!(outline[0].dest, Some(Destination::PageIndex(0))),
        "expected the UTF-16BE-with-BOM named destination to resolve to page 0, got {:?}",
        outline[0].dest
    );
}
