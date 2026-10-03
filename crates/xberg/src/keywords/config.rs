//! Configuration for keyword extraction.

use super::types::KeywordAlgorithm;
use serde::{Deserialize, Deserializer, Serialize};

fn default_max_keywords() -> usize {
    10
}

/// Inclusive word-count range used to form keyword candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct NgramRange {
    /// Minimum number of words in a candidate.
    pub min: usize,
    /// Maximum number of words in a candidate.
    pub max: usize,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum NgramRangeWire {
    Positional((usize, usize)),
    Named { min: usize, max: usize },
}

impl<'de> Deserialize<'de> for NgramRange {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let range = match NgramRangeWire::deserialize(deserializer)? {
            NgramRangeWire::Positional(range) => range.into(),
            NgramRangeWire::Named { min, max } => Self { min, max },
        };

        range.validate().map_err(serde::de::Error::custom)
    }
}

impl NgramRange {
    fn validate(self) -> Result<Self, String> {
        if self.min == 0 {
            return Err("ngram range minimum must be at least 1, got 0".to_string());
        }
        if self.min > self.max {
            return Err(format!(
                "ngram range minimum must not exceed maximum ({} > {})",
                self.min, self.max
            ));
        }

        Ok(self)
    }
}

#[cfg(feature = "api")]
impl utoipa::PartialSchema for NgramRange {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        use utoipa::openapi::schema::{ObjectBuilder, Type};

        let positive_integer = ObjectBuilder::new().schema_type(Type::Integer).minimum(Some(1)).build();

        ObjectBuilder::new()
            .property("min", positive_integer.clone())
            .required("min")
            .property("max", positive_integer)
            .required("max")
            .into()
    }
}

#[cfg(feature = "api")]
impl utoipa::ToSchema for NgramRange {}

impl From<(usize, usize)> for NgramRange {
    fn from((min, max): (usize, usize)) -> Self {
        Self { min, max }
    }
}

impl From<NgramRange> for (usize, usize) {
    fn from(range: NgramRange) -> Self {
        (range.min, range.max)
    }
}

impl Default for NgramRange {
    fn default() -> Self {
        Self { min: 1, max: 3 }
    }
}

/// YAKE-specific parameters.
#[cfg(feature = "keywords-yake")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(all(feature = "api", feature = "keywords-yake"), derive(utoipa::ToSchema))]
pub struct YakeParams {
    /// Window size for co-occurrence analysis (default: 2).
    ///
    /// Controls the context window for computing co-occurrence statistics.
    pub window_size: usize,
}

#[cfg(feature = "keywords-yake")]
impl Default for YakeParams {
    fn default() -> Self {
        Self { window_size: 2 }
    }
}

/// RAKE-specific parameters.
#[cfg(feature = "keywords-rake")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(all(feature = "api", feature = "keywords-rake"), derive(utoipa::ToSchema))]
pub struct RakeParams {
    /// Minimum word length to consider (default: 1).
    pub min_word_length: usize,

    /// Maximum words in a keyword phrase (default: 3).
    pub max_words_per_phrase: usize,
}

#[cfg(feature = "keywords-rake")]
impl Default for RakeParams {
    fn default() -> Self {
        Self {
            min_word_length: 1,
            max_words_per_phrase: 3,
        }
    }
}

/// Keyword extraction configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct KeywordConfig {
    /// Algorithm to use for extraction.
    #[serde(default)]
    pub algorithm: KeywordAlgorithm,

    /// Maximum number of keywords to extract (default: 10).
    #[serde(default = "default_max_keywords")]
    pub max_keywords: usize,

    /// Minimum score threshold (0.0-1.0, default: 0.0).
    ///
    /// Keywords with scores below this threshold are filtered out.
    /// Note: Score ranges differ between algorithms.
    #[serde(default)]
    pub min_score: f32,

    /// N-gram range for keyword extraction (min, max).
    ///
    /// (1, 1) = unigrams only
    /// (1, 2) = unigrams and bigrams
    /// (1, 3) = unigrams, bigrams, and trigrams (default)
    #[serde(default)]
    pub ngram_range: NgramRange,

    /// Language code for stopword filtering (e.g., "en", "de", "fr").
    ///
    /// If None, no stopword filtering is applied.
    pub language: Option<String>,

    /// YAKE-specific tuning parameters.
    #[cfg(feature = "keywords-yake")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "api", schema(nullable = false))]
    pub yake_params: Option<YakeParams>,

    /// RAKE-specific tuning parameters.
    #[cfg(feature = "keywords-rake")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "api", schema(nullable = false))]
    pub rake_params: Option<RakeParams>,
}

impl Default for KeywordConfig {
    fn default() -> Self {
        Self {
            algorithm: KeywordAlgorithm::default(),
            max_keywords: 10,
            min_score: 0.0,
            ngram_range: NgramRange::default(),
            language: Some("en".to_string()),
            #[cfg(feature = "keywords-yake")]
            yake_params: None,
            #[cfg(feature = "keywords-rake")]
            rake_params: None,
        }
    }
}

impl KeywordConfig {
    pub(crate) fn validate(&self) -> crate::Result<()> {
        if !self.min_score.is_finite() || !(0.0..=1.0).contains(&self.min_score) {
            return Err(crate::XbergError::validation(format!(
                "keywords.min_score must be a finite value between 0.0 and 1.0, got {}",
                self.min_score
            )));
        }

        self.ngram_range
            .validate()
            .map(|_| ())
            .map_err(|message| crate::XbergError::Validation { message, source: None })
    }
}

#[cfg(test)]
impl KeywordConfig {
    /// Create a new configuration with YAKE algorithm.
    #[cfg(feature = "keywords-yake")]
    pub(crate) fn yake() -> Self {
        Self {
            algorithm: KeywordAlgorithm::Yake,
            ..Default::default()
        }
    }

    /// Create a new configuration with RAKE algorithm.
    #[cfg(feature = "keywords-rake")]
    pub(crate) fn rake() -> Self {
        Self {
            algorithm: KeywordAlgorithm::Rake,
            ..Default::default()
        }
    }

    /// Set maximum number of keywords to extract.
    #[cfg(feature = "keywords-yake")]
    pub(crate) fn with_max_keywords(mut self, max: usize) -> Self {
        self.max_keywords = max;
        self
    }

    /// Set minimum score threshold.
    pub(crate) fn with_min_score(mut self, score: f32) -> Self {
        self.min_score = score;
        self
    }

    /// Set n-gram range.
    pub(crate) fn with_ngram_range(mut self, min: usize, max: usize) -> Self {
        self.ngram_range = NgramRange { min, max };
        self
    }

    /// Set language for stopword filtering.
    #[cfg(all(test, feature = "keywords-rake"))]
    pub(crate) fn with_language(mut self, lang: impl Into<String>) -> Self {
        self.language = Some(lang.into());
        self
    }

    /// Set YAKE-specific parameters.
    #[cfg(feature = "keywords-yake")]
    pub(crate) fn with_yake_params(mut self, params: YakeParams) -> Self {
        self.yake_params = Some(params);
        self
    }

    /// Set RAKE-specific parameters.
    #[cfg(feature = "keywords-rake")]
    pub(crate) fn with_rake_params(mut self, params: RakeParams) -> Self {
        self.rake_params = Some(params);
        self
    }
}

#[cfg(test)]
mod binding_value_serde_tests {
    use super::{KeywordConfig, NgramRange};
    use serde_json::json;

    #[cfg(feature = "api")]
    fn assert_named_object_schema<T: utoipa::PartialSchema>(expected: serde_json::Value) {
        let schema = serde_json::to_value(T::schema()).expect("schema must serialize");
        assert_eq!(schema, expected);
    }

    #[cfg(feature = "api")]
    #[test]
    fn should_describe_ngram_range_as_named_object_schema() {
        assert_named_object_schema::<NgramRange>(json!({
            "type": "object",
            "required": ["min", "max"],
            "properties": {
                "min": {"type": "integer", "minimum": 1},
                "max": {"type": "integer", "minimum": 1}
            }
        }));
    }

    #[test]
    fn should_serialize_ngram_range_as_named_object() {
        let legacy = json!([1, 3]);
        let named_json = json!({"min": 1, "max": 3});
        let range: NgramRange = serde_json::from_value(legacy).expect("legacy range must deserialize");
        let named: NgramRange = serde_json::from_value(named_json.clone()).expect("named range must deserialize");

        assert_eq!(range, NgramRange { min: 1, max: 3 });
        assert_eq!(named, range);
        assert_eq!(serde_json::to_value(range).expect("range must serialize"), named_json);
        assert_eq!(
            serde_json::to_value(named).expect("named range must serialize"),
            named_json
        );
    }

    #[test]
    fn should_reject_zero_ngram_range_in_both_wire_shapes() {
        for value in [json!([0, 0]), json!({"min": 0, "max": 0})] {
            let error = serde_json::from_value::<NgramRange>(value).expect_err("zero range must be rejected");

            assert_eq!(error.to_string(), "ngram range minimum must be at least 1, got 0");
        }
    }

    #[test]
    fn should_reject_reversed_ngram_range_in_both_wire_shapes() {
        for value in [json!([4, 2]), json!({"min": 4, "max": 2})] {
            let error = serde_json::from_value::<NgramRange>(value).expect_err("reversed range must be rejected");

            assert_eq!(error.to_string(), "ngram range minimum must not exceed maximum (4 > 2)");
        }
    }

    #[test]
    fn should_deserialize_keyword_config_with_legacy_range() {
        let config: KeywordConfig = serde_json::from_value(json!({
            "algorithm": "rake",
            "max_keywords": 5,
            "min_score": 0.1,
            "ngram_range": [2, 4],
            "language": "en"
        }))
        .expect("legacy keyword config must deserialize");

        assert_eq!(config.ngram_range, NgramRange { min: 2, max: 4 });
        assert_eq!(
            serde_json::to_value(config.ngram_range).expect("range must serialize"),
            json!({"min": 2, "max": 4})
        );
    }
}
