// SPDX-License-Identifier: CC0-1.0

//! The account's own usage and quota counters, end to end over the awsQuery wire:
//! `GetAccountSummary` and the one preference `SetSecurityTokenServicePreferences` writes
//! into it.
//!
//! The rules under test:
//!
//! * **the account summary is AGGREGATED from the account's own records**, so every count
//!   it reports is the collection a list operation pages over — creating an entity moves
//!   its count by one and nothing else — and every quota it reports is the figure the
//!   operation that enforces that quota refuses past;
//! * **it is a REPORT**: two calls with no write between them answer the same pairs, so
//!   reading the summary changes nothing a later caller sees;
//! * **the global endpoint token version the summary reports is what
//!   `SetSecurityTokenServicePreferences` last set**, which is the member's own
//!   documentation ("to view the current session token version, see the
//!   GlobalEndpointTokenVersion entry in the response of the GetAccountSummary
//!   operation"), and a repeat set of the same version leaves the account on it.
//!
//! The rows differ along what those rules generalise over: which entity kind is created,
//! how many of them, which quota is read, and which token version is set in which order.
//!
//! Every call goes over the wire IAM speaks, through the `MultiServiceRouter` harness
//! `tests/all/user_lifecycle.rs` owns, so what is asserted is the HTTP behaviour a client
//! on the socket sees.

use std::collections::BTreeMap;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{call, elements, error_code, router};

/// A trust policy with the one element a trust document's statements must name, for the
/// roles the counting rows create.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// An identity policy document the managed-policy writes accept.
const IDENTITY_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// The `SummaryMap` the response carries, read as the pairs a client looks keys up in.
///
/// awsQuery renders a map as `<entry><key>…</key><value>…</value></entry>`, so the keys
/// and the values are read as two leaf sequences and zipped — the order they are written
/// in pairs them.
async fn summary(router: &MultiServiceRouter) -> BTreeMap<String, i64> {
    let (status, body) = call(router, "GetAccountSummary", &[]).await;
    assert_eq!(
        status, 200,
        "GetAccountSummary must be served, got {status}: {body}"
    );
    let keys = elements(&body, "key");
    let values = elements(&body, "value");
    assert_eq!(
        keys.len(),
        values.len(),
        "every summary key must carry a value, got {keys:?} and {values:?}"
    );
    assert!(
        !keys.is_empty(),
        "the summary must report the account's counters, got {body}"
    );
    keys.into_iter()
        .zip(values)
        .map(|(key, value)| {
            let parsed = value.parse::<i64>().unwrap_or_else(|_| {
                panic!("the summary value of {key:?} must be an integer, got {value:?}")
            });
            (key, parsed)
        })
        .collect()
}

/// One summary entry, which the account must report.
fn entry(summary: &BTreeMap<String, i64>, key: &str) -> i64 {
    *summary
        .get(key)
        .unwrap_or_else(|| panic!("the summary must report {key:?}, got {summary:?}"))
}

/// RULE: the summary reports the keys the model declares, and nothing outside them.
#[tokio::test]
async fn every_reported_key_is_one_the_model_declares() {
    let router = router();
    let reported = summary(&router).await;
    // SOURCE for the published key set: `summaryKeyType`'s own values
    // (`smithy_query(query="shape", shape="summaryKeyType")`), which the service's state
    // module names one by one.
    for key in reported.keys() {
        assert!(
            SUMMARY_KEY_TYPE_VALUES.contains(&key.as_str()),
            "the summary reported {key:?}, which summaryKeyType does not declare"
        );
    }
    // A key the issue did not cite, read to show the catalog is the model's and not the
    // cited example's: the account's own role quota.
    assert_eq!(
        entry(&reported, "RolesQuota"),
        i64::try_from(state::MAX_ROLES_PER_ACCOUNT).expect("the quota fits"),
        "the reported role quota must be the one CreateRole refuses past"
    );
}

/// The values `summaryKeyType` declares, in the model's own order
/// (`smithy_query(query="shape", shape="summaryKeyType")`). Written out here because the
/// enum has no generated value list of its own in this crate: the summary's keys are the
/// map's KEY shape, not a member.
const SUMMARY_KEY_TYPE_VALUES: &[&str] = &[
    "Users",
    "UsersQuota",
    "Groups",
    "GroupsQuota",
    "ServerCertificates",
    "ServerCertificatesQuota",
    "UserPolicySizeQuota",
    "GroupPolicySizeQuota",
    "GroupsPerUserQuota",
    "SigningCertificatesPerUserQuota",
    "AccessKeysPerUserQuota",
    "MFADevices",
    "MFADevicesInUse",
    "AccountMFAEnabled",
    "AccountAccessKeysPresent",
    "AccountPasswordPresent",
    "AccountSigningCertificatesPresent",
    "AttachedPoliciesPerGroupQuota",
    "AttachedPoliciesPerRoleQuota",
    "AttachedPoliciesPerUserQuota",
    "Policies",
    "PoliciesQuota",
    "PolicySizeQuota",
    "PolicyVersionsInUse",
    "PolicyVersionsInUseQuota",
    "VersionsPerPolicyQuota",
    "GlobalEndpointTokenVersion",
    "AssumeRolePolicySizeQuota",
    "InstanceProfiles",
    "InstanceProfilesQuota",
    "Providers",
    "RolePolicySizeQuota",
    "Roles",
    "RolesQuota",
];

/// One counting row: the entity kind to create, the summary key that counts it, and how
/// the create is driven.
struct Counted {
    case: &'static str,
    key: &'static str,
    action: &'static str,
    /// The create's parameters, as a function of the entity's name.
    params: fn(&str) -> Vec<(String, String)>,
    /// The other keys this kind's create moves by the same step, because the entity it
    /// creates IS one of them: a managed policy carries its first version, so creating one
    /// moves the account's policy-version count too.
    also_moves: &'static [&'static str],
}

/// Rows spanning the entity kinds the summary counts, each created through its own
/// operation. They differ in the create's shape (a name alone, a name with a document) and
/// in how many the row creates.
const COUNTED: &[Counted] = &[
    Counted {
        case: "users",
        key: "Users",
        action: "CreateUser",
        params: |name| vec![("UserName".to_string(), name.to_string())],
        also_moves: &[],
    },
    Counted {
        case: "groups",
        key: "Groups",
        action: "CreateGroup",
        params: |name| vec![("GroupName".to_string(), name.to_string())],
        also_moves: &[],
    },
    Counted {
        case: "roles",
        key: "Roles",
        action: "CreateRole",
        params: |name| {
            vec![
                ("RoleName".to_string(), name.to_string()),
                (
                    "AssumeRolePolicyDocument".to_string(),
                    TRUST_POLICY.to_string(),
                ),
            ]
        },
        also_moves: &[],
    },
    Counted {
        case: "customer managed policies",
        key: "Policies",
        action: "CreatePolicy",
        params: |name| {
            vec![
                ("PolicyName".to_string(), name.to_string()),
                ("PolicyDocument".to_string(), IDENTITY_POLICY.to_string()),
            ]
        },
        also_moves: &["PolicyVersionsInUse"],
    },
    Counted {
        case: "instance profiles",
        key: "InstanceProfiles",
        action: "CreateInstanceProfile",
        params: |name| vec![("InstanceProfileName".to_string(), name.to_string())],
        also_moves: &[],
    },
    Counted {
        case: "virtual MFA devices",
        key: "MFADevices",
        action: "CreateVirtualMFADevice",
        params: |name| vec![("VirtualMFADeviceName".to_string(), name.to_string())],
        also_moves: &[],
    },
];

/// RULE: every count the summary reports is aggregated from the account's own records, so
/// creating N entities of a kind moves that kind's count by N and leaves the other counts
/// where they were.
#[tokio::test]
async fn a_count_follows_the_entities_the_account_holds() {
    // Rows differ in HOW MANY are created, so the rule is asserted over a step of one and
    // a step of several rather than over a single create.
    for created in [1_usize, 3] {
        for row in COUNTED {
            let router = router();
            let before = summary(&router).await;
            for index in 0..created {
                let name = format!("summary-{}-{index}", row.key.to_ascii_lowercase());
                let owned = (row.params)(&name);
                let params: Vec<(&str, &str)> = owned
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_str()))
                    .collect();
                let (status, body) = call(&router, row.action, &params).await;
                assert_eq!(
                    status, 200,
                    "case {:?}: the fixture {} must succeed, got {status}: {body}",
                    row.case, row.action
                );
            }
            let after = summary(&router).await;
            let step = i64::try_from(created).expect("the count fits");
            for key in ::std::iter::once(row.key).chain(row.also_moves.iter().copied()) {
                assert_eq!(
                    entry(&after, key),
                    entry(&before, key) + step,
                    "case {:?}: creating {created} must move {key} by {created}",
                    row.case
                );
            }
            // Every OTHER key the summary reports is untouched by this kind's creates,
            // which is what shows each count is read off its own collection.
            for (key, value) in &before {
                if key == row.key || row.also_moves.contains(&key.as_str()) {
                    continue;
                }
                assert_eq!(
                    entry(&after, key),
                    *value,
                    "case {:?}: creating {} must leave {key} where it was",
                    row.case,
                    row.key
                );
            }
        }
    }
}

/// RULE: the summary is a report, so two calls with no write between them answer the same
/// pairs — reading it changes nothing a later caller sees.
#[tokio::test]
async fn reading_the_summary_changes_nothing() {
    let router = router();
    let (status, body) = call(&router, "CreateUser", &[("UserName", "summary-reader")]).await;
    assert_eq!(
        status, 200,
        "the fixture CreateUser must succeed, got {status}: {body}"
    );
    let first = summary(&router).await;
    let second = summary(&router).await;
    assert_eq!(
        first, second,
        "two reads of the summary must report the same pairs"
    );
}

/// RULE: the token version the summary reports is the one
/// `SetSecurityTokenServicePreferences` last set — and an account that has never set one
/// reports the version its tokens are already issued under.
#[tokio::test]
async fn the_reported_token_version_is_the_one_last_set() {
    let router = router();
    let initial = entry(&summary(&router).await, "GlobalEndpointTokenVersion");
    assert_eq!(
        initial,
        i64::from(state::DEFAULT_GLOBAL_ENDPOINT_TOKEN_VERSION),
        "an account that has set no preference must report the default token version"
    );

    // Rows: the other version, the same version again (a repeat set is the same state),
    // and back to the first one — so the rule is asserted over both values and over the
    // order they are set in.
    // SOURCE for the two values: `globalEndpointTokenVersion`'s own value set
    // (`state::GLOBAL_ENDPOINT_TOKEN_VERSIONS`), which pairs each with the number the
    // summary reports it as.
    for (version, reported) in [
        ("v2Token", 2),
        ("v2Token", 2),
        ("v1Token", 1),
        ("v2Token", 2),
    ] {
        let (status, body) = call(
            &router,
            "SetSecurityTokenServicePreferences",
            &[("GlobalEndpointTokenVersion", version)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {version:?}: the preference must be set, got {status}: {body}"
        );
        let after = entry(&summary(&router).await, "GlobalEndpointTokenVersion");
        assert_eq!(
            after, reported,
            "case {version:?}: expected the summary to report {reported}, got {after}"
        );
    }
}

/// RULE: the version is read against the values the model declares, so a value outside
/// them is refused before anything is written and the account keeps the version it had.
#[tokio::test]
async fn a_version_the_model_does_not_declare_is_refused() {
    let router = router();
    let (status, body) = call(
        &router,
        "SetSecurityTokenServicePreferences",
        &[("GlobalEndpointTokenVersion", "v1Token")],
    )
    .await;
    assert_eq!(status, 200, "the fixture set must succeed, got {status}: {body}");

    for value in ["v3Token", "v1token", "1", ""] {
        let (status, body) = call(
            &router,
            "SetSecurityTokenServicePreferences",
            &[("GlobalEndpointTokenVersion", value)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {value:?}: expected the undeclared version to be refused, got {status}: {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "ValidationError",
            "case {value:?}: expected a client-input refusal, got {code:?}: {body}"
        );
        let held = entry(&summary(&router).await, "GlobalEndpointTokenVersion");
        assert_eq!(
            held, 1,
            "case {value:?}: a refused set must leave the account on the version it had"
        );
    }
}
