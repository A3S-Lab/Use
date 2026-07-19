# A3S Use OCR

`a3s-use-ocr` implements the first-party built-in OCR domain for A3S Use. A3S
Code receives it as `mcp__use_ocr__*` through the release-matched Use registry,
without a separate extension install. It exposes the same typed extraction
through a native CLI and standard stdio MCP, and does not silently install an
OCR provider.

Provider selection is explicit:

- `A3S_OCR_PROVIDER=auto|tesseract|vision`
- `A3S_OCR_TESSERACT_EXECUTABLE=/absolute/path/to/tesseract`
- `A3S_OCR_VISION_MODEL=<model>`
- `A3S_OCR_VISION_BASE_URL=https://provider.example/v1/`
- `A3S_OCR_VISION_API_KEY=<secret>`
- `A3S_OCR_TIMEOUT_MS=60000`

`auto` prefers a configured or discoverable local Tesseract executable. It uses
the vision provider only when the vision environment is configured. Remote
vision endpoints require HTTPS and an API key; loopback HTTP is allowed for a
local provider.

Build and exercise the domain through the Use facade:

```bash
a3s use ocr doctor --json
a3s use ocr extract ./scan.png --language eng --json
a3s use mcp serve ocr
```

The A3S Use release packages the OCR Skill beside the facade binary. A3S Code
can first-use install that verified release and hot-plug the built-in route.
Provider setup remains explicit: local Tesseract never sends source bytes
off-device, while a configured remote vision provider requires parent HITL.
