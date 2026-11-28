// SPDX-License-Identifier: CC0-1.0

//! `DescribeInstance` end to end over the `awsJson1_1` wire.
//!
//! The rule under test: `DescribeInstance` REPORTS the IAM Identity Center instance its
//! `InstanceArn` names, from the same records `ListInstances` enumerates — every member it
//! shares with `InstanceMetadata` carries the stored value, it reports no member the
//! instance has no value for, and it writes nothing; an `InstanceArn` this account holds
//! no instance for is refused with the modelled `AccessDeniedException` (403), the only
//! not-found channel the operation's model declares.
//!
//! The table spans the class the rule generalises over: the ARN that resolves, and ARNs
//! that do not — in each spelling the model's `InstanceArn` pattern
//! `^arn:aws(-[a-z]{1,5}){0,3}:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$` admits (the
//! `ins-` and `ssoins-` prefixes, each partition suffix the pattern allows, and the full
//! `[a-zA-Z0-9-.]` id character class), plus the held instance's id under another
//! partition, which is a DIFFERENT instance.
//!
//! `NetworkConfigurationDetails` has a rule of its own, asserted over its own table below:
//! the member is "Present only after a network configuration has been set on the instance",
//! and what it reports is the configuration the last accepted `UpdateInstance` stated —
//! replaced "in its entirety", with an omitted or empty list cleared and every list cleared
//! when `VpceAccessRequired` is `false`.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints and
//! the `X-Amz-Target` routing are exercised rather than bypassed. The harness is the one
//! `ssoadmin_permission_sets.rs` owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call, error_code, is_instance_arn, router, seeded_instance_arn, UNKNOWN_INSTANCE_ARN,
};

/// What a row expects of `DescribeInstance`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The ARN names an instance this account holds: 200, and the stored record.
    Reported,
    /// No instance of this account has that ARN: 403 `AccessDeniedException`.
    Denied,
}

/// One row: a label, the `InstanceArn` to describe, and what must come back.
struct Case {
    label: &'static str,
    /// `None` means "the ARN of the seeded instance", read at run time.
    instance_arn: Option<&'static str>,
    expected: Expected,
}

/// The rows differ along what the rule generalises over: whether the ARN resolves, the
/// `(sso)?ins-` prefix spelling, the partition segment the pattern admits, and the id's
/// character class.
const CASES: &[Case] = &[
    Case {
        label: "the ARN ListInstances reported",
        instance_arn: None,
        expected: Expected::Reported,
    },
    Case {
        label: "a pattern-valid ssoins- ARN that was never created",
        instance_arn: Some(UNKNOWN_INSTANCE_ARN),
        expected: Expected::Denied,
    },
    Case {
        // The pattern's `(sso)?` group makes the bare `ins-` spelling legal input.
        label: "the bare ins- spelling",
        instance_arn: Some("arn:aws:sso:::instance/ins-abcdefghij123456"),
        expected: Expected::Denied,
    },
    Case {
        // The id class is `[a-zA-Z0-9-.]`: mixed case, a hyphen and a dot are all legal.
        label: "an id using the full [a-zA-Z0-9-.] class",
        instance_arn: Some("arn:aws:sso:::instance/ssoins-Ab.9-cDeF0123456"),
        expected: Expected::Denied,
    },
    Case {
        // `aws(-[a-z]{1,5}){0,3}`: another partition is a legal ARN and a different
        // instance.
        label: "a GovCloud partition ARN",
        instance_arn: Some("arn:aws-us-gov:sso:::instance/ssoins-1111111111111111"),
        expected: Expected::Denied,
    },
    Case {
        label: "a China partition ARN",
        instance_arn: Some("arn:aws-cn:sso:::instance/ssoins-2222222222222222"),
        expected: Expected::Denied,
    },
];

/// The `InstanceMetadata` `ListInstances` reports for `arn`.
async fn listed_instance(router: &MultiServiceRouter, arn: &str) -> Value {
    let (status, body) = call(router, "ListInstances", &json!({})).await;
    assert_eq!(status, 200, "ListInstances expected 200, got {status} {body}");
    body["Instances"]
        .as_array()
        .unwrap_or_else(|| panic!("ListInstances returned no Instances: {body}"))
        .iter()
        .find(|instance| instance["InstanceArn"].as_str() == Some(arn))
        .unwrap_or_else(|| panic!("ListInstances does not list {arn}: {body}"))
        .clone()
}

/// Whether `arn` has the shape the model's `InstanceIdentityStoreArn` pattern requires:
/// `arn:aws…:identitystore::<12 digits>:identitystore/` then `d-` and 10 hex digits (or
/// a hyphenated UUID).
fn is_identity_store_arn(arn: &str) -> bool {
    let Some((prefix, id)) = arn.split_once(":identitystore/") else {
        return false;
    };
    // `arn:<partition>:identitystore::<account>` — the identity-store ARN carries no
    // region, so the account id follows the empty region segment.
    let Some(account) = prefix.split(":identitystore::").nth(1) else {
        return false;
    };
    let directory = id
        .strip_prefix("d-")
        .is_some_and(|rest| rest.len() == 10 && rest.chars().all(|c| c.is_ascii_hexdigit()));
    let uuid = id.split('-').map(str::len).eq([8usize, 4, 4, 4, 12]);
    prefix.starts_with("arn:aws")
        && account.len() == 12
        && account.chars().all(|c| c.is_ascii_digit())
        && (directory || uuid)
        && arn.len() >= 62
        && arn.len() <= 93
}

/// The rule, over the whole table: an ARN this account holds is reported from the stored
/// record and agrees with `ListInstances` member for member; any other pattern-valid ARN
/// is refused `AccessDeniedException` with HTTP 403.
#[tokio::test]
async fn describe_instance_reports_the_stored_instance_and_denies_every_other_arn() {
    let router = router();
    let seeded = seeded_instance_arn(&router).await;
    assert!(
        is_instance_arn(&seeded),
        "the seeded instance must carry a modelled InstanceArn, got {seeded:?}"
    );

    for case in CASES {
        let arn = case.instance_arn.unwrap_or(seeded.as_str());
        let (status, body) = call(&router, "DescribeInstance", &json!({"InstanceArn": arn})).await;

        match case.expected {
            Expected::Denied => {
                assert_eq!(
                    status, 403,
                    "case {:?}: expected HTTP 403 for {arn}, got {status} {body}",
                    case.label
                );
                assert_eq!(
                    error_code(&body),
                    "AccessDeniedException",
                    "case {:?}: expected AccessDeniedException for {arn}, got {body}",
                    case.label
                );
                continue;
            }
            Expected::Reported => {
                assert_eq!(
                    status, 200,
                    "case {:?}: expected HTTP 200 for {arn}, got {status} {body}",
                    case.label
                );
            }
        }

        let listed = listed_instance(&router, arn).await;
        // Every member `DescribeInstance` shares with `InstanceMetadata` must carry the
        // SAME stored value: one record, two projections.
        for member in [
            "InstanceArn",
            "IdentityStoreId",
            "IdentityStoreArn",
            "OwnerAccountId",
            "Name",
            "CreatedDate",
            "Status",
            "PrimaryRegion",
            "Regions",
        ] {
            assert_eq!(
                body[member], listed[member],
                "case {:?}: member {member} must match the listed instance, got {:?} vs {:?}",
                case.label, body[member], listed[member]
            );
        }
        assert_eq!(
            body["Status"].as_str(),
            Some("ACTIVE"),
            "case {:?}: a seeded instance is ACTIVE, got {:?}",
            case.label,
            body["Status"]
        );
        assert!(
            is_identity_store_arn(body["IdentityStoreArn"].as_str().unwrap_or_default()),
            "case {:?}: expected a modelled IdentityStoreArn, got {:?}",
            case.label,
            body["IdentityStoreArn"]
        );
        assert_eq!(
            body["OwnerAccountId"].as_str(),
            Some(engine::EMULATOR_ACCOUNT_ID),
            "case {:?}: expected the emulator's account id, got {:?}",
            case.label,
            body["OwnerAccountId"]
        );
        assert_eq!(
            body["PermissionSetsEnabled"],
            Value::Bool(false),
            "case {:?}: the instance a client discovers through ListInstances is an \
             account instance, which serves no multi-account permissions, so it must \
             report permission sets disabled, got {:?}",
            case.label,
            body["PermissionSetsEnabled"]
        );
        // An ACTIVE instance has no additional status context, so that member is not
        // invented.
        assert_eq!(
            body["StatusReason"],
            Value::Null,
            "case {:?}: an ACTIVE instance must report no StatusReason, got {:?}",
            case.label,
            body["StatusReason"]
        );
        // Every instance IS encrypted, so its configuration is always reported: nothing
        // has opted this one into a key of its own, so it is the Amazon Web Services
        // owned default, in effect, with no ARN naming it.
        let details = body["EncryptionConfigurationDetails"].clone();
        assert_eq!(
            details["KeyType"].as_str(),
            Some("AWS_OWNED_KMS_KEY"),
            "case {:?}: an instance nobody has reconfigured is encrypted under the AWS \
             owned key, got {details:?}",
            case.label
        );
        assert_eq!(
            details["EncryptionStatus"].as_str(),
            Some("ENABLED"),
            "case {:?}: that default is already in effect, so encryption is enabled, \
             got {details:?}",
            case.label
        );
        assert_eq!(
            details["KmsKeyArn"],
            Value::Null,
            "case {:?}: no ARN names an AWS owned key, got {details:?}",
            case.label
        );
        assert_eq!(
            details["EncryptionStatusReason"],
            Value::Null,
            "case {:?}: nothing failed, so no EncryptionStatusReason may be reported, \
             got {details:?}",
            case.label
        );

        // The primary Region is one of the enabled Regions, and it is the one flagged
        // primary.
        let regions = body["Regions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {:?}: no Regions in {body}", case.label));
        let primary: Vec<&Value> = regions
            .iter()
            .filter(|region| region["IsPrimaryRegion"] == Value::Bool(true))
            .collect();
        assert_eq!(
            primary.len(),
            1,
            "case {:?}: exactly one Region is primary, got {regions:?}",
            case.label
        );
        assert_eq!(
            primary[0]["RegionName"], body["PrimaryRegion"],
            "case {:?}: the primary Region must be the one PrimaryRegion names, got {:?} \
             vs {:?}",
            case.label, primary[0]["RegionName"], body["PrimaryRegion"]
        );
    }
}

/// `DescribeInstance` is a READ: describing an instance twice reports the identical
/// record, and it changes nothing a later caller sees.
#[tokio::test]
async fn describing_an_instance_changes_nothing() {
    let router = router();
    let arn = seeded_instance_arn(&router).await;

    let (status, before) = call(&router, "ListInstances", &json!({})).await;
    assert_eq!(status, 200, "ListInstances expected 200, got {status} {before}");

    let (status, first) = call(&router, "DescribeInstance", &json!({"InstanceArn": arn})).await;
    assert_eq!(status, 200, "DescribeInstance expected 200, got {status} {first}");
    let (status, second) = call(&router, "DescribeInstance", &json!({"InstanceArn": arn})).await;
    assert_eq!(status, 200, "DescribeInstance expected 200, got {status} {second}");
    assert_eq!(
        first, second,
        "two reads of one instance must report the identical record, got {first} then {second}"
    );

    let (status, after) = call(&router, "ListInstances", &json!({})).await;
    assert_eq!(status, 200, "ListInstances expected 200, got {status} {after}");
    assert_eq!(
        before, after,
        "a read must not change what a later caller sees, got {before} then {after}"
    );
}

/// An `InstanceArn` outside the model's pattern is a BAD REQUEST, not a missing
/// resource: the framework refuses it before the handler runs, with the service's
/// `ValidationException` (400). The rows span the ways the pattern and its 10..1224
/// length can be missed.
#[tokio::test]
async fn an_instance_arn_outside_the_pattern_is_a_validation_error() {
    let router = router();

    // (case, InstanceArn)
    let rows: [(&str, &str); 5] = [
        ("empty", ""),
        ("not an arn at all", "ssoins-7223b1d1ac5e1f0e"),
        ("another service's arn", "arn:aws:iam::123456789012:role/demo"),
        // The id must be exactly 16 characters of `[a-zA-Z0-9-.]`.
        ("an id one character short", "arn:aws:sso:::instance/ssoins-000000000000000"),
        ("an id one character long", "arn:aws:sso:::instance/ssoins-00000000000000000"),
    ];

    for (case, arn) in rows {
        let (status, body) =
            call(&router, "DescribeInstance", &json!({"InstanceArn": arn})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {arn:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException for {arn:?}, got {body}"
        );
    }
}

/// A VPC ARN in each spelling the model's `VpcArn` pattern admits
/// (`^arn:aws(-[a-z]{1,5}){0,3}:ec2:[a-z0-9-]+:[0-9]{12}:vpc/vpc-([0-9a-f]){8}(([0-9a-f]){9})?$`):
/// the 8-hex id in the `aws` partition, and the 17-hex id under another partition.
const VPC_ARN_SHORT: &str = "arn:aws:ec2:us-east-1:123456789012:vpc/vpc-0a1b2c3d";
const VPC_ARN_LONG: &str =
    "arn:aws-us-gov:ec2:us-gov-west-1:123456789012:vpc/vpc-0a1b2c3d4e5f6a7b8";

/// An application ARN of the form the model's `FirstOrThirdPartyApplicationArnType`
/// pattern admits (`^arn:aws(-[a-z]{1,5}){0,3}:[a-z0-9-]+:[a-z0-9-*]*:[0-9]{12}:[-a-zA-Z0-9./:_*]+$`).
const APPLICATION_ARN: &str =
    "arn:aws:sso::123456789012:application/ssoins-1234567890abcdef/apl-1234567890abcdef";

/// The `NetworkConfigurationDetails` the described instance reports, or `Value::Null` when
/// it reports none.
async fn network_details(router: &MultiServiceRouter, arn: &str) -> Value {
    let (status, body) = call(router, "DescribeInstance", &json!({"InstanceArn": arn})).await;
    assert_eq!(
        status, 200,
        "DescribeInstance expected 200 for {arn}, got {status} {body}"
    );
    body["NetworkConfigurationDetails"].clone()
}

/// `UpdateInstance` with `NetworkConfiguration` set to `configuration`, which must be
/// accepted.
async fn configure_network(router: &MultiServiceRouter, arn: &str, configuration: &Value) {
    let (status, body) = call(
        router,
        "UpdateInstance",
        &json!({"InstanceArn": arn, "NetworkConfiguration": configuration}),
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdateInstance expected 200 for {configuration}, got {status} {body}"
    );
}

/// THE RULE for `NetworkConfigurationDetails`: it is absent until a network configuration
/// has been set, and from then on it reports the configuration the last accepted
/// `UpdateInstance` stated — the stored record replaced whole, with the clearing rules the
/// members document applied.
///
/// The rows differ along what the rule generalises over: whether VPC endpoint access is
/// required at all, WHICH of the three control lists the configuration carries, how many
/// entries each list holds, the value spellings each list's member pattern admits (both
/// `VpcArn` id lengths, an IPv4 address, an IPv4 CIDR block and an IPv6 CIDR block), and a
/// list stated EMPTY. Each row's expectation is absolute rather than cumulative, so a
/// configuration that merged into the stored one instead of replacing it fails here.
#[tokio::test]
async fn the_reported_network_configuration_is_the_one_the_last_update_stated() {
    let router = router();
    let arn = seeded_instance_arn(&router).await;

    // "Present only after a network configuration has been set on the instance."
    assert_eq!(
        network_details(&router, &arn).await,
        Value::Null,
        "an instance nobody has configured network access controls on must report no \
         NetworkConfigurationDetails"
    );

    // (case, the NetworkConfiguration stated, the details the instance must then report)
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "VPC endpoint access required, no list at all",
            json!({"VpceAccessRequired": true}),
            json!({"VpceAccessRequired": true}),
        ),
        (
            "one VPC permitted",
            json!({"VpceAccessRequired": true, "RestrictSourceVpcs": [VPC_ARN_SHORT]}),
            json!({"VpceAccessRequired": true, "RestrictSourceVpcs": [VPC_ARN_SHORT]}),
        ),
        (
            "two VPCs permitted, in both id spellings",
            json!({
                "VpceAccessRequired": true,
                "RestrictSourceVpcs": [VPC_ARN_SHORT, VPC_ARN_LONG],
            }),
            json!({
                "VpceAccessRequired": true,
                "RestrictSourceVpcs": [VPC_ARN_SHORT, VPC_ARN_LONG],
            }),
        ),
        (
            "an IPv4 address, an IPv4 block and an IPv6 block allowed",
            json!({
                "VpceAccessRequired": true,
                "AllowSourceIps": ["198.51.100.7", "203.0.113.0/24", "2001:db8::/32"],
            }),
            json!({
                "VpceAccessRequired": true,
                "AllowSourceIps": ["198.51.100.7", "203.0.113.0/24", "2001:db8::/32"],
            }),
        ),
        (
            "one application exempted",
            json!({"VpceAccessRequired": true, "ExcludeApplicationArns": [APPLICATION_ARN]}),
            json!({"VpceAccessRequired": true, "ExcludeApplicationArns": [APPLICATION_ARN]}),
        ),
        (
            "every control at once",
            json!({
                "VpceAccessRequired": true,
                "RestrictSourceVpcs": [VPC_ARN_LONG],
                "AllowSourceIps": ["203.0.113.0/24"],
                "ExcludeApplicationArns": [APPLICATION_ARN],
            }),
            json!({
                "VpceAccessRequired": true,
                "RestrictSourceVpcs": [VPC_ARN_LONG],
                "AllowSourceIps": ["203.0.113.0/24"],
                "ExcludeApplicationArns": [APPLICATION_ARN],
            }),
        ),
        (
            // "Any list that you omit from the object, or include as an empty list, is
            // cleared."
            "lists stated empty are cleared",
            json!({
                "VpceAccessRequired": true,
                "RestrictSourceVpcs": [],
                "AllowSourceIps": [],
                "ExcludeApplicationArns": [],
            }),
            json!({"VpceAccessRequired": true}),
        ),
        (
            // "When this value is false, no network access control is enforced and the
            // values of RestrictSourceVpcs, AllowSourceIps, and ExcludeApplicationArns are
            // cleared."
            "no network access control enforced",
            json!({
                "VpceAccessRequired": false,
                "RestrictSourceVpcs": [VPC_ARN_SHORT],
                "AllowSourceIps": ["203.0.113.0/24"],
                "ExcludeApplicationArns": [APPLICATION_ARN],
            }),
            json!({"VpceAccessRequired": false}),
        ),
    ];

    for (case, configuration, expected) in rows {
        configure_network(&router, &arn, &configuration).await;
        let reported = network_details(&router, &arn).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the stated configuration {expected}, got {reported}"
        );
        // A read reports rather than changes: the next read answers the same thing.
        let reread = network_details(&router, &arn).await;
        assert_eq!(
            reread, expected,
            "case {case:?}: a second read must report the same configuration, got {reread}"
        );
    }
}

/// THE RULE where the request does NOT state a network configuration: "If you omit
/// NetworkConfiguration, the stored network configuration is preserved" — and a request
/// that states it beside another of the operation's one-of members is refused and writes
/// nothing.
#[tokio::test]
async fn an_omitted_or_refused_network_configuration_leaves_the_stored_one_alone() {
    let router = router();
    let arn = seeded_instance_arn(&router).await;

    let stored = json!({
        "VpceAccessRequired": true,
        "RestrictSourceVpcs": [VPC_ARN_SHORT],
        "AllowSourceIps": ["203.0.113.0/24"],
    });
    configure_network(&router, &arn, &stored).await;
    assert_eq!(
        network_details(&router, &arn).await,
        stored,
        "the configuration under test must be the stored one"
    );

    // (case, the request body, the expected HTTP status, the expected error code)
    let rows: Vec<(&str, Value, u16, Option<&str>)> = vec![
        (
            "only the InstanceArn",
            json!({"InstanceArn": arn}),
            200,
            None,
        ),
        (
            // `Name` is not one of the one-of members, so it travels with anything.
            "a name update, which names no network configuration",
            json!({"InstanceArn": arn, "Name": "NetworkControlled"}),
            200,
            None,
        ),
        (
            "a network configuration beside PermissionSetsEnabled",
            json!({
                "InstanceArn": arn,
                "PermissionSetsEnabled": true,
                "NetworkConfiguration": {"VpceAccessRequired": false},
            }),
            400,
            Some("ValidationException"),
        ),
        (
            "a network configuration beside EncryptionConfiguration",
            json!({
                "InstanceArn": arn,
                "EncryptionConfiguration": {"KeyType": "AWS_OWNED_KMS_KEY"},
                "NetworkConfiguration": {"VpceAccessRequired": false},
            }),
            400,
            Some("ValidationException"),
        ),
        (
            "all three one-of members at once",
            json!({
                "InstanceArn": arn,
                "PermissionSetsEnabled": true,
                "EncryptionConfiguration": {"KeyType": "AWS_OWNED_KMS_KEY"},
                "NetworkConfiguration": {"VpceAccessRequired": true},
            }),
            400,
            Some("ValidationException"),
        ),
        (
            // `VpcArnList`'s member pattern admits no such value, so the boundary refuses
            // the request before the handler runs — and nothing is written either way.
            "a VPC that is not a VPC ARN",
            json!({
                "InstanceArn": arn,
                "NetworkConfiguration": {
                    "VpceAccessRequired": true,
                    "RestrictSourceVpcs": ["vpc-0a1b2c3d"],
                },
            }),
            400,
            Some("ValidationException"),
        ),
    ];

    for (case, request, expected_status, expected_code) in rows {
        let (status, body) = call(&router, "UpdateInstance", &request).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        if let Some(expected_code) = expected_code {
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected {expected_code}, got {body}"
            );
        }
        let reported = network_details(&router, &arn).await;
        assert_eq!(
            reported, stored,
            "case {case:?}: the stored network configuration must be preserved, got \
             {reported} vs {stored}"
        );
    }
}
