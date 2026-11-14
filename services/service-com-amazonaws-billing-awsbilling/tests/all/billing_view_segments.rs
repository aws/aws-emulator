// SPDX-License-Identifier: CC0-1.0

//! `ListBillingViewSegments`, end to end over the awsJson1_0 wire.
//!
//! Every assertion here is about the RULE the operation follows: it REPORTS the
//! segment timeline persisted in
//! `ComAmazonawsBillingAwsbillingState::billing_view_segments` for the
//! account's PRIMARY billing view — the records that overlap the half-open
//! period the request asks about and are not hidden, oldest first, each one's
//! period clipped to that requested period — paged by `maxResults`/`nextToken`,
//! and it writes nothing.
//!
//! A segment is "a time range during which the billing domain and account
//! relationships for a billing view remained unchanged", so the timeline below
//! is seeded directly in the shared engine's store (the same engine the router
//! hands every handler), exactly as the credit-allocation and Support-spend
//! suites seed theirs: no modelled operation moves an account into an
//! organization or a billing group, so nothing in the API records a change.
//!
//! The rows differ along everything the rule generalises over: whether the
//! requested period falls inside one segment, straddles a change, covers the
//! whole timeline or none of it; whether a segment is bounded on neither, one
//! or both sides; each published `BillingDomain` value; which account
//! relationships a segment carries; whether a segment is hidden; which bounds
//! the request itself carries; and the page size.
//!
//! The harness (`call`, `error_code`, `emulator_with_engine`, `PRIMARY_ARN`) is
//! the one `billing_views.rs` owns, the view creation and the store-side health
//! move are `source_view_disassociation.rs`'s, and the civil-calendar `at` is
//! `credit_allocation_history.rs`'s.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_billing_awsbilling::state::{
    BillingViewSegment, BILLING_DOMAIN_BILLABLE, BILLING_DOMAIN_PRO_FORMA,
};
use service_com_amazonaws_billing_awsbilling::ComAmazonawsBillingAwsbillingState;
use smithy_server_runtime::MultiServiceRouter;

use crate::billing_views::{
    call, emulator_with_engine, error_code, standalone_emulator_with_engine, ABSENT_ARN,
    PRIMARY_ARN,
};
use crate::credit_allocation_history::at;
use crate::source_view_disassociation::{create_view, set_health_status, source_arn};

/// A second 12-digit account, the one the seeded relationships name as the
/// organization's management account and as the billing group's primary account.
const OTHER_ACCOUNT_ID: &str = "123456789012";

/// A third 12-digit account, the billing transfer account of the segment in
/// effect.
const TRANSFER_ACCOUNT_ID: &str = "210987654321";

/// One seeded segment of the PRIMARY view's timeline: its label (what a row
/// names it by), the domain that applied, its own bounds in epoch seconds
/// (`None` for an unbounded side), the account relationships it carries, and
/// whether it is hidden from the caller.
#[derive(Debug, Clone)]
struct Seed {
    label: &'static str,
    domain: &'static str,
    begin: Option<i64>,
    end: Option<i64>,
    management_account_id: Option<&'static str>,
    billing_group_primary_account_id: Option<&'static str>,
    billing_transfer_account_id: Option<&'static str>,
    hidden: bool,
}

impl Seed {
    /// This seed as the stored record, for the billing view `arn`.
    fn segment(&self, arn: &str) -> BillingViewSegment {
        BillingViewSegment {
            domain: self.domain.to_string(),
            begin_date_inclusive: self.begin.map(instant),
            end_date_exclusive: self.end.map(instant),
            management_account_id: self.management_account_id.map(str::to_string),
            billing_group_primary_account_id: self
                .billing_group_primary_account_id
                .map(str::to_string),
            billing_transfer_account_id: self.billing_transfer_account_id.map(str::to_string),
            hidden: self.hidden,
            ..BillingViewSegment::in_effect(arn, self.domain)
        }
    }
}

/// Epoch seconds as the instant the store holds.
fn instant(secs: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(u64::try_from(secs).expect("a post-epoch instant"))
}

/// Now, in epoch seconds — the clock the default period is read off.
fn now_secs() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs(),
    )
    .expect("the clock fits an i64")
}

/// The half-open UTC calendar month the instant `secs` falls in, as epoch
/// seconds: the period the operation answers for when the request names no
/// bound of its own ("the current billing period, which is the calendar month
/// in UTC").
///
/// Read off the civil calendar by rendering the instant, so the month's length
/// is the calendar's and not a fixed number of days.
fn calendar_month(secs: i64) -> (i64, i64) {
    let rendered = aws_smithy_types::DateTime::from_secs(secs)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .expect("the instant renders on the civil calendar");
    let year: i64 = rendered[0..4].parse().expect("a four-digit year");
    let month: i64 = rendered[5..7].parse().expect("a two-digit month");
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    (at(year, month, 1, 0), at(next_year, next_month, 1, 0))
}

/// The seeded timeline of the PRIMARY view: an account whose data was its own
/// final charges, that joined a billing group mid-March 2024, spent five days
/// in a period it cannot see, and has since been an organization member and a
/// billing transfer source.
///
/// The first record is bounded on neither its begin side — it was in effect
/// before any period a caller can ask about — and so replaces the one segment a
/// fresh account's timeline carries; the last is bounded on neither its end
/// side, being the segment still in effect. The domains are the published
/// `BillingDomain` values (`smithy_query(query="shape",shape="BillingDomain")`
/// = BILLABLE, PRO_FORMA).
fn seeds() -> Vec<Seed> {
    vec![
        Seed {
            label: "before the billing group",
            domain: BILLING_DOMAIN_BILLABLE,
            begin: None,
            end: Some(at(2024, 3, 10, 0)),
            management_account_id: None,
            billing_group_primary_account_id: None,
            billing_transfer_account_id: None,
            hidden: false,
        },
        Seed {
            label: "in the billing group",
            domain: BILLING_DOMAIN_PRO_FORMA,
            begin: Some(at(2024, 3, 10, 0)),
            end: Some(at(2024, 3, 20, 0)),
            management_account_id: None,
            billing_group_primary_account_id: Some(OTHER_ACCOUNT_ID),
            billing_transfer_account_id: None,
            hidden: false,
        },
        Seed {
            label: "hidden",
            domain: BILLING_DOMAIN_PRO_FORMA,
            begin: Some(at(2024, 3, 20, 0)),
            end: Some(at(2024, 3, 25, 0)),
            management_account_id: None,
            billing_group_primary_account_id: Some(OTHER_ACCOUNT_ID),
            billing_transfer_account_id: None,
            hidden: true,
        },
        Seed {
            label: "in effect",
            domain: BILLING_DOMAIN_BILLABLE,
            begin: Some(at(2024, 3, 25, 0)),
            end: None,
            management_account_id: Some(OTHER_ACCOUNT_ID),
            billing_group_primary_account_id: None,
            billing_transfer_account_id: Some(TRANSFER_ACCOUNT_ID),
            hidden: false,
        },
    ]
}

/// One segment as a response reports it.
#[derive(Debug, Clone, PartialEq)]
struct Reported {
    domain: Option<String>,
    begin: Option<f64>,
    end: Option<f64>,
    management_account_id: Option<String>,
    billing_group_primary_account_id: Option<String>,
    billing_transfer_account_id: Option<String>,
}

/// The segments a response reports, in the order it reports them.
fn reported(body: &Value) -> Vec<Reported> {
    body["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| Reported {
            domain: item["domain"].as_str().map(str::to_string),
            begin: item["timeRange"]["beginDateInclusive"].as_f64(),
            end: item["timeRange"]["endDateExclusive"].as_f64(),
            management_account_id: item["managementAccountId"].as_str().map(str::to_string),
            billing_group_primary_account_id: item["billingGroupPrimaryAccountId"]
                .as_str()
                .map(str::to_string),
            billing_transfer_account_id: item["billingTransferAccountId"]
                .as_str()
                .map(str::to_string),
        })
        .collect()
}

/// The seeded segment `label`, as a response must report it for a request whose
/// clipped period is `[begin, end)`.
fn expected(label: &str, begin: i64, end: i64) -> Reported {
    let seed = seeds()
        .into_iter()
        .find(|seed| seed.label == label)
        .expect("every expectation names a seeded segment");
    Reported {
        domain: Some(seed.domain.to_string()),
        begin: Some(begin as f64),
        end: Some(end as f64),
        management_account_id: seed.management_account_id.map(str::to_string),
        billing_group_primary_account_id: seed
            .billing_group_primary_account_id
            .map(str::to_string),
        billing_transfer_account_id: seed.billing_transfer_account_id.map(str::to_string),
    }
}

/// Persist the seeded timeline for `arn` in the shared engine's store.
fn persist(engine: &Arc<engine::InMemoryEngine>, arn: &str) {
    let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
    let mut state = state.lock();
    for seed in seeds() {
        assert!(
            state.record_billing_view_segment(seed.segment(arn)),
            "case {:?}: the seeded segment must fit in the store",
            seed.label
        );
    }
}

/// The whole stored timeline, key by key, so a read can be shown not to write.
fn timeline(engine: &Arc<engine::InMemoryEngine>) -> Vec<String> {
    let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
    let state = state.lock();
    state
        .billing_view_segments
        .iter()
        .map(|(key, segment)| format!("{key}={segment:?}"))
        .collect()
}

/// One call, asserted to have answered 200, with its reported segments and its
/// `nextToken`.
async fn list(
    router: &MultiServiceRouter,
    case: &str,
    request: &Value,
) -> (Vec<Reported>, Option<String>) {
    let (status, body) = call(router, "ListBillingViewSegments", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} {body:?}"
    );
    (
        reported(&body),
        body["nextToken"].as_str().map(str::to_string),
    )
}

/// A request for the PRIMARY view over the half-open period `[begin, end)`.
fn over(begin: i64, end: i64) -> Value {
    json!({
        "arn": PRIMARY_ARN,
        "timeRange": { "beginDateInclusive": begin, "endDateExclusive": end },
    })
}

/// One row of the period table: the case, the requested period, and the
/// segments that period must select, in order.
type PeriodRow = (&'static str, i64, i64, Vec<Reported>);

fn period_rows() -> Vec<PeriodRow> {
    vec![
        (
            "the whole month the relationships changed in",
            at(2024, 3, 1, 0),
            at(2024, 4, 1, 0),
            vec![
                expected(
                    "before the billing group",
                    at(2024, 3, 1, 0),
                    at(2024, 3, 10, 0),
                ),
                expected("in the billing group", at(2024, 3, 10, 0), at(2024, 3, 20, 0)),
                expected("in effect", at(2024, 3, 25, 0), at(2024, 4, 1, 0)),
            ],
        ),
        (
            "a period inside one segment",
            at(2024, 3, 2, 0),
            at(2024, 3, 5, 0),
            vec![expected(
                "before the billing group",
                at(2024, 3, 2, 0),
                at(2024, 3, 5, 0),
            )],
        ),
        (
            "a period straddling one change",
            at(2024, 3, 9, 12),
            at(2024, 3, 11, 6),
            vec![
                expected(
                    "before the billing group",
                    at(2024, 3, 9, 12),
                    at(2024, 3, 10, 0),
                ),
                expected("in the billing group", at(2024, 3, 10, 0), at(2024, 3, 11, 6)),
            ],
        ),
        (
            "a period only a hidden segment covers",
            at(2024, 3, 20, 0),
            at(2024, 3, 25, 0),
            vec![],
        ),
        (
            "a period beginning inside a hidden segment",
            at(2024, 3, 22, 0),
            at(2024, 3, 28, 0),
            vec![expected("in effect", at(2024, 3, 25, 0), at(2024, 3, 28, 0))],
        ),
        (
            "a period before the first recorded change",
            at(2023, 1, 1, 0),
            at(2023, 2, 1, 0),
            vec![expected(
                "before the billing group",
                at(2023, 1, 1, 0),
                at(2023, 2, 1, 0),
            )],
        ),
        (
            "a period after every closed segment",
            at(2025, 6, 1, 0),
            at(2025, 7, 1, 0),
            vec![expected("in effect", at(2025, 6, 1, 0), at(2025, 7, 1, 0))],
        ),
        (
            "one second at a segment's own first instant",
            at(2024, 3, 10, 0),
            at(2024, 3, 10, 0) + 1,
            vec![expected(
                "in the billing group",
                at(2024, 3, 10, 0),
                at(2024, 3, 10, 0) + 1,
            )],
        ),
        (
            "one second at a segment's own last instant",
            at(2024, 3, 10, 0) - 1,
            at(2024, 3, 10, 0),
            vec![expected(
                "before the billing group",
                at(2024, 3, 10, 0) - 1,
                at(2024, 3, 10, 0),
            )],
        ),
        (
            "a period spanning the whole timeline",
            at(2020, 1, 1, 0),
            at(2026, 1, 1, 0),
            vec![
                expected(
                    "before the billing group",
                    at(2020, 1, 1, 0),
                    at(2024, 3, 10, 0),
                ),
                expected("in the billing group", at(2024, 3, 10, 0), at(2024, 3, 20, 0)),
                expected("in effect", at(2024, 3, 25, 0), at(2026, 1, 1, 0)),
            ],
        ),
    ]
}

#[tokio::test]
async fn the_requested_period_selects_the_segments_that_overlap_it() {
    let (router, engine) = emulator_with_engine();
    persist(&engine, PRIMARY_ARN);
    let before = timeline(&engine);
    assert_eq!(
        before.len(),
        seeds().len(),
        "case \"seeding\": the seeded timeline replaces the fresh account's one \
         open segment, got {before:?}"
    );

    for (case, begin, end, want) in period_rows() {
        let (got, next) = list(&router, case, &over(begin, end)).await;
        assert_eq!(
            got, want,
            "case {case:?}: expected {want:?}, got {got:?}"
        );
        assert_eq!(
            next, None,
            "case {case:?}: a page that holds every selected segment carries no \
             nextToken, got {next:?}"
        );
    }

    assert_eq!(
        timeline(&engine),
        before,
        "case \"a read writes nothing\": the stored timeline must be untouched"
    );
}

#[tokio::test]
async fn a_hidden_segment_leaves_a_gap_the_answer_does_not_cover() {
    // "The response omits hidden segments, so the segments it returns might not
    // cover the entire requested time period": the month's answer jumps the
    // hidden record's five days instead of stretching a visible segment over
    // them.
    let (router, engine) = emulator_with_engine();
    persist(&engine, PRIMARY_ARN);

    let case = "the hidden five days";
    let (got, _) = list(&router, case, &over(at(2024, 3, 1, 0), at(2024, 4, 1, 0))).await;
    let covered: Vec<(Option<f64>, Option<f64>)> =
        got.iter().map(|item| (item.begin, item.end)).collect();
    assert!(
        !covered
            .iter()
            .any(|(begin, end)| *begin <= Some(at(2024, 3, 21, 0) as f64)
                && *end > Some(at(2024, 3, 21, 0) as f64)),
        "case {case:?}: no reported segment covers an instant inside the hidden \
         record, got {covered:?}"
    );
    assert_eq!(
        got.len(),
        3,
        "case {case:?}: the three visible segments of the month are reported, got {got:?}"
    );
}

#[tokio::test]
async fn a_page_walks_the_timeline_oldest_first() {
    let (router, engine) = emulator_with_engine();
    persist(&engine, PRIMARY_ARN);

    let case_request = over(at(2020, 1, 1, 0), at(2026, 1, 1, 0));
    let (whole, _) = list(&router, "unpaged", &case_request).await;

    // The page sizes span the class: one segment at a time, one short of the
    // result set, exactly the result set, and the modeled `maxResults` maximum.
    for page_size in [1usize, 2, 3, 100] {
        let case = format!("maxResults {page_size}");
        let mut walked: Vec<Reported> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = case_request.clone();
            request["maxResults"] = json!(page_size);
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let (page, next) = list(&router, &case, &request).await;
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most maxResults segments, got {}",
                page.len()
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case {case:?}: the walk must terminate, got {pages} pages"
            );
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            walked, whole,
            "case {case:?}: the pages concatenate to the unpaged answer {whole:?}, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn the_default_period_is_the_current_calendar_month() {
    let (router, engine) = emulator_with_engine();
    persist(&engine, PRIMARY_ARN);

    // Only the segment in effect overlaps any month after the seeded changes,
    // and its reported period is the whole current calendar month. The month is
    // read from the clock on both sides of the call, so a call that crosses a
    // month boundary is satisfied by the month it ran in rather than flaking.
    let opened = now_secs();
    let (got, next) = list(
        &router,
        "no timeRange at all",
        &json!({ "arn": PRIMARY_ARN }),
    )
    .await;
    let closed = now_secs();

    let candidates: Vec<Vec<Reported>> = [opened, closed]
        .into_iter()
        .map(|secs| {
            let (begin, end) = calendar_month(secs);
            vec![expected("in effect", begin, end)]
        })
        .collect();
    assert!(
        candidates.contains(&got),
        "case \"no timeRange at all\": expected one of {candidates:?}, got {got:?}"
    );
    assert_eq!(
        next, None,
        "case \"no timeRange at all\": one segment is one page, got {next:?}"
    );
}

#[tokio::test]
async fn a_bound_the_request_omits_comes_from_the_current_month() {
    let (router, engine) = emulator_with_engine();
    persist(&engine, PRIMARY_ARN);

    // Only `beginDateInclusive`: the period runs from it to the end of the
    // current month, so every visible segment from that instant on is reported
    // and the last one is clipped at the month's end.
    let opened = now_secs();
    let (got, _) = list(
        &router,
        "beginDateInclusive only",
        &json!({
            "arn": PRIMARY_ARN,
            "timeRange": { "beginDateInclusive": at(2024, 3, 1, 0) },
        }),
    )
    .await;
    let closed = now_secs();
    let candidates: Vec<Vec<Reported>> = [opened, closed]
        .into_iter()
        .map(|secs| {
            let (_, month_end) = calendar_month(secs);
            vec![
                expected(
                    "before the billing group",
                    at(2024, 3, 1, 0),
                    at(2024, 3, 10, 0),
                ),
                expected("in the billing group", at(2024, 3, 10, 0), at(2024, 3, 20, 0)),
                expected("in effect", at(2024, 3, 25, 0), month_end),
            ]
        })
        .collect();
    assert!(
        candidates.contains(&got),
        "case \"beginDateInclusive only\": expected one of {candidates:?}, got {got:?}"
    );

    // Only `endDateExclusive`: the period begins at the start of the current
    // month, so a one-second end reports the segment in effect over that second.
    let opened = now_secs();
    let (month_begin, _) = calendar_month(opened);
    let (got, _) = list(
        &router,
        "endDateExclusive only",
        &json!({
            "arn": PRIMARY_ARN,
            "timeRange": { "endDateExclusive": month_begin + 1 },
        }),
    )
    .await;
    let closed = now_secs();
    let candidates: Vec<Vec<Reported>> = [opened, closed]
        .into_iter()
        .map(|secs| {
            let (begin, _) = calendar_month(secs);
            if begin == month_begin {
                vec![expected("in effect", begin, month_begin + 1)]
            } else {
                // The call crossed a month boundary, so the requested end falls
                // before the month the answer is built from: an empty period
                // selects nothing.
                Vec::new()
            }
        })
        .collect();
    assert!(
        candidates.contains(&got),
        "case \"endDateExclusive only\": expected one of {candidates:?}, got {got:?}"
    );
}

#[tokio::test]
async fn a_request_with_no_arn_is_answered_for_the_callers_primary_view() {
    let (router, engine) = emulator_with_engine();
    persist(&engine, PRIMARY_ARN);

    for (case, begin, end, want) in period_rows() {
        let request = json!({
            "timeRange": { "beginDateInclusive": begin, "endDateExclusive": end },
        });
        let (got, _) = list(&router, case, &request).await;
        assert_eq!(
            got, want,
            "case {case:?} without an arn: expected the PRIMARY view's own answer \
             {want:?}, got {got:?}"
        );
    }
}

#[tokio::test]
async fn a_fresh_account_reports_one_billable_segment_for_the_current_month() {
    // Nothing is seeded here: the account's default PRIMARY view carries the one
    // segment a standalone account has — its own final charges, no organization,
    // no billing group, no billing transfer — covering whatever period is asked
    // about.
    let (router, _engine) = standalone_emulator_with_engine();

    let case = "a fresh account";
    let (got, _) = list(&router, case, &over(at(2024, 3, 1, 0), at(2024, 4, 1, 0))).await;
    assert_eq!(
        got,
        vec![Reported {
            domain: Some(BILLING_DOMAIN_BILLABLE.to_string()),
            begin: Some(at(2024, 3, 1, 0) as f64),
            end: Some(at(2024, 4, 1, 0) as f64),
            management_account_id: None,
            billing_group_primary_account_id: None,
            billing_transfer_account_id: None,
        }],
        "case {case:?}: expected one BILLABLE segment over the requested period, got {got:?}"
    );
}

#[tokio::test]
async fn a_primary_view_that_is_not_healthy_refuses_the_read() {
    let (router, engine) = emulator_with_engine();
    persist(&engine, PRIMARY_ARN);

    // Every `BillingViewStatus` the model declares except HEALTHY — the states
    // the modeled error calls "a state other than HEALTHY".
    for status_code in ["CREATING", "UNHEALTHY", "UPDATING"] {
        set_health_status(&engine, PRIMARY_ARN, status_code);
        let (status, body) = call(
            &router,
            "ListBillingViewSegments",
            &over(at(2024, 3, 1, 0), at(2024, 4, 1, 0)),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "BillingViewHealthStatusException"),
            "case {status_code:?}: expected 400 BillingViewHealthStatusException, \
             got {status} {body:?}"
        );
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.contains(status_code)
                    && message.contains("HEALTHY")),
            "case {status_code:?}: the refusal names the status that blocked it, got {:?}",
            body["message"]
        );
    }

    // The control: the very same request once the view is HEALTHY again.
    set_health_status(&engine, PRIMARY_ARN, "HEALTHY");
    let (got, _) = list(
        &router,
        "HEALTHY again",
        &over(at(2024, 3, 1, 0), at(2024, 4, 1, 0)),
    )
    .await;
    assert_eq!(
        got.len(),
        3,
        "case \"HEALTHY again\": expected the month's three visible segments, got {got:?}"
    );
}

/// One row of the refusal table: the case, the request, the status and error
/// code it must answer, and the member its `fieldList` must name (`None` for a
/// refusal the boundary validator words, which names the member in its own
/// envelope).
type RefusalRow = (
    &'static str,
    Value,
    u16,
    &'static str,
    Option<&'static str>,
);

#[tokio::test]
async fn an_invalid_request_is_refused_as_the_model_declares() {
    let (router, engine) = emulator_with_engine();
    persist(&engine, PRIMARY_ARN);
    // A CUSTOM view, the kind the modeled `arn` says "aren't supported".
    let custom = create_view(
        &router,
        "a custom view",
        "segments-custom",
        &[source_arn("segments-source")],
    )
    .await;
    let before = timeline(&engine);

    let rows: Vec<RefusalRow> = vec![
        (
            "an arn in another account",
            json!({ "arn": "arn:aws:billing::123456789012:billingview/primary" }),
            403,
            "AccessDeniedException",
            None,
        ),
        (
            "an arn no view of this account is stored under",
            json!({ "arn": ABSENT_ARN }),
            404,
            "ResourceNotFoundException",
            None,
        ),
        (
            "an arn naming a CUSTOM view",
            json!({ "arn": custom }),
            400,
            "ValidationException",
            Some("arn"),
        ),
        (
            "an arn outside the modeled pattern",
            json!({ "arn": "arn:aws:billing::000000000000:notaview/primary" }),
            400,
            "ValidationException",
            None,
        ),
        (
            "endDateExclusive equal to beginDateInclusive",
            over(at(2024, 3, 1, 0), at(2024, 3, 1, 0)),
            400,
            "ValidationException",
            Some("timeRange"),
        ),
        (
            "endDateExclusive one second before beginDateInclusive",
            over(at(2024, 3, 1, 0), at(2024, 3, 1, 0) - 1),
            400,
            "ValidationException",
            Some("timeRange"),
        ),
        (
            "endDateExclusive a month before beginDateInclusive",
            over(at(2024, 3, 1, 0), at(2024, 2, 1, 0)),
            400,
            "ValidationException",
            Some("timeRange"),
        ),
        (
            "a nextToken this service never issued",
            json!({ "arn": PRIMARY_ARN, "nextToken": "zzzz" }),
            400,
            "ValidationException",
            Some("nextToken"),
        ),
        (
            // The modeled `maxResults` range is 1..100.
            "maxResults below the modeled minimum",
            json!({ "arn": PRIMARY_ARN, "maxResults": 0 }),
            400,
            "ValidationException",
            None,
        ),
        (
            "maxResults above the modeled maximum",
            json!({ "arn": PRIMARY_ARN, "maxResults": 101 }),
            400,
            "ValidationException",
            None,
        ),
    ];

    for (case, request, want_status, want_code, want_field) in rows {
        let (status, body) = call(&router, "ListBillingViewSegments", &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (want_status, want_code),
            "case {case:?}: expected {want_status} {want_code}, got {status} {body:?}"
        );
        assert!(
            body["message"].as_str().is_some_and(|m| !m.is_empty()),
            "case {case:?}: the refusal carries a message, got {body:?}"
        );
        if let Some(field) = want_field {
            let named: Vec<String> = body["fieldList"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|entry| entry["name"].as_str().unwrap_or_default().to_string())
                .collect();
            assert!(
                named.iter().any(|name| name == field),
                "case {case:?}: expected fieldList to name {field:?}, got {named:?}"
            );
        }
    }

    // The 404 names the resource it could not find, in the members the model
    // makes required.
    let (_, body) = call(
        &router,
        "ListBillingViewSegments",
        &json!({ "arn": ABSENT_ARN }),
    )
    .await;
    assert_eq!(
        (
            body["resourceId"].as_str(),
            body["resourceType"].as_str()
        ),
        (Some(ABSENT_ARN), Some("billingview")),
        "case \"an arn no view of this account is stored under\": expected the ARN and \
         the billing-view resource type, got {body:?}"
    );

    assert_eq!(
        timeline(&engine),
        before,
        "case \"a refused read writes nothing\": the stored timeline must be untouched"
    );
}
