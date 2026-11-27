// SPDX-License-Identifier: CC0-1.0

//! The SOURCE drift an `EnabledBaseline` reports, end to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches with,
//! so what these tests exercise is the wire behaviour a client sees — status codes and JSON
//! bodies — not the handlers' Rust signatures.
//!
//! THE RULE under test: a deployed enablement of a baseline AWS Control Tower assesses
//! drift on is in source drift when what it has deployed no longer matches the landing
//! zone's configuration, so a deployment that leaves the landing zone configured
//! DIFFERENTLY drifts every enablement deployed before it (`DRIFTED`), one that re-applies
//! the configuration already in place drifts nothing, an enablement deployed after the
//! change matches it (`IN_SYNC`), and re-deploying a drifted enablement brings it back in
//! sync. `ListEnabledBaselines` narrows on that status through `filter.sourceDriftStatuses`
//! exactly as it reports it.
//!
//! The rows span what the rule generalises over: WHICH deployment the landing zone takes
//! (an update that moves its version, an update that re-writes its manifest at the version
//! already deployed, an update that repeats the deployed configuration, a reset), whether
//! the enablement was deployed before or after that deployment, and whether its baseline is
//! one AWS assesses drift on at all.
//!
//! Where the values come from: the drift member and its two dimensions from
//! `smithy_query(query="shape", shape="EnabledBaselineDriftStatusSummary,\
//! EnabledBaselineDriftTypes,EnabledBaselineSourceDrift,EnabledBaselineDriftStatus")` on
//! this crate's `smithy-model/` — `types.source.status`, whose value set is
//! `IN_SYNC` | `DRIFTED`, with source drift documented as occurring "when an enabled
//! baseline no longer matches the landing zone's configuration" — and the filter member
//! from `smithy_query(query="shape", shape="EnabledBaselineFilter")`
//! (`sourceDriftStatuses`, a list of one). The baselines, the landing zone versions and the
//! manifest are read off `state::BASELINE_CATALOG`,
//! `state::CATALOG_LANDING_ZONE_VERSIONS` and the shared manifest helper, so no row names a
//! baseline or version this emulator does not publish, and which baselines are assessed
//! comes from `state::assesses_drift`.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    assesses_drift, CatalogBaseline, BASELINE_CATALOG, CATALOG_LANDING_ZONE_VERSIONS,
};

use crate::baseline_catalog::served_arn;
use crate::enabled_control_lifecycle::entries;
use crate::landing_zone_lifecycle::{
    call, emulator, manifest_governing, poll_until, simple_manifest, Answer,
};

/// The version a case's landing zone is CREATED at: the oldest the catalog publishes, so a
/// row can move it forward without writing a version out by hand.
fn oldest_version() -> &'static str {
    CATALOG_LANDING_ZONE_VERSIONS
        .first()
        .expect("the catalog publishes at least one landing zone version")
}

/// The version a row updates the landing zone TO: the newest the catalog publishes.
fn newest_version() -> &'static str {
    CATALOG_LANDING_ZONE_VERSIONS
        .last()
        .expect("the catalog publishes at least one landing zone version")
}

/// A baseline AWS Control Tower assesses drift on, read off the catalog.
fn assessed_baseline() -> &'static CatalogBaseline {
    BASELINE_CATALOG
        .iter()
        .find(|baseline| assesses_drift(&served_arn(baseline)))
        .expect("the catalog publishes a baseline whose drift AWS assesses")
}

/// A baseline AWS Control Tower assesses no drift on, read off the same catalog: it must
/// report no drift summary in every phase, drifted landing zone or not.
fn unassessed_baseline() -> &'static CatalogBaseline {
    BASELINE_CATALOG
        .iter()
        .find(|baseline| !assesses_drift(&served_arn(baseline)))
        .expect("the catalog publishes a baseline whose drift AWS does not assess")
}

/// One organizational unit per row and per baseline, so no two enablements of a case
/// interact.
fn target(index: usize) -> String {
    format!("arn:aws:organizations::123456789012:ou/o-exampleorg/ou-sdrf-{index:04}")
}

/// POST `/create-landingzone` at `version` with `manifest`, asserting it was accepted, and
/// answer the landing zone's ARN.
async fn create_landing_zone(
    router: &MultiServiceRouter,
    case: &str,
    version: &str,
    manifest: serde_json::Value,
) -> String {
    let answer = call(
        router,
        "/create-landingzone",
        serde_json::json!({ "version": version, "manifest": manifest }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: create-landingzone expected 200, got {} body {:?}",
        answer.status, answer.body
    );
    answer.string("arn")
}

/// POST `/enable-baseline` for `baseline` on `target`, at the newest version it offers,
/// asserting it was accepted, and answer the enablement's ARN.
async fn enable_baseline(
    router: &MultiServiceRouter,
    case: &str,
    baseline: &CatalogBaseline,
    target: &str,
) -> String {
    let version = baseline
        .versions
        .last()
        .expect("every catalog baseline offers at least one version");
    let answer = call(
        router,
        "/enable-baseline",
        serde_json::json!({
            "baselineIdentifier": served_arn(baseline),
            "baselineVersion": version,
            "targetIdentifier": target,
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: enable-baseline expected 200, got {} body {:?}",
        answer.status, answer.body
    );
    answer.string("arn")
}

/// `GetEnabledBaseline`'s `enabledBaselineDetails` for `arn`, asserting it resolved.
async fn details(router: &MultiServiceRouter, case: &str, arn: &str) -> serde_json::Value {
    let answer = call(
        router,
        "/get-enabled-baseline",
        serde_json::json!({ "enabledBaselineIdentifier": arn }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: get-enabled-baseline expected 200, got {} body {:?}",
        answer.status, answer.body
    );
    answer.get("enabledBaselineDetails").clone()
}

/// The `driftStatusSummary` `GetEnabledBaseline` reports for `arn`, absent as `None`.
async fn reported_summary(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> Option<serde_json::Value> {
    details(router, case, arn)
        .await
        .get("driftStatusSummary")
        .cloned()
}

/// The `driftStatusSummary` `ListEnabledBaselines` reports for `arn`, absent as `None`.
async fn listed_summary(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> Option<serde_json::Value> {
    let listed = list(router, case, serde_json::json!({})).await;
    entries(listed.get("enabledBaselines"))
        .into_iter()
        .find(|entry| entry.get("arn").and_then(serde_json::Value::as_str) == Some(arn))
        .and_then(|entry| entry.get("driftStatusSummary").cloned())
}

/// POST `/list-enabled-baselines` with `body`, asserting the call succeeded.
async fn list(router: &MultiServiceRouter, case: &str, body: serde_json::Value) -> Answer {
    let listed = call(router, "/list-enabled-baselines", body.clone()).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?} ({body}): list-enabled-baselines expected 200, got {} body {:?}",
        listed.status, listed.body
    );
    listed
}

/// The ARNs a `filter` selects, sorted, so a case compares SETS.
async fn selected_arns(
    router: &MultiServiceRouter,
    case: &str,
    filter: serde_json::Value,
) -> Vec<String> {
    let listed = list(router, case, serde_json::json!({ "filter": filter })).await;
    let mut arns: Vec<String> = entries(listed.get("enabledBaselines"))
        .iter()
        .map(|entry| {
            entry
                .get("arn")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    arns.sort();
    arns
}

/// The whole drift summary an assessed enablement reports, as the model nests its two
/// dimensions.
fn summary(inheritance: &str, source: &str) -> Option<serde_json::Value> {
    Some(serde_json::json!({
        "types": {
            "inheritance": { "status": inheritance },
            "source": { "status": source }
        }
    }))
}

/// Whether `GetEnabledBaseline` reports `arn` as deployed (`SUCCEEDED`).
async fn is_deployed(router: &MultiServiceRouter, case: &str, arn: &str) -> bool {
    details(router, case, arn)
        .await
        .get("statusSummary")
        .and_then(|status_summary| status_summary.get("status"))
        .and_then(serde_json::Value::as_str)
        == Some("SUCCEEDED")
}

/// Wait for the clock to make `arn`'s deployment due, looking for the CONDITION rather than
/// for a duration.
async fn await_deployed(router: &MultiServiceRouter, case: &str, arn: &str) {
    poll_until(case, "the enablement reads as deployed", || {
        is_deployed(router, case, arn)
    })
    .await;
}

/// Wait for the clock to make the landing zone operation `identifier` due.
async fn await_landing_zone_operation(router: &MultiServiceRouter, case: &str, identifier: &str) {
    poll_until(
        case,
        "the landing zone operation reaches a terminal status",
        || async {
            let answer = call(
                router,
                "/get-landingzone-operation",
                serde_json::json!({ "operationIdentifier": identifier }),
            )
            .await;
            answer
                .get("operationDetails")
                .get("status")
                .and_then(serde_json::Value::as_str)
                == Some("SUCCEEDED")
        },
    )
    .await;
}

/// Wait for `arn` to report `expected` as the status of its source drift dimension.
async fn await_source_drift(router: &MultiServiceRouter, case: &str, arn: &str, expected: &str) {
    poll_until(case, "the enablement reports its source drift", || async {
        reported_summary(router, case, arn)
            .await
            .as_ref()
            .and_then(|summary| summary.get("types"))
            .and_then(|types| types.get("source"))
            .and_then(|source| source.get("status"))
            .and_then(serde_json::Value::as_str)
            == Some(expected)
    })
    .await;
}

/// What a row does to the landing zone once its enablements are deployed.
enum Deployment {
    /// `UpdateLandingZone` at `version`, with `manifest` when the row supplies one.
    Update {
        version: &'static str,
        manifest: Option<serde_json::Value>,
    },
    /// `ResetLandingZone`, which re-deploys the configuration already in place.
    Reset,
}

/// Start `deployment` against `landing_zone`, asserting it was accepted, and wait for the
/// clock to make it due.
async fn deploy(
    router: &MultiServiceRouter,
    case: &str,
    landing_zone: &str,
    deployment: &Deployment,
) {
    let (path, body) = match deployment {
        Deployment::Update { version, manifest } => {
            let mut body = serde_json::json!({
                "landingZoneIdentifier": landing_zone,
                "version": version,
            });
            if let Some(manifest) = manifest.clone() {
                body["manifest"] = manifest;
            }
            ("/update-landingzone", body)
        }
        Deployment::Reset => (
            "/reset-landingzone",
            serde_json::json!({ "landingZoneIdentifier": landing_zone }),
        ),
    };
    let answer = call(router, path, body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: POST {path} expected 200, got {} body {:?}",
        answer.status, answer.body
    );
    await_landing_zone_operation(router, case, &answer.string("operationIdentifier")).await;
}

/// Enable one assessed and one unassessed baseline on their own targets, wait for both
/// deployments, and assert what each reports while the landing zone is untouched.
async fn deploy_two_enablements(
    router: &MultiServiceRouter,
    case: &str,
    index: usize,
) -> (String, String) {
    let assessed = enable_baseline(router, case, assessed_baseline(), &target(index)).await;
    let unassessed =
        enable_baseline(router, case, unassessed_baseline(), &target(index + 100)).await;
    await_deployed(router, case, &assessed).await;
    await_deployed(router, case, &unassessed).await;

    let reported = reported_summary(router, case, &assessed).await;
    assert_eq!(
        reported,
        summary("IN_SYNC", "IN_SYNC"),
        "case {case:?}: a freshly deployed enablement matches its landing zone's \
         configuration, so expected {:?}, got {reported:?}",
        summary("IN_SYNC", "IN_SYNC")
    );
    let reported = reported_summary(router, case, &unassessed).await;
    assert_eq!(
        reported, None,
        "case {case:?}: a baseline AWS assesses no drift on reports no driftStatusSummary, \
         got {reported:?}"
    );
    (assessed, unassessed)
}

/// RULE: a landing zone deployment that leaves it configured DIFFERENTLY puts every
/// enablement deployed before it into source drift, whichever member of the configuration
/// it changed; re-deploying the enablement brings it back in sync, and an enablement of a
/// baseline AWS assesses no drift on reports no assessment throughout.
#[tokio::test]
async fn a_changed_landing_zone_configuration_drifts_the_enablements_under_it() {
    // The rows differ along WHICH member of the landing zone's configuration the deployment
    // changes: its version, or the manifest it deploys at the version already in place.
    let rows: Vec<(&str, Deployment)> = vec![
        (
            "an update that moves the landing zone to a newer version",
            Deployment::Update {
                version: newest_version(),
                manifest: None,
            },
        ),
        (
            "an update that deploys another manifest at the version already deployed",
            Deployment::Update {
                version: oldest_version(),
                manifest: Some(manifest_governing(&["us-west-2", "eu-west-1"])),
            },
        ),
    ];

    for (index, (case, deployment)) in rows.iter().enumerate() {
        let router = emulator();
        let landing_zone =
            create_landing_zone(&router, case, oldest_version(), simple_manifest()).await;
        let (assessed, unassessed) = deploy_two_enablements(&router, case, index).await;

        deploy(&router, case, &landing_zone, deployment).await;
        await_source_drift(&router, case, &assessed, "DRIFTED").await;

        // Both reporting operations project one stored record, so the listing says what the
        // detail says — and only the SOURCE dimension moved: nothing happened to the
        // accounts under the target organizational unit.
        let expected = summary("IN_SYNC", "DRIFTED");
        let reported = reported_summary(&router, case, &assessed).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?} once the landing zone's configuration \
             changed under the enablement, got {reported:?}"
        );
        let listed = listed_summary(&router, case, &assessed).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the listing to report {expected:?}, got {listed:?}"
        );
        let unassessed_summary = reported_summary(&router, case, &unassessed).await;
        assert_eq!(
            unassessed_summary, None,
            "case {case:?}: a baseline AWS assesses no drift on reports none however the \
             landing zone changed, got {unassessed_summary:?}"
        );

        // The filter narrows on the status the record carries, so the drifted enablement is
        // exactly what `DRIFTED` selects and `IN_SYNC` selects nothing.
        let drifted = selected_arns(
            &router,
            case,
            serde_json::json!({ "sourceDriftStatuses": ["DRIFTED"] }),
        )
        .await;
        assert_eq!(
            drifted,
            vec![assessed.clone()],
            "case {case:?}: expected sourceDriftStatuses DRIFTED to select the drifted \
             enablement, got {drifted:?}"
        );
        let in_sync = selected_arns(
            &router,
            case,
            serde_json::json!({ "sourceDriftStatuses": ["IN_SYNC"] }),
        )
        .await;
        assert!(
            in_sync.is_empty(),
            "case {case:?}: expected sourceDriftStatuses IN_SYNC to select nothing while \
             the only assessed enablement is drifted, got {in_sync:?}"
        );

        // Re-deploying the enablement is what repairs it: the assessment is withdrawn while
        // the reset is in flight, and what it deploys is the landing zone's current
        // configuration, so the enablement is in sync with it again.
        let reset = call(
            &router,
            "/reset-enabled-baseline",
            serde_json::json!({ "enabledBaselineIdentifier": assessed }),
        )
        .await;
        assert_eq!(
            reset.status, 200,
            "case {case:?}: reset-enabled-baseline expected 200, got {} body {:?}",
            reset.status, reset.body
        );
        let in_flight = reported_summary(&router, case, &assessed).await;
        assert_eq!(
            in_flight, None,
            "case {case:?}: expected no driftStatusSummary while the reset's configuration \
             is still being applied, got {in_flight:?}"
        );

        await_deployed(&router, case, &assessed).await;
        let repaired = reported_summary(&router, case, &assessed).await;
        assert_eq!(
            repaired,
            summary("IN_SYNC", "IN_SYNC"),
            "case {case:?}: expected the re-deployed enablement to match the landing zone \
             again, got {repaired:?}"
        );
    }
}

/// RULE (the other side of the same one): a landing zone deployment that re-applies the
/// configuration already in place changes nothing the enablements follow, so every
/// enablement under it stays in sync.
#[tokio::test]
async fn a_landing_zone_deployment_that_changes_nothing_drifts_nothing() {
    // The rows differ along WHICH call re-applies the deployed configuration: an update
    // repeating the version and manifest in place, and a reset, which carries neither.
    let rows: Vec<(&str, Deployment)> = vec![
        (
            "an update repeating the deployed version and manifest",
            Deployment::Update {
                version: oldest_version(),
                manifest: Some(simple_manifest()),
            },
        ),
        (
            "a reset, which re-deploys the configuration already in place",
            Deployment::Reset,
        ),
    ];

    for (index, (case, deployment)) in rows.iter().enumerate() {
        let router = emulator();
        let landing_zone =
            create_landing_zone(&router, case, oldest_version(), simple_manifest()).await;
        let (assessed, unassessed) = deploy_two_enablements(&router, case, index + 10).await;

        deploy(&router, case, &landing_zone, deployment).await;

        let expected = summary("IN_SYNC", "IN_SYNC");
        let reported = reported_summary(&router, case, &assessed).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?} after a deployment that changed nothing, \
             got {reported:?}"
        );
        // A report does not disturb what a later caller sees.
        let again = reported_summary(&router, case, &assessed).await;
        assert_eq!(
            again, expected,
            "case {case:?}: expected the drift summary to be stable across reads, got \
             {again:?}"
        );
        let unassessed_summary = reported_summary(&router, case, &unassessed).await;
        assert_eq!(
            unassessed_summary, None,
            "case {case:?}: a baseline AWS assesses no drift on reports none here either, \
             got {unassessed_summary:?}"
        );
        let drifted = selected_arns(
            &router,
            case,
            serde_json::json!({ "sourceDriftStatuses": ["DRIFTED"] }),
        )
        .await;
        assert!(
            drifted.is_empty(),
            "case {case:?}: expected sourceDriftStatuses DRIFTED to select nothing, got \
             {drifted:?}"
        );
    }
}

/// RULE (the same one, over WHEN an enablement was deployed): what decides an enablement's
/// source drift is whether its own deployment is older than the landing zone's latest
/// configuration change, so one enabled AFTER that change is in sync while one enabled
/// before it is drifted — and the filter partitions the listing between them.
#[tokio::test]
async fn an_enablement_deployed_after_the_change_matches_the_landing_zone() {
    let case = "one enablement on each side of the landing zone's configuration change";
    let router = emulator();
    let landing_zone = create_landing_zone(&router, case, oldest_version(), simple_manifest()).await;

    let baseline = assessed_baseline();
    let before = enable_baseline(&router, case, baseline, &target(20)).await;
    await_deployed(&router, case, &before).await;

    deploy(
        &router,
        case,
        &landing_zone,
        &Deployment::Update {
            version: newest_version(),
            manifest: None,
        },
    )
    .await;
    await_source_drift(&router, case, &before, "DRIFTED").await;

    // The same baseline, enabled on another target once the landing zone carries its new
    // configuration: what it deploys IS that configuration.
    let after = enable_baseline(&router, case, baseline, &target(21)).await;
    await_deployed(&router, case, &after).await;
    let reported = reported_summary(&router, case, &after).await;
    assert_eq!(
        reported,
        summary("IN_SYNC", "IN_SYNC"),
        "case {case:?}: an enablement deployed after the change matches the landing zone, \
         so expected {:?}, got {reported:?}",
        summary("IN_SYNC", "IN_SYNC")
    );

    let mut both = vec![before.clone(), after.clone()];
    both.sort();
    for (filter_case, status, expected) in [
        ("sourceDriftStatuses DRIFTED", "DRIFTED", vec![before.clone()]),
        ("sourceDriftStatuses IN_SYNC", "IN_SYNC", vec![after.clone()]),
    ] {
        let selected = selected_arns(
            &router,
            filter_case,
            serde_json::json!({ "sourceDriftStatuses": [status] }),
        )
        .await;
        assert_eq!(
            selected, expected,
            "case {filter_case:?}: expected {expected:?}, got {selected:?}"
        );
    }
    // The inheritance dimension did not move for either of them, so the listing is not
    // partitioned by THAT filter: both enablements still carry IN_SYNC there.
    let inheritance_in_sync = selected_arns(
        &router,
        case,
        serde_json::json!({ "inheritanceDriftStatuses": ["IN_SYNC"] }),
    )
    .await;
    assert_eq!(
        inheritance_in_sync, both,
        "case {case:?}: expected inheritanceDriftStatuses IN_SYNC to select both \
         enablements, got {inheritance_in_sync:?}"
    );
}
