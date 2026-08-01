---
type: Attested Computation
title: Revenue for fiscal year
runtime: python
parameters:
  - { name: year, type: integer, required: true }
executor:
  resource: ../references/run-revenue.md
  receipt: [run_id, result]
attester:
  resource: ../references/attesters/revenue.py
---

# Computation

```python
sum_revenue(year)
```

The executor and attester fields are inert metadata at the package boundary.
