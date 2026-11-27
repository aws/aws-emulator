// SPDX-License-Identifier: CC0-1.0

//! A stored recommendation preference record always HOLDS a preference.
//!
//! One rule, asserted over the forms a request can take rather than the one a recording
//! happened to use: a `PutRecommendationPreferences` whose merged result sets no preference
//! stores no record at that (resourceType, scope) key — it creates none, it drops one whose
//! last value it cleared, and it claims none of the store's capacity — so no later read can
//! report or inherit a preference nobody set. `DeleteRecommendationPreferences` keeps the
//! same invariant from the other side, and `tests/all/enrollment_and_preferences.rs` pins
//! that half.
//!
//! Every call goes through the shared wire harness (`tests/all/harness.rs`), i.e. the same
//! `MultiServiceRouter::dispatch` the emulator's front door uses, so what these tests prove
//! is what a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    DEFAULT_ENHANCED_INFRASTRUCTURE_METRICS, DEFAULT_LOOK_BACK_PERIOD, MAX_PREFERENCE_RECORDS,
    ORGANIZATION_SCOPE_VALUE, SCOPE_ACCOUNT_ID, SCOPE_ORGANIZATION, SCOPE_RESOURCE_ARN,
};

use crate::harness::{
    auto_scaling_group_arn, call, effective_for, emulator, get_preferences, instance_arn,
    management_account_emulator, opt_in, put_preference, ACCOUNT,
};

/// One row per form a request that names no preference can take: a different resource type,
/// a different scope level (the three `ScopeName` values the model admits, an empty `Scope`
/// object, and no scope at all), and a different way of carrying a member that sets nothing
/// (absent, or a list with no entries — which neither of the two list preferences is set by).
fn requests_that_set_no_preference() -> Vec<(&'static str, Value)> {
    vec![
        ("resourceType alone", json!({"resourceType": "EbsVolume"})),
        (
            "an empty scope object",
            json!({"resourceType": "LambdaFunction", "scope": {}}),
        ),
        (
            "an account-level scope",
            json!({
                "resourceType": "Ec2Instance",
                "scope": {"name": SCOPE_ACCOUNT_ID, "value": ACCOUNT},
            }),
        ),
        (
            "an organization-level scope",
            json!({
                "resourceType": "RdsDBInstance",
                "scope": {"name": SCOPE_ORGANIZATION, "value": ORGANIZATION_SCOPE_VALUE},
            }),
        ),
        (
            "a resource-level scope",
            json!({
                "resourceType": "AutoScalingGroup",
                "scope": {"name": SCOPE_RESOURCE_ARN, "value": auto_scaling_group_arn("batch")},
            }),
        ),
        (
            "an empty utilizationPreferences list",
            json!({
                "resourceType": "Ec2Instance",
                "scope": {"name": SCOPE_RESOURCE_ARN, "value": instance_arn("i-0123456789abcdef0")},
                "utilizationPreferences": [],
            }),
        ),
        (
            "an empty preferredResources list",
            json!({"resourceType": "EcsService", "preferredResources": []}),
        ),
        (
            "both preference lists empty",
            json!({
                "resourceType": "Idle",
                "utilizationPreferences": [],
                "preferredResources": [],
            }),
        ),
    ]
}

/// How many preference records a `GetRecommendationPreferences` response reports.
fn reported(read: &Value) -> usize {
    read["recommendationPreferencesDetails"]
        .as_array()
        .map_or(0, Vec::len)
}

/// A request that sets no preference is accepted and stores nothing: the read of its own
/// key reports no record, the read of its whole resource type reports none either, and the
/// effective preferences of a resource resolve to the documented defaults.
#[tokio::test]
async fn a_put_that_sets_no_preference_stores_no_record() {
    for (case, request) in requests_that_set_no_preference() {
        // A fresh emulator per row: what the reads report is this row's write alone. One row
        // addresses the organization-level scope, which the model admits "for management
        // accounts of an organization only" (`PutRecommendationPreferences$scope`), so every
        // row's caller manages an organization.
        let (router, _engine) = management_account_emulator();
        opt_in(&router).await;

        let resource_type = request["resourceType"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the row names a resourceType"))
            .to_string();
        let scope = request.get("scope").cloned();

        let (status, body) = call(&router, "PutRecommendationPreferences", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {request} to be accepted, got {status}: {body}"
        );
        assert_eq!(
            body,
            json!({}),
            "case {case:?}: PutRecommendationPreferences has an empty output, got {body}"
        );

        for (what, at) in [
            ("the key it addressed", scope.as_ref()),
            ("the whole resource type", None),
        ] {
            let read = get_preferences(&router, &resource_type, at).await;
            assert_eq!(
                reported(&read),
                0,
                "case {case:?}: a request that sets no preference must leave nothing for a \
                 read of {what} to report, got {read}"
            );
        }

        // And nothing to inherit either: every member resolves to the service's documented
        // default ("When this preference isn't specified, we use the default value
        // DAYS_14"), with no value reported for a preference that has no default.
        let effective = effective_for(&router, &instance_arn("i-0123456789abcdef0")).await;
        assert_eq!(
            effective["lookBackPeriod"],
            json!(DEFAULT_LOOK_BACK_PERIOD),
            "case {case:?}: expected the default lookBackPeriod, got {effective}"
        );
        assert_eq!(
            effective["enhancedInfrastructureMetrics"],
            json!(DEFAULT_ENHANCED_INFRASTRUCTURE_METRICS),
            "case {case:?}: no record sets enhancedInfrastructureMetrics, so the documented \
             default status is in effect, got {effective}"
        );
    }
}

/// One row per preference member kind — two scalars, an object, and the two lists — written
/// at a resource type and scope the service supports it for, with what a read reports back.
fn stored_preferences() -> Vec<(&'static str, Value, Value, Option<Value>)> {
    vec![
        (
            "enhancedInfrastructureMetrics",
            json!("Active"),
            json!("Ec2Instance"),
            Some(json!({"name": SCOPE_ACCOUNT_ID, "value": ACCOUNT})),
        ),
        (
            "lookBackPeriod",
            json!("DAYS_93"),
            json!("EbsVolume"),
            None,
        ),
        (
            "savingsEstimationMode",
            json!("AfterDiscounts"),
            json!("LambdaFunction"),
            Some(json!({"name": SCOPE_ORGANIZATION, "value": ORGANIZATION_SCOPE_VALUE})),
        ),
        (
            "externalMetricsPreference",
            json!({"source": "Datadog"}),
            json!("Ec2Instance"),
            Some(json!({"name": SCOPE_RESOURCE_ARN, "value": instance_arn("i-00a0b0c0d0e0f0001")})),
        ),
        (
            "utilizationPreferences",
            json!([{
                "metricName": "CpuUtilization",
                "metricParameters": {"threshold": "P95", "headroom": "PERCENT_20"},
            }]),
            json!("Ec2Instance"),
            None,
        ),
        (
            "preferredResources",
            json!([{"name": "Ec2InstanceTypes", "includeList": ["m5.large"]}]),
            json!("AutoScalingGroup"),
            Some(json!({"name": SCOPE_RESOURCE_ARN, "value": auto_scaling_group_arn("web-tier")})),
        ),
    ]
}

/// A request that sets no preference is a no-op on a key that already holds one: the stored
/// record survives untouched, whichever member kind it holds.
#[tokio::test]
async fn a_put_that_sets_no_preference_leaves_a_stored_record_alone() {
    for (member, value, resource_type, scope) in stored_preferences() {
        let resource_type = resource_type.as_str().expect("the row names a resourceType");
        let case = format!("{member} on {resource_type} at {scope:?}");

        // One row stores at the organization-level scope, which only a management account may
        // create a preference at.
        let (router, _engine) = management_account_emulator();
        opt_in(&router).await;

        let mut request = json!({"resourceType": resource_type});
        if let Some(scope) = &scope {
            request["scope"] = scope.clone();
        }
        let mut setting = request.clone();
        setting[member] = value.clone();
        put_preference(&router, &setting).await;

        // The same key, with no preference member at all.
        let (status, body) = call(&router, "PutRecommendationPreferences", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {request} to be accepted, got {status}: {body}"
        );

        let read = get_preferences(&router, resource_type, scope.as_ref()).await;
        assert_eq!(
            reported(&read),
            1,
            "case {case:?}: expected the one stored preference to survive, got {read}"
        );
        let detail = &read["recommendationPreferencesDetails"][0];
        if member == "preferredResources" {
            // Reported as `EffectivePreferredResource`: the stored lists plus the
            // `effectiveIncludeList` expansion, so the stored lists are what survives.
            for field in ["name", "includeList"] {
                assert_eq!(
                    detail[member][0][field], value[0][field],
                    "case {case:?}: expected the stored {field} {value}, got {read}"
                );
            }
            continue;
        }
        assert_eq!(
            detail[member], value,
            "case {case:?}: expected the stored {member} to be read back, got {read}"
        );
    }
}

/// One row of [`clearing_updates`]: a record written with a list preference, the member
/// whose empty list clears it, and what is left set afterwards.
struct ClearingUpdate {
    /// What this row is, for the assertion messages.
    case: &'static str,
    /// The `PutRecommendationPreferences` request the record is written with.
    written: Value,
    /// The list member the second request sends as an empty list.
    cleared_member: &'static str,
    /// The member still set on the record afterwards, and its value — `None` when the
    /// clearing leaves the record holding no preference at all.
    survivor: Option<(&'static str, Value)>,
}

/// One row per way a list preference can be the last value on a record: what the record is
/// written with, the member whose empty list clears it, and the member that is left set
/// afterwards (or `None` when the clearing leaves the record holding nothing).
fn clearing_updates() -> Vec<ClearingUpdate> {
    let utilization = json!([{
        "metricName": "MemoryUtilization",
        "metricParameters": {"headroom": "PERCENT_30"},
    }]);
    let preferred = json!([{"name": "Ec2InstanceTypes", "excludeList": ["t2"]}]);
    vec![
        ClearingUpdate {
            case: "utilizationPreferences was the only preference",
            written: json!({
                "resourceType": "Ec2Instance",
                "scope": {"name": SCOPE_ACCOUNT_ID, "value": ACCOUNT},
                "utilizationPreferences": utilization,
            }),
            cleared_member: "utilizationPreferences",
            survivor: None,
        },
        ClearingUpdate {
            case: "preferredResources was the only preference",
            written: json!({"resourceType": "EbsVolume", "preferredResources": preferred}),
            cleared_member: "preferredResources",
            survivor: None,
        },
        ClearingUpdate {
            case: "another preference is left on the record",
            written: json!({
                "resourceType": "Ec2Instance",
                "utilizationPreferences": utilization,
                "lookBackPeriod": "DAYS_32",
            }),
            cleared_member: "utilizationPreferences",
            survivor: Some(("lookBackPeriod", json!("DAYS_32"))),
        },
    ]
}

/// An update that leaves NOTHING set on a key already held drops the record, and one that
/// leaves another preference set keeps it: what is stored follows from what the merged
/// record holds, not from whether the key existed.
#[tokio::test]
async fn an_update_that_leaves_nothing_set_drops_the_record() {
    for ClearingUpdate {
        case,
        written,
        cleared_member,
        survivor,
    } in clearing_updates()
    {
        let router = emulator();
        opt_in(&router).await;
        put_preference(&router, &written).await;

        let resource_type = written["resourceType"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the row names a resourceType"))
            .to_string();
        let scope = written.get("scope").cloned();

        let mut clearing = json!({"resourceType": resource_type});
        if let Some(scope) = &scope {
            clearing["scope"] = scope.clone();
        }
        clearing[cleared_member] = json!([]);
        let (status, body) = call(&router, "PutRecommendationPreferences", clearing.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {clearing} to be accepted, got {status}: {body}"
        );

        let read = get_preferences(&router, &resource_type, scope.as_ref()).await;
        let Some((member, value)) = survivor else {
            assert_eq!(
                reported(&read),
                0,
                "case {case:?}: a record left holding no preference must be dropped, got {read}"
            );
            continue;
        };
        assert_eq!(
            reported(&read),
            1,
            "case {case:?}: a record still holding a preference must survive, got {read}"
        );
        let detail = &read["recommendationPreferencesDetails"][0];
        assert_eq!(
            detail[member], value,
            "case {case:?}: expected {member} {value} to survive, got {read}"
        );
        assert!(
            detail[cleared_member].is_null(),
            "case {case:?}: expected {cleared_member} to be cleared, got {read}"
        );
    }
}

/// A request that stores no record claims none of the store's capacity: repeating it at
/// `MAX_PREFERENCE_RECORDS` distinct scopes leaves the store empty, so a real preference at
/// a new key is still accepted and read back.
#[tokio::test]
async fn a_put_that_sets_no_preference_claims_no_capacity() {
    let router = emulator();
    opt_in(&router).await;

    for index in 0..MAX_PREFERENCE_RECORDS {
        let request = json!({
            "resourceType": "Ec2Instance",
            "scope": {"name": SCOPE_RESOURCE_ARN, "value": instance_arn(&format!("i-{index:016x}"))},
        });
        let (status, body) = call(&router, "PutRecommendationPreferences", request).await;
        assert_eq!(
            status, 200,
            "no-preference put {index} of {MAX_PREFERENCE_RECORDS}: expected 200, got \
             {status}: {body}"
        );
    }

    let scope = json!({"name": SCOPE_RESOURCE_ARN, "value": instance_arn("i-ffffffffffffffff")});
    put_preference(
        &router,
        &json!({
            "resourceType": "Ec2Instance",
            "scope": scope.clone(),
            "enhancedInfrastructureMetrics": "Active",
        }),
    )
    .await;

    let read = get_preferences(&router, "Ec2Instance", None).await;
    assert_eq!(
        reported(&read),
        1,
        "the store must hold only the one real preference after \
         {MAX_PREFERENCE_RECORDS} no-preference puts, got {read}"
    );
    let detail = &read["recommendationPreferencesDetails"][0];
    assert_eq!(
        detail["enhancedInfrastructureMetrics"],
        json!("Active"),
        "expected the stored preference to be read back, got {read}"
    );
    assert_eq!(
        detail["scope"], scope,
        "expected the detail to carry the scope written, got {read}"
    );
}
