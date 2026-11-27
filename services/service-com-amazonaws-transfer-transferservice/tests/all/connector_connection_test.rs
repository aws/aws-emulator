// SPDX-License-Identifier: CC0-1.0

//! What `TestConnection` answers for a stored connector.
//!
//! Three rules, each asserted over a TABLE of connectors rather than one recorded call:
//!
//! 1. testing a stored SFTP connector answers HTTP 200 carrying the connector's own id
//!    and the verdict its STORED configuration earns: `Status` at the operation's
//!    documented failure value, a `StatusMessage` naming the first defect that
//!    configuration carries — no secret to authenticate with, no trusted host key, or a
//!    trusted host key that is not an SSH public key — or, when it carries none, saying
//!    the connection itself was never verified, and NO `SftpConnectionDetails`, because
//!    the remote server's host key is only ever observed on a connection
//!    ([`testing_an_sftp_connector_reports_the_verdict_its_configuration_earns`]). The
//!    same suite asserts the text is FIXED per condition (no identifier, secret or key
//!    body reaches it) and that a test is a pure read: it answers the same thing twice
//!    and leaves the described connector unchanged. The verdict is ONE LINE, stating the
//!    condition and nothing else
//!    ([`a_test_connection_verdict_is_one_line_and_carries_no_troubleshooting_guidance`]);
//! 2. a connector that configures no SFTP connection has none for this call to test, so
//!    it is refused with the modelled `InvalidRequestException`
//!    ([`testing_a_connector_that_configures_no_sftp_connection_is_refused`]);
//! 3. a `ConnectorId` the store does not hold answers this service's generic not-found
//!    error ([`testing_a_connector_that_was_never_created_is_not_found`]).
//!
//! The rows differ along what each rule generalises over: which member of the stored
//! SFTP configuration is missing or unusable, the FORM of the missing value (absent, or
//! present but blank/empty), the number and ORDER of trusted host keys, the key TYPE
//! (`ssh-ed25519` and `ssh-rsa`, both encoded here by the OpenSSH key-format crate), the
//! egress type (service-managed with a URL, VPC Lattice without one), and where the
//! probe id sits in the id space. The harness (router, request signing) is the shared
//! [`crate::harness`].

use serde_json::{json, Value};

use crate::harness::{generated_public_key, is_modelled_connector_id, member_of, Emulator, ROLE};

/// `Status`'s two documented values: "Returns OK for successful test, or ERROR if the
/// test fails" (`smithy_query(query="operation",operation="TestConnection")` reports
/// `Status` as a plain string, so these are the only two the operation documents).
const STATUS_OK: &str = "OK";
const STATUS_ERROR: &str = "ERROR";

/// A `VpcLatticeResourceConfigurationArn` satisfying the member's modelled pattern
/// `^arn:[a-z0-9\-]+:vpc-lattice:[a-zA-Z0-9\-]+:\d{12}:resourceconfiguration/rcfg-[0-9a-z]{17}$`.
const RESOURCE_CONFIGURATION_ARN: &str =
    "arn:aws:vpc-lattice:us-east-1:000000000000:resourceconfiguration/rcfg-0123456789abcdef0";

/// A `UserSecretId` satisfying the member's modelled `len 1..2048`.
const SECRET: &str = "arn:aws:secretsmanager:us-east-1:000000000000:secret:sftp-partner";

/// Which condition a row's stored configuration is in — the thing the `StatusMessage`
/// must diagnose, and the key the text is asserted to be FIXED per.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Condition {
    /// No `UserSecretId` to read the SFTP user's credentials from.
    NoSecret,
    /// No `TrustedHostKeys` to identify the remote server by.
    NoTrustedHostKey,
    /// A `TrustedHostKeys` entry that is not an SSH public key.
    UnreadableTrustedHostKey,
    /// Nothing wrong with the stored configuration: the verdict is about the connection.
    ConnectionNotVerified,
}

impl Condition {
    /// The word the diagnosis must name, so a caller can fix the connector from the
    /// message alone: the member at fault, or — where no member is at fault — the thing
    /// that was not established.
    fn names(self) -> &'static str {
        match self {
            Self::NoSecret => "UserSecretId",
            Self::NoTrustedHostKey | Self::UnreadableTrustedHostKey => "TrustedHostKeys",
            Self::ConnectionNotVerified => "connection",
        }
    }
}

/// Create a connector, assert the create succeeded, and return its `ConnectorId`.
///
/// Local to this suite rather than on the shared [`Emulator`] for the reason
/// `connector_configuration.rs` records for its own copy: it carries the TABLE ROW's
/// label into every assertion message.
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
            panic!(
                "case {case:?}: CreateConnector returned no ConnectorId: {}",
                answer.body
            )
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

/// An `ssh-rsa` public key in the `key-type key-body comment` form `ssh-keygen` writes.
///
/// Assembled from the two integers an `ssh-rsa` key is made of by the OpenSSH
/// key-format crate itself, so the string is produced by that format's implementation
/// rather than pasted in — the readable-key rule is about the FORMAT, so the table
/// carries a second key type beside the generated `ssh-ed25519` ones.
fn rsa_public_key() -> String {
    let e = ssh_key::Mpint::from_positive_bytes(&[0x01, 0x00, 0x01])
        .expect("65537 is a positive integer");
    let n = ssh_key::Mpint::from_positive_bytes(&[0xc5; 256])
        .expect("a 2048-bit modulus is a positive integer");
    ssh_key::PublicKey::new(
        ssh_key::public::KeyData::Rsa(ssh_key::public::RsaPublicKey { e, n }),
        "rsa@example",
    )
    .to_openssh()
    .expect("the assembled key encodes as an OpenSSH public key")
}

// ---------------------------------------------------------------------------
// Rule 1: the verdict a stored SFTP connector's configuration earns.
// ---------------------------------------------------------------------------

/// RULE: `TestConnection` on a stored SFTP connector answers HTTP 200 carrying that
/// connector's own `ConnectorId`, `Status` at the operation's documented FAILURE value
/// (`OK` is a "successful test", which no emulated test is: this emulator opens no
/// outbound connection), a `StatusMessage` diagnosing the first defect the stored SFTP
/// configuration carries — or saying the connection itself was not verified when it
/// carries none — and no `SftpConnectionDetails`, since the remote server's host key is
/// only ever observed on a connection attempt.
///
/// The rows differ along which member is missing or unusable, the form of the missing
/// value (absent, or present but blank or empty), the count and order of trusted host
/// keys, the key type, and the egress type. Each diagnosis is also asserted to be the
/// SAME fixed text for every row in its condition and a DIFFERENT text across
/// conditions, to carry no identifier, secret or key body, and not to change what a
/// later read of the connector sees.
#[tokio::test]
async fn testing_an_sftp_connector_reports_the_verdict_its_configuration_earns() {
    let emulator = Emulator::new();

    let ed25519_one = generated_public_key();
    let ed25519_two = generated_public_key();
    let rsa = rsa_public_key();
    // The line `ssh-keyscan` prints: the key, with the HOST NAME in front of it. The
    // model documents that element as one a caller must not paste into
    // `TrustedHostKeys` ("Specify only the <key type> and <body base64>"), so it is not
    // a key this service can read.
    let keyscan_line = format!("ftp.host.com {ed25519_one}");

    let cases: Vec<(&str, Value, Condition)> = vec![
        (
            "service-managed SFTP, SftpConfig names no UserSecretId at all",
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com",
                "SftpConfig": {"TrustedHostKeys": [ed25519_one.clone()]},
            }),
            Condition::NoSecret,
        ),
        (
            "service-managed SFTP, UserSecretId present but blank: len 1..2048 admits \
             it, and it names no secret",
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com:2222",
                "SftpConfig": {"UserSecretId": " ", "TrustedHostKeys": [rsa.clone()]},
            }),
            Condition::NoSecret,
        ),
        (
            "service-managed SFTP, secret named, TrustedHostKeys absent",
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com",
                "SftpConfig": {"UserSecretId": SECRET},
            }),
            Condition::NoTrustedHostKey,
        ),
        (
            "service-managed SFTP, secret named, TrustedHostKeys the empty list",
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com",
                "SftpConfig": {
                    "UserSecretId": SECRET,
                    "TrustedHostKeys": [],
                    "MaxConcurrentConnections": 5,
                },
            }),
            Condition::NoTrustedHostKey,
        ),
        (
            "service-managed SFTP, one trusted host key, which is an ssh-keyscan line \
             with the host name still in front of it",
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com",
                "SftpConfig": {
                    "UserSecretId": SECRET,
                    "TrustedHostKeys": [keyscan_line.clone()],
                },
            }),
            Condition::UnreadableTrustedHostKey,
        ),
        (
            "service-managed SFTP, two trusted host keys of which the SECOND is not a key",
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com",
                "SftpConfig": {
                    "UserSecretId": SECRET,
                    "TrustedHostKeys": [ed25519_one.clone(), "ssh-ed25519 not-base64!!"],
                },
            }),
            Condition::UnreadableTrustedHostKey,
        ),
        (
            "service-managed SFTP, two trusted host keys of which the FIRST is a key \
             type with a truncated body",
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com",
                "SftpConfig": {
                    "UserSecretId": SECRET,
                    "TrustedHostKeys": [
                        "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ==",
                        ed25519_two.clone(),
                    ],
                },
            }),
            Condition::UnreadableTrustedHostKey,
        ),
        (
            "service-managed SFTP, secret named and ONE readable ssh-ed25519 trusted \
             host key: nothing stored is at fault",
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com",
                "SftpConfig": {
                    "UserSecretId": SECRET,
                    "TrustedHostKeys": [ed25519_one.clone()],
                    "OrderedUserSecretVersionStages": ["AWSCURRENT", "AWSPREVIOUS"],
                },
            }),
            Condition::ConnectionNotVerified,
        ),
        (
            "service-managed SFTP, three readable trusted host keys of two key types",
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com",
                "SecurityPolicyName": "TransferSFTPConnectorSecurityPolicy-2024-03",
                "SftpConfig": {
                    "UserSecretId": SECRET,
                    "TrustedHostKeys": [rsa.clone(), ed25519_one.clone(), ed25519_two.clone()],
                    "MaxConcurrentConnections": 3,
                },
            }),
            Condition::ConnectionNotVerified,
        ),
        (
            "SFTP over VPC Lattice (no Url at all), one readable trusted host key",
            json!({
                "AccessRole": ROLE,
                "SftpConfig": {
                    "UserSecretId": SECRET,
                    "TrustedHostKeys": [ed25519_two.clone()],
                },
                "EgressConfig": {
                    "VpcLattice": {"ResourceConfigurationArn": RESOURCE_CONFIGURATION_ARN},
                },
            }),
            Condition::ConnectionNotVerified,
        ),
        (
            "SFTP over VPC Lattice, secret named, no trusted host key",
            json!({
                "AccessRole": ROLE,
                "SftpConfig": {"UserSecretId": SECRET},
                "EgressConfig": {
                    "VpcLattice": {
                        "ResourceConfigurationArn": RESOURCE_CONFIGURATION_ARN,
                        "PortNumber": 2222,
                    },
                },
            }),
            Condition::NoTrustedHostKey,
        ),
    ];

    // Every value a row sent that must NOT reach the diagnosis: a secret id and the
    // key bodies, which are the caller's own data.
    let request_values = [
        SECRET.to_string(),
        ed25519_one.clone(),
        ed25519_two.clone(),
        rsa.clone(),
        keyscan_line.clone(),
    ];

    let mut diagnoses: Vec<(&str, Condition, String)> = Vec::new();

    for (case, request, condition) in cases {
        let connector_id = create_connector(&emulator, case, request).await;
        let before = describe_connector(&emulator, case, &connector_id).await;

        let answer = emulator
            .call("TestConnection", json!({"ConnectorId": connector_id}))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected TestConnection 200, got {} with body {}",
            answer.status, answer.body
        );

        let reported_id = answer.get("ConnectorId").as_str().unwrap_or_default().to_string();
        assert_eq!(
            reported_id, connector_id,
            "case {case:?}: expected the tested connector's id {connector_id:?}, \
             got {reported_id:?}"
        );

        let status = answer.get("Status").as_str().unwrap_or_default().to_string();
        assert_eq!(
            status, STATUS_ERROR,
            "case {case:?}: expected Status {STATUS_ERROR:?}, got {status:?}"
        );
        assert_ne!(
            status, STATUS_OK,
            "case {case:?}: {STATUS_OK:?} is a successful test, which nothing here \
             performed, got {status:?}"
        );

        assert!(
            answer.get("SftpConnectionDetails").is_null(),
            "case {case:?}: the remote host key is observed on a connection, and none \
             was made, so SftpConnectionDetails must be absent, got {}",
            answer.body
        );

        let message = answer.get("StatusMessage").as_str().unwrap_or_default().to_string();
        assert!(
            !message.is_empty(),
            "case {case:?}: a failed test reports a descriptive StatusMessage, got {}",
            answer.body
        );
        assert!(
            message.contains(condition.names()),
            "case {case:?}: expected the diagnosis to name {:?}, got {message:?}",
            condition.names()
        );
        assert!(
            !message.contains(connector_id.as_str()),
            "case {case:?}: expected no identifier in StatusMessage, got {message:?}"
        );
        for value in &request_values {
            assert!(
                !message.contains(value.as_str()),
                "case {case:?}: expected no request data in StatusMessage, but it \
                 carries {value:?}: {message:?}"
            );
        }

        // A test is a REPORT: it answers the same thing again, and the connector a
        // later read sees is the one it found.
        let again = emulator
            .call("TestConnection", json!({"ConnectorId": connector_id}))
            .await;
        assert_eq!(
            (again.status, &again.body),
            (answer.status, &answer.body),
            "case {case:?}: two tests of one connector must answer the same thing, got \
             {} {} after {} {}",
            again.status,
            again.body,
            answer.status,
            answer.body
        );
        let after = describe_connector(&emulator, case, &connector_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: testing a connector must not change what a later read sees, \
             got {after:?} after {before:?}"
        );

        diagnoses.push((case, condition, message));
    }

    // The text is FIXED per condition: same condition, same words; different
    // condition, different words.
    for (case, condition, message) in &diagnoses {
        for (other_case, other_condition, other_message) in &diagnoses {
            if condition == other_condition {
                assert_eq!(
                    message, other_message,
                    "cases {case:?} and {other_case:?} are both {condition:?}, so they \
                     must report the same diagnosis, got {message:?} and {other_message:?}"
                );
            } else {
                assert_ne!(
                    message, other_message,
                    "case {case:?} is {condition:?} and case {other_case:?} is \
                     {other_condition:?}, so they must not report the same diagnosis, \
                     got {message:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 1, stated over the WORDS of the verdict rather than its condition.
// ---------------------------------------------------------------------------

/// RULE: a `StatusMessage` is the service's own one-line VERDICT on the connection —
/// for a connector that trusts no host key, the recorded `No TrustedHostKeys provided.`
/// — and carries none of the troubleshooting guidance the member's documentation offers
/// the reader of that documentation ("Verify that your secret name aligns ...", "Verify
/// the server URL ...", "Verify that the secret exists ...", "Verify that the trusted
/// host key ... matches the `ssh-keyscan` output"), which is no part of the answer.
///
/// The rows differ along which condition the stored configuration is in and the FORM of
/// the member at fault (absent, blank, the empty list, one unreadable entry among
/// readable ones), so the rule is asserted over the class of verdicts rather than the
/// one recorded row.
#[tokio::test]
async fn a_test_connection_verdict_is_one_line_and_carries_no_troubleshooting_guidance() {
    let emulator = Emulator::new();

    let ed25519 = generated_public_key();

    // The sentences the member's documentation offers a reader
    // (`smithy_query(query="doc",shape="TestConnectionResponse",member="StatusMessage")`),
    // none of which the service answers.
    let guidance = [
        "Verify that your secret name aligns",
        "Verify the server URL",
        "Verify that the secret exists",
        "Verify that the trusted host key",
        "ssh-keyscan",
    ];

    // (case, the connector's SftpConfig, the verdict the service states)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "no TrustedHostKeys member at all",
            json!({"UserSecretId": SECRET}),
            "No TrustedHostKeys provided.",
        ),
        (
            "TrustedHostKeys the empty list, at the low bound of its modelled len 0..10",
            json!({"UserSecretId": SECRET, "TrustedHostKeys": []}),
            "No TrustedHostKeys provided.",
        ),
        (
            "no UserSecretId member at all",
            json!({"TrustedHostKeys": [ed25519.clone()]}),
            "No UserSecretId provided.",
        ),
        (
            "UserSecretId present but blank, which names no secret",
            json!({"UserSecretId": " ", "TrustedHostKeys": [ed25519.clone()]}),
            "No UserSecretId provided.",
        ),
    ];

    for (case, sftp_config, expected) in cases {
        let connector_id = create_connector(
            &emulator,
            case,
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com",
                "SftpConfig": sftp_config,
            }),
        )
        .await;

        let answer = emulator
            .call("TestConnection", json!({"ConnectorId": connector_id}))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected TestConnection 200, got {} with body {}",
            answer.status, answer.body
        );
        let message = answer.get("StatusMessage").as_str().unwrap_or_default().to_string();
        assert_eq!(
            message, expected,
            "case {case:?}: expected the verdict {expected:?}, got {message:?}"
        );
    }

    // Every verdict this operation states, from every condition reachable without a
    // server to connect to, is ONE line and carries no guidance sentence.
    let probes: Vec<(&str, Value)> = vec![
        (
            "a configuration that trusts no host key",
            json!({"UserSecretId": SECRET}),
        ),
        ("a configuration with no secret", json!({})),
        (
            "a trusted host key that is not an SSH public key",
            json!({"UserSecretId": SECRET, "TrustedHostKeys": ["ssh-ed25519 not-base64!!"]}),
        ),
        (
            "a configuration with nothing at fault, whose endpoint is no server of this \
             emulator",
            json!({"UserSecretId": SECRET, "TrustedHostKeys": [ed25519.clone()]}),
        ),
    ];

    for (case, sftp_config) in probes {
        let connector_id = create_connector(
            &emulator,
            case,
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com",
                "SftpConfig": sftp_config,
            }),
        )
        .await;
        let answer = emulator
            .call("TestConnection", json!({"ConnectorId": connector_id}))
            .await;
        let message = answer.get("StatusMessage").as_str().unwrap_or_default().to_string();
        assert!(
            !message.is_empty() && !message.contains('\n'),
            "case {case:?}: expected a one-line verdict, got {message:?}"
        );
        assert_eq!(
            message.trim(),
            message,
            "case {case:?}: expected no surrounding whitespace in the verdict, got \
             {message:?}"
        );
        for sentence in guidance {
            assert!(
                !message.contains(sentence),
                "case {case:?}: expected no troubleshooting guidance in the verdict, but \
                 it carries {sentence:?}: {message:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a connector that configures no SFTP connection.
// ---------------------------------------------------------------------------

/// RULE: `TestConnection` tests an SFTP connection, so a connector that configures none
/// — an AS2 one, since `CreateConnector` takes "exactly one configuration object:
/// either for AS2 (`As2Config`) or SFTP (`SftpConfig`)" — has nothing for the call to
/// test: HTTP 400 `InvalidRequestException` whose `Message` names the member at fault,
/// and no verdict at all.
///
/// The rows differ along how much of the AS2 configuration is supplied and the URL
/// scheme it points at; the refusal is the connector's PROTOCOL, so nothing else about
/// the request can change it.
#[tokio::test]
async fn testing_a_connector_that_configures_no_sftp_connection_is_refused() {
    let emulator = Emulator::new();

    let cases: Vec<(&str, Value)> = vec![
        (
            "AS2 with the two profiles and nothing beyond the members an AS2 \
             configuration is required to settle",
            json!({
                "AccessRole": ROLE,
                "Url": "https://partner.example.com/as2",
                "As2Config": {
                    "LocalProfileId": "p-0123456789abcdef0",
                    "PartnerProfileId": "p-00000000000000001",
                    "Compression": "DISABLED",
                    "EncryptionAlgorithm": "NONE",
                    "SigningAlgorithm": "NONE",
                },
            }),
        ),
        (
            "AS2 over plain HTTP with the full message configuration and a tag",
            json!({
                "AccessRole": ROLE,
                "Url": "http://partner.example.com:8080/as2",
                "As2Config": {
                    "LocalProfileId": "p-0123456789abcdef0",
                    "PartnerProfileId": "p-00000000000000001",
                    "Compression": "ZLIB",
                    "EncryptionAlgorithm": "AES256_CBC",
                    "SigningAlgorithm": "SHA384",
                    "MdnResponse": "SYNC",
                },
                "Tags": [{"Key": "team", "Value": "edi"}],
            }),
        ),
    ];

    for (case, request) in cases {
        let connector_id = create_connector(&emulator, case, request).await;

        let answer = emulator
            .call("TestConnection", json!({"ConnectorId": connector_id}))
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
            message.contains("SftpConfig"),
            "case {case:?}: expected the refusal to name SftpConfig, got {message:?}"
        );
        assert!(
            answer.get("Status").is_null() && answer.get("StatusMessage").is_null(),
            "case {case:?}: a refused test reports no verdict, got {}",
            answer.body
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a connector the store does not hold.
// ---------------------------------------------------------------------------

/// RULE: `TestConnection` for a syntactically valid `ConnectorId` the store does not
/// hold answers this service's generic not-found error — HTTP 400
/// `ResourceNotFoundException`, `ResourceType` `Connector`, `Resource` the id the
/// request named, and a fixed `Message` carrying no identifier — rather than a failed
/// test of a connector that does not exist.
///
/// The rows are ids from the low end of the id space, a mid value, the high end, and one
/// that neighbours an id that WAS minted.
#[tokio::test]
async fn testing_a_connector_that_was_never_created_is_not_found() {
    let emulator = Emulator::new();

    let minted = create_connector(
        &emulator,
        "one connector the store does hold",
        json!({
            "AccessRole": ROLE,
            "Url": "sftp://partner.example.com",
            "SftpConfig": {"UserSecretId": SECRET},
        }),
    )
    .await;
    let sequence = u64::from_str_radix(&minted[2..], 16).expect("a minted id ends in 17 hex digits");
    let neighbour = format!("c-{:017x}", sequence + 1);

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
            .call("TestConnection", json!({"ConnectorId": connector_id}))
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
        let resource_type = member_of(&answer.body, "ResourceType")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            resource_type, "Connector",
            "case {connector_id:?}: expected ResourceType Connector, got {resource_type:?}"
        );
        let resource = member_of(&answer.body, "Resource")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            &resource, connector_id,
            "case {connector_id:?}: expected Resource to be the id the request named, \
             got {resource:?}"
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
