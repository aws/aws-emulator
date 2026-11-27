// SPDX-License-Identifier: CC0-1.0

//! What an effective-preferences read reports for a preference whose own vocabulary names
//! the not-set state.
//!
//! The rule: a preference whose value set itself names the state of a resource no record
//! covers is REPORTED as a status, always — the value of the most specific scope that sets
//! it, and the service's documented default where none does — so the member is never
//! dropped from an answer. `enhancedInfrastructureMetrics` is such a preference: "a status
//! of `Active` confirms that the preference is applied in the latest recommendation
//! refresh, and a status of `Inactive` confirms that it's not yet applied to
//! recommendations" (`EffectiveRecommendationPreferences$enhancedInfrastructureMetrics`),
//! so `Inactive` IS the answer for a resource with nothing on record. A preference whose
//! own documentation makes absence the answer — `externalMetricsPreference`, "if the
//! preference isn't applied to the recommendations already, then this object doesn't appear
//! in the response" — stays absent, which the table below pins too.
//!
//! The table spans what the resolution generalises over: which scope holds the record
//! (none, organization, account, the resource itself, another resource), which value it
//! holds (every value of the model's own `EnhancedInfrastructureMetrics` set), whether the
//! record sets this preference at all, and which of the two resource ARN forms the read
//! names. The reported status is never an echo of a stored record: the rows prove a written
//! value wins over the default, a more specific scope wins over a less specific one, and a
//! deleted record falls back to the default again.

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::ops::types::ENHANCED_INFRASTRUCTURE_METRICS_VALUES;
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    ANALYSIS_DELAY, DEFAULT_ENHANCED_INFRASTRUCTURE_METRICS, ORGANIZATION_SCOPE_VALUE,
    SCOPE_ACCOUNT_ID, SCOPE_ORGANIZATION, SCOPE_RESOURCE_ARN,
};

use crate::harness::{
    auto_scaling_group_arn, call, effective_for, emulator, get_preferences, instance_arn,
    management_account_emulator, opt_in, put_preference, ACCOUNT,
};

/// The member every row below reads.
const MEMBER: &str = "enhancedInfrastructureMetrics";

/// The EC2 instance every row that reads an instance names.
fn read_instance() -> String {
    instance_arn("i-0ab0c0d0e0f000011")
}

/// Another instance of the same account, so a resource-scoped record can be written where
/// it must NOT apply to the resource read.
fn other_instance() -> String {
    instance_arn("i-0ab0c0d0e0f000022")
}

/// The Auto Scaling group every row that reads a group names — the other ARN form
/// `GetEffectiveRecommendationPreferences` accepts.
fn read_group() -> String {
    auto_scaling_group_arn("effective-status-tier")
}

/// One row: the records to write before the read, the resource whose effective preferences
/// are then read, and the status that must be in force for it.
struct Row {
    case: &'static str,
    writes: Vec<Value>,
    resource_arn: String,
    expected: &'static str,
}

/// Every row, in the order they are run — each on its own emulator, so no row inherits
/// another's records.
fn rows() -> Vec<Row> {
    let instance = read_instance();
    let group = read_group();
    let account_scope = json!({"name": SCOPE_ACCOUNT_ID, "value": ACCOUNT});
    let organization_scope = json!({"name": SCOPE_ORGANIZATION, "value": ORGANIZATION_SCOPE_VALUE});
    let instance_scope = json!({"name": SCOPE_RESOURCE_ARN, "value": instance});
    let other_scope = json!({"name": SCOPE_RESOURCE_ARN, "value": other_instance()});
    let group_scope = json!({"name": SCOPE_RESOURCE_ARN, "value": group});
    // An Auto Scaling group preference "can only be set at the resource level by specifying
    // a scope name of ResourceArn" (`PutRecommendationPreferences$scope`), so the group rows
    // write at that scope only.
    let ec2 = |scope: &Value, status: &str| {
        json!({
            "resourceType": "Ec2Instance",
            "scope": scope.clone(),
            MEMBER: status,
        })
    };
    let group_preference = |status: &str| {
        json!({
            "resourceType": "AutoScalingGroup",
            "scope": group_scope.clone(),
            MEMBER: status,
        })
    };

    vec![
        Row {
            case: "nothing on record",
            writes: vec![],
            resource_arn: instance.clone(),
            expected: DEFAULT_ENHANCED_INFRASTRUCTURE_METRICS,
        },
        Row {
            case: "a record that sets another preference only",
            writes: vec![json!({
                "resourceType": "Ec2Instance",
                "scope": account_scope.clone(),
                "lookBackPeriod": "DAYS_93",
            })],
            resource_arn: instance.clone(),
            expected: DEFAULT_ENHANCED_INFRASTRUCTURE_METRICS,
        },
        Row {
            case: "the account scope activates it",
            writes: vec![ec2(&account_scope, "Active")],
            resource_arn: instance.clone(),
            expected: "Active",
        },
        Row {
            case: "the account scope deactivates it",
            writes: vec![ec2(&account_scope, "Inactive")],
            resource_arn: instance.clone(),
            expected: "Inactive",
        },
        Row {
            case: "the organization scope alone",
            writes: vec![ec2(&organization_scope, "Active")],
            resource_arn: instance.clone(),
            expected: "Active",
        },
        Row {
            case: "the account scope wins over the organization scope",
            writes: vec![
                ec2(&organization_scope, "Active"),
                ec2(&account_scope, "Inactive"),
            ],
            resource_arn: instance.clone(),
            expected: "Inactive",
        },
        Row {
            case: "the resource scope wins over the account scope",
            writes: vec![ec2(&account_scope, "Inactive"), ec2(&instance_scope, "Active")],
            resource_arn: instance.clone(),
            expected: "Active",
        },
        Row {
            case: "another resource's record does not apply",
            writes: vec![ec2(&other_scope, "Active")],
            resource_arn: instance.clone(),
            expected: DEFAULT_ENHANCED_INFRASTRUCTURE_METRICS,
        },
        Row {
            case: "an auto scaling group with nothing on record",
            writes: vec![],
            resource_arn: group.clone(),
            expected: DEFAULT_ENHANCED_INFRASTRUCTURE_METRICS,
        },
        Row {
            case: "an auto scaling group's own record",
            writes: vec![group_preference("Active")],
            resource_arn: group.clone(),
            expected: "Active",
        },
        Row {
            case: "an Ec2Instance record does not reach an auto scaling group",
            writes: vec![ec2(&account_scope, "Active")],
            resource_arn: group,
            expected: DEFAULT_ENHANCED_INFRASTRUCTURE_METRICS,
        },
    ]
}

/// Every row's effective status is the value of the most specific scope that set it, or the
/// documented default where none did — reported as a member of the answer in every case,
/// never left out.
#[tokio::test]
async fn the_effective_status_is_reported_for_every_resource() {
    for Row {
        case,
        writes,
        resource_arn,
        expected,
    } in rows()
    {
        // Some rows write at the organization-level scope, which the model admits "for
        // management accounts of an organization only"
        // (`PutRecommendationPreferences$scope`), so every row's caller manages an organization
        // — which decides nothing about the status that is then in force.
        let (router, _engine) = management_account_emulator();
        opt_in(&router).await;
        for request in &writes {
            put_preference(&router, request).await;
        }

        let effective = effective_for(&router, &resource_arn).await;
        let reported = effective[MEMBER].as_str();
        assert_eq!(
            reported,
            Some(expected),
            "case {case:?}: expected the status in force to be reported as {expected:?}, got \
             {effective}"
        );
        // The invariant the rule implies: whatever is reported is one of the preference's own
        // modelled values, so a client can always branch on it.
        assert!(
            reported.is_some_and(|status| ENHANCED_INFRASTRUCTURE_METRICS_VALUES.contains(&status)),
            "case {case:?}: expected one of {ENHANCED_INFRASTRUCTURE_METRICS_VALUES:?}, got \
             {effective}"
        );
        // The member whose own documentation makes absence the answer stays absent: the rule
        // is about a preference whose vocabulary names the not-set state, not about every
        // member of the answer.
        assert!(
            effective["externalMetricsPreference"].is_null(),
            "case {case:?}: no record sets externalMetricsPreference, and that member reports \
             nothing when it is not applied, got {effective}"
        );
    }
}

/// Every value of the preference's own vocabulary is reported back as the status in force
/// when a record holds it, and the documented default returns once that record is deleted —
/// so the reported status is resolved, not stored, and not hard-coded either.
#[tokio::test]
async fn a_written_status_is_reported_and_its_deletion_restores_the_default() {
    let router = emulator();
    opt_in(&router).await;
    let instance = read_instance();
    let account_scope = json!({"name": SCOPE_ACCOUNT_ID, "value": ACCOUNT});

    // Before anything is written there is no record to echo, and the status is still
    // reported — the no-record case the default covers.
    let effective = effective_for(&router, &instance).await;
    assert_eq!(
        effective[MEMBER].as_str(),
        Some(DEFAULT_ENHANCED_INFRASTRUCTURE_METRICS),
        "case \"before any write\": expected the documented default, got {effective}"
    );
    let stored = get_preferences(&router, "Ec2Instance", Some(&account_scope)).await;
    assert!(
        stored["recommendationPreferencesDetails"]
            .as_array()
            .is_some_and(|details| details.is_empty()),
        "case \"before any write\": a read of the records must report none, got {stored}"
    );

    // One row per value the model's `EnhancedInfrastructureMetrics` set holds, taken from the
    // crate's generated value set rather than written out here.
    for status in ENHANCED_INFRASTRUCTURE_METRICS_VALUES {
        put_preference(
            &router,
            &json!({
                "resourceType": "Ec2Instance",
                "scope": account_scope.clone(),
                MEMBER: status,
            }),
        )
        .await;
        let effective = effective_for(&router, &instance).await;
        assert_eq!(
            effective[MEMBER].as_str(),
            Some(*status),
            "case {status:?}: a stored status is the one in force, got {effective}"
        );

        let (code, body) = call(
            &router,
            "DeleteRecommendationPreferences",
            json!({
                "resourceType": "Ec2Instance",
                "scope": account_scope.clone(),
                "recommendationPreferenceNames": ["EnhancedInfrastructureMetrics"],
            }),
        )
        .await;
        assert_eq!(
            code, 200,
            "case {status:?}: deleting the record expected 200, got {code}: {body}"
        );
        let effective = effective_for(&router, &instance).await;
        assert_eq!(
            effective[MEMBER].as_str(),
            Some(DEFAULT_ENHANCED_INFRASTRUCTURE_METRICS),
            "case {status:?}: with the record gone the documented default is in force, got \
             {effective}"
        );
    }
}

/// The ONE resolution: the status a recommendation says it was generated under is the status
/// an effective-preferences read reports for the same resource — with nothing on record as
/// much as with each value written at the account scope.
#[tokio::test]
async fn a_recommendation_reports_the_same_status_as_the_effective_read() {
    let router = emulator();
    opt_in(&router).await;
    // The analysis an enrollment starts has to be due before a recommendation is reported.
    tokio::time::sleep(ANALYSIS_DELAY + std::time::Duration::from_millis(250)).await;
    let account_scope = json!({"name": SCOPE_ACCOUNT_ID, "value": ACCOUNT});

    // The written status of each row, `None` for the row that writes nothing.
    let mut written: Vec<Option<&str>> = vec![None];
    written.extend(ENHANCED_INFRASTRUCTURE_METRICS_VALUES.iter().map(|status| Some(*status)));

    for status in written {
        let case = status.unwrap_or("nothing on record");
        if let Some(status) = status {
            put_preference(
                &router,
                &json!({
                    "resourceType": "Ec2Instance",
                    "scope": account_scope.clone(),
                    MEMBER: status,
                }),
            )
            .await;
        }

        let (code, body) = call(&router, "GetEC2InstanceRecommendations", json!({})).await;
        assert_eq!(
            code, 200,
            "case {case:?}: reading instance recommendations expected 200, got {code}: {body}"
        );
        let recommendation = body["instanceRecommendations"]
            .as_array()
            .and_then(|reported| reported.first())
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected the analysis to report a recommendation: {body}")
            });
        let instance = recommendation["instanceArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: a recommendation names its instance: {recommendation}")
            })
            .to_string();

        let effective = effective_for(&router, &instance).await;
        let reported = recommendation["effectiveRecommendationPreferences"][MEMBER].as_str();
        assert_eq!(
            reported,
            Some(status.unwrap_or(DEFAULT_ENHANCED_INFRASTRUCTURE_METRICS)),
            "case {case:?}: expected the recommendation to report the status in force, got \
             {recommendation}"
        );
        assert_eq!(
            reported,
            effective[MEMBER].as_str(),
            "case {case:?}: the two readers must report one resolution, got {effective} and \
             {recommendation}"
        );
    }
}
