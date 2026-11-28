// SPDX-License-Identifier: CC0-1.0

//! The values an operation of this service accepts for a member that targets a Smithy
//! enum.
//!
//! RULE under test: the set of values an operation accepts for an enum-targeted member is
//! the value list the model declares for that enum — every declared value is accepted,
//! and any other value is refused 400 — so a value the upstream model adds to an enum is
//! served without a handler change.
//!
//! Every row drives the real restJson1 wire path through `MultiServiceRouter`, so a
//! status code that is wrong on the wire fails here. The value lists come from the
//! modules rendered from the model (`STANDARDS_PROVIDER_VALUES`,
//! `SECURITY_CONTROLS_PROVIDER_VALUES`, `CONTROL_STATUS_VALUES`), never from a list
//! written out here: a test that named the values would stop testing the rule the moment
//! the model grew one.

use serde_json::{json, Value};
use service_com_amazonaws_securityhub_securityhubapiservice::ops::types::{
    CONTROL_STATUS_VALUES, SECURITY_CONTROLS_PROVIDER_VALUES, STANDARDS_PROVIDER_VALUES,
};

// The router setup and the SigV4 credential scope every suite of this crate shares, so
// they cannot drift between suites.
use crate::security_hub_core_loop::{Harness, ACCOUNT_ID, REGION};

/// How long a poll for a transition the clock owes may run before the test gives up.
/// Generous on purpose: the test waits for the CONDITION, so its outcome does not depend
/// on how fast the host runs.
const READY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// How long a poll waits before looking again.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);

/// Candidate values no provider enum of this service declares. Each is checked against
/// the rendered value list before it is used, so a model that adds one of them turns the
/// check into a failure rather than into a silently weaker test.
///
/// The forms differ: a cloud nobody models, a declared value in the wrong case, and a
/// declared value with a suffix.
const UNDEFINED_PROVIDERS: [&str; 3] = ["OracleCloud", "aws", "AzureGov"];

/// Candidate values `ControlStatus` does not declare: an unmodelled status, and the two
/// declared ones in the wrong case.
const UNDEFINED_CONTROL_STATUSES: [&str; 3] = ["SUSPENDED", "Enabled", "disabled"];

/// The subscription ARN of the AWS Foundational Security Best Practices standard, one of
/// the two standards `EnableSecurityHub` enables by default.
fn fsbp_subscription() -> String {
    format!(
        "arn:aws:securityhub:{REGION}:{ACCOUNT_ID}:subscription/aws-foundational-security-best-practices/v/1.0.0"
    )
}

/// Which operation a row drives, so one table can span the operations that validate the
/// same enum.
#[derive(Debug, Clone, Copy)]
enum ProviderFilter {
    DescribeStandards,
    ListSecurityControlDefinitions,
    GetEnabledStandards,
}

/// One emulator with the Security Hub service mounted and its hub subscribed with the
/// default standards.
async fn subscribed() -> Harness {
    let harness = Harness::new();
    let (status, body) = harness.enable(None).await;
    assert_eq!(
        status, 200,
        "EnableSecurityHub must succeed, got {status} with {body}"
    );
    harness
}

/// One operation's provider filter carrying exactly `provider`.
async fn filter_by_provider(
    harness: &Harness,
    operation: ProviderFilter,
    provider: &str,
) -> (u16, Value) {
    match operation {
        ProviderFilter::DescribeStandards => {
            harness
                .call("GET", &format!("/standards?Providers={provider}"), None)
                .await
        }
        ProviderFilter::ListSecurityControlDefinitions => {
            harness
                .call(
                    "GET",
                    &format!("/securityControls/definitions?Providers={provider}&MaxResults=100"),
                    None,
                )
                .await
        }
        ProviderFilter::GetEnabledStandards => {
            harness
                .call(
                    "POST",
                    "/standards/get",
                    Some(json!({ "Providers": [provider] })),
                )
                .await
        }
    }
}

/// `UpdateStandardsControl`, as the modelled
/// `PATCH /standards/control/{StandardsControlArn+}` binding carries the control.
async fn update_standards_control(
    harness: &Harness,
    control_arn: &str,
    body: Value,
) -> (u16, Value) {
    harness
        .call(
            "PATCH",
            &format!("/standards/control/{control_arn}"),
            Some(body),
        )
        .await
}

/// The ARN of one control of the default FSBP subscription, once that subscription's
/// `PENDING` -> `READY` transition has been materialised: a control is configurable
/// only from then on. Looks again until the controls are reported, or the deadline
/// passes — never for a fixed span.
async fn a_configurable_control_arn(harness: &Harness) -> String {
    let deadline = std::time::Instant::now() + READY_DEADLINE;
    let subscription = fsbp_subscription();
    loop {
        let (status, body) = harness
            .call("GET", &format!("/standards/controls/{subscription}"), None)
            .await;
        if status == 200 {
            if let Some(arn) = body["Controls"]
                .as_array()
                .and_then(|controls| controls.first())
                .and_then(|control| control["StandardsControlArn"].as_str())
            {
                return arn.to_string();
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "DescribeStandardsControls must report the subscription's controls, \
             got {status} with {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The members of `candidates` that `enum_values` does not declare — asserted non-empty,
/// so a refusal row is never silently dropped.
fn undefined_values(
    enum_name: &str,
    enum_values: &[&str],
    candidates: &[&'static str],
) -> Vec<&'static str> {
    let undefined: Vec<&'static str> = candidates
        .iter()
        .copied()
        .filter(|candidate| !enum_values.contains(candidate))
        .collect();
    assert!(
        !undefined.is_empty(),
        "case {enum_name:?}: the test needs at least one value the enum does not declare, \
         got {candidates:?} against {enum_values:?}"
    );
    undefined
}

/// RULE: every value a provider enum declares is accepted by the operations that filter
/// on it — selecting the entries whose provider it names, and selecting none when nothing
/// carries that provider — and a value the enum does not declare is refused 400.
///
/// The table spans the three operations that take a provider filter (two enums, a query
/// filter and a body filter, a service-metadata read and an account-data read), every
/// value each enum declares, and three forms of undeclared value.
#[tokio::test]
async fn every_modelled_provider_is_accepted_and_any_other_is_refused() {
    let harness = subscribed().await;

    // (operation, the enum its filter targets, that enum's declared values, the response
    // member that carries the selection)
    let rows: [(ProviderFilter, &str, &[&str], &str); 3] = [
        (
            ProviderFilter::DescribeStandards,
            "StandardsProvider",
            STANDARDS_PROVIDER_VALUES,
            "Standards",
        ),
        (
            ProviderFilter::ListSecurityControlDefinitions,
            "SecurityControlsProvider",
            SECURITY_CONTROLS_PROVIDER_VALUES,
            "SecurityControlDefinitions",
        ),
        (
            ProviderFilter::GetEnabledStandards,
            "StandardsProvider",
            STANDARDS_PROVIDER_VALUES,
            "StandardsSubscriptions",
        ),
    ];

    for (operation, enum_name, values, member) in rows {
        assert!(
            !values.is_empty(),
            "case {operation:?}: {enum_name} must declare at least one value"
        );
        let mut selected_something = false;
        for provider in values {
            let case = format!("{operation:?} Providers={provider}");
            let (status, body) = filter_by_provider(&harness, operation, provider).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a value {enum_name} declares must be accepted, \
                 got {status} with {body}"
            );
            let selection = body[member].as_array().unwrap_or_else(|| {
                panic!("case {case:?}: the response must carry {member} as a list, got {body}")
            });
            selected_something |= !selection.is_empty();
        }
        // Accepting a declared value is not enough: at least one of them must SELECT
        // what it names, so a handler that answered 200 with an empty list for every
        // provider fails here rather than passing the accept half of the rule. Which
        // provider that is belongs to the catalog and to the account's own data, so it is
        // not named — the per-provider selections are asserted element for element in
        // `describe_standards.rs`, `security_control_definitions.rs` and
        // `enabled_standards.rs`.
        assert!(
            selected_something,
            "case {operation:?}: at least one value {enum_name} declares must select \
             something, got an empty {member} for every one of {values:?}"
        );

        for provider in undefined_values(enum_name, values, &UNDEFINED_PROVIDERS) {
            let case = format!("{operation:?} Providers={provider}");
            let (status, body) = filter_by_provider(&harness, operation, provider).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a value {enum_name} does not declare must be refused, \
                 got {status} with {body}"
            );
        }
    }
}

/// RULE, on an enum that is not a provider: `UpdateStandardsControl` accepts every
/// `ControlStatus` the model declares and refuses any other value 400.
///
/// The rows span every declared status — including the one that needs a `DisabledReason`
/// — and three forms of undeclared value.
#[tokio::test]
async fn every_modelled_control_status_is_accepted_and_any_other_is_refused() {
    let harness = subscribed().await;
    let control_arn = a_configurable_control_arn(&harness).await;

    assert!(
        !CONTROL_STATUS_VALUES.is_empty(),
        "ControlStatus must declare at least one value"
    );
    for status_value in CONTROL_STATUS_VALUES {
        let case = format!("ControlStatus={status_value}");
        // `DisabledReason` is the reason for the most recent change in a control's
        // status, which the service requires of a change that disables one, so every row
        // carries it.
        let (status, body) = update_standards_control(
            &harness,
            &control_arn,
            json!({
                "ControlStatus": status_value,
                "DisabledReason": "a status the modelled enum declares",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a value ControlStatus declares must be accepted, \
             got {status} with {body}"
        );
    }

    for status_value in undefined_values(
        "ControlStatus",
        CONTROL_STATUS_VALUES,
        &UNDEFINED_CONTROL_STATUSES,
    ) {
        let case = format!("ControlStatus={status_value}");
        let (status, body) = update_standards_control(
            &harness,
            &control_arn,
            json!({
                "ControlStatus": status_value,
                "DisabledReason": "a status outside the modelled enum",
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a value ControlStatus does not declare must be refused, \
             got {status} with {body}"
        );
    }
}
