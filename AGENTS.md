# AGENTS.md

## Repository

This repository provides the `a3s-use` facade, Use-owned shared crates, and
routing for independently maintained capability repositories.

## Boundaries

- Browser and OCR routes are built into the default facade from immutable
  revisions of their independent repositories.
- Search depends on `a3s-use-browser`, never on the CLI or a background service.
- OCR integrations depend on the `OcrProvider` contract; PP-OCRv6 is the
  default provider, not the interface boundary.
- External domains declare native CLI, standard MCP, and/or Skill surfaces.
- Do not add an A3S Use JSON-RPC dialect or universal action envelope.
- Human-authored configuration and extension manifests use A3S ACL (`.acl`)
  parsed by `a3s-acl`. ACL is not HCL.
- Machine-owned command output and receipts may use versioned JSON.

## Engineering

- Keep domain APIs typed and `Send + Sync` where applicable.
- Avoid production panics; return contextual errors.
- Use Tokio for I/O.
- Keep Browser implementation types out of public Search-facing contracts.
- Office mutations with ambiguous outcomes return
  `use.office.outcome_unknown` and are never retried automatically.
- Run `cargo fmt --all` and focused tests before completion.
