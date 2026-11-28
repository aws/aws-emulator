// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListAutomationEvents`.
//!
//! Every case drives the service the way a client does: an awsJson1_0 `POST /`
//! with `X-Amz-Target: ComputeOptimizerAutomationService.ListAutomationEvents`,
//! dispatched through the same [`MultiServiceRouter`] the emulator binary
//! mounts, so a response that is not computed from persisted state fails here.
//!
//! The events themselves are SEEDED into the shared store (the state struct is
//! this crate's own) rather than created through the API: an event created
//! through `StartAutomationEvent` needs a stored recommended action, which only
//! Compute Optimizer's analysis produces. The harness below hands the engine
//! back for that seeding, and is `pub(crate)` so the sibling suites that also
//! have to seed share it.

use std::sync::Arc;

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state::{
    caller_account_id, now,
    ComAmazonawsComputeoptimizerautomationComputeoptimizerautomationserviceState as AutomationState,
    StoredEvent, EVENT_RETRIEVAL_WINDOW, INVALID_TIME_RANGE_MESSAGE,
};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A router with one freshly-engined copy of this service mounted, plus the
/// engine itself so a case can seed the store the handlers read. The account is
/// OPTED IN, because every operation these cases drive refuses one that is not
/// (see `state::…::opt_in_required`); the gate itself is asserted in
/// [`crate::enrollment_gate`].
pub(crate) fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Deterministic regardless of the process-wide default: these cases are
    // about the service's behaviour, not about SigV4.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    crate::automation_rule_lifecycle::enroll_caller(&engine);
    (router, engine)
}

/// Call one operation and return `(http status, parsed body)`.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("https://aco-automation.us-east-1.amazonaws.com/")
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("ComputeOptimizerAutomationService.{operation}"),
        )
        // The router keys on the SigV4 credential scope's service field
        // (`aco-automation`, this service's signing name). The signature itself
        // is not checked, because authentication is off above.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIAEXAMPLEEXAMPLE/20260101/us-east-1/aco-automation/\
             aws4_request, SignedHeaders=host;x-amz-target, Signature=0",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Call `ListAutomationEvents`, requiring success.
async fn list_ok(router: &MultiServiceRouter, body: &Value, case: &str) -> Value {
    let (status, parsed) = call(router, "ListAutomationEvents", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListAutomationEvents to succeed with HTTP 200, got {status} \
         and {parsed}"
    );
    parsed
}

/// The `eventId`s of one `ListAutomationEvents` response, in the order returned.
fn listed_ids(body: &Value) -> Vec<String> {
    body.get("automationEvents")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("automationEvents must be a list, got {body}"))
        .iter()
        .map(|event| string_member(event, "eventId"))
        .collect()
}

fn member<'a>(body: &'a Value, name: &str) -> &'a Value {
    body.get(name).unwrap_or(&Value::Null)
}

fn string_member(body: &Value, name: &str) -> String {
    member(body, name).as_str().unwrap_or_default().to_string()
}

/// The awsJson error code an error body carries in `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Assert a call was refused with `expected_code` and `expected_status`.
fn assert_refused(
    (status, body): (u16, Value),
    expected_status: u16,
    expected_code: &str,
    case: &str,
) -> Value {
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {status} and {body}"
    );
    assert_eq!(
        error_code(&body),
        expected_code,
        "case {case:?}: expected error code {expected_code}, got {body}"
    );
    body
}

/// `NextToken` is modelled as `^[A-Za-z0-9+/=]+$`.
fn is_next_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
}

/// Now, truncated to whole seconds, so a case can name an event's exact
/// `createdTimestamp` on the wire.
///
/// `pub(crate)` with the two seeding helpers below, because the sibling suites
/// that summarise the same events seed them the same way rather than each
/// growing a copy.
pub(crate) fn now_secs() -> i64 {
    DateTime::from(std::time::SystemTime::now()).secs()
}

/// One stored event. `eventId` is 16 characters of `[0-9A-Za-z]` (the model's
/// `EventId` pattern) and `resourceType` has the single value the model's
/// `ResourceType` enum declares (`EbsVolume`).
pub(crate) fn stored_event(
    index: usize,
    account_id: &str,
    event_type: &str,
    event_status: &str,
    created_secs: i64,
) -> StoredEvent {
    StoredEvent {
        event_id: format!("evt{index:013}"),
        event_description: Some(format!("event {index}")),
        event_type: event_type.to_string(),
        event_status: event_status.to_string(),
        event_status_reason: None,
        resource_arn: None,
        resource_id: Some(format!("vol-{index:08}")),
        recommended_action_id: None,
        account_id: account_id.to_string(),
        region: Some("us-east-1".to_string()),
        rule_id: None,
        resource_type: "EbsVolume".to_string(),
        created_timestamp: DateTime::from_secs(created_secs),
        completed_timestamp: None,
        estimated_monthly_savings: None,
        // A seeded event carries whatever steps the case gives it; the default
        // is none, and the cases about steps build their own.
        steps: Vec::new(),
        // A seeded event is already in the status it names, so the clock owes it
        // no transition.
        pending: None,
    }
}

/// Put `events` in the shared store, in the order given.
///
/// An event of an account OTHER than the caller's is in that store because the
/// caller automated a recommended action of that account, which only an
/// association permits, so the association is recorded with it — the same rule
/// the recommended-action fixture follows
/// (`crate::recommended_actions::record`). The retrieval scope is read from the
/// association rather than from the stored event, so a fixture that recorded
/// only the event would describe a store no call can produce.
pub(crate) fn seed(engine: &Arc<engine::InMemoryEngine>, events: &[StoredEvent]) {
    let state = engine.state::<AutomationState>();
    let mut store = state.lock();
    for event in events {
        if event.account_id != caller_account_id() {
            store.associate_account(&event.account_id, now());
        }
        store.events.insert(event.clone());
    }
}

/// Four events differing in account, event type and event status, created one
/// hour apart in store order.
fn sample_events(base_secs: i64) -> Vec<StoredEvent> {
    vec![
        stored_event(
            0,
            "111111111111",
            "SnapshotAndDeleteUnattachedEbsVolume",
            "Complete",
            base_secs - 4 * 3600,
        ),
        stored_event(
            1,
            "222222222222",
            "UpgradeEbsVolumeType",
            "Failed",
            base_secs - 3 * 3600,
        ),
        stored_event(
            2,
            "111111111111",
            "UpgradeEbsVolumeType",
            "RollbackComplete",
            base_secs - 2 * 3600,
        ),
        stored_event(
            3,
            "333333333333",
            "SnapshotAndDeleteUnattachedEbsVolume",
            "InProgress",
            base_secs - 3600,
        ),
    ]
}

// ---------------------------------------------------------------------------
// Reporting what is stored
// ---------------------------------------------------------------------------

/// An unfiltered list reports every stored event, in store order, with every
/// member built from the stored event rather than from the request.
#[tokio::test]
async fn every_stored_event_is_reported_from_state() {
    let (router, engine) = emulator();
    let base = now_secs();
    let events = sample_events(base);
    seed(&engine, &events);

    let listed = list_ok(&router, &json!({}), "unfiltered").await;
    assert_eq!(
        listed_ids(&listed),
        events
            .iter()
            .map(|event| event.event_id.clone())
            .collect::<Vec<_>>(),
        "case {:?}: every stored event must be reported in store order, got {listed}",
        "unfiltered"
    );
    assert!(
        member(&listed, "nextToken").is_null(),
        "case {:?}: a single page that holds everything carries no token, got {listed}",
        "unfiltered"
    );

    let reported = member(&listed, "automationEvents")
        .as_array()
        .expect("automationEvents is a list")
        .clone();
    for (stored, seen) in events.iter().zip(reported.iter()) {
        let case = &stored.event_id;
        assert_eq!(
            string_member(seen, "accountId"),
            stored.account_id,
            "case {case:?}: accountId must come from the stored event, got {seen}"
        );
        assert_eq!(
            string_member(seen, "eventType"),
            stored.event_type,
            "case {case:?}: eventType must come from the stored event, got {seen}"
        );
        assert_eq!(
            string_member(seen, "eventStatus"),
            stored.event_status,
            "case {case:?}: eventStatus must come from the stored event, got {seen}"
        );
        assert_eq!(
            string_member(seen, "resourceType"),
            stored.resource_type,
            "case {case:?}: resourceType must come from the stored event, got {seen}"
        );
        assert_eq!(
            string_member(seen, "resourceId"),
            stored.resource_id.clone().unwrap_or_default(),
            "case {case:?}: resourceId must come from the stored event, got {seen}"
        );
        assert_eq!(
            member(seen, "createdTimestamp").as_i64(),
            Some(stored.created_timestamp.secs()),
            "case {case:?}: createdTimestamp must be the stored instant, got {seen}"
        );
        assert!(
            member(seen, "completedTimestamp").is_null(),
            "case {case:?}: an event the store has no completion for reports none, got {seen}"
        );
    }
}

/// A list on an empty store is an empty page, not a refusal.
#[tokio::test]
async fn an_empty_store_lists_no_events() {
    let (router, _engine) = emulator();
    let listed = list_ok(&router, &json!({}), "empty-store").await;
    assert_eq!(
        listed_ids(&listed),
        Vec::<String>::new(),
        "case {:?}: an empty store must answer an empty page, got {listed}",
        "empty-store"
    );
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Each `AutomationEventFilterName` selects on the field it names; several
/// filters are ANDed and a filter's values are ORed.
#[tokio::test]
async fn list_filters_select_on_the_field_they_name() {
    let (router, engine) = emulator();
    let base = now_secs();
    let events = sample_events(base);
    seed(&engine, &events);

    let id = |index: usize| events[index].event_id.clone();

    // (case, filters, the eventIds the filter must select) — one row per value
    // of the model's `AutomationEventFilterName`
    // (`smithy_query(query="shape",shape="AutomationEventFilterName")`:
    // AccountId, ResourceType, EventType, EventStatus), plus the OR, AND and
    // no-match rows.
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "AccountId",
            json!([{"name": "AccountId", "values": ["111111111111"]}]),
            vec![id(0), id(2)],
        ),
        (
            "ResourceType",
            json!([{"name": "ResourceType", "values": ["EbsVolume"]}]),
            vec![id(0), id(1), id(2), id(3)],
        ),
        (
            "EventType",
            json!([{"name": "EventType", "values": ["UpgradeEbsVolumeType"]}]),
            vec![id(1), id(2)],
        ),
        (
            "EventStatus",
            json!([{"name": "EventStatus", "values": ["RollbackComplete"]}]),
            vec![id(2)],
        ),
        (
            "values-are-ored",
            json!([{"name": "EventStatus", "values": ["Failed", "InProgress"]}]),
            vec![id(1), id(3)],
        ),
        (
            "filters-are-anded",
            json!([
                {"name": "AccountId", "values": ["111111111111"]},
                {"name": "EventType", "values": ["UpgradeEbsVolumeType"]}
            ]),
            vec![id(2)],
        ),
        (
            "filters-are-anded-to-nothing",
            json!([
                {"name": "AccountId", "values": ["222222222222"]},
                {"name": "EventStatus", "values": ["Complete"]}
            ]),
            vec![],
        ),
        (
            "AccountId-no-match",
            json!([{"name": "AccountId", "values": ["999999999999"]}]),
            vec![],
        ),
        (
            "EventStatus-no-match",
            json!([{"name": "EventStatus", "values": ["Cancelled"]}]),
            vec![],
        ),
    ];

    for (case, filters, expected) in &cases {
        let listed = list_ok(&router, &json!({"filters": filters}), case).await;
        assert_eq!(
            &listed_ids(&listed),
            expected,
            "case {case:?}: the filter must select exactly those events, got {listed}"
        );
    }

    // A filter name outside the modelled value set is refused, not ignored.
    assert_refused(
        call(
            &router,
            "ListAutomationEvents",
            &json!({"filters": [{"name": "EventId", "values": [id(0)]}]}),
        )
        .await,
        400,
        "InvalidParameterValueException",
        "unknown-filter-name",
    );
}

// ---------------------------------------------------------------------------
// The queried time range
// ---------------------------------------------------------------------------

/// One row of the half-open-window table: the bounds a case sends (absent when
/// `None`) and the `eventId`s the window must select. A named struct rather
/// than a tuple so the row reads at the call site.
struct WindowCase {
    case: &'static str,
    start: Option<i64>,
    end: Option<i64>,
    expected: Vec<String>,
}

impl WindowCase {
    fn new(
        case: &'static str,
        start: Option<i64>,
        end: Option<i64>,
        expected: Vec<String>,
    ) -> Self {
        Self {
            case,
            start,
            end,
            expected,
        }
    }
}

/// `[startTimeInclusive, endTimeExclusive)` is half-open: an event created at
/// the start bound is reported, one created at the end bound is not, and an
/// absent bound leaves that side unbounded.
#[tokio::test]
async fn the_queried_time_range_is_a_half_open_window() {
    let (router, engine) = emulator();
    let base = now_secs();
    let events = sample_events(base);
    seed(&engine, &events);

    let id = |index: usize| events[index].event_id.clone();
    let at = |index: usize| events[index].created_timestamp.secs();

    // One row per way a half-open window can fall against the stored events.
    let cases = vec![
        WindowCase::new("both-bounds-absent", None, None, vec![id(0), id(1), id(2), id(3)]),
        WindowCase::new(
            "start-is-inclusive",
            Some(at(1)),
            None,
            vec![id(1), id(2), id(3)],
        ),
        WindowCase::new(
            "end-is-exclusive",
            None,
            Some(at(2)),
            vec![id(0), id(1)],
        ),
        WindowCase::new(
            "both-bounds",
            Some(at(1)),
            Some(at(3)),
            vec![id(1), id(2)],
        ),
        WindowCase::new(
            "one-second-inside-the-start",
            Some(at(1) + 1),
            Some(at(3)),
            vec![id(2)],
        ),
        WindowCase::new("window-after-every-event", Some(at(3) + 1), None, vec![]),
        WindowCase::new("window-before-every-event", None, Some(at(0)), vec![]),
    ];

    for WindowCase {
        case,
        start,
        end,
        expected,
    } in &cases
    {
        let mut body = json!({});
        if let Some(start) = start {
            body["startTimeInclusive"] = json!(start);
        }
        if let Some(end) = end {
            body["endTimeExclusive"] = json!(end);
        }
        let listed = list_ok(&router, &body, case).await;
        assert_eq!(
            &listed_ids(&listed),
            expected,
            "case {case:?}: the window must select exactly those events, got {listed}"
        );
    }
}

/// A range whose end does not come after its start holds nothing, and is
/// refused with the modelled `InvalidParameterValueException` rather than
/// answered with an empty page. A range with one bound is always valid.
#[tokio::test]
async fn a_range_that_cannot_hold_anything_is_refused() {
    let (router, engine) = emulator();
    let base = now_secs();
    let events = sample_events(base);
    seed(&engine, &events);

    let at = |index: usize| events[index].created_timestamp.secs();

    // (case, startTimeInclusive, endTimeExclusive)
    let refused: Vec<(&str, i64, i64)> = vec![
        ("end-one-second-before-start", at(2), at(2) - 1),
        ("end-equals-start", at(1), at(1)),
        ("end-long-before-start", at(3), at(0) - 86_400),
        ("epoch-end-with-a-recent-start", base, 0),
    ];

    for (case, start, end) in &refused {
        let body = assert_refused(
            call(
                &router,
                "ListAutomationEvents",
                &json!({"startTimeInclusive": start, "endTimeExclusive": end}),
            )
            .await,
            400,
            "InvalidParameterValueException",
            case,
        );
        assert_eq!(
            string_member(&body, "message"),
            INVALID_TIME_RANGE_MESSAGE,
            "case {case:?}: the refusal must say why the range cannot hold anything, got {body}"
        );
    }

    // The accepted side of the same rule: a window with room, and either bound
    // on its own.
    let accepted: Vec<(&str, Value)> = vec![
        (
            "end-one-second-after-start",
            json!({"startTimeInclusive": at(0), "endTimeExclusive": at(0) + 1}),
        ),
        ("start-only", json!({"startTimeInclusive": at(3)})),
        ("end-only", json!({"endTimeExclusive": base})),
        ("neither-bound", json!({})),
    ];
    for (case, body) in &accepted {
        list_ok(&router, body, case).await;
    }
}

/// An event created before the retrieval window is never reported, however far
/// back the caller's `startTimeInclusive` points: `ListAutomationEvents`
/// documents that events created within the past year can be retrieved.
#[tokio::test]
async fn events_outside_the_retrieval_window_are_never_reported() {
    let (router, engine) = emulator();
    let base = now_secs();
    let window = i64::try_from(EVENT_RETRIEVAL_WINDOW.as_secs()).expect("a representable window");

    let inside = stored_event(
        1,
        "111111111111",
        "UpgradeEbsVolumeType",
        "Complete",
        base - 3600,
    );
    // Just inside the far edge of the window, and just outside it.
    let edge = stored_event(
        2,
        "111111111111",
        "UpgradeEbsVolumeType",
        "Complete",
        base - window + 60,
    );
    let outside = stored_event(
        3,
        "111111111111",
        "UpgradeEbsVolumeType",
        "Complete",
        base - window - 60,
    );
    let long_outside = stored_event(
        4,
        "111111111111",
        "UpgradeEbsVolumeType",
        "Complete",
        base - 5 * window,
    );
    seed(
        &engine,
        &[
            outside.clone(),
            edge.clone(),
            long_outside.clone(),
            inside.clone(),
        ],
    );

    let reachable = vec![edge.event_id.clone(), inside.event_id.clone()];

    // (case, request) — every way of asking for everything still stops at the
    // retrieval window.
    let cases: Vec<(&str, Value)> = vec![
        ("no-bounds", json!({})),
        ("start-at-the-epoch", json!({"startTimeInclusive": 0})),
        (
            "start-well-before-the-window",
            json!({"startTimeInclusive": base - 10 * window, "endTimeExclusive": base + 1}),
        ),
        (
            "window-entirely-outside",
            json!({"startTimeInclusive": base - 4 * window, "endTimeExclusive": base - 2 * window}),
        ),
    ];

    for (case, body) in &cases {
        let listed = list_ok(&router, body, case).await;
        let expected = if *case == "window-entirely-outside" {
            Vec::new()
        } else {
            reachable.clone()
        };
        assert_eq!(
            listed_ids(&listed),
            expected,
            "case {case:?}: only events created inside the retrieval window are reportable, \
             got {listed}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Paging walks the whole sequence exactly once, whatever the page size, and a
/// token is only honoured for the query it was issued for.
#[tokio::test]
async fn paging_walks_every_event_exactly_once() {
    let (router, engine) = emulator();
    let base = now_secs();
    let events: Vec<StoredEvent> = (0..7)
        .map(|index| {
            stored_event(
                index,
                "111111111111",
                if index % 2 == 0 {
                    "UpgradeEbsVolumeType"
                } else {
                    "SnapshotAndDeleteUnattachedEbsVolume"
                },
                "Complete",
                base - 7200 + i64::try_from(index).expect("a small index") * 60,
            )
        })
        .collect();
    seed(&engine, &events);
    let all: Vec<String> = events.iter().map(|event| event.event_id.clone()).collect();

    // Page sizes spanning the model's `maxResults` range (1..=100): one at a
    // time, a size that does not divide the sequence, the exact length, and
    // more than the store holds.
    for page_size in [1_usize, 2, 3, 7, 100] {
        let case = format!("page-size-{page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut body = json!({"maxResults": page_size});
            if let Some(token) = &token {
                body["nextToken"] = json!(token);
            }
            let listed = list_ok(&router, &body, &case).await;
            let page = listed_ids(&listed);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must not exceed maxResults, got {} ids",
                page.len()
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case {case:?}: the walk must terminate, it has taken {pages} pages"
            );
            match member(&listed, "nextToken").as_str() {
                Some(next) => {
                    assert!(
                        is_next_token(next),
                        "case {case:?}: a token must match the modelled NextToken pattern, \
                         got {next:?}"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            walked, all,
            "case {case:?}: the walk must report every event exactly once, in store order"
        );
        assert_eq!(
            pages,
            all.len().div_ceil(page_size.min(all.len().max(1))),
            "case {case:?}: no page beyond the last, and no token on it"
        );
    }

    // A page size the caller did not send: the default page holds everything
    // this store has.
    let listed = list_ok(&router, &json!({}), "default-page-size").await;
    assert_eq!(
        listed_ids(&listed),
        all,
        "case {:?}: the default page must hold every event, got {listed}",
        "default-page-size"
    );

    // A token this service did not issue, and one issued for a different query,
    // are both refused rather than answered with a page of something else.
    let first = list_ok(&router, &json!({"maxResults": 1}), "token-reuse").await;
    let token = string_member(&first, "nextToken");
    for (case, body) in &[
        (
            "token-with-different-filters",
            json!({"maxResults": 1, "nextToken": token.clone(), "filters": [
                {"name": "EventType", "values": ["UpgradeEbsVolumeType"]}
            ]}),
        ),
        (
            "token-with-different-time-range",
            json!({"maxResults": 1, "nextToken": token.clone(), "startTimeInclusive": base - 3600}),
        ),
        ("foreign-token", json!({"nextToken": "notatokenweissued"})),
    ] {
        assert_refused(
            call(&router, "ListAutomationEvents", body).await,
            400,
            "InvalidParameterValueException",
            case,
        );
    }
}
