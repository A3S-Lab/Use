use a3s_use_core::{
    OkfKnowledgeObservedState, PlanQualifiedSurfaceRef, PlanScope, UseError, UseResult,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::super::canonical_json;
use crate::okf_knowledge::{
    OkfKnowledgeBinding, MAX_OKF_KNOWLEDGE_SCOPE_PROJECTIONS, MAX_OKF_KNOWLEDGE_SCOPE_TOMBSTONES,
};

const CONTROL_KNOWLEDGE_PAYLOAD_INVENTORY_DOMAIN: &[u8] =
    b"a3s.use.control-knowledge-payload-inventory.v1\0";
const MAX_INVENTORY_ITEM_BYTES: usize = 512 * 1024;

pub(super) fn knowledge_inventory_digest(
    scope: &PlanScope,
    bindings: &[OkfKnowledgeBinding],
    selected: &[(PlanQualifiedSurfaceRef, u64)],
) -> UseResult<String> {
    let max_bindings = MAX_OKF_KNOWLEDGE_SCOPE_PROJECTIONS
        .checked_add(MAX_OKF_KNOWLEDGE_SCOPE_TOMBSTONES)
        .ok_or_else(|| inventory_error("The Knowledge inventory count bound overflowed."))?;
    if bindings.len() > max_bindings || selected.len() > MAX_OKF_KNOWLEDGE_SCOPE_PROJECTIONS {
        return Err(inventory_error(
            "The Knowledge inventory exceeds its hard item-count bounds.",
        ));
    }
    let mut digest = Sha256::new();
    digest.update(CONTROL_KNOWLEDGE_PAYLOAD_INVENTORY_DOMAIN);
    digest_item(&mut digest, scope)?;
    digest_count(&mut digest, bindings.len())?;
    let mut prior_binding = None;
    for binding in bindings {
        binding.validate().map_err(wrap_inventory_error)?;
        if binding.receipt.scope != *scope {
            return Err(inventory_error(
                "A Knowledge inventory binding belongs to another installation.",
            ));
        }
        let key = (binding.receipt.surface.clone(), binding.receipt.generation);
        if prior_binding.as_ref().is_some_and(|prior| prior >= &key) {
            return Err(inventory_error(
                "Knowledge inventory bindings are duplicated or not canonical.",
            ));
        }
        prior_binding = Some(key);
        digest_item(&mut digest, binding)?;
    }

    digest_count(&mut digest, selected.len())?;
    let mut prior_selection: Option<&PlanQualifiedSurfaceRef> = None;
    for (surface, generation) in selected {
        if prior_selection.is_some_and(|prior| prior >= surface) {
            return Err(inventory_error(
                "Knowledge inventory selections are duplicated or not canonical.",
            ));
        }
        let binding = bindings
            .iter()
            .find(|binding| {
                binding.receipt.surface == *surface
                    && binding.receipt.generation == *generation
                    && binding.observation.state == OkfKnowledgeObservedState::Promoted
            })
            .ok_or_else(|| {
                inventory_error("A Knowledge inventory selection has no retained promoted binding.")
            })?;
        if binding
            .observation
            .selected
            .as_ref()
            .is_none_or(|selected| {
                selected.generation != *generation
                    || selected.projection_receipt_digest
                        != binding.observation.projection_receipt_digest
            })
        {
            return Err(inventory_error(
                "A Knowledge inventory selection disagrees with its promoted observation.",
            ));
        }
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Selection<'a> {
            surface: &'a PlanQualifiedSurfaceRef,
            generation: u64,
        }
        digest_item(
            &mut digest,
            &Selection {
                surface,
                generation: *generation,
            },
        )?;
        prior_selection = Some(surface);
    }
    Ok(format!("sha256:{:x}", digest.finalize()))
}

fn digest_count(digest: &mut Sha256, count: usize) -> UseResult<()> {
    let count = u64::try_from(count)
        .map_err(|_| inventory_error("The Knowledge inventory item count overflowed."))?;
    digest.update(count.to_be_bytes());
    Ok(())
}

fn digest_item<T: Serialize>(digest: &mut Sha256, value: &T) -> UseResult<()> {
    let bytes = canonical_json(value).map_err(|error| {
        inventory_error(format!(
            "Failed to encode a canonical Knowledge inventory item: {error}"
        ))
    })?;
    if bytes.is_empty() || bytes.len() > MAX_INVENTORY_ITEM_BYTES {
        return Err(inventory_error(
            "A Knowledge inventory item exceeds its canonical byte bound.",
        ));
    }
    digest_count(digest, bytes.len())?;
    digest.update(bytes);
    Ok(())
}

fn wrap_inventory_error(error: UseError) -> UseError {
    inventory_error(format!(
        "Control Knowledge inventory verification failed: {}",
        error.message
    ))
}

fn inventory_error(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.control_store.knowledge_payload_snapshot_invalid",
        message,
    )
}
