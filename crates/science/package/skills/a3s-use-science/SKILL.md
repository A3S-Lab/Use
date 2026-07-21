---
name: a3s-use-science
description: Retrieve and cross-check public biomedical evidence from PubMed, ChEMBL, ClinicalTrials.gov, bioRxiv, and Ensembl. Use for literature searches, preprint checks, compound and target research, trial discovery, gene lookup, and ortholog analysis through A3S Use.
allowed-tools: Bash(a3s:*)
---

# A3S Use Science

The broader first-party scientific Skill catalog and workflow assets are
maintained in [A3S Science](https://github.com/A3S-Lab/Science).

Use the host surface that is already available:

- In an A3S Code `use` worker, call the available
  `mcp__use_science__*` tools directly. The host owns installation and MCP
  lifecycle; do not run installation or shell commands there.
- In a CLI-only agent host, use `a3s use science ...` commands.

Select the narrowest authoritative source:

- Use PubMed for peer-reviewed biomedical literature and article metadata.
- Use bioRxiv for preprints; always label results as preprints.
- Use ChEMBL for molecules, targets, and bioactivity records.
- Use ClinicalTrials.gov for registered study protocols and recruitment status.
- Use Ensembl for gene coordinates, identifiers, and orthologs.

Start with `science_doctor`. PubMed calls require
`A3S_SCIENCE_CONTACT_EMAIL`; `NCBI_API_KEY` is optional. Other sources do not
require those variables.

Preserve PMID, DOI, ChEMBL, NCT, and Ensembl identifiers in the answer. State
which source supports each claim, distinguish database metadata from research
conclusions, and report empty or partial results plainly. Never invent missing
records, silently treat a preprint as peer reviewed, or present retrieved data
as diagnosis or medical advice. Cross-check important claims in more than one
source when the task warrants it.

CLI examples:

```bash
a3s use science doctor --json
a3s use science pubmed search "CRISPR off-target effects" --limit 10 --json
a3s use science pubmed get 39712345 --json
a3s use science chembl search-molecules aspirin --limit 10 --json
a3s use science chembl activities --molecule CHEMBL25 --limit 20 --json
a3s use science clinical-trials search melanoma --status RECRUITING --json
a3s use science biorxiv search --from 2026-01-01 --to 2026-01-31 --query protein --json
a3s use science ensembl lookup homo_sapiens TP53 --json
a3s use science ensembl homologs homo_sapiens TP53 --target-species mus_musculus --json
```
