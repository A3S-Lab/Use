// Plugin package graph upgrade/rollback methods (included into graph).

impl PluginPackageGraphLifecycleCoordinator {
    /// Prepare every added or replaced package generation in dependency order,
    /// atomically publish the candidate closure, and only then retire replaced
    /// generations in the prior graph's reverse dependency order.
    ///
    /// The prior lock is required because the candidate lock cannot prove the
    /// dependency ordering or immutable state of the generations being
    /// retired. A failed candidate preparation returns before publication, so
    /// every prior generation remains the Registry snapshot commit point.
    pub async fn apply_upgrade(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        prior_lock: &PluginPackageLock,
        candidate_units: &[PluginPackageLifecycleUnit],
        retirement_units: &[PluginPackageLifecycleUnit],
        completed_at_ms: impl Fn() -> u64,
    ) -> UseResult<Vec<PluginLifecycleOperationRecord>> {
        self.apply_upgrade_inner(
            envelope,
            prior_lock,
            candidate_units,
            retirement_units,
            #[cfg(test)]
            None,
            completed_at_ms,
        )
        .await
    }

    #[cfg(test)]
    pub async fn apply_upgrade_with_grants(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        prior_lock: &PluginPackageLock,
        candidate_units: &[PluginPackageLifecycleUnit],
        retirement_units: &[PluginPackageLifecycleUnit],
        grants: &PluginGrantLifecycleUnit,
        completed_at_ms: impl Fn() -> u64,
    ) -> UseResult<Vec<PluginLifecycleOperationRecord>> {
        self.apply_upgrade_inner(
            envelope,
            prior_lock,
            candidate_units,
            retirement_units,
            Some(grants),
            completed_at_ms,
        )
        .await
    }

    async fn apply_upgrade_inner(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        prior_lock: &PluginPackageLock,
        candidate_units: &[PluginPackageLifecycleUnit],
        retirement_units: &[PluginPackageLifecycleUnit],
        #[cfg(test)] grants: Option<&PluginGrantLifecycleUnit>,
        completed_at_ms: impl Fn() -> u64,
    ) -> UseResult<Vec<PluginLifecycleOperationRecord>> {
        let candidate_lock =
            validate_upgrade_graph(envelope, prior_lock, candidate_units, retirement_units)?;
        #[cfg(test)]
        if let Some(grants) = grants {
            grants.validate_envelope(envelope)?;
        }
        let candidates = units_by_package(candidate_units)?;
        let retirements = units_by_package(retirement_units)?;
        let mut ordered_candidates = Vec::with_capacity(candidates.len());

        let mut interrupted_rollback = Vec::new();
        let mut saw_rolling_back = false;
        let mut saw_rolled_back = false;
        for package in candidate_lock.install_order()? {
            let Some(unit) = candidates.get(package.package_id()).copied() else {
                continue;
            };
            let status = unit
                .coordinator
                .graph_candidate_status(&unit.intent)
                .await?;
            match status {
                Some(super::PluginLifecycleOperationStatus::RollingBack) => {
                    saw_rolling_back = true;
                    interrupted_rollback.push(unit);
                }
                Some(super::PluginLifecycleOperationStatus::RolledBack) => {
                    saw_rolled_back = true;
                    interrupted_rollback.push(unit);
                }
                Some(super::PluginLifecycleOperationStatus::Applying) => {
                    interrupted_rollback.push(unit);
                }
                _ => {}
            }
        }
        if saw_rolling_back {
            let replay_error = UseError::new(
                "use.plugin.package_graph_upgrade_rolled_back",
                "The interrupted candidate rollback was completed; create and review a fresh upgrade plan.",
            );
            return match self
                .rollback_upgrade_operation(
                    envelope,
                    candidate_lock,
                    &interrupted_rollback,
                    &retirements,
                    #[cfg(test)]
                    grants,
                    &completed_at_ms,
                )
                .await
            {
                Ok(()) => Err(replay_error),
                Err(rollback) => Err(attach_rollback_error(replay_error, rollback)),
            };
        }
        if saw_rolled_back {
            let replay_error = UseError::new(
                "use.plugin.package_graph_upgrade_rolled_back",
                "This candidate graph was rolled back; create and review a fresh upgrade plan.",
            );
            #[cfg(test)]
            if let Some(grants) = grants {
                let rolled_back_at_ms = completed_at_ms();
                if let Err(rollback) = grants
                    .rollback(
                        grant_rollback_key(envelope)?,
                        rolled_back_at_ms,
                        rolled_back_at_ms,
                    )
                    .await
                {
                    return Err(attach_rollback_error(replay_error, rollback));
                }
            }
            return Err(replay_error);
        }
        #[cfg(test)]
        if let Some(grants) = grants {
            if grants.is_rolled_back().await? {
                return Err(UseError::new(
                    "use.plugin.package_graph_upgrade_rolled_back",
                    "This candidate graph grant operation was rolled back; create and review a fresh upgrade plan.",
                ));
            }
            grants.prepare(completed_at_ms()).await?;
        }

        for package in candidate_lock.install_order()? {
            let transition = transition_for(envelope, package.package_id())?;
            let action = match transition.change {
                PlanPackageChangeKind::Add => PluginLifecycleAction::Install,
                PlanPackageChangeKind::Replace => PluginLifecycleAction::Upgrade,
                PlanPackageChangeKind::Retain => continue,
                PlanPackageChangeKind::Remove => {
                    return Err(graph_error(
                        "A removed package cannot appear in the candidate dependency lock.",
                    ))
                }
            };
            let unit = *candidates.get(package.package_id()).ok_or_else(|| {
                graph_error("A changed candidate dependency has no package lifecycle unit.")
            })?;
            validate_unit(envelope, unit, package.package_id(), action)?;
            ordered_candidates.push(unit);
            if let Err(error) = unit
                .coordinator
                .prepare_for_graph(&unit.intent, &unit.manifest, &completed_at_ms)
                .await
            {
                return match self
                    .rollback_upgrade_operation(
                        envelope,
                        candidate_lock,
                        &ordered_candidates,
                        &retirements,
                        #[cfg(test)]
                        grants,
                        &completed_at_ms,
                    )
                    .await
                {
                    Ok(()) => Err(error),
                    Err(rollback) => Err(attach_rollback_error(error, rollback)),
                };
            }
        }

        let intents = ordered_candidates
            .iter()
            .map(|unit| unit.intent.clone())
            .collect::<Vec<_>>();
        let removed_intents = prior_lock
            .removal_order()?
            .into_iter()
            .filter_map(|package| {
                envelope
                    .plan
                    .packages
                    .iter()
                    .find(|transition| transition.package_id == package.package_id())
                    .filter(|transition| transition.change == PlanPackageChangeKind::Remove)
                    .and_then(|_| retirements.get(package.package_id()))
                    .map(|unit| unit.intent.clone())
            })
            .collect::<Vec<_>>();
        #[cfg(test)]
        let grant_has_cutover = match grants {
            Some(grants) => grants.has_cutover().await?,
            None => false,
        };
        #[cfg(not(test))]
        let grant_has_cutover = false;
        let cutover_key = publication_key(envelope)?;
        let mut records = Vec::with_capacity(candidate_units.len() + retirement_units.len());
        if grant_has_cutover {
            records.extend(completed_publication_records(&ordered_candidates).await?);
        } else {
            let publication = self
                .publication
                .publish_upgrade_capabilities_with_cutover(
                    candidate_lock,
                    &intents,
                    &removed_intents,
                    envelope.plan.state.capability_generation,
                    &cutover_key,
                )
                .await;
            let publication = match publication {
                Ok(publication) => publication,
                Err(error) => {
                    return match self
                        .rollback_upgrade_operation(
                            envelope,
                            candidate_lock,
                            &ordered_candidates,
                            &retirements,
                            #[cfg(test)]
                            grants,
                            &completed_at_ms,
                        )
                        .await
                    {
                        Ok(()) => Err(error),
                        Err(rollback) => Err(attach_rollback_error(error, rollback)),
                    };
                }
            };
            let evidence = publication.packages;
            if evidence.len() != ordered_candidates.len() {
                return Err(graph_error(
                    "Package-graph upgrade publication omitted candidate capability evidence.",
                ));
            }
            for (unit, evidence) in ordered_candidates.iter().copied().zip(evidence) {
                if evidence.package_id != unit.intent.package_id {
                    return Err(graph_error(
                        "Package-graph upgrade evidence changed candidate order or identity.",
                    ));
                }
                records.push(
                    unit.coordinator
                        .complete_graph_publication(
                            &unit.intent,
                            &unit.manifest,
                            &evidence.evidence,
                            &completed_at_ms,
                        )
                        .await?,
                );
            }

            #[cfg(test)]
            if let Some(grants) = grants {
                let committed_at_ms = completed_at_ms();
                grants
                    .commit_cutover(&publication.cutover, committed_at_ms, committed_at_ms)
                    .await?;
            }
        }

        self.activate_capability_cutover(&cutover_key).await?;

        for package in prior_lock.removal_order()? {
            let Some(transition) = envelope
                .plan
                .packages
                .iter()
                .find(|transition| transition.package_id == package.package_id())
            else {
                continue;
            };
            if !matches!(
                transition.change,
                PlanPackageChangeKind::Replace | PlanPackageChangeKind::Remove
            ) {
                continue;
            }
            let unit = *retirements.get(package.package_id()).ok_or_else(|| {
                graph_error("A replaced dependency has no prior-generation retirement unit.")
            })?;
            validate_unit(
                envelope,
                unit,
                package.package_id(),
                PluginLifecycleAction::Uninstall,
            )?;
            unit.coordinator
                .drain_graph_retirement(&unit.intent, &unit.manifest, &completed_at_ms)
                .await?;
        }
        #[cfg(test)]
        if let Some(grants) = grants {
            grants.retire().await?;
        }

        for package in prior_lock.removal_order()? {
            let Some(transition) = envelope
                .plan
                .packages
                .iter()
                .find(|transition| transition.package_id == package.package_id())
            else {
                continue;
            };
            if !matches!(
                transition.change,
                PlanPackageChangeKind::Replace | PlanPackageChangeKind::Remove
            ) {
                continue;
            }
            let unit = *retirements.get(package.package_id()).ok_or_else(|| {
                graph_error("A replaced dependency has no prior-generation retirement unit.")
            })?;
            validate_unit(
                envelope,
                unit,
                package.package_id(),
                PluginLifecycleAction::Uninstall,
            )?;
            records.push(
                unit.coordinator
                    .apply(&unit.intent, &unit.manifest, &completed_at_ms)
                    .await?,
            );
        }
        self.publication
            .complete_capability_cutover(&cutover_key)
            .await?;
        Ok(records)
    }

    async fn rollback_upgrade_operation(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        candidate_lock: &PluginPackageLock,
        candidates: &[&PluginPackageLifecycleUnit],
        retirements: &BTreeMap<&str, &PluginPackageLifecycleUnit>,
        #[cfg(test)] grants: Option<&PluginGrantLifecycleUnit>,
        completed_at_ms: &impl Fn() -> u64,
    ) -> UseResult<()> {
        let package_rollback = self
            .rollback_upgrade_candidates(candidate_lock, candidates, retirements, completed_at_ms)
            .await;
        #[cfg(test)]
        let grant_rollback = match grants {
            Some(grants) => {
                let rolled_back_at_ms = completed_at_ms();
                grants
                    .rollback(
                        grant_rollback_key(envelope)?,
                        rolled_back_at_ms,
                        rolled_back_at_ms,
                    )
                    .await
                    .map(drop)
            }
            None => Ok(()),
        };
        #[cfg(not(test))]
        let grant_rollback = {
            let _ = envelope;
            Ok(())
        };
        match (package_rollback, grant_rollback) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(package), Ok(())) => Err(package),
            (Ok(()), Err(grant)) => Err(grant),
            (Err(package), Err(grant)) => Err(attach_rollback_error(package, grant)),
        }
    }

    async fn rollback_upgrade_candidates(
        &self,
        candidate_lock: &PluginPackageLock,
        candidates: &[&PluginPackageLifecycleUnit],
        retirements: &BTreeMap<&str, &PluginPackageLifecycleUnit>,
        completed_at_ms: &impl Fn() -> u64,
    ) -> UseResult<()> {
        for unit in candidates {
            let status = unit
                .coordinator
                .graph_candidate_status(&unit.intent)
                .await?;
            if status == Some(super::PluginLifecycleOperationStatus::Applying) {
                unit.coordinator
                    .start_graph_rollback(&unit.intent, &unit.manifest)
                    .await?;
            } else if !matches!(
                status,
                Some(super::PluginLifecycleOperationStatus::RollingBack)
                    | Some(super::PluginLifecycleOperationStatus::RolledBack)
            ) {
                return Err(graph_error(
                    "A candidate rollback lost its exact applying lifecycle operation.",
                ));
            }
        }

        let mut surface_evidence = BTreeMap::new();
        for unit in candidates.iter().rev() {
            let evidence = unit
                .coordinator
                .rollback_graph_candidate_surfaces(&unit.intent, &unit.manifest)
                .await?;
            surface_evidence.insert(unit.intent.package_id.as_str(), evidence);
        }

        let candidate_intents = candidates
            .iter()
            .map(|unit| unit.intent.clone())
            .collect::<Vec<_>>();
        let mut prior_intents = Vec::new();
        for unit in candidates {
            let transition = candidate_lock
                .package(&unit.intent.package_id)
                .ok_or_else(|| {
                    graph_error("A rollback candidate disappeared from its dependency lock.")
                })?;
            if let Some(prior) = retirements.get(transition.package_id()) {
                prior_intents.push(prior.intent.clone());
            }
        }
        let package_evidence = self
            .publication
            .rollback_candidates(
                candidate_lock,
                &candidate_intents,
                &prior_intents,
                &rollback_key(candidate_lock, &candidate_intents)?,
            )
            .await?;
        if package_evidence.len() != candidates.len() {
            return Err(graph_error(
                "Package-graph rollback omitted candidate package evidence.",
            ));
        }
        let package_evidence = package_evidence
            .into_iter()
            .map(|evidence| (evidence.package_id, evidence.evidence))
            .collect::<BTreeMap<_, _>>();
        if package_evidence.len() != candidates.len() {
            return Err(graph_error(
                "Package-graph rollback returned duplicate candidate evidence.",
            ));
        }
        for unit in candidates {
            if unit
                .coordinator
                .graph_candidate_status(&unit.intent)
                .await?
                == Some(super::PluginLifecycleOperationStatus::RolledBack)
            {
                continue;
            }
            let surfaces = surface_evidence
                .get(unit.intent.package_id.as_str())
                .ok_or_else(|| {
                    graph_error("A candidate rollback omitted surface cleanup evidence.")
                })?;
            let package = package_evidence
                .get(&unit.intent.package_id)
                .ok_or_else(|| {
                    graph_error("A candidate rollback changed package evidence identity.")
                })?;
            unit.coordinator
                .complete_graph_rollback(
                    &unit.intent,
                    &unit.manifest,
                    surfaces,
                    package,
                    completed_at_ms,
                )
                .await?;
        }
        Ok(())
    }
}
