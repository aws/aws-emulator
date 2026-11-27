// SPDX-License-Identifier: CC0-1.0

//! What `UpdateConnector` writes to a stored connector, and what it refuses.
//!
//! Four rules, each asserted over a TABLE of inputs rather than one recorded call:
//!
//! 1. an update is a PARTIAL write: every member the request names replaces the stored
//!    one, every member it does not name keeps the value the connector already had, and
//!    a later `DescribeConnector` reports exactly that
//!    ([`an_update_replaces_the_members_it_names_and_leaves_the_rest`]) — and the rule
//!    reaches INSIDE a nested configuration member, so an `SftpConfig` or an `As2Config`
//!    the request names modifies that structure member by member
//!    ([`an_as2_connectors_members_update_the_same_way`]), which is what the documented
//!    provisioning sequences need: pinning the host key `TestConnection` reported leaves
//!    the connector's `UserSecretId`, and turning AS2 Basic authentication off takes an
//!    update carrying an EMPTY `BasicAuthSecretId`
//!    ([`an_empty_basic_auth_secret_configures_no_basic_authentication`]);
//! 2. an `EgressConfig` update MODIFIES the VPC Lattice settings the connector already
//!    has, member by member, and is refused for a connector that has none — `EgressType`
//!    is not an updatable member
//!    ([`an_egress_update_modifies_the_settings_the_connector_already_has`]);
//! 3. an update whose RESULTING configuration violates a documented cross-member
//!    requirement is refused with the modelled `InvalidRequestException`, and the stored
//!    connector is left exactly as it was
//!    ([`updates_whose_result_cannot_hold_together_are_refused_and_write_nothing`]);
//! 4. a `ConnectorId` the store does not hold answers this service's generic not-found
//!    error ([`updating_a_connector_that_was_never_created_is_not_found`]).
//!
//! The rows differ along which members the request carries and how many, the protocol
//! configuration (AS2 or SFTP), the egress type (service-managed or VPC Lattice), the
//! enum value each member carries, list lengths, and where a numeric member sits
//! relative to its published bounds. The harness (router, request signing,
//! `assert_members`) is the shared [`crate::harness`].

use serde_json::{json, Value};

use crate::harness::{assert_members, member_of, Emulator, ROLE};

/// A `VpcLatticeResourceConfigurationArn` satisfying the member's modelled pattern
/// `^arn:[a-z0-9\-]+:vpc-lattice:[a-zA-Z0-9\-]+:\d{12}:resourceconfiguration/rcfg-[0-9a-z]{17}$`.
const RESOURCE_CONFIGURATION_ARN: &str =
    "arn:aws:vpc-lattice:us-east-1:000000000000:resourceconfiguration/rcfg-0123456789abcdef0";

/// A SECOND Resource Configuration, so an egress update can move the connector to a
/// different one.
const OTHER_RESOURCE_CONFIGURATION_ARN: &str =
    "arn:aws:vpc-lattice:us-east-1:000000000000:resourceconfiguration/rcfg-00000000000000001";

/// A `ProfileId` (`len 19..19`, pattern `^p-([0-9a-f]{17})$`) for the local AS2 profile.
const LOCAL_PROFILE: &str = "p-0123456789abcdef0";

/// A second `ProfileId`, for the partner side of the same AS2 connector.
const PARTNER_PROFILE: &str = "p-00000000000000001";

/// An `As2ConnectorSecretId` (`len 0..2048`) for the secret AS2 Basic authentication
/// reads its credentials from.
const AS2_BASIC_AUTH_SECRET: &str =
    "arn:aws:secretsmanager:us-east-1:000000000000:secret:as2-basic";

/// A `ServerId` (`len 19..19`, pattern `^s-([0-9a-f]{17})$`) for a server that can handle
/// asynchronous MDN responses.
const MDN_SERVER: &str = "s-0123456789abcdef0";

/// A second such server, so `AsyncMdnConfig.ServerIds` (`len 1..10`) carries more than
/// one.
const OTHER_MDN_SERVER: &str = "s-00000000000000001";

/// Create a connector over the wire and return its `ConnectorId`.
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

/// Describe `connector_id` and return the `Connector` the read reports.
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

/// Update `connector_id`, assert the call succeeded and answered the id it was given,
/// and return the connector as a later read reports it.
async fn update_connector(
    emulator: &Emulator,
    case: &str,
    connector_id: &str,
    mut request: Value,
) -> Value {
    request["ConnectorId"] = json!(connector_id);
    let answer = emulator.call("UpdateConnector", request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected UpdateConnector 200, got {} with body {}",
        answer.status, answer.body
    );
    let answered = answer.get("ConnectorId").as_str().unwrap_or_default().to_string();
    assert_eq!(
        answered, connector_id,
        "case {case:?}: expected UpdateConnector to answer the connector it updated \
         ({connector_id:?}), got {answered:?}"
    );
    describe_connector(emulator, case, connector_id).await
}

// ---------------------------------------------------------------------------
// Rule 1: an update replaces the members it names, and only those.
// ---------------------------------------------------------------------------

/// RULE: `UpdateConnector` writes the connector it names in the service's state —
/// replacing every member the request carries, leaving every member it does not carry at
/// the value the connector already had — and answers that connector's id; the write is
/// what a later `DescribeConnector` reports.
///
/// Each row updates the SAME stored connector, so the rows also prove the writes
/// accumulate: the expectation of every row names members earlier rows set, which a
/// handler that rebuilt the connector from the request alone would have dropped. The
/// rows differ along which members they carry and how many (one, several, all, none),
/// which enum value each carries, the nested list lengths, and where
/// `MaxConcurrentConnections` sits relative to its published bounds.
#[tokio::test]
async fn an_update_replaces_the_members_it_names_and_leaves_the_rest() {
    let emulator = Emulator::new();

    let connector_id = create_connector(
        &emulator,
        "the SFTP connector every row updates",
        json!({
            "AccessRole": ROLE,
            "Url": "sftp://partner.example.com",
            "SftpConfig": {"UserSecretId": "sftp-partner-credentials"},
        }),
    )
    .await;

    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "an empty update names nothing, so the connector is untouched",
            json!({}),
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com",
                "SftpConfig": {
                    "UserSecretId": "sftp-partner-credentials",
                    "TrustedHostKeys": [],
                    "MaxConcurrentConnections": 1,
                    "OrderedUserSecretVersionStages": ["AWSCURRENT"],
                },
                "EgressType": "SERVICE_MANAGED",
                "Status": "ACTIVE",
                "LoggingRole": null,
                // The members the service assigns itself are what the create resolved,
                // and an update that names nothing leaves them there.
                "SecurityPolicyName": "TransferSFTPConnectorSecurityPolicy-2024-03",
                "IpAddressType": "IPV4",
                "As2Config": null,
                "EgressConfig": null,
            }),
        ),
        (
            "one member: the partner endpoint moves",
            json!({"Url": "sftp://partner.example.com:2222"}),
            json!({
                "Url": "sftp://partner.example.com:2222",
                "AccessRole": ROLE,
                "SftpConfig": {
                    "UserSecretId": "sftp-partner-credentials",
                    "TrustedHostKeys": [],
                    "MaxConcurrentConnections": 1,
                    "OrderedUserSecretVersionStages": ["AWSCURRENT"],
                },
                "LoggingRole": null,
            }),
        ),
        (
            "the two roles, leaving the endpoint the previous row set",
            json!({
                "AccessRole": "arn:aws:iam::000000000000:role/transfer-rotated",
                "LoggingRole": "arn:aws:iam::000000000000:role/transfer-logging",
            }),
            json!({
                "AccessRole": "arn:aws:iam::000000000000:role/transfer-rotated",
                "LoggingRole": "arn:aws:iam::000000000000:role/transfer-logging",
                "Url": "sftp://partner.example.com:2222",
            }),
        ),
        (
            "a security policy and an IP address type, at one of each enum's values",
            json!({
                "SecurityPolicyName": "TransferSFTPConnectorSecurityPolicy-2024-03",
                "IpAddressType": "DUALSTACK",
            }),
            json!({
                "SecurityPolicyName": "TransferSFTPConnectorSecurityPolicy-2024-03",
                "IpAddressType": "DUALSTACK",
                "AccessRole": "arn:aws:iam::000000000000:role/transfer-rotated",
                "LoggingRole": "arn:aws:iam::000000000000:role/transfer-logging",
            }),
        ),
        (
            "the other IpAddressType value, and a FIPS policy from the published catalog",
            json!({
                "IpAddressType": "IPV4",
                "SecurityPolicyName": "TransferSFTPConnectorSecurityPolicy-FIPS-2024-10",
            }),
            json!({
                "IpAddressType": "IPV4",
                "SecurityPolicyName": "TransferSFTPConnectorSecurityPolicy-FIPS-2024-10",
            }),
        ),
        (
            "the whole SftpConfig at the published MaxConcurrentConnections maximum, with \
             two trusted host keys and two secret version stages",
            json!({
                "SftpConfig": {
                    "UserSecretId": "arn:aws:secretsmanager:us-east-1:000000000000:secret:sftp",
                    "MaxConcurrentConnections": 5,
                    "TrustedHostKeys": [
                        "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ==",
                        "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTY=",
                    ],
                    "OrderedUserSecretVersionStages": ["AWSCURRENT", "AWSPREVIOUS"],
                },
            }),
            json!({
                "SftpConfig": {
                    "UserSecretId": "arn:aws:secretsmanager:us-east-1:000000000000:secret:sftp",
                    "MaxConcurrentConnections": 5,
                    "TrustedHostKeys": [
                        "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ==",
                        "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTY=",
                    ],
                    "OrderedUserSecretVersionStages": ["AWSCURRENT", "AWSPREVIOUS"],
                },
                "Url": "sftp://partner.example.com:2222",
                "IpAddressType": "IPV4",
            }),
        ),
        (
            "an SftpConfig that names only the user secret MODIFIES that member and \
             leaves the rest of the configuration as it stands, which is what the \
             documented host-key provisioning sequence needs",
            json!({"SftpConfig": {"UserSecretId": "sftp-partner-credentials"}}),
            json!({
                // Compared exactly: the trusted host keys, the secret version stages and
                // the concurrency the previous row set are all still there, because this
                // update names none of them.
                "SftpConfig": {
                    "UserSecretId": "sftp-partner-credentials",
                    "MaxConcurrentConnections": 5,
                    "TrustedHostKeys": [
                        "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ==",
                        "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTY=",
                    ],
                    "OrderedUserSecretVersionStages": ["AWSCURRENT", "AWSPREVIOUS"],
                },
            }),
        ),
    ];

    for (case, request, expected) in cases {
        let described = update_connector(&emulator, case, &connector_id, request.clone()).await;
        assert_members(case, &described, &expected);

        // The members no update ever names keep their create-time values, so an update
        // never re-mints or re-resolves the connector itself.
        assert_members(
            case,
            &described,
            &json!({
                "ConnectorId": connector_id,
                "EgressType": "SERVICE_MANAGED",
                "Status": "ACTIVE",
                "As2Config": null,
                "EgressConfig": null,
            }),
        );

        // Applying the same update again lands the connector in the same place: the
        // write depends on the request and the stored connector, not on how many times
        // it ran.
        let again = update_connector(&emulator, case, &connector_id, request).await;
        assert_eq!(
            again, described,
            "case {case:?}: expected the same update applied twice to leave the same \
             connector, got {again:?} after {described:?}"
        );
    }
}

/// RULE (rule 1, varied along the protocol configuration): an AS2 connector's own
/// members update the same way — an `As2Config` the request names MODIFIES the AS2
/// settings the connector already has, member by member, so a member the request leaves
/// out keeps its stored value, and only `BasicAuthSecretId`, the one member the model
/// documents as turned off by an UPDATE CARRYING AN EMPTY VALUE, is cleared by naming it
/// empty.
///
/// The rows differ along which AS2 members the request carries and how many (one,
/// several, all of the nested `AsyncMdnConfig`, none), which value each enum member
/// carries, the nested list length, and whether the secret member is named, named empty
/// or not named at all.
#[tokio::test]
async fn an_as2_connectors_members_update_the_same_way() {
    let emulator = Emulator::new();

    let connector_id = create_connector(
        &emulator,
        "an AS2 connector",
        json!({
            "AccessRole": ROLE,
            "Url": "https://partner.example.com/as2",
            "As2Config": {
                "LocalProfileId": LOCAL_PROFILE,
                "PartnerProfileId": PARTNER_PROFILE,
                "Compression": "ZLIB",
                "EncryptionAlgorithm": "NONE",
                "SigningAlgorithm": "NONE",
            },
        }),
    )
    .await;

    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "one AS2 member: the subject is set, and the members the create configured \
             stay as they are",
            json!({"As2Config": {"MessageSubject": "invoices for the next period"}}),
            json!({
                // A nested object is compared EXACTLY, so this expectation is also what
                // proves `Compression` and both profile ids — which the create set and
                // this update does not name — are still on the connector.
                "As2Config": {
                    "LocalProfileId": LOCAL_PROFILE,
                    "PartnerProfileId": PARTNER_PROFILE,
                    "Compression": "ZLIB",
                    "EncryptionAlgorithm": "NONE",
                    "SigningAlgorithm": "NONE",
                    "MessageSubject": "invoices for the next period",
                },
                "Url": "https://partner.example.com/as2",
                "EgressType": "SERVICE_MANAGED",
                "SftpConfig": null,
            }),
        ),
        (
            "several AS2 members at once, each at one of its enum's values, leaving the \
             subject the previous row set",
            json!({
                "As2Config": {
                    "Compression": "DISABLED",
                    "EncryptionAlgorithm": "AES256_CBC",
                    "SigningAlgorithm": "SHA384",
                    "MdnSigningAlgorithm": "DEFAULT",
                    "MdnResponse": "SYNC",
                    "PreserveContentType": "ENABLED",
                },
            }),
            json!({
                "As2Config": {
                    "LocalProfileId": LOCAL_PROFILE,
                    "PartnerProfileId": PARTNER_PROFILE,
                    "MessageSubject": "invoices for the next period",
                    "Compression": "DISABLED",
                    "EncryptionAlgorithm": "AES256_CBC",
                    "SigningAlgorithm": "SHA384",
                    "MdnSigningAlgorithm": "DEFAULT",
                    "MdnResponse": "SYNC",
                    "PreserveContentType": "ENABLED",
                },
            }),
        ),
        (
            "Basic authentication is enabled by naming its secret, and nothing else moves",
            json!({"As2Config": {"BasicAuthSecretId": AS2_BASIC_AUTH_SECRET}}),
            json!({
                "As2Config": {
                    "LocalProfileId": LOCAL_PROFILE,
                    "PartnerProfileId": PARTNER_PROFILE,
                    "MessageSubject": "invoices for the next period",
                    "Compression": "DISABLED",
                    "EncryptionAlgorithm": "AES256_CBC",
                    "SigningAlgorithm": "SHA384",
                    "MdnSigningAlgorithm": "DEFAULT",
                    "MdnResponse": "SYNC",
                    "PreserveContentType": "ENABLED",
                    "BasicAuthSecretId": AS2_BASIC_AUTH_SECRET,
                },
            }),
        ),
        (
            "the nested asynchronous-MDN configuration, named for the first time: the \
             connector had none, so it is what the request names",
            json!({
                "As2Config": {
                    "MdnResponse": "ASYNC",
                    "AsyncMdnConfig": {"Url": "https://mdn.example.com/async"},
                },
            }),
            json!({
                "As2Config": {
                    "LocalProfileId": LOCAL_PROFILE,
                    "PartnerProfileId": PARTNER_PROFILE,
                    "MessageSubject": "invoices for the next period",
                    "Compression": "DISABLED",
                    "EncryptionAlgorithm": "AES256_CBC",
                    "SigningAlgorithm": "SHA384",
                    "MdnSigningAlgorithm": "DEFAULT",
                    "MdnResponse": "ASYNC",
                    "PreserveContentType": "ENABLED",
                    "BasicAuthSecretId": AS2_BASIC_AUTH_SECRET,
                    "AsyncMdnConfig": {"Url": "https://mdn.example.com/async"},
                },
            }),
        ),
        (
            "the same rule INSIDE that nested structure: naming the servers that handle \
             the responses leaves the URL they are sent to",
            json!({
                "As2Config": {
                    "AsyncMdnConfig": {"ServerIds": [MDN_SERVER, OTHER_MDN_SERVER]},
                },
            }),
            json!({
                "As2Config": {
                    "LocalProfileId": LOCAL_PROFILE,
                    "PartnerProfileId": PARTNER_PROFILE,
                    "MessageSubject": "invoices for the next period",
                    "Compression": "DISABLED",
                    "EncryptionAlgorithm": "AES256_CBC",
                    "SigningAlgorithm": "SHA384",
                    "MdnSigningAlgorithm": "DEFAULT",
                    "MdnResponse": "ASYNC",
                    "PreserveContentType": "ENABLED",
                    "BasicAuthSecretId": AS2_BASIC_AUTH_SECRET,
                    "AsyncMdnConfig": {
                        "Url": "https://mdn.example.com/async",
                        "ServerIds": [MDN_SERVER, OTHER_MDN_SERVER],
                    },
                },
            }),
        ),
        (
            "an As2Config that names NO member leaves every AS2 setting as it stands",
            json!({"As2Config": {}}),
            json!({
                "As2Config": {
                    "LocalProfileId": LOCAL_PROFILE,
                    "PartnerProfileId": PARTNER_PROFILE,
                    "MessageSubject": "invoices for the next period",
                    "Compression": "DISABLED",
                    "EncryptionAlgorithm": "AES256_CBC",
                    "SigningAlgorithm": "SHA384",
                    "MdnSigningAlgorithm": "DEFAULT",
                    "MdnResponse": "ASYNC",
                    "PreserveContentType": "ENABLED",
                    "BasicAuthSecretId": AS2_BASIC_AUTH_SECRET,
                    "AsyncMdnConfig": {
                        "Url": "https://mdn.example.com/async",
                        "ServerIds": [MDN_SERVER, OTHER_MDN_SERVER],
                    },
                },
            }),
        ),
        (
            "Basic authentication is DISABLED by an empty secret id — the one member an \
             update clears by naming it empty — and the rest of the configuration stays",
            json!({"As2Config": {"BasicAuthSecretId": ""}}),
            json!({
                "As2Config": {
                    "LocalProfileId": LOCAL_PROFILE,
                    "PartnerProfileId": PARTNER_PROFILE,
                    "MessageSubject": "invoices for the next period",
                    "Compression": "DISABLED",
                    "EncryptionAlgorithm": "AES256_CBC",
                    "SigningAlgorithm": "SHA384",
                    "MdnSigningAlgorithm": "DEFAULT",
                    "MdnResponse": "ASYNC",
                    "PreserveContentType": "ENABLED",
                    "AsyncMdnConfig": {
                        "Url": "https://mdn.example.com/async",
                        "ServerIds": [MDN_SERVER, OTHER_MDN_SERVER],
                    },
                },
            }),
        ),
        (
            "the endpoint moves to another scheme and port, leaving the AS2 configuration",
            json!({"Url": "http://partner.example.com:8080/as2"}),
            json!({
                "Url": "http://partner.example.com:8080/as2",
                "As2Config": {
                    "LocalProfileId": LOCAL_PROFILE,
                    "PartnerProfileId": PARTNER_PROFILE,
                    "MessageSubject": "invoices for the next period",
                    "Compression": "DISABLED",
                    "EncryptionAlgorithm": "AES256_CBC",
                    "SigningAlgorithm": "SHA384",
                    "MdnSigningAlgorithm": "DEFAULT",
                    "MdnResponse": "ASYNC",
                    "PreserveContentType": "ENABLED",
                    "AsyncMdnConfig": {
                        "Url": "https://mdn.example.com/async",
                        "ServerIds": [MDN_SERVER, OTHER_MDN_SERVER],
                    },
                },
            }),
        ),
    ];

    for (case, request, expected) in cases {
        let described = update_connector(&emulator, case, &connector_id, request).await;
        assert_members(case, &described, &expected);
    }
}

/// RULE (the same rule at CREATE time): `BasicAuthSecretId` is either SET or NOT SET —
/// "the default value for this parameter is null, which indicates that Basic
/// authentication is not enabled for the connector" — so a connector created with an
/// empty one is created with Basic authentication off, and a reader omits the member
/// rather than reporting the empty string back.
#[tokio::test]
async fn an_empty_basic_auth_secret_configures_no_basic_authentication() {
    let emulator = Emulator::new();

    // The rows differ along the form the member arrives in: not named at all, named
    // empty (its target `As2ConnectorSecretId` is modelled `len 0..2048`, so the empty
    // string reaches the handler), and named with a secret.
    let cases: Vec<(&str, Value, Value)> = vec![
        ("the member is not named", json!({}), Value::Null),
        ("the member is named empty", json!({"BasicAuthSecretId": ""}), Value::Null),
        (
            "the member names a secret",
            json!({"BasicAuthSecretId": AS2_BASIC_AUTH_SECRET}),
            json!(AS2_BASIC_AUTH_SECRET),
        ),
    ];

    for (case, mut as2_config, expected) in cases {
        as2_config["LocalProfileId"] = json!(LOCAL_PROFILE);
        as2_config["PartnerProfileId"] = json!(PARTNER_PROFILE);
        // The members an AS2 configuration is required to settle, which every row
        // carries so the rows differ only along the secret member under test.
        as2_config["Compression"] = json!("DISABLED");
        as2_config["EncryptionAlgorithm"] = json!("NONE");
        as2_config["SigningAlgorithm"] = json!("NONE");
        let connector_id = create_connector(
            &emulator,
            case,
            json!({
                "AccessRole": ROLE,
                "Url": "https://partner.example.com/as2",
                "As2Config": as2_config,
            }),
        )
        .await;

        let described = describe_connector(&emulator, case, &connector_id).await;
        let reported = member_of(&described, "As2Config")
            .get("BasicAuthSecretId")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected DescribeConnector to report \
             As2Config.BasicAuthSecretId as {expected:?}, got {reported:?} in {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: an egress update modifies what the connector already has.
// ---------------------------------------------------------------------------

/// RULE: `EgressConfig` on an update MODIFIES the VPC Lattice settings the connector
/// already has — every member of `UpdateConnectorVpcLatticeEgressConfig` is optional, so
/// one it leaves out keeps its stored value — and a connector with no egress
/// configuration has nothing to modify, which is refused with the modelled
/// `InvalidRequestException` because no update member names `EgressType`.
///
/// The rows differ along which egress members the request carries and where `PortNumber`
/// sits in its modelled `range 1..65535` (each published bound, and a value in between).
#[tokio::test]
async fn an_egress_update_modifies_the_settings_the_connector_already_has() {
    let emulator = Emulator::new();

    let connector_id = create_connector(
        &emulator,
        "a VPC Lattice connector",
        json!({
            "AccessRole": ROLE,
            "SftpConfig": {"UserSecretId": "sftp-partner-credentials"},
            "EgressConfig": {
                "VpcLattice": {"ResourceConfigurationArn": RESOURCE_CONFIGURATION_ARN},
            },
        }),
    )
    .await;

    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "only the port: the Resource Configuration stays where it was",
            json!({"EgressConfig": {"VpcLattice": {"PortNumber": 2222}}}),
            json!({
                "VpcLattice": {
                    "ResourceConfigurationArn": RESOURCE_CONFIGURATION_ARN,
                    "PortNumber": 2222,
                },
            }),
        ),
        (
            "only the Resource Configuration: the port stays where it was",
            json!({
                "EgressConfig": {
                    "VpcLattice": {
                        "ResourceConfigurationArn": OTHER_RESOURCE_CONFIGURATION_ARN,
                    },
                },
            }),
            json!({
                "VpcLattice": {
                    "ResourceConfigurationArn": OTHER_RESOURCE_CONFIGURATION_ARN,
                    "PortNumber": 2222,
                },
            }),
        ),
        (
            "both, at the low end of PortNumber's modelled range (1)",
            json!({
                "EgressConfig": {
                    "VpcLattice": {
                        "ResourceConfigurationArn": RESOURCE_CONFIGURATION_ARN,
                        "PortNumber": 1,
                    },
                },
            }),
            json!({
                "VpcLattice": {
                    "ResourceConfigurationArn": RESOURCE_CONFIGURATION_ARN,
                    "PortNumber": 1,
                },
            }),
        ),
        (
            "both, at the high end of PortNumber's modelled range (65535)",
            json!({
                "EgressConfig": {
                    "VpcLattice": {
                        "ResourceConfigurationArn": OTHER_RESOURCE_CONFIGURATION_ARN,
                        "PortNumber": 65535,
                    },
                },
            }),
            json!({
                "VpcLattice": {
                    "ResourceConfigurationArn": OTHER_RESOURCE_CONFIGURATION_ARN,
                    "PortNumber": 65535,
                },
            }),
        ),
        (
            "an egress update naming neither member leaves both where they were",
            json!({"EgressConfig": {"VpcLattice": {}}}),
            json!({
                "VpcLattice": {
                    "ResourceConfigurationArn": OTHER_RESOURCE_CONFIGURATION_ARN,
                    "PortNumber": 65535,
                },
            }),
        ),
    ];

    for (case, request, expected_egress) in cases {
        let described = update_connector(&emulator, case, &connector_id, request).await;
        assert_members(case, &described, &json!({"EgressConfig": expected_egress}));
        // The egress type is not an updatable member, and a VPC Lattice connector still
        // reports no `Url`.
        assert_members(
            case,
            &described,
            &json!({"EgressType": "VPC_LATTICE", "Url": null}),
        );
    }

    // The other side of the rule: a connector with no egress configuration has none to
    // modify.
    let service_managed = create_connector(
        &emulator,
        "a service-managed connector",
        json!({
            "AccessRole": ROLE,
            "Url": "sftp://partner.example.com",
            "SftpConfig": {"UserSecretId": "sftp-partner-credentials"},
        }),
    )
    .await;
    let before = describe_connector(&emulator, "service-managed before", &service_managed).await;

    for (case, egress) in [
        (
            "a service-managed connector given a whole egress configuration",
            json!({
                "VpcLattice": {
                    "ResourceConfigurationArn": RESOURCE_CONFIGURATION_ARN,
                    "PortNumber": 22,
                },
            }),
        ),
        (
            "a service-managed connector given a partial egress configuration",
            json!({"VpcLattice": {"PortNumber": 22}}),
        ),
    ] {
        let answer = emulator
            .call(
                "UpdateConnector",
                json!({"ConnectorId": service_managed, "EgressConfig": egress}),
            )
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
        let message = member_of(&answer.body, "Message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            message.contains("EgressConfig"),
            "case {case:?}: expected the message to name EgressConfig, got {message:?}"
        );
        let after = describe_connector(&emulator, case, &service_managed).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must leave the connector as it was, \
             got {after:?} after {before:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: an update whose result cannot hold together is refused.
// ---------------------------------------------------------------------------

/// RULE: the configuration a connector would HAVE after an update must satisfy the same
/// documented cross-member requirements a create enforces; one that would not is refused
/// with HTTP 400 `InvalidRequestException`, naming the member at fault, and the stored
/// connector is left exactly as it was.
///
/// The rows cover each requirement from the side an update can reach it: the protocol
/// configurations as alternatives, `Url` against the egress type from both sides, and
/// `MaxConcurrentConnections` one step past its published maximum.
#[tokio::test]
async fn updates_whose_result_cannot_hold_together_are_refused_and_write_nothing() {
    let emulator = Emulator::new();

    let sftp = create_connector(
        &emulator,
        "a service-managed SFTP connector",
        json!({
            "AccessRole": ROLE,
            "Url": "sftp://partner.example.com",
            "SftpConfig": {"UserSecretId": "sftp-partner-credentials"},
        }),
    )
    .await;
    let as2 = create_connector(
        &emulator,
        "an AS2 connector",
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
    let vpc_lattice = create_connector(
        &emulator,
        "a VPC Lattice SFTP connector",
        json!({
            "AccessRole": ROLE,
            "SftpConfig": {"UserSecretId": "sftp-partner-credentials"},
            "EgressConfig": {
                "VpcLattice": {"ResourceConfigurationArn": RESOURCE_CONFIGURATION_ARN},
            },
        }),
    )
    .await;

    let cases: Vec<(&str, &String, Value, &str)> = vec![
        (
            "an SFTP connector given an As2Config would have two protocol configurations",
            &sftp,
            json!({"As2Config": {"LocalProfileId": LOCAL_PROFILE}}),
            "As2Config",
        ),
        (
            "an AS2 connector given an SftpConfig would have two protocol configurations",
            &as2,
            json!({"SftpConfig": {"UserSecretId": "sftp"}}),
            "SftpConfig",
        ),
        (
            "a service-managed connector whose Url is blanked would name no endpoint",
            &sftp,
            json!({"Url": ""}),
            "Url",
        ),
        (
            "a VPC Lattice connector given a Url, which the model says must be null",
            &vpc_lattice,
            json!({"Url": "sftp://partner.example.com"}),
            "Url",
        ),
        (
            "MaxConcurrentConnections one past the published maximum of 5",
            &sftp,
            json!({"SftpConfig": {"UserSecretId": "sftp", "MaxConcurrentConnections": 6}}),
            "MaxConcurrentConnections",
        ),
    ];

    for (case, connector_id, mut request, member_at_fault) in cases {
        let before = describe_connector(&emulator, case, connector_id).await;

        request["ConnectorId"] = json!(connector_id);
        let answer = emulator.call("UpdateConnector", request).await;
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
        let message = member_of(&answer.body, "Message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            message.contains(member_at_fault),
            "case {case:?}: expected the message to name {member_at_fault:?}, \
             got {message:?}"
        );

        let after = describe_connector(&emulator, case, connector_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must persist nothing, got {after:?} \
             after {before:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: a connector the store does not hold.
// ---------------------------------------------------------------------------

/// RULE: `UpdateConnector` for a syntactically valid `ConnectorId` the store does not
/// hold answers this service's generic not-found error — `ResourceNotFoundException`,
/// `ResourceType` `Connector`, `Resource` the id the request named, and a `Message`
/// carrying no identifier — and writes nothing.
///
/// The rows are ids from the low end of the id space, a mid value and the high end, each
/// carrying a different set of update members.
#[tokio::test]
async fn updating_a_connector_that_was_never_created_is_not_found() {
    let emulator = Emulator::new();

    let existing = create_connector(
        &emulator,
        "one connector the store does hold",
        json!({
            "AccessRole": ROLE,
            "Url": "sftp://partner.example.com",
            "SftpConfig": {"UserSecretId": "sftp-partner-credentials"},
        }),
    )
    .await;
    let before = describe_connector(&emulator, "the untouched connector", &existing).await;

    let cases: Vec<(&str, Value)> = vec![
        ("c-00000000000000000", json!({"Url": "sftp://elsewhere.example.com"})),
        ("c-0000000000000dead", json!({"AccessRole": ROLE})),
        (
            "c-fffffffffffffffff",
            json!({
                "SftpConfig": {"UserSecretId": "sftp", "MaxConcurrentConnections": 2},
                "IpAddressType": "DUALSTACK",
            }),
        ),
    ];

    let mut messages: Vec<(&str, String)> = Vec::new();

    for (connector_id, mut request) in cases {
        request["ConnectorId"] = json!(connector_id);
        let answer = emulator.call("UpdateConnector", request).await;
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
            !message.contains(connector_id),
            "case {connector_id:?}: expected no identifier in Message, got {message:?}"
        );
        messages.push((connector_id, message));
    }

    let (first_case, first) = messages.first().expect("every row produced a message").clone();
    for (case, message) in &messages {
        assert_eq!(
            message, &first,
            "case {case:?}: expected the same fixed Connector message as case \
             {first_case:?} ({first:?}), got {message:?}"
        );
    }

    // None of the refused updates touched the connector the store does hold.
    let after = describe_connector(&emulator, "the untouched connector", &existing).await;
    assert_eq!(
        after, before,
        "expected the stored connector to be untouched, got {after:?} after {before:?}"
    );
}
