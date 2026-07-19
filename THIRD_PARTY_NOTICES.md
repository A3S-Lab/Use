# Third-Party Notices

## vercel-labs/agent-browser

`a3s-use-browser-driver` incorporates and modifies the native Rust browser
engine and Dashboard from `vercel-labs/agent-browser` version `0.32.1`, commit
`2b202640ee89dc7aadb5e8c9d600e089e9056985`.

The imported work is licensed under the Apache License, Version 2.0. The full
license text is distributed as `LICENSE-APACHE-2.0`, and detailed provenance is
distributed as `UPSTREAM.md`.

Upstream repository: <https://github.com/vercel-labs/agent-browser>

## PaddlePaddle/PaddleOCR PP-OCRv6 Models

A3S Use release archives redistribute the official
`PP-OCRv6_small_det` and `PP-OCRv6_small_rec` ONNX inference model bundles
published by PaddlePaddle/PaddleOCR. The installer pins the upstream archive
URLs, byte sizes, and SHA-256 digests and does not modify the model weights.

PaddleOCR is licensed under the Apache License, Version 2.0.

Upstream repository: <https://github.com/PaddlePaddle/PaddleOCR>

Model collection: <https://huggingface.co/collections/PaddlePaddle/pp-ocrv6>

## Microsoft ONNX Runtime

`a3s-use-ocr` executes the models with Microsoft ONNX Runtime 1.22.0, obtained
through the pinned `ort`/`ort-sys` Rust dependencies. ONNX Runtime is licensed
under the MIT License.

Copyright (c) Microsoft Corporation.

Upstream repository: <https://github.com/microsoft/onnxruntime>

## pykeio/ort

The `ort` and `ort-sys` Rust crates, version `2.0.0-rc.10`, provide the native
ONNX Runtime bindings and build integration. They are available under the MIT
License or the Apache License, Version 2.0.

Upstream repository: <https://github.com/pykeio/ort>

## image-rs/imageproc

`a3s-use-ocr` uses `imageproc` version `0.25.0` for geometric image
transformations. `imageproc` is licensed under the MIT License.

Copyright (c) 2015 PistonDevelopers.

Upstream repository: <https://github.com/image-rs/imageproc>

## clipper2

`a3s-use-ocr` uses the `clipper2` Rust crate version `0.5.3` and
`clipper2c-sys` version `0.1.6` for bounded polygon offsetting during DB
post-processing. The Rust crates are available under the MIT License or the
Apache License, Version 2.0. Their bundled Clipper2 C/C++ implementation is
licensed under the Boost Software License, Version 1.0.

Upstream repositories:

- <https://github.com/tirithen/clipper2>
- <https://github.com/tirithen/clipper2c-sys>
- <https://github.com/AngusJohnson/Clipper2>
