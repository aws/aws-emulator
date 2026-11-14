// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the USAGE DATA a usage plan reports: `GetUsage`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service, and the `execute-api` data plane on the
//! api's own invoke host — so what is asserted is what a client sees, not what a handler
//! returns. The emulator, the request builder and the
//! `CreateRestApi`/`CreateApiKey`/`CreateUsagePlan`/`CreateUsagePlanKey` helpers are this
//! crate's existing harness, reused from `rest_api_lifecycle`, `api_key_lifecycle`,
//! `usage_plan_lifecycle` and `usage_plan_key_lifecycle`.
//!
//! The RULES under test, asserted over SETS of inputs rather than one recorded call:
//!
//! * the report is "the usage data, as daily logs of used and remaining quotas, over the
//!   specified time interval indexed over the API keys in a usage plan", each daily entry
//!   "of the format [used quota, remaining quota]" (`Usage$items`): the keys INDEXED are
//!   the ones with usage data over the interval — a key that sent nothing over those days
//!   is in no report — and an indexed key carries one entry per day of the inclusive
//!   interval, over a table spanning the quota period, the limit, the offset, a plan with
//!   no quota at all, how many keys the plan meters and how wide the interval is;
//! * `used` is what the data plane counted for that key on that day and `remaining` is
//!   what the plan's quota has left after it, so invoking a deployed stage with the key is
//!   what moves the numbers, a request past the period's limit is refused `429` and counts
//!   for nothing, and reading the report consumes no part of the quota;
//! * the quota RESETS at the start of each metering period, and the quota's `offset` —
//!   "the number of requests subtracted from the given limit in the initial time period"
//!   (`QuotaSettings$offset`) — is subtracted only in the period the plan was created in;
//! * the report is paged over the keys it is indexed by, through the `position`/`limit`
//!   envelope every paginated collection of this service answers in;
//! * a `usagePlanId` naming no stored plan is refused 404, a `keyId` naming no key the plan
//!   meters is refused 404, and an interval that names no days — a date in no other
//!   spelling, an end before its start, one wider than a report may cover — is refused 400.

use std::time::SystemTime;

use ::chrono::{DateTime, Days, NaiveDate, Utc};
use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    INVALID_POSITION, INVALID_USAGE_END_DATE, INVALID_USAGE_INTERVAL, INVALID_USAGE_START_DATE,
    MAX_USAGE_INTERVAL_DAYS, NO_SUCH_API_KEY, NO_SUCH_USAGE_PLAN,
};

use crate::api_key_lifecycle::create_key;
use crate::rest_api_lifecycle::{
    assert_failure, create_api, create_resource, string_member, Answer, Gateway,
};
use crate::usage_plan_key_lifecycle::create_usage_plan_key;
use crate::usage_plan_lifecycle::create_usage_plan;

/// The one key type a `UsagePlanKey` takes: "Currently, the valid key type is `API_KEY`"
/// (`smithy_query(query="doc", shape="UsagePlanKey", member="type")`).
const API_KEY_TYPE: &str = "API_KEY";

/// The stage every deployed api of this suite is invoked through.
///
/// The items below are `pub(crate)` so the sibling suite over the GRANT of a remaining quota
/// (`usage_plan_quota_extension`) drives the same metered api, the same keys and the same
/// report reader rather than standing a second copy of this harness up.
pub(crate) const STAGE: &str = "prod";

/// The region the invoke host is addressed in; any region is served, so one is enough.
const REGION: &str = "us-east-1";

/// The body the `MOCK` integration behind every keyed method answers.
const SERVED: &str = r#"{"served":true}"#;

/// The `message` API Gateway answers a request past a usage plan's quota with, through the
/// `QUOTA_EXCEEDED` gateway response the catalog gives HTTP 429.
pub(crate) const LIMIT_EXCEEDED: &str = "Limit Exceeded";

/// The UTC day the emulator is counting usage on, which every interval below is stated
/// relative to.
pub(crate) fn today() -> NaiveDate {
    DateTime::<Utc>::from(SystemTime::now()).date_naive()
}

/// `day` as this surface spells a date: the format `GetUsage`'s own members document, "the
/// starting date (e.g., 2016-01-01) of the usage data"
/// (`smithy_query(query="doc", shape="GetUsageRequest", member="startDate")`).
pub(crate) fn date(day: NaiveDate) -> String {
    day.format("%Y-%m-%d").to_string()
}

/// The day `days` after `from`.
pub(crate) fn plus(from: NaiveDate, days: u64) -> NaiveDate {
    from.checked_add_days(Days::new(days))
        .expect("a day inside the calendar")
}

/// `GetUsage` for `plan` over the interval `from` through `through`, with `query` appended
/// verbatim (`&keyId=…`, `&limit=…`, `&position=…`).
pub(crate) async fn get_usage(
    gateway: &Gateway,
    plan: &str,
    from: &str,
    through: &str,
    query: &str,
) -> Answer {
    gateway
        .call(
            "GET",
            &format!("/usageplans/{plan}/usage?startDate={from}&endDate={through}{query}"),
            None,
        )
        .await
}

/// The report's daily logs, as the `values` member carries them: the key id each log is
/// indexed by, with that key's `[used, remaining]` entries.
pub(crate) fn logs(body: &Value, case: &str) -> Vec<(String, Vec<Vec<i64>>)> {
    let values = body["values"]
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: expected a values map, got {body:?}"));
    values
        .iter()
        .map(|(key_id, entries)| {
            let entries = entries
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: expected daily logs, got {entries:?}"));
            let entries = entries
                .iter()
                .map(|entry| {
                    let pair = entry.as_array().unwrap_or_else(|| {
                        panic!("case {case}: expected a [used, remaining] pair, got {entry:?}")
                    });
                    pair.iter()
                        .map(|number| {
                            number.as_i64().unwrap_or_else(|| {
                                panic!("case {case}: expected a count, got {number:?}")
                            })
                        })
                        .collect()
                })
                .collect();
            (key_id.clone(), entries)
        })
        .collect()
}

/// The key ids the report is indexed by, sorted so a set can be compared.
pub(crate) fn indexed_keys(body: &Value, case: &str) -> Vec<String> {
    let mut keys: Vec<String> = logs(body, case).into_iter().map(|(key, _)| key).collect();
    keys.sort();
    keys
}

/// The daily log of ONE key the report covers.
pub(crate) fn log_of(body: &Value, key_id: &str, case: &str) -> Vec<Vec<i64>> {
    logs(body, case)
        .into_iter()
        .find(|(reported, _)| reported == key_id)
        .unwrap_or_else(|| panic!("case {case}: expected a log for {key_id}, got {body:?}"))
        .1
}

/// `CreateApiKey` for a distinct 40-character value, answering `(id, value)`: a key's value
/// is unique per account, so no two keys of one suite may carry the same one.
pub(crate) async fn api_key(gateway: &Gateway, case: &str, n: usize) -> (String, String) {
    let value = format!("{}{n:04}", "u".repeat(36));
    let created = create_key(
        gateway,
        case,
        json!({ "name": format!("key-{case}-{n}"), "value": value, "enabled": true }),
    )
    .await;
    (string_member(&created, "id", case), value)
}

/// A usage plan over the api stage `api`/[`STAGE`] carrying `quota`, metering every key in
/// `key_ids`.
pub(crate) async fn metering_plan(
    gateway: &Gateway,
    case: &str,
    quota: Option<Value>,
    api: &str,
    key_ids: &[String],
) -> String {
    let mut request = json!({
        "name": format!("plan-{case}"),
        "apiStages": [{ "apiId": api, "stage": STAGE }],
    });
    if let Some(quota) = quota {
        request["quota"] = quota;
    }
    let plan = create_usage_plan(gateway, case, request).await;
    let plan = string_member(&plan, "id", case);
    for key_id in key_ids {
        let answer = create_usage_plan_key(gateway, &plan, key_id, API_KEY_TYPE).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected CreateUsagePlanKey to answer 201, got {} with {:?}",
            answer.status, answer.body
        );
    }
    plan
}

/// One answered invoke of a deployed stage: its status and its body verbatim.
pub(crate) struct Invoked {
    pub(crate) status: u16,
    pub(crate) body: String,
}

impl Invoked {
    /// The `message` member of a JSON body — what every gateway response carries — or the
    /// body itself when it carries none.
    pub(crate) fn message(&self) -> String {
        serde_json::from_str::<Value>(&self.body)
            .ok()
            .and_then(|body| body.get("message")?.as_str().map(str::to_owned))
            .unwrap_or_else(|| self.body.clone())
    }
}

/// Issue one unsigned GET to `api`'s own invoke endpoint, presenting `key_value`.
pub(crate) async fn invoke(gateway: &Gateway, api: &str, key_value: &str) -> Invoked {
    let host = format!("{api}.execute-api.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("https://{host}/{STAGE}/metered"))
        .header("host", &host)
        .header("x-api-key", key_value)
        .body(bytes::Bytes::new())
        .expect("the invoke request is well formed");
    let response = gateway.send(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(&response.into_body()).into_owned();
    Invoked { status, body }
}

/// One served invoke, asserting the stage answered the integration's own body: the request
/// the records admitted is the one the usage data counts.
pub(crate) async fn send_one(gateway: &Gateway, api: &str, key_value: &str, case: &str) {
    let served = invoke(gateway, api, key_value).await;
    assert_eq!(
        served.status, 200,
        "case {case}: expected the invoke to be served, got {} with {:?}",
        served.status, served.body
    );
    assert_eq!(
        served.body, SERVED,
        "case {case}: expected the integration's own body, got {:?}",
        served.body
    );
}

/// An api with ONE key-required method behind a `MOCK` integration, deployed to [`STAGE`];
/// answers the api id.
pub(crate) async fn metered_api(gateway: &Gateway, name: &str) -> String {
    let (api, root) = create_api(gateway, name).await;
    let resource = create_resource(gateway, &api, &root, "metered").await;
    let resource = string_member(&resource, "id", name);
    for (what, uri, request) in [
        (
            "PutMethod",
            format!("/restapis/{api}/resources/{resource}/methods/GET"),
            json!({ "authorizationType": "NONE", "apiKeyRequired": true }),
        ),
        (
            "PutIntegration",
            format!("/restapis/{api}/resources/{resource}/methods/GET/integration"),
            json!({ "type": "MOCK" }),
        ),
        (
            "PutMethodResponse",
            format!("/restapis/{api}/resources/{resource}/methods/GET/responses/200"),
            json!({}),
        ),
        (
            "PutIntegrationResponse",
            format!("/restapis/{api}/resources/{resource}/methods/GET/integration/responses/200"),
            json!({ "responseTemplates": { "application/json": SERVED } }),
        ),
    ] {
        let answer = gateway.call("PUT", &uri, Some(request)).await;
        assert_eq!(
            answer.status, 201,
            "case {what}({name}): expected 201, got {} with {:?}",
            answer.status, answer.body
        );
    }
    let deployed = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/deployments"),
            Some(json!({ "stageName": STAGE })),
        )
        .await;
    assert_eq!(
        deployed.status, 201,
        "case CreateDeployment({name}): expected 201, got {} with {:?}",
        deployed.status, deployed.body
    );
    api
}

/// One row of the report table: what the plan's quota is, how many keys it meters, and what
/// the quota makes of one request from each of them.
struct Row {
    case: &'static str,
    /// The plan's `quota` member, `None` for a plan created without one.
    quota: Option<Value>,
    /// How many API keys the plan meters.
    keys: usize,
    /// Whether the quota admits a request at all: a period whose budget is zero admits
    /// none, so nothing is counted and nothing is indexed.
    admits: bool,
    /// The daily entry a key earns once one request of it has been counted: what it sent
    /// that day, and what the period has left after it.
    after: [i64; 2],
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a daily quota",
            quota: Some(json!({ "limit": 100, "offset": 0, "period": "DAY" })),
            keys: 1,
            admits: true,
            after: [1, 99],
        },
        Row {
            case: "a weekly quota with an offset",
            quota: Some(json!({ "limit": 500, "offset": 6, "period": "WEEK" })),
            keys: 1,
            admits: true,
            after: [1, 493],
        },
        Row {
            case: "a monthly quota with an offset, two keys",
            quota: Some(json!({ "limit": 1_000, "offset": 27, "period": "MONTH" })),
            keys: 2,
            admits: true,
            after: [1, 972],
        },
        Row {
            case: "a monthly quota of one request",
            quota: Some(json!({ "limit": 1, "offset": 0, "period": "MONTH" })),
            keys: 1,
            admits: true,
            after: [1, 0],
        },
        Row {
            case: "a quota of no requests at all",
            quota: Some(json!({ "limit": 0, "offset": 0, "period": "DAY" })),
            keys: 1,
            admits: false,
            after: [0, 0],
        },
        Row {
            case: "a quota whose offset is its whole limit",
            quota: Some(json!({ "limit": 6, "offset": 6, "period": "WEEK" })),
            keys: 1,
            admits: false,
            after: [0, 0],
        },
        Row {
            case: "a plan with no quota at all, three keys",
            quota: None,
            keys: 3,
            admits: true,
            after: [1, 0],
        },
    ]
}

#[tokio::test]
async fn the_report_indexes_the_keys_with_usage_data_and_logs_one_entry_per_day() {
    let gateway = Gateway::new();
    let api = metered_api(&gateway, "usage-report-table").await;
    let from = today();

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let mut keys = Vec::new();
        for n in 0..row.keys {
            keys.push(api_key(&gateway, case, index * 10 + n).await);
        }
        let key_ids: Vec<String> = keys.iter().map(|(id, _)| id.clone()).collect();
        let plan = metering_plan(&gateway, case, row.quota, &api, &key_ids).await;

        // Nothing has invoked the stage with these keys, so the plan has no usage data and
        // the report indexes nothing — not a log of zeroes for each key it meters.
        let empty = get_usage(&gateway, &plan, &date(from), &date(from), "").await;
        assert_eq!(
            empty.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            empty.status, empty.body
        );
        assert_eq!(
            empty.body["usagePlanId"].as_str(),
            Some(plan.as_str()),
            "case {case}: expected the plan the report is of, got {:?}",
            empty.body["usagePlanId"]
        );
        assert_eq!(
            empty.body["startDate"].as_str(),
            Some(date(from).as_str()),
            "case {case}: expected the interval's own start, got {:?}",
            empty.body["startDate"]
        );
        assert_eq!(
            empty.body["endDate"].as_str(),
            Some(date(from).as_str()),
            "case {case}: expected the interval's own end, got {:?}",
            empty.body["endDate"]
        );
        assert_eq!(
            indexed_keys(&empty.body, case),
            Vec::<String>::new(),
            "case {case}: expected no key indexed before any request, got {:?}",
            empty.body["values"]
        );

        // One request from each key, which the quota either admits or refuses.
        for (key_id, key_value) in &keys {
            let sent = invoke(&gateway, &api, key_value).await;
            if row.admits {
                assert_eq!(
                    sent.status, 200,
                    "case {case}: expected the quota to admit one request from {key_id}, got \
                     {} with {:?}",
                    sent.status, sent.body
                );
            } else {
                assert_eq!(
                    sent.status, 429,
                    "case {case}: expected the quota to refuse every request from {key_id}, \
                     got {} with {:?}",
                    sent.status, sent.body
                );
            }
        }

        // What the report says about those days: every key the quota counted a request for
        // is indexed, with one `[used, remaining]` entry per day of the interval, and a key
        // whose request was refused is still indexed by nothing.
        let days = 3_u64;
        let through = plus(from, days - 1);
        let answer = get_usage(&gateway, &plan, &date(from), &date(through), "").await;
        let mut expected = if row.admits {
            key_ids.clone()
        } else {
            Vec::new()
        };
        expected.sort();
        assert_eq!(
            indexed_keys(&answer.body, case),
            expected,
            "case {case}: expected the keys with usage data, got {:?}",
            answer.body["values"]
        );
        for key_id in &expected {
            let entries = log_of(&answer.body, key_id, case);
            let covered = usize::try_from(days).expect("the interval fits a usize");
            assert_eq!(
                entries.len(),
                covered,
                "case {case}: expected {covered} daily entries for {key_id}, got {entries:?}"
            );
            assert_eq!(
                entries.first(),
                Some(&row.after.to_vec()),
                "case {case}: expected the first day of {key_id} to be {:?}, got {:?}",
                row.after,
                entries.first()
            );
        }
    }
}

#[tokio::test]
async fn the_offset_is_subtracted_in_the_initial_period_and_every_later_period_starts_whole() {
    let gateway = Gateway::new();
    let api = metered_api(&gateway, "usage-report-periods").await;
    let from = today();

    // One row per metering period, with the number of days that certainly lands in a LATER
    // period of that length: the next day, the same weekday a week on, and a day more than
    // the longest month away.
    for (index, (period, later, limit, offset)) in [
        ("DAY", 1_u64, 50_i64, 0_i64),
        ("WEEK", 7, 500, 6),
        ("MONTH", 31, 1_000, 27),
    ]
    .into_iter()
    .enumerate()
    {
        let case = format!("a {period} quota with an offset of {offset}");
        let (key_id, key_value) = api_key(&gateway, &case, 100 + index).await;
        let plan = metering_plan(
            &gateway,
            &case,
            Some(json!({ "limit": limit, "offset": offset, "period": period })),
            &api,
            ::std::slice::from_ref(&key_id),
        )
        .await;
        send_one(&gateway, &api, &key_value, &case).await;

        let through = plus(from, later);
        let answer = get_usage(&gateway, &plan, &date(from), &date(through), "").await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let entries = log_of(&answer.body, &key_id, &case);
        assert_eq!(
            entries.first(),
            Some(&vec![1, limit - offset - 1]),
            "case {case}: expected the initial period to be short by the offset and the \
             request, got {:?}",
            entries.first()
        );
        assert_eq!(
            entries.last(),
            Some(&vec![0, limit]),
            "case {case}: expected a later period to start at the whole limit, got {:?}",
            entries.last()
        );
    }
}

#[tokio::test]
async fn the_used_quota_is_what_the_data_plane_counted_and_a_request_past_it_is_refused() {
    let gateway = Gateway::new();
    let from = today();
    let api = metered_api(&gateway, "usage-report-counting").await;
    let case = "a daily quota of four requests";

    let (key_id, key_value) = api_key(&gateway, case, 1).await;
    let plan = metering_plan(
        &gateway,
        case,
        Some(json!({ "limit": 4, "offset": 0, "period": "DAY" })),
        &api,
        ::std::slice::from_ref(&key_id),
    )
    .await;

    // Every request the quota admits is served and counted, and what the report says about
    // today's log is exactly how many were sent and how many the period has left.
    for sent in 1..=4_i64 {
        send_one(&gateway, &api, &key_value, case).await;
        let answer = get_usage(&gateway, &plan, &date(from), &date(from), "").await;
        let entries = log_of(&answer.body, &key_id, case);
        assert_eq!(
            entries,
            vec![vec![sent, 4 - sent]],
            "case {case}: expected [{sent}, {}] after {sent} request(s), got {entries:?}",
            4 - sent
        );
    }

    // One past the period's limit: refused through the `QUOTA_EXCEEDED` gateway response,
    // and counted for nothing — a refused request consumed no part of the quota.
    let refused = invoke(&gateway, &api, &key_value).await;
    assert_eq!(
        refused.status, 429,
        "case {case}: expected 429 past the quota, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.message(),
        LIMIT_EXCEEDED,
        "case {case}: expected {LIMIT_EXCEEDED:?} past the quota, got {:?}",
        refused.message()
    );

    // READING the report changes nothing a later caller sees: the log is the same on the
    // second read, and the day after today is a period of its own, whole again.
    let first = get_usage(&gateway, &plan, &date(from), &date(plus(from, 1)), "").await;
    let second = get_usage(&gateway, &plan, &date(from), &date(plus(from, 1)), "").await;
    assert_eq!(
        first.body, second.body,
        "case {case}: expected a read to change nothing, got {:?} then {:?}",
        first.body, second.body
    );
    assert_eq!(
        log_of(&first.body, &key_id, case),
        vec![vec![4, 0], vec![0, 4]],
        "case {case}: expected today spent and tomorrow whole, got {:?}",
        log_of(&first.body, &key_id, case)
    );

    // And the interval is what decides which keys are indexed: this key's usage data is
    // today's, so an interval of days it sent nothing on indexes it nowhere.
    let elsewhere = get_usage(
        &gateway,
        &plan,
        &date(plus(from, 1)),
        &date(plus(from, 3)),
        "",
    )
    .await;
    assert_eq!(
        indexed_keys(&elsewhere.body, case),
        Vec::<String>::new(),
        "case {case}: expected no key indexed over days it sent nothing on, got {:?}",
        elsewhere.body["values"]
    );
}

#[tokio::test]
async fn the_report_is_paged_over_the_keys_it_is_indexed_by() {
    let gateway = Gateway::new();
    let api = metered_api(&gateway, "usage-report-paging").await;
    let from = date(today());
    let case = "three keys, a page of one";

    let mut keys = Vec::new();
    for n in 0..3 {
        keys.push(api_key(&gateway, case, 200 + n).await);
    }
    let key_ids: Vec<String> = keys.iter().map(|(id, _)| id.clone()).collect();
    let plan = metering_plan(
        &gateway,
        case,
        Some(json!({ "limit": 10, "offset": 0, "period": "DAY" })),
        &api,
        &key_ids,
    )
    .await;
    for (_, key_value) in &keys {
        send_one(&gateway, &api, key_value, case).await;
    }

    // Walked a page at a time, the report covers every indexed key exactly once, in the
    // order the plan's associations were added.
    let mut walked = Vec::new();
    let mut position: Option<String> = None;
    for page in 0..3 {
        let query = position.as_ref().map_or_else(
            || "&limit=1".to_string(),
            |cursor| format!("&limit=1&position={cursor}"),
        );
        let answer = get_usage(&gateway, &plan, &from, &from, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200 for page {page}, got {} with {:?}",
            answer.status, answer.body
        );
        let reported = logs(&answer.body, case);
        assert_eq!(
            reported.len(),
            1,
            "case {case}: expected one key on page {page}, got {reported:?}"
        );
        walked.push(reported[0].0.clone());
        position = answer.body["position"].as_str().map(str::to_owned);
    }
    assert_eq!(
        walked, key_ids,
        "case {case}: expected the keys in the order they were added, got {walked:?}"
    );

    // The page that reported the last indexed key is where the walk ENDS: it hands back no
    // cursor, so a client looping "while a cursor is reported" stops there.
    assert_eq!(
        position, None,
        "case {case}: expected no cursor after the last indexed key, got {position:?}"
    );

    // A cursor the service never issued is refused rather than served an arbitrary page.
    let refused = get_usage(&gateway, &plan, &from, &from, "&position=not-a-key").await;
    assert_failure(&refused, 400, "BadRequestException", case);
    assert_eq!(
        refused.body["message"].as_str(),
        Some(INVALID_POSITION),
        "case {case}: expected {INVALID_POSITION:?}, got {:?}",
        refused.body["message"]
    );
}

#[tokio::test]
async fn an_unresolvable_plan_key_or_interval_is_refused() {
    let gateway = Gateway::new();
    let api = metered_api(&gateway, "usage-report-refusals").await;
    let from = today();
    let case = "the plan every row reads";

    let (key_id, key_value) = api_key(&gateway, case, 300).await;
    let (unmetered, _) = api_key(&gateway, case, 301).await;
    let plan = metering_plan(
        &gateway,
        case,
        Some(json!({ "limit": 10, "offset": 0, "period": "DAY" })),
        &api,
        ::std::slice::from_ref(&key_id),
    )
    .await;
    send_one(&gateway, &api, &key_value, case).await;

    // A plan the store does not hold, and a key the plan does not meter: each names a
    // record the report would be of, so each is refused as the record it names.
    for (case, plan_id, query, message) in [
        (
            "a usagePlanId naming no stored plan",
            "no-such-plan".to_string(),
            String::new(),
            NO_SUCH_USAGE_PLAN,
        ),
        (
            "a keyId naming no stored key at all",
            plan.clone(),
            "&keyId=no-such-key".to_string(),
            NO_SUCH_API_KEY,
        ),
        (
            "a keyId naming a stored key the plan does not meter",
            plan.clone(),
            format!("&keyId={unmetered}"),
            NO_SUCH_API_KEY,
        ),
    ] {
        let refused = get_usage(&gateway, &plan_id, &date(from), &date(from), &query).await;
        assert_failure(&refused, 404, "NotFoundException", case);
        assert_eq!(
            refused.body["message"].as_str(),
            Some(message),
            "case {case}: expected {message:?}, got {:?}",
            refused.body["message"]
        );
    }

    // The interval: both dates name a day, in that order, no wider than a report covers.
    let widest = plus(from, MAX_USAGE_INTERVAL_DAYS - 1);
    let too_wide = plus(from, MAX_USAGE_INTERVAL_DAYS);
    for (case, start, end, message) in [
        (
            "a startDate in no date spelling at all",
            "yesterday".to_string(),
            date(from),
            INVALID_USAGE_START_DATE.to_string(),
        ),
        (
            "a startDate carrying a time as well as a day",
            "2026-01-01T00:00:00Z".to_string(),
            date(from),
            INVALID_USAGE_START_DATE.to_string(),
        ),
        (
            "a startDate naming a day no month has",
            "2026-02-30".to_string(),
            date(from),
            INVALID_USAGE_START_DATE.to_string(),
        ),
        (
            "a startDate in the other field order",
            "01-01-2026".to_string(),
            date(from),
            INVALID_USAGE_START_DATE.to_string(),
        ),
        (
            "an empty startDate",
            String::new(),
            date(from),
            INVALID_USAGE_START_DATE.to_string(),
        ),
        (
            "an endDate in no date spelling at all",
            date(from),
            "tomorrow".to_string(),
            INVALID_USAGE_END_DATE.to_string(),
        ),
        (
            "an endDate naming a month no year has",
            date(from),
            "2026-13-01".to_string(),
            INVALID_USAGE_END_DATE.to_string(),
        ),
        (
            "an endDate one day before the startDate",
            date(from),
            date(from.checked_sub_days(Days::new(1)).expect("a day before")),
            INVALID_USAGE_INTERVAL.to_string(),
        ),
        (
            "an interval one day wider than a report covers",
            date(from),
            date(too_wide),
            format!(
                "Invalid usage interval specified. The usage interval must be at most \
                 {MAX_USAGE_INTERVAL_DAYS} days"
            ),
        ),
    ] {
        let refused = get_usage(&gateway, &plan, &start, &end, "").await;
        assert_failure(&refused, 400, "BadRequestException", case);
        assert_eq!(
            refused.body["message"].as_str(),
            Some(message.as_str()),
            "case {case}: expected {message:?}, got {:?}",
            refused.body["message"]
        );
    }

    // The control: the interval AT the bound is served, so the refusal above is the bound
    // and not the interval.
    let served = get_usage(&gateway, &plan, &date(from), &date(widest), "").await;
    assert_eq!(
        served.status, 200,
        "case the interval at the bound: expected 200, got {} with {:?}",
        served.status, served.body
    );
    let entries = log_of(&served.body, &key_id, "the interval at the bound");
    let covered = usize::try_from(MAX_USAGE_INTERVAL_DAYS).expect("the bound fits a usize");
    assert_eq!(
        entries.len(),
        covered,
        "case the interval at the bound: expected {covered} daily entries, got {}",
        entries.len()
    );

    // And one key of the plan, named by `keyId`, is the report of that key alone.
    let narrowed = get_usage(
        &gateway,
        &plan,
        &date(from),
        &date(from),
        &format!("&keyId={key_id}"),
    )
    .await;
    assert_eq!(
        narrowed.status, 200,
        "case a keyId the plan meters: expected 200, got {} with {:?}",
        narrowed.status, narrowed.body
    );
    assert_eq!(
        indexed_keys(&narrowed.body, "a keyId the plan meters"),
        vec![key_id],
        "case a keyId the plan meters: expected that key alone, got {:?}",
        narrowed.body
    );
}
