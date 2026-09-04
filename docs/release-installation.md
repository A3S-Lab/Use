# Verified release installation

A3S Use publishes one installer contract for Linux, macOS, and Windows release
previews. It is a distribution mechanism, not a claim that the product release
gate is complete.

## Supported targets

| Host | Release archive |
| --- | --- |
| Linux x86_64 | `a3s-use-<version>-linux-x86_64.tar.gz` |
| Linux arm64 | `a3s-use-<version>-linux-arm64.tar.gz` |
| macOS x86_64 | `a3s-use-<version>-darwin-x86_64.tar.gz` |
| macOS arm64 | `a3s-use-<version>-darwin-arm64.tar.gz` |
| Windows x86_64 | `a3s-use-<version>-windows-x86_64.zip` |

`install.sh` supports Linux and macOS. `install.ps1` requires Windows x86_64
and Windows PowerShell 5.1 or PowerShell 7. Both installers require a trusted Cosign executable on
`PATH`, or an explicit path supplied by the operator. Unsupported systems fail
before a release request.

## Trust and failure model

The installer:

1. resolves an explicit semantic version or the latest GitHub Release;
2. accepts HTTPS downloads only, including every redirect;
3. permits plain HTTP only for an explicit loopback test server;
4. downloads `checksums.txt` and its Sigstore bundle;
5. requires Cosign to authenticate the manifest against the exact tag workflow
   identity and `https://token.actions.githubusercontent.com` issuer;
6. downloads exactly one platform archive only after signature verification;
7. requires one well-formed SHA-256 entry and verifies the complete archive;
8. rejects absolute paths, parent traversal, links, and special files;
9. requires the Browser driver, Browser Skills, OCR Skills, OCR model files,
   and dashboard shipped by the release contract;
10. retains the verified checksum manifest and Sigstore bundle in the staged
    version directory;
11. stages under the destination filesystem and atomically promotes one
   versioned directory;
12. refuses an existing version unless its marker and complete file tree match
   the newly verified archive; and
13. atomically replaces only an A3S-managed command entry.

Missing Cosign, signature or identity failure, checksum failure, unsafe archive
metadata, incomplete content, tampered installed files, link or reparse-point
state, concurrent installation, and an unmanaged command conflict all fail
closed. Temporary downloads and staging directories are removed. The
installer never activates partially extracted content.

The manifest, bundle, and archive may share the GitHub Release HTTPS transport,
but the manifest is authenticated independently through Sigstore's Fulcio and
Rekor evidence. The accepted certificate identity is exactly
`https://github.com/A3S-Lab/Use/.github/workflows/release.yml@refs/tags/v<version>`;
a branch workflow, another repository, another issuer, a missing bundle, or an
invalid transparency-log proof is rejected. The installer script itself is a
separate bootstrap trust boundary and should be downloaded for review or
distributed through a trusted system package.

Each successful tagged Release also publishes:

- deterministic tar.gz or ZIP serialization using the tag commit timestamp,
  sorted paths, normalized ownership and modes, and fixed compression tools;
- one `a3s-use-<version>-<platform>.spdx.json` SBOM per archive;
- one `a3s-use-<version>-<platform>.reproducibility.json` record proving that
  every shipped native executable byte-matched a second build on a clean
  cache-free runner;
- GitHub OIDC build-provenance and SBOM attestations for every archive;
- a GitHub OIDC attestation for every independent-rebuild record;
- `checksums.txt.sigstore.json`, a keyless Sigstore bundle that authenticates
  `checksums.txt` through the public transparency log; and
- checksums covering every archive, SBOM, rebuild record, and installer.

Every Action and release tool version is immutable in the workflow. The
archive packager is byte-reproducible for an identical staged tree and is
tested against source-path, creation-order, and timestamp drift. A separate
runner checks out the frozen source commit and Browser revision, uses the same
pinned toolchain without a compiled-artifact cache, rebuilds every native
executable, and compares it directly with the primary archive. An externally
operated witness for the complete staged tree/final archive, evidence retention
outside GitHub Release, and the other product gates remain incomplete.

## Non-publishing qualification

Before creating a version tag, maintainers can run the complete five-target
archive, isolated-install, SBOM, attestation, and independent-rebuild matrix
against the current `main` commit:

```bash
gh workflow run release.yml \
  --repo A3S-Lab/Use \
  --ref main \
  -f qualification=true
```

Qualification rejects a supplied `release_tag`, freezes the exact `main`
revision, and skips both crates.io publication and GitHub Release creation.
Its run-scoped archives, SBOMs, attestations, and reproducibility records are
evidence for release review; they are not installable release assets and do
not change the development-preview status.

Before evidence publication, each extracted archive is scanned for the build
checkout path in native and slash-normalized UTF-8 and UTF-16 encodings. The
workflow then runs both native executables from an isolated working directory
and an isolated `A3S_USE_HOME`. The current non-publishing qualification run
[33651777660](https://github.com/A3S-Lab/Use/actions/runs/33651777660)
passed this gate and the cache-free, byte-for-byte rebuild matrix on all five
targets from exact `main` commit
`4f6e4725205d06ab81f8ea98bfee85c7eb4b2bcd`.

The tagged `v0.3.2` workflow failed the independent rebuild comparison on four
targets and therefore did not create a GitHub Release. The later `v0.3.5`
publication attempt also created no Release because the public
`a3s-use-core` crate was still `0.2.4`. The successful release workflow
[33675697857](https://github.com/A3S-Lab/Use/actions/runs/33675697857) built tag
`v0.3.6` from exact `main` commit
`54758910f2f4ad9498137410e0a2207d412e99a1`, passed the independent evidence on
all five targets, and published the development-preview
[GitHub Release](https://github.com/A3S-Lab/Use/releases/tag/v0.3.6), including
`a3s-use-core 0.2.5`, `a3s-use-extension 0.3.6`, and `a3s-use 0.3.6`.
Release workflow
[33687297386](https://github.com/A3S-Lab/Use/actions/runs/33687297386) then built
tag `v0.3.7` from exact `main` commit
`48a0b76f8a4a87a11d16627c7bd7567920852508`, passed the independent evidence on
all five targets, and published the current development-preview
[GitHub Release](https://github.com/A3S-Lab/Use/releases/tag/v0.3.7), including
`a3s-use-core 0.2.6`, `a3s-use-extension 0.3.7`, and `a3s-use 0.3.7`.
The non-publishing qualification run
[33651777660](https://github.com/A3S-Lab/Use/actions/runs/33651777660) remains
historical evidence from the prior source commit; it never publishes crates or
release assets.

Release workflow
[33720485826](https://github.com/A3S-Lab/Use/actions/runs/33720485826) then built
tag `v0.3.8` from exact `main` commit
`6d3a7baf32ce998a2e487c40fbf78b4a6cda2579`, passed the independent evidence on
all five targets, and published the current development-preview
[GitHub Release](https://github.com/A3S-Lab/Use/releases/tag/v0.3.8), including
`a3s-use-core 0.2.7`, `a3s-use-extension 0.3.8`, and `a3s-use 0.3.8`.

Release workflow
[33756618837](https://github.com/A3S-Lab/Use/actions/runs/33756618837) then built
tag `v0.3.9` from exact `main` commit
`a5f3cc40bfb0a1021ca150d2ce4295409b74d220`, passed the independent evidence on
all five targets, and published the current development-preview
[GitHub Release](https://github.com/A3S-Lab/Use/releases/tag/v0.3.9), including
19 release assets and `a3s-use-core 0.2.7`, `a3s-use-extension 0.3.9`, and
`a3s-use 0.3.9`.

Release workflow
[33791616307](https://github.com/A3S-Lab/Use/actions/runs/33791616307) then built
tag `v0.3.10` from exact `main` commit
`c4c80a223bfff3698ca4b4598e7175c6e3303239`, passed the independent evidence on
all five targets, and published the current development-preview
[GitHub Release](https://github.com/A3S-Lab/Use/releases/tag/v0.3.10), including
19 release assets and `a3s-use-core 0.2.8`, `a3s-use-extension 0.3.10`, and
`a3s-use 0.3.10`.

Release workflow
[33830280138](https://github.com/A3S-Lab/Use/actions/runs/33830280138) then built
tag `v0.3.11` from exact `main` commit
`c25028ae0245ba1d28f7e2837e2a87f7e9f6fe40`, passed the independent evidence on
all five targets, and published the current development-preview
[GitHub Release](https://github.com/A3S-Lab/Use/releases/tag/v0.3.11), including
19 release assets and `a3s-use-core 0.2.9`, `a3s-use-extension 0.3.11`, and
`a3s-use 0.3.11`.

## Additional independent verification

The installer already performs the Cosign verification below. GitHub CLI can
add an independent attestation check, or the same commands can be used before
running a reviewed installer:

```bash
version=0.3.11
tag="v${version}"
archive="a3s-use-${version}-darwin-arm64.tar.gz"
rebuild="a3s-use-${version}-darwin-arm64.reproducibility.json"

gh release download "${tag}" --repo A3S-Lab/Use \
  --pattern "${archive}" \
  --pattern "${archive%.tar.gz}.spdx.json" \
  --pattern "${rebuild}" \
  --pattern checksums.txt \
  --pattern checksums.txt.sigstore.json

cosign verify-blob \
  --bundle checksums.txt.sigstore.json \
  --certificate-identity "https://github.com/A3S-Lab/Use/.github/workflows/release.yml@refs/tags/${tag}" \
  --certificate-oidc-issuer "https://token.actions.githubusercontent.com" \
  checksums.txt

grep "  ${archive}$" checksums.txt | sha256sum --check --strict
grep "  ${rebuild}$" checksums.txt | sha256sum --check --strict
gh attestation verify "${archive}" --repo A3S-Lab/Use
gh attestation verify "${rebuild}" --repo A3S-Lab/Use
```

Replace the archive name with the exact target from [Supported targets](#supported-targets).
On macOS, use `shasum -a 256` to compare the selected line when GNU
`sha256sum` is unavailable. Verification must use the exact tag identity shown
above; a different workflow ref or OIDC issuer is not equivalent evidence.

## Unix usage

```bash
sh install.sh [--version <version>] \
  [--install-root <absolute-path>] \
  [--bin-dir <absolute-path>] \
  [--cosign <trusted-executable>] \
  [--base-url <https-url>]
```

Defaults:

- release store: `$XDG_DATA_HOME/a3s-use/releases/<version>`, falling back to
  `$HOME/.local/share/a3s-use/releases/<version>`;
- command: `$HOME/.local/bin/a3s-use`;
- source: `https://github.com/A3S-Lab/Use/releases/download`.

The command is an absolute symlink to an installer-owned launcher inside the
verified version directory. The launcher supplies the packaged OCR model,
OCR Skill, and Browser Skill roots only when the operator has not provided an
explicit environment override. The installer does not edit shell profiles. If
`$HOME/.local/bin` is not already on `PATH`, it prints the required directory.

Environment equivalents are `A3S_USE_VERSION`,
`A3S_USE_RELEASE_BASE_URL`, `A3S_USE_INSTALL_ROOT`, `A3S_USE_BIN_DIR`, and
`A3S_USE_COSIGN`.

## Windows usage

```powershell
./install.ps1 [-Version <version>] `
  [-InstallRoot <absolute-path>] `
  [-BinDir <absolute-path>] `
  [-CosignPath <trusted-executable>] `
  [-BaseUrl <https-url>] `
  [-NoPathUpdate]
```

Defaults:

- release store: `%LOCALAPPDATA%\A3S\Use\releases\<version>`;
- managed command shim: `%LOCALAPPDATA%\A3S\bin\a3s-use.cmd`;
- source: `https://github.com/A3S-Lab/Use/releases/download`.

The installer prepends its bin directory to the user `PATH` unless
`-NoPathUpdate` is supplied. It never replaces a command file without the A3S
managed-shim marker. The shim supplies packaged OCR and Browser resource roots
only when the matching environment variable is unset. Windows file locking
serializes concurrent installers; reparse points in owned release paths are
rejected.

## Upgrade and repeat installation

Running the installer with a newer version creates a new immutable release
directory, then changes only the managed command entry. Prior versions remain
available for operator-controlled rollback. Running it again with the same
version redownloads and authenticates the signed manifest, verifies the
archive, compares every installed file including retained evidence, and
republishes the same command only when the trees agree exactly.

Automatic retention, a signed rollback command, and whole-product state
backup/restore are separate operational release gates. Removing old version
directories does not remove package receipts, Registry evidence, Grants, Flow
history, UI data, or OKF projections.
