---
name: a3s-use-ocr
description: Extract text and layout evidence from local image files through the built-in A3S Use PP-OCRv6 domain. Use when an agent needs optical character recognition for a PNG, JPEG, WebP, GIF, BMP, or TIFF image and must preserve the source digest, confidence, polygon, and bounding-box evidence.
---

# A3S Use OCR

Use the host-provided A3S Use surface. In an A3S Code `use` worker, call
`mcp__use_ocr__ocr_doctor` and `mcp__use_ocr__ocr_extract` directly. The host
owns the MCP process; do not run a shell command, install models, or read the
file through another tool.

## Workflow

1. Call `mcp__use_ocr__ocr_doctor`.
2. Confirm that `pp-ocr-v6`, `onnx-runtime`, and `PP-OCRv6_small` are ready.
3. Call `mcp__use_ocr__ocr_extract` with the exact local image path from the
   task.
4. Preserve the returned source path, media type, size, and SHA-256. Treat the
   decoded text, recognition/detection confidence, polygons, and bounding boxes
   as OCR evidence rather than verified source text.

The engine runs detection, reading-order sorting, perspective crop correction,
tall-crop rotation, recognition, and CTC decoding locally. It does not require
Python or PaddlePaddle and never sends the source image off the device. If the
doctor reports missing or damaged models, return its typed error and explicit
`a3s install use/ocr` suggestion to the parent; never install or repair models
from inside the `use` worker.

In a CLI-only host, equivalent commands are:

```bash
a3s use ocr doctor --json
a3s use ocr extract "$IMAGE" --json
```

`a3s-use-ocr` accepts the same arguments when invoked as a standalone
development binary.

## Boundaries

- Only bounded local image files are accepted. URLs and PDF rasterization are
  outside this domain.
- Do not ask OCR to interpret unrelated content or present OCR output as
  verified source text.
- Do not hide empty results, warnings, model readiness failures, or source
  digest evidence from the parent agent.
