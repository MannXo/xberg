---
title: "Rust Core API"
description: Comprehensive guide to the wide Rust-only public surface of the xberg crate — plugin traits, embeddings, NER, chunking, diff, MIME detection, PDF rendering, and code intelligence.
---

The `xberg` crate exposes advanced primitives that are intentionally absent from language bindings. Use it directly to:
register custom extractors or OCR backends without writing any FFI; call embeddings, NER, and chunking pipelines from Rust without a binding layer; render PDF pages to PNG bytes; or diff two document extractions line by line. The APIs below are available either from the crate root or from the public module path shown.

For generated field-by-field references, see [`docs/reference/api-rust.md`](/reference/api-rust/) and [`docs/reference/configuration.md`](/reference/configuration/).

## Entry Points

The crate-level functions are the simplest extraction entry points:

```rust
pub async fn extract(
    input: ExtractInput,
    config: &ExtractionConfig,
) -> Result<ExtractionResult>;

pub async fn extract_batch(
    inputs: Vec<ExtractInput>,
    config: &ExtractionConfig,
) -> Result<ExtractionResult>;
```

`extract_batch` processes inputs concurrently when the `tokio-runtime` feature is active; otherwise it uses a sequential loop. Set `ExtractionConfig::concurrency` with `xberg::ConcurrencyConfig` to constrain Rust-side worker and thread budgets.

Both functions return an `ExtractionResult` envelope rather than a bare document vector. A URI may expand to multiple documents through crawling, so even `extract` can return more than one result. Initial batch outcomes are collected in input order, and each input may contribute zero or more documents; recursively followed document URLs are appended in traversal order. Per-input failures are recorded in `errors` while successful inputs remain in `results`. An outer `Err` means the operation itself could not run, for example because configuration validation failed or the operation was cancelled. An empty batch returns an empty successful envelope.

Rust applications that need reusable cache and progress implementations can construct `xberg::engine::Engine`. The crate-level functions use the default engine behavior.

**Constructing inputs**

```rust
// Local path, file:// URI, or https:// URL
let input = ExtractInput::from_uri("report.pdf");

// In-memory bytes — must supply MIME type and optional filename
let input = ExtractInput::from_bytes(bytes, "application/pdf", Some("report.pdf".into()));
```

`ExtractInputKind` distinguishes the two variants (`Uri` / `Bytes`) to inspect them.

## Result Types

`ExtractionResult` is the envelope; each element in `results` is an `ExtractedDocument`.

```rust
pub struct ExtractionResult {
    pub results: Vec<ExtractedDocument>,
    pub errors: Vec<ExtractionErrorItem>,   // non-fatal per-input errors
    pub summary: ExtractionSummary,
    pub crawl_final_urls: Vec<String>,
    pub crawl_redirect_count: usize,
    pub crawl_unique_normalized_urls: Vec<String>,
}
```

The convenience constructor `ExtractionResult::single(doc: ExtractedDocument) -> Self` wraps a single document.

`ExtractedDocument` (exported via `pub use types::*`) carries the per-document payload:

```rust
pub struct ExtractedDocument {
    pub content: String,
    pub mime_type: Cow<'static, str>,
    pub metadata: Metadata,
    pub tables: Vec<Table>,
    pub extraction_method: Option<ExtractionMethod>,
    pub detected_languages: Option<Vec<String>>,
    pub chunks: Option<Vec<Chunk>>,
    pub images: Option<Vec<ExtractedImage>>,
    pub pages: Option<Vec<PageContent>>,
    pub elements: Option<Vec<Element>>,
    // ...additional optional fields
}
```

## Configuration

`ExtractionConfig`, `ConcurrencyConfig`, and the detailed LLM budget, cache, provider, and rate-limit configs are re-exported at the crate root. Their defining module remains `xberg::core::config`, so both paths are available to Rust callers. Pass `ExtractionConfig::default()` to get safe defaults.

Key sub-configs that unlock capabilities at the field level:

| Field on `ExtractionConfig` | Type | Capability |
|---|---|---|
| `ocr` | `Option<OcrConfig>` | OCR backend selection and language |
| `chunking` | `Option<ChunkingConfig>` | Text chunking |
| `images` | `Option<ImageExtractionConfig>` | Image extraction |
| `pdf_options` | `Option<PdfConfig>` | PDF hierarchy and page settings |
| `ner` | `Option<NerConfig>` | Named-entity recognition |
| `redaction` | `Option<RedactionConfig>` | PII redaction patterns |
| `summarization` | `Option<SummarizationConfig>` | Document summarization |
| `translation` | `Option<TranslationConfig>` | Output translation |
| `layout` | `Option<LayoutDetectionConfig>` | Table and layout detection |
| `tree_sitter` | `Option<TreeSitterConfig>` | Code intelligence |
| `security_limits` | `Option<SecurityLimits>` | Archive and size caps |

See [`docs/reference/configuration.md`](/reference/configuration/) for all fields and defaults.

## Plugin System

Implement custom extractors, OCR backends, post-processors, embedding backends, rerankers, renderers, and validators by implementing the corresponding trait, then register it with the global registry. All traits are re-exported at the crate root.

**Base trait — every plugin implements this**

```rust
pub trait Plugin: Send + Sync {
    fn name(&self) -> &str;
    fn version(&self) -> String;
    fn initialize(&self) -> Result<()>;
    fn shutdown(&self) -> Result<()>;
}
```

**DocumentExtractor**

```rust
#[async_trait]
pub trait DocumentExtractor: Plugin {
    async fn extract(
        &self,
        input: ExtractInput,
        config: &ExtractionConfig,
    ) -> Result<ExtractedDocument>;

    fn supported_mime_types(&self) -> &[&str];

    // Default 50. Higher wins when multiple extractors support the same MIME.
    fn priority(&self) -> i32 { 50 }

    // Override for content-aware routing beyond MIME matching.
    fn can_handle(&self, path: &Path, mime_type: &str) -> bool { true }
}
```

**OcrBackend**

```rust
#[async_trait]
pub trait OcrBackend: Plugin {
    async fn process_image(
        &self,
        image_bytes: &[u8],
        config: &OcrConfig,
    ) -> Result<ExtractedDocument>;

    fn supports_language(&self, lang: &str) -> bool;
    fn backend_type(&self) -> OcrBackendType;
}
```

**Registry functions**

Each of the eight plugin types (`DocumentExtractor`, `OcrBackend`, `PostProcessor`, `EmbeddingBackend`, `RerankerBackend`, `TokenizerBackend`, `Renderer`, `Validator`) has four symmetric functions:

```rust
register_document_extractor(extractor: Arc<dyn DocumentExtractor>) -> Result<()>;
unregister_document_extractor(name: &str) -> Result<()>;
list_document_extractors() -> Result<Vec<String>>;
clear_document_extractors() -> Result<()>;
```

Replace `document_extractor` with `ocr_backend`, `post_processor`, `embedding_backend`, `reranker_backend`, `tokenizer_backend`, `renderer`, or `validator` to manage the other registries.

**Example — registering a custom extractor**

```rust
use std::sync::Arc;
use xberg::{
    ExtractInput, ExtractionConfig, ExtractedDocument, Result,
    plugins::{Plugin, DocumentExtractor},
    register_document_extractor,
};
use async_trait::async_trait;

struct JsonExtractor;

impl Plugin for JsonExtractor {
    fn name(&self) -> &str { "json-extractor" }
    fn version(&self) -> String { "1.0.0".into() }
    fn initialize(&self) -> Result<()> { Ok(()) }
    fn shutdown(&self) -> Result<()> { Ok(()) }
}

#[async_trait]
impl DocumentExtractor for JsonExtractor {
    async fn extract(&self, input: ExtractInput, _config: &ExtractionConfig)
        -> Result<ExtractedDocument>
    {
        let bytes = input.bytes.unwrap_or_default();
        // `ExtractedDocument` has `pub(crate)` internal fields, so a struct literal with
        // `..Default::default()` does not compile outside the crate. Build a default and
        // assign the public fields instead.
        let mut document = ExtractedDocument::default();
        document.content = String::from_utf8_lossy(&bytes).into_owned();
        document.mime_type = "application/json".into();
        Ok(document)
    }
    fn supported_mime_types(&self) -> &[&str] { &["application/json"] }
    fn priority(&self) -> i32 { 75 }
}

register_document_extractor(Arc::new(JsonExtractor))?;
```

## Embeddings and Reranking

Requires the `embeddings` / `embedding-presets` feature for embeddings; `reranker` / `reranker-presets` for reranking. Both expose a preset system backed by HuggingFace model repositories.

**Embeddings**

```rust
// Synchronous (all targets)
pub fn embed_texts(texts: Vec<String>, config: &EmbeddingConfig) -> Result<Vec<Vec<f32>>>;

// Async (requires tokio-runtime feature)
pub async fn embed_texts_async(texts: Vec<String>, config: &EmbeddingConfig) -> Result<Vec<Vec<f32>>>;

// Preset discovery
pub fn list_embedding_presets() -> Vec<String>;
pub fn get_embedding_preset(name: &str) -> Option<EmbeddingPreset>;
```

`EmbeddingPreset` fields include `name`, `model_repo`, `chunk_size`, `overlap`, `pooling`, `dimensions`, and `description`.

**Reranking** (requires `reranker` feature)

```rust
// Synchronous
pub fn rerank(
    query: String,
    documents: Vec<String>,
    config: &RerankerConfig,
) -> Result<Vec<RerankedDocument>>;

// Async (requires tokio-runtime feature)
pub async fn rerank_async(
    query: String,
    documents: Vec<String>,
    config: &RerankerConfig,
) -> Result<Vec<RerankedDocument>>;

pub fn list_reranker_presets() -> Vec<String>;
pub fn get_reranker_preset(name: &str) -> Option<RerankerPreset>;
```

`RerankedDocument` has `index: usize`, `score: f32`, and `document: String`. Documents are returned sorted descending by score.

## Text Enrichment

### Named-Entity Recognition

Requires the `ner` feature. Use either `LlmBackend` (HTTP providers via liter-llm, requires `ner-llm`) or `GlineBackend` (GLiNER ONNX, requires `ner-onnx`).

```rust
// Requires feature = "ner"
pub async fn detect_entities(
    text: &str,
    backend: &dyn NerBackend,
    categories: &[EntityCategory],
) -> Result<Vec<Entity>>;
```

Both backend types implement `NerBackend`. Each `Entity` carries category, span offsets, text, and confidence.

### Unified Enrichment Chokepoint

`enrich` is always available (no feature gate). It composes classification, NER, and image captioning in a single call:

```rust
pub async fn enrich(
    extraction: ExtractedDocument,
    config: &EnrichmentConfig,
) -> Result<EnrichedResult>;
```

`EnrichmentConfig` holds optional sub-configs:

```rust
pub struct EnrichmentConfig {
    pub classification: Option<ClassificationEnrichmentConfig>, // feature = "classification"
    pub ner: Option<NerEnrichmentConfig>,                       // feature = "ner"
    pub captioning: Option<CaptioningEnrichmentConfig>,         // feature = "captioning"
    // ...
}
```

`EnrichedResult` fields mirror the enabled stages.

`xberg::enrichment` is a separate schema-only module. Its `EnrichOptions`, `EnrichResult`, and `EnrichStatus` types are serializable request and lifecycle shapes for external enrichment workers; they do not execute the stages above.

### Image Captioning

Requires `captioning` + `tokio-runtime` features. Calls a vision-language model.

```rust
pub async fn caption_image(
    image_bytes: &[u8],
    llm_config: &LlmConfig,
    custom_prompt: Option<&str>,
) -> Result<String>;

pub async fn caption_image_file(
    path: impl AsRef<Path>,
    llm_config: &LlmConfig,
    custom_prompt: Option<&str>,
) -> Result<String>;

pub async fn caption_images(
    images: &[&[u8]],
    llm_config: &LlmConfig,
    custom_prompt: Option<&str>,
) -> Result<Vec<String>>;
```

### Keyword Extraction

Requires `keywords-yake` or `keywords-rake` feature. Types `Keyword`, `KeywordAlgorithm`, `KeywordConfig` are re-exported at the crate root. Functions live in `xberg::keywords`.

## Chunking

Requires the `chunking` feature. Three entry points cover the common cases:

```rust
// General chunking with optional page-boundary hints
pub fn chunk_text(
    text: &str,
    config: &ChunkingConfig,
    page_boundaries: Option<&[PageBoundary]>,
) -> Result<ChunkingResult>;

// RAG-optimised: enriches each chunk with heading-path context
pub fn chunk_for_rag(
    text: &str,
    config: &ChunkingConfig,
) -> Result<ChunkingResult>;

// Token counting against a model tokenizer (falls back to GPT-4o)
pub fn count_tokens(text: &str, model: Option<&str>) -> usize;
```

`ChunkingConfig` controls the chunker type (`ChunkerType`: `Markdown`, `Text`, `Semantic`, `Yaml`), `ChunkSizing` (character count or token count), and overlap. `ChunkingResult` wraps `Vec<Chunk>`; each `Chunk` carries `content`, `metadata: ChunkMetadata`, and optional embeddings.

Both `chunk_text` and `chunk_for_rag` are also accessible through `xberg::chunking::chunk_text` and `xberg::chunking::chunk_for_rag`.

## MIME Detection

Four functions are re-exported at the crate root; no feature gate required.

```rust
// Extension-based detection; set check_exists = true to verify the file exists
pub fn detect_mime_type(path: String, check_exists: bool) -> Result<String>;

// Magic-number detection from raw bytes
pub fn detect_mime_type_from_bytes(content: &[u8]) -> Result<String>;

// Reverse lookup: MIME type → registered file extensions
pub fn get_extensions_for_mime(mime_type: &str) -> Result<Vec<String>>;

// Returns the registered extension/MIME pairs available in this build
pub fn list_supported_formats() -> Vec<SupportedFormat>;
```

`SupportedFormat` carries `extension: String` and `mime_type: String`. The list is sorted by extension and reflects the active extractor registry, so feature-gated or unavailable extractors are not advertised.

## PDF Rendering

Requires the `pdf` feature. Renders PDF pages to PNG bytes using the bundled `xberg-native-pdf` renderer.

```rust
pub fn render_pdf_page_to_png(
    pdf_bytes: &[u8],
    page_index: usize,    // 0-indexed
    dpi: Option<i32>,     // None → auto-safe DPI (caps at 16 384 px per dimension)
    password: Option<&str>,
) -> Result<Vec<u8>>;

pub fn pdf_page_count(
    pdf_bytes: &[u8],
    password: Option<&str>,
) -> Result<usize>;

pub struct PdfRenderSession { /* private fields */ }

impl PdfRenderSession {
    pub fn open(pdf_bytes: &[u8], password: Option<&str>) -> Result<Self>;
    pub fn page_count(&self) -> usize;
    pub fn render_page_to_png(
        &self,
        page_index: usize,
        dpi: Option<i32>,
    ) -> Result<Vec<u8>>;
}
```

Import `PdfRenderSession` from `xberg::pdf`. Use it when rendering multiple pages from one document: the session opens and authenticates the PDF once, then reuses that parsed document for every page. The standalone functions remain convenient for one page or a page-count-only call.

The renderer automatically reduces DPI on very wide pages to avoid exceeding the 16 384 px dimension cap.

## Code Intelligence

Requires the `tree-sitter` feature. Delegates to `tree_sitter_language_pack::process`, re-exported as `process_code`.

```rust
// Alias for tree_sitter_language_pack::process
pub use tree_sitter_language_pack::process as process_code;
```

Configure via `TreeSitterProcessConfig` (structure extraction, imports, exports, symbols, comments, docstrings, diagnostics, chunk size, content mode). The result type is `ProcessResult`, which contains `Vec<CodeChunk>`, `FileMetrics`, and collected `StructureItem`s.

Config types re-exported at root: `TreeSitterConfig`, `TreeSitterProcessConfig`, `CodeContentMode`.
Result and data types re-exported at root: `ProcessResult`, `CodeChunk`, `StructureItem`, `StructureKind`, `SymbolInfo`, `SymbolKind`, `FileMetrics`, `ImportInfo`, `ExportInfo`, `CommentInfo`, `DocstringInfo`, `Span`, `Diagnostic`, `DiagnosticSeverity`.

## Diff

Requires the `diff` feature. Compare two `ExtractedDocument` values to produce a structured diff of text, tables, metadata, and embedded children.

```rust
pub fn compare(
    a: &ExtractedDocument,
    b: &ExtractedDocument,
    opts: &DiffOptions,
) -> ExtractionDiff;
```

`DiffOptions` has three fields: `include_metadata`, `include_embedded`, and `max_content_chars` (truncates `content` before diffing; `None` means no truncation). `ExtractionDiff` contains:

| Field | Type | Description |
|---|---|---|
| `content_diff` | `Vec<DiffHunk>` | Unified-diff hunks over plain text |
| `tables_added` | `Vec<Table>` | Tables only in `b` |
| `tables_removed` | `Vec<Table>` | Tables only in `a` |
| `tables_changed` | `Vec<TableDiff>` | Per-cell changes for matching tables |
| `metadata_changed` | `serde_json::Value` | Field-level metadata diff |
| `embedded_changes` | `EmbeddedChanges` | Added/removed/changed embedded files |

`DiffLine` and `CellChange` are always available (no feature gate) via `pub use types::*`.

**Example**

```rust
use xberg::{ExtractInput, ExtractionConfig, extract, diff::{compare, DiffOptions}};

async fn diff_two_docs() -> xberg::Result<()> {
    let config = ExtractionConfig::default();
    let a = extract(ExtractInput::from_uri("v1/report.pdf"), &config).await?;
    let b = extract(ExtractInput::from_uri("v2/report.pdf"), &config).await?;

    let diff = compare(&a.results[0], &b.results[0], &DiffOptions::default());
    println!("{} changed hunks", diff.content_diff.len());
    Ok(())
}
```

## Security Limits

`SecurityLimits` is re-exported at the crate root (no feature gate). Pass it via `ExtractionConfig::security_limits` to cap archive decompression, file counts, nesting depth, and text growth on user-supplied content.

```rust
pub struct SecurityLimits {
    pub max_archive_size: usize,
    pub max_compression_ratio: usize,
    pub max_files_in_archive: usize,
    pub max_nesting_depth: usize,
    // ...
}
```

`SecurityLimits::default()` bounds archive expansion, parser growth, nesting, iterations, XML depth, and table cells. `max_pages` defaults to `None`; set it explicitly to bound per-page OCR, layout, and rendering work for your workload.
