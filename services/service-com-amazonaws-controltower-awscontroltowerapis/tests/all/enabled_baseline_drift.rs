// SPDX-License-Identifier: CC0-1.0

//! The drift an `EnabledBaseline` reports, end to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches with,
//! so what these tests exercise is the wire behaviour a client sees — status codes and JSON
//! bodies — not the handlers' Rust signatures.
//!
//! The rule under test, asserted over a TABLE spanning every baseline the service
//! publishes rather than one case: a DEPLOYED enablement reports a status for each drift
//! dimension the model declares for a baseline —
//! `driftStatusSummary.types.inheritance.status` and `driftStatusSummary.types.source.status`
//! — exactly when its baseline is one AWS Control Tower assesses drift on, and both read
//! `IN_SYNC` while no account under the target organizational unit has diverged from the
//! configuration the enablement applies to it and the landing zone's configuration has not
//! changed under it; an enablement whose deployment — of the configuration it was
//! enabled with, or of one a later call started applying — the clock has not made due yet,
//! and one of a baseline no drift is assessed for, carry no
//! `driftStatusSummary` member at all.
//!
//! The table is `state::BASELINE_CATALOG` itself — the catalog `ListBaselines`/`GetBaseline`
//! publish — so every baseline is covered, including the ones whose drift AWS does not
//! assess, and a retune of the catalog retunes these cases. Which baselines are assessed
//! comes from the model: `EnabledBaselineDriftTypes`
//! (`smithy_query(query="shape", shape="EnabledBaselineDriftTypes")`) documents that
//! Control Tower detects inheritance drift on the enabled baselines that target OUs and
//! does not detect drift on the ones that apply to the landing zone, which is the catalog's
//! `detects_drift` flag the service reads through
//! `state::assesses_drift`. The status value set (`IN_SYNC` | `DRIFTED`) comes
//! from `smithy_query(query="shape", shape="EnabledBaselineDriftStatus")`.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    assesses_drift, CatalogBaseline, BASELINE_CATALOG,
};

use crate::baseline_catalog::served_arn;
use crate::landing_zone_lifecycle::{
    call, past_the_settle_span, poll_until, set_up_emulator, Answer,
};

/// The ARN the service serves `baseline` under, in the region these tests sign for. The
/// sibling `baseline_catalog` suite owns that form, so the two cannot disagree about it.
fn baseline_identifier(baseline: &CatalogBaseline) -> String {
    served_arn(baseline)
}

/// POST `/enable-baseline` for one catalog baseline on its own organizational unit, at the
/// NEWEST version that baseline offers.
async fn enable(router: &MultiServiceRouter, baseline: &CatalogBaseline, index: usize) -> Answer {
    let version = baseline
        .versions
        .last()
        .expect("every catalog baseline offers at least one version");
    call(
        router,
        "/enable-baseline",
        serde_json::json!({
            "baselineIdentifier": baseline_identifier(baseline),
            "baselineVersion": version,
            "targetIdentifier":
                format!("arn:aws:organizations::123456789012:ou/o-exampleorg/ou-dr11-{index:04}"),
        }),
    )
    .await
}

/// `GetEnabledBaseline`'s `enabledBaselineDetails` for `identifier`, asserting it resolved.
async fn details(router: &MultiServiceRouter, case: &str, identifier: &str) -> serde_json::Value {
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

/// The `driftStatusSummary` member of a reported record, absent as `None`.
fn drift_summary(record: &serde_json::Value) -> Option<serde_json::Value> {
    record.get("driftStatusSummary").cloned()
}

/// Wait for the clock to make `arn`'s deployment due, looking again for the CONDITION the
/// case waits on rather than for a duration it measures.
async fn await_deployed(router: &MultiServiceRouter, case: &str, arn: &str) {
    poll_until(case, "the enablement reads as deployed", || async {
        details(router, case, arn)
            .await
            .get("statusSummary")
            .and_then(|summary| summary.get("status"))
            .and_then(serde_json::Value::as_str)
            == Some("SUCCEEDED")
    })
    .await;
}

/// The drift summary a deployed enablement of `baseline` must report: the whole member as
/// the model nests it (`driftStatusSummary.types.inheritance.status` and
/// `driftStatusSummary.types.source.status`) for a baseline whose drift AWS assesses, and
/// nothing at all for one it does not.
fn expected_summary(baseline: &CatalogBaseline) -> Option<serde_json::Value> {
    assesses_drift(&baseline_identifier(baseline)).then(|| {
        serde_json::json!({
            "types": {
                "inheritance": { "status": "IN_SYNC" },
                "source": { "status": "IN_SYNC" }
            }
        })
    })
}

/// RULE: a deployed enablement reports a drift status in each dimension exactly when its
/// baseline is one AWS Control Tower assesses drift on, and both are
/// `IN_SYNC` for an enablement nothing has diverged from; an enablement still being
/// deployed reports no drift summary, because drift is a property of a deployed
/// configuration.
#[tokio::test]
async fn a_deployed_enablement_reports_the_drift_of_its_baseline() {
    let router = set_up_emulator().await;

    // Every baseline the catalog publishes, each on its own target so the rows do not
    // interact.
    let mut rows: Vec<(&CatalogBaseline, String)> = Vec::new();
    for (index, baseline) in BASELINE_CATALOG.iter().enumerate() {
        let enabled = enable(&router, baseline, index).await;
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

    // While the enablement's operation is in flight nothing is deployed yet, so no drift
    // is assessed for any baseline — not even one AWS assesses drift on.
    for (baseline, arn) in &rows {
        let detail = details(&router, baseline.name, arn).await;
        assert_eq!(
            drift_summary(&detail),
            None,
            "case {:?}: expected no driftStatusSummary while the enablement is in flight, \
             got {detail:?}",
            baseline.name
        );
    }

    // The clock makes every enablement due; nothing sleeps server-side, and the case waits
    // for that CONDITION rather than for a duration.
    for (baseline, arn) in &rows {
        await_deployed(&router, baseline.name, arn).await;
    }

    for (baseline, arn) in &rows {
        let expected = expected_summary(baseline);
        let detail = details(&router, baseline.name, arn).await;
        assert_eq!(
            detail
                .get("statusSummary")
                .and_then(|summary| summary.get("status"))
                .and_then(serde_json::Value::as_str),
            Some("SUCCEEDED"),
            "case {:?}: expected the enablement to be deployed before its drift is read, \
             got {detail:?}",
            baseline.name
        );
        assert_eq!(
            drift_summary(&detail),
            expected,
            "case {:?}: expected driftStatusSummary {expected:?} for a deployed \
             enablement, got {detail:?}",
            baseline.name
        );

        // A report does not disturb what a later caller sees: the drift summary is the
        // same on a second read.
        let again = details(&router, baseline.name, arn).await;
        assert_eq!(
            drift_summary(&again),
            expected,
            "case {:?}: expected the drift summary to be stable across reads, got \
             {again:?}",
            baseline.name
        );

        // `parentIdentifier` stays absent whatever the drift is: every enablement here is
        // applied to an organizational unit, so none is the child of another one.
        assert!(
            detail.get("parentIdentifier").is_none(),
            "case {:?}: an OU-level enablement inherits from nothing, so it carries no \
             parentIdentifier, got {detail:?}",
            baseline.name
        );
    }
}

/// RULE (the same one, through the other reporting operation): `ListEnabledBaselines`
/// reports the drift summary of the record it lists, so the listing and
/// `GetEnabledBaseline` agree member for member — before and after an update, which
/// changes the version and parameters applied but not which baseline is enabled and so not
/// whether its drift is assessed once the new configuration is deployed; while that update
/// is in flight no assessment is reported at all.
#[tokio::test]
async fn the_listing_and_an_update_report_the_same_drift_summary() {
    let router = set_up_emulator().await;

    // The rows that can be UPDATED are the ones whose baseline offers a second version, so
    // the update asks for a version the baseline really offers; the rest are enabled too,
    // so the listing still spans the whole catalog.
    let mut rows: Vec<(&CatalogBaseline, String)> = Vec::new();
    for (index, baseline) in BASELINE_CATALOG.iter().enumerate() {
        let enabled = enable(&router, baseline, index).await;
        assert_eq!(
            enabled.status, 200,
            "case {:?}: enable-baseline expected 200, got {} body {:?}",
            baseline.name, enabled.status, enabled.body
        );
        rows.push((baseline, enabled.string("arn")));
    }

    tokio::time::sleep(past_the_settle_span()).await;

    let listed = call(&router, "/list-enabled-baselines", serde_json::json!({})).await;
    assert_eq!(
        listed.status, 200,
        "list-enabled-baselines expected 200, got {} body {:?}",
        listed.status, listed.body
    );
    let summaries = crate::enabled_control_lifecycle::entries(listed.get("enabledBaselines"));

    for (baseline, arn) in &rows {
        let expected = expected_summary(baseline);
        let summary = summaries
            .iter()
            .find(|entry| entry.get("arn").and_then(serde_json::Value::as_str) == Some(arn))
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        assert_eq!(
            drift_summary(&summary),
            expected,
            "case {:?}: expected the listing to report driftStatusSummary {expected:?}, \
             got {summary:?}",
            baseline.name
        );

        // An update applies the baseline at another version it offers: the enablement goes
        // back to UNDER_CHANGE, and the assessment the PREVIOUS configuration earned stops
        // being reported, because the configuration being applied is not deployed yet.
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
        let updated = call(
            &router,
            "/update-enabled-baseline",
            serde_json::json!({
                "enabledBaselineIdentifier": arn,
                "baselineVersion": other_version,
            }),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: update-enabled-baseline expected 200, got {} body {:?}",
            baseline.name, updated.status, updated.body
        );
        let in_flight = details(&router, baseline.name, arn).await;
        assert_eq!(
            drift_summary(&in_flight),
            None,
            "case {:?}: expected no driftStatusSummary while the update's configuration is \
             still being applied, got {in_flight:?}",
            baseline.name
        );
    }

    tokio::time::sleep(past_the_settle_span()).await;

    for (baseline, arn) in &rows {
        let expected = expected_summary(baseline);
        let detail = details(&router, baseline.name, arn).await;
        assert_eq!(
            drift_summary(&detail),
            expected,
            "case {:?}: expected driftStatusSummary {expected:?} once the update is due, \
             got {detail:?}",
            baseline.name
        );
    }
}
