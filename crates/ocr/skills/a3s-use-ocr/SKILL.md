---
name: a3s-use-ocr
description: Extract text and layout evidence from local image files through the built-in A3S Use PP-OCRv6 domain. Use when an agent needs optical character recognition for a PNG, JPEG, WebP, GIF, BMP, or TIFF image and must preserve the source digest, confidence, polygon, and bounding-box evidence.
---

# A3S Use OCR

Use the host-provided A3S Use surface. In an A3S Code `use` worker, call
`mcp__use_ocr__ocr_doctor`, `mcp__use_ocr__ocr_install`, and
`mcp__use_ocr__ocr_extract` directly. The host owns the MCP process; do not run
a shell command or read the file through another tool.

## Workflow

1. Call `mcp__use_ocr__ocr_doctor`.
2. If the pinned model bundle is missing or broken, call
   `mcp__use_ocr__ocr_install`. This bounded network mutation must pass the
   parent TUI confirmation. Do not replace it with a shell installation.
3. Confirm that `pp-ocr-v6`, `onnx-runtime`, and `PP-OCRv6_small` are ready.
4. Call `mcp__use_ocr__ocr_extract` with the exact local image path from the
   task.
5. Preserve the returned source path, media type, size, and SHA-256. Treat the
   decoded text, recognition/detection confidence, polygons, and bounding boxes
   as OCR evidence rather than verified source text.

The engine runs detection, reading-order sorting, perspective crop correction,
tall-crop rotation, recognition, and CTC decoding locally. It does not require
Python or PaddlePaddle and never sends the source image off the device. Offline
mode and `A3S_NO_AUTO_INSTALL=1` prohibit the bounded installer; return that
typed policy failure to the parent instead of attempting a fallback.

In a CLI-only host, equivalent commands are:

```bash
a3s use ocr doctor --json
a3s use ocr extract "$IMAGE" --json
```

The first extract automatically installs or repairs the pinned models when
networking and first-use installation are allowed. `doctor` remains read-only
and never downloads anything. `a3s install use/ocr` is available for explicit
preparation.

`a3s-use-ocr` accepts the same arguments when invoked as a standalone
development binary.

## Boundaries

- Only bounded local image files are accepted. URLs and PDF rasterization are
  outside this domain.
- Do not ask OCR to interpret unrelated content or present OCR output as
  verified source text.
- Do not hide empty results, warnings, model readiness failures, or source
  digest evidence from the parent agent.
