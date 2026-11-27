// SPDX-License-Identifier: CC0-1.0

//! The preference members the service applies to EVERY resource type.
//!
//! `PutRecommendationPreferences` refuses a preference for a resource type only where that
//! preference is restricted to a subset of them; a preference the service restricts nowhere
//! is accepted and stored for every `ResourceType` the model's enum admits. Two members are
//! of that kind:
//!
//! * `savingsEstimationMode` — every resource type's own effective-preferences shape in the
//!   model (`EffectiveRecommendationPreferences`, `EBSEffectiveRecommendationPreferences`,
//!   `ECSEffectiveRecommendationPreferences`, `LambdaEffectiveRecommendationPreferences`,
//!   `RDSEffectiveRecommendationPreferences`) carries it, so the preference applies wherever
//!   a recommendation is generated;
//! * `inferredWorkloadTypes` — a status switch whose documentation describes only what Active
//!   and Inactive do ("Specify the Active status to activate the preference, or specify
//!   Inactive to deactivate the preference"), which names no resource type, and whose refusal
//!   points at none either.
//!
//! The tables below therefore walk the whole `ResourceType` enum rather than the one
//! resource type a recording happened to use, and each preference round-trips through
//! `GetRecommendationPreferences` for the `(resourceType, scope)` key it was written at.
//!
//! A stored preference also has to be OBSERVABLE: the last table here drives the member
//! `savingsEstimationMode` activates — the savings opportunity "after any applying discounts"
//! — over the recommendation readers of several resource types, so the round-trip is checked
//! where the preference takes effect and not only where it is stored.
//!
//! Where a member IS restricted to a subset of the resource types, the refusal is asserted
//! in `tests/all/preference_resource_type_support.rs`.
//!
//! Every call goes through the shared wire harness (`tests/all/harness.rs`), i.e. the same
//! `MultiServiceRouter::dispatch` the emulator's front door uses.

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::ops::types::SAVINGS_ESTIMATION_MODE_VALUES;
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    RESOURCE_TYPE_EBS_VOLUME, RESOURCE_TYPE_IDLE, RESOURCE_TYPE_LAMBDA_FUNCTION,
    SAVINGS_ESTIMATION_AFTER_DISCOUNTS, SCOPE_ACCOUNT_ID,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, emulator, emulator_with_engine, get_preferences, make_analysis_due,
    management_account_emulator, opt_in, put_preference, register_analysed_functions, scope_for,
    ACCOUNT, RESOURCE_TYPES,
};

/// `savingsEstimationMode` is accepted — HTTP 200 with an empty body — and stored for EVERY
/// `ResourceType` the enum admits, and the stored record is what the matching
/// `GetRecommendationPreferences` read reports back.
#[tokio::test]
async fn savings_estimation_mode_is_settable_for_every_resource_type() {
    // The two `SavingsEstimationMode` values (model: AfterDiscounts | BeforeDiscounts),
    // alternated so neither is the only one exercised.
    for (index, resource_type) in RESOURCE_TYPES.iter().enumerate() {
        let mode = if index % 2 == 0 {
            "AfterDiscounts"
        } else {
            "BeforeDiscounts"
        };
        let scope = scope_for(resource_type);
        let case = format!("{resource_type} savingsEstimationMode={mode}");

        // A fresh emulator per row: what the read reports is this row's write, not the last.
        let router = emulator();
        opt_in(&router).await;

        let request = json!({
            "resourceType": resource_type,
            "scope": scope,
            "savingsEstimationMode": mode,
        });
        // `put_preference` asserts HTTP 200 and the modelled empty output body.
        put_preference(&router, &request).await;

        let read = get_preferences(&router, resource_type, Some(&scope)).await;
        let details = read["recommendationPreferencesDetails"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a list of details, got {read}"));
        assert_eq!(
            details.len(),
            1,
            "case {case:?}: expected exactly the stored preference, got {read}"
        );
        assert_eq!(
            details[0],
            json!({
                "scope": scope,
                "resourceType": resource_type,
                "savingsEstimationMode": mode,
                // Not written by this request, so not reported: the detail carries the
                // members that were set.
            }),
            "case {case:?}: expected the written preference read back, got {read}"
        );
    }
}

/// The preference store is keyed by `(resourceType, scope)`: a preference written for one
/// resource type is invisible to a read of another, a scope-less read sees the records of
/// every scope of that type, and a second write to the same key MERGES into it.
#[tokio::test]
async fn preferences_are_keyed_by_resource_type_and_scope() {
    // One of the keys written below is at the organization-level scope, which the model admits
    // "for management accounts of an organization only"
    // (`PutRecommendationPreferences$scope`), so the caller here manages an organization.
    let (router, _engine) = management_account_emulator();
    opt_in(&router).await;

    let account_scope = json!({"name": SCOPE_ACCOUNT_ID, "value": ACCOUNT});
    let organization_scope = json!({"name": "Organization", "value": "ALL_ACCOUNTS"});

    // `savingsEstimationMode` is settable for every resource type, and `lookBackPeriod` for
    // the RDS DB instance type, so each write below is one the service accepts at its key.
    put_preference(
        &router,
        &json!({
            "resourceType": "RdsDBInstance",
            "scope": account_scope,
            "savingsEstimationMode": "BeforeDiscounts",
            "lookBackPeriod": "DAYS_14",
        }),
    )
    .await;
    put_preference(
        &router,
        &json!({
            "resourceType": "RdsDBInstance",
            "scope": organization_scope,
            "savingsEstimationMode": "AfterDiscounts",
        }),
    )
    .await;
    put_preference(
        &router,
        &json!({
            "resourceType": "AuroraDBClusterStorage",
            "scope": account_scope,
            "savingsEstimationMode": "AfterDiscounts",
        }),
    )
    .await;

    // Keyed by scope: the account-scoped read sees only the account-scoped record.
    let read = get_preferences(&router, "RdsDBInstance", Some(&account_scope)).await;
    assert_eq!(
        read["recommendationPreferencesDetails"],
        json!([{
            "scope": account_scope,
            "resourceType": "RdsDBInstance",
            "savingsEstimationMode": "BeforeDiscounts",
            "lookBackPeriod": "DAYS_14",
        }]),
        "case \"account-scoped read\": got {read}"
    );

    // Keyed by resource type: a read of one type never reports another's record.
    let read = get_preferences(&router, "RdsDBInstance", None).await;
    let details = read["recommendationPreferencesDetails"]
        .as_array()
        .unwrap_or_else(|| panic!("case \"scope-less read\": expected a list, got {read}"));
    assert_eq!(
        details.len(),
        2,
        "case \"scope-less read\": expected both RdsDBInstance records, got {read}"
    );
    assert!(
        details
            .iter()
            .all(|detail| detail["resourceType"] == json!("RdsDBInstance")),
        "case \"scope-less read\": every detail must be an RdsDBInstance record, got {read}"
    );

    // "Creates a new recommendation preference or updates an existing recommendation
    // preference": a second write to the same key replaces what it sets and leaves the rest.
    put_preference(
        &router,
        &json!({
            "resourceType": "RdsDBInstance",
            "scope": account_scope,
            "savingsEstimationMode": "AfterDiscounts",
        }),
    )
    .await;
    let read = get_preferences(&router, "RdsDBInstance", Some(&account_scope)).await;
    assert_eq!(
        read["recommendationPreferencesDetails"],
        json!([{
            "scope": account_scope,
            "resourceType": "RdsDBInstance",
            "savingsEstimationMode": "AfterDiscounts",
            "lookBackPeriod": "DAYS_14",
        }]),
        "case \"upsert of one key\": got {read}"
    );

    // A preference a resource type does NOT support is still refused, whatever it holds:
    // the whole table of those lives in `tests/all/preference_resource_type_support.rs`.
    let (status, body) = call(
        &router,
        "PutRecommendationPreferences",
        json!({
            "resourceType": "RdsDBInstance",
            "scope": account_scope,
            "externalMetricsPreference": {"source": "Datadog"},
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"restricted member on an unsupported type\": expected 400, got {status}: {body}"
    );
}

/// The unrestricted status switch, at each `InferredWorkloadTypesPreference` value (the
/// model's enum: Active | Inactive), is accepted for EVERY `ResourceType` the enum admits —
/// nothing restricts it to any of them — and the status written is what the matching read
/// reports back.
#[tokio::test]
async fn a_status_switch_preference_is_settable_for_every_resource_type() {
    let member = "inferredWorkloadTypes";
    for status_value in ["Active", "Inactive"] {
        for resource_type in RESOURCE_TYPES {
            let scope = scope_for(resource_type);
            let case = format!("{member}={status_value} on {resource_type}");

            // A fresh emulator per row: what the read reports is this row's write.
            let router = emulator();
            opt_in(&router).await;

            let mut request = json!({"resourceType": resource_type, "scope": scope});
            request[member] = json!(status_value);
            // `put_preference` asserts HTTP 200 and the modelled empty output body.
            put_preference(&router, &request).await;

            let read = get_preferences(&router, resource_type, Some(&scope)).await;
            assert_eq!(
                read["recommendationPreferencesDetails"],
                json!([{
                    "scope": scope,
                    "resourceType": resource_type,
                    member: status_value,
                }]),
                "case {case:?}: expected the written preference read back, got {read}"
            );
        }
    }
}

/// One recommendation surface carrying the member `savingsEstimationMode` activates: the
/// `ResourceType` the preference is written for, the reader that reports its recommendations,
/// the response member holding them, and the member of one recommendation holding the
/// recommendation OPTIONS that carry the savings members — `None` where the recommendation
/// carries them itself.
struct GatedSurface {
    resource_type: &'static str,
    reader: &'static str,
    list: &'static str,
    options: Option<&'static str>,
}

/// The surfaces this table drives: three resource types whose recommendations carry a savings
/// opportunity "after any applying discounts" and whose analysis one emulator holds — the idle
/// resource analysis, the account's Amazon EBS volumes and its AWS Lambda functions. The
/// response members are the model's own
/// (`GetIdleRecommendationsResponse$idleRecommendations`,
/// `GetEBSVolumeRecommendationsResponse$volumeRecommendations`,
/// `GetLambdaFunctionRecommendationsResponse$lambdaFunctionRecommendations`, and the option
/// lists `VolumeRecommendation$volumeRecommendationOptions` /
/// `LambdaFunctionRecommendation$memorySizeRecommendationOptions`).
const GATED_SURFACES: &[GatedSurface] = &[
    GatedSurface {
        resource_type: RESOURCE_TYPE_IDLE,
        reader: "GetIdleRecommendations",
        list: "idleRecommendations",
        options: None,
    },
    GatedSurface {
        resource_type: RESOURCE_TYPE_EBS_VOLUME,
        reader: "GetEBSVolumeRecommendations",
        list: "volumeRecommendations",
        options: Some("volumeRecommendationOptions"),
    },
    GatedSurface {
        resource_type: RESOURCE_TYPE_LAMBDA_FUNCTION,
        reader: "GetLambdaFunctionRecommendations",
        list: "lambdaFunctionRecommendations",
        options: Some("memorySizeRecommendationOptions"),
    },
];

/// The `savingsOpportunity` / `savingsOpportunityAfterDiscounts` pair of every recommendation
/// (or every option of one, where the surface reports options) the reader of `surface` answers
/// with, in the order it reports them.
async fn reported_savings(
    router: &MultiServiceRouter,
    case: &str,
    surface: &GatedSurface,
) -> Vec<(Value, Value)> {
    // A page size inside every one of these readers' modelled `maxResults` ranges, large
    // enough to hold the whole analysis this emulator holds.
    let request = json!({"maxResults": 100});
    let (status, body) = call(router, surface.reader, request.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {} {request}, got {status}: {body}",
        surface.reader
    );
    let reported = body[surface.list]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: {} reports a list in {}, got {body}",
                surface.reader, surface.list
            )
        })
        .clone();
    let pair = |carrier: &Value| {
        (
            carrier["savingsOpportunity"].clone(),
            carrier["savingsOpportunityAfterDiscounts"].clone(),
        )
    };
    reported
        .iter()
        .flat_map(|recommendation| match surface.options {
            // A recommendation whose analysis reached no option (an `Unavailable` function)
            // carries no savings member to judge.
            Some(member) => recommendation[member]
                .as_array()
                .map(|options| options.iter().map(pair).collect::<Vec<_>>())
                .unwrap_or_default(),
            None => vec![pair(recommendation)],
        })
        .collect()
}

/// The savings-estimation-mode preference is what ACTIVATES the savings member reported net of
/// discounts: a recommendation carries `savingsOpportunityAfterDiscounts` exactly when a stored
/// record activates the preference for the resource, and carries none while no record does —
/// "specify the `AfterDiscounts` status to activate the preference, or specify
/// `BeforeDiscounts` to deactivate the preference".
///
/// The table spans the resource types the rule holds for (one per reader whose recommendation
/// carries the member) and, for each, every `SavingsEstimationMode` the model declares plus the
/// account writing no preference at all. The before-discount saving the same option reports is
/// the ORACLE for the figures: this emulator records no negotiated discount, so where the
/// member is reported it restates them.
#[tokio::test]
async fn the_savings_estimation_mode_preference_gates_the_member_it_activates() {
    // (the mode stored for the surface's resource type, whether the member is then reported)
    let mut modes: Vec<(Option<&str>, bool)> = vec![(None, false)];
    modes.extend(SAVINGS_ESTIMATION_MODE_VALUES.iter().map(|mode| {
        (
            Some(*mode),
            *mode == SAVINGS_ESTIMATION_AFTER_DISCOUNTS,
        )
    }));

    for surface in GATED_SURFACES {
        for (mode, is_reported) in &modes {
            let case = format!(
                "{} with savingsEstimationMode {}",
                surface.reader,
                mode.unwrap_or("unset")
            );
            // A fresh store per row: a preference one row stored must not decide another's.
            // The analysis is made due by moving the enrollment instant back rather than by
            // waiting, so nothing here depends on how fast the host runs.
            let (router, engine) = emulator_with_engine();
            if surface.resource_type == RESOURCE_TYPE_LAMBDA_FUNCTION {
                register_analysed_functions(&router, &engine).await;
            }
            opt_in(&router).await;
            make_analysis_due(&engine);
            if let Some(mode) = mode {
                put_preference(
                    &router,
                    &json!({
                        "resourceType": surface.resource_type,
                        "scope": {"name": SCOPE_ACCOUNT_ID, "value": ACCOUNT},
                        "savingsEstimationMode": mode,
                    }),
                )
                .await;
            }

            let reported = reported_savings(&router, &case, surface).await;
            assert!(
                reported.iter().any(|(before, _)| !before.is_null()),
                "case {case:?}: the analysis must report a savings opportunity on at least one \
                 option, or the row proves nothing"
            );
            for (before, after) in &reported {
                let expected = if *is_reported {
                    before.clone()
                } else {
                    Value::Null
                };
                assert_eq!(
                    after, &expected,
                    "case {case:?}: expected savingsOpportunityAfterDiscounts {expected} \
                     beside savingsOpportunity {before}, got {after}"
                );
            }
        }
    }
}
