#![cfg(feature = "api")]
//! Shape of optional reference properties in the emitted OpenAPI document (GH#1841).
//!
//! An `Option<T>` whose serde contract omits `None` must be typed as a direct `$ref`, not as
//! `oneOf[$ref, {"type":"null"}]`. An `Option<T>` that serialises `None` as JSON `null` must keep
//! the union. Inner primitive nullability is a separate contract and is left alone.

use serde_json::Value;

/// The optional references that deliberately serialise `None` as JSON `null`, because they carry no
/// `skip_serializing_if`. Every other optional reference in the document is omission-only.
const DELIBERATELY_NULLABLE_REFERENCES: [&str; 4] = [
    "DocumentRevision.anchor",
    "ElementMetadata.coordinates",
    "ImageMetadataType.dimensions",
    "ImagePreprocessingMetadata.new_dimensions",
];

fn spec() -> Value {
    serde_json::from_str(&xberg::api::openapi::openapi_json()).expect("openapi_json must emit valid JSON")
}

fn property<'a>(spec: &'a Value, schema: &str, field: &str) -> &'a Value {
    spec.pointer(&format!("/components/schemas/{schema}/properties/{field}"))
        .unwrap_or_else(|| panic!("components.schemas.{schema}.properties.{field} is absent from the spec"))
}

fn required_fields(spec: &Value, schema: &str) -> Vec<String> {
    spec.pointer(&format!("/components/schemas/{schema}/required"))
        .and_then(Value::as_array)
        .map(|entries| entries.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Every property in the whole document that is a `oneOf` mixing a `$ref` branch with a
/// `{"type":"null"}` branch, named rather than counted so a failure identifies the offenders.
fn null_reference_unions(spec: &Value) -> Vec<String> {
    let schemas = spec
        .pointer("/components/schemas")
        .and_then(Value::as_object)
        .expect("the document must define components.schemas");
    assert!(!schemas.is_empty(), "components.schemas must not be empty");

    let mut found = Vec::new();
    for (schema_name, schema) in schemas {
        let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
            continue;
        };
        for (field, property) in properties {
            let Some(branches) = property.get("oneOf").and_then(Value::as_array) else {
                continue;
            };
            let has_null = branches
                .iter()
                .any(|branch| branch.get("type").and_then(Value::as_str) == Some("null"));
            let has_reference = branches.iter().any(|branch| branch.get("$ref").is_some());
            if has_null && has_reference {
                found.push(format!("{schema_name}.{field}"));
            }
        }
    }
    found.sort();
    found
}

#[test]
fn should_emit_a_direct_reference_when_the_serde_contract_omits_none() {
    let spec = spec();
    for (schema, field, target) in [
        ("ExtractedDocument", "djot_content", "DjotContent"),
        ("ExtractedDocument", "document", "DocumentStructure"),
        ("Table", "bounding_box", "BoundingBox"),
        ("PageContent", "hierarchy", "PageHierarchy"),
        ("ExtractedImage", "bounding_box", "BoundingBox"),
    ] {
        let property = property(&spec, schema, field);

        assert!(
            property.get("oneOf").is_none(),
            "{schema}.{field} is omission-only and must not be a null union, got {property}"
        );
        assert_eq!(
            property.get("$ref").and_then(Value::as_str),
            Some(format!("#/components/schemas/{target}").as_str()),
            "{schema}.{field} must be a direct reference to {target}, got {property}"
        );
        assert!(
            !required_fields(&spec, schema).iter().any(|name| name == field),
            "{schema}.{field} must stay out of `required` — nullable = false must not make it required"
        );
    }
}

/// Asserts the omission-only shape on a type's own schema, for config types the document does not
/// register but downstream specs that embed them do (GH#2041).
fn assert_omission_only_reference<T>(field: &str, target: &str)
where
    T: utoipa::ToSchema + serde::Serialize + Default,
{
    let schema = T::name();
    let wire = serde_json::to_value(T::default()).expect("a default config must serialise");
    assert!(
        wire.get(field).is_none(),
        "{schema}.{field} must be absent, not null, when unset"
    );

    let definition = serde_json::to_value(T::schema()).expect("a derived schema must serialise");
    let property = definition
        .pointer(&format!("/properties/{field}"))
        .unwrap_or_else(|| panic!("{schema}.{field} is absent from its schema"));
    assert!(
        property.get("oneOf").is_none(),
        "{schema}.{field} is omission-only and must not be a null union, got {property}"
    );
    assert_eq!(
        property.get("$ref").and_then(Value::as_str),
        Some(format!("#/components/schemas/{target}").as_str()),
        "{schema}.{field} must be a direct reference to {target}, got {property}"
    );
    assert!(
        property.get("default").is_none(),
        "{schema}.{field} must not advertise a default, got {property}"
    );
    let required = definition.get("required").and_then(Value::as_array);
    assert!(
        !required.is_some_and(|names| names.iter().any(|name| name.as_str() == Some(field))),
        "{schema}.{field} must stay out of `required`"
    );
}

#[test]
fn should_emit_a_direct_reference_for_omission_only_config_fields() {
    assert_omission_only_reference::<xberg::types::TesseractConfig>("preprocessing", "ImagePreprocessingConfig");
    #[cfg(feature = "keywords-yake")]
    assert_omission_only_reference::<xberg::KeywordConfig>("yake_params", "YakeParams");
    #[cfg(feature = "keywords-rake")]
    assert_omission_only_reference::<xberg::KeywordConfig>("rake_params", "RakeParams");
}

#[test]
fn should_carry_the_description_on_the_property_rather_than_a_union_branch() {
    let spec = spec();
    let property = property(&spec, "ExtractedDocument", "djot_content");
    let description = property
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("djot_content must describe itself on the property, got {property}"));
    assert!(
        description.contains("Djot"),
        "djot_content's description must survive the flattening, got {description:?}"
    );
}

#[test]
fn should_keep_the_null_union_when_none_serialises_as_null() {
    let spec = spec();
    for (schema, field) in [
        ("ElementMetadata", "coordinates"),
        ("DocumentRevision", "anchor"),
        ("ImageMetadataType", "dimensions"),
        ("ImagePreprocessingMetadata", "new_dimensions"),
    ] {
        let property = property(&spec, schema, field);
        let branches = property.get("oneOf").and_then(Value::as_array).unwrap_or_else(|| {
            panic!("{schema}.{field} serialises None as null and must stay a union, got {property}")
        });
        assert!(
            branches
                .iter()
                .any(|branch| branch.get("type").and_then(Value::as_str) == Some("null")),
            "{schema}.{field}'s union must retain its null branch, got {property}"
        );
    }
}

#[test]
fn should_leave_exactly_the_deliberately_nullable_references_as_unions() {
    let expected: Vec<String> = DELIBERATELY_NULLABLE_REFERENCES
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    assert_eq!(
        null_reference_unions(&spec()),
        expected,
        "every other optional reference is omission-only and must be a direct $ref"
    );
}

#[cfg(feature = "heuristics")]
#[test]
fn should_keep_inner_primitive_nullability_while_flattening_the_outer_reference() {
    let spec = spec();

    let outer = property(&spec, "ExtractedDocument", "extraction_confidence");
    assert!(
        outer.get("oneOf").is_none(),
        "extraction_confidence must be a direct $ref, got {outer}"
    );
    assert_eq!(
        outer.get("$ref").and_then(Value::as_str),
        Some("#/components/schemas/ExtractionConfidence")
    );

    let inner = property(&spec, "ExtractionConfidence", "ocr_aggregate");
    let types = inner
        .get("type")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ocr_aggregate must carry a type array, got {inner}"));
    assert!(
        types.iter().any(|entry| entry.as_str() == Some("null")),
        "ExtractionConfidence.ocr_aggregate is a separate contract and must stay nullable, got {inner}"
    );
}

#[test]
fn should_omit_an_absent_optional_reference_from_the_wire() {
    let json = serde_json::to_value(xberg::ExtractedDocument::default()).expect("a default document must serialise");
    for field in ["djot_content", "document", "extraction_method", "summary"] {
        assert!(
            json.get(field).is_none(),
            "{field} must be absent, not null, when unset"
        );
    }
    assert_eq!(
        json.get("content").and_then(Value::as_str),
        Some(""),
        "a non-optional field must still be present"
    );
}
