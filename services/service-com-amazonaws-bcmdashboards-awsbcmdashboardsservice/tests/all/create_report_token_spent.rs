// SPDX-License-Identifier: CC0-1.0

//! What a `CreateScheduledReport` `clientToken` is worth once a create has used it, end
//! to end over the wire.
//!
//! The rule under test: the `@idempotencyToken` a create is presented with keys THAT
//! create and is SPENT by it, so every later call presenting it creates no second report
//! and is REFUSED with the modelled `ConflictException` (409) naming the report already
//! created under the token — the same request again as much as one that differs from it,
//! however long after it arrives and however often, since one token keys at most one
//! create and a duplicate of it is never served that create's success. The separate
//! "creation is in progress" wording belongs only to a call that overlaps a create which
//! has not stored its report yet: there is no report to name then, and that conflict is
//! transient.
//!
//! The table spans what the rule generalises over — how the later call differs from the
//! create that spent the token (not at all, each member of the submitted report, the tags,
//! their order and count) and when it arrives (straight after that create's answer, and
//! again once every other row's calls have gone through the same store in between, so a
//! token that answered from its create only while it was the latest call is caught). The
//! recorded case, a reuse carrying a different `scheduledReport.name` straight after the
//! first answer, is one row of it.

use serde_json::{json, Value};

use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::state::service_state;

use super::create_report_client_token::{
    first_request, serve_first, stored_arns, Served, TOKEN,
};
use super::create_report_conflict_message::{already_exists_message, IN_PROGRESS_MESSAGE};
use super::dashboard_lifecycle::{call, emulator, error_code, ok_call};
use super::scheduled_report_lifecycle::source_dashboard;

/// The members of `scheduledReport` of a request under rewrite.
fn report_mut(request: &mut Value) -> &mut serde_json::Map<String, Value> {
    request["scheduledReport"]
        .as_object_mut()
        .expect("the request carries a scheduledReport object")
}

/// One row: how the call presenting the token again differs from the create that took it.
struct ReuseCase {
    label: &'static str,
    /// Rewrites the first request into the reuse. The second argument is the ARN of
    /// another dashboard this account stores, so a row can point the reuse elsewhere.
    reuse: fn(&mut Value, &str),
}

fn cases() -> Vec<ReuseCase> {
    vec![
        ReuseCase {
            label: "the identical request an SDK transport retry resends",
            reuse: |_request, _other| {},
        },
        ReuseCase {
            label: "the same request with its members in another order",
            reuse: |request, _other| {
                // The same members, spelled in another order: the token is spent either
                // way, so what the request says no longer decides anything.
                let report = report_mut(request).clone();
                let reordered: serde_json::Map<String, Value> =
                    report.into_iter().rev().collect();
                request["scheduledReport"] = Value::Object(reordered);
            },
        },
        ReuseCase {
            label: "a different report name",
            reuse: |request, _other| {
                request["scheduledReport"]["name"] = json!("Idempotent Rollup 2");
            },
        },
        ReuseCase {
            label: "a different dashboard to report on",
            reuse: |request, other| {
                request["scheduledReport"]["dashboardArn"] = json!(other);
            },
        },
        ReuseCase {
            label: "a different execution role",
            reuse: |request, _other| {
                request["scheduledReport"]["scheduledReportExecutionRoleArn"] =
                    json!("arn:aws:iam::123456789012:role/OtherReportRole");
            },
        },
        ReuseCase {
            label: "a different schedule expression",
            reuse: |request, _other| {
                request["scheduledReport"]["scheduleConfig"]["scheduleExpression"] =
                    json!("cron(0/15 * * * ? *)");
            },
        },
        ReuseCase {
            label: "a different schedule time zone",
            reuse: |request, _other| {
                request["scheduledReport"]["scheduleConfig"]["scheduleExpressionTimeZone"] =
                    json!("America/New_York");
            },
        },
        ReuseCase {
            label: "a different schedule period",
            reuse: |request, _other| {
                request["scheduledReport"]["scheduleConfig"]["schedulePeriod"]["endTime"] =
                    json!(1_830_297_600);
            },
        },
        ReuseCase {
            label: "the schedule state spelled out as the value the first create stored",
            reuse: |request, _other| {
                // `ScheduleState` values, source: the `ScheduleState` enum in the model; a
                // create that omits `state` stores ENABLED, which the first request did —
                // but naming it is a parameter the first request did not carry, so this is
                // a different request under the token, not the same one.
                request["scheduledReport"]["scheduleConfig"]["state"] = json!("ENABLED");
            },
        },
        ReuseCase {
            label: "the other schedule state",
            reuse: |request, _other| {
                request["scheduledReport"]["scheduleConfig"]["state"] = json!("DISABLED");
            },
        },
        ReuseCase {
            label: "the description dropped",
            reuse: |request, _other| {
                report_mut(request).remove("description");
            },
        },
        ReuseCase {
            label: "the widget ids dropped",
            reuse: |request, _other| {
                report_mut(request).remove("widgetIds");
            },
        },
        ReuseCase {
            label: "a different date-range override",
            reuse: |request, _other| {
                request["scheduledReport"]["widgetDateRangeOverride"]["startTime"] =
                    json!({"type": "RELATIVE", "value": "-P3M"});
            },
        },
        ReuseCase {
            label: "the same tags in the other order",
            // The tag list is ordered on the wire and the order it is submitted in is the
            // order the resource stores; either way the token is already spent.
            reuse: |request, _other| {
                let tags = request["resourceTags"]
                    .as_array_mut()
                    .expect("resourceTags is a list");
                tags.reverse();
            },
        },
        ReuseCase {
            label: "a different value on a tag the first request carried",
            reuse: |request, _other| {
                request["resourceTags"] = json!([
                    {"key": "team", "value": "platform"},
                    {"key": "env", "value": "prod"}
                ]);
            },
        },
        ReuseCase {
            label: "one more tag than the first request carried",
            reuse: |request, _other| {
                request["resourceTags"]
                    .as_array_mut()
                    .expect("resourceTags is a list")
                    .push(json!({"key": "cost-center", "value": "4711"}));
            },
        },
        ReuseCase {
            label: "the tags dropped",
            reuse: |request, _other| {
                request
                    .as_object_mut()
                    .expect("the request is an object")
                    .remove("resourceTags");
            },
        },
    ]
}

/// Present `request` to the create under a token a create already took and assert the rule:
/// the modelled conflict naming the report that create stored, with nothing created and
/// nothing changed.
async fn assert_answered_as_the_rule_says(label: &str, served: &Served, request: &Value) {
    let (status, body) = call(&served.router, "CreateScheduledReport", request).await;
    assert_eq!(
        status, 409,
        "case {label}: a create presenting a clientToken a create has spent is refused with \
         409, got {status}: {body}"
    );
    assert!(
        error_code(&body).ends_with("ConflictException"),
        "case {label}: expected ConflictException, got {body}"
    );
    assert_eq!(
        body["arn"],
        Value::Null,
        "case {label}: a refused create answers no arn, got {body}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    let expected = already_exists_message(&served.arn);
    assert_eq!(
        message, expected,
        "case {label}: expected {expected:?}, got {message:?}"
    );
    assert!(
        !message.contains(TOKEN),
        "case {label}: the presented clientToken must not appear in the refusal, got {message:?}"
    );

    // The refusal created no second report, and it touched neither the first one.
    let arns = stored_arns(&served.router).await;
    assert_eq!(
        arns,
        vec![served.arn.clone()],
        "case {label}: the token must have created exactly one report, got {arns:?}"
    );
    let reread = ok_call(
        &served.router,
        "GetScheduledReport",
        &json!({"arn": served.arn}),
    )
    .await;
    assert_eq!(
        reread, served.stored,
        "case {label}: presenting the token again must leave the stored record untouched, got {reread}"
    );
}

/// Every call presenting a token a create already took is refused in the name of the report
/// that create stored — over the table above, both straight after that create's answer and
/// again once every other row's calls have gone through in between.
#[tokio::test]
async fn a_used_client_token_answers_from_the_create_that_took_it() {
    let cases = cases();
    let mut served: Vec<Served> = Vec::new();
    for case in &cases {
        served.push(serve_first(case.label).await);
    }

    // The first pass: the caller holds the first call's answer and presents the token
    // again at once, which is the recorded case.
    for (case, served) in cases.iter().zip(&served) {
        let mut reuse = served.request.clone();
        (case.reuse)(&mut reuse, &served.other_dashboard);
        assert_answered_as_the_rule_says(case.label, served, &reuse).await;
    }

    // And again, now that every row above has created, read and presented its own token
    // through the store: what a token answers is decided by the create that took it, not
    // by how recently that create was the last call.
    for (case, served) in cases.iter().zip(&served) {
        let label = format!("{} (presented again after the whole table)", case.label);
        let mut reuse = served.request.clone();
        (case.reuse)(&mut reuse, &served.other_dashboard);
        assert_answered_as_the_rule_says(&label, served, &reuse).await;
    }
}

/// The refusal is no one-shot: however many times the token comes back it is refused the
/// same way and the store still holds the one report — while a token this service has not
/// seen is a create key of its own and creates its own report.
#[tokio::test]
async fn a_used_client_token_answers_the_same_way_however_often_it_is_presented() {
    let served = serve_first("repeated reuse").await;

    for attempt in 1..=4 {
        let repeated = format!("identical reuse attempt {attempt}");
        assert_answered_as_the_rule_says(&repeated, &served, &served.request.clone()).await;

        let mut different = served.request.clone();
        different["scheduledReport"]["name"] = json!(format!("Idempotent Rollup {attempt}"));
        let refused = format!("refused reuse attempt {attempt}");
        assert_answered_as_the_rule_says(&refused, &served, &different).await;
    }

    let mut fresh = served.request.clone();
    fresh["clientToken"] = json!("another-sdk-token-91ab");
    let second = ok_call(&served.router, "CreateScheduledReport", &fresh).await;
    let second_arn = second["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case fresh token: the create returns an arn: {second}"))
        .to_string();
    assert_ne!(
        second_arn, served.arn,
        "case fresh token: a token this service has not seen creates its own report: {second}"
    );
    let mut expected = vec![served.arn.clone(), second_arn];
    expected.sort();
    let mut arns = stored_arns(&served.router).await;
    arns.sort();
    assert_eq!(
        arns, expected,
        "case fresh token: the two tokens must have created exactly two reports"
    );
}

/// The other conflict the model documents, "a resource that ... is being created": a call
/// presenting a token whose create has claimed it and not stored its report yet is told
/// the creation is in progress — the transient refusal, which a caller may present the
/// same token for again, and which even a repeat of that create's very request is given
/// while there is no report to name. The claim is taken in the store, which is where a
/// create holds it between claiming the token and storing its report, and the call that
/// follows once that create has given the claim up succeeds.
#[tokio::test]
async fn a_create_overlapping_an_unsettled_claim_is_told_the_creation_is_in_progress() {
    let (router, engine) = emulator();
    let (dashboard_arn, widget_id) = source_dashboard(&router, "InFlightClaimSource").await;
    let request = first_request(&dashboard_arn, &widget_id);

    // A create has claimed the token and has not stored its report yet.
    {
        let state = service_state(&*engine).expect("the dashboard store");
        let claimed = state
            .lock()
            .reserve_scheduled_report_creation(TOKEN.to_string());
        assert!(
            claimed.is_none(),
            "case in flight: the token must be free before the claim, got {claimed:?}"
        );
    }

    let (status, body) = call(&router, "CreateScheduledReport", &request).await;
    assert_eq!(
        status, 409,
        "case in flight: a create overlapping an unsettled claim is refused with 409, got {status}: {body}"
    );
    assert!(
        error_code(&body).ends_with("ConflictException"),
        "case in flight: expected ConflictException, got {body}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, IN_PROGRESS_MESSAGE,
        "case in flight: expected the fixed in-progress text {IN_PROGRESS_MESSAGE:?}, got {message:?}"
    );
    let stored = stored_arns(&router).await;
    assert!(
        stored.is_empty(),
        "case in flight: the refused create must store no report, got {stored:?}"
    );

    // The conflict really was transient: once the create holding the claim has given it
    // up — as a create refused before it stores anything does — the same token works.
    {
        let state = service_state(&*engine).expect("the dashboard store");
        state.lock().release_scheduled_report_creation(TOKEN);
    }
    let created = ok_call(&router, "CreateScheduledReport", &request).await;
    let arn = created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case in flight: the create returns an arn: {created}"))
        .to_string();
    assert_eq!(
        stored_arns(&router).await,
        vec![arn.clone()],
        "case in flight: the create that got the token must have stored its report"
    );

    // And that create now answers for the token: every call presenting it, the same
    // request as much as a different one, is refused in the name of the report it created
    // — and none is told a creation is in progress any more.
    let expected = already_exists_message(&arn);
    for (branch, presented) in [
        ("the same request again", request.clone()),
        ("a different request", {
            let mut different = request.clone();
            different["scheduledReport"]["name"] = json!("In Flight Other");
            different
        }),
    ] {
        let (status, body) = call(&router, "CreateScheduledReport", &presented).await;
        assert_eq!(
            status, 409,
            "case in flight then settled ({branch}): expected 409, got {status}: {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(expected.as_str()),
            "case in flight then settled ({branch}): expected {expected:?}, got {body}"
        );
        assert_eq!(
            stored_arns(&router).await,
            vec![arn.clone()],
            "case in flight then settled ({branch}): the refusal creates no second report"
        );
    }

    // One claim per token, and it is the one that create settled.
    let settled = service_state(&*engine)
        .expect("the dashboard store")
        .lock()
        .scheduled_report_client_tokens
        .get(TOKEN)
        .and_then(|claim| claim.created_arn().map(str::to_string));
    assert_eq!(
        settled,
        Some(arn),
        "case in flight then settled: the token's claim must name the report it created, got {settled:?}"
    );
}
