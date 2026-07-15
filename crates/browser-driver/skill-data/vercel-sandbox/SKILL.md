---
name: vercel-sandbox
description: Run A3S Use Browser and Chrome inside Vercel Sandbox microVMs. Use for browser automation from Vercel applications, isolated Chrome execution, persistent multi-command sessions, or reusable sandbox snapshots.
allowed-tools: Bash(a3s:*)
---

# A3S Use Browser in Vercel Sandbox

Vercel Sandbox can run the Linux A3S Use release in an isolated microVM. The
same `open` / `snapshot` / interaction workflow applies; only command transport
changes from the local process to `sandbox.runCommand`.

Every example below ultimately executes the normal `a3s use browser ...`
command surface inside the sandbox.

## Dependencies

```bash
pnpm add @vercel/sandbox
```

Pin the A3S Use release. Do not install `latest` implicitly:

```text
A3S_USE_VERSION=0.1.0
A3S_USE_BROWSER_SNAPSHOT_ID=snap_xxx   # optional after creating a snapshot
```

## Verified bootstrap

The release archive contains `a3s-use`, `a3s-use-browser-driver`, the packaged
Skills, Dashboard, and provenance notices. Bootstrap downloads the archive and
published checksum from GitHub, verifies it, and installs Chrome through the
A3S component lifecycle.

```ts
import { Sandbox } from "@vercel/sandbox";

type Session = Awaited<ReturnType<typeof Sandbox.create>>;

const VERSION = process.env.A3S_USE_VERSION;
if (!VERSION || !/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(VERSION)) {
  throw new Error("A3S_USE_VERSION must be a pinned semantic version");
}

async function bootstrap(sandbox: Session): Promise<string> {
  const target = "linux-x86_64";
  const archive = `a3s-use-${VERSION}-${target}.tar.gz`;
  const base = `https://github.com/A3S-Lab/Use/releases/download/v${VERSION}`;
  const script = `
set -eu
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
curl --fail --silent --show-error --location --proto '=https' \\
  -o "$work/${archive}" "${base}/${archive}"
curl --fail --silent --show-error --location --proto '=https' \\
  -o "$work/checksums.txt" "${base}/checksums.txt"
expected="$(awk '$2 == \"${archive}\" { print $1 }' "$work/checksums.txt")"
test "${"$"}{#expected}" -eq 64
actual="$(sha256sum "$work/${archive}" | awk '{ print $1 }')"
test "$actual" = "$expected"
install -d "$HOME/.local/bin"
tar -xzf "$work/${archive}" -C "$HOME/.local/bin"
cat > "$HOME/.local/bin/a3s" <<'SH'
#!/bin/sh
set -eu
test "${"$"}{1:-}" = use || { echo 'sandbox shim supports only: a3s use ...' >&2; exit 2; }
shift
exec "$(dirname "$0")/a3s-use" "$@"
SH
chmod 0755 "$HOME/.local/bin/a3s" "$HOME/.local/bin/a3s-use" \\
  "$HOME/.local/bin/a3s-use-browser-driver"
"$HOME/.local/bin/a3s" use browser install --with-deps
printf '%s\\n' "$HOME/.local/bin/a3s"
`;
  const result = await sandbox.runCommand("sh", ["-lc", script]);
  if (result.exitCode !== 0) throw new Error(await result.stderr());
  return (await result.stdout()).trim().split("\n").at(-1)!;
}
```

## Run commands

Pass browser arguments as an array so URLs and user-provided values never pass
through shell interpolation.

```ts
async function runBrowser(
  sandbox: Session,
  a3s: string,
  args: readonly string[],
) {
  const result = await sandbox.runCommand(a3s, ["use", "browser", ...args]);
  const stdout = await result.stdout();
  const stderr = await result.stderr();
  if (result.exitCode !== 0) {
    throw new Error(stderr.trim() || stdout.trim() || `exit ${result.exitCode}`);
  }
  return { stdout, json: tryJson(stdout) };
}

function tryJson(value: string): unknown | null {
  try { return JSON.parse(value); } catch { return null; }
}

export async function snapshotUrl(url: string) {
  const snapshotId = process.env.A3S_USE_BROWSER_SNAPSHOT_ID;
  const sandbox = await Sandbox.create(
    snapshotId
      ? { source: { type: "snapshot", snapshotId }, timeout: 120_000 }
      : { runtime: "node24", timeout: 300_000 },
  );
  try {
    const a3s = snapshotId
      ? (await sandbox.runCommand("sh", ["-lc", "command -v a3s"]))
      : null;
    const executable = a3s
      ? (await a3s.stdout()).trim()
      : await bootstrap(sandbox);
    await runBrowser(sandbox, executable, ["open", url, "--json"]);
    const snapshot = await runBrowser(sandbox, executable, ["snapshot", "-i", "-c"]);
    await runBrowser(sandbox, executable, ["close"]);
    return snapshot.stdout;
  } finally {
    await sandbox.stop();
  }
}
```

## Create a reusable snapshot

```ts
const sandbox = await Sandbox.create({ runtime: "node24", timeout: 300_000 });
try {
  await bootstrap(sandbox);
  const { snapshotId } = await sandbox.snapshot();
  console.log(snapshotId);
} finally {
  await sandbox.stop();
}
```

Store the resulting ID as `A3S_USE_BROWSER_SNAPSHOT_ID`. A sandbox snapshot is
a saved VM image; it is unrelated to the Browser accessibility `snapshot`
command. Rebuild it whenever the pinned A3S Use version changes.

Vercel deployments authenticate through OIDC. Local callers may supply
`VERCEL_TOKEN`, `VERCEL_TEAM_ID`, and `VERCEL_PROJECT_ID` through the normal
Vercel Sandbox SDK configuration.
