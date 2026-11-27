// SPDX-License-Identifier: CC0-1.0

//! The ONE drift-summary projection per resource kind, driven through BOTH reporting
//! operations of each kind, end to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches with,
//! so what these tests exercise is the wire behaviour a client sees — status codes and JSON
//! bodies — not the handlers' Rust signatures.
//!
//! The rule under test, asserted over a TABLE rather than one case: a resource's reported
//! `driftStatusSummary` is a projection of the drift the resource has STORED, so the detail
//! operation and the listing operation report the same member, nested the same way, for the
//! same stored record — in every drift state that record can be in. The rows differ along
//! what the rule generalises over: the resource kind (enabled baseline, enabled control),
//! the drift state (not assessed, assessed in sync, withdrawn while a change is in flight),
//! and, for a baseline, whether its drift is assessed at all.
//!
//! The baseline rows are `state::BASELINE_CATALOG` itself, so every baseline the service
//! publishes is covered, including the ones whose drift AWS does not assess; which ones
//! those are comes from the model (`smithy_query(query="shape",
//! shape="EnabledBaselineDriftTypes")` documents that Control Tower detects inheritance
//! drift on enabled baselines that target OUs and does not detect drift on the baselines
//! that apply to the landing zone), read through `state::assesses_drift`. The nesting each
//! summary is asserted at, and the status value sets written out by hand, come from
//! `smithy_query(query="shape", shape="EnabledBaselineDriftStatusSummary,\
//! EnabledBaselineDriftStatus,DriftStatusSummary,DriftStatus")`: a baseline summary carries
//! `types.inheritance.status` and `types.source.status` (`IN_SYNC` | `DRIFTED`), while a
//! control summary carries its own `driftStatus` plus `types.inheritance.status` and
//! `types.resource.status` (`DRIFTED` | `IN_SYNC` | `NOT_CHECKING` | `UNKNOWN`).

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    assesses_drift, CatalogBaseline, BASELINE_CATALOG,
};

use crate::baseline_catalog::served_arn;
use crate::enabled_control_lifecycle::{control, details as control_details, entries, ou};
use crate::landing_zone_lifecycle::{call, past_the_settle_span, set_up_emulator};

/// The `driftStatusSummary` member of a reported record, absent as `None`.
fn drift_summary(record: &serde_json::Value) -> Option<serde_json::Value> {
    record.get("driftStatusSummary").cloned()
}

/// The one entry of a listing carrying `arn`, or `Value::Null` when the listing holds none.
fn entry_for(listed: &serde_json::Value, arn: &str) -> serde_json::Value {
    entries(listed)
        .into_iter()
        .find(|entry| entry.get("arn").and_then(serde_json::Value::as_str) == Some(arn))
        .unwrap_or(serde_json::Value::Null)
}

/// `GetEnabledBaseline`'s `enabledBaselineDetails` for `identifier`, asserting it resolved.
async fn baseline_details(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
) -> serde_json::Value {
    let got = call(
        router,
        "/get-enabled-baseline",
        serde_json::json!({ "enabledBaselineIdentifier": identifier }),
    )
    .await;
    assert_eq!(
        got.status, 200,
        "case {case:?}: get-enabled-baseline expected 200, got {} body {:?}",
        got.status, got.body
    );
    got.get("enabledBaselineDetails").clone()
}

/// The whole `ListEnabledBaselines` listing, as the `enabledBaselines` member.
async fn baseline_listing(router: &MultiServiceRouter, case: &str) -> serde_json::Value {
    let listed = call(router, "/list-enabled-baselines", serde_json::json!({})).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: list-enabled-baselines expected 200, got {} body {:?}",
        listed.status, listed.body
    );
    listed.get("enabledBaselines").clone()
}

/// The whole `ListEnabledControls` listing, as the `enabledControls` member.
async fn control_listing(router: &MultiServiceRouter, case: &str) -> serde_json::Value {
    let listed = call(router, "/list-enabled-controls", serde_json::json!({})).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: list-enabled-controls expected 200, got {} body {:?}",
        listed.status, listed.body
    );
    listed.get("enabledControls").clone()
}

/// Assert the two reporting operations report the SAME drift summary for one stored record,
/// and that it is the summary `expected` says the record's stored drift projects onto.
fn assert_agrees(
    case: &str,
    phase: &str,
    detail: &serde_json::Value,
    listed_entry: &serde_json::Value,
    expected: Option<serde_json::Value>,
) {
    let from_detail = drift_summary(detail);
    let from_listing = drift_summary(listed_entry);
    assert_eq!(
        from_detail, expected,
        "case {case:?} ({phase}): the detail expected driftStatusSummary {expected:?}, got \
         {from_detail:?}"
    );
    assert_eq!(
        from_listing, expected,
        "case {case:?} ({phase}): the listing expected driftStatusSummary {expected:?}, got \
         {from_listing:?}"
    );
    assert_eq!(
        from_detail, from_listing,
        "case {case:?} ({phase}): the detail and the listing must report one projection of \
         one stored record, got {from_detail:?} and {from_listing:?}"
    );
}

/// The drift summary a DEPLOYED enablement of `baseline` projects onto: the whole member as
/// the model nests it for a baseline whose drift AWS assesses, and nothing at
/// all for one it does not.
fn expected_baseline_summary(baseline: &CatalogBaseline) -> Option<serde_json::Value> {
    assesses_drift(&served_arn(baseline)).then(|| {
        serde_json::json!({
            "types": {
                "inheritance": { "status": "IN_SYNC" },
                "source": { "status": "IN_SYNC" }
            }
        })
    })
}

/// The drift summary a control whose three stored dimensions all read `status` projects
/// onto, as the model nests it.
fn expected_control_summary(status: &str) -> Option<serde_json::Value> {
    Some(serde_json::json!({
        "driftStatus": status,
        "types": {
            "inheritance": { "status": status },
            "resource": { "status": status }
        }
    }))
}

/// The call one row starts its next control change with: both deploy a configuration, so
/// both withdraw the drift assessment the deployed one earned.
enum Change {
    /// `ResetEnabledControl`, which re-deploys the configuration the control already
    /// carries and so needs no parameters.
    Reset,
    /// `UpdateEnabledControl` with the configuration it asks for.
    Update(serde_json::Value),
}

/// RULE: `GetEnabledBaseline` and `ListEnabledBaselines` report one projection of one
/// stored enablement, so for every baseline the service publishes and in every drift state
/// an enablement reaches — nothing deployed yet, deployed, and a change in flight again —
/// the detail and the listing report the same `driftStatusSummary`, nested as the model
/// declares it and absent when no drift is assessed.
#[tokio::test]
async fn both_enabled_baseline_readers_report_one_projection() {
    let router = set_up_emulator().await;

    // Every baseline the catalog publishes, each on its own target so the rows do not
    // interact.
    let mut rows: Vec<(&CatalogBaseline, String)> = Vec::new();
    for (index, baseline) in BASELINE_CATALOG.iter().enumerate() {
        let version = baseline
            .versions
            .last()
            .expect("every catalog baseline offers at least one version");
        let enabled = call(
            &router,
            "/enable-baseline",
            serde_json::json!({
                "baselineIdentifier": served_arn(baseline),
                "baselineVersion": version,
                "targetIdentifier":
                    format!("arn:aws:organizations::123456789012:ou/o-exampleorg/ou-dsp1-{index:04}"),
            }),
        )
        .await;
        assert_eq!(
            enabled.status, 200,
            "case {:?}: enable-baseline expected 200, got {} body {:?}",
            baseline.name, enabled.status, enabled.body
        );
        rows.push((baseline, enabled.string("arn")));
    }
    assert!(
        rows.len() >= 2,
        "the catalog must publish at least two baselines for this table to span the rule, \
         got {}",
        rows.len()
    );

    // Nothing is deployed while the enabling operation is in flight, so no drift is
    // assessed for any baseline and both readers report no member at all.
    let listing = baseline_listing(&router, "in flight").await;
    for (baseline, arn) in &rows {
        let detail = baseline_details(&router, baseline.name, arn).await;
        assert_agrees(
            baseline.name,
            "the enablement is still being deployed",
            &detail,
            &entry_for(&listing, arn),
            None,
        );
    }

    // The clock makes every enablement due; nothing sleeps server-side, the wait is the
    // test's.
    tokio::time::sleep(past_the_settle_span()).await;

    let listing = baseline_listing(&router, "deployed").await;
    for (baseline, arn) in &rows {
        let detail = baseline_details(&router, baseline.name, arn).await;
        assert_agrees(
            baseline.name,
            "the enablement is deployed",
            &detail,
            &entry_for(&listing, arn),
            expected_baseline_summary(baseline),
        );
    }

    // A change in flight withdraws the assessment the deployed configuration earned, and
    // both readers stop reporting the member together.
    let mut updated: Vec<(&CatalogBaseline, String)> = Vec::new();
    for (baseline, arn) in &rows {
        let newest = baseline
            .versions
            .last()
            .copied()
            .expect("every catalog baseline offers at least one version");
        let Some(other_version) = baseline
            .versions
            .iter()
            .copied()
            .find(|version| *version != newest)
        else {
            continue;
        };
        let answer = call(
            &router,
            "/update-enabled-baseline",
            serde_json::json!({
                "enabledBaselineIdentifier": arn,
                "baselineVersion": other_version,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: update-enabled-baseline expected 200, got {} body {:?}",
            baseline.name, answer.status, answer.body
        );
        updated.push((baseline, arn.clone()));
    }

    let listing = baseline_listing(&router, "update in flight").await;
    for (baseline, arn) in &updated {
        let detail = baseline_details(&router, baseline.name, arn).await;
        assert_agrees(
            baseline.name,
            "an update is still being applied",
            &detail,
            &entry_for(&listing, arn),
            None,
        );
    }

    tokio::time::sleep(past_the_settle_span()).await;

    let listing = baseline_listing(&router, "update deployed").await;
    for (baseline, arn) in &updated {
        let detail = baseline_details(&router, baseline.name, arn).await;
        assert_agrees(
            baseline.name,
            "the update is deployed",
            &detail,
            &entry_for(&listing, arn),
            expected_baseline_summary(baseline),
        );
    }
}

/// RULE (the same one, one level over): `GetEnabledControl` and `ListEnabledControls`
/// report one projection of one stored enabled control, so for any control, target and
/// configuration, and in every drift state the enablement reaches — `UNKNOWN` while a
/// change is in flight, `IN_SYNC` once it is deployed — the detail and the listing report
/// the same `driftStatusSummary`, with its own `driftStatus` and both `types` members.
#[tokio::test]
async fn both_enabled_control_readers_report_one_projection() {
    // The rows differ along what the rule generalises over: the control enabled, whether a
    // configuration is applied, and which call starts the next change — a reset, which
    // re-deploys the configuration a control already carries, or an update, which deploys
    // another one (a control is enabled at most once per target, so each row owns its
    // target).
    let rows: Vec<(&str, String, String, Option<serde_json::Value>, Change)> = vec![
        (
            "a control configured with nothing, re-deployed by a reset",
            control("AWS-GR_ENCRYPTED_VOLUMES"),
            ou("ds01"),
            None,
            Change::Reset,
        ),
        (
            "a configured control, re-valued by an update",
            control("AWS-GR_REGION_DENY"),
            ou("ds02"),
            Some(serde_json::json!([{ "key": "AllowedRegions", "value": "us-west-2" }])),
            Change::Update(
                serde_json::json!([{ "key": "AllowedRegions", "value": ["us-west-2", "eu-west-1"] }]),
            ),
        ),
        (
            "a control enabled with an empty configuration, re-deployed by a reset",
            control("AWS-GR_MFA_ENABLED_FOR_IAM_CONSOLE_ACCESS"),
            ou("ds03"),
            Some(serde_json::json!([])),
            Change::Reset,
        ),
    ];

    for (case, control_identifier, target_identifier, initial, change) in rows {
        let router = set_up_emulator().await;

        let mut body = serde_json::json!({
            "controlIdentifier": control_identifier,
            "targetIdentifier": target_identifier,
        });
        if let Some(initial) = initial.clone() {
            body["parameters"] = initial;
        }
        let enabled = call(&router, "/enable-control", body).await;
        assert_eq!(
            enabled.status, 200,
            "case {case:?}: enable-control expected 200, got {} body {:?}",
            enabled.status, enabled.body
        );
        let arn = enabled.string("arn");

        // While the enablement is in flight the service is not able to check any drift
        // dimension, so every one of them reads `UNKNOWN` through both readers.
        let listing = control_listing(&router, case).await;
        assert_agrees(
            case,
            "the enablement is still being deployed",
            &control_details(&router, &arn).await,
            &entry_for(&listing, &arn),
            expected_control_summary("UNKNOWN"),
        );

        tokio::time::sleep(past_the_settle_span()).await;

        let listing = control_listing(&router, case).await;
        assert_agrees(
            case,
            "the enablement is deployed",
            &control_details(&router, &arn).await,
            &entry_for(&listing, &arn),
            expected_control_summary("IN_SYNC"),
        );

        // A configuration change withdraws the assessment again, and both readers follow
        // the stored statuses back to `UNKNOWN` together. An update must deploy a
        // configuration that DIFFERS from the applied one, so a control that declares no
        // parameter is changed by a reset instead.
        let (path, body) = match &change {
            Change::Reset => (
                "/reset-enabled-control",
                serde_json::json!({ "enabledControlIdentifier": arn }),
            ),
            Change::Update(requested) => (
                "/update-enabled-control",
                serde_json::json!({
                    "enabledControlIdentifier": arn,
                    "parameters": requested,
                }),
            ),
        };
        let answer = call(&router, path, body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: POST {path} expected 200, got {} body {:?}",
            answer.status, answer.body
        );

        let listing = control_listing(&router, case).await;
        assert_agrees(
            case,
            "an update is still being applied",
            &control_details(&router, &arn).await,
            &entry_for(&listing, &arn),
            expected_control_summary("UNKNOWN"),
        );

        tokio::time::sleep(past_the_settle_span()).await;

        let listing = control_listing(&router, case).await;
        assert_agrees(
            case,
            "the update is deployed",
            &control_details(&router, &arn).await,
            &entry_for(&listing, &arn),
            expected_control_summary("IN_SYNC"),
        );
    }
}
