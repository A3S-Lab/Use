# A3S Science Repository Boundary

The first-party [A3S Science](https://github.com/A3S-Lab/Science) repository is
the canonical home for the broader scientific Skill catalog, MCP data services,
compute workflows, and supporting assets used across A3S.

`a3s-use-science` is the process-isolated native integration for A3S Use. It
deliberately exposes a smaller source-specific Rust contract:

- no Python environment, model wrapper, or catalog runtime is bundled or
  launched by this package;
- this initial package implements a smaller source-specific retrieval surface
  and does not claim command, MCP-tool, or output compatibility with every
  catalog component;
- Ensembl access uses the documented Ensembl REST API rather than a BioMart
  adapter;
- the extension's code and package assets are licensed with A3S Use under MIT.

Public database names and documented HTTP contracts are factual integration
points. Components in A3S Science retain their component-specific licenses, and
each remote data service retains its own terms, licenses, and attribution
requirements; see [DATA_SOURCES.md](DATA_SOURCES.md).
