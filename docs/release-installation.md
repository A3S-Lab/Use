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
and PowerShell 7. Unsupported systems fail before a release request.

## Trust and failure model

The installer:

1. resolves an explicit semantic version or the latest GitHub Release;
2. accepts HTTPS downloads only, including every redirect;
3. permits plain HTTP only for an explicit loopback test server;
4. downloads `checksums.txt` and exactly one platform archive;
5. requires one well-formed SHA-256 entry for that archive;
6. verifies the complete archive before extraction;
7. rejects absolute paths, parent traversal, links, and special files;
8. requires the Browser driver, Browser Skills, OCR Skills, OCR model files,
   and dashboard shipped by the release contract;
9. stages under the destination filesystem and atomically promotes one
   versioned directory;
10. refuses an existing version unless its marker and complete file tree match
   the newly verified archive; and
11. atomically replaces only an A3S-managed command entry.

Checksum failure, unsafe archive metadata, incomplete content, tampered
installed files, link or reparse-point state, concurrent installation, and an
unmanaged command conflict all fail closed. Temporary downloads and staging
directories are removed. The installer never activates partially extracted
content.

The published checksum and archive currently share the GitHub Release HTTPS
trust boundary. This detects corruption, truncation, platform mismatch, and a
different archive under an expected checksum. It is not an independent
signature. Reproducible archives, signed checksum evidence, SBOMs, and
provenance attestations remain required before a supported product release.

## Unix usage

```bash
sh install.sh [--version <version>] \
  [--install-root <absolute-path>] \
  [--bin-dir <absolute-path>] \
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
`A3S_USE_RELEASE_BASE_URL`, `A3S_USE_INSTALL_ROOT`, and `A3S_USE_BIN_DIR`.

## Windows usage

```powershell
./install.ps1 [-Version <version>] `
  [-InstallRoot <absolute-path>] `
  [-BinDir <absolute-path>] `
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
version redownloads and verifies the archive, compares every installed file,
and republishes the same command only when the trees agree exactly.

Automatic retention, a signed rollback command, and whole-product state
backup/restore are separate operational release gates. Removing old version
directories does not remove package receipts, Registry evidence, Grants, Flow
history, UI data, or OKF projections.
