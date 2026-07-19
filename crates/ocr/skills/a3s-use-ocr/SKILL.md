---
name: a3s-use-ocr
description: Extract text and layout evidence from local image files through the built-in A3S Use OCR domain. Use when an agent needs optical character recognition for a PNG, JPEG, WebP, GIF, BMP, or TIFF image and must preserve the source digest, provider disclosure, confidence, and bounding-box evidence.
---

# A3S Use OCR

Use the host-provided A3S Use surface. In an A3S Code `use` worker, call
`mcp__use_ocr__ocr_doctor` and `mcp__use_ocr__ocr_extract` directly. The host
owns the MCP process; do not run a shell command, install a provider, or read the
file through another tool.

## Workflow

1. Call `mcp__use_ocr__ocr_doctor`.
2. Confirm which provider is ready and whether `sendsSourceOffDevice` is true.
3. Call `mcp__use_ocr__ocr_extract` with the exact local image path from the
   task. Supply language identifiers only when known.
4. Preserve the returned source path, media type, size, and SHA-256 in the
   result. Treat text, confidence, and bounding boxes as OCR evidence, not as a
   verified transcription.

The local Tesseract provider does not send the image over the network. The
vision provider sends the complete source image and prompt to its disclosed
endpoint. Do not use a non-loopback vision provider unless the user has
authorized that data transfer. Never install, repair, or switch providers from
inside the `use` worker.

In a CLI-only host, equivalent commands are:

```bash
a3s use ocr doctor --json
a3s use ocr extract "$IMAGE" --language eng --json
```

`a3s-use-ocr` accepts the same arguments when invoked as a standalone
development binary.

## Boundaries

- Only bounded local image files are accepted. URLs and PDF rasterization are
  outside this domain.
- Keep the default prompt for faithful transcription. A custom vision prompt
  must remain an extraction instruction; do not ask the provider to interpret
  unrelated content.
- Never report vision output as calibrated confidence or layout evidence.
- Do not silently fall back from a requested provider. Report typed provider,
  source, and response errors to the parent agent.
