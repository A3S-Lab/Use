use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::{
    inspect_verified_target_cache, prune_verified_target_cache, ExtensionPaths, TrustedRegistry,
    VerifiedTargetCachePolicy, DEFAULT_VERIFIED_TARGET_CACHE_MAX_BYTES,
    DEFAULT_VERIFIED_TARGET_CACHE_MAX_ENTRIES, DEFAULT_VERIFIED_TARGET_CACHE_MIN_FREE_BYTES,
};

use super::{flag_argument, option_argument, usage_error, CommandOutput};

pub(super) async fn run(args: &[String]) -> UseResult<CommandOutput> {
    match (
        args.first().map(String::as_str),
        args.get(1).map(String::as_str),
    ) {
        (Some("cache"), Some("usage")) => usage(args).await,
        (Some("cache"), Some("prune")) => prune(args).await,
        (Some("cache"), Some(command)) => Err(usage_error(format!(
            "unknown registry cache command '{command}'"
        ))),
        (Some(command), _) => Err(usage_error(format!("unknown registry command '{command}'"))),
        (None, _) => Err(usage_error("registry requires cache usage or cache prune")),
    }
}

pub(super) fn cache_policy(args: &[String]) -> UseResult<VerifiedTargetCachePolicy> {
    VerifiedTargetCachePolicy::new(
        numeric_option(
            args,
            "--cache-max-bytes",
            DEFAULT_VERIFIED_TARGET_CACHE_MAX_BYTES,
        )?,
        numeric_option(
            args,
            "--cache-max-entries",
            DEFAULT_VERIFIED_TARGET_CACHE_MAX_ENTRIES,
        )?,
        numeric_option(
            args,
            "--cache-min-free-bytes",
            DEFAULT_VERIFIED_TARGET_CACHE_MIN_FREE_BYTES,
        )?,
    )
}

async fn usage(args: &[String]) -> UseResult<CommandOutput> {
    validate_options(args, false)?;
    let registry = configured_registry(args)?;
    let usage = inspect_verified_target_cache(&registry).await?;
    Ok(CommandOutput::success(
        format!(
            "Registry '{}' verified target cache contains {} entries and {} bytes.",
            usage.registry_name, usage.target_entries, usage.target_bytes
        ),
        serde_json::json!({ "registryCache": usage }),
    ))
}

async fn prune(args: &[String]) -> UseResult<CommandOutput> {
    validate_options(args, true)?;
    if !flag_argument(args, "--yes")? {
        return Err(usage_error(
            "registry cache prune requires --yes because cached offline targets may be removed",
        ));
    }
    let registry = configured_registry(args)?;
    let result = prune_verified_target_cache(&registry).await?;
    Ok(CommandOutput::success(
        format!(
            "Pruned {} verified targets and {} stale cache files from Registry '{}'.",
            result.removed_target_entries, result.removed_stale_entries, result.after.registry_name
        ),
        serde_json::json!({ "registryCache": result }),
    ))
}

fn configured_registry(args: &[String]) -> UseResult<TrustedRegistry> {
    let registry_name = required_option(args, "--registry-name")?;
    let registry_url = required_option(args, "--registry-url")?;
    let trust_root = required_option(args, "--trust-root")?;
    let paths = ExtensionPaths::from_env()?;
    Ok(TrustedRegistry::new(
        registry_name,
        registry_url,
        trust_root,
        None,
        paths.tuf_datastore(registry_name)?,
    )?
    .with_target_cache_policy(cache_policy(args)?))
}

fn required_option<'a>(args: &'a [String], name: &str) -> UseResult<&'a str> {
    option_argument(args, name)?
        .ok_or_else(|| usage_error(format!("registry cache requires {name}")))
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

fn validate_options(args: &[String], allow_yes: bool) -> UseResult<()> {
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--json" => index += 1,
            "--yes" if allow_yes => index += 1,
            "--registry-name"
            | "--registry-url"
            | "--trust-root"
            | "--cache-max-bytes"
            | "--cache-max-entries"
            | "--cache-min-free-bytes" => {
                if args.get(index + 1).is_none() {
                    return Err(usage_error(format!("{} requires a value", args[index])));
                }
                index += 2;
            }
            value => {
                return Err(usage_error(format!(
                    "unknown registry cache option '{value}'"
                )))
            }
        }
    }
    Ok(())
}
