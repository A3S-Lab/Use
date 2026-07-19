---
name: a3s-use-document
description: Inspect and parse local DOCX, XLSX, PPTX, and raster images by combining native Office structure with selected local PP-OCRv6 evidence. Use when an agent must recover document text while preserving unit, OOXML part, embedded-image digest, model, confidence, polygon, and bounding-box provenance.
---

# A3S Use Document

Use the host-provided A3S Use surface. In an A3S Code `use` worker, call
`mcp__use_document__document_doctor`,
`mcp__use_document__document_inspect`,
`mcp__use_document__document_install_ocr`, and
`mcp__use_document__document_parse` directly. The host owns the MCP process; do
not run a shell command or read the source through another tool.

## Workflow

1. Call `document_inspect` with the exact local path supplied by the task.
2. Treat native Office text as the structural source of truth. Review each
   semantic unit and image recommendation.
3. Use `ocr: "auto"` for images marked `required` or `suggested`. Use
   `ocr: "never"` when native content is sufficient, or `ocr: "always"` only
   when the task explicitly requires every eligible raster.
4. For a narrower operation, pass exact, repeatable `imagePaths` returned by
   inspection. Do not invent semantic paths.
5. If parsing reports missing or broken models, call `document_install_ocr`.
   This bounded network mutation must pass parent TUI confirmation. Retry
   `document_parse` only after it succeeds.
6. Preserve the source archive SHA-256, Office content SHA-256, semantic unit
   and path, OOXML image part, embedded-image SHA-256, native-versus-OCR origin,
   PP-OCRv6 model and engine, confidence, detection confidence, polygon, and
   bounding box.
7. Surface truncation, unsupported images, empty OCR results, and warnings.
   Never present OCR evidence as verified native text.

The native engine reads DOCX, XLSX, and PPTX in process. PP-OCRv6 detection and
recognition run locally through ONNX Runtime. The parser does not require
Tesseract, Python, PaddlePaddle, Microsoft Office, LibreOffice, Browser, or an
off-device service. It extracts embedded OOXML raster parts directly instead
of rendering a web page or installing a second browser.

In a CLI-only host, equivalent commands are:

```bash
a3s use document doctor --json
a3s use document inspect "$DOCUMENT" --json
a3s use document parse "$DOCUMENT" --ocr auto --json
```

The first direct CLI parse that actually selects raster evidence automatically
installs or repairs the pinned PP-OCRv6 models when networking and first-use
installation are allowed. `doctor` and `inspect` remain read-only and never
download anything. `a3s install use/document` and `a3s install use/ocr` are
available for explicit preparation.

## Boundaries

- Supported Office sources are DOCX, XLSX, and PPTX. Word units are body,
  header, and footer semantic regions; do not invent physical page numbers.
  Spreadsheet units are worksheets and Presentation units are slides.
- Supported standalone or embedded raster formats are PNG, JPEG, WebP, GIF,
  BMP, and TIFF.
- PDF parsing and rasterization are deliberately unsupported. Return the typed
  error instead of using Browser, shell utilities, or an unrelated fallback.
- `document_parse` is read-only and never silently installs models. Only the
  bounded install tool may prepare them inside an MCP workflow.
- Offline mode and `A3S_NO_AUTO_INSTALL=1` prohibit the bounded installer.
  Return that typed policy failure instead of attempting a fallback.
