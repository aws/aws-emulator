// SPDX-License-Identifier: CC0-1.0

//! The drift an enablement reports WHILE a configuration change it accepted is still being
//! applied, end to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches with,
//! so what these tests exercise is the wire behaviour a client sees — status codes and JSON
//! bodies — not the handlers' Rust signatures.
//!
//! The RULE under test: drift is a property of a DEPLOYED configuration, so an enablement
//! whose configuration a call has started applying reports no drift assessment at all until
//! the clock makes that operation due — the assessment the configuration it carried before
//! earned stops being reported the moment the change starts, and the one the new
//! configuration earns is reported once it is deployed. For an enabled baseline "no
//! assessment" is the absent `driftStatusSummary` member (the status set the model declares
//! is `IN_SYNC` | `DRIFTED`, neither of which means "unassessed"); for an enabled control it
//! is `UNKNOWN` in all three dimensions, the value for which the service "is not able to
//! check the drift status". The same rule for the call that REMOVES a deployed
//! configuration rather than applying one is `in_flight_teardown_drift`.
//!
//! Asserted over a TABLE whose rows are what the rule generalises over — WHICH call starts
//! applying a configuration to an already deployed enablement, and WHICH configuration:
//!
//! * for an enabled baseline, `UpdateEnabledBaseline` moving the applied version,
//!   rewriting the applied parameters at the version already applied, and doing both, plus
//!   `ResetEnabledBaseline` re-applying what is already applied — over every baseline the
//!   catalog publishes, including the ones whose drift AWS does not assess (an
//!   enablement that reports no assessment when deployed must report none mid-change
//!   either);
//! * for an enabled control, `UpdateEnabledControl` with a configuration that differs from
//!   the deployed one (a key added, a value re-valued, the configuration cleared) over a
//!   control the catalog publishes with parameters, and `ResetEnabledControl` over two
//!   controls whose reset starts an operation at all — a reset of a control implemented
//!   with a service control policy is refused rather than started, which is
//!   `enabled_control_reset`'s own case.
//!
//! The baselines, their versions and their parameter keys are read off
//! `state::BASELINE_CATALOG`, and the controls and their parameter keys off
//! `state::CONTROL_CATALOG` — the catalogs these operations resolve an identifier against —
//! so no row can name a baseline, version, control or parameter key this emulator does not
//! publish. Which baselines report a drift status comes from the same catalog
//! through `state::assesses_drift`. The enum values written out here come from
//! `smithy_query(query="shape", shape="EnablementStatus,EnabledBaselineDriftStatus,\
//! DriftStatus")` on this crate's `smithy-model/`: `SUCCEEDED` | `FAILED` | `UNDER_CHANGE`,
//! `IN_SYNC` | `DRIFTED`, and `DRIFTED` | `IN_SYNC` | `NOT_CHECKING` | `UNKNOWN`.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    assesses_drift, CatalogBaseline, CatalogControl, BASELINE_CATALOG,
    CONTROL_CATALOG,
};

use crate::baseline_catalog::served_arn;
use crate::enabled_control_lifecycle::{
    configured, control, details as control_details, drift_dimensions, enable_with, entries, ou,
    status_summary_status as control_status,
};
use crate::landing_zone_lifecycle::{call, past_the_settle_span, set_up_emulator, Answer};

/// Which call starts applying a configuration to an already deployed enablement of a
/// baseline, and with which configuration.
#[derive(Debug, Clone)]
enum BaselineChange {
    /// `UpdateEnabledBaseline` at `version`, carrying `parameters` when present (an absent
    /// member leaves the applied parameters as they are).
    Update {
        version: &'static str,
        parameters: Option<serde_json::Value>,
    },
    /// `ResetEnabledBaseline`, which carries no configuration of its own.
    Reset,
}

/// One row of the baseline table.
#[derive(Debug, Clone)]
struct BaselineRow {
    case: String,
    baseline: &'static CatalogBaseline,
    /// The version `EnableBaseline` applies.
    version: &'static str,
    /// The `parameters` member `EnableBaseline` is given; `None` omits it.
    parameters: Option<serde_json::Value>,
    change: BaselineChange,
}

/// The baseline table: for every baseline the catalog publishes, each kind of call that
/// starts applying a configuration to an enablement already deployed.
fn baseline_rows() -> Vec<BaselineRow> {
    let mut rows = Vec::new();
    for baseline in BASELINE_CATALOG {
        let oldest = baseline
            .versions
            .first()
            .copied()
            .expect("every catalog baseline offers at least one version");
        let newest = baseline
            .versions
            .last()
            .copied()
            .expect("every catalog baseline offers at least one version");
        // A parameters list names only keys the baseline declares, so a baseline that
        // declares none can only be given the empty list.
        let declared = baseline.parameters.first().copied();
        let rewritten = declared.map_or_else(
            || serde_json::json!([]),
            |key| serde_json::json!([{ "key": key, "value": { "on": true, "at": [1, 2] } }]),
        );
        let applied = declared.map_or_else(
            || serde_json::json!([]),
            |key| serde_json::json!([{ "key": key, "value": "false" }]),
        );

        if newest != oldest {
            rows.push(BaselineRow {
                case: format!("{}: an update moving {oldest} to {newest}", baseline.name),
                baseline,
                version: oldest,
                parameters: None,
                change: BaselineChange::Update {
                    version: newest,
                    parameters: None,
                },
            });
            rows.push(BaselineRow {
                case: format!(
                    "{}: an update moving {oldest} to {newest} and rewriting the parameters",
                    baseline.name
                ),
                baseline,
                version: oldest,
                parameters: Some(applied.clone()),
                change: BaselineChange::Update {
                    version: newest,
                    parameters: Some(rewritten.clone()),
                },
            });
        }
        rows.push(BaselineRow {
            case: format!(
                "{}: an update rewriting the parameters at the applied version {newest}",
                baseline.name
            ),
            baseline,
            version: newest,
            parameters: Some(applied),
            change: BaselineChange::Update {
                version: newest,
                parameters: Some(rewritten),
            },
        });
        rows.push(BaselineRow {
            case: format!("{}: a reset re-applying version {oldest}", baseline.name),
            baseline,
            version: oldest,
            parameters: None,
            change: BaselineChange::Reset,
        });
    }
    rows
}

/// The organizational-unit target of the `index`th row of the table `id` names, so no two
/// rows of any table enable anything on the same target.
pub(crate) fn ou_target(id: &str, index: usize) -> String {
    format!("arn:aws:organizations::123456789012:ou/o-exampleorg/ou-{id}-{index:04}")
}

/// POST `/enable-baseline` for `baseline` at `version` on `target`, carrying `parameters`
/// when present (an absent member applies the baseline's own defaults).
pub(crate) async fn enable_baseline_on(
    router: &MultiServiceRouter,
    baseline: &CatalogBaseline,
    version: &str,
    parameters: Option<serde_json::Value>,
    target: &str,
) -> Answer {
    let mut body = serde_json::json!({
        "baselineIdentifier": served_arn(baseline),
        "baselineVersion": version,
        "targetIdentifier": target,
    });
    if let Some(parameters) = parameters {
        body["parameters"] = parameters;
    }
    call(router, "/enable-baseline", body).await
}

/// POST `/enable-baseline` for one row, on its own organizational unit so the rows do not
/// interact.
async fn enable_baseline(router: &MultiServiceRouter, row: &BaselineRow, index: usize) -> Answer {
    enable_baseline_on(
        router,
        row.baseline,
        row.version,
        row.parameters.clone(),
        &ou_target("ifd1", index),
    )
    .await
}

/// Start `row`'s change against the enablement `arn` names.
async fn start_baseline_change(
    router: &MultiServiceRouter,
    row: &BaselineRow,
    arn: &str,
) -> Answer {
    match &row.change {
        BaselineChange::Update {
            version,
            parameters,
        } => {
            let mut body = serde_json::json!({
                "enabledBaselineIdentifier": arn,
                "baselineVersion": version,
            });
            if let Some(parameters) = parameters.clone() {
                body["parameters"] = parameters;
            }
            call(router, "/update-enabled-baseline", body).await
        }
        BaselineChange::Reset => {
            call(
                router,
                "/reset-enabled-baseline",
                serde_json::json!({ "enabledBaselineIdentifier": arn }),
            )
            .await
        }
    }
}

/// `GetEnabledBaseline`'s `enabledBaselineDetails` for `identifier`, asserting it resolved.
pub(crate) async fn baseline_details(
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

/// The `ListEnabledBaselines` summary carrying `identifier`, so a case asserts the listing
/// and the detail agree about the same record.
pub(crate) async fn baseline_summary(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
) -> serde_json::Value {
    let listed = call(
        router,
        "/list-enabled-baselines",
        serde_json::json!({ "maxResults": 100 }),
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: list-enabled-baselines expected 200, got {} body {:?}",
        listed.status, listed.body
    );
    entries(listed.get("enabledBaselines"))
        .into_iter()
        .find(|entry| entry.get("arn").and_then(serde_json::Value::as_str) == Some(identifier))
        .unwrap_or(serde_json::Value::Null)
}

/// The `driftStatusSummary` member of a reported record, absent as `None`.
pub(crate) fn drift_summary(record: &serde_json::Value) -> Option<serde_json::Value> {
    record.get("driftStatusSummary").cloned()
}

/// `statusSummary.status` of an `EnabledBaselineDetails` or an `EnabledBaselineSummary`.
pub(crate) fn baseline_status(record: &serde_json::Value) -> Option<&str> {
    record
        .get("statusSummary")
        .and_then(|summary| summary.get("status"))
        .and_then(serde_json::Value::as_str)
}

/// The drift summary an enablement of `baseline` reports once its configuration is
/// deployed: the whole member as the model nests it for a baseline whose drift
/// AWS assesses, and nothing at all for one it does not.
pub(crate) fn deployed_summary(baseline: &CatalogBaseline) -> Option<serde_json::Value> {
    assesses_drift(&served_arn(baseline)).then(|| {
        serde_json::json!({
            "types": {
                "inheritance": { "status": "IN_SYNC" },
                "source": { "status": "IN_SYNC" }
            }
        })
    })
}

/// RULE: an enabled baseline whose configuration a call has started applying reports no
/// `driftStatusSummary` until the clock makes that operation due — whichever call started
/// it and whichever configuration it applies — and reports the assessment of the deployed
/// configuration again once it is due.
#[tokio::test]
async fn a_baseline_change_in_flight_reports_no_drift_assessment() {
    let router = set_up_emulator().await;

    let rows = baseline_rows();
    assert!(
        rows.len() >= 4,
        "the catalog must publish enough baselines and versions for this table to span the \
         rule, got {} rows",
        rows.len()
    );

    // Every row is its own enablement on its own target, so the rows do not interact.
    let mut enabled: Vec<(BaselineRow, String)> = Vec::new();
    for (index, row) in rows.into_iter().enumerate() {
        let answer = enable_baseline(&router, &row, index).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: enable-baseline expected 200, got {} body {:?}",
            row.case, answer.status, answer.body
        );
        let arn = answer.string("arn");
        enabled.push((row, arn));
    }

    // The enables are asynchronous too, so wait for them: the assessment a change withdraws
    // has to exist first. Nothing sleeps server-side; the wait is the test's.
    tokio::time::sleep(past_the_settle_span()).await;

    for (row, arn) in &enabled {
        let expected = deployed_summary(row.baseline);
        let detail = baseline_details(&router, &row.case, arn).await;
        assert_eq!(
            (baseline_status(&detail), drift_summary(&detail)),
            (Some("SUCCEEDED"), expected.clone()),
            "case {:?}: expected a deployed enablement reporting driftStatusSummary \
             {expected:?} before the change, got {detail:?}",
            row.case
        );
    }

    // Each change is started and read back straight away, so the read falls inside the
    // window the operation is in flight.
    for (row, arn) in &enabled {
        let answer = start_baseline_change(&router, row, arn).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: the change expected 200, got {} body {:?}",
            row.case, answer.status, answer.body
        );

        let detail = baseline_details(&router, &row.case, arn).await;
        assert_eq!(
            (baseline_status(&detail), drift_summary(&detail)),
            (Some("UNDER_CHANGE"), None),
            "case {:?}: expected UNDER_CHANGE and no driftStatusSummary while the \
             configuration is still being applied, got {detail:?}",
            row.case
        );

        // The listing reads the same record, so it withdraws the assessment too.
        let summary = baseline_summary(&router, &row.case, arn).await;
        assert_eq!(
            drift_summary(&summary),
            None,
            "case {:?}: expected the listing to report no driftStatusSummary while the \
             configuration is still being applied, got {summary:?}",
            row.case
        );
    }

    tokio::time::sleep(past_the_settle_span()).await;

    for (row, arn) in &enabled {
        let expected = deployed_summary(row.baseline);
        let detail = baseline_details(&router, &row.case, arn).await;
        assert_eq!(
            (baseline_status(&detail), drift_summary(&detail)),
            (Some("SUCCEEDED"), expected.clone()),
            "case {:?}: expected driftStatusSummary {expected:?} once the configuration is \
             deployed, got {detail:?}",
            row.case
        );

        // Reporting does not disturb what a later caller sees: the assessment is the same
        // on a second read.
        let again = baseline_details(&router, &row.case, arn).await;
        assert_eq!(
            drift_summary(&again),
            expected,
            "case {:?}: expected the drift summary to be stable across reads, got {again:?}",
            row.case
        );
    }
}

/// Which call starts deploying a configuration to an already deployed enabled control.
#[derive(Debug, Clone)]
pub(crate) enum ControlChange {
    /// `UpdateEnabledControl` with a configuration differing from the deployed one, which
    /// is what the operation requires of a control whose enablement succeeded.
    Update(serde_json::Value),
    /// `ResetEnabledControl`, which re-deploys the configuration already carried.
    Reset,
}

/// One row of the control table.
#[derive(Debug, Clone)]
pub(crate) struct ControlRow {
    pub(crate) case: String,
    pub(crate) control_identifier: String,
    pub(crate) target_identifier: String,
    /// The `parameters` member `EnableControl` is given; `None` omits it.
    pub(crate) initial: Option<serde_json::Value>,
    pub(crate) change: ControlChange,
}

/// A `parameters` list over the first `count` keys `control` declares, each carrying the
/// `Document` value at the same position of `values` (cycled), so a row configures a
/// control only with keys it declares.
pub(crate) fn parameters_of(
    control: &CatalogControl,
    count: usize,
    values: &[serde_json::Value],
) -> serde_json::Value {
    serde_json::Value::Array(
        control
            .parameters
            .iter()
            .take(count)
            .enumerate()
            .map(|(index, key)| {
                serde_json::json!({ "key": key, "value": values[index % values.len()] })
            })
            .collect(),
    )
}

/// The control table: each kind of call that starts deploying a configuration to a control
/// already deployed, over a catalog control that declares parameters and the controls a
/// reset starts an operation against.
pub(crate) fn control_rows() -> Vec<ControlRow> {
    let parameterized = CONTROL_CATALOG
        .iter()
        .find(|control| control.enablable && !control.parameters.is_empty())
        .expect("the catalog must publish an enablable control that declares parameters");
    // `ResetEnabledControl` "does not work for controls implemented with SCPs", so only a
    // control the catalog publishes with another implementation has a reset to be in flight.
    let resettable: Vec<&CatalogControl> = CONTROL_CATALOG
        .iter()
        .filter(|control| control.enablable && !control.implemented_with_service_control_policy())
        .collect();
    let first_resettable = resettable
        .first()
        .copied()
        .expect("the catalog must publish an enablable control a reset works for");
    let last_resettable = resettable
        .last()
        .copied()
        .expect("the catalog must publish an enablable control a reset works for");
    let values = [
        serde_json::json!(["us-west-2", "eu-west-1"]),
        serde_json::json!("arn:aws:iam::123456789012:role/exempted"),
        serde_json::json!({ "nested": { "deep": [1, 2, 3] }, "on": true }),
    ];

    vec![
        ControlRow {
            case: format!("{}: an update adding a parameter", parameterized.name),
            control_identifier: control(parameterized.name),
            target_identifier: ou("ifd1"),
            initial: None,
            change: ControlChange::Update(parameters_of(parameterized, 1, &values)),
        },
        ControlRow {
            case: format!("{}: an update re-valuing every parameter", parameterized.name),
            control_identifier: control(parameterized.name),
            target_identifier: ou("ifd2"),
            initial: Some(parameters_of(parameterized, 1, &values)),
            change: ControlChange::Update(parameters_of(
                parameterized,
                parameterized.parameters.len(),
                &[serde_json::json!("changed"), serde_json::json!(7)],
            )),
        },
        ControlRow {
            case: format!("{}: an update clearing the configuration", parameterized.name),
            control_identifier: control(parameterized.name),
            target_identifier: ou("ifd3"),
            initial: Some(parameters_of(parameterized, 2, &values)),
            change: ControlChange::Update(serde_json::json!([])),
        },
        ControlRow {
            case: format!("{}: a reset of a deployed control", first_resettable.name),
            control_identifier: control(first_resettable.name),
            target_identifier: ou("ifd4"),
            initial: None,
            change: ControlChange::Reset,
        },
        ControlRow {
            case: format!(
                "{}: a reset of a control enabled with an empty configuration",
                last_resettable.name
            ),
            control_identifier: control(last_resettable.name),
            target_identifier: ou("ifd5"),
            initial: Some(serde_json::json!([])),
            change: ControlChange::Reset,
        },
    ]
}

/// Start `row`'s change against the enabled control `arn` names.
pub(crate) async fn start_control_change(
    router: &MultiServiceRouter,
    row: &ControlRow,
    arn: &str,
) -> Answer {
    match &row.change {
        ControlChange::Update(parameters) => {
            call(
                router,
                "/update-enabled-control",
                serde_json::json!({
                    "enabledControlIdentifier": arn,
                    "parameters": parameters.clone(),
                }),
            )
            .await
        }
        ControlChange::Reset => {
            call(
                router,
                "/reset-enabled-control",
                serde_json::json!({ "enabledControlIdentifier": arn }),
            )
            .await
        }
    }
}

/// RULE (the same one, on the sibling resource): an enabled control whose configuration a
/// call has started deploying reports `UNKNOWN` in all three drift dimensions until the
/// clock makes that operation due — whichever call started it and whichever configuration
/// it deploys — and `IN_SYNC` again once the configuration is deployed.
#[tokio::test]
async fn a_control_change_in_flight_reports_unknown_drift() {
    let router = set_up_emulator().await;

    let rows = control_rows();

    // Every row is its own enablement on its own target, so the rows do not interact.
    let mut enabled: Vec<(ControlRow, String)> = Vec::new();
    for row in rows {
        let (arn, _) = enable_with(
            &router,
            &row.control_identifier,
            &row.target_identifier,
            row.initial.clone(),
        )
        .await;
        enabled.push((row, arn));
    }

    // The enables are asynchronous too, so wait for them: a control still being deployed
    // refuses a further change as a conflict, and the assessment a change withdraws has to
    // exist first.
    tokio::time::sleep(past_the_settle_span()).await;

    for (row, arn) in &enabled {
        let detail = control_details(&router, arn).await;
        assert_eq!(
            (control_status(&detail), drift_dimensions(&detail)),
            (
                Some("SUCCEEDED"),
                (Some("IN_SYNC"), Some("IN_SYNC"), Some("IN_SYNC"))
            ),
            "case {:?}: expected a deployed control assessed IN_SYNC before the change, got \
             {detail:?}",
            row.case
        );
    }

    // Each change is started and read back straight away, so the read falls inside the
    // window the operation is in flight.
    for (row, arn) in &enabled {
        let configured_before = configured(&router, arn).await;
        let answer = start_control_change(&router, row, arn).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: the change expected 200, got {} body {:?}",
            row.case, answer.status, answer.body
        );

        let detail = control_details(&router, arn).await;
        assert_eq!(
            (control_status(&detail), drift_dimensions(&detail)),
            (
                Some("UNDER_CHANGE"),
                (Some("UNKNOWN"), Some("UNKNOWN"), Some("UNKNOWN"))
            ),
            "case {:?}: expected UNDER_CHANGE with UNKNOWN drift while the configuration is \
             still being deployed, got {detail:?}",
            row.case
        );
        assert_eq!(
            configured(&router, arn).await,
            configured_before,
            "case {:?}: the configuration in place must not move before the change is due, \
             got {detail:?}",
            row.case
        );
    }

    tokio::time::sleep(past_the_settle_span()).await;

    for (row, arn) in &enabled {
        let detail = control_details(&router, arn).await;
        assert_eq!(
            (control_status(&detail), drift_dimensions(&detail)),
            (
                Some("SUCCEEDED"),
                (Some("IN_SYNC"), Some("IN_SYNC"), Some("IN_SYNC"))
            ),
            "case {:?}: expected IN_SYNC in every dimension once the configuration is \
             deployed, got {detail:?}",
            row.case
        );
    }
}
