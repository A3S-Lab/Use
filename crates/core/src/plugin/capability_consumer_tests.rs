use super::*;

#[test]
fn generic_profile_has_no_extensions_and_round_trips_canonically() {
    let profile = CapabilityConsumerProfile::generic_mcp();
    profile.validate().unwrap();
    let bytes = profile.canonical_bytes().unwrap();
    assert_eq!(
        CapabilityConsumerProfile::from_json(&bytes).unwrap(),
        profile
    );
    assert!(profile.is_generic_mcp());
    assert!(profile.requested_extensions().is_empty());
}

#[test]
fn a3s_profile_sorts_extensions_and_rejects_duplicates() {
    let profile = CapabilityConsumerProfile::a3s([
        CapabilityConsumerExtension::Ui,
        CapabilityConsumerExtension::Flow,
    ])
    .unwrap();
    assert_eq!(
        profile.requested_extensions(),
        &[
            CapabilityConsumerExtension::Flow,
            CapabilityConsumerExtension::Ui,
        ]
    );
    assert!(CapabilityConsumerProfile::a3s([
        CapabilityConsumerExtension::Flow,
        CapabilityConsumerExtension::Flow,
    ])
    .is_err());
}

#[test]
fn generic_profile_cannot_request_a3s_extensions() {
    let profile = CapabilityConsumerProfile {
        schema: CAPABILITY_CONSUMER_PROFILE_SCHEMA_V1.to_owned(),
        kind: CapabilityConsumerKind::GenericMcp,
        requested_extensions: vec![CapabilityConsumerExtension::Knowledge],
    };
    assert!(profile.validate().is_err());
}

#[test]
fn negotiation_rejects_an_unsupported_extension_instead_of_downgrading() {
    let profile = CapabilityConsumerProfile::a3s([
        CapabilityConsumerExtension::Flow,
        CapabilityConsumerExtension::Knowledge,
    ])
    .unwrap();
    assert!(
        CapabilityConsumerNegotiation::negotiate(profile, [CapabilityConsumerExtension::Flow])
            .is_err()
    );
}

#[test]
fn negotiation_binds_the_exact_requested_set_and_digest() {
    let profile = CapabilityConsumerProfile::a3s([
        CapabilityConsumerExtension::Ui,
        CapabilityConsumerExtension::Knowledge,
    ])
    .unwrap();
    let negotiation = CapabilityConsumerNegotiation::negotiate(
        profile.clone(),
        [
            CapabilityConsumerExtension::Knowledge,
            CapabilityConsumerExtension::Ui,
            CapabilityConsumerExtension::Flow,
        ],
    )
    .unwrap();
    assert_eq!(negotiation.profile(), &profile);
    assert!(negotiation.accepts(CapabilityConsumerExtension::Ui));
    assert!(!negotiation.accepts(CapabilityConsumerExtension::Flow));
    let bytes = negotiation.canonical_bytes().unwrap();
    assert_eq!(
        CapabilityConsumerNegotiation::from_json(&bytes).unwrap(),
        negotiation
    );
    assert_eq!(
        negotiation.descriptor_digest().unwrap(),
        CapabilityConsumerNegotiation::from_json(&bytes)
            .unwrap()
            .descriptor_digest()
            .unwrap()
    );
}

#[test]
fn negotiation_rejects_a_tampered_accepted_set() {
    let mut negotiation = CapabilityConsumerNegotiation::generic_mcp();
    negotiation.accepted_extensions = vec![CapabilityConsumerExtension::Flow];
    assert!(negotiation.validate().is_err());
}

#[test]
fn consumer_contracts_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CapabilityConsumerProfile>();
    assert_send_sync::<CapabilityConsumerNegotiation>();
}
