# Science Data Sources

The Science extension sends user-supplied search terms and identifiers over
HTTPS to public third-party services. A query can reveal research interests;
do not submit confidential, patient-identifying, controlled, or unpublished
information unless the applicable policy and upstream terms permit it.

| Source | Endpoint | Data returned | Local credential |
| --- | --- | --- | --- |
| PubMed / NCBI E-utilities | `eutils.ncbi.nlm.nih.gov` | Citation summaries and identifiers | Contact email required; API key optional |
| ChEMBL | `www.ebi.ac.uk/chembl` | Molecules, targets, and activities | None |
| ClinicalTrials.gov | `clinicaltrials.gov/api/v2` | Public study protocol records | None |
| bioRxiv | `api.biorxiv.org` | Public preprint metadata | None |
| Ensembl REST | `rest.ensembl.org` | Public gene and homology records | None |

The extension applies per-source request pacing, bounded result limits, a
30-second default request timeout, and bounded upstream error bodies. bioRxiv
free-text filtering scans at most 500 records per command. PubMed requests
identify `a3s-use-science` and include the configured contact email in line
with NCBI guidance.

Upstream services remain authoritative for licenses, terms, retention,
availability, update cadence, and record interpretation. Their schemas and
content can change independently of A3S. A successful response means only that
the public API returned a record; it does not establish scientific validity,
peer review, clinical suitability, or regulatory approval.

Always preserve source identifiers and retrieval context. Label bioRxiv
records as preprints, verify consequential conclusions against the underlying
publication or protocol, and do not use this toolkit as a substitute for
medical, safety, ethics, or regulatory review.
