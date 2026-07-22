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

After packaging and installing the extension, the A3S host route is:

```bash
a3s use mcp serve a3s/science
```

The server exposes 13 source-specific `science_*` tools. It does not introduce
an A3S-specific RPC envelope or combine unrelated source vocabularies into a
generic execute action.

## Package

Create a local extension directory at a new path:

```bash
./crates/science/scripts/package.sh /tmp/a3s-use-science-package
a3s install use/a3s/science \
  --from /tmp/a3s-use-science-package \
  --allow-unsigned
a3s use science doctor --json
```

The script refuses to overwrite an existing output directory. The package may
also be archived as `.tar.gz`, `.tgz`, or `.zip` and passed directly to
`--from`. Local directories and archives require explicit `--allow-unsigned`
trust. The package also contains a `contributes.activity_bar` workbench with an
HTML entry and explicitly declared CSS/JavaScript assets. A3S Use hashes every
asset into its capability snapshot, and A3S Web verifies and injects them inside
the isolated plugin document. The contribution remains bound to the packaged
`a3s-use-science` Skill. Official A3S Use archives ship the same complete
package as an optional release bundle, allowing A3S Web Market to install it
through a digest-reviewed umbrella plan even when no remote registry is
configured. The receipt records `release-bundle`; the plugin remains removable
and is not a built-in route. Signed remote distribution continues to use a
configured TUF registry and records `registry-tuf` provenance.
