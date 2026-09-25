//! `a3s-use-registry-tools` assembles, signs, and verifies the static TUF
//! tree an A3S Use Registry publishes.
//!
//! The tool is operator-facing: it never talks to the network, never touches
//! an installation, and writes only the directories it is given. Package
//! identity comes from each package's `a3s-use-extension.acl` manifest;
//! presentation and review metadata come from a separate admission ACL so
//! the two authorities stay distinct.

mod admission;
mod assemble;
mod catalog_build;
mod expiry;
mod keys;
mod mirror;
mod package_build;
mod rotate;
mod verify;
mod withdraw;

use std::process::ExitCode;

use a3s_use_core::{UseError, UseResult};

const USAGE: &str = "\
a3s-use-registry-tools — assemble, sign, and verify A3S Use registries

USAGE:
  a3s-use-registry-tools keygen --keys-dir <dir>
        [--root-share-count <n>] [--root-threshold <m>]
  a3s-use-registry-tools lint --package-dir <dir>
  a3s-use-registry-tools pack --package-dir <dir> --out <archive.tar.gz>
  a3s-use-registry-tools assemble --keys-dir <dir> --admissions <file.acl> --out-root <dir>
        [--metadata-version <n>] [--root-expires <rfc3339>] [--metadata-expires <rfc3339>]
        [--description-trust-store <trust-store.json>]
  a3s-use-registry-tools verify --registry <dir> [--expected-root-sha256 <64-hex>]
  a3s-use-registry-tools rotate-root --registry <dir>
        --previous-keys-dir <dir> --next-keys-dir <dir>
        [--root-expires <rfc3339>] [--metadata-expires <rfc3339>]
  a3s-use-registry-tools check-expiry --registry <dir> [--warn-within-hours <n>]
  a3s-use-registry-tools compare-mirrors --left <dir> --right <dir>
  a3s-use-registry-tools withdraw-targets --registry <dir> --keys-dir <dir>
        --target <path> [--target <path> ...] [--metadata-expires <rfc3339>]

keygen writes Ed25519 seed files into --keys-dir with 0600 permissions.
Default is one key per TUF role (`root.key`, `targets.key`, …). For offline
threshold root custody, set --root-share-count N and --root-threshold M
(1 <= M <= N) to write `root-0.key` … `root-(N-1).key` plus `root.policy.json`.
Seeds are custody material: keep them outside every published repository and
back them up offline. Assemble requires at least M root shares present.

lint parses a3s-use-extension.acl, requires README.md and every declared
surface file, and prints the expanded-content fingerprint Registry assembly
will reuse. It never writes archives or talks to the network.

assemble reads the admission ACL, packs each admitted package directory into
a deterministic archive, derives catalog-v3 records from the package
manifests, signs the four TUF roles (root with every present root share), and
writes metadata/ plus targets/ under --out-root. When --description-trust-store
is set, the Registry also publishes capability/description-trust-store-v1.json
as a signed TUF target so clients load description verification keys from the
same trust root. It prints the bootstrap root SHA-256 clients pin with
--trust-root.

verify re-opens a assembled tree with the same tough loader a released
client uses, decodes and validates every catalog record and planning bundle,
validates the description trust store target when present, and re-hashes
every on-disk target against its signed digest.

rotate-root proves current custody via --previous-keys-dir, writes root
version N+1 signed by --next-keys-dir, resigns online roles with the next
keys, and retains the prior root under metadata/root.history/. Clients that
pin the old bootstrap digest must receive an explicit source replacement
with the new rootSha256; GitHub redirects never become trust authority.

check-expiry reads signed.expires for root/targets/snapshot/timestamp and
fails closed when any role is already expired or inside --warn-within-hours
(default 72).

compare-mirrors requires two assembled trees to share the same root digest
and the same target path/digest set. Use it for mirror replacement drills
before promoting a failover copy.

withdraw-targets removes one or more signed target paths, deletes their
bytes, and resigns targets/snapshot/timestamp with the current online keys.
The bootstrap root pin is unchanged. Clients must refresh before install.
";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("the registry tools need a Tokio runtime");
    match runtime.block_on(run(&arguments)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: [{}] {}", error.code, error.message);
            if let Some(suggestion) = error.suggestion {
                eprintln!("  suggestion: {suggestion}");
            }
            ExitCode::FAILURE
        }
    }
}

async fn run(arguments: &[String]) -> UseResult<()> {
    let Some(command) = arguments.first() else {
        print_usage_error("a subcommand is required");
        return Ok(());
    };
    let options = Options::parse(&arguments[1..])?;
    match command.as_str() {
        "keygen" => keys::keygen_command(&options),
        "lint" => package_build::lint_command(&options).await,
        "pack" => package_build::pack_command(&options),
        "assemble" => assemble::assemble_command(&options).await,
        "verify" => verify::verify_command(&options).await,
        "rotate-root" => rotate::rotate_root_command(&options),
        "check-expiry" => expiry::check_expiry_command(&options),
        "compare-mirrors" => mirror::compare_mirrors_command(&options),
        "withdraw-targets" => withdraw::withdraw_targets_command(&options),
        "--help" | "-h" | "help" => {
            print!("{USAGE}");
            Ok(())
        }
        other => {
            print_usage_error(&format!("unknown subcommand '{other}'"));
            Ok(())
        }
    }
}

fn print_usage_error(message: &str) {
    eprintln!("error: {message}\n\n{USAGE}");
}

/// Flat `--name value` option bag shared by every subcommand.
pub(crate) struct Options {
    values: Vec<(String, String)>,
}

impl Options {
    pub(crate) fn parse(arguments: &[String]) -> UseResult<Self> {
        let mut values = Vec::new();
        let mut index = 0;
        while index < arguments.len() {
            let name = arguments[index].as_str();
            if !name.starts_with("--") {
                return Err(tools_error(
                    "arguments_invalid",
                    "Options must start with --.",
                ));
            }
            let Some(value) = arguments.get(index + 1) else {
                return Err(tools_error(
                    "arguments_invalid",
                    &format!("Option {name} requires a value."),
                ));
            };
            if value.starts_with("--") {
                return Err(tools_error(
                    "arguments_invalid",
                    &format!("Option {name} requires a value, found another option."),
                ));
            }
            values.push((name.trim_start_matches("--").to_string(), value.clone()));
            index += 2;
        }
        Ok(Self { values })
    }

    pub(crate) fn require(&self, name: &str) -> UseResult<String> {
        self.values
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .ok_or_else(|| {
                tools_error(
                    "arguments_invalid",
                    &format!("--{name} is required for this subcommand."),
                )
            })
    }

    pub(crate) fn optional(&self, name: &str) -> Option<String> {
        self.values
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    }

    pub(crate) fn all(&self, name: &str) -> Vec<String> {
        self.values
            .iter()
            .filter(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .collect()
    }
}

pub(crate) fn tools_error(code: &str, message: &str) -> UseError {
    UseError::new(code, message)
}
