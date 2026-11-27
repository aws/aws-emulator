// SPDX-License-Identifier: CC0-1.0

//! `ResetEnabledBaseline`, end to end over HTTP, with `GetEnabledBaseline` and
//! `GetBaselineOperation` reading back what it wrote.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches with,
//! so what these tests exercise is the wire behaviour a client sees — status codes, error
//! codes and JSON bodies — not the handlers' Rust signatures.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * `ResetEnabledBaseline` starts an asynchronous `RESET_ENABLED_BASELINE` operation that
//!   re-applies the configuration the `EnabledBaseline` ALREADY carries, so the applied
//!   `baselineVersion` and `parameters` read the same before, during and after it —
//!   whichever baseline of the catalog is enabled, at whichever version it offers and with
//!   whichever parameters that baseline declares applied;
//! * while that operation is in flight the enablement reads as `UNDER_CHANGE` with the reset
//!   as its `lastOperationIdentifier` and no drift assessed, because an enablement cannot be
//!   assessed against a configuration still being applied; once the clock has made the
//!   operation due the enablement is `SUCCEEDED` again and reports the drift its
//!   baseline is assessed for;
//! * every reset starts its OWN operation, so a repeated reset mints a fresh
//!   `operationIdentifier` that resolves to its own `RESET_ENABLED_BASELINE` record;
//! * a reset that overlaps an operation still changing the enablement is
//!   `ConflictException` (409) and mints nothing, whichever call started that operation, and
//!   an identifier no `EnabledBaseline` matches is `ResourceNotFoundException` (404).
//!
//! The baselines a row enables, the versions it enables them at and the parameter keys it
//! configures them with are read off `state::BASELINE_CATALOG` — the catalog
//! `ListBaselines`/`GetBaseline` publish and `EnableBaseline` resolves a `baselineIdentifier`
//! against — rather than written out by hand, so no row can drift from what this emulator
//! publishes, and which baselines report drift comes from the same catalog
//! through `state::assesses_drift`. The enum values written out here come from
//! `smithy_query(query="shape", shape="BaselineOperationType,BaselineOperationStatus,\
//! EnablementStatus,EnabledBaselineDriftStatus")` on this crate's `smithy-model/`:
//! `ENABLE_BASELINE` | `DISABLE_BASELINE` | `UPDATE_ENABLED_BASELINE` |
//! `RESET_ENABLED_BASELINE`, `SUCCEEDED` | `FAILED` | `IN_PROGRESS`, `SUCCEEDED` | `FAILED` |
//! `UNDER_CHANGE` and `IN_SYNC` | `DRIFTED`. `enabledBaselineIdentifier` is always the full
//! ARN `EnableBaseline` minted: the model constrains it to `Arn` (pattern
//! `^arn:aws[0-9a-zA-Z_\-:\/]+$`, length 20..2048) and documents it "in ARN format", so no
//! other identifier form reaches the handler.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    assesses_drift, CatalogBaseline, BASELINE_CATALOG,
};

use crate::baseline_catalog::served_arn;
use crate::landing_zone_lifecycle::{
    call, is_operation_identifier, past_the_settle_span, set_up_emulator, Answer,
};

/// One row of the reset table: which catalog baseline is enabled, at which version, with
/// which `parameters` member (`None` omits it entirely).
struct ResetCase {
    case: String,
    baseline: &'static CatalogBaseline,
    version: &'static str,
    parameters: Option<serde_json::Value>,
}

/// The reset table: rows differ along what the rule generalises over — the baseline (every
/// entry of the catalog, including the ones whose drift AWS does not assess),
/// the version it is enabled at (the OLDEST and the NEWEST version each baseline offers)
/// and the parameters applied (absent, empty, and the key the baseline declares with
/// several `Document` kinds — `EnabledBaselineParameter$value` is "a document of any type").
/// All of it is read off `state::BASELINE_CATALOG`, so a row cannot name a baseline,
/// version or parameter key this emulator does not publish.
fn reset_cases() -> Vec<ResetCase> {
    let mut cases = Vec::new();
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

        for (position, version) in [("oldest", oldest), ("newest", newest)] {
            if position == "newest" && newest == oldest {
                // A baseline offering one version is already covered by its oldest row.
                continue;
            }
            cases.push(ResetCase {
                case: format!("{} at its {position} version {version}", baseline.name),
                baseline,
                version,
                parameters: None,
            });
        }

        // The parameter rows exist only for a baseline that DECLARES one: any `parameters`
        // entry at all is refused for a baseline that declares none, so the empty list is
        // the only one those rows can carry.
        let Some(key) = baseline.parameters.first().copied() else {
            cases.push(ResetCase {
                case: format!("{} with an empty parameter list", baseline.name),
                baseline,
                version: oldest,
                parameters: Some(serde_json::json!([])),
            });
            continue;
        };
        for (kind, value) in [
            ("a string", serde_json::json!("true")),
            ("a boolean", serde_json::json!(false)),
            (
                "a nested object",
                serde_json::json!({ "enabled": true, "regions": ["us-west-2"] }),
            ),
        ] {
            cases.push(ResetCase {
                case: format!("{} with {key} as {kind}", baseline.name),
                baseline,
                version: newest,
                parameters: Some(serde_json::json!([{ "key": key, "value": value }])),
            });
        }
        cases.push(ResetCase {
            case: format!("{} with an empty parameter list", baseline.name),
            baseline,
            version: oldest,
            parameters: Some(serde_json::json!([])),
        });
    }
    cases
}

/// POST `/enable-baseline` for one row, on its own organizational unit so the rows do not
/// interact, omitting `parameters` when the row does.
async fn enable(router: &MultiServiceRouter, row: &ResetCase, index: usize) -> Answer {
    let mut body = serde_json::json!({
        "baselineIdentifier": served_arn(row.baseline),
        "baselineVersion": row.version,
        "targetIdentifier":
            format!("arn:aws:organizations::123456789012:ou/o-exampleorg/ou-rb11-{index:04}"),
    });
    if let Some(parameters) = row.parameters.clone() {
        body["parameters"] = parameters;
    }
    call(router, "/enable-baseline", body).await
}

/// POST `/reset-enabled-baseline`. `enabledBaselineIdentifier` is the only member the model
/// gives this operation: a reset carries no configuration of its own.
async fn reset(router: &MultiServiceRouter, identifier: &str) -> Answer {
    call(
        router,
        "/reset-enabled-baseline",
        serde_json::json!({ "enabledBaselineIdentifier": identifier }),
    )
    .await
}

/// POST `/get-enabled-baseline` and hand back its `enabledBaselineDetails`.
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

/// POST `/get-baseline-operation` and hand back the answer plus its `baselineOperation`.
async fn operation(
    router: &MultiServiceRouter,
    operation_identifier: &str,
) -> (Answer, serde_json::Value) {
    let answer = call(
        router,
        "/get-baseline-operation",
        serde_json::json!({ "operationIdentifier": operation_identifier }),
    )
    .await;
    let record = answer.get("baselineOperation").clone();
    (answer, record)
}

/// A member of a JSON object, as a `&str`.
fn member<'a>(value: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    value.get(name).and_then(serde_json::Value::as_str)
}

/// `statusSummary.status` of an `EnabledBaselineDetails`.
fn status(record: &serde_json::Value) -> Option<&str> {
    record
        .get("statusSummary")
        .and_then(|summary| member(summary, "status"))
}

/// `statusSummary.lastOperationIdentifier` of an `EnabledBaselineDetails`.
fn last_operation(record: &serde_json::Value) -> Option<&str> {
    record
        .get("statusSummary")
        .and_then(|summary| member(summary, "lastOperationIdentifier"))
}

/// The applied `parameters` of an `EnabledBaselineDetails`, as a list — an absent member
/// reads as the empty list, which is what "no parameters are applied" means.
fn parameters(record: &serde_json::Value) -> serde_json::Value {
    record
        .get("parameters")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]))
}

/// The `driftStatusSummary` member of a reported record, absent as `None`.
fn drift_summary(record: &serde_json::Value) -> Option<serde_json::Value> {
    record.get("driftStatusSummary").cloned()
}

/// The drift summary an enablement of `baseline` reports once its configuration is applied:
/// the whole member as the model nests it for a baseline whose drift AWS
/// assesses, and nothing at all for one it does not.
fn expected_summary(baseline: &CatalogBaseline) -> Option<serde_json::Value> {
    assesses_drift(&served_arn(baseline)).then(|| {
        serde_json::json!({
            "types": {
                "inheritance": { "status": "IN_SYNC" },
                "source": { "status": "IN_SYNC" }
            }
        })
    })
}

/// RULE: `ResetEnabledBaseline` starts an asynchronous `RESET_ENABLED_BASELINE` operation
/// that re-applies the configuration the enablement already carries; while that operation is
/// in flight the enablement is `UNDER_CHANGE` with the reset as its `lastOperationIdentifier`
/// and no drift assessed, and once the clock has made the operation due the applied version
/// and parameters are the ones the enablement had and the drift its baseline is assessed for
/// is reported again.
#[tokio::test]
async fn a_reset_re_applies_the_configuration_already_applied() {
    let router = set_up_emulator().await;

    let rows = reset_cases();
    assert!(
        rows.len() >= 2,
        "the catalog must publish enough baselines for this table to span the rule, got {}",
        rows.len()
    );

    // Every row is its own enablement on its own target, so the rows do not interact.
    let mut enabled_rows: Vec<(ResetCase, String, String)> = Vec::new();
    for (index, row) in rows.into_iter().enumerate() {
        let enabled = enable(&router, &row, index).await;
        assert_eq!(
            enabled.status, 200,
            "case {:?}: enable-baseline expected 200, got {} body {:?}",
            row.case, enabled.status, enabled.body
        );
        let arn = enabled.string("arn");
        let enable_operation = enabled.string("operationIdentifier");
        enabled_rows.push((row, arn, enable_operation));
    }

    // The enables are asynchronous too, so wait for them: an enablement still being applied
    // refuses a reset as a conflict (asserted separately below). Nothing sleeps
    // server-side; the wait is the test's.
    tokio::time::sleep(past_the_settle_span()).await;

    // What is applied BEFORE the reset, read off the wire rather than assumed, is what the
    // reset must leave alone.
    let mut applied: Vec<(ResetCase, String, String, serde_json::Value)> = Vec::new();
    for (row, arn, enable_operation) in enabled_rows {
        let before = details(&router, &row.case, &arn).await;
        assert_eq!(
            status(&before),
            Some("SUCCEEDED"),
            "case {:?}: expected the enablement applied before it is reset, got {before:?}",
            row.case
        );
        applied.push((row, arn, enable_operation, before));
    }

    let mut started: Vec<(ResetCase, String, String, serde_json::Value)> = Vec::new();
    for (row, arn, enable_operation, before) in applied {
        let answer = reset(&router, &arn).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: reset-enabled-baseline expected 200, got {} body {:?}",
            row.case, answer.status, answer.body
        );
        let reset_operation = answer.string("operationIdentifier");
        assert!(
            is_operation_identifier(&reset_operation),
            "case {:?}: expected a 36-character lowercase-hex operationIdentifier, got \
             {reset_operation:?}",
            row.case
        );
        assert_ne!(
            reset_operation, enable_operation,
            "case {:?}: a reset must start its own operation, not report the enable's \
             {enable_operation:?}",
            row.case
        );

        // The reset "starts an asynchronous operation", so a caller acting straight after
        // the call sees the transient state: the operation in flight, the enablement
        // changing, the configuration it already had still applied and no drift assessed
        // against a configuration still being applied.
        let (resolved, record) = operation(&router, &reset_operation).await;
        assert_eq!(
            resolved.status, 200,
            "case {:?}: the returned identifier must resolve, got {} body {:?}",
            row.case, resolved.status, resolved.body
        );
        assert_eq!(
            (member(&record, "operationType"), member(&record, "status")),
            (Some("RESET_ENABLED_BASELINE"), Some("IN_PROGRESS")),
            "case {:?}: expected RESET_ENABLED_BASELINE IN_PROGRESS, got {record:?}",
            row.case
        );

        let in_flight = details(&router, &row.case, &arn).await;
        assert_eq!(
            status(&in_flight),
            Some("UNDER_CHANGE"),
            "case {:?}: expected UNDER_CHANGE while the reset is in flight, got {in_flight:?}",
            row.case
        );
        assert_eq!(
            last_operation(&in_flight),
            Some(reset_operation.as_str()),
            "case {:?}: expected the reset to be the lastOperationIdentifier, got \
             {in_flight:?}",
            row.case
        );
        assert_eq!(
            (
                member(&in_flight, "baselineVersion"),
                parameters(&in_flight)
            ),
            (Some(row.version), parameters(&before)),
            "case {:?}: a reset applies the configuration already applied, so it must \
             change neither the version nor the parameters, got {in_flight:?}",
            row.case
        );
        assert_eq!(
            drift_summary(&in_flight),
            None,
            "case {:?}: expected no driftStatusSummary while the configuration is being \
             re-applied, got {in_flight:?}",
            row.case
        );

        started.push((row, arn, reset_operation, before));
    }

    for (index, (row, _, reset_operation, _)) in started.iter().enumerate() {
        for (other, _, other_operation, _) in &started[index + 1..] {
            assert_ne!(
                reset_operation, other_operation,
                "cases {:?} and {:?}: two resets must mint different operationIdentifiers, \
                 got {reset_operation:?} twice",
                row.case, other.case
            );
        }
    }

    tokio::time::sleep(past_the_settle_span()).await;

    for (row, arn, reset_operation, before) in &started {
        let (_, record) = operation(&router, reset_operation).await;
        assert_eq!(
            (member(&record, "operationType"), member(&record, "status")),
            (Some("RESET_ENABLED_BASELINE"), Some("SUCCEEDED")),
            "case {:?}: expected RESET_ENABLED_BASELINE SUCCEEDED once due, got {record:?}",
            row.case
        );
        assert!(
            record.get("endTime").is_some(),
            "case {:?}: expected an endTime once the reset is due, got {record:?}",
            row.case
        );

        let after = details(&router, &row.case, arn).await;
        assert_eq!(
            status(&after),
            Some("SUCCEEDED"),
            "case {:?}: expected SUCCEEDED once the reset is due, got {after:?}",
            row.case
        );
        assert_eq!(
            last_operation(&after),
            Some(reset_operation.as_str()),
            "case {:?}: the reset stays the lastOperationIdentifier, got {after:?}",
            row.case
        );
        assert_eq!(
            (member(&after, "baselineVersion"), parameters(&after)),
            (Some(row.version), parameters(before)),
            "case {:?}: expected the configuration already applied re-applied unchanged, \
             got {after:?}",
            row.case
        );
        // A reset re-applies a configuration and moves NOTHING else: the resource keeps its
        // ARN, its baseline and its target.
        assert_eq!(
            (
                member(&after, "arn"),
                member(&after, "baselineIdentifier"),
                member(&after, "targetIdentifier")
            ),
            (
                Some(arn.as_str()),
                member(before, "baselineIdentifier"),
                member(before, "targetIdentifier")
            ),
            "case {:?}: a reset must not move the resource, its baseline or its target, got \
             {after:?}",
            row.case
        );
        let expected = expected_summary(row.baseline);
        assert_eq!(
            drift_summary(&after),
            expected,
            "case {:?}: expected driftStatusSummary {expected:?} once the configuration is \
             applied again, got {after:?}",
            row.case
        );
    }
}

/// RULE (the repeat): every reset starts its OWN asynchronous operation, so a reset repeated
/// once the previous one is due mints a fresh `operationIdentifier` that resolves to its own
/// `RESET_ENABLED_BASELINE` record, and the configuration applied is unchanged throughout.
#[tokio::test]
async fn every_reset_starts_its_own_operation() {
    let baseline = BASELINE_CATALOG
        .iter()
        .find(|baseline| !baseline.parameters.is_empty())
        .expect("the catalog publishes a baseline that declares a parameter");
    let key = baseline.parameters[0];
    let version = baseline
        .versions
        .last()
        .copied()
        .expect("every catalog baseline offers at least one version");

    let router = set_up_emulator().await;
    let enabled = call(
        &router,
        "/enable-baseline",
        serde_json::json!({
            "baselineIdentifier": served_arn(baseline),
            "baselineVersion": version,
            "targetIdentifier": "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-rp11-1a",
            "parameters": [{ "key": key, "value": "keep me" }],
        }),
    )
    .await;
    assert_eq!(
        enabled.status, 200,
        "enable-baseline: expected 200, got {} body {:?}",
        enabled.status, enabled.body
    );
    let arn = enabled.string("arn");
    tokio::time::sleep(past_the_settle_span()).await;

    let mut seen: Vec<String> = Vec::new();
    for round in 0..3u8 {
        let case = format!("reset {round}");
        let answer = reset(&router, &arn).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} body {:?}",
            answer.status, answer.body
        );
        let reset_operation = answer.string("operationIdentifier");
        assert!(
            !seen.contains(&reset_operation),
            "case {case:?}: expected a fresh operationIdentifier, got {reset_operation:?} \
             which was already minted in {seen:?}"
        );
        seen.push(reset_operation.clone());

        tokio::time::sleep(past_the_settle_span()).await;

        let (_, record) = operation(&router, &reset_operation).await;
        assert_eq!(
            (member(&record, "operationType"), member(&record, "status")),
            (Some("RESET_ENABLED_BASELINE"), Some("SUCCEEDED")),
            "case {case:?}: expected this reset's own record SUCCEEDED, got {record:?}"
        );

        let after = details(&router, &case, &arn).await;
        assert_eq!(
            (
                member(&after, "baselineVersion"),
                parameters(&after),
                status(&after)
            ),
            (
                Some(version),
                serde_json::json!([{ "key": key, "value": "keep me" }]),
                Some("SUCCEEDED")
            ),
            "case {case:?}: expected the configuration unchanged by a repeated reset, got \
             {after:?}"
        );
    }

    // Each round's record is still its own: an earlier identifier keeps resolving to the
    // reset it named.
    for (round, reset_operation) in seen.iter().enumerate() {
        let (resolved, record) = operation(&router, reset_operation).await;
        assert_eq!(
            resolved.status, 200,
            "round {round}: expected {reset_operation:?} to keep resolving, got {} body {:?}",
            resolved.status, resolved.body
        );
        assert_eq!(
            member(&record, "operationType"),
            Some("RESET_ENABLED_BASELINE"),
            "round {round}: expected a RESET_ENABLED_BASELINE record, got {record:?}"
        );
    }
}

/// RULE: a reset that overlaps an operation still changing the enablement is refused with
/// the modelled `ConflictException` (409) and mints nothing, whichever call started that
/// operation; once the clock has made that operation due the same request is accepted.
#[tokio::test]
async fn a_reset_overlapping_an_operation_in_flight_is_a_conflict() {
    let baseline = BASELINE_CATALOG
        .iter()
        .find(|baseline| baseline.versions.len() > 1)
        .expect("the catalog publishes a baseline offering more than one version");
    let oldest = baseline.versions[0];
    let newest = baseline.versions[baseline.versions.len() - 1];

    let router = set_up_emulator().await;

    for (index, (case, overlapping)) in [
        ("while the enable is in flight", None),
        ("while an update is in flight", Some("/update-enabled-baseline")),
        ("while another reset is in flight", Some("/reset-enabled-baseline")),
    ]
    .into_iter()
    .enumerate()
    {
        let target = format!(
            "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-rc11-{index:04}"
        );
        let enabled = call(
            &router,
            "/enable-baseline",
            serde_json::json!({
                "baselineIdentifier": served_arn(baseline),
                "baselineVersion": oldest,
                "targetIdentifier": target,
            }),
        )
        .await;
        assert_eq!(
            enabled.status, 200,
            "case {case:?}: enable-baseline expected 200, got {} body {:?}",
            enabled.status, enabled.body
        );
        let arn = enabled.string("arn");

        if let Some(path) = overlapping {
            tokio::time::sleep(past_the_settle_span()).await;
            let mut body = serde_json::json!({ "enabledBaselineIdentifier": arn });
            if path == "/update-enabled-baseline" {
                body["baselineVersion"] = serde_json::json!(newest);
            }
            let first = call(&router, path, body).await;
            assert_eq!(
                first.status, 200,
                "case {case:?}: {path} expected 200, got {} body {:?}",
                first.status, first.body
            );
        }

        let refused = reset(&router, &arn).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?}: expected 409 ConflictException, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
        assert!(
            refused.body.get("operationIdentifier").is_none(),
            "case {case:?}: a refused reset must mint no operation, got {:?}",
            refused.body
        );

        // Once everything in flight is due, the SAME request is accepted.
        tokio::time::sleep(past_the_settle_span()).await;
        let accepted = reset(&router, &arn).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: the same reset once nothing is in flight: expected 200, got {} \
             body {:?}",
            accepted.status, accepted.body
        );
        let accepted_operation = accepted.string("operationIdentifier");
        let (_, record) = operation(&router, &accepted_operation).await;
        assert_eq!(
            member(&record, "operationType"),
            Some("RESET_ENABLED_BASELINE"),
            "case {case:?}: expected the accepted reset to start a RESET_ENABLED_BASELINE \
             operation, got {record:?}"
        );
    }
}

/// RULE: an identifier no `EnabledBaseline` matches is `ResourceNotFoundException` (404) from
/// `ResetEnabledBaseline` — including a well-formed ARN of another resource kind, the
/// baseline's own ARN rather than the enablement's, and the ARN of an enablement whose
/// teardown the clock has made due — and nothing is written for it.
#[tokio::test]
async fn an_identifier_no_enablement_matches_is_not_found() {
    let baseline = BASELINE_CATALOG
        .first()
        .expect("the catalog publishes at least one baseline");
    let baseline_identifier = served_arn(baseline);

    let router = set_up_emulator().await;
    let enabled = call(
        &router,
        "/enable-baseline",
        serde_json::json!({
            "baselineIdentifier": baseline_identifier,
            "baselineVersion": baseline.versions[0],
            "targetIdentifier": "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-rn11-1a",
        }),
    )
    .await;
    assert_eq!(
        enabled.status, 200,
        "enable-baseline: expected 200, got {} body {:?}",
        enabled.status, enabled.body
    );
    let torn_down_arn = enabled.string("arn");
    tokio::time::sleep(past_the_settle_span()).await;

    let disabled = call(
        &router,
        "/disable-baseline",
        serde_json::json!({ "enabledBaselineIdentifier": torn_down_arn }),
    )
    .await;
    assert_eq!(
        disabled.status, 200,
        "disable-baseline: expected 200, got {} body {:?}",
        disabled.status, disabled.body
    );
    tokio::time::sleep(past_the_settle_span()).await;

    // Every value satisfies `Arn`'s pattern and length, so the front door passes them and
    // the handler is what refuses them.
    for (case, identifier) in [
        (
            "an EnabledBaseline ARN no call minted",
            "arn:aws:controltower:us-west-2:123456789012:enabledbaseline/absent".to_string(),
        ),
        (
            "a well-formed ARN of another resource kind",
            "arn:aws:controltower:us-west-2:123456789012:landingzone/absent".to_string(),
        ),
        (
            "the baseline ARN rather than the enablement's",
            baseline_identifier.clone(),
        ),
        ("a torn-down enablement's ARN", torn_down_arn.clone()),
    ] {
        let refused = reset(&router, &identifier).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected 404 ResourceNotFoundException, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
        assert!(
            refused.body.get("operationIdentifier").is_none(),
            "case {case:?}: a refused reset must mint no operation, got {:?}",
            refused.body
        );
    }
}
