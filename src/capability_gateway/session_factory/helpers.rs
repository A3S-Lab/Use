//! Session identity and durable publication checks for the Gateway factory.

use a3s_use_core::{CapabilityGatewayCatalog, UseError, UseResult};

use super::super::CapabilityGatewayMcpServer;
use super::{
    CapabilityGatewayCatalogPublication, CapabilityGatewayCatalogStore,
    CapabilityGatewaySessionKey, SESSION_PUBLICATION_ERROR,
};

pub(super) fn session_key(
    catalog: &CapabilityGatewayCatalog,
) -> UseResult<CapabilityGatewaySessionKey> {
    catalog.validate()?;
    Ok(CapabilityGatewaySessionKey {
        installation: catalog.installation().clone(),
        generation: catalog.generation(),
        revision: catalog.revision().to_owned(),
        digest: catalog.descriptor_digest()?,
    })
}

pub(super) fn server_session_key(
    server: &CapabilityGatewayMcpServer,
) -> UseResult<CapabilityGatewaySessionKey> {
    session_key(server.source_catalog())
}

pub(super) async fn verify_published_server(
    store: &CapabilityGatewayCatalogStore,
    publication: &CapabilityGatewayCatalogPublication,
    server: &CapabilityGatewayMcpServer,
) -> UseResult<()> {
    publication.validate().map_err(|_| {
        UseError::new(
            SESSION_PUBLICATION_ERROR,
            "The catalog publication identity is invalid.",
        )
    })?;
    if store.installation() != &publication.installation {
        return Err(UseError::new(
            SESSION_PUBLICATION_ERROR,
            "The catalog publication belongs to another installation store.",
        ));
    }
    let Some(published) = store
        .get_exact(
            &publication.digest,
            publication.generation,
            &publication.revision,
        )
        .await
        .map_err(|_| {
            UseError::new(
                SESSION_PUBLICATION_ERROR,
                "The durable catalog publication could not be verified.",
            )
        })?
    else {
        return Err(UseError::new(
            SESSION_PUBLICATION_ERROR,
            "The durable catalog publication is missing.",
        ));
    };
    let projected = published
        .for_consumer(server.consumer_negotiation())
        .map_err(|_| {
            UseError::new(
                SESSION_PUBLICATION_ERROR,
                "The durable catalog cannot be projected for this consumer.",
            )
        })?;
    // Verify both layers: the visible catalog must be the exact negotiated
    // projection, and the retained source must be the complete durable
    // publication. Checking only the visible subset would allow an optional
    // descriptor to be smuggled into the source and then influence lifecycle
    // identity after negotiation filters it from discovery.
    if *server.source_catalog() != published
        || projected != *server.catalog()
        || server.catalog().installation() != &publication.installation
        || server.catalog().generation() != publication.generation
    {
        return Err(UseError::new(
            SESSION_PUBLICATION_ERROR,
            "The live Gateway source or negotiated catalog does not match the durable publication.",
        ));
    }
    Ok(())
}
