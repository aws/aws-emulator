// SPDX-License-Identifier: CC0-1.0

//! What `CreateConnector` persists, and what a described connector reports.
//!
//! Six rules, each asserted over a TABLE of inputs rather than one recorded call:
//!
//! 1. a created connector is STORED: the call mints a `ConnectorId` of the modelled
//!    shape, and a later `DescribeConnector` reports every member the caller supplied
//!    exactly as supplied plus every member the service resolves — the ARN, the id, the
//!    egress type, the status and the nested documented defaults
//!    ([`created_connectors_are_described_exactly_as_configured`]);
//! 2. every member the SERVICE settles for itself is resolved at create time and
//!    reported by every read — the address type, the security policy, the egress
//!    addresses it assigned, the trusted-host-key set and the secret version-stage order
//!    ([`a_connector_reports_the_members_the_service_settles`], and
//!    [`service_managed_egress_addresses_are_reported_for_every_region`] along the signed
//!    region);
//! 3. a connector whose members cannot hold together is refused with the modelled
//!    `InvalidRequestException` and NOTHING is persisted for it — the next create mints
//!    the very next id ([`connectors_whose_members_cannot_hold_together_are_refused`]);
//! 4. a `ConnectorId` the store does not hold answers this service's generic not-found
//!    error ([`describing_a_connector_that_was_never_created_is_not_found`]);
//! 5. each `As2Config` member the service fills no default for is required ON ITS OWN,
//!    and the refusal names the one that is missing
//!    ([`each_required_as2_config_member_is_required_on_its_own`]);
//! 6. a `Url` that names no endpoint earns the sentence for ITS OWN condition — nothing
//!    but path information, or a host that could never be resolved
//!    ([`a_url_that_names_no_endpoint_earns_the_sentence_for_its_own_condition`]).
//!
//! The rows differ along the protocol configuration (AS2 or SFTP), the egress type
//! (service-managed or VPC Lattice), how many of the nested members the request
//! supplies, which enum value each carries, the list lengths, and where a numeric
//! member sits relative to its published bounds. The harness (router, request signing,
//! `assert_members`) is the shared [`crate::harness`].

use serde_json::{json, Value};

use crate::harness::{
    assert_members, is_modelled_connector_id, member_of, Emulator, ACCOUNT, REGION, ROLE,
};

/// A `VpcLatticeResourceConfigurationArn` satisfying the member's modelled pattern
/// `^arn:[a-z0-9\-]+:vpc-lattice:[a-zA-Z0-9\-]+:\d{12}:resourceconfiguration/rcfg-[0-9a-z]{17}$`.
const RESOURCE_CONFIGURATION_ARN: &str =
    "arn:aws:vpc-lattice:us-east-1:000000000000:resourceconfiguration/rcfg-0123456789abcdef0";

/// A `ProfileId` (`len 19..19`, pattern `^p-([0-9a-f]{17})$`) for the local AS2 profile.
const LOCAL_PROFILE: &str = "p-0123456789abcdef0";

/// A second `ProfileId`, for the partner side of the same AS2 connector.
const PARTNER_PROFILE: &str = "p-00000000000000001";

/// The 17 hex digits of a minted id, as a number — so a test can assert that a refused
/// create minted nothing.
fn id_sequence(connector_id: &str) -> u64 {
    u64::from_str_radix(&connector_id[2..], 16).expect("a minted id ends in 17 hex digits")
}

/// Create a connector, assert the call succeeded, and return its `ConnectorId`.
///
/// Kept HERE rather than on the shared [`Emulator`] (beside `create_server`) because it
/// carries the TABLE ROW's label into every assertion message, which only this suite
/// has.
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
        .unwrap_or_else(|| panic!("case {case:?}: CreateConnector returned no ConnectorId: {}", answer.body))
        .to_string()
}

/// Describe `connector_id`, assert the read succeeded, and return the `Connector`.
async fn describe_connector(emulator: &Emulator, case: &str, connector_id: &str) -> Value {
    let answer = emulator
        .call("DescribeConnector", json!({"ConnectorId": connector_id}))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected DescribeConnector 200, got {} with body {}",
        answer.status, answer.body
    );
    answer.get("Connector").clone()
}

// ---------------------------------------------------------------------------
// Rule 1: a created connector is stored, and described as configured.
// ---------------------------------------------------------------------------

/// RULE: `CreateConnector` persists the connector it is given and answers a minted
/// `ConnectorId`; `DescribeConnector` then reports every supplied member exactly as
/// supplied and every member the service resolves — `Arn`, `ConnectorId`, `EgressType`
/// (`VPC_LATTICE` with an `EgressConfig`, else the modelled default `SERVICE_MANAGED`),
/// `Status` `ACTIVE`, the tag set as a list, and each nested member at the default its
/// own model doc states.
///
/// The rows differ along the protocol configuration, the egress type, how many nested
/// members the request supplies, which enum value each carries, the list lengths, and
/// whether `MaxConcurrentConnections` / `PortNumber` are absent, at a published bound,
/// or in between.
#[tokio::test]
async fn created_connectors_are_described_exactly_as_configured() {
    let emulator = Emulator::new();

    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "AS2, nothing beyond the members an AS2 configuration is required to settle",
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
                "EgressType": "SERVICE_MANAGED",
                "Status": "ACTIVE",
                "Tags": [],
                "SftpConfig": null,
                "EgressConfig": null,
                "LoggingRole": null,
                // `ConnectorSecurityPolicyName`'s modelled pattern
                // `^TransferSFTPConnectorSecurityPolicy-[A-Za-z0-9-]+$` admits only the
                // SFTP-connector policies, so an AS2 connector carries none of that kind.
                "SecurityPolicyName": null,
                // Every connector's network connections have an address type, so one
                // the request did not name takes the documented default.
                "IpAddressType": "IPV4",
                "ErrorMessage": null,
            }),
        ),
        (
            "AS2, every nested member supplied at a non-default enum value",
            json!({
                "AccessRole": ROLE,
                "LoggingRole": "arn:aws:iam::000000000000:role/transfer-logging",
                "Url": "http://partner.example.com:8080/as2",
                "IpAddressType": "DUALSTACK",
                "As2Config": {
                    "LocalProfileId": LOCAL_PROFILE,
                    "PartnerProfileId": PARTNER_PROFILE,
                    "MessageSubject": "invoices for 2026-10",
                    "Compression": "ZLIB",
                    "EncryptionAlgorithm": "AES256_CBC",
                    "SigningAlgorithm": "SHA384",
                    "MdnSigningAlgorithm": "DEFAULT",
                    "MdnResponse": "SYNC",
                    "BasicAuthSecretId": "arn:aws:secretsmanager:us-east-1:000000000000:secret:as2-basic",
                    "PreserveContentType": "ENABLED",
                },
                "Tags": [
                    {"Key": "team", "Value": "edi"},
                    {"Key": "env", "Value": "test"},
                ],
            }),
            json!({
                "AccessRole": ROLE,
                "LoggingRole": "arn:aws:iam::000000000000:role/transfer-logging",
                "Url": "http://partner.example.com:8080/as2",
                "IpAddressType": "DUALSTACK",
                "As2Config": {
                    "LocalProfileId": LOCAL_PROFILE,
                    "PartnerProfileId": PARTNER_PROFILE,
                    "MessageSubject": "invoices for 2026-10",
                    "Compression": "ZLIB",
                    "EncryptionAlgorithm": "AES256_CBC",
                    "SigningAlgorithm": "SHA384",
                    "MdnSigningAlgorithm": "DEFAULT",
                    "MdnResponse": "SYNC",
                    "BasicAuthSecretId": "arn:aws:secretsmanager:us-east-1:000000000000:secret:as2-basic",
                    "PreserveContentType": "ENABLED",
                },
                "EgressType": "SERVICE_MANAGED",
                "Status": "ACTIVE",
                "Tags": [
                    {"Key": "team", "Value": "edi"},
                    {"Key": "env", "Value": "test"},
                ],
                "SftpConfig": null,
                "EgressConfig": null,
            }),
        ),
        (
            "SFTP service-managed, MaxConcurrentConnections left to its documented default of 1",
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com",
                "SftpConfig": {"UserSecretId": "sftp-partner-credentials"},
            }),
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com",
                "SftpConfig": {
                    "UserSecretId": "sftp-partner-credentials",
                    // The set of trusted keys the connector carries: EMPTY, because the
                    // request named none, and the order the credentials are read in at
                    // its default single stage.
                    "TrustedHostKeys": [],
                    "MaxConcurrentConnections": 1,
                    "OrderedUserSecretVersionStages": ["AWSCURRENT"],
                },
                "EgressType": "SERVICE_MANAGED",
                "Status": "ACTIVE",
                "As2Config": null,
                "EgressConfig": null,
                "Tags": [],
                // The default SFTP-connector policy, which the published catalog holds.
                "SecurityPolicyName": "TransferSFTPConnectorSecurityPolicy-2024-03",
                "IpAddressType": "IPV4",
            }),
        ),
        (
            "SFTP service-managed at the published MaxConcurrentConnections maximum, with \
             two trusted host keys and a security policy",
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com:2222",
                "SecurityPolicyName": "TransferSFTPConnectorSecurityPolicy-2024-03",
                "SftpConfig": {
                    "UserSecretId": "arn:aws:secretsmanager:us-east-1:000000000000:secret:sftp",
                    "MaxConcurrentConnections": 5,
                    "TrustedHostKeys": [
                        "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ==",
                        "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTY=",
                    ],
                    "OrderedUserSecretVersionStages": ["AWSCURRENT", "AWSPREVIOUS"],
                },
                "IpAddressType": "IPV4",
            }),
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com:2222",
                "SecurityPolicyName": "TransferSFTPConnectorSecurityPolicy-2024-03",
                "SftpConfig": {
                    "UserSecretId": "arn:aws:secretsmanager:us-east-1:000000000000:secret:sftp",
                    "MaxConcurrentConnections": 5,
                    "TrustedHostKeys": [
                        "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ==",
                        "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTY=",
                    ],
                    "OrderedUserSecretVersionStages": ["AWSCURRENT", "AWSPREVIOUS"],
                },
                "EgressType": "SERVICE_MANAGED",
                "Status": "ACTIVE",
                "IpAddressType": "IPV4",
            }),
        ),
        (
            "SFTP over VPC Lattice, no port: PortNumber takes its documented default of 22 \
             and Url stays null",
            json!({
                "AccessRole": ROLE,
                "SftpConfig": {"UserSecretId": "sftp-partner-credentials"},
                "EgressConfig": {
                    "VpcLattice": {"ResourceConfigurationArn": RESOURCE_CONFIGURATION_ARN},
                },
            }),
            json!({
                "AccessRole": ROLE,
                "Url": null,
                "EgressType": "VPC_LATTICE",
                "Status": "ACTIVE",
                "EgressConfig": {
                    "VpcLattice": {
                        "ResourceConfigurationArn": RESOURCE_CONFIGURATION_ARN,
                        "PortNumber": 22,
                    },
                },
                "SftpConfig": {
                    "UserSecretId": "sftp-partner-credentials",
                    "TrustedHostKeys": [],
                    "MaxConcurrentConnections": 1,
                    "OrderedUserSecretVersionStages": ["AWSCURRENT"],
                },
            }),
        ),
        (
            "SFTP over VPC Lattice at the low end of PortNumber's modelled range (1)",
            json!({
                "AccessRole": ROLE,
                "SftpConfig": {"UserSecretId": "sftp", "MaxConcurrentConnections": 3},
                "EgressConfig": {
                    "VpcLattice": {
                        "ResourceConfigurationArn": RESOURCE_CONFIGURATION_ARN,
                        "PortNumber": 1,
                    },
                },
            }),
            json!({
                "EgressType": "VPC_LATTICE",
                "EgressConfig": {
                    "VpcLattice": {
                        "ResourceConfigurationArn": RESOURCE_CONFIGURATION_ARN,
                        "PortNumber": 1,
                    },
                },
                "SftpConfig": {
                    "UserSecretId": "sftp",
                    "TrustedHostKeys": [],
                    "MaxConcurrentConnections": 3,
                    "OrderedUserSecretVersionStages": ["AWSCURRENT"],
                },
                "Url": null,
            }),
        ),
        (
            "SFTP over VPC Lattice at the high end of PortNumber's modelled range (65535), \
             with a blank Url that names no endpoint",
            json!({
                "AccessRole": ROLE,
                "Url": "",
                "SftpConfig": {"UserSecretId": "sftp", "TrustedHostKeys": []},
                "EgressConfig": {
                    "VpcLattice": {
                        "ResourceConfigurationArn": RESOURCE_CONFIGURATION_ARN,
                        "PortNumber": 65535,
                    },
                },
            }),
            json!({
                "EgressType": "VPC_LATTICE",
                // A blank Url names no endpoint, so the connector reports none rather
                // than the empty string the model says must be null.
                "Url": null,
                "EgressConfig": {
                    "VpcLattice": {
                        "ResourceConfigurationArn": RESOURCE_CONFIGURATION_ARN,
                        "PortNumber": 65535,
                    },
                },
                "SftpConfig": {
                    "UserSecretId": "sftp",
                    "TrustedHostKeys": [],
                    "MaxConcurrentConnections": 1,
                    "OrderedUserSecretVersionStages": ["AWSCURRENT"],
                },
            }),
        ),
    ];

    let mut minted: Vec<String> = Vec::new();

    for (case, request, expected) in cases {
        let connector_id = create_connector(&emulator, case, request).await;
        assert!(
            is_modelled_connector_id(&connector_id),
            "case {case:?}: expected a ConnectorId matching ^c-([0-9a-f]{{17}})$, \
             got {connector_id:?}"
        );

        let described = describe_connector(&emulator, case, &connector_id).await;
        assert_members(case, &described, &expected);

        let arn = member_of(&described, "Arn").as_str().unwrap_or_default().to_string();
        assert_eq!(
            arn,
            format!("arn:aws:transfer:{REGION}:{ACCOUNT}:connector/{connector_id}"),
            "case {case:?}: expected the connector's own ARN, got {arn:?}"
        );
        let reported_id = member_of(&described, "ConnectorId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            reported_id, connector_id,
            "case {case:?}: expected the described connector to report the minted id \
             {connector_id:?}, got {reported_id:?}"
        );

        // A read must not change what a later read sees.
        let again = describe_connector(&emulator, case, &connector_id).await;
        assert_eq!(
            again, described,
            "case {case:?}: two reads of one connector must answer the same thing, \
             got {again:?} after {described:?}"
        );

        minted.push(connector_id);
    }

    // Every create minted its OWN connector, and the earlier ones are all still
    // readable: a create never overwrites a stored connector.
    let mut unique = minted.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        minted.len(),
        "expected one distinct ConnectorId per create, got {minted:?}"
    );
    for connector_id in &minted {
        let described = describe_connector(&emulator, "all connectors still readable", connector_id).await;
        let reported_id = member_of(&described, "ConnectorId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            &reported_id, connector_id,
            "expected {connector_id:?} to still be stored, got {described:?}"
        );
    }
}

/// RULE (same rule, varied along the signed region): the ARN a connector reports is
/// built from the region the caller signed for, so it is not fixed to one region.
#[tokio::test]
async fn a_connectors_arn_carries_the_region_the_request_was_signed_for() {
    let emulator = Emulator::new();

    for region in ["us-east-1", "eu-west-2", "ap-southeast-1"] {
        let answer = emulator
            .call_in(
                region,
                "CreateConnector",
                json!({
                    "AccessRole": ROLE,
                    "Url": "sftp://partner.example.com",
                    "SftpConfig": {"UserSecretId": "sftp"},
                }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {region:?}: expected CreateConnector 200, got {} with body {}",
            answer.status, answer.body
        );
        let connector_id = answer.get("ConnectorId").as_str().unwrap_or_default().to_string();

        let described = describe_connector(&emulator, region, &connector_id).await;
        let arn = member_of(&described, "Arn").as_str().unwrap_or_default().to_string();
        assert_eq!(
            arn,
            format!("arn:aws:transfer:{region}:{ACCOUNT}:connector/{connector_id}"),
            "case {region:?}: expected the ARN to carry the signed region, got {arn:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 1, stated over the members the SERVICE settles rather than the caller.
// ---------------------------------------------------------------------------

/// What one row of [`a_connector_reports_the_members_the_service_settles`] expects of
/// the members the service settles for itself.
struct Settled {
    /// `IpAddressType`, which every connector carries.
    ip_address_type: &'static str,
    /// `SecurityPolicyName`, or `None` for a connector that carries no policy of the
    /// SFTP-connector kind the member's pattern admits.
    security_policy_name: Option<&'static str>,
    /// Whether the service manages this connector's egress, and so assigns it addresses.
    service_managed_egress: bool,
    /// `SftpConfig.TrustedHostKeys`, or `None` for a connector with no `SftpConfig`.
    trusted_host_keys: Option<Vec<&'static str>>,
    /// `SftpConfig.OrderedUserSecretVersionStages`, likewise.
    ordered_user_secret_version_stages: Option<Vec<&'static str>>,
}

/// Every address of `addresses` parsed as an IPv4 address, so a row asserts the member's
/// FORM rather than one run's literals.
///
/// `ServiceManagedEgressIpAddress` is modelled with the pattern
/// `^\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}$`
/// (`smithy_query(query="shape",shape="ServiceManagedEgressIpAddresses")`), which is a
/// dotted quad of octets.
fn assert_ipv4_addresses(case: &str, addresses: &Value) {
    let listed = addresses
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a list of addresses, got {addresses:?}"));
    assert!(
        !listed.is_empty(),
        "case {case:?}: expected the service-managed egress addresses the connector was \
         assigned, got an empty list"
    );
    for address in listed {
        let text = address.as_str().unwrap_or_default();
        assert!(
            text.parse::<std::net::Ipv4Addr>().is_ok(),
            "case {case:?}: expected an IPv4 address of four octets, got {address:?}"
        );
    }
}

/// RULE: every member of a connector that the SERVICE settles — rather than the caller
/// supplying it — is resolved when the connector is created and reported by every read
/// of it: `IpAddressType` at the caller's value else the documented default,
/// `SecurityPolicyName` at the caller's value else the service's default SFTP-connector
/// policy (and absent only for a connector whose protocol carries no policy of that
/// kind), `ServiceManagedEgressIpAddresses` as the non-empty set of IPv4 addresses
/// assigned to a connector whose egress the service manages (absent for one it does
/// not), `SftpConfig.TrustedHostKeys` as the — possibly empty — set of keys the
/// connector trusts, and `SftpConfig.OrderedUserSecretVersionStages` as the order the
/// credentials are read in, at its default single stage when the caller named no order.
///
/// The rows differ along the protocol configuration (AS2, SFTP), the egress type
/// (service-managed, VPC Lattice), how many of those members the request supplies (none,
/// one, all), which enum value each carries, and the list lengths (`TrustedHostKeys` at
/// 0, 1 and 2 of its modelled `len 0..10`; `OrderedUserSecretVersionStages` at both
/// bounds of its modelled `len 1..2`).
///
/// Each row is also read TWICE, and once more after an update that names none of these
/// members, which is what proves the set is assigned once and survives a read-modify-write
/// round trip rather than being re-derived per read.
#[tokio::test]
async fn a_connector_reports_the_members_the_service_settles() {
    let emulator = Emulator::new();

    // `ConnectorsIpAddressType` is `[IPV4, DUALSTACK]` and the published SFTP-connector
    // policies are the `TransferSFTPConnectorSecurityPolicy-*` generations
    // (`smithy_query(query="shape",shape="ConnectorsIpAddressType")`, and this service's
    // `ListSecurityPolicies`), so the rows below name values from those sets only.
    let cases: Vec<(&str, Value, Settled)> = vec![
        (
            "SFTP service-managed, nothing the service settles supplied",
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com",
                "SftpConfig": {"UserSecretId": "sftp-partner-credentials"},
            }),
            Settled {
                ip_address_type: "IPV4",
                security_policy_name: Some("TransferSFTPConnectorSecurityPolicy-2024-03"),
                service_managed_egress: true,
                trusted_host_keys: Some(Vec::new()),
                ordered_user_secret_version_stages: Some(vec!["AWSCURRENT"]),
            },
        ),
        (
            "SFTP service-managed, one trusted host key and the other IpAddressType value",
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com:2222",
                "IpAddressType": "DUALSTACK",
                "SftpConfig": {
                    "UserSecretId": "sftp-partner-credentials",
                    "TrustedHostKeys": ["ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ=="],
                },
            }),
            Settled {
                ip_address_type: "DUALSTACK",
                security_policy_name: Some("TransferSFTPConnectorSecurityPolicy-2024-03"),
                service_managed_egress: true,
                trusted_host_keys: Some(vec!["ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ=="]),
                ordered_user_secret_version_stages: Some(vec!["AWSCURRENT"]),
            },
        ),
        (
            "SFTP service-managed, every settled member supplied: a FIPS policy from the \
             published catalog, two trusted host keys and both secret version stages",
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com",
                "IpAddressType": "IPV4",
                "SecurityPolicyName": "TransferSFTPConnectorSecurityPolicy-FIPS-2024-10",
                "SftpConfig": {
                    "UserSecretId": "sftp-partner-credentials",
                    "TrustedHostKeys": [
                        "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ==",
                        "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTY=",
                    ],
                    "OrderedUserSecretVersionStages": ["AWSPREVIOUS", "AWSCURRENT"],
                },
            }),
            Settled {
                ip_address_type: "IPV4",
                security_policy_name: Some("TransferSFTPConnectorSecurityPolicy-FIPS-2024-10"),
                service_managed_egress: true,
                trusted_host_keys: Some(vec![
                    "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ==",
                    "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTY=",
                ]),
                // Reported in the caller's own ORDER, which is what the member is for.
                ordered_user_secret_version_stages: Some(vec!["AWSPREVIOUS", "AWSCURRENT"]),
            },
        ),
        (
            "SFTP over VPC Lattice: the customer's own VPC carries the egress, so the \
             service assigns no addresses for it",
            json!({
                "AccessRole": ROLE,
                "SftpConfig": {"UserSecretId": "sftp-partner-credentials"},
                "EgressConfig": {
                    "VpcLattice": {"ResourceConfigurationArn": RESOURCE_CONFIGURATION_ARN},
                },
            }),
            Settled {
                ip_address_type: "IPV4",
                security_policy_name: Some("TransferSFTPConnectorSecurityPolicy-2024-03"),
                service_managed_egress: false,
                trusted_host_keys: Some(Vec::new()),
                ordered_user_secret_version_stages: Some(vec!["AWSCURRENT"]),
            },
        ),
        (
            "AS2 service-managed: no SftpConfig, and no policy of the SFTP-connector kind",
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
            Settled {
                ip_address_type: "IPV4",
                security_policy_name: None,
                service_managed_egress: true,
                trusted_host_keys: None,
                ordered_user_secret_version_stages: None,
            },
        ),
    ];

    // The addresses every service-managed connector of this Region reports: assigned to
    // the account's managed egress rather than to one connector, so they are collected
    // here and compared across the rows.
    let mut regional_egress: Option<Value> = None;

    for (case, request, settled) in cases {
        let connector_id = create_connector(&emulator, case, request).await;
        let described = describe_connector(&emulator, case, &connector_id).await;

        assert_members(
            case,
            &described,
            &json!({
                "IpAddressType": settled.ip_address_type,
                "SecurityPolicyName": settled.security_policy_name,
            }),
        );

        let egress = member_of(&described, "ServiceManagedEgressIpAddresses").clone();
        if settled.service_managed_egress {
            assert_ipv4_addresses(case, &egress);
            if let Some(regional) = regional_egress.as_ref() {
                assert_eq!(
                    &egress, regional,
                    "case {case:?}: the service's managed egress belongs to the account \
                     and Region, so every service-managed connector of this Region \
                     reports the same addresses, expected {regional:?}, got {egress:?}"
                );
            } else {
                regional_egress = Some(egress.clone());
            }
        } else {
            assert!(
                egress.is_null(),
                "case {case:?}: the service manages no egress for this connector, so it \
                 reports no addresses, got {egress:?}"
            );
        }

        let sftp_config = member_of(&described, "SftpConfig").clone();
        match settled.trusted_host_keys {
            Some(keys) => assert_eq!(
                member_of(&sftp_config, "TrustedHostKeys"),
                &json!(keys),
                "case {case:?}: expected TrustedHostKeys {keys:?}, got {sftp_config:?}"
            ),
            None => assert!(
                sftp_config.is_null(),
                "case {case:?}: a connector with no SftpConfig reports none, got \
                 {sftp_config:?}"
            ),
        }
        if let Some(stages) = settled.ordered_user_secret_version_stages {
            assert_eq!(
                member_of(&sftp_config, "OrderedUserSecretVersionStages"),
                &json!(stages),
                "case {case:?}: expected OrderedUserSecretVersionStages {stages:?}, got \
                 {sftp_config:?}"
            );
        }

        // The policy the connector reports is one the service PUBLISHES, so a caller can
        // read the ciphers it governs: an invented name resolves to nothing.
        if let Some(name) = settled.security_policy_name {
            let policy = emulator
                .call("DescribeSecurityPolicy", json!({"SecurityPolicyName": name}))
                .await;
            assert_eq!(
                policy.status, 200,
                "case {case:?}: expected the reported policy {name:?} to be published, \
                 got {} with body {}",
                policy.status, policy.body
            );
        }

        // Assigned once: a second read reports the same connector, and so does a read
        // after an update that names none of these members — the read-modify-write round
        // trip a caller makes.
        let again = describe_connector(&emulator, case, &connector_id).await;
        assert_eq!(
            again, described,
            "case {case:?}: two reads of one connector must answer the same thing, got \
             {again:?} after {described:?}"
        );
        let update = emulator
            .call(
                "UpdateConnector",
                json!({"ConnectorId": &connector_id, "AccessRole": ROLE}),
            )
            .await;
        assert_eq!(
            update.status, 200,
            "case {case:?}: expected UpdateConnector 200, got {} with body {}",
            update.status, update.body
        );
        let after_update = describe_connector(&emulator, case, &connector_id).await;
        assert_eq!(
            after_update, described,
            "case {case:?}: an update that names none of the settled members must leave \
             them all, got {after_update:?} after {described:?}"
        );
    }
}

/// RULE (the same one, varied along the signed region): a connector's service-managed
/// egress addresses are the ones its own Region's managed egress uses, so a connector
/// created in another Region reports that Region's set.
#[tokio::test]
async fn service_managed_egress_addresses_are_reported_for_every_region() {
    let emulator = Emulator::new();

    for region in ["us-east-1", "eu-west-2", "ap-southeast-1"] {
        let answer = emulator
            .call_in(
                region,
                "CreateConnector",
                json!({
                    "AccessRole": ROLE,
                    "Url": "sftp://partner.example.com",
                    "SftpConfig": {"UserSecretId": "sftp"},
                }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {region:?}: expected CreateConnector 200, got {} with body {}",
            answer.status, answer.body
        );
        let connector_id = answer.get("ConnectorId").as_str().unwrap_or_default().to_string();

        let described = describe_connector(&emulator, region, &connector_id).await;
        assert_ipv4_addresses(
            region,
            member_of(&described, "ServiceManagedEgressIpAddresses"),
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a configuration that cannot hold together is refused, and stores nothing.
// ---------------------------------------------------------------------------

/// RULE: a `CreateConnector` whose members violate a requirement the model states
/// across them is refused with HTTP 400 `InvalidRequestException` and persists nothing
/// — proved by the id counter, which only a stored connector moves.
///
/// The rows cover each documented requirement, from both sides where it has two, and
/// each numeric row sits one step past a published bound.
#[tokio::test]
async fn connectors_whose_members_cannot_hold_together_are_refused() {
    let emulator = Emulator::new();

    let before = create_connector(
        &emulator,
        "a connector created before the refusals",
        json!({
            "AccessRole": ROLE,
            "Url": "sftp://partner.example.com",
            "SftpConfig": {"UserSecretId": "sftp"},
        }),
    )
    .await;

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "neither As2Config nor SftpConfig: no protocol configured",
            json!({"AccessRole": ROLE, "Url": "sftp://partner.example.com"}),
            "As2Config",
        ),
        (
            "both As2Config and SftpConfig: two protocols configured",
            json!({
                "AccessRole": ROLE,
                "Url": "https://partner.example.com/as2",
                "As2Config": {"LocalProfileId": LOCAL_PROFILE},
                "SftpConfig": {"UserSecretId": "sftp"},
            }),
            "SftpConfig",
        ),
        (
            "AS2 without a Url",
            json!({
                "AccessRole": ROLE,
                "As2Config": {"LocalProfileId": LOCAL_PROFILE},
            }),
            "Url",
        ),
        (
            "service-managed SFTP without a Url",
            json!({"AccessRole": ROLE, "SftpConfig": {"UserSecretId": "sftp"}}),
            "Url",
        ),
        (
            "service-managed SFTP whose Url is the empty string: Url's len 0..255 admits \
             it, but it names no endpoint",
            json!({"AccessRole": ROLE, "Url": "", "SftpConfig": {"UserSecretId": "sftp"}}),
            "Url",
        ),
        (
            "VPC Lattice with a Url, which the model says must be null",
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com",
                "SftpConfig": {"UserSecretId": "sftp"},
                "EgressConfig": {
                    "VpcLattice": {"ResourceConfigurationArn": RESOURCE_CONFIGURATION_ARN},
                },
            }),
            "Url",
        ),
        (
            "an AS2 connector with an EgressConfig, which routes to an SFTP server",
            json!({
                "AccessRole": ROLE,
                "As2Config": {"LocalProfileId": LOCAL_PROFILE},
                "EgressConfig": {
                    "VpcLattice": {"ResourceConfigurationArn": RESOURCE_CONFIGURATION_ARN},
                },
            }),
            "EgressConfig",
        ),
        (
            "MaxConcurrentConnections one past the published maximum of 5",
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com",
                "SftpConfig": {"UserSecretId": "sftp", "MaxConcurrentConnections": 6},
            }),
            "MaxConcurrentConnections",
        ),
    ];

    for (case, request, member_at_fault) in cases {
        let answer = emulator.call("CreateConnector", request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {}",
            answer.body
        );
        assert!(
            answer.get("ConnectorId").is_null(),
            "case {case:?}: a refused create must answer no ConnectorId, got {}",
            answer.body
        );
        let message = member_of(&answer.body, "Message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !message.is_empty(),
            "case {case:?}: InvalidRequestException models Message as required, got {}",
            answer.body
        );
        // The refusal NAMES the member at fault, so a caller can fix the request from
        // the message alone.
        assert!(
            message.contains(member_at_fault),
            "case {case:?}: expected the message to name {member_at_fault:?}, \
             got {message:?}"
        );
    }

    // Nothing was persisted for any refused create: the next create mints the id
    // immediately after the one minted before them.
    let after = create_connector(
        &emulator,
        "a connector created after the refusals",
        json!({
            "AccessRole": ROLE,
            "Url": "sftp://partner.example.com",
            "SftpConfig": {"UserSecretId": "sftp"},
        }),
    )
    .await;
    assert_eq!(
        id_sequence(&after),
        id_sequence(&before) + 1,
        "expected the refused creates to persist nothing, but the id moved from \
         {before:?} to {after:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: a connector the store does not hold.
// ---------------------------------------------------------------------------

/// RULE: `DescribeConnector` for a syntactically valid `ConnectorId` the store does not
/// hold answers this service's generic not-found error — HTTP 400
/// `ResourceNotFoundException`, `ResourceType` `Connector`, `Resource` the id the
/// request named, and a fixed `Message` carrying no identifier.
///
/// The rows are ids from the low end of the id space, a mid value, the high end, and
/// one that neighbours an id that WAS minted.
#[tokio::test]
async fn describing_a_connector_that_was_never_created_is_not_found() {
    let emulator = Emulator::new();

    let minted = create_connector(
        &emulator,
        "one connector the store does hold",
        json!({
            "AccessRole": ROLE,
            "Url": "sftp://partner.example.com",
            "SftpConfig": {"UserSecretId": "sftp"},
        }),
    )
    .await;
    let neighbour = format!("c-{:017x}", id_sequence(&minted) + 1);

    let absent = [
        "c-00000000000000000".to_string(),
        "c-0000000000000dead".to_string(),
        "c-fffffffffffffffff".to_string(),
        neighbour,
    ];

    let mut messages: Vec<(String, String)> = Vec::new();

    for connector_id in &absent {
        assert!(
            is_modelled_connector_id(connector_id),
            "the probe id {connector_id:?} must satisfy ConnectorId's modelled pattern"
        );
        let answer = emulator
            .call("DescribeConnector", json!({"ConnectorId": connector_id}))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {connector_id:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {connector_id:?}: expected ResourceNotFoundException, got {}",
            answer.body
        );
        assert_members(
            connector_id,
            &answer.body,
            &json!({"ResourceType": "Connector", "Resource": connector_id}),
        );
        let message = member_of(&answer.body, "Message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !message.contains(connector_id.as_str()),
            "case {connector_id:?}: expected no identifier in Message, got {message:?}"
        );
        messages.push((connector_id.clone(), message));
    }

    let (first_case, first) = messages.first().expect("every row produced a message").clone();
    for (case, message) in &messages {
        assert_eq!(
            message, &first,
            "case {case:?}: expected the same fixed Connector message as case \
             {first_case:?} ({first:?}), got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: each As2Config member the service fills no default for is required
// ---------------------------------------------------------------------------

/// The `As2Config` members an AS2 connector cannot be written without, in the order the
/// model declares them.
///
/// The catalogue is the three members that settle how an AS2 MESSAGE IS BUILT and that
/// the model documents no substitute value for, per
/// `smithy_query(query="shape",shape="As2ConnectorConfig")`: `Compression` ("specifies
/// whether the AS2 file is compressed"), `EncryptionAlgorithm` ("the algorithm that is
/// used to encrypt the file") and `SigningAlgorithm` ("the algorithm that is used to sign
/// the AS2 messages sent with the connector"). The two enum members the service DOES
/// settle are deliberately absent: `MdnSigningAlgorithm` documents "if set to DEFAULT (or
/// not set at all), the value for `SigningAlgorithm` is used", and `PreserveContentType`
/// is "disabled by default when you create an AS2 connector by calling the API directly".
const REQUIRED_AS2_MEMBERS: [&str; 3] = ["Compression", "EncryptionAlgorithm", "SigningAlgorithm"];

/// The `As2Config` members the rows below prove are NOT required, because the model
/// documents the reading an omitted one takes — the same source as
/// [`REQUIRED_AS2_MEMBERS`].
const DEFAULTED_AS2_MEMBERS: [&str; 2] = ["MdnSigningAlgorithm", "PreserveContentType"];

/// Complete AS2 configurations, which the rows below each remove ONE member from.
///
/// They differ along which value every required member carries (each at a different
/// value of its own enum, including the weakest algorithms the model documents) and how
/// many members surround them — none, the ones the service defaults, and the nested
/// asynchronous-MDN structure.
fn complete_as2_configurations() -> Vec<(&'static str, Value)> {
    vec![
        (
            "the settings that ask for no compression, encryption or signing, and nothing \
             else at all",
            json!({
                "LocalProfileId": LOCAL_PROFILE,
                "PartnerProfileId": PARTNER_PROFILE,
                "Compression": "DISABLED",
                "EncryptionAlgorithm": "NONE",
                "SigningAlgorithm": "NONE",
            }),
        ),
        (
            "every required member at another value of its enum, beside the members the \
             service defaults",
            json!({
                "LocalProfileId": LOCAL_PROFILE,
                "PartnerProfileId": PARTNER_PROFILE,
                "MessageSubject": "invoices for the period",
                "Compression": "ZLIB",
                "EncryptionAlgorithm": "AES256_CBC",
                "SigningAlgorithm": "SHA384",
                "MdnSigningAlgorithm": "DEFAULT",
                "MdnResponse": "SYNC",
                "PreserveContentType": "ENABLED",
            }),
        ),
        (
            "the weakest algorithms the model documents, with the asynchronous-MDN settings",
            json!({
                "LocalProfileId": LOCAL_PROFILE,
                "PartnerProfileId": PARTNER_PROFILE,
                "Compression": "ZLIB",
                "EncryptionAlgorithm": "DES_EDE3_CBC",
                "SigningAlgorithm": "SHA1",
                "MdnResponse": "ASYNC",
                "AsyncMdnConfig": {"Url": "https://mdn.example.com/async"},
            }),
        ),
    ]
}

/// The `CreateConnector` request an AS2 configuration is sent in.
fn as2_create_request(as2_config: Value) -> Value {
    json!({
        "AccessRole": ROLE,
        "Url": "https://partner.example.com/as2",
        "As2Config": as2_config,
    })
}

/// The `Message` an answer carries, or `""`.
fn message_of(answer: &crate::harness::Answer) -> String {
    member_of(&answer.body, "Message")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// RULE: each `As2Config` member the service fills no default for
/// ([`REQUIRED_AS2_MEMBERS`]) is required ON ITS OWN — a configuration complete but for
/// that one member is refused 400 `InvalidRequestException` with the service's sentence
/// NAMING it, and nothing is persisted — while a configuration naming all of them is
/// written however little else it carries, and the members the model gives a reading to
/// when absent ([`DEFAULTED_AS2_MEMBERS`]) are never asked for.
///
/// The requirement is on the configuration the connector would HAVE, not on the request:
/// the last leg proves an `UpdateConnector` that names one member alone is served,
/// because the members it does not name stay where they are.
///
/// The rows are [`complete_as2_configurations`] crossed with the member each removes.
#[tokio::test]
async fn each_required_as2_config_member_is_required_on_its_own() {
    let emulator = Emulator::new();

    let before = create_connector(
        &emulator,
        "a connector created before the refusals",
        json!({
            "AccessRole": ROLE,
            "Url": "sftp://partner.example.com",
            "SftpConfig": {"UserSecretId": "sftp"},
        }),
    )
    .await;

    let configurations = complete_as2_configurations();
    let mut written = 0_u64;

    for (configuration_case, configuration) in &configurations {
        let configuration_case = *configuration_case;
        for omitted in REQUIRED_AS2_MEMBERS {
            let case = format!("{configuration_case}, with {omitted} omitted");
            let mut as2_config = configuration.clone();
            as2_config
                .as_object_mut()
                .expect("a configuration is an object")
                .remove(omitted);

            let answer = emulator
                .call("CreateConnector", as2_create_request(as2_config))
                .await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "InvalidRequestException",
                "case {case:?}: expected InvalidRequestException, got {}",
                answer.body
            );
            let message = message_of(&answer);
            assert_eq!(
                message,
                format!("{omitted} is required in As2Config"),
                "case {case:?}: expected the refusal to name the missing member, got \
                 {message:?}"
            );
            assert!(
                answer.get("ConnectorId").is_null(),
                "case {case:?}: a refused create must answer no ConnectorId, got {}",
                answer.body
            );
        }

        // The members the model gives a reading to when absent are NOT asked for: the
        // same configuration without either of them is written.
        for defaulted in DEFAULTED_AS2_MEMBERS {
            let case = format!("{configuration_case}, with {defaulted} omitted");
            let mut as2_config = configuration.clone();
            as2_config
                .as_object_mut()
                .expect("a configuration is an object")
                .remove(defaulted);
            create_connector(&emulator, &case, as2_create_request(as2_config)).await;
            written += 1;
        }

        // And the complete configuration is written and read back exactly as configured.
        let connector_id =
            create_connector(&emulator, configuration_case, as2_create_request(configuration.clone()))
                .await;
        written += 1;
        let described = describe_connector(&emulator, configuration_case, &connector_id).await;
        assert_members(
            configuration_case,
            &described,
            &json!({"As2Config": configuration}),
        );
    }

    // Nothing was persisted for any refused create: the ids minted since `before` are
    // exactly the ones the served creates took.
    let after = create_connector(
        &emulator,
        "a connector created after the refusals",
        json!({
            "AccessRole": ROLE,
            "Url": "sftp://partner.example.com",
            "SftpConfig": {"UserSecretId": "sftp"},
        }),
    )
    .await;
    assert_eq!(
        id_sequence(&after),
        id_sequence(&before) + written + 1,
        "expected the refused creates to persist nothing, but the id moved from \
         {before:?} to {after:?} over {written} served creates"
    );

    // The rule is about the configuration the connector would HAVE: an update naming one
    // member alone leaves the others in place, so it is served and the connector still
    // names every required member.
    let (_, configuration) = configurations
        .first()
        .expect("the catalogue has a configuration");
    let stored = create_connector(
        &emulator,
        "a connector the update addresses",
        as2_create_request(configuration.clone()),
    )
    .await;
    let case = "an update naming one AS2 member alone";
    let answer = emulator
        .call(
            "UpdateConnector",
            json!({
                "ConnectorId": &stored,
                "As2Config": {"MessageSubject": "invoices for the next period"},
            }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected UpdateConnector 200, got {} with body {}",
        answer.status, answer.body
    );
    let described = describe_connector(&emulator, case, &stored).await;
    let mut expected = configuration.clone();
    expected["MessageSubject"] = json!("invoices for the next period");
    assert_members(case, &described, &json!({"As2Config": expected}));
}

// ---------------------------------------------------------------------------
// Rule 6: WHICH sentence a Url that names no endpoint earns
// ---------------------------------------------------------------------------

/// The service's sentence for a `Url` that carries nothing but path information — a value
/// with no scheme, which has no authority to name a host with.
const URL_CARRIES_PATH_INFORMATION: &str =
    "Invalid URL; Configured protocol does not support path information in the url";

/// The service's sentence for a `Url` that IS a URL but whose host could never be
/// resolved.
const URL_NOT_VALID: &str = "The specified Url is not valid";

/// A malformed `Url`: the value, whether the row configures SFTP (else AS2), and the
/// sentence the value's OWN condition earns.
struct MalformedUrl {
    case: &'static str,
    url: &'static str,
    sftp: bool,
    expected: &'static str,
}

/// The `Url` values that name no endpoint, one per way of failing to, crossed with the
/// protocol configuration the connector would carry.
///
/// The first group carries NO SCHEME — a bare host name, a host with a path, a
/// scheme-relative reference, a path on its own and a relative path of several
/// segments — so each is read to its end as path information. The second group IS a URL,
/// and fails on its host: a reserved top-level domain the DNS never delegates, a scheme
/// whose URL has no host at all, and a name the DNS root holds no address for.
fn malformed_urls() -> Vec<MalformedUrl> {
    vec![
        MalformedUrl {
            case: "a bare host name, the whole of which is path information",
            url: "ftp.example.com",
            sftp: true,
            expected: URL_CARRIES_PATH_INFORMATION,
        },
        MalformedUrl {
            case: "a host and a path with no scheme",
            url: "partner.example.com/as2",
            sftp: false,
            expected: URL_CARRIES_PATH_INFORMATION,
        },
        MalformedUrl {
            case: "a scheme-relative reference",
            url: "//partner.example.com/as2",
            sftp: false,
            expected: URL_CARRIES_PATH_INFORMATION,
        },
        MalformedUrl {
            case: "a path on its own",
            url: "/as2",
            sftp: true,
            expected: URL_CARRIES_PATH_INFORMATION,
        },
        MalformedUrl {
            case: "a relative path of several segments",
            url: "inbox/partner/as2",
            sftp: true,
            expected: URL_CARRIES_PATH_INFORMATION,
        },
        MalformedUrl {
            case: "an https endpoint under the reserved .invalid",
            url: "https://partner.invalid/as2",
            sftp: false,
            expected: URL_NOT_VALID,
        },
        MalformedUrl {
            case: "an sftp endpoint under the reserved .test",
            url: "sftp://partner.test",
            sftp: true,
            expected: URL_NOT_VALID,
        },
        MalformedUrl {
            case: "a scheme whose URL carries no host at all",
            url: "mailto:partner@example.com",
            sftp: false,
            expected: URL_NOT_VALID,
        },
        MalformedUrl {
            case: "a single-label host, which the DNS root holds no address for",
            url: "sftp://partner",
            sftp: true,
            expected: URL_NOT_VALID,
        },
    ]
}

/// RULE: a connector `Url` that names no endpoint is refused 400
/// `InvalidRequestException` with the service's sentence for ITS OWN condition — a value
/// carrying no scheme is nothing but path information, which no connector protocol
/// addresses its endpoint by, while a value that is a URL whose host could never be
/// resolved is not a valid one — on every operation that writes a connector, and nothing
/// is written either way. The same value WITH its scheme is served, and read back
/// verbatim.
///
/// The rows are [`malformed_urls`], crossed with the two operations that write a
/// connector.
#[tokio::test]
async fn a_url_that_names_no_endpoint_earns_the_sentence_for_its_own_condition() {
    let emulator = Emulator::new();

    // One stored connector per protocol, so an update can carry each row's value onto a
    // connector configured the way the row is.
    let stored_sftp_url = "sftp://partner.example.com";
    let stored_sftp = create_connector(
        &emulator,
        "a stored SFTP connector",
        json!({
            "AccessRole": ROLE,
            "Url": stored_sftp_url,
            "SftpConfig": {"UserSecretId": "sftp"},
        }),
    )
    .await;
    let stored_as2_url = "https://partner.example.com/as2";
    let stored_as2 = create_connector(
        &emulator,
        "a stored AS2 connector",
        as2_create_request(json!({
            "LocalProfileId": LOCAL_PROFILE,
            "PartnerProfileId": PARTNER_PROFILE,
            "Compression": "DISABLED",
            "EncryptionAlgorithm": "NONE",
            "SigningAlgorithm": "NONE",
        })),
    )
    .await;

    for row in malformed_urls() {
        let protocol_config = if row.sftp {
            json!({"SftpConfig": {"UserSecretId": "sftp"}})
        } else {
            json!({"As2Config": {
                "LocalProfileId": LOCAL_PROFILE,
                "PartnerProfileId": PARTNER_PROFILE,
                "Compression": "DISABLED",
                "EncryptionAlgorithm": "NONE",
                "SigningAlgorithm": "NONE",
            }})
        };

        let mut request = json!({"AccessRole": ROLE, "Url": row.url});
        let object = request.as_object_mut().expect("the request is an object");
        for (member, value) in protocol_config
            .as_object()
            .expect("a configuration is an object")
        {
            object.insert(member.clone(), value.clone());
        }

        let case = format!("CreateConnector with {}", row.case);
        let answer = emulator.call("CreateConnector", request).await;
        assert_eq!(
            (answer.status, answer.error_code().as_str()),
            (400, "InvalidRequestException"),
            "case {case:?}: expected 400 InvalidRequestException, got {} with body {}",
            answer.status,
            answer.body
        );
        let message = message_of(&answer);
        assert_eq!(
            message, row.expected,
            "case {case:?}: expected the sentence for this condition, got {message:?}"
        );
        assert!(
            !message.contains(row.url),
            "case {case:?}: the sentence is fixed text, so it carries no submitted value, \
             got {message:?}"
        );
        assert!(
            answer.get("ConnectorId").is_null(),
            "case {case:?}: a refused create must answer no ConnectorId, got {}",
            answer.body
        );

        // The same value on an UPDATE of a connector of the row's protocol: the same
        // sentence, and the stored connector keeps the endpoint it had.
        let (stored, stored_url) = if row.sftp {
            (&stored_sftp, stored_sftp_url)
        } else {
            (&stored_as2, stored_as2_url)
        };
        let case = format!("UpdateConnector with {}", row.case);
        let answer = emulator
            .call(
                "UpdateConnector",
                json!({"ConnectorId": stored, "Url": row.url}),
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code().as_str()),
            (400, "InvalidRequestException"),
            "case {case:?}: expected 400 InvalidRequestException, got {} with body {}",
            answer.status,
            answer.body
        );
        let message = message_of(&answer);
        assert_eq!(
            message, row.expected,
            "case {case:?}: expected the sentence for this condition, got {message:?}"
        );
        let described = describe_connector(&emulator, &case, stored).await;
        assert_eq!(
            member_of(&described, "Url").as_str(),
            Some(stored_url),
            "case {case:?}: expected the stored connector to keep its endpoint, got {described}"
        );
    }

    // The control, bounding the rule from the other side: each schemeless value above,
    // given the scheme its protocol speaks, names an endpoint — so the create is served
    // and the read echoes the value verbatim.
    let served: Vec<(&str, &str, bool)> = vec![
        ("the bare host name, given the sftp scheme", "sftp://ftp.example.com", true),
        ("the host and path, given the https scheme", "https://partner.example.com/as2", false),
        ("a host and a deep path over plain http", "http://partner.example.com:8080/inbox/as2", false),
    ];
    for (case, url, sftp) in served {
        let request = if sftp {
            json!({
                "AccessRole": ROLE,
                "Url": url,
                "SftpConfig": {"UserSecretId": "sftp"},
            })
        } else {
            json!({
                "AccessRole": ROLE,
                "Url": url,
                "As2Config": {
                    "LocalProfileId": LOCAL_PROFILE,
                    "PartnerProfileId": PARTNER_PROFILE,
                    "Compression": "DISABLED",
                    "EncryptionAlgorithm": "NONE",
                    "SigningAlgorithm": "NONE",
                },
            })
        };
        let connector_id = create_connector(&emulator, case, request).await;
        let described = describe_connector(&emulator, case, &connector_id).await;
        assert_eq!(
            member_of(&described, "Url").as_str(),
            Some(url),
            "case {case:?}: expected the Url to be echoed verbatim, got {described}"
        );
    }
}
