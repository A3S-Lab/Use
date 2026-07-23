# External Repository Capabilities

A3S Use can host independently released application capabilities without
embedding their source code or inventing a private RPC protocol. An external
repository builds a bounded package containing one ACL manifest and one or more
native CLI, standard MCP, and Skill surfaces. A3S Use validates, installs,
activates, discovers, and delegates those surfaces.

The repository remains the source of development and release artifacts. A3S
Use does not clone a repository, execute its build scripts, or resolve a mutable
branch during installation. This keeps source acquisition and package execution
separate: the installed package is immutable and content-bound, while the
manifest records where its source is maintained.

## Manifest version 2

Every package has one `a3s-use-extension.acl` at its root:

```acl
extension "acme/calendar" {
  schema_version = 2
  version        = "1.4.0"
  route          = "calendar"
  requires_use   = ">=0.2.0, <0.3.0"
  actions        = ["read", "mutate"]

  repository {
    url      = "https://github.com/acme/calendar"
    revision = "0123456789abcdef0123456789abcdef01234567"
  }

  cli {
    executable  = "bin/acme-calendar"
    json_output = true
  }

  mcp {
    executable = "bin/acme-calendar"
    args       = ["mcp"]
    transport  = "stdio"
  }

  skill {
    path = "skills/calendar/SKILL.md"
  }
}
```

Version 2 requires:

- a SemVer package `version`;
- a unique lowercase `<publisher>/<name>` package identity;
- one lowercase route that does not collide with a built-in command;
- a valid `requires_use` SemVer range;
- a credential-free HTTPS repository URL without a query or fragment; and
- at least one CLI, standard MCP, or `SKILL.md` surface.

`repository.revision` is optional for release channels that bind source
provenance elsewhere. When present, it must be a lowercase 40- or 64-character
commit digest. It is provenance metadata, not an instruction to fetch or run
source code.

All declared paths are relative to the package root. Installation rejects
missing surfaces, path traversal, symbolic links, non-executable native
surfaces, invalid archives, oversized packages, identity drift, route
conflicts, and host-version incompatibility before activation.

## Package layout

```text
calendar-package/
├── a3s-use-extension.acl
├── bin/
│   └── acme-calendar
└── skills/
    └── calendar/
        └── SKILL.md
```

The CLI and MCP declarations may reference the same executable with different
arguments. MCP remains standard stdio or Streamable HTTP MCP. A3S Use does not
wrap either surface in JSON-RPC or translate its tool vocabulary.

## Installation and trust

A package can enter the registry through one of three explicit trust paths:

1. a local directory or bounded archive with `--allow-unsigned`;
2. a digest-reviewed package bundled with an A3S Use release; or
3. a TUF-verified remote extension registry with a separately trusted root.

For local development:

```bash
a3s-use component install acme/calendar \
  --from ./calendar-package \
  --allow-unsigned \
  --json
```

Local trust is never implied. Without `--allow-unsigned`, a local package is
rejected.

## Identity, routes, and lifecycle

The package ID is the stable lifecycle identity. The route is the user-facing
command and discovery alias.

```bash
# Stable package lifecycle operations
a3s-use extension inspect acme/calendar --json
a3s-use extension disable acme/calendar --json
a3s-use extension enable acme/calendar --json

# Route-based use and inspection
a3s-use calendar events list --json
a3s-use doctor calendar --json
a3s-use component status calendar --json
a3s-use mcp serve calendar
a3s-use component uninstall calendar --json
```

`use/calendar` is accepted wherever a route alias is accepted.
`use/acme/calendar` is the canonical component-qualified package identity.
Initial installation still requires the package ID because an uninstalled route
has no trustworthy owner.

Disable and uninstall first make the route invisible, then wait for accepted
CLI or MCP calls to release their package lease. A timed-out drain remains
disabled. Upgrade switches the active receipt atomically while existing calls
continue against the generation they accepted.

## Host compatibility and discovery

Installation rejects a package when `requires_use` does not match the running
A3S Use version. Existing receipts that become incompatible after a host
upgrade remain visible for diagnosis but are not callable. They project as
`broken` or `incompatible`, without MCP, Skill, or workbench activation.

Resident hosts consume:

```bash
a3s-use capability snapshot --json
a3s-use capability watch \
  --after-generation 12 \
  --after-revision <sha256> \
  --json
```

Each external capability projection includes its package identity, route,
version, immutable package root, required A3S Use range, repository identity,
surface list, MCP target, and content-bound Skill or workbench assets. The
registry generation covers lifecycle commits; the projection revision also
changes when projected content changes.

## Office reference package

[A3S Office](https://github.com/A3S-Lab/Office) uses this boundary. Its package
identity is `a3s/office`, its route is `office`, and its native `a3s-office`
binary provides both CLI and standard MCP surfaces.

From an Office source checkout:

```bash
./scripts/package-a3s-use-extension.sh ./dist/a3s-use-office

a3s-use component install a3s/office \
  --from ./dist/a3s-use-office \
  --allow-unsigned \
  --json

a3s-use office --help
a3s-use mcp serve office
```

Office can evolve, test, and release independently. A3S Use owns only the
standard package contract and lifecycle around it.
