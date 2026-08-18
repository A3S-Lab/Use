# A3S Use Science

`a3s-use-science` is a process-isolated, read-only life-science extension for
A3S Use. It provides one typed asynchronous Rust client and projects the same
operations through a native CLI and a standard MCP server.

The initial toolkit covers:

| Source | Operations |
| --- | --- |
| PubMed | Search article summaries; retrieve a PMID |
| ChEMBL | Search molecules and targets; retrieve molecules and activities |
| ClinicalTrials.gov | Search studies; retrieve an NCT record |
| bioRxiv | Search a bounded date range; retrieve a DOI |
| Ensembl | Look up a gene; retrieve orthologs |

All operations are retrieval-only. The broader scientific Skill catalog, MCP
services, compute workflows, and supporting assets are maintained in the
first-party [A3S Science](https://github.com/A3S-Lab/Science) repository. This
crate provides a smaller native Rust surface and does not bundle or run the
catalog's Python environments. See [UPSTREAM.md](UPSTREAM.md) for the repository
boundary.

## Configuration

Set a contact email before using PubMed, as requested by NCBI E-utilities:

```bash
export A3S_SCIENCE_CONTACT_EMAIL=researcher@example.org
export NCBI_API_KEY=optional-ncbi-key
```

`NCBI_API_KEY` is optional. The other sources currently use public endpoints
without credentials. See [DATA_SOURCES.md](DATA_SOURCES.md) for network,
provenance, and usage considerations.

## CLI

Build and run from the A3S Use workspace:

```bash
cargo build -p a3s-use-science
./target/debug/a3s-use-science doctor --json
./target/debug/a3s-use-science pubmed search "single-cell atlas" --limit 10 --json
./target/debug/a3s-use-science chembl get-molecule CHEMBL25 --json
./target/debug/a3s-use-science clinical-trials search glioblastoma --status RECRUITING --json
./target/debug/a3s-use-science biorxiv search --from 2026-01-01 --to 2026-01-31 --json
./target/debug/a3s-use-science ensembl lookup homo_sapiens BRCA1 --json
```

Every `--json` invocation returns one versioned CLI document. Without
`--json`, commands print the retrieved typed value as readable JSON.

## Standard MCP

Run the extension's stdio MCP server directly with:

```bash
./target/debug/a3s-use-science serve --mcp
```

The server exposes 13 source-specific `science_*` tools. It does not introduce
an A3S-specific RPC envelope or combine unrelated source vocabularies into a
generic execute action.

## Registry Distribution

Discover, inspect, and install the signed stable package from a trusted A3S
registry:

```bash
a3s plugin search science
a3s plugin inspect a3s/science
a3s plugin install a3s/science --channel stable
```

The registry package declares its CLI, MCP, Skill, and activity-bar surfaces.
A3S Use verifies the signed registry metadata and every package asset before
installation, records `registry-tuf` provenance, and exposes the declared MCP
surface through the host. The activity bar runs in the isolated plugin
document with explicitly declared CSS and JavaScript assets.

Official A3S Use platform archives do not embed Science or other optional
plugins. Registry publication and installation are therefore independent of a
particular Use platform release. For local package development, run
`./crates/science/scripts/package.sh` with a new output directory and publish
the result through the trusted registry workflow.
