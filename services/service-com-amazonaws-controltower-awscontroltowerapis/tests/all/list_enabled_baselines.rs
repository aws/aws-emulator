// SPDX-License-Identifier: CC0-1.0

//! `ListEnabledBaselines`, end to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches
//! with, so what these tests exercise is the wire behaviour a client sees — status codes,
//! error codes and JSON bodies — not the handlers' Rust signatures.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * the listing is the stored set of enablements: empty before any `EnableBaseline` (in an
//!   account Control Tower has been set up in, which is the precondition the enablement
//!   reads share — see `landing_zone_precondition` for the un-set-up account),
//!   reporting for each one the baseline, version, target and operation that call
//!   persisted — the same record `GetEnabledBaseline` reports, with the status the clock
//!   has reached — and losing an enablement the clock has torn down;
//! * the listing is narrowed by EVERY member of `filter`: a member the caller omits
//!   narrows nothing, one they send keeps the enablements whose own value its set holds,
//!   a member no enablement has a value for (`parentIdentifiers`) selects nothing, and
//!   `inheritanceDriftStatuses`/`sourceDriftStatuses` select the enablements carrying that
//!   drift status, so they select nothing while no enablement has one yet;
//! * a paginated walk returns every selected enablement exactly once, in pages of at most
//!   `maxResults`, ending when the answer carries no `nextToken`; a token this operation
//!   never handed out is refused.
//!
//! Every value set written out by hand here names its source: the `EnablementStatus`
//! value set (`SUCCEEDED` | `FAILED` | `UNDER_CHANGE`) and the `EnabledBaselineDriftStatus`
//! value set (`IN_SYNC` | `DRIFTED`) come from `smithy_query(query="shape",
//! shape="EnablementStatus,EnabledBaselineDriftStatus")`, and the `maxResults` bounds from
//! `smithy_query(query="operation", operation="ListEnabledBaselines")`
//! (`ListEnabledBaselinesMaxResults`, range 5..100) on this crate's `smithy-model/`. The
//! enablements themselves are the `enablements()` table the sibling `baseline_operations`
//! suite drives `EnableBaseline` with, so no baseline, version or target is written out
//! here either.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_controltower_awscontroltowerapis::state::assesses_drift;

use crate::baseline_operations::{disable, enable, enablements, Enablement};
use crate::enabled_control_lifecycle::entries;
use crate::front_door_validation::FRONT_DOOR_CODE;
use crate::landing_zone_lifecycle::{call, poll_until, set_up_emulator, Answer};

/// POST `/list-enabled-baselines` with `body`.
async fn list(router: &MultiServiceRouter, body: serde_json::Value) -> Answer {
    call(router, "/list-enabled-baselines", body).await
}

/// A member of a JSON object, as a `&str`.
fn text<'a>(value: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    value.get(name).and_then(serde_json::Value::as_str)
}

/// The `arn` of every entry of a listing, sorted, so a case compares SETS without
/// depending on the order the ARNs were minted in.
fn listed_arns(listed: &Answer) -> Vec<String> {
    let mut arns: Vec<String> = entries(listed.get("enabledBaselines"))
        .iter()
        .map(|entry| text(entry, "arn").unwrap_or_default().to_string())
        .collect();
    arns.sort();
    arns
}

/// The one listed entry carrying `arn`, or `Value::Null`.
fn listed_entry(listed: &Answer, arn: &str) -> serde_json::Value {
    entries(listed.get("enabledBaselines"))
        .into_iter()
        .find(|entry| text(entry, "arn") == Some(arn))
        .unwrap_or(serde_json::Value::Null)
}

/// The four members a listed summary and `GetEnabledBaseline`'s details both report, as a
/// tuple, so the two reporting operations can be compared member by member.
#[allow(clippy::type_complexity)]
fn reported(
    value: &serde_json::Value,
) -> (Option<&str>, Option<&str>, Option<&str>, Option<&str>) {
    (
        text(value, "arn"),
        text(value, "baselineIdentifier"),
        text(value, "baselineVersion"),
        text(value, "targetIdentifier"),
    )
}

/// `GetEnabledBaseline`'s `enabledBaselineDetails` for `identifier`, asserting it resolved.
async fn details(router: &MultiServiceRouter, identifier: &str) -> serde_json::Value {
    let got = call(
        router,
        "/get-enabled-baseline",
        serde_json::json!({ "enabledBaselineIdentifier": identifier }),
    )
    .await;
    assert_eq!(
        got.status, 200,
        "get-enabled-baseline {identifier}: expected 200, got {} body {:?}",
        got.status, got.body
    );
    got.get("enabledBaselineDetails").clone()
}

/// Wait for the clock to make each enablement of `arns` due, looking again for the
/// CONDITION the case waits on rather than for a duration it measures.
async fn await_all_deployed(router: &MultiServiceRouter, case: &str, arns: &[String]) {
    for arn in arns {
        poll_until(case, "the enablement reads as deployed", || async {
            details(router, arn)
                .await
                .get("statusSummary")
                .and_then(|summary| summary.get("status"))
                .and_then(serde_json::Value::as_str)
                == Some("SUCCEEDED")
        })
        .await;
    }
}

/// Enable every row of `rows` on `router`, handing back each row's `(arn,
/// operationIdentifier)` in the rows' own order.
async fn enable_rows(router: &MultiServiceRouter, rows: &[Enablement]) -> Vec<(String, String)> {
    let mut enabled = Vec::new();
    for row in rows {
        let answer = enable(router, row).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: enable-baseline expected 200, got {} body {:?}",
            row.case, answer.status, answer.body
        );
        enabled.push((answer.string("arn"), answer.string("operationIdentifier")));
    }
    enabled
}

/// RULE: the listing is the stored set of enablements — empty before any enablement, one
/// entry per enablement afterwards reporting what the call that applied it persisted and
/// the status the clock has reached, agreeing with `GetEnabledBaseline` because both read
/// one record, unchanged by being read, and without the enablement the clock has torn
/// down.
#[tokio::test]
async fn the_listing_reports_every_enablement_the_store_holds() {
    let router = set_up_emulator().await;
    // The account is set up first, because Control Tower lists enablements for an account it
    // has been set up in; what this case is about is the listing of a set-up account that
    // has enabled nothing yet.

    let empty = list(&router, serde_json::json!({})).await;
    assert_eq!(
        empty.status, 200,
        "before any enablement: expected 200, got {} body {:?}",
        empty.status, empty.body
    );
    assert!(
        listed_arns(&empty).is_empty(),
        "before any enablement: expected no entries, got {:?}",
        empty.body
    );

    let rows = enablements();
    let enabled = enable_rows(&router, &rows).await;

    let listed = list(&router, serde_json::json!({})).await;
    assert_eq!(
        listed.status, 200,
        "the populated listing: expected 200, got {} body {:?}",
        listed.status, listed.body
    );
    let mut expected: Vec<String> = enabled.iter().map(|(arn, _)| arn.clone()).collect();
    expected.sort();
    assert_eq!(
        listed_arns(&listed),
        expected,
        "the populated listing: expected exactly the enabled baselines, got {:?}",
        listed.body
    );

    for (row, (arn, operation_identifier)) in rows.iter().zip(&enabled) {
        let summary = listed_entry(&listed, arn);
        assert_eq!(
            reported(&summary),
            (
                Some(arn.as_str()),
                Some(row.baseline_identifier),
                Some(row.baseline_version),
                Some(row.target_identifier),
            ),
            "case {:?}: expected the summary to report what the enable carried, got \
             {summary:?}",
            row.case
        );
        assert_eq!(
            summary.get("statusSummary").cloned().unwrap_or_default(),
            serde_json::json!({
                "status": "UNDER_CHANGE",
                "lastOperationIdentifier": operation_identifier,
            }),
            "case {:?}: expected UNDER_CHANGE under the operation the call returned while \
             the enablement is in flight, got {summary:?}",
            row.case
        );
        // Neither member is invented: no parent enablement is recorded, and drift is only
        // assessed once the configuration is deployed — which is what
        // `GetEnabledBaseline` reports for the same resource while it is in flight.
        assert!(
            summary.get("parentIdentifier").is_none()
                && summary.get("driftStatusSummary").is_none(),
            "case {:?}: expected no parentIdentifier and no driftStatusSummary while the \
             enablement is in flight, got {summary:?}",
            row.case
        );

        let detail = details(&router, arn).await;
        assert_eq!(
            reported(&summary),
            reported(&detail),
            "case {:?}: the listing and GetEnabledBaseline must report one record, got \
             {summary:?} and {detail:?}",
            row.case
        );
        assert_eq!(
            summary.get("statusSummary").cloned().unwrap_or_default(),
            detail.get("statusSummary").cloned().unwrap_or_default(),
            "case {:?}: the two reporting operations must agree on the status, got \
             {summary:?} and {detail:?}",
            row.case
        );
    }

    // A listing reports the store; it does not disturb it.
    let again = list(&router, serde_json::json!({})).await;
    assert_eq!(
        again.body, listed.body,
        "re-listing must report the same enablements, got {:?}",
        again.body
    );

    // The clock makes every enablement's operation due; nothing sleeps server-side, and the
    // case waits for that CONDITION rather than for a duration.
    await_all_deployed(
        &router,
        "every enablement is deployed",
        &enabled
            .iter()
            .map(|(arn, _)| arn.clone())
            .collect::<Vec<String>>(),
    )
    .await;

    let settled = list(&router, serde_json::json!({})).await;
    assert_eq!(
        listed_arns(&settled),
        expected,
        "once every enablement is due: expected the same set, got {:?}",
        settled.body
    );
    for (row, (arn, operation_identifier)) in rows.iter().zip(&enabled) {
        let summary = listed_entry(&settled, arn);
        assert_eq!(
            summary.get("statusSummary").cloned().unwrap_or_default(),
            serde_json::json!({
                "status": "SUCCEEDED",
                "lastOperationIdentifier": operation_identifier,
            }),
            "case {:?}: expected SUCCEEDED under the same operation once the enablement is \
             due, got {summary:?}",
            row.case
        );
        assert_eq!(
            reported(&summary),
            (
                Some(arn.as_str()),
                Some(row.baseline_identifier),
                Some(row.baseline_version),
                Some(row.target_identifier),
            ),
            "case {:?}: the persisted members must not change as the status does, got \
             {summary:?}",
            row.case
        );
        // The deployed enablement carries the drift summary for the baselines AWS
        // assesses drift on, and none for the others — the same summary
        // `GetEnabledBaseline` reports for the same record.
        let expected_drift = assesses_drift(row.baseline_identifier).then(|| {
            serde_json::json!({
                "types": {
                    "inheritance": { "status": "IN_SYNC" },
                    "source": { "status": "IN_SYNC" }
                }
            })
        });
        assert_eq!(
            summary.get("driftStatusSummary").cloned(),
            expected_drift,
            "case {:?}: expected driftStatusSummary {expected_drift:?} once the \
             enablement is due, got {summary:?}",
            row.case
        );
        let detail = details(&router, arn).await;
        assert_eq!(
            summary.get("driftStatusSummary").cloned(),
            detail.get("driftStatusSummary").cloned(),
            "case {:?}: the two reporting operations must agree on the drift summary, got \
             {summary:?} and {detail:?}",
            row.case
        );
    }

    // An enablement the clock has torn down is no longer in the store, so it is no longer
    // in the listing — and the others still are.
    let (torn_down, _) = &enabled[0];
    let disabled = disable(&router, torn_down).await;
    assert_eq!(
        disabled.status, 200,
        "disable-baseline: expected 200, got {} body {:?}",
        disabled.status, disabled.body
    );
    expected.retain(|arn| arn != torn_down);
    // The teardown the clock owes removes the enablement; the case looks again until the
    // listing no longer holds it rather than waiting a fixed time for it.
    poll_until(
        "the torn-down enablement is gone",
        "the listing stops holding the torn-down enablement",
        || async { listed_arns(&list(&router, serde_json::json!({})).await) == expected },
    )
    .await;
    let after = list(&router, serde_json::json!({})).await;
    assert_eq!(
        listed_arns(&after),
        expected,
        "once the teardown is due: expected the torn-down enablement to be gone, got {:?}",
        after.body
    );
}

/// RULE: the listing is narrowed by every member of `filter` — a member the caller omits
/// narrows nothing, one they send keeps the enablements whose own value its set holds, and
/// a member no enablement here has a value for selects nothing.
///
/// Every expected set is computed from the same `enablements()` table the rows were
/// enabled from, so a case states which enablements the filter selects rather than
/// repeating their ARNs.
#[tokio::test]
async fn the_listing_is_narrowed_by_every_filter_member() {
    let router = set_up_emulator().await;
    let rows = enablements();
    let enabled = enable_rows(&router, &rows).await;

    // The ARNs of the rows a predicate over the table selects, sorted.
    let selects = |keep: &dyn Fn(&Enablement) -> bool| -> Vec<String> {
        let mut arns: Vec<String> = rows
            .iter()
            .zip(&enabled)
            .filter(|(row, _)| keep(row))
            .map(|(_, (arn, _))| arn.clone())
            .collect();
        arns.sort();
        arns
    };

    let all = selects(&|_| true);
    let none: Vec<String> = Vec::new();

    // Values taken off the table rather than written out: the baseline several of its
    // rows share, one only a single row carries, and the targets those two are on. Every
    // row of the table names a different target, so a target selects one row and a shared
    // baseline selects several — which is what makes the intersection cases below narrower
    // than either member alone.
    let shared_baseline = rows[0].baseline_identifier;
    let lone_baseline = rows[1].baseline_identifier;
    let first_target = rows[0].target_identifier;
    let lone_target = rows[1].target_identifier;
    // A target of the right form that no row enabled anything on.
    let absent_target = "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-zz99-9z";

    let cases: Vec<(&str, serde_json::Value, Vec<String>)> = vec![
        ("no filter at all", serde_json::json!({}), all.clone()),
        (
            "an empty filter structure",
            serde_json::json!({ "filter": {} }),
            all.clone(),
        ),
        (
            "targetIdentifiers naming one target",
            serde_json::json!({ "filter": { "targetIdentifiers": [first_target] } }),
            selects(&|row| row.target_identifier == first_target),
        ),
        (
            "targetIdentifiers naming two targets",
            serde_json::json!({
                "filter": { "targetIdentifiers": [first_target, lone_target] }
            }),
            selects(&|row| {
                row.target_identifier == first_target || row.target_identifier == lone_target
            }),
        ),
        (
            "targetIdentifiers naming a target nothing is enabled on",
            serde_json::json!({ "filter": { "targetIdentifiers": [absent_target] } }),
            none.clone(),
        ),
        (
            "baselineIdentifiers naming the baseline several rows share",
            serde_json::json!({ "filter": { "baselineIdentifiers": [shared_baseline] } }),
            selects(&|row| row.baseline_identifier == shared_baseline),
        ),
        (
            "baselineIdentifiers naming two baselines",
            serde_json::json!({
                "filter": { "baselineIdentifiers": [shared_baseline, lone_baseline] }
            }),
            selects(&|row| {
                row.baseline_identifier == shared_baseline
                    || row.baseline_identifier == lone_baseline
            }),
        ),
        (
            "a baseline and two targets at once, selecting their intersection",
            serde_json::json!({
                "filter": {
                    "baselineIdentifiers": [shared_baseline],
                    "targetIdentifiers": [first_target, lone_target]
                }
            }),
            selects(&|row| {
                row.baseline_identifier == shared_baseline
                    && (row.target_identifier == first_target
                        || row.target_identifier == lone_target)
            }),
        ),
        (
            "a baseline and a target at once whose intersection is empty",
            serde_json::json!({
                "filter": {
                    "baselineIdentifiers": [lone_baseline],
                    "targetIdentifiers": [first_target]
                }
            }),
            none.clone(),
        ),
        (
            "statuses naming the status every enablement is in while in flight",
            serde_json::json!({ "filter": { "statuses": ["UNDER_CHANGE"] } }),
            all.clone(),
        ),
        (
            "statuses naming a status no enablement is in yet",
            serde_json::json!({ "filter": { "statuses": ["SUCCEEDED"] } }),
            none.clone(),
        ),
        (
            "statuses naming a status nothing here reaches",
            serde_json::json!({ "filter": { "statuses": ["FAILED"] } }),
            none.clone(),
        ),
        (
            "parentIdentifiers, which no enablement here has a value for",
            serde_json::json!({ "filter": { "parentIdentifiers": [first_target] } }),
            none.clone(),
        ),
        (
            "inheritanceDriftStatuses IN_SYNC, which no enablement carries while its \
             deployment is still in flight",
            serde_json::json!({ "filter": { "inheritanceDriftStatuses": ["IN_SYNC"] } }),
            none.clone(),
        ),
        (
            "inheritanceDriftStatuses DRIFTED, which nothing here reaches",
            serde_json::json!({ "filter": { "inheritanceDriftStatuses": ["DRIFTED"] } }),
            none.clone(),
        ),
        (
            "sourceDriftStatuses IN_SYNC, which no enablement carries while its \
             deployment is still in flight",
            serde_json::json!({ "filter": { "sourceDriftStatuses": ["IN_SYNC"] } }),
            none.clone(),
        ),
        (
            "sourceDriftStatuses DRIFTED, which no enablement under an unchanged landing \
             zone reaches",
            serde_json::json!({ "filter": { "sourceDriftStatuses": ["DRIFTED"] } }),
            none.clone(),
        ),
        (
            "includeChildren, which widens the listing by nothing",
            serde_json::json!({ "includeChildren": true }),
            all.clone(),
        ),
        (
            "includeChildren beside a filter, which still narrows",
            serde_json::json!({
                "includeChildren": true,
                "filter": { "targetIdentifiers": [first_target] }
            }),
            selects(&|row| row.target_identifier == first_target),
        ),
        (
            "every filter member at once, one of them selecting nothing",
            serde_json::json!({
                "filter": {
                    "baselineIdentifiers": [shared_baseline],
                    "targetIdentifiers": [first_target],
                    "parentIdentifiers": [first_target],
                    "statuses": ["UNDER_CHANGE"],
                    "inheritanceDriftStatuses": ["IN_SYNC"],
                    "sourceDriftStatuses": ["IN_SYNC"]
                }
            }),
            none.clone(),
        ),
    ];

    for (case, body, expected) in cases {
        let listed = list(&router, body.clone()).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200, got {} body {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed_arns(&listed),
            expected,
            "case {case:?} ({body}): expected {expected:?}, got {:?}",
            listed_arns(&listed)
        );
    }

    // The status filter reads the store rather than a constant, so the two status cases
    // above swap over once the clock has made every enablement due — the case waits for
    // that CONDITION rather than for a duration.
    await_all_deployed(
        &router,
        "every enablement is deployed",
        &enabled
            .iter()
            .map(|(arn, _)| arn.clone())
            .collect::<Vec<String>>(),
    )
    .await;

    for (case, status, expected) in [
        ("UNDER_CHANGE once every enablement is due", "UNDER_CHANGE", none.clone()),
        ("SUCCEEDED once every enablement is due", "SUCCEEDED", all.clone()),
    ] {
        let listed = list(
            &router,
            serde_json::json!({ "filter": { "statuses": [status] } }),
        )
        .await;
        assert_eq!(
            listed_arns(&listed),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            listed.body
        );
    }

    // So do the two drift filters: the enablements of a drift-assessed baseline carry
    // IN_SYNC in both dimensions once deployed under a landing zone whose configuration has
    // not changed under them, and are the ones those filters then select.
    for (case, member, status, expected) in [
        (
            "inheritanceDriftStatuses IN_SYNC once every enablement is due",
            "inheritanceDriftStatuses",
            "IN_SYNC",
            selects(&|row| assesses_drift(row.baseline_identifier)),
        ),
        (
            "inheritanceDriftStatuses DRIFTED once every enablement is due",
            "inheritanceDriftStatuses",
            "DRIFTED",
            none.clone(),
        ),
        (
            "sourceDriftStatuses IN_SYNC once every enablement is due",
            "sourceDriftStatuses",
            "IN_SYNC",
            selects(&|row| assesses_drift(row.baseline_identifier)),
        ),
        (
            "sourceDriftStatuses DRIFTED once every enablement is due",
            "sourceDriftStatuses",
            "DRIFTED",
            none.clone(),
        ),
    ] {
        let listed = list(
            &router,
            serde_json::json!({ "filter": { member: [status] } }),
        )
        .await;
        assert_eq!(
            listed_arns(&listed),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            listed.body
        );
    }
}

/// RULE: a paginated walk returns every enablement the narrowing selected exactly once, in
/// pages of at most `maxResults`, and the walk ends when the answer carries no
/// `nextToken`; a token this operation never handed out is refused rather than read as the
/// first page.
///
/// The rows are the page sizes: the published lower bound of
/// `ListEnabledBaselinesMaxResults` (range 5..100), a size that does not divide the set
/// evenly, one equal to the set, and the published upper bound.
#[tokio::test]
async fn a_paginated_walk_returns_every_enablement_exactly_once() {
    let router = set_up_emulator().await;
    let row = enablements()
        .into_iter()
        .next()
        .expect("the enablement table has at least one row");

    // One baseline on twelve targets: the enablements are what is paginated, so the rows
    // vary only along the target, which is what makes them different resources.
    let page_target =
        |index: usize| format!("arn:aws:organizations::123456789012:ou/o-exampleorg/ou-page-{index:04}");

    let mut expected: Vec<String> = Vec::new();
    for index in 0..12 {
        let target = page_target(index);
        let enabled = call(
            &router,
            "/enable-baseline",
            serde_json::json!({
                "baselineIdentifier": row.baseline_identifier,
                "baselineVersion": row.baseline_version,
                "targetIdentifier": target,
            }),
        )
        .await;
        assert_eq!(
            enabled.status, 200,
            "enablement {index}: expected 200, got {} body {:?}",
            enabled.status, enabled.body
        );
        expected.push(enabled.string("arn"));
    }
    expected.sort();

    for page_size in [5usize, 7, 12, 100] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = serde_json::json!({ "maxResults": page_size });
            if let Some(token) = &token {
                body["nextToken"] = serde_json::json!(token);
            }
            let page = list(&router, body).await;
            assert_eq!(
                page.status, 200,
                "page size {page_size}, page {pages}: expected 200, got {} body {:?}",
                page.status, page.body
            );
            let arns = listed_arns(&page);
            assert!(
                arns.len() <= page_size && !arns.is_empty(),
                "page size {page_size}, page {pages}: expected 1..={page_size} entries, \
                 got {arns:?}"
            );
            walked.extend(arns);
            token = page
                .get("nextToken")
                .as_str()
                .map(|value| value.to_string());
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "page size {page_size}: the walk did not terminate, walked {walked:?}"
            );
            if token.is_none() {
                break;
            }
        }
        walked.sort();
        assert_eq!(
            walked, expected,
            "page size {page_size}: expected every enablement exactly once, got {walked:?}"
        );
        let full_pages = expected.len().div_ceil(page_size);
        assert_eq!(
            pages, full_pages,
            "page size {page_size}: expected {full_pages} pages for {} enablements, got \
             {pages}",
            expected.len()
        );
    }

    // A page of a NARROWED walk is narrowed too: the token resumes the same selection.
    let narrowed = list(
        &router,
        serde_json::json!({
            "maxResults": 5,
            "filter": { "targetIdentifiers": [page_target(0)] }
        }),
    )
    .await;
    assert_eq!(
        listed_arns(&narrowed).len(),
        1,
        "a narrowed page: expected the one selected enablement, got {:?}",
        narrowed.body
    );
    assert!(
        narrowed.get("nextToken").as_str().is_none(),
        "a narrowed page that holds every selected enablement must carry no nextToken, \
         got {:?}",
        narrowed.body
    );

    // A token this operation never handed out is refused, not silently read as the first
    // page — the same rule `ListEnabledControls` applies to its own token.
    for (case, token) in [
        ("a token that is not an index", "not-a-token"),
        ("an index past the selected set", "99"),
    ] {
        let refused = list(&router, serde_json::json!({ "nextToken": token })).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected 400 ValidationException, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
    }

    // And a page with no token at all is the first page. An EMPTY token is not: this
    // operation's `ListEnabledBaselinesNextToken` is modelled `pattern \S+`, so the front
    // door refuses it before the handler runs.
    let first = list(&router, serde_json::json!({ "maxResults": 5 })).await;
    assert_eq!(
        listed_arns(&first),
        expected[..5].to_vec(),
        "no nextToken: expected the first page, got {:?}",
        first.body
    );
    let empty_token = list(
        &router,
        serde_json::json!({ "nextToken": "", "maxResults": 5 }),
    )
    .await;
    assert_eq!(
        (empty_token.status, empty_token.error_code.as_deref()),
        (400, Some(FRONT_DOOR_CODE)),
        "an empty nextToken: expected the front door's 400 {FRONT_DOOR_CODE}, got {:?} \
         body {:?}",
        (empty_token.status, empty_token.error_code.as_deref()),
        empty_token.body
    );
}
