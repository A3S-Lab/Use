use std::path::PathBuf;

use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::{
    inspect_verified_target_cache, prune_verified_target_cache, GitHubRegistryRepository,
    RegistrySourceInput, RegistrySourceStore, TrustedRegistry, VerifiedTargetCachePolicy,
    DEFAULT_GITHUB_REGISTRY_PATH, DEFAULT_GITHUB_REGISTRY_REF,
    DEFAULT_VERIFIED_TARGET_CACHE_MAX_BYTES, DEFAULT_VERIFIED_TARGET_CACHE_MAX_ENTRIES,
    DEFAULT_VERIFIED_TARGET_CACHE_MIN_FREE_BYTES,
};

use super::{flag_argument, option_argument, usage_error, value_argument, CommandOutput};

pub(super) async fn run(args: &[String]) -> UseResult<CommandOutput> {
    match (
        args.first().map(String::as_str),
        args.get(1).map(String::as_str),
    ) {
        (Some("source"), Some("list")) => source_list(args).await,
        (Some("source"), Some("add")) => source_add(args).await,
        (Some("source"), Some("replace")) => source_replace(args).await,
        (Some("source"), Some("remove")) => source_remove(args).await,
        (Some("source"), Some("default")) => source_default(args).await,
        (Some("source"), Some("enable")) => source_enablement(args, true).await,
        (Some("source"), Some("disable")) => source_enablement(args, false).await,
        (Some("source"), Some(command)) => Err(usage_error(format!(
            "unknown registry source command '{command}'"
        ))),
        (Some("source"), None) => Err(usage_error(
            "registry source requires list, add, replace, remove, default, enable, or disable",
        )),
        (Some("cache"), Some("usage")) => cache_usage(args).await,
        (Some("cache"), Some("prune")) => cache_prune(args).await,
        (Some("cache"), Some(command)) => Err(usage_error(format!(
            "unknown registry cache command '{command}'"
        ))),
        (Some("cache"), None) => Err(usage_error("registry cache requires usage or prune")),
        (Some(command), _) => Err(usage_error(format!("unknown registry command '{command}'"))),
        (None, _) => Err(usage_error("registry requires source or cache")),
    }
}

async fn source_list(args: &[String]) -> UseResult<CommandOutput> {
    validate_source_list_options(args)?;
    let snapshot = RegistrySourceStore::from_env()?.snapshot().await?;
    let human = if snapshot.sources.is_empty() {
        "No Registry sources are configured.".to_owned()
    } else {
        snapshot
            .sources
            .iter()
            .map(|source| {
                format!(
                    "{}{}\t{}\tsha256:{}\t{}",
                    if snapshot.default_registry.as_deref() == Some(&source.name) {
                        "* "
                    } else {
                        "  "
                    },
                    source.name,
                    source.registry_url,
                    source.root_sha256,
                    if source.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    },
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(CommandOutput::success(
        human,
        serde_json::json!({ "registrySources": snapshot }),
    ))
}

async fn source_add(args: &[String]) -> UseResult<CommandOutput> {
    validate_source_write_options(args, false)?;
    let name = value_argument(args, 2, "registry source add requires a name")?;
    let mutation = RegistrySourceStore::from_env()?
        .add(source_input(args, name)?)
        .await?;
    Ok(CommandOutput::success(
        if mutation.changed {
            format!("Added Registry source '{name}'.")
        } else {
            format!("Registry source '{name}' already has the exact requested configuration.")
        },
        serde_json::json!({ "registrySources": mutation }),
    ))
}

async fn source_replace(args: &[String]) -> UseResult<CommandOutput> {
    validate_source_write_options(args, true)?;
    require_confirmation(args, "registry source replace")?;
    let name = value_argument(args, 2, "registry source replace requires a name")?;
    let expected = required_option(args, "--expected-revision", "registry source replace")?;
    let mutation = RegistrySourceStore::from_env()?
        .replace(expected, source_input(args, name)?)
        .await?;
    Ok(CommandOutput::success(
        if mutation.changed {
            format!("Replaced Registry source '{name}' without rewriting prior source state.")
        } else {
            format!("Registry source '{name}' already has the exact requested configuration.")
        },
        serde_json::json!({ "registrySources": mutation }),
    ))
}

async fn source_remove(args: &[String]) -> UseResult<CommandOutput> {
    validate_source_revision_options(args, "remove")?;
    require_confirmation(args, "registry source remove")?;
    let name = value_argument(args, 2, "registry source remove requires a name")?;
    let expected = required_option(args, "--expected-revision", "registry source remove")?;
    let mutation = RegistrySourceStore::from_env()?
        .remove(name, expected)
        .await?;
    Ok(CommandOutput::success(
        format!("Removed Registry source '{name}'; its identity-bound TUF state was retained."),
        serde_json::json!({ "registrySources": mutation }),
    ))
}

async fn source_default(args: &[String]) -> UseResult<CommandOutput> {
    validate_source_revision_options(args, "default")?;
    require_confirmation(args, "registry source default")?;
    let name = value_argument(args, 2, "registry source default requires a name")?;
    let expected = required_option(args, "--expected-revision", "registry source default")?;
    let mutation = RegistrySourceStore::from_env()?
        .set_default(name, expected)
        .await?;
    Ok(CommandOutput::success(
        if mutation.changed {
            format!("Selected Registry source '{name}' as the default.")
        } else {
            format!("Registry source '{name}' is already the default.")
        },
        serde_json::json!({ "registrySources": mutation }),
    ))
}

async fn source_enablement(args: &[String], enabled: bool) -> UseResult<CommandOutput> {
    let action = if enabled { "enable" } else { "disable" };
    validate_source_revision_options(args, action)?;
    require_confirmation(args, &format!("registry source {action}"))?;
    let name = value_argument(
        args,
        2,
        &format!("registry source {action} requires a name"),
    )?;
    let expected = required_option(
        args,
        "--expected-revision",
        &format!("registry source {action}"),
    )?;
    let store = RegistrySourceStore::from_env()?;
    let mutation = if enabled {
        store.enable(name, expected).await?
    } else {
        store.disable(name, expected).await?
    };
    Ok(CommandOutput::success(
        if mutation.changed {
            format!("{action}d Registry source '{name}'.")
        } else {
            format!("Registry source '{name}' is already {action}d.")
        },
        serde_json::json!({ "registrySources": mutation }),
    ))
}

async fn cache_usage(args: &[String]) -> UseResult<CommandOutput> {
    validate_cache_options(args, false)?;
    let registry = configured_registry(args).await?;
    let usage = inspect_verified_target_cache(&registry).await?;
    Ok(CommandOutput::success(
        format!(
            "Registry '{}' source cache contains {} target observations ({} referenced blob bytes), {} resumable partials ({} physical bytes), and {} stale writes ({} physical bytes).",
            usage.registry_name,
            usage.target_entries,
            usage.target_bytes,
            usage.partial_entries,
            usage.partial_bytes,
            usage.stale_entries,
            usage.stale_bytes
        ),
        serde_json::json!({ "registryCache": usage }),
    ))
}

async fn cache_prune(args: &[String]) -> UseResult<CommandOutput> {
    validate_cache_options(args, true)?;
    require_confirmation(args, "registry cache prune")?;
    let registry = configured_registry(args).await?;
    let result = prune_verified_target_cache(&registry).await?;
    Ok(CommandOutput::success(
        format!(
            "Pruned {} target observations, {} resumable partials, and {} stale cache files from Registry '{}'; global artifact blobs were retained.",
            result.removed_target_entries,
            result.removed_partial_entries,
            result.removed_stale_entries,
            result.after.registry_name
        ),
        serde_json::json!({ "registryCache": result }),
    ))
}

fn source_input(args: &[String], name: &str) -> UseResult<RegistrySourceInput> {
    let url = option_argument(args, "--url")?;
    let github = option_argument(args, "--github")?;
    let registry_url = match (url, github) {
        (Some(url), None) => {
            if option_argument(args, "--github-ref")?.is_some()
                || option_argument(args, "--github-path")?.is_some()
            {
                return Err(usage_error(
                    "--github-ref and --github-path require --github",
                ));
            }
            url.to_owned()
        }
        (None, Some(slug)) => GitHubRegistryRepository::parse(slug)?
            .with_git_ref(
                option_argument(args, "--github-ref")?.unwrap_or(DEFAULT_GITHUB_REGISTRY_REF),
            )?
            .with_registry_path(
                option_argument(args, "--github-path")?.unwrap_or(DEFAULT_GITHUB_REGISTRY_PATH),
            )?
            .registry_url()?,
        _ => {
            return Err(usage_error(
                "registry source requires exactly one of --url or --github",
            ));
        }
    };
    let trust_root = required_option(args, "--trust-root", "registry source")?;
    let trusted_root = option_argument(args, "--trusted-root")?
        .map(resolve_path)
        .transpose()?;
    Ok(RegistrySourceInput::new(
        name,
        registry_url,
        trust_root,
        trusted_root,
        source_cache_policy(args)?,
    ))
}

async fn configured_registry(args: &[String]) -> UseResult<TrustedRegistry> {
    let selected = option_argument(args, "--registry-name")?;
    let resolved = RegistrySourceStore::from_env()?.resolve(selected).await?;
    let baseline = resolved.root().target_cache_policy();
    Ok(resolved
        .root()
        .clone()
        .with_target_cache_policy(cache_policy(args, baseline)?))
}

fn source_cache_policy(args: &[String]) -> UseResult<VerifiedTargetCachePolicy> {
    cache_policy(
        args,
        VerifiedTargetCachePolicy::new(
            DEFAULT_VERIFIED_TARGET_CACHE_MAX_BYTES,
            DEFAULT_VERIFIED_TARGET_CACHE_MAX_ENTRIES,
            DEFAULT_VERIFIED_TARGET_CACHE_MIN_FREE_BYTES,
        )?,
    )
}

fn cache_policy(
    args: &[String],
    baseline: VerifiedTargetCachePolicy,
) -> UseResult<VerifiedTargetCachePolicy> {
    VerifiedTargetCachePolicy::new(
        numeric_option(args, "--cache-max-bytes", baseline.max_bytes())?,
        numeric_option(args, "--cache-max-entries", baseline.max_entries())?,
        numeric_option(args, "--cache-min-free-bytes", baseline.min_free_bytes())?,
    )
}

fn required_option<'a>(args: &'a [String], name: &str, command: &str) -> UseResult<&'a str> {
    option_argument(args, name)?.ok_or_else(|| usage_error(format!("{command} requires {name}")))
}

fn require_confirmation(args: &[String], command: &str) -> UseResult<()> {
    if flag_argument(args, "--yes")? {
        Ok(())
    } else {
        Err(usage_error(format!(
            "{command} requires --yes after reviewing the current source revision"
        )))
    }
}

fn resolve_path(value: &str) -> UseResult<PathBuf> {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Ok(path)
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(path))
            .map_err(|error| {
                UseError::new(
                    "use.extension.registry_path_invalid",
                    format!("Failed to resolve the trusted root path: {error}"),
                )
            })
    }
}

fn numeric_option(args: &[String], name: &str, default: u64) -> UseResult<u64> {
    let Some(value) = option_argument(args, name)? else {
        return Ok(default);
    };
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(usage_error(format!(
            "{name} must be an unsigned decimal integer"
        )));
    }
    value.parse::<u64>().map_err(|error| {
        UseError::new(
            "use.cli.invalid_usage",
            format!("{name} is outside the supported integer range: {error}"),
        )
    })
}

fn validate_source_list_options(args: &[String]) -> UseResult<()> {
    validate_options(args, 2, &["--json"], &[], "registry source list")
}

fn validate_source_write_options(args: &[String], replacement: bool) -> UseResult<()> {
    let mut flags = vec!["--json"];
    let mut values = vec![
        "--url",
        "--github",
        "--github-ref",
        "--github-path",
        "--trust-root",
        "--trusted-root",
        "--cache-max-bytes",
        "--cache-max-entries",
        "--cache-min-free-bytes",
    ];
    if replacement {
        flags.push("--yes");
        values.push("--expected-revision");
    }
    validate_options(args, 3, &flags, &values, "registry source")
}

fn validate_source_revision_options(args: &[String], action: &str) -> UseResult<()> {
    validate_options(
        args,
        3,
        &["--json", "--yes"],
        &["--expected-revision"],
        &format!("registry source {action}"),
    )
}

fn validate_cache_options(args: &[String], allow_prune: bool) -> UseResult<()> {
    let mut flags = vec!["--json"];
    if allow_prune {
        flags.push("--yes");
    }
    validate_options(
        args,
        2,
        &flags,
        &[
            "--registry-name",
            "--cache-max-bytes",
            "--cache-max-entries",
            "--cache-min-free-bytes",
        ],
        "registry cache",
    )
}

fn validate_options(
    args: &[String],
    mut index: usize,
    flags: &[&str],
    values: &[&str],
    command: &str,
) -> UseResult<()> {
    while index < args.len() {
        let option = args[index].as_str();
        if flags.contains(&option) {
            index += 1;
        } else if values.contains(&option) {
            if args
                .get(index + 1)
                .is_none_or(|value| value.starts_with('-'))
            {
                return Err(usage_error(format!("{option} requires a value")));
            }
            index += 2;
        } else {
            return Err(usage_error(format!("unknown {command} option '{option}'")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_registry_options_resolve_to_a_canonical_repository_registry() {
        let args = vec![
            "source".to_owned(),
            "add".to_owned(),
            "official".to_owned(),
            "--github".to_owned(),
            "A3S-Lab/Use-Registry".to_owned(),
            "--github-ref".to_owned(),
            "main".to_owned(),
            "--github-path".to_owned(),
            "registry".to_owned(),
            "--trust-root".to_owned(),
            format!("sha256:{}", "a".repeat(64)),
        ];

        validate_source_write_options(&args, false).unwrap();
        let input = source_input(&args, "official").unwrap();

        assert_eq!(
            input.registry_url,
            "https://raw.githubusercontent.com/A3S-Lab/Use-Registry/main/registry/"
        );
        assert_eq!(input.root_sha256, format!("sha256:{}", "a".repeat(64)));
    }

    #[test]
    fn registry_source_rejects_ambiguous_url_and_github_authority() {
        let args = vec![
            "source".to_owned(),
            "add".to_owned(),
            "official".to_owned(),
            "--url".to_owned(),
            "https://packages.example/".to_owned(),
            "--github".to_owned(),
            "A3S-Lab/Use-Registry".to_owned(),
            "--trust-root".to_owned(),
            "a".repeat(64),
        ];

        validate_source_write_options(&args, false).unwrap();
        let error = source_input(&args, "official").unwrap_err();

        assert_eq!(error.code, "use.cli.invalid_usage");
        assert!(error.message.contains("exactly one of --url or --github"));
    }

    #[test]
    fn source_cache_policy_options_are_typed_and_non_repeatable() {
        let args = vec![
            "source".to_owned(),
            "add".to_owned(),
            "packages".to_owned(),
            "--cache-max-bytes".to_owned(),
            "1024".to_owned(),
            "--cache-max-entries".to_owned(),
            "8".to_owned(),
            "--cache-min-free-bytes".to_owned(),
            "0".to_owned(),
        ];
        let policy = source_cache_policy(&args).unwrap();
        assert_eq!(policy.max_bytes(), 1024);
        assert_eq!(policy.max_entries(), 8);
        assert_eq!(policy.min_free_bytes(), 0);

        let mut duplicate = args.clone();
        duplicate.extend(["--cache-max-bytes".to_owned(), "2048".to_owned()]);
        let error = source_cache_policy(&duplicate).unwrap_err();
        assert_eq!(error.code, "use.cli.invalid_usage");
        assert_eq!(error.message, "--cache-max-bytes may be provided only once");

        let invalid = vec![
            "source".to_owned(),
            "add".to_owned(),
            "packages".to_owned(),
            "--cache-max-entries".to_owned(),
            "-1".to_owned(),
        ];
        let error = source_cache_policy(&invalid).unwrap_err();
        assert_eq!(error.code, "use.cli.invalid_usage");
    }
}
