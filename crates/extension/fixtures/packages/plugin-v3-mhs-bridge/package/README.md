# MHS Bridge Fixture

This package proves the A3S Use integration boundary for a Model Hardware
Standard (MHS) adapter without defining or emulating the MHS protocol.

The package-owned MCP service is an adapter. It has no direct device authority.
A trusted host must inject a scope-bound MHS gateway session, and the gateway
must enforce device identity, procedure permissions, parameter bounds,
exclusive leases, interlocks, and emergency-stop behavior outside the package.

This fixture is not an MHS implementation and must not be deployed against
physical equipment.

The package deliberately reuses the standard MCP, Flow, Skill, and UI surface
graph. Dynamic device state stays behind MCP rather than entering the A3S Use
capability snapshot. Physical mutations must not be retried after an ambiguous
outcome; the caller must observe and reconcile first.

See `docs/mhs-integration.md` in the A3S Use repository for the complete
ownership, readiness, permission, and safety contract.
