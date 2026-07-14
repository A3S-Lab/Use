# AGENTS.md

## Repository

This repository provides the `a3s-use` binary and typed Rust libraries for
Browser, Office, and externally implemented application domains.

## Boundaries

- Browser and Office are built into the default binary.
- Search depends on `a3s-use-browser`, never on the CLI or a background service.
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
