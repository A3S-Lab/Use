# Upstream Inspiration and Clean-Room Boundary

The public capability inventory in
[`baifan-wang/skills/claude-science`](https://github.com/baifan-wang/skills/tree/main/claude-science)
inspired the source selection and agent workflow for this extension. The
inventory was reviewed at commit
`2b61d890c5ba50570717599b16d34514458b3955` on 2026-07-17.

That repository describes a much larger collection of data tools, model
workflows, compute integrations, and scientific Skills. Its components carry
component-specific licensing rather than one clearly stated project-level
license. Consequently, `a3s-use-science` is an independent clean-room Rust
implementation:

- no Python source, JSON schema, prompt, test fixture, model wrapper, or other
  implementation artifact is copied or distributed;
- this initial package implements a smaller source-specific retrieval surface
  and does not claim command, MCP-tool, or output compatibility;
- Ensembl access uses the documented Ensembl REST API, not the upstream
  collection's BioMart implementation;
- the extension's code and package assets are licensed with A3S Use under MIT.

Public database names and documented HTTP contracts are factual integration
points, not bundled upstream software. Each remote data service retains its own
terms, licenses, and attribution requirements; see
[DATA_SOURCES.md](DATA_SOURCES.md).
