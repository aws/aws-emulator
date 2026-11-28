// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the support-case lifecycle: `CreateCase`,
//! `DescribeCases`, `AddCommunicationToCase`, `DescribeCommunications` and
//! `ResolveCase`, driven over the wire through the same router the emulator
//! boots, so every assertion is about the HTTP behaviour a client sees.
//!
//! Each suite asserts a RULE over a set of inputs rather than one recorded
//! request: the tables vary the member set, the page size, the date window and
//! the operation, and the case data every assertion reads is whatever an earlier
//! call persisted.

use std::sync::Arc;

use serde_json::{json, Value};

/// The `X-Amz-Target` prefix this service answers on (awsJson1_1).
const TARGET_PREFIX: &str = "AWSSupport_20130415";

/// An instant before any case this emulator can create.
const EPOCH: &str = "1970-01-01T00:00:00Z";
/// An instant after any case this emulator can create.
const FAR_FUTURE: &str = "2999-12-31T23:59:59Z";

/// A Support plan that may use this API, so the suites below reach the operations
/// at all: the Support API is served only to an account whose plan qualifies (see
/// `subscription_gate.rs` for that rule), and this harness records one the way an
/// operator does with `SUPPORT_PLAN`, before any request runs.
pub(crate) const SUBSCRIBED_PLAN: &str = "enterprise";

/// One emulator with just this service mounted, driven through the production
/// router so routing, deserialization, constraint validation and error
/// rendering are the real ones.
pub(crate) struct Support {
    router: smithy_server_runtime::MultiServiceRouter,
    /// The one shared engine the mounted service reads, kept so a suite can change
    /// the account's Support plan on a running emulator ([`Support::set_plan`]).
    engine: Arc<engine::InMemoryEngine>,
}

impl Support {
    /// An emulator whose account holds [`SUBSCRIBED_PLAN`], so every operation is
    /// reachable.
    pub(crate) fn new() -> Self {
        Self::with_plan(Some(SUBSCRIBED_PLAN))
    }

    /// An emulator whose account holds `plan` — `None` for an account with no
    /// Premium Support subscription at all. Written into the store rather than read
    /// from the environment, so one suite's account cannot change another's.
    pub(crate) fn with_plan(plan: Option<&str>) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        service_com_amazonaws_support_awssupport_20130415::state::service_state(&*engine)
            .expect("the support store is reachable from the shared in-memory engine")
            .lock()
            .support_plan = plan.map(str::to_string);
        let mut router = smithy_server_runtime::MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_support_awssupport_20130415::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        Self { router, engine }
    }

    /// Record `plan` as the account's Support plan on a running emulator, leaving
    /// every case already in the store where it is.
    pub(crate) fn set_plan(&self, plan: Option<&str>) {
        service_com_amazonaws_support_awssupport_20130415::state::service_state(&*self.engine)
            .expect("the support store is reachable from the shared in-memory engine")
            .lock()
            .support_plan = plan.map(str::to_string);
    }

    /// Call one operation; answer its HTTP status and JSON body.
    pub(crate) async fn call(&self, operation: &str, input: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/us-east-1/support/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef",
            )
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .body(bytes::Bytes::from(
                serde_json::to_vec(input).expect("the test input serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, body)
    }

    /// Call one operation and require a 200.
    pub(crate) async fn ok(&self, operation: &str, input: Value) -> Value {
        let (status, body) = self.call(operation, &input).await;
        assert_eq!(
            status, 200,
            "{operation} {input}: expected HTTP 200, got {status} {body}"
        );
        body
    }

    /// Create a case and answer its `caseId`.
    pub(crate) async fn create_case(&self, input: Value) -> String {
        let body = self.ok("CreateCase", input).await;
        body["caseId"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateCase must answer a caseId, got {body}"))
            .to_string()
    }

    /// The one case the store holds for `case_id`, as `DescribeCases` reports it.
    async fn describe_case(&self, case_id: &str, extra: Value) -> Value {
        let mut input = json!({ "caseIdList": [case_id] });
        for (key, value) in extra.as_object().into_iter().flatten() {
            input[key] = value.clone();
        }
        let body = self.ok("DescribeCases", input).await;
        let cases = body["cases"]
            .as_array()
            .unwrap_or_else(|| panic!("DescribeCases must answer a cases list, got {body}"));
        assert_eq!(
            cases.len(),
            1,
            "case {case_id}: expected exactly one case, got {body}"
        );
        cases[0].clone()
    }
}

/// The error code an awsJson1_1 failure carries in its body.
pub(crate) fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_else(|| panic!("an error body carries __type, got {body}"))
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// `caseId` is `case-<12-digit account>-<realm>-<year>-<16 hex>`, the shape
/// `CreateCaseResponse$caseId` documents
/// (`case-12345678910-exen-2025-c4c1d2bf33c5cf47`).
fn assert_case_id_shape(case: &str, label: &str) {
    let segments: Vec<&str> = case.split('-').collect();
    assert_eq!(
        segments.len(),
        5,
        "{label}: expected 5 dash-separated segments in a caseId, got {case:?}"
    );
    assert_eq!(
        segments[0], "case",
        "{label}: expected a caseId to start with \"case\", got {case:?}"
    );
    assert!(
        segments[1].len() == 12 && segments[1].bytes().all(|b| b.is_ascii_digit()),
        "{label}: expected a 12-digit account segment, got {case:?}"
    );
    assert!(
        !segments[2].is_empty() && segments[2].bytes().all(|b| b.is_ascii_lowercase()),
        "{label}: expected a lowercase realm segment, got {case:?}"
    );
    assert!(
        segments[3].len() == 4 && segments[3].bytes().all(|b| b.is_ascii_digit()),
        "{label}: expected a 4-digit year segment, got {case:?}"
    );
    assert!(
        segments[4].len() == 16 && segments[4].bytes().all(|b| b.is_ascii_hexdigit()),
        "{label}: expected a 16-hex-digit segment, got {case:?}"
    );
}

/// A comparable key for an ISO-8601 instant: the fractional seconds padded to 9
/// digits, so comparing two keys as text matches comparing the instants
/// (`.635Z` is `.635000000`, which is EARLIER than `.635289Z`, while a plain
/// string comparison would call it later).
fn instant_key(text: &str) -> String {
    let stem = text.trim_end_matches('Z');
    match stem.split_once('.') {
        Some((whole, fraction)) => format!("{whole}.{fraction:0<9}"),
        None => format!("{stem}.000000000"),
    }
}

/// The `body` of each communication a `DescribeCommunications` page carries.
fn communication_bodies(page: &Value) -> Vec<String> {
    page["communications"]
        .as_array()
        .unwrap_or_else(|| panic!("DescribeCommunications must answer a list, got {page}"))
        .iter()
        .map(|communication| {
            communication["body"]
                .as_str()
                .unwrap_or_else(|| panic!("a communication carries a body, got {communication}"))
                .to_string()
        })
        .collect()
}

/// RULE: `CreateCase` persists the case it was asked for, and `DescribeCases`
/// reports every member back from the store — whatever subset of the optional
/// members the request carried.
#[tokio::test]
async fn create_case_persists_what_describe_cases_reports() {
    // Rows differ along which optional members are present, the value of each,
    // and the number of cc addresses; the serviceCode values satisfy the
    // modelled `^[0-9a-z\-_]+$` pattern.
    let rows = [
        (
            "every member",
            json!({
                "subject": "Emulator smoke test",
                "communicationBody": "initial body",
                "serviceCode": "amazon-elastic-compute-cloud-linux",
                "severityCode": "low",
                "categoryCode": "performance",
                "language": "en",
                "issueType": "technical",
                "ccEmailAddresses": ["one@example.com", "two@example.com"],
            }),
        ),
        (
            "only the required members",
            json!({ "subject": "bare", "communicationBody": "b" }),
        ),
        (
            "another service and severity",
            json!({
                "subject": "throughput drop on a queue",
                "communicationBody": "the queue is slow",
                "serviceCode": "amazon_simple_queue_service",
                "severityCode": "urgent",
                "categoryCode": "other",
                "language": "ja",
                "issueType": "customer-service",
            }),
        ),
        (
            "the modelled maximum of cc addresses",
            json!({
                "subject": "ten watchers",
                "communicationBody": "many eyes",
                "ccEmailAddresses": [
                    "a@example.com", "b@example.com", "c@example.com", "d@example.com",
                    "e@example.com", "f@example.com", "g@example.com", "h@example.com",
                    "i@example.com", "j@example.com",
                ],
            }),
        ),
        (
            "an 8000-character communication body",
            json!({ "subject": "long body", "communicationBody": "x".repeat(8000) }),
        ),
    ];

    let support = Support::new();
    let mut seen_case_ids: Vec<String> = Vec::new();
    let mut seen_display_ids: Vec<String> = Vec::new();

    for (label, request) in rows {
        let case_id = support.create_case(request.clone()).await;
        assert_case_id_shape(&case_id, label);
        assert!(
            !seen_case_ids.contains(&case_id),
            "case {label:?}: expected a caseId no other case has, got {case_id:?} among \
             {seen_case_ids:?}"
        );
        seen_case_ids.push(case_id.clone());

        let case = support.describe_case(&case_id, json!({})).await;
        assert_eq!(
            case["caseId"], json!(case_id),
            "case {label:?}: expected the stored caseId, got {case}"
        );
        assert_eq!(
            case["subject"], request["subject"],
            "case {label:?}: expected the submitted subject, got {case}"
        );
        assert_eq!(
            case["status"],
            json!("opened"),
            "case {label:?}: expected a new case to be opened, got {case}"
        );
        for member in ["serviceCode", "severityCode", "categoryCode", "language"] {
            assert_eq!(
                case[member], request[member],
                "case {label:?}: expected {member} to round-trip, got {case}"
            );
        }
        let expected_cc = request
            .get("ccEmailAddresses")
            .cloned()
            .unwrap_or_else(|| json!([]));
        assert_eq!(
            case["ccEmailAddresses"], expected_cc,
            "case {label:?}: expected the submitted ccEmailAddresses, got {case}"
        );
        assert!(
            case["submittedBy"].as_str().is_some_and(|by| !by.is_empty()),
            "case {label:?}: expected a submittedBy, got {case}"
        );
        assert!(
            case["timeCreated"]
                .as_str()
                .is_some_and(|time| time.len() >= "1970-01-01T00:00:00Z".len()),
            "case {label:?}: expected an ISO 8601 timeCreated, got {case}"
        );

        let display_id = case["displayId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: expected a displayId, got {case}"))
            .to_string();
        assert!(
            !display_id.is_empty() && display_id.bytes().all(|b| b.is_ascii_digit()),
            "case {label:?}: expected a numeric displayId, got {display_id:?}"
        );
        assert!(
            !seen_display_ids.contains(&display_id),
            "case {label:?}: expected a displayId no other case has, got {display_id:?} among \
             {seen_display_ids:?}"
        );
        seen_display_ids.push(display_id.clone());

        // The communicationBody that describes the issue is the case's first
        // communication, both in `recentCommunications` and through
        // `DescribeCommunications`.
        let recent = &case["recentCommunications"]["communications"];
        assert_eq!(
            recent.as_array().map(Vec::len),
            Some(1),
            "case {label:?}: expected exactly one recent communication, got {case}"
        );
        assert_eq!(
            recent[0]["body"], request["communicationBody"],
            "case {label:?}: expected the create body as the first communication, got {case}"
        );
        assert_eq!(
            recent[0]["caseId"], json!(case_id),
            "case {label:?}: expected the communication to name its case, got {case}"
        );

        // And the case is reachable by the displayId it was given.
        let by_display_id = support
            .ok("DescribeCases", json!({ "displayId": display_id }))
            .await;
        assert_eq!(
            by_display_id["cases"].as_array().map(Vec::len),
            Some(1),
            "case {label:?}: expected displayId {display_id} to select one case, got \
             {by_display_id}"
        );
        assert_eq!(
            by_display_id["cases"][0]["caseId"], json!(case_id),
            "case {label:?}: expected displayId {display_id} to select its own case, got \
             {by_display_id}"
        );
    }

    // Every case created above is still listed, so nothing was overwritten.
    let all = support.ok("DescribeCases", json!({})).await;
    assert_eq!(
        all["cases"].as_array().map(Vec::len),
        Some(seen_case_ids.len()),
        "expected every created case to be listed, got {all}"
    );
}

/// RULE: `includeCommunications` selects whether a reported case carries its
/// communications, and `recentCommunications` carries at most the five most
/// recent ones plus a resumption point when the case has more.
#[tokio::test]
async fn recent_communications_follow_include_communications() {
    // Rows differ in how many communications the case carries: fewer than,
    // exactly, and more than the documented five.
    for total in [1_usize, 5, 9] {
        let support = Support::new();
        let case_id = support
            .create_case(json!({ "subject": "recent", "communicationBody": "body 0" }))
            .await;
        for index in 1..total {
            support
                .ok(
                    "AddCommunicationToCase",
                    json!({ "caseId": case_id, "communicationBody": format!("body {index}") }),
                )
                .await;
        }

        let expected_recent = total.min(5);
        for include in [None, Some(true)] {
            let extra = include.map_or_else(|| json!({}), |flag| json!({ "includeCommunications": flag }));
            let case = support.describe_case(&case_id, extra).await;
            let recent = &case["recentCommunications"]["communications"];
            assert_eq!(
                recent.as_array().map(Vec::len),
                Some(expected_recent),
                "case with {total} communications (includeCommunications: {include:?}): \
                 expected {expected_recent} recent, got {case}"
            );
            assert_eq!(
                recent[0]["body"],
                json!(format!("body {}", total - 1)),
                "case with {total} communications (includeCommunications: {include:?}): \
                 expected the newest communication first, got {case}"
            );
            let has_more = total > 5;
            assert_eq!(
                case["recentCommunications"]["nextToken"].is_string(),
                has_more,
                "case with {total} communications: expected a recentCommunications nextToken \
                 only when more than five exist, got {case}"
            );
        }

        let case = support
            .describe_case(&case_id, json!({ "includeCommunications": false }))
            .await;
        assert!(
            case["recentCommunications"].is_null(),
            "case with {total} communications: expected no recentCommunications when \
             includeCommunications is false, got {case}"
        );
    }
}

/// RULE: `DescribeCommunications` reports every communication persisted for a
/// case, newest first, and honours `maxResults` by emitting a `nextToken` the
/// next call resumes from.
#[tokio::test]
async fn describe_communications_pages_newest_first() {
    // Rows differ in page size across the modelled `MaxResults` range (10..100):
    // the lower bound, one past it, and the upper bound.
    for page_size in [10_usize, 11, 100] {
        let support = Support::new();
        let total = 23_usize;
        let case_id = support
            .create_case(json!({ "subject": "pages", "communicationBody": "body 0" }))
            .await;
        for index in 1..total {
            support
                .ok(
                    "AddCommunicationToCase",
                    json!({ "caseId": case_id, "communicationBody": format!("body {index}") }),
                )
                .await;
        }

        let expected: Vec<String> = (0..total).rev().map(|i| format!("body {i}")).collect();
        let mut collected: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut input = json!({ "caseId": case_id, "maxResults": page_size });
            if let Some(token) = next_token.as_deref() {
                input["nextToken"] = json!(token);
            }
            let page = support.ok("DescribeCommunications", input).await;
            let bodies = communication_bodies(&page);
            assert!(
                bodies.len() <= page_size,
                "maxResults {page_size}: expected at most {page_size} communications per page, \
                 got {}",
                bodies.len()
            );
            collected.extend(bodies);
            pages += 1;
            next_token = page["nextToken"].as_str().map(str::to_string);
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= total,
                "maxResults {page_size}: pagination did not terminate after {pages} pages"
            );
        }

        assert_eq!(
            collected, expected,
            "maxResults {page_size}: expected every communication newest-first exactly once, \
             got {collected:?}"
        );
        let expected_pages = total.div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "maxResults {page_size}: expected {expected_pages} pages, got {pages}"
        );
    }
}

/// RULE: `ResolveCase` reports the status the case had before the call and the
/// `resolved` status it has after it, and `DescribeCases` hides a resolved case
/// unless `includeResolvedCases` is true.
#[tokio::test]
async fn resolve_case_moves_the_stored_status() {
    let support = Support::new();
    let case_id = support
        .create_case(json!({ "subject": "to resolve", "communicationBody": "please resolve" }))
        .await;
    let other_id = support
        .create_case(json!({ "subject": "left open", "communicationBody": "still open" }))
        .await;

    // Rows: the first call sees the case as the store holds it (`opened`), and a
    // repeat sees the status the first call left behind.
    for (label, expected_initial) in [("first resolve", "opened"), ("repeat resolve", "resolved")] {
        let resolved = support
            .ok("ResolveCase", json!({ "caseId": case_id }))
            .await;
        assert_eq!(
            resolved["initialCaseStatus"], json!(expected_initial),
            "{label}: expected the status before the call, got {resolved}"
        );
        assert_eq!(
            resolved["finalCaseStatus"],
            json!("resolved"),
            "{label}: expected the case to end resolved, got {resolved}"
        );
    }

    let hidden = support
        .ok("DescribeCases", json!({ "caseIdList": [&case_id] }))
        .await;
    assert_eq!(
        hidden["cases"].as_array().map(Vec::len),
        Some(0),
        "expected a resolved case to be omitted by default, got {hidden}"
    );

    let listed = support
        .ok(
            "DescribeCases",
            json!({ "caseIdList": [&case_id], "includeResolvedCases": true }),
        )
        .await;
    assert_eq!(
        listed["cases"][0]["status"],
        json!("resolved"),
        "expected includeResolvedCases to list the case as resolved, got {listed}"
    );

    // Resolving one case leaves every other case where it was.
    let open = support.ok("DescribeCases", json!({})).await;
    assert_eq!(
        open["cases"].as_array().map(Vec::len),
        Some(1),
        "expected only the untouched case to be listed, got {open}"
    );
    assert_eq!(
        open["cases"][0]["caseId"], json!(other_id),
        "expected the untouched case to be the one listed, got {open}"
    );
}

/// RULE: an operation that takes a `caseId` the store does not hold fails with
/// `CaseIdNotFound` and HTTP 400 — never a success and never a 501.
#[tokio::test]
async fn an_unknown_case_id_is_case_id_not_found() {
    let support = Support::new();
    // A real case exists, so the refusal is about the id and not about an empty
    // store.
    let known = support
        .create_case(json!({ "subject": "known", "communicationBody": "known body" }))
        .await;

    // Rows: ids of the documented shape that were never minted, and ids that are
    // not even case-shaped.
    let unknown_ids = [
        "case-000000000000-muen-2024-0000000000000000",
        "case-999999999999-abcd-1999-ffffffffffffffff",
        "not-a-case-id",
        "",
    ];
    for unknown in unknown_ids {
        let requests = [
            ("DescribeCases", json!({ "caseIdList": [unknown] })),
            ("DescribeCommunications", json!({ "caseId": unknown })),
            (
                "AddCommunicationToCase",
                json!({ "caseId": unknown, "communicationBody": "follow-up" }),
            ),
            ("ResolveCase", json!({ "caseId": unknown })),
        ];
        for (operation, input) in requests {
            let (status, body) = support.call(operation, &input).await;
            assert_eq!(
                status, 400,
                "{operation} with caseId {unknown:?}: expected HTTP 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "CaseIdNotFound",
                "{operation} with caseId {unknown:?}: expected CaseIdNotFound, got {body}"
            );
        }
    }

    // None of those refusals touched the case that does exist.
    let case = support.describe_case(&known, json!({})).await;
    assert_eq!(
        case["status"],
        json!("opened"),
        "expected the known case to be untouched, got {case}"
    );
    let communications = support
        .ok("DescribeCommunications", json!({ "caseId": known }))
        .await;
    assert_eq!(
        communication_bodies(&communications),
        vec!["known body".to_string()],
        "expected the known case to still carry just its create body, got {communications}"
    );
}

/// RULE: `dryRun` validates the request and then refuses it with
/// `DryRunOperationException`, leaving the store exactly as it was.
#[tokio::test]
async fn dry_run_refuses_and_changes_nothing() {
    let support = Support::new();
    let case_id = support
        .create_case(json!({ "subject": "dry run", "communicationBody": "only body" }))
        .await;

    // Rows: every operation in the slice, each with a request that would
    // otherwise succeed.
    let rows = [
        (
            "CreateCase",
            json!({ "subject": "never stored", "communicationBody": "never stored", "dryRun": true }),
        ),
        (
            "DescribeCases",
            json!({ "caseIdList": [&case_id], "dryRun": true }),
        ),
        (
            "AddCommunicationToCase",
            json!({ "caseId": &case_id, "communicationBody": "never appended", "dryRun": true }),
        ),
        (
            "DescribeCommunications",
            json!({ "caseId": &case_id, "dryRun": true }),
        ),
        ("ResolveCase", json!({ "caseId": &case_id, "dryRun": true })),
    ];
    for (operation, input) in rows {
        let (status, body) = support.call(operation, &input).await;
        assert_eq!(
            status, 400,
            "{operation} with dryRun: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperationException",
            "{operation} with dryRun: expected DryRunOperationException, got {body}"
        );
    }

    let all = support.ok("DescribeCases", json!({})).await;
    assert_eq!(
        all["cases"].as_array().map(Vec::len),
        Some(1),
        "expected dryRun to create no case, got {all}"
    );
    assert_eq!(
        all["cases"][0]["status"],
        json!("opened"),
        "expected dryRun to resolve no case, got {all}"
    );
    let communications = support
        .ok("DescribeCommunications", json!({ "caseId": case_id }))
        .await;
    assert_eq!(
        communication_bodies(&communications),
        vec!["only body".to_string()],
        "expected dryRun to append no communication, got {communications}"
    );
}

/// RULE: `afterTime` and `beforeTime` bound a reported record by its
/// `timeCreated`, inclusively at each bound, and an instant that is not ISO 8601
/// is refused as a bad request.
#[tokio::test]
async fn the_date_window_filters_both_readers() {
    let support = Support::new();
    let case_id = support
        .create_case(json!({ "subject": "windowed", "communicationBody": "windowed body" }))
        .await;
    let case = support.describe_case(&case_id, json!({})).await;
    let case_time = case["timeCreated"]
        .as_str()
        .expect("a case carries timeCreated")
        .to_string();
    let communications = support
        .ok("DescribeCommunications", json!({ "caseId": &case_id }))
        .await;
    let communication_time = communications["communications"][0]["timeCreated"]
        .as_str()
        .expect("a communication carries timeCreated")
        .to_string();

    // Rows: each published bound of the window, both bounds together, and the
    // record's own instant at each bound (the inclusive edge).
    let rows: [(&str, Option<&str>, Option<&str>, bool); 7] = [
        ("after the epoch", Some(EPOCH), None, true),
        ("after the far future", Some(FAR_FUTURE), None, false),
        ("before the far future", None, Some(FAR_FUTURE), true),
        ("before the epoch", None, Some(EPOCH), false),
        ("a window spanning it", Some(EPOCH), Some(FAR_FUTURE), true),
        ("a window below it", Some(EPOCH), Some(EPOCH), false),
        ("a window above it", Some(FAR_FUTURE), Some(FAR_FUTURE), false),
    ];
    for (label, after, before, expected) in rows {
        for (operation, own_time, mut input) in [
            (
                "DescribeCases",
                case_time.as_str(),
                json!({ "caseIdList": [&case_id] }),
            ),
            (
                "DescribeCommunications",
                communication_time.as_str(),
                json!({ "caseId": &case_id }),
            ),
        ] {
            if let Some(after) = after {
                input["afterTime"] = json!(after);
            }
            if let Some(before) = before {
                input["beforeTime"] = json!(before);
            }
            let body = support.ok(operation, input.clone()).await;
            let items = if operation == "DescribeCases" {
                &body["cases"]
            } else {
                &body["communications"]
            };
            let found = items.as_array().map(Vec::len).unwrap_or_default();
            assert_eq!(
                found,
                usize::from(expected),
                "{operation} with {label} ({input}): expected {} record(s), got {body}",
                usize::from(expected)
            );

            // The record's own instant is inside the window at either bound.
            for member in ["afterTime", "beforeTime"] {
                let mut inclusive = input.clone();
                inclusive.as_object_mut().expect("an object").remove("afterTime");
                inclusive.as_object_mut().expect("an object").remove("beforeTime");
                inclusive[member] = json!(own_time);
                let body = support.ok(operation, inclusive.clone()).await;
                let items = if operation == "DescribeCases" {
                    &body["cases"]
                } else {
                    &body["communications"]
                };
                assert_eq!(
                    items.as_array().map(Vec::len),
                    Some(1),
                    "{operation} with {member} at the record's own instant: expected the \
                     record to be inside the window, got {body}"
                );
            }
        }
    }

    // An instant that is not ISO 8601 is a bad request, named by its member.
    for operation in ["DescribeCases", "DescribeCommunications"] {
        for member in ["afterTime", "beforeTime"] {
            let mut input = if operation == "DescribeCases" {
                json!({ "caseIdList": [&case_id] })
            } else {
                json!({ "caseId": &case_id })
            };
            input[member] = json!("yesterday");
            let (status, body) = support.call(operation, &input).await;
            assert_eq!(
                status, 400,
                "{operation} with {member} = \"yesterday\": expected HTTP 400, got \
                 {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "{operation} with {member} = \"yesterday\": expected ValidationException, got \
                 {body}"
            );
            assert!(
                body["message"].as_str().is_some_and(|m| m.contains(member)),
                "{operation} with {member} = \"yesterday\": expected the message to name the \
                 member, got {body}"
            );
        }
    }
}

/// RULE: `DescribeCases` reports the stored cases in a deterministic order —
/// newest `timeCreated` first, ties broken by `caseId` — and `maxResults` pages
/// through that order without dropping or repeating a case.
#[tokio::test]
async fn describe_cases_pages_in_a_deterministic_order() {
    let support = Support::new();
    let total = 23_usize;
    for index in 0..total {
        support
            .create_case(json!({
                "subject": format!("case {index}"),
                "communicationBody": format!("body {index}"),
            }))
            .await;
    }

    for page_size in [10_usize, 100] {
        let mut collected: Vec<(String, String)> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut input = json!({ "maxResults": page_size });
            if let Some(token) = next_token.as_deref() {
                input["nextToken"] = json!(token);
            }
            let page = support.ok("DescribeCases", input).await;
            let cases = page["cases"]
                .as_array()
                .unwrap_or_else(|| panic!("DescribeCases must answer a list, got {page}"));
            assert!(
                cases.len() <= page_size,
                "maxResults {page_size}: expected at most {page_size} cases per page, got {}",
                cases.len()
            );
            for case in cases {
                collected.push((
                    case["timeCreated"].as_str().unwrap_or_default().to_string(),
                    case["caseId"].as_str().unwrap_or_default().to_string(),
                ));
            }
            pages += 1;
            next_token = page["nextToken"].as_str().map(str::to_string);
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= total,
                "maxResults {page_size}: pagination did not terminate after {pages} pages"
            );
        }

        assert_eq!(
            collected.len(),
            total,
            "maxResults {page_size}: expected every case exactly once, got {collected:?}"
        );
        let mut unique: Vec<&(String, String)> = collected.iter().collect();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            total,
            "maxResults {page_size}: expected no case to repeat across pages, got {collected:?}"
        );
        let mut expected = collected.clone();
        expected.sort_by(|left, right| {
            instant_key(&right.0)
                .cmp(&instant_key(&left.0))
                .then_with(|| left.1.cmp(&right.1))
        });
        assert_eq!(
            collected, expected,
            "maxResults {page_size}: expected newest timeCreated first with caseId breaking \
             ties, got {collected:?}"
        );
    }
}

/// RULE: a case can only carry attachments the attachment operations minted, and
/// those operations are still scaffolded — so an `attachmentSetId` names a set
/// that cannot be found, rather than being silently dropped.
#[tokio::test]
async fn an_attachment_set_that_was_never_created_is_not_found() {
    let support = Support::new();
    let case_id = support
        .create_case(json!({ "subject": "attachments", "communicationBody": "no files" }))
        .await;

    let rows = [
        (
            "CreateCase",
            json!({
                "subject": "with an attachment set",
                "communicationBody": "see attached",
                "attachmentSetId": "as-0000000000000000",
            }),
        ),
        (
            "AddCommunicationToCase",
            json!({
                "caseId": &case_id,
                "communicationBody": "see attached",
                "attachmentSetId": "as-ffffffffffffffff",
            }),
        ),
    ];
    for (operation, input) in rows {
        let (status, body) = support.call(operation, &input).await;
        assert_eq!(
            status, 400,
            "{operation} with an unknown attachmentSetId: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "AttachmentSetIdNotFound",
            "{operation} with an unknown attachmentSetId: expected AttachmentSetIdNotFound, got \
             {body}"
        );
    }

    // The refusals stored nothing.
    let all = support.ok("DescribeCases", json!({})).await;
    assert_eq!(
        all["cases"].as_array().map(Vec::len),
        Some(1),
        "expected the refused CreateCase to store no case, got {all}"
    );
    let communications = support
        .ok("DescribeCommunications", json!({ "caseId": case_id }))
        .await;
    assert_eq!(
        communication_bodies(&communications),
        vec!["no files".to_string()],
        "expected the refused AddCommunicationToCase to append nothing, got {communications}"
    );
}

/// RULE: a `nextToken` this service did not mint is a bad request, not a page.
#[tokio::test]
async fn a_forged_next_token_is_refused() {
    let support = Support::new();
    let case_id = support
        .create_case(json!({ "subject": "tokens", "communicationBody": "token body" }))
        .await;

    for token in ["zzzz", "0", "not-hex", "ff"] {
        for (operation, mut input) in [
            ("DescribeCases", json!({ "maxResults": 10 })),
            (
                "DescribeCommunications",
                json!({ "caseId": &case_id, "maxResults": 10 }),
            ),
        ] {
            input["nextToken"] = json!(token);
            let (status, body) = support.call(operation, &input).await;
            assert_eq!(
                status, 400,
                "{operation} with nextToken {token:?}: expected HTTP 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "{operation} with nextToken {token:?}: expected ValidationException, got {body}"
            );
        }
    }
}

/// RULE: the store is bounded — the case count at
/// `state::MAX_STORED_CASES`, past which `CreateCase` answers its own modelled
/// `CaseCreationLimitExceeded`; and one case's correspondence log at
/// `state::MAX_COMMUNICATIONS_PER_CASE`, past which the OLDEST communications
/// are dropped and the newest are still reported.
#[tokio::test]
async fn the_store_is_bounded_at_both_growth_sites() {
    use service_com_amazonaws_support_awssupport_20130415::state::{
        MAX_COMMUNICATIONS_PER_CASE, MAX_STORED_CASES,
    };

    let support = Support::new();
    for index in 0..MAX_STORED_CASES {
        support
            .create_case(json!({
                "subject": format!("case {index}"),
                "communicationBody": format!("body {index}"),
            }))
            .await;
    }

    let (status, body) = support
        .call(
            "CreateCase",
            &json!({ "subject": "one too many", "communicationBody": "one too many" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {} of {MAX_STORED_CASES}: expected HTTP 400, got {status} {body}",
        MAX_STORED_CASES + 1
    );
    assert_eq!(
        error_code(&body),
        "CaseCreationLimitExceeded",
        "case {} of {MAX_STORED_CASES}: expected CaseCreationLimitExceeded, got {body}",
        MAX_STORED_CASES + 1
    );

    // The bound refuses the new case without disturbing the stored ones.
    let all = support
        .ok("DescribeCases", json!({ "maxResults": 100 }))
        .await;
    assert!(
        all["cases"]
            .as_array()
            .is_some_and(|cases| cases.len() == 100),
        "expected the stored cases to still be listed, got {all}"
    );

    // The per-case log: push well past the bound and only the newest survive.
    let busy = Support::new();
    let case_id = busy
        .create_case(json!({ "subject": "busy", "communicationBody": "body 0" }))
        .await;
    let total = MAX_COMMUNICATIONS_PER_CASE + 8;
    for index in 1..total {
        busy.ok(
            "AddCommunicationToCase",
            json!({ "caseId": &case_id, "communicationBody": format!("body {index}") }),
        )
        .await;
    }

    let page = busy
        .ok(
            "DescribeCommunications",
            json!({ "caseId": &case_id, "maxResults": 100 }),
        )
        .await;
    let expected: Vec<String> = (total - MAX_COMMUNICATIONS_PER_CASE..total)
        .rev()
        .map(|index| format!("body {index}"))
        .collect();
    assert_eq!(
        communication_bodies(&page),
        expected,
        "expected the newest {MAX_COMMUNICATIONS_PER_CASE} communications of {total}, got {page}"
    );
}

/// RULE: `ccEmailAddresses` that would take a case past the addresses the
/// modelled `CcEmailAddressList` allows is refused, not partly applied.
#[tokio::test]
async fn cc_addresses_past_the_modelled_maximum_are_refused() {
    let support = Support::new();
    // The case starts with 6 of the 10 addresses a case may carry.
    let case_id = support
        .create_case(json!({
            "subject": "cc limits",
            "communicationBody": "first body",
            "ccEmailAddresses": [
                "a@example.com", "b@example.com", "c@example.com",
                "d@example.com", "e@example.com", "f@example.com",
            ],
        }))
        .await;

    // Rows: addresses the case already holds (a no-op merge), a merge that just
    // fits, and one that would overflow.
    let rows: [(&str, Value, bool); 3] = [
        (
            "addresses the case already holds",
            json!(["a@example.com", "b@example.com"]),
            true,
        ),
        (
            "a merge that exactly fills the list",
            json!(["g@example.com", "h@example.com", "i@example.com", "j@example.com"]),
            true,
        ),
        (
            "a merge past the maximum",
            json!(["k@example.com"]),
            false,
        ),
    ];
    for (label, addresses, accepted) in rows {
        let input = json!({
            "caseId": &case_id,
            "communicationBody": format!("body for {label}"),
            "ccEmailAddresses": addresses,
        });
        let (status, body) = support.call("AddCommunicationToCase", &input).await;
        if accepted {
            assert_eq!(
                status, 200,
                "{label}: expected HTTP 200, got {status} {body}"
            );
            assert_eq!(
                body["result"],
                json!(true),
                "{label}: expected result true, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "{label}: expected HTTP 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "{label}: expected ValidationException, got {body}"
            );
            assert!(
                body["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("ccEmailAddresses")),
                "{label}: expected the message to name ccEmailAddresses, got {body}"
            );
        }
    }

    // The case holds exactly the 10 accepted addresses, and the refused merge
    // appended no communication.
    let case = support.describe_case(&case_id, json!({})).await;
    assert_eq!(
        case["ccEmailAddresses"].as_array().map(Vec::len),
        Some(10),
        "expected the case to hold the ten accepted addresses, got {case}"
    );
    let communications = support
        .ok("DescribeCommunications", json!({ "caseId": &case_id }))
        .await;
    assert_eq!(
        communication_bodies(&communications).len(),
        3,
        "expected only the create body and the two accepted communications, got {communications}"
    );
}

/// RULE: a value a member of this service's model constrains is served only inside
/// that constraint — a request carrying one outside it is refused as a bad request,
/// and a refused request neither creates a case nor appends a communication.
///
/// Rows span the constraint kinds the model declares on the case-lifecycle inputs,
/// each at the bound and one step outside it, plus an absent `@required` member:
/// `CommunicationBody` (`@length` 1..8000, carried by `CreateCase$communicationBody`
/// and `AddCommunicationToCase$communicationBody`), `ServiceCode2` (`@pattern`
/// `^[0-9a-z\-_]+$`, carried by `CreateCase$serviceCode`), `MaxResults` (`@range`
/// 10..100, carried by `DescribeCases$maxResults` and
/// `DescribeCommunications$maxResults`), `CaseIdList` (`@length` 0..100),
/// `CcEmailAddressList` (`@length` 0..10) and `UploadIds` (`@length` 0..10 over
/// `UploadId`, itself `@length` 1..2048). An id a client may legally send but no
/// operation has minted is NOT a constraint violation: an unknown `caseId` is the
/// operation's own `CaseIdNotFound`.
#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_refused_and_stores_nothing() {
    let support = Support::new();
    let case_id = support
        .create_case(json!({ "subject": "bounds", "communicationBody": "bounds body" }))
        .await;
    // A `caseIdList` of exactly the modelled maximum of 100 ids, none of them minted.
    let hundred_unknown: Vec<String> = (0..100)
        .map(|index| format!("case-000000000000-muen-2024-{index:016x}"))
        .collect();
    let hundred_and_one_unknown: Vec<String> = (0..101)
        .map(|index| format!("case-000000000000-muen-2024-{index:016x}"))
        .collect();
    let eleven_addresses: Vec<String> = (0..11).map(|i| format!("cc{i}@example.com")).collect();
    let eleven_uploads: Vec<String> = (0..11).map(|i| format!("upload-{i:04}")).collect();

    // `None` expects HTTP 200; `Some(code)` expects HTTP 400 carrying that code.
    let rows: [(&str, &str, Value, Option<&str>); 22] = [
        (
            "communicationBody at the modelled minimum of 1",
            "CreateCase",
            json!({ "subject": "at the minimum", "communicationBody": "x" }),
            None,
        ),
        (
            "communicationBody below the modelled minimum of 1",
            "CreateCase",
            json!({ "subject": "below the minimum", "communicationBody": "" }),
            Some("ValidationException"),
        ),
        (
            "communicationBody at the modelled maximum of 8000",
            "AddCommunicationToCase",
            json!({ "caseId": &case_id, "communicationBody": "y".repeat(8000) }),
            None,
        ),
        (
            "communicationBody past the modelled maximum of 8000",
            "AddCommunicationToCase",
            json!({ "caseId": &case_id, "communicationBody": "y".repeat(8001) }),
            Some("ValidationException"),
        ),
        (
            "the required subject absent",
            "CreateCase",
            json!({ "communicationBody": "no subject" }),
            Some("ValidationException"),
        ),
        (
            "the required communicationBody absent",
            "CreateCase",
            json!({ "subject": "no body" }),
            Some("ValidationException"),
        ),
        (
            "the required caseId absent",
            "DescribeCommunications",
            json!({}),
            Some("ValidationException"),
        ),
        (
            "a serviceCode of lowercase letters, digits, dashes and underscores",
            "CreateCase",
            json!({
                "subject": "a legal serviceCode",
                "communicationBody": "a legal serviceCode",
                "serviceCode": "a-0_z",
            }),
            None,
        ),
        (
            "a serviceCode carrying an uppercase letter",
            "CreateCase",
            json!({
                "subject": "an illegal serviceCode",
                "communicationBody": "an illegal serviceCode",
                "serviceCode": "Amazon-EC2",
            }),
            Some("ValidationException"),
        ),
        (
            "a serviceCode carrying a space",
            "CreateCase",
            json!({
                "subject": "an illegal serviceCode",
                "communicationBody": "an illegal serviceCode",
                "serviceCode": "amazon ec2",
            }),
            Some("ValidationException"),
        ),
        (
            "a serviceCode carrying a dot",
            "CreateCase",
            json!({
                "subject": "an illegal serviceCode",
                "communicationBody": "an illegal serviceCode",
                "serviceCode": "amazon.ec2",
            }),
            Some("ValidationException"),
        ),
        (
            "an empty serviceCode, which the pattern's + cannot match",
            "CreateCase",
            json!({
                "subject": "an illegal serviceCode",
                "communicationBody": "an illegal serviceCode",
                "serviceCode": "",
            }),
            Some("ValidationException"),
        ),
        (
            "maxResults at the bottom of the modelled range of 10..100",
            "DescribeCases",
            json!({ "maxResults": 10 }),
            None,
        ),
        (
            "maxResults at the top of the modelled range of 10..100",
            "DescribeCases",
            json!({ "maxResults": 100 }),
            None,
        ),
        (
            "maxResults below the modelled range of 10..100",
            "DescribeCases",
            json!({ "maxResults": 9 }),
            Some("ValidationException"),
        ),
        (
            "maxResults above the modelled range of 10..100",
            "DescribeCases",
            json!({ "maxResults": 101 }),
            Some("ValidationException"),
        ),
        (
            "maxResults below the modelled range on the other paged reader",
            "DescribeCommunications",
            json!({ "caseId": &case_id, "maxResults": 9 }),
            Some("ValidationException"),
        ),
        (
            "maxResults at the bottom of the range on the other paged reader",
            "DescribeCommunications",
            json!({ "caseId": &case_id, "maxResults": 10 }),
            None,
        ),
        (
            "a caseIdList of the modelled maximum of 100 ids, none of them minted",
            "DescribeCases",
            json!({ "caseIdList": hundred_unknown }),
            Some("CaseIdNotFound"),
        ),
        (
            "a caseIdList past the modelled maximum of 100",
            "DescribeCases",
            json!({ "caseIdList": hundred_and_one_unknown }),
            Some("ValidationException"),
        ),
        (
            "ccEmailAddresses past the modelled maximum of 10",
            "CreateCase",
            json!({
                "subject": "too many watchers",
                "communicationBody": "too many watchers",
                "ccEmailAddresses": eleven_addresses,
            }),
            Some("ValidationException"),
        ),
        (
            "uploadIds past the modelled maximum of 10",
            "CreateCase",
            json!({
                "subject": "too many uploads",
                "communicationBody": "too many uploads",
                "uploadIds": eleven_uploads,
            }),
            Some("ValidationException"),
        ),
    ];

    // What the accepted rows add, so the store can be checked against them.
    let mut created = 1_usize;
    let mut communications = 1_usize;
    for (label, operation, input, expected) in rows {
        let (status, body) = support.call(operation, &input).await;
        match expected {
            None => {
                assert_eq!(
                    status, 200,
                    "{operation} with {label}: expected HTTP 200, got {status} {body}"
                );
                match operation {
                    "CreateCase" => created += 1,
                    "AddCommunicationToCase" => communications += 1,
                    _ => {}
                }
            }
            Some(code) => {
                assert_eq!(
                    status, 400,
                    "{operation} with {label}: expected HTTP 400, got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    code,
                    "{operation} with {label}: expected {code}, got {body}"
                );
            }
        }
    }

    // Every refusal left the store as it was: only the accepted rows wrote.
    let all = support
        .ok("DescribeCases", json!({ "maxResults": 100 }))
        .await;
    assert_eq!(
        all["cases"].as_array().map(Vec::len),
        Some(created),
        "expected only the accepted creates to be stored, got {all}"
    );
    let log = support
        .ok("DescribeCommunications", json!({ "caseId": &case_id }))
        .await;
    assert_eq!(
        communication_bodies(&log).len(),
        communications,
        "expected only the accepted communications on the case, got {log}"
    );
}

/// RULE: a reported `Communication` carries both attachment views the model
/// declares — `attachments`, every attachment on the communication, and
/// `attachmentSet`, those 5 MB or smaller — so `attachmentSet` is always a subset
/// of `attachments`, wherever a communication is reported.
///
/// No attachment can be on a communication yet: the operations that mint one
/// (`AddAttachmentsToSet`, `GetAttachmentUploadLinks`) are scaffolded, and a case
/// submission naming an attachment set or an upload id is refused — so both views
/// are present and empty here, and neither may be omitted.
#[tokio::test]
async fn a_reported_communication_carries_both_modelled_attachment_views() {
    let support = Support::new();
    // Rows differ along how many communications the case carries.
    for extra in 0..3_usize {
        let label = format!("{} communication(s)", extra + 1);
        let case_id = support
            .create_case(json!({
                "subject": format!("attachment views, {label}"),
                "communicationBody": "create body",
            }))
            .await;
        for index in 0..extra {
            support
                .ok(
                    "AddCommunicationToCase",
                    json!({ "caseId": &case_id, "communicationBody": format!("body {index}") }),
                )
                .await;
        }

        // Both readers that report a communication: the correspondence log and the
        // `recentCommunications` a case carries.
        let log = support
            .ok("DescribeCommunications", json!({ "caseId": &case_id }))
            .await;
        let case = support
            .ok("DescribeCases", json!({ "caseIdList": [&case_id] }))
            .await;
        let readers = [
            ("DescribeCommunications", log["communications"].clone()),
            (
                "DescribeCases.recentCommunications",
                case["cases"][0]["recentCommunications"]["communications"].clone(),
            ),
        ];
        for (reader, reported) in readers {
            let reported = reported.as_array().cloned().unwrap_or_else(|| {
                panic!("{reader} with {label}: expected a list of communications, got {reported}")
            });
            assert_eq!(
                reported.len(),
                extra + 1,
                "{reader} with {label}: expected {} communications, got {reported:?}",
                extra + 1
            );
            for communication in &reported {
                let attachments = communication["attachments"].as_array().unwrap_or_else(|| {
                    panic!(
                        "{reader} with {label}: expected an attachments list, got {communication}"
                    )
                });
                let attachment_set =
                    communication["attachmentSet"].as_array().unwrap_or_else(|| {
                        panic!(
                            "{reader} with {label}: expected an attachmentSet list, got \
                             {communication}"
                        )
                    });
                assert!(
                    attachment_set
                        .iter()
                        .all(|entry| attachments.contains(entry)),
                    "{reader} with {label}: attachmentSet must be a subset of attachments, got \
                     {communication}"
                );
                assert!(
                    attachments.is_empty(),
                    "{reader} with {label}: no operation can attach a file to a communication \
                     yet, got {communication}"
                );
            }
        }
    }
}
