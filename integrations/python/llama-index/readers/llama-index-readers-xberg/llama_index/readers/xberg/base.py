"""XbergReader — LlamaIndex reader for 106 document formats.

Wraps xberg's async, Rust-core extraction engine (``extract`` /
``extract_batch``) with true async support, maximalist metadata, and
element-aware output for the companion ``XbergNodeParser``.
"""

import asyncio
import logging
from collections.abc import AsyncIterator, Iterable
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any, Literal

from llama_index.core.readers.base import BasePydanticReader
from llama_index.core.schema import Document
from llama_index.readers.xberg._utils import (
    append_tables,
    build_metadata,
    excluded_keys,
    generate_doc_id,
)
from pydantic import Field, field_validator

from xberg import (
    ExtractedDocument,
    ExtractInput,
    ExtractionConfig,
    ExtractionResult,
    extract,
    extract_batch,
)

logger = logging.getLogger(__name__)

# Default result format so ``ExtractedDocument.elements`` is populated and the
# companion XbergNodeParser can split documents element-by-element. ~keep
_DEFAULT_RESULT_FORMAT = "element_based"


@dataclass(frozen=True, slots=True)
class _ExtractionTask:
    """Describes what to extract after input validation and routing.

    Built by ``_prepare_extractions``; single-item inputs (including a
    one-element list) use ``kind="file"`` or ``kind="bytes"``, multi-item
    inputs use the ``_batch`` variants which dispatch to ``extract_batch``.
    """

    kind: Literal["file", "file_batch", "bytes", "bytes_batch"]
    paths: tuple[Path, ...] = ()
    data_list: tuple[bytes, ...] = ()
    mime_types: tuple[str, ...] = ()


@dataclass(frozen=True, slots=True)
class _Source:
    """Tracks the origin of one extraction input for metadata/id purposes."""

    path: Path | None
    data: bytes | None


_DocSource = tuple[ExtractedDocument, _Source]


class XbergReader(BasePydanticReader):
    """Reader for 107 document formats powered by xberg's Rust extraction engine.

    Supports file paths, raw bytes, batch input, per-page splitting, and true
    async via xberg's native async ``extract`` / ``extract_batch`` functions.
    Multiple inputs are dispatched through ``extract_batch`` in a single call.

    Note:
        This is a local-only reader (``is_remote = False``). Remote/virtual
        filesystems (the ``fs`` parameter used by ``SimpleDirectoryReader``)
        are not supported.

    """

    is_remote: bool = False
    raise_on_error: bool = Field(
        default=False,
        description="If True, propagate xberg extraction failures. If False, log warnings and skip failed inputs.",
    )
    extraction_config: dict[str, Any] | None = Field(
        default=None,
        description="xberg ExtractionConfig (a dataclass) or a plain dict controlling output format, "
        "OCR, image extraction, result format, and all other extraction options.",
    )

    @classmethod
    def class_name(cls) -> str:
        """Return the canonical class name used for serialization."""
        return "XbergReader"

    @field_validator("extraction_config", mode="before")
    @classmethod
    def _validate_config(cls, v: dict[str, Any] | None) -> dict[str, Any] | None:
        if v is None:
            return None
        if isinstance(v, dict):
            return dict(v)
        if isinstance(v, ExtractionConfig):
            return asdict(v)
        msg = f"Expected ExtractionConfig, dict, or None, got {type(v)}"
        raise ValueError(msg)

    @staticmethod
    def _pages_requested(config: dict[str, Any]) -> bool:
        """Return True when the config opts into page extraction (dict or object)."""
        pages = config.get("pages")
        if pages is None:
            return False
        if isinstance(pages, dict):
            return bool(pages.get("extract_pages"))
        return bool(getattr(pages, "extract_pages", False))

    def _build_config(self) -> dict[str, Any]:
        """Return the ExtractionConfig dict to use, defaulting ``result_format``.

        With no explicit ``result_format``, the reader defaults to
        ``element_based`` so ``ExtractedDocument.elements`` is populated and
        forwarded to the companion node parser as one Document per source.

        The one exception is when the caller opts into page extraction
        (``pages.extract_pages``): ``element_based`` populates *both* pages and
        a single document-wide element stream, so splitting per page would
        replicate every element onto every page. In that case the reader
        defaults to ``unified`` to yield clean per-page Documents. An explicit
        ``result_format`` always wins.
        """
        config = dict(self.extraction_config or {})
        if "result_format" not in config:
            config["result_format"] = "unified" if self._pages_requested(config) else _DEFAULT_RESULT_FORMAT
        return config

    @staticmethod
    def _prepare_extractions(
        *,
        file_path: str | Path | list[str] | list[Path] | None = None,
        data: bytes | list[bytes] | None = None,
        mime_type: str | list[str] | None = None,
    ) -> _ExtractionTask:
        """Validate inputs and build an extraction task descriptor."""
        if file_path is not None:
            paths = tuple(Path(p) for p in file_path) if isinstance(file_path, list) else (Path(file_path),)
            if len(paths) == 1:
                return _ExtractionTask(kind="file", paths=paths)
            return _ExtractionTask(kind="file_batch", paths=paths)

        if data is not None:
            if isinstance(data, list):
                if not isinstance(mime_type, list) or len(data) != len(mime_type):
                    msg = "data and mime_type must be parallel lists of equal length"
                    raise ValueError(msg)
                return _ExtractionTask(kind="bytes_batch", data_list=tuple(data), mime_types=tuple(mime_type))
            if mime_type is None or isinstance(mime_type, list):
                msg = "mime_type must be a string for single bytes input"
                raise ValueError(msg)
            return _ExtractionTask(kind="bytes", data_list=(data,), mime_types=(mime_type,))

        msg = "Either file_path or data must be provided"
        raise ValueError(msg)

    @staticmethod
    def _build_inputs(task: _ExtractionTask) -> tuple[list[ExtractInput], list[_Source]]:
        """Build parallel lists of xberg inputs and their source descriptors."""
        if task.kind in ("file", "file_batch"):
            inputs = [ExtractInput(kind="uri", uri=str(path)) for path in task.paths]
            sources = [_Source(path=path, data=None) for path in task.paths]
            return inputs, sources
        inputs = [
            ExtractInput(kind="bytes", bytes=data, mime_type=mime)
            for data, mime in zip(task.data_list, task.mime_types, strict=True)
        ]
        sources = [_Source(path=None, data=data) for data in task.data_list]
        return inputs, sources

    def load_data(  # noqa: D102
        self,
        file_path: str | Path | list[str] | list[Path] | None = None,
        extra_info: dict[str, Any] | None = None,
        *,
        data: bytes | list[bytes] | None = None,
        mime_type: str | list[str] | None = None,
    ) -> list[Document]:
        return list(
            self.lazy_load_data(
                file_path=file_path,
                extra_info=extra_info,
                data=data,
                mime_type=mime_type,
            )
        )

    def lazy_load_data(  # noqa: D102
        self,
        file_path: str | Path | list[str] | list[Path] | None = None,
        extra_info: dict[str, Any] | None = None,
        *,
        data: bytes | list[bytes] | None = None,
        mime_type: str | list[str] | None = None,
    ) -> Iterable[Document]:
        # Sync bridge: run the whole (possibly batched) async extraction once. ~keep
        doc_sources = asyncio.run(
            self._extract(file_path=file_path, data=data, mime_type=mime_type),
        )
        yield from self._results_to_documents(doc_sources, extra_info)

    async def aload_data(  # noqa: D102
        self,
        file_path: str | Path | list[str] | list[Path] | None = None,
        extra_info: dict[str, Any] | None = None,
        *,
        data: bytes | list[bytes] | None = None,
        mime_type: str | list[str] | None = None,
    ) -> list[Document]:
        return [
            doc
            async for doc in self.alazy_load_data(
                file_path=file_path, extra_info=extra_info, data=data, mime_type=mime_type
            )
        ]

    async def alazy_load_data(  # type: ignore[override]  # noqa: D102
        self,
        file_path: str | Path | list[str] | list[Path] | None = None,
        extra_info: dict[str, Any] | None = None,
        *,
        data: bytes | list[bytes] | None = None,
        mime_type: str | list[str] | None = None,
    ) -> AsyncIterator[Document]:
        doc_sources = await self._extract(file_path=file_path, data=data, mime_type=mime_type)
        for doc in self._results_to_documents(doc_sources, extra_info):
            yield doc

    async def _extract(
        self,
        *,
        file_path: str | Path | list[str] | list[Path] | None = None,
        data: bytes | list[bytes] | None = None,
        mime_type: str | list[str] | None = None,
    ) -> list[_DocSource]:
        """Extract all inputs, dispatching single inputs to ``extract`` and
        multiple inputs to ``extract_batch``.
        """
        task = self._prepare_extractions(file_path=file_path, data=data, mime_type=mime_type)
        inputs, sources = self._build_inputs(task)
        config = self._build_config()

        try:
            if len(inputs) == 1:
                result = await extract(inputs[0], config)
            else:
                result = await extract_batch(inputs, config)
        except Exception:
            if self.raise_on_error:
                raise
            logger.warning("xberg extraction failed", exc_info=True)
            return []

        return self._map_results(result, sources)

    def _map_results(self, result: ExtractionResult, sources: list[_Source]) -> list[_DocSource]:
        """Pair extracted documents with their sources, handling per-input errors.

        ``extract_batch`` returns successful documents in ``result.results`` and
        per-input failures in ``result.errors`` (each carrying an input
        ``index``). Successful documents preserve input order, so the surviving
        sources are the inputs whose index is not in the error set.
        """
        failed_indices = {error.index for error in result.errors}
        for error in result.errors:
            logger.warning(
                "xberg failed to extract input %d (%s): %s",
                error.index,
                error.error_type,
                error.message,
            )
        if result.errors and self.raise_on_error:
            first = result.errors[0]
            msg = f"xberg extraction failed for input {first.index}: {first.message}"
            raise RuntimeError(msg)

        surviving = [source for index, source in enumerate(sources) if index not in failed_indices]
        return list(zip(result.results, surviving, strict=False))

    @staticmethod
    def _results_to_documents(
        doc_sources: list[_DocSource],
        extra_info: dict[str, Any] | None = None,
    ) -> Iterable[Document]:
        """Yield Documents from extracted documents.

        When an element stream or native chunk list is present the source
        becomes a single Document carrying ``_xberg_elements`` / ``_xberg_chunks``
        for the node parser. Otherwise, if pages are present, one Document is
        emitted per page. Elements and chunks are document-global, so per-page
        splitting is suppressed for them to avoid replicating every element or
        chunk onto every page.
        """
        for document, source in doc_sources:
            if document.pages and document.elements is None and not document.chunks:
                for page in document.pages:
                    content = append_tables(page.content, page.tables)
                    meta = build_metadata(
                        document=document,
                        file_path=source.path,
                        source="bytes" if source.data is not None else None,
                        extra_info=extra_info,
                        page_number=page.page_number,
                    )
                    excl = excluded_keys(meta)
                    yield Document(
                        text=content,
                        id_=generate_doc_id(file_path=source.path, data=source.data, page_number=page.page_number),
                        metadata=meta,
                        excluded_llm_metadata_keys=excl,
                        excluded_embed_metadata_keys=excl,
                    )
            else:
                content = append_tables(document.content, document.tables)
                meta = build_metadata(
                    document=document,
                    file_path=source.path,
                    source="bytes" if source.data is not None else None,
                    extra_info=extra_info,
                )
                excl = excluded_keys(meta)
                yield Document(
                    text=content,
                    id_=generate_doc_id(file_path=source.path, data=source.data),
                    metadata=meta,
                    excluded_llm_metadata_keys=excl,
                    excluded_embed_metadata_keys=excl,
                )
