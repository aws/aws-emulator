// SPDX-License-Identifier: CC0-1.0

//! `DeleteConnector` over the class of stored connectors.
//!
//! RULE under test: `DeleteConnector` RELEASES the stored connector its `ConnectorId`
//! names and nothing else — the deleted id is afterwards unknown to every reader of a
//! connector (`DescribeConnector`, `ListConnectors`, `StartFileTransfer`) and the store
//! capacity it held is free again, while every other stored connector reads back
//! unchanged; a `ConnectorId` the store does not hold (never created, or already
//! deleted) is answered with this service's not-found error and changes nothing.
//!
//! The rows differ along what the rule generalises over: the protocol configuration of
//! the connector deleted (AS2 or SFTP), its egress type (service-managed or VPC
//! Lattice), whether it carries tags and optional members, how many connectors the
//! store holds beside it, its position in `ConnectorId` order (first, middle, last),
//! and — for the refusals — where the absent id sits in the id space.
//!
//! Every call goes over the real wire path through the shared [`crate::harness`], so
//! what is asserted is what an SDK sees.

use serde_json::{json, Value};

use service_com_amazonaws_transfer_transferservice::ops::types::SftpConnectorConfig;
use service_com_amazonaws_transfer_transferservice::state::{
    connector_arn, service_managed_egress_ip_addresses, Connector, CONNECTOR_STATUS_ACTIVE,
    DEFAULT_CONNECTOR_EGRESS_TYPE, DEFAULT_CONNECTOR_SECURITY_POLICY_NAME,
    DEFAULT_IP_ADDRESS_TYPE, DEFAULT_MAX_CONCURRENT_CONNECTIONS, DEFAULT_SECRET_VERSION_STAGE,
    MAX_STORED_CONNECTORS, RESOURCE_NOT_FOUND_HTTP_STATUS,
};

use crate::harness::{assert_members, member_of, Emulator, REGION, ROLE};

/// A `VpcLatticeResourceConfigurationArn` satisfying the member's modelled pattern
/// `^arn:[a-z0-9\-]+:vpc-lattice:[a-zA-Z0-9\-]+:\d{12}:resourceconfiguration/rcfg-[0-9a-z]{17}$`.
const RESOURCE_CONFIGURATION_ARN: &str =
    "arn:aws:vpc-lattice:us-east-1:000000000000:resourceconfiguration/rcfg-0123456789abcdef0";

/// A `ProfileId` (`len 19..19`, pattern `^p-([0-9a-f]{17})$`) for an AS2 connector.
const LOCAL_PROFILE: &str = "p-0123456789abcdef0";

/// A second `ProfileId`, for the partner side of the same AS2 connector.
const PARTNER_PROFILE: &str = "p-00000000000000001";

/// The 17 hex digits of a minted id, as a number — so a test can build an id that
/// neighbours a minted one, and check a refused call minted nothing.
fn id_sequence(connector_id: &str) -> u64 {
    u64::from_str_radix(&connector_id[2..], 16).expect("a minted id ends in 17 hex digits")
}

/// The `CreateConnector` request bodies this suite deletes, one per row of the class:
/// AS2 and SFTP, service-managed and VPC Lattice egress, with and without the optional
/// members and tags.
fn connector_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "AS2 over HTTPS, nothing beyond the required members supplied",
            json!({
                "AccessRole": ROLE,
                "Url": "https://partner.example.com/as2",
                "As2Config": {
                    "LocalProfileId": LOCAL_PROFILE,
                    "PartnerProfileId": PARTNER_PROFILE,
                    "Compression": "DISABLED",
                    "EncryptionAlgorithm": "NONE",
                    "SigningAlgorithm": "NONE",
                },
            }),
        ),
        (
            "AS2 with tags, a logging role and an IP address type",
            json!({
                "AccessRole": ROLE,
                "LoggingRole": "arn:aws:iam::000000000000:role/transfer-logging",
                "Url": "http://partner.example.com:8080/as2",
                "IpAddressType": "DUALSTACK",
                "As2Config": {
                    "LocalProfileId": LOCAL_PROFILE,
                    "PartnerProfileId": PARTNER_PROFILE,
                    "Compression": "ZLIB",
                    "EncryptionAlgorithm": "AES256_CBC",
                    "SigningAlgorithm": "SHA384",
                },
                "Tags": [
                    {"Key": "team", "Value": "edi"},
                    {"Key": "env", "Value": "test"},
                ],
            }),
        ),
        (
            "service-managed SFTP, defaults left to the service",
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com",
                "SftpConfig": {"UserSecretId": "sftp-partner-credentials"},
            }),
        ),
        (
            "service-managed SFTP with a security policy and two trusted host keys",
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com:2222",
                "SecurityPolicyName": "TransferSFTPConnectorSecurityPolicy-2024-03",
                "SftpConfig": {
                    "UserSecretId": "sftp",
                    "MaxConcurrentConnections": 5,
                    "TrustedHostKeys": [
                        "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ==",
                        "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTY=",
                    ],
                },
            }),
        ),
        (
            "SFTP over VPC Lattice, which carries no Url",
            json!({
                "AccessRole": ROLE,
                "SftpConfig": {"UserSecretId": "sftp"},
                "EgressConfig": {
                    "VpcLattice": {
                        "ResourceConfigurationArn": RESOURCE_CONFIGURATION_ARN,
                        "PortNumber": 65535,
                    },
                },
            }),
        ),
    ]
}

/// Create a connector, assert the call succeeded, and return its `ConnectorId`.
async fn create_connector(emulator: &Emulator, case: &str, request: Value) -> String {
    let answer = emulator.call("CreateConnector", request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateConnector 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .get("ConnectorId")
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: CreateConnector returned no ConnectorId: {}", answer.body)
        })
        .to_string()
}

/// `DescribeConnector` for `connector_id`, as the raw answer.
async fn describe(emulator: &Emulator, connector_id: &str) -> crate::harness::Answer {
    emulator
        .call("DescribeConnector", json!({"ConnectorId": connector_id}))
        .await
}

/// Every `ConnectorId` `ListConnectors` reports, in the order it reports them.
async fn listed_ids(emulator: &Emulator, case: &str) -> Vec<String> {
    let answer = emulator.call("ListConnectors", json!({})).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListConnectors 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .get("Connectors")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            member_of(item, "ConnectorId")
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Assert that `answer` is this service's not-found refusal for `connector_id`: the
/// status `state::RESOURCE_NOT_FOUND_HTTP_STATUS` fixes, the `ResourceNotFoundException`
/// code, the connector resource type, the id in `Resource`, and a non-empty `Message`
/// that carries no identifier. Returns the `Message`, so a caller can check every row
/// answers the SAME fixed text.
fn assert_connector_not_found(case: &str, answer: &crate::harness::Answer, connector_id: &str) -> String {
    assert_eq!(
        answer.status, RESOURCE_NOT_FOUND_HTTP_STATUS,
        "case {case:?}: expected HTTP {RESOURCE_NOT_FOUND_HTTP_STATUS}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {}",
        answer.body
    );
    assert_members(
        case,
        &answer.body,
        &json!({"ResourceType": "Connector", "Resource": connector_id}),
    );
    let message = member_of(&answer.body, "Message")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !message.is_empty(),
        "case {case:?}: ResourceNotFoundException models Message as required, got {}",
        answer.body
    );
    assert!(
        !message.contains(connector_id),
        "case {case:?}: expected no identifier in Message, got {message:?}"
    );
    message
}

// ---------------------------------------------------------------------------
// Rule: a delete releases exactly the connector it names.
// ---------------------------------------------------------------------------

/// RULE: `DeleteConnector` answers HTTP 200 with an empty body and releases the stored
/// connector its `ConnectorId` names, so `DescribeConnector` and `ListConnectors` no
/// longer know that id, while EVERY other stored connector still reads back exactly as
/// it did before the delete.
///
/// The rows are the whole table of [`connector_rows`], deleted one at a time from a
/// store that holds all of them — so each delete is exercised at a different position
/// in `ConnectorId` order (first, middle, last) and with a different number of
/// connectors left beside it.
#[tokio::test]
async fn deleting_a_connector_releases_exactly_that_connector() {
    let emulator = Emulator::new();

    // Create every row first, and record what each one describes as, so a later read of
    // a surviving connector is compared against its OWN pre-delete description.
    let mut created: Vec<(&str, String, Value)> = Vec::new();
    for (case, request) in connector_rows() {
        let connector_id = create_connector(&emulator, case, request).await;
        let described = describe(&emulator, &connector_id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected DescribeConnector 200 before the delete, got {} with body {}",
            described.status, described.body
        );
        created.push((case, connector_id, described.get("Connector").clone()));
    }

    let mut deleted: Vec<String> = Vec::new();

    for index in 0..created.len() {
        let (case, connector_id, _) = created[index].clone();

        let answer = emulator
            .call("DeleteConnector", json!({"ConnectorId": &connector_id}))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected DeleteConnector 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body,
            json!({}),
            "case {case:?}: DeleteConnector models no output members, got {}",
            answer.body
        );

        // The deleted id is unknown to every reader of a connector.
        let described = describe(&emulator, &connector_id).await;
        assert_connector_not_found(case, &described, &connector_id);

        let transfer = emulator
            .call(
                "StartFileTransfer",
                json!({"ConnectorId": &connector_id, "RetrieveFilePaths": ["/inbox/invoice.edi"]}),
            )
            .await;
        assert_connector_not_found(case, &transfer, &connector_id);

        deleted.push(connector_id.clone());

        // The listing reports exactly the connectors that are left — compared as a
        // SET, since what the listing reports is this rule's concern and the ORDER it
        // reports them in belongs to the `connector_listing` suite.
        let mut survivors: Vec<String> = created[index + 1..]
            .iter()
            .map(|(_, id, _)| id.clone())
            .collect();
        survivors.sort();
        let mut listed = listed_ids(&emulator, case).await;
        listed.sort();
        assert_eq!(
            listed, survivors,
            "case {case:?}: expected the listing to report exactly the remaining \
             connectors {survivors:?}, got {listed:?}"
        );

        // Every surviving connector still describes exactly as it did before: a delete
        // touches no other record.
        for (other_case, other_id, before) in &created[index + 1..] {
            let again = describe(&emulator, other_id).await;
            assert_eq!(
                again.status, 200,
                "case {case:?}: expected the surviving connector {other_case:?} to still \
                 describe with 200, got {} with body {}",
                again.status, again.body
            );
            assert_eq!(
                again.get("Connector"),
                before,
                "case {case:?}: deleting {connector_id:?} changed the surviving connector \
                 {other_case:?}: expected {before:?}, got {}",
                again.body
            );
        }
    }

    // Nothing is left: the store is empty once every row has been deleted.
    let listed = listed_ids(&emulator, "every connector deleted").await;
    assert!(
        listed.is_empty(),
        "expected an empty store after deleting every connector, got {listed:?}"
    );
    assert_eq!(
        deleted.len(),
        created.len(),
        "expected one delete per created connector, got {deleted:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule: a `ConnectorId` the store does not hold is not found, and nothing changes.
// ---------------------------------------------------------------------------

/// RULE: `DeleteConnector` for a syntactically valid `ConnectorId` the store does not
/// hold answers this service's not-found error — so a delete is NOT idempotent — and
/// leaves the stored connectors exactly as they were.
///
/// The rows are ids from the low end of the id space, a mid value, the high end, one
/// that neighbours a minted id, and the id of a connector this test just deleted.
#[tokio::test]
async fn deleting_a_connector_the_store_does_not_hold_is_not_found() {
    let emulator = Emulator::new();

    let kept = create_connector(
        &emulator,
        "a connector the store keeps throughout",
        json!({
            "AccessRole": ROLE,
            "Url": "sftp://partner.example.com",
            "SftpConfig": {"UserSecretId": "sftp"},
        }),
    )
    .await;
    let doomed = create_connector(
        &emulator,
        "a connector deleted before the refusals",
        json!({
            "AccessRole": ROLE,
            "Url": "https://partner.example.com/as2",
            "As2Config": {
                "LocalProfileId": LOCAL_PROFILE,
                "PartnerProfileId": PARTNER_PROFILE,
                "Compression": "DISABLED",
                "EncryptionAlgorithm": "NONE",
                "SigningAlgorithm": "NONE",
            },
        }),
    )
    .await;
    let first_delete = emulator
        .call("DeleteConnector", json!({"ConnectorId": &doomed}))
        .await;
    assert_eq!(
        first_delete.status, 200,
        "expected the first DeleteConnector to succeed, got {} with body {}",
        first_delete.status, first_delete.body
    );

    let absent = [
        "c-00000000000000000".to_string(),
        "c-0000000000000dead".to_string(),
        "c-fffffffffffffffff".to_string(),
        format!("c-{:017x}", id_sequence(&kept) + 7),
        // The same id a second time: an already-deleted connector is as absent as one
        // that was never created.
        doomed.clone(),
    ];

    let mut messages: Vec<(String, String)> = Vec::new();

    for connector_id in &absent {
        let answer = emulator
            .call("DeleteConnector", json!({"ConnectorId": connector_id}))
            .await;
        let message = assert_connector_not_found(connector_id, &answer, connector_id);
        messages.push((connector_id.clone(), message));

        // The refusal changed nothing: the kept connector is still there.
        let described = describe(&emulator, &kept).await;
        assert_eq!(
            described.status, 200,
            "case {connector_id:?}: a refused delete must leave the stored connector \
             {kept:?} readable, got {} with body {}",
            described.status, described.body
        );
    }

    let (first_case, first) = messages
        .first()
        .expect("every row produced a message")
        .clone();
    for (case, message) in &messages {
        assert_eq!(
            message, &first,
            "case {case:?}: expected the same fixed Connector not-found message as case \
             {first_case:?} ({first:?}), got {message:?}"
        );
    }

    let listed = listed_ids(&emulator, "after the refusals").await;
    assert_eq!(
        listed,
        vec![kept.clone()],
        "expected only {kept:?} to remain stored, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule: the delete releases the store capacity the connector held.
// ---------------------------------------------------------------------------

/// RULE (the same release, observed through the store bound): a deleted connector frees
/// the capacity it held, so a store at its connector bound — where `CreateConnector`
/// refuses with the modelled `ServiceUnavailableException` — takes exactly one new
/// connector per connector deleted.
///
/// The store is seeded to [`MAX_STORED_CONNECTORS`] directly, exactly as
/// `CreateConnector` writes a service-managed SFTP connector, because reaching the bound
/// over the wire would be 200 creates. The deletes and creates themselves all go over
/// the wire, and the rows vary the number deleted before the next create (1, then 2).
#[tokio::test]
async fn a_deleted_connector_releases_the_store_capacity_it_held() {
    let emulator = Emulator::new();

    let seeded: Vec<String> = {
        let store = emulator.store();
        let mut guard = store.lock();
        let mut ids = Vec::with_capacity(MAX_STORED_CONNECTORS);
        while guard.connectors.len() < MAX_STORED_CONNECTORS {
            let connector_id = guard.mint_id("c");
            let connector = Connector {
                connector_id: connector_id.clone(),
                arn: connector_arn(REGION, &connector_id),
                url: Some("sftp://partner.example.com".to_string()),
                as2_config: None,
                sftp_config: Some(SftpConnectorConfig {
                    user_secret_id: Some("sftp-partner-credentials".to_string()),
                    trusted_host_keys: Some(Vec::new()),
                    max_concurrent_connections: DEFAULT_MAX_CONCURRENT_CONNECTIONS,
                    ordered_user_secret_version_stages: Some(vec![
                        DEFAULT_SECRET_VERSION_STAGE.to_string()
                    ]),
                }),
                access_role: ROLE.to_string(),
                logging_role: None,
                security_policy_name: Some(DEFAULT_CONNECTOR_SECURITY_POLICY_NAME.to_string()),
                service_managed_egress_ip_addresses: service_managed_egress_ip_addresses(
                    REGION,
                    DEFAULT_CONNECTOR_EGRESS_TYPE,
                ),
                egress_config: None,
                egress_type: DEFAULT_CONNECTOR_EGRESS_TYPE.to_string(),
                ip_address_type: DEFAULT_IP_ADDRESS_TYPE.to_string(),
                status: CONNECTOR_STATUS_ACTIVE.to_string(),
                tags: Vec::new(),
            };
            guard.connectors.insert(connector_id.clone(), connector);
            ids.push(connector_id);
        }
        ids
    };

    let create_request = json!({
        "AccessRole": ROLE,
        "Url": "sftp://partner.example.com",
        "SftpConfig": {"UserSecretId": "sftp"},
    });

    // At the bound, the create is refused.
    let refused = emulator.call("CreateConnector", create_request.clone()).await;
    assert_eq!(
        refused.status, 503,
        "case {:?}: expected the create at the store bound to be refused with HTTP 503, \
         got {} with body {}",
        "at capacity", refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ServiceUnavailableException",
        "case {:?}: expected ServiceUnavailableException, got {}",
        "at capacity",
        refused.body
    );

    // Rows: delete one connector, then two — each delete releases exactly one slot, so
    // the store takes exactly as many new connectors as were deleted.
    let mut next_seeded = seeded.iter();
    for release_count in [1usize, 2] {
        let case = format!("{release_count} connector(s) deleted");
        for _ in 0..release_count {
            let victim = next_seeded.next().expect("the seeded store has enough connectors");
            let answer = emulator
                .call("DeleteConnector", json!({"ConnectorId": victim}))
                .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected DeleteConnector 200 for {victim:?}, got {} with body {}",
                answer.status, answer.body
            );
        }

        for created in 0..release_count {
            let answer = emulator.call("CreateConnector", create_request.clone()).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected create {} after the deletes to succeed, got {} \
                 with body {}",
                created + 1,
                answer.status,
                answer.body
            );
        }

        // One past the released capacity, the store is full again.
        let refused = emulator.call("CreateConnector", create_request.clone()).await;
        assert_eq!(
            refused.status, 503,
            "case {case:?}: expected the store to be at its bound again, got {} with body {}",
            refused.status, refused.body
        );
    }
}
