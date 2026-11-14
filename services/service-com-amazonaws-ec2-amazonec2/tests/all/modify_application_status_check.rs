// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ModifyApplicationStatusCheck`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=ModifyApplicationStatusCheck`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test: a modify REPLACES exactly the members the request carries on the check
//! it names — "you can update the protocol, port, path, thresholds, and other configuration
//! settings" — leaves every member it does not carry at the value the check already holds, and
//! answers the check as the store then holds it, which is what
//! `DescribeApplicationStatusChecks` reports afterwards. Each value is held to the same
//! published bound the create holds it to, so nothing can be stored through the modify that
//! the create would have refused.
//!
//! The tables below span that class rather than one recorded request: one row per member the
//! operation can change (both enum catalogs driven by the model's own values), then one row per
//! published bound driven at the bound and one step past it, then the ids the call cannot
//! resolve. `DryRun` has rows of its own: the flag is answered before the check is resolved, so
//! a dry run of a modify that would have succeeded changes nothing and one naming no check of
//! the account reads the same verdict.

use ec2::ops::types::{
    AGGREGATION_STATUS_ENUM_VALUES, IP_SCOPE_ENUM_VALUES, IP_VERSION_ENUM_VALUES,
    NETWORK_PROTOCOL_ENUM_VALUES,
};
use ec2::state::{
    APPLICATION_STATUS_CHECK_INTERVAL_SECONDS, APPLICATION_STATUS_CHECK_TIMEOUT_RANGE,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message};

/// The Region every call here is signed for: a check's configuration is a fact of the check
/// rather than of the Region.
const REGION: &str = "us-west-2";

/// What an authorized dry run is answered with, and the status EC2 carries it on.
const DRY_RUN_CODE: &str = "DryRunOperation";
const DRY_RUN_STATUS: u16 = 412;

/// A well-formed application status check id the account holds no check for.
const ABSENT_CHECK: &str = "asc-0fedcba987654321f";

/// One `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// Create one check with the two required members and answer its id.
async fn create_check(router: &MultiServiceRouter, case: &str) -> String {
    let (status, body) = call(
        router,
        REGION,
        "CreateApplicationStatusCheck",
        &[p("Protocol", "http"), p("Port", "80")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateApplicationStatusCheck must succeed, got {status} with body {body}"
    );
    element(&body, "applicationStatusCheckId")
        .unwrap_or_else(|| panic!("case {case:?}: no check id in {body}"))
}

/// One `ModifyApplicationStatusCheck` call on `id` carrying `params`.
async fn modify(
    router: &MultiServiceRouter,
    id: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let mut form = vec![p("ApplicationStatusCheckId", id)];
    form.extend_from_slice(params);
    call(router, REGION, "ModifyApplicationStatusCheck", &form).await
}

/// The members of the one check `DescribeApplicationStatusChecks` reports for `id`, as
/// `(member, value)` pairs this suite compares.
async fn described(router: &MultiServiceRouter, case: &str, id: &str) -> Vec<(String, String)> {
    let (status, body) = call(
        router,
        REGION,
        "DescribeApplicationStatusChecks",
        &[p("ApplicationStatusCheckId.1", id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeApplicationStatusChecks must succeed, got {status} with body \
         {body}"
    );
    MEMBERS
        .iter()
        .filter_map(|member| {
            element(&body, member).map(|value| ((*member).to_string(), value))
        })
        .collect()
}

/// The configuration members a modify can change, as the response spells them.
const MEMBERS: &[&str] = &[
    "aggregation",
    "protocol",
    "port",
    "path",
    "deviceIndex",
    "ipVersion",
    "ipScope",
    "interval",
    "timeout",
    "failureThreshold",
    "successThreshold",
    "statusCodeMatcher",
    "initializationGracePeriodSeconds",
];

/// RULE: a modify replaces exactly the member it names and leaves every other one where the
/// create left it — on the answer it gives and on what the describe reports afterwards.
#[tokio::test]
async fn a_modify_replaces_only_the_members_the_request_names() {
    // (case, the request's member, the reported member, the value due)
    let rows: Vec<(&str, &str, &str, String)> = vec![
        (
            "the other protocol of the modelled catalog",
            "Protocol",
            "protocol",
            NETWORK_PROTOCOL_ENUM_VALUES
                .iter()
                .find(|value| **value != "http")
                .unwrap_or(&"https")
                .to_string(),
        ),
        ("another port", "Port", "port", "8443".to_string()),
        (
            "the largest port the model admits",
            "Port",
            "port",
            "65535".to_string(),
        ),
        ("a path of its own", "Path", "path", "/healthz".to_string()),
        (
            "another network device",
            "DeviceIndex",
            "deviceIndex",
            "3".to_string(),
        ),
        (
            "the other IP version of the modelled catalog",
            "IpVersion",
            "ipVersion",
            IP_VERSION_ENUM_VALUES
                .iter()
                .find(|value| **value != "ipv4")
                .unwrap_or(&"ipv6")
                .to_string(),
        ),
        (
            "the one IP scope of the modelled catalog",
            "IpScope",
            "ipScope",
            IP_SCOPE_ENUM_VALUES
                .first()
                .copied()
                .unwrap_or("private")
                .to_string(),
        ),
        (
            "the other aggregation setting of the modelled catalog",
            "Aggregation",
            "aggregation",
            AGGREGATION_STATUS_ENUM_VALUES
                .iter()
                .find(|value| **value != "included")
                .unwrap_or(&"excluded")
                .to_string(),
        ),
        (
            "the one interval the operation publishes",
            "Interval",
            "interval",
            APPLICATION_STATUS_CHECK_INTERVAL_SECONDS.to_string(),
        ),
        (
            "the shortest timeout the operation publishes",
            "Timeout",
            "timeout",
            APPLICATION_STATUS_CHECK_TIMEOUT_RANGE.0.to_string(),
        ),
        (
            "the longest timeout the operation publishes",
            "Timeout",
            "timeout",
            APPLICATION_STATUS_CHECK_TIMEOUT_RANGE.1.to_string(),
        ),
        (
            "another failure threshold",
            "FailureThreshold",
            "failureThreshold",
            "5".to_string(),
        ),
        (
            "another success threshold",
            "SuccessThreshold",
            "successThreshold",
            "4".to_string(),
        ),
        (
            "a status code matcher naming a range",
            "StatusCodeMatcher",
            "statusCodeMatcher",
            "200,202,300-399".to_string(),
        ),
        (
            "the shortest grace period the model admits",
            "InitializationGracePeriodSeconds",
            "initializationGracePeriodSeconds",
            "-1".to_string(),
        ),
        (
            "the longest grace period the model admits",
            "InitializationGracePeriodSeconds",
            "initializationGracePeriodSeconds",
            "600".to_string(),
        ),
    ];

    for (case, member, reported, value) in rows {
        let (router, _engine) = emulator();
        let id = create_check(&router, case).await;
        let before = described(&router, case, &id).await;

        let (status, body) = modify(&router, &id, &[p(member, value.clone())]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "applicationStatusCheckId").as_deref(),
            Some(id.as_str()),
            "case {case:?}: the answer must name the check that was modified, got body {body}"
        );
        assert_eq!(
            element(&body, reported).as_deref(),
            Some(value.as_str()),
            "case {case:?}: the answer must carry the modified {reported}, got body {body}"
        );
        assert!(
            element(&body, "modifyTime").is_some(),
            "case {case:?}: a modified check carries the instant it was modified, got body {body}"
        );

        // The describe reports what the modify answered, and every OTHER member is exactly
        // where the create left it.
        let after = described(&router, case, &id).await;
        assert!(
            after.contains(&(reported.to_string(), value.clone())),
            "case {case:?}: the describe must report {reported} as {value:?}, got {after:?}"
        );
        let unchanged_before: Vec<&(String, String)> =
            before.iter().filter(|(name, _)| name != reported).collect();
        let unchanged_after: Vec<&(String, String)> =
            after.iter().filter(|(name, _)| name != reported).collect();
        assert_eq!(
            unchanged_before, unchanged_after,
            "case {case:?}: a modify must leave every member it does not name alone"
        );
    }
}

/// RULE, continued: two modifies of DIFFERENT members accumulate — the second does not reset
/// what the first one changed.
#[tokio::test]
async fn successive_modifies_accumulate_on_the_same_check() {
    let (router, _engine) = emulator();
    let case = "two modifies of different members";
    let id = create_check(&router, case).await;

    for (member, reported, value) in [
        ("Path", "path", "/first"),
        ("Port", "port", "8080"),
        ("FailureThreshold", "failureThreshold", "7"),
    ] {
        let (status, body) = modify(&router, &id, &[p(member, value)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: modifying {member} must succeed, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, reported).as_deref(),
            Some(value),
            "case {case:?}: expected {reported} to be {value:?}, got body {body}"
        );
    }

    let reported = described(&router, case, &id).await;
    for (member, value) in [
        ("path", "/first"),
        ("port", "8080"),
        ("failureThreshold", "7"),
    ] {
        assert!(
            reported.contains(&(member.to_string(), value.to_string())),
            "case {case:?}: every member a modify set must still be set: expected {member} \
             {value:?}, got {reported:?}"
        );
    }
}

/// RULE, continued: every published bound the create enforces is enforced by the modify too,
/// and a refused modify changes nothing.
#[tokio::test]
async fn a_value_outside_a_published_bound_is_refused_and_changes_nothing() {
    // (case, the member, the value, the code due)
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a path that does not start with a slash",
            "Path",
            "healthz",
            "InvalidParameterValue",
        ),
        (
            "an interval the operation does not publish",
            "Interval",
            "59",
            "InvalidParameterValue",
        ),
        (
            "a timeout one below the published range",
            "Timeout",
            "0",
            "InvalidParameterValue",
        ),
        (
            "a timeout one above the published range",
            "Timeout",
            "31",
            "InvalidParameterValue",
        ),
        (
            "a failure threshold of none",
            "FailureThreshold",
            "0",
            "InvalidParameterValue",
        ),
        (
            "a success threshold of none",
            "SuccessThreshold",
            "0",
            "InvalidParameterValue",
        ),
        (
            "a negative network device index",
            "DeviceIndex",
            "-1",
            "InvalidParameterValue",
        ),
        (
            "a status code matcher that is no list of status codes",
            "StatusCodeMatcher",
            "healthy",
            "InvalidParameterValue",
        ),
        (
            "a status code matcher whose range runs backwards",
            "StatusCodeMatcher",
            "399-300",
            "InvalidParameterValue",
        ),
    ];

    for (case, member, value, code) in rows {
        let (router, _engine) = emulator();
        let id = create_check(&router, case).await;
        let before = described(&router, case, &id).await;

        let (status, body) = modify(&router, &id, &[p(member, value)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got body {body}"
        );
        let after = described(&router, case, &id).await;
        assert_eq!(
            before, after,
            "case {case:?}: a refused modify must leave the check exactly as it was"
        );
        assert!(
            element(&body, "modifyTime").is_none(),
            "case {case:?}: a refused modify reports no modification, got body {body}"
        );
    }
}

/// RULE, continued: "the application status check must exist and belong to your account" — a
/// value that is no id of this kind is malformed, and a well-formed id the account holds no
/// check for is not found, including one it has deleted.
#[tokio::test]
async fn an_id_the_call_cannot_resolve_is_refused() {
    let (router, _engine) = emulator();
    let case = "a deleted check";
    let deleted = create_check(&router, case).await;
    let (status, body) = call(
        &router,
        REGION,
        "DeleteApplicationStatusCheck",
        &[p("ApplicationStatusCheckId", &deleted)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the delete must succeed, got {status} with body {body}"
    );

    // (case, the id, the code and message due)
    let rows: Vec<(&str, String, &str, String)> = vec![
        (
            "a value that is no application status check id at all",
            "not-a-check-id".to_string(),
            "InvalidApplicationStatusCheckId.Malformed",
            "The application-status-check ID 'not-a-check-id' is malformed".to_string(),
        ),
        (
            "another resource's id",
            "vol-0123456789abcdef0".to_string(),
            "InvalidApplicationStatusCheckId.Malformed",
            "The application-status-check ID 'vol-0123456789abcdef0' is malformed".to_string(),
        ),
        (
            "a well-formed id the account holds no check for",
            ABSENT_CHECK.to_string(),
            "InvalidApplicationStatusCheckId.NotFound",
            format!("The application-status-check ID '{ABSENT_CHECK}' does not exist"),
        ),
        (
            "a check the account has deleted",
            deleted.clone(),
            "InvalidApplicationStatusCheckId.NotFound",
            format!("The application-status-check ID '{deleted}' does not exist"),
        ),
    ];

    for (case, id, code, message) in rows {
        let (status, body) = modify(&router, &id, &[p("Port", "8080")]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got body {body}"
        );
        assert_eq!(
            error_message(&body),
            message,
            "case {case:?}: expected that refusal's own sentence, got body {body}"
        );
    }
}

/// RULE, continued: `DryRun` is answered before the check is resolved — so a dry run of a
/// modify that would have succeeded changes nothing, and one naming a check the account does
/// not hold reads the same verdict.
#[tokio::test]
async fn a_dry_run_modifies_nothing() {
    let (router, _engine) = emulator();
    let case = "a dry run";
    let id = create_check(&router, case).await;
    let before = described(&router, case, &id).await;

    // Rows differing along which check the dry run names and how many have been answered
    // already: each asks about the caller, so none of them writes anything.
    for (row, named) in [(1, id.clone()), (2, ABSENT_CHECK.to_string()), (3, id.clone())] {
        let case = format!("dry run {row} naming {named}");
        let (status, body) = modify(
            &router,
            &named,
            &[p("Port", "9000"), p("Path", "/dry"), p("DryRun", "true")],
        )
        .await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            DRY_RUN_CODE,
            "case {case:?}: expected {DRY_RUN_CODE}, got body {body}"
        );
        let after = described(&router, &case, &id).await;
        assert_eq!(
            before, after,
            "case {case:?}: a dry run must leave the check exactly as it was"
        );
    }

    // `DryRun=false` is not a dry run: the modify is performed.
    let (status, body) = modify(
        &router,
        &id,
        &[p("Port", "9000"), p("DryRun", "false")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"DryRun=false\": expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "port").as_deref(),
        Some("9000"),
        "case \"DryRun=false\": the modify must be performed, got body {body}"
    );
}
