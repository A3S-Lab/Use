use super::*;

pub(super) async fn completed_publication_records(
    units: &[&PluginPackageLifecycleUnit],
) -> UseResult<Vec<PluginLifecycleOperationRecord>> {
    let mut records = Vec::with_capacity(units.len());
    for unit in units {
        let record = unit
            .coordinator
            .load_exact_record(&unit.intent)
            .await?
            .ok_or_else(|| {
                graph_error(
                    "A committed Grant cutover has no matching package publication journal.",
                )
            })?;
        if record.status != super::super::PluginLifecycleOperationStatus::Completed {
            return Err(graph_error(
                "A committed Grant cutover has an incomplete package publication journal.",
            ));
        }
        records.push(record);
    }
    Ok(records)
}

pub(super) async fn validate_hidden_records(
    units: &[&PluginPackageLifecycleUnit],
) -> UseResult<()> {
    for unit in units {
        let record = unit
            .coordinator
            .load_exact_record(&unit.intent)
            .await?
            .ok_or_else(|| {
                graph_error("A committed Grant cutover has no matching package hide journal.")
            })?;
        if record.next_checkpoint().is_some_and(|checkpoint| {
            checkpoint.kind == super::super::PluginLifecycleCheckpointKind::CapabilityHidden
        }) || matches!(
            record.status,
            super::super::PluginLifecycleOperationStatus::RollingBack
                | super::super::PluginLifecycleOperationStatus::RolledBack
        ) {
            return Err(graph_error(
                "A committed Grant cutover has no durable package hide checkpoint.",
            ));
        }
    }
    Ok(())
}
