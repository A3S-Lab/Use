# Local Registry transport (client view)

Status: development preview

A3S Use consumes Registries as **host-selected sources** with a pinned bootstrap
root digest. A local HTTP server of the official Use-Registry `registry/` tree
is a **transport only**.

## Rules

1. Pin `--trust-root` to the independently obtained bootstrap digest.
2. Use loopback `http://127.0.0.1:<port>/` only for local testing; non-loopback
   Registry URLs require HTTPS.
3. Use a package-manager `a3s-use` build that exposes `registry` / `plugin`
   routes (`0.3.x+`). Capability wrappers without those routes are not clients.
4. Treat `state/remote-registries/.../verified-targets` as cache, never as
   install or recovery authority.

## Operator pointer

Publication and local serve live in the Use-Registry repository:

- architecture: `docs/registry-service-architecture.md`
- serve/smoke/consume/gate: `scripts/serve_local.sh`, `smoke_local.sh`,
  `consume_local.sh`, `test_local_registry.sh`

Example:

```bash
export A3S_USE_BIN=/path/to/use/target/debug/a3s-use
cd /path/to/Use-Registry
./scripts/serve_local.sh start
./scripts/consume_local.sh
```
