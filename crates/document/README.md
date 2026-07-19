# A3S Use Document

`a3s-use-document` combines bounded native Office structure with selected local
PP-OCRv6 evidence. It is the implementation behind the built-in
`use/document` CLI, standard MCP, and Skill surfaces.

The parser supports:

- DOCX, XLSX, and PPTX packages through `a3s-use-office`;
- standalone or embedded PNG, JPEG, WebP, GIF, BMP, and TIFF images; and
- local PP-OCRv6 Small detection and recognition through `a3s-use-ocr`.

PDF input is deliberately unsupported. The parser does not invoke Tesseract,
Python, PaddlePaddle, Microsoft Office, LibreOffice, Browser, or a remote OCR
service. Source bytes remain on the device.

## Workflow

Inspect before parsing:

```bash
a3s use document doctor --json
a3s use document inspect report.docx --json
a3s use document parse report.docx --ocr auto --json
```

Inspection reads native Office text, semantic units, and embedded image
metadata without running OCR or installing models. Each image receives one
recommendation:

- `required`: no native text layer is available;
- `suggested`: OCR is likely to add evidence to a unit with little native text;
- `optional`: native text is already substantial; or
- `unsupported`: the part is not an eligible bounded raster.

Parsing accepts three policies:

- `never` keeps native Office text only;
- `auto` selects `required` and `suggested` images; and
- `always` selects every eligible image.

Pass repeated `--image-path` values from inspection to narrow OCR to exact
semantic images. The default maximum is 8 selected image occurrences and the
hard maximum is 16. Repeated references to the same embedded image reuse one
inference result.

Native Office text remains the structural source of truth. OCR blocks are
separate evidence and are omitted from merged text when they duplicate native
content. Results preserve:

- source path, media type, size, archive SHA-256, and Office content SHA-256;
- semantic unit and image paths plus the OOXML source part;
- embedded-image SHA-256 and dimensions;
- native Office versus PP-OCRv6 origin;
- model, engine, recognition confidence, and detection confidence; and
- text polygon and bounding box.

## First-Use Model Preparation

The direct CLI calls `parse_with_first_use`. It prepares the shared
`PP-OCRv6_small` bundle only when the selected policy actually requires raster
inference. Complete official Use archives package this bundle next to
`a3s-use`, so normal umbrella-CLI installation resolves it without a second
download. If packaged or explicit assets are absent, the installer downloads
only the fixed-size, SHA-256-pinned official ONNX archives.

```bash
a3s install use/document
# Equivalent shared model preparation:
a3s install use/ocr
```

`A3S_OFFLINE=1`, `--offline`, and `A3S_NO_AUTO_INSTALL=1` prohibit a first-use
download. `A3S_OCR_MODEL_DIR` selects an explicit development model bundle, and
`A3S_USE_OCR_HOME` selects an isolated managed or packaged model root.

MCP keeps `document_inspect` and `document_parse` read-only and never downloads
models from either tool. If PP-OCRv6 is unavailable, a host may expose
`document_install_ocr` as a separately confirmed, idempotent network mutation,
then retry `document_parse`.

## Safety Bounds

The implementation admits at most a 64 MiB Office source, 4,096 package parts,
256 MiB of uncompressed package data, 256 discovered images, and 32 MiB per
raster. Native and merged text are bounded to 2 MiB. Image dimensions, decoded
allocation, output blocks, and OCR blocks per image are bounded as well.
Truncation and selection limits are returned as structured warnings rather than
silently discarded.
