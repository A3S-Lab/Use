# A3S Use OCR

`a3s-use-ocr` implements the first-party built-in OCR domain for A3S Use. A3S
Code receives it as `mcp__use_ocr__*` through the release-matched Use registry,
without installing a separate extension. The native CLI and standard stdio MCP
share one local PP-OCRv6 implementation.

There is one OCR provider:

- provider: `pp-ocr-v6`
- engine: `onnx-runtime`
- model bundle: `PP-OCRv6_small`

The release packages the pinned detection and recognition models. If the model
bundle is absent or damaged, install or repair it explicitly:

```bash
a3s install use/ocr
a3s install use/ocr --force
```

`A3S_OCR_MODEL_DIR` can point development builds at an explicit model bundle.
`A3S_USE_OCR_HOME` overrides the managed model root for packaging, tests, or an
isolated installation. Neither setting selects another OCR backend.

## Workflow

For each bounded local image, the native engine:

1. decodes the image and applies PP-OCRv6 BGR normalization;
2. runs `PP-OCRv6_small_det` through ONNX Runtime;
3. applies DB post-processing, polygon unclipping, and reading-order sorting;
4. perspective-rectifies each text polygon and rotates tall crops;
5. runs batched `PP-OCRv6_small_rec` inference; and
6. applies CTC decoding and returns text, recognition/detection confidence,
   polygons, bounding boxes, and the source SHA-256.

All inference stays in the local `a3s-use` process. It does not require Python
or PaddlePaddle, does not call an OCR API, and does not transfer image bytes off
the device.

## Commands

```bash
a3s use ocr doctor --json
a3s use ocr extract ./scan.png --json
a3s use mcp serve ocr
```

Supported inputs are bounded local PNG, JPEG, WebP, GIF, BMP, and TIFF files.
URLs and PDF rasterization are outside this crate.
