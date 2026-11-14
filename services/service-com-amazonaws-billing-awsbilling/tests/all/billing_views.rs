// SPDX-License-Identifier: CC0-1.0

//! The billing-view lifecycle, end to end over the awsJson1_0 wire.
//!
//! Every assertion here is about the RULE the five implemented operations follow:
//! each one answers from the record persisted in
//! `ComAmazonawsBillingAwsbillingState::billing_views`, so what a create stored is
//! what a later get, list or update reports, and what a delete removed is gone for
//! the next caller. The tables span the class of inputs the rule generalises over
//! (name/description shape, source-view count, tag count, filter kind, page size),
//! not one recorded request.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_billing_awsbilling::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT_ID: &str = "000000000000";

/// The ARN of the `PRIMARY` billing view every account owns by default, in the
/// modeled `BillingViewArn` form.
pub(crate) const PRIMARY_ARN: &str = "arn:aws:billing::000000000000:billingview/primary";

/// A billing-view ARN that is pattern-valid but names no view this emulator
/// created.
pub(crate) const ABSENT_ARN: &str = "arn:aws:billing::000000000000:billingview/does-not-exist";

/// One router over ONE shared engine — what `core/router` builds in production,
/// so every call in a test observes the same state.
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// A router over the service's DEFAULT state, plus its engine: the emulated
/// account as it stands up, a standalone account that is nobody's management
/// account.
pub(crate) fn standalone_emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>)
{
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// The same router, plus the engine it was built over, for the one test that
/// asserts a store's resident size rather than an API answer.
///
/// `CreateBillingView` is an organization-management-account action, so this
/// harness models the caller as the management account of an organization
/// (`organization_management_account_id`) — the world in which the billing
/// view lifecycle these tests are about is reachable at all. The refusal a
/// caller that is NOT the management account gets is asserted in
/// `tests/all/rejection_fidelity.rs`, over the default state.
pub(crate) fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = standalone_emulator_with_engine();
    engine
        .state::<service_com_amazonaws_billing_awsbilling::ComAmazonawsBillingAwsbillingState>()
        .lock()
        .organization_management_account_id = Some(ACCOUNT_ID.to_string());
    (router, engine)
}

/// Send one awsJson1_0 call and return its HTTP status and decoded body.
///
/// The `Authorization` credential scope names `billing`, which is how the router
/// picks this service (`resolve_route_key`); `X-Amz-Target` names the operation.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let (status, _headers, value) = call_with_headers(router, operation, body).await;
    (status, value)
}

/// As [`call`], handing back the response HEADERS too — for the suite that
/// asserts on one (the content-type this surface answers with).
pub(crate) async fn call_with_headers(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, http::HeaderMap, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AWSBilling.{operation}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/billing/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let (parts, bytes) = response.into_parts();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parts.headers, value)
}

/// The `__type` an awsJson error body carries, without its namespace prefix.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Plant one billing view directly in the store under `arn`, cloning the
/// account's `PRIMARY` view as the template and handing the clone to `shape` for
/// the fields the calling suite varies.
///
/// `CreateBillingView` mints a `CUSTOM` view in the CALLER's own account only, so
/// a suite about any other record the store can hold — a view type the service
/// manages, a view that came into existence in the past, a record under an ARN of
/// another account — writes it the way the service would. The template is a
/// record the service itself produced, so every field the caller does not set
/// holds a value the service made.
pub(crate) fn plant_view(
    engine: &Arc<engine::InMemoryEngine>,
    arn: &str,
    shape: impl FnOnce(&mut service_com_amazonaws_billing_awsbilling::state::BillingView),
) {
    let state = engine
        .state::<service_com_amazonaws_billing_awsbilling::ComAmazonawsBillingAwsbillingState>();
    let mut state = state.lock();
    let mut view = state
        .billing_views
        .get(PRIMARY_ARN)
        .cloned()
        .expect("the account's PRIMARY view is stored at stand-up");
    view.arn = arn.to_string();
    shape(&mut view);
    state.billing_views.insert(arn.to_string(), view);
}

/// One row of the create/get round-trip table: a billing view this emulator must
/// store and report back unchanged.
struct ViewRow {
    case: &'static str,
    name: String,
    description: Option<String>,
    source_views: Vec<String>,
    tags: Vec<(&'static str, &'static str)>,
    data_filter_expression: Option<Value>,
}

/// The rows differ along everything the rule generalises over: name shape and
/// length (including the `@length` maximum of 128), whether a description is
/// absent, empty or long, how many source views are associated (1 through the
/// modeled maximum of 10), how many tags are attached, and whether a data filter
/// expression is carried.
fn view_rows() -> Vec<ViewRow> {
    let long_source_views: Vec<String> = (0..10)
        .map(|i| format!("arn:aws:billing::{ACCOUNT_ID}:billingview/source-{i}"))
        .collect();
    vec![
        ViewRow {
            case: "one source view, a description",
            name: "demo-view".to_string(),
            description: Some("first view".to_string()),
            source_views: vec![PRIMARY_ARN.to_string()],
            tags: vec![],
            data_filter_expression: None,
        },
        ViewRow {
            case: "no description, two source views",
            name: "demo view two".to_string(),
            description: None,
            source_views: vec![PRIMARY_ARN.to_string(), long_source_views[0].clone()],
            tags: vec![("team", "finance")],
            data_filter_expression: None,
        },
        ViewRow {
            case: "empty description, every allowed name character",
            name: "a_Z9 +=.-@".to_string(),
            description: Some(String::new()),
            source_views: vec![PRIMARY_ARN.to_string()],
            tags: vec![("a", ""), ("b", "2")],
            data_filter_expression: Some(json!({
                "dimensions": { "key": "LINKED_ACCOUNT", "values": [ACCOUNT_ID] }
            })),
        },
        ViewRow {
            case: "name at the @length maximum, 10 source views",
            name: "n".repeat(128),
            description: Some("d".repeat(1024)),
            source_views: long_source_views,
            tags: vec![],
            data_filter_expression: Some(json!({
                "tags": { "key": "costCenter", "values": ["a", "b"] }
            })),
        },
    ]
}

impl ViewRow {
    /// The `CreateBillingView` request body for this row.
    fn create_body(&self, client_token: &str) -> Value {
        let mut body = json!({
            "name": self.name,
            "sourceViews": self.source_views,
            "clientToken": client_token,
        });
        if let Some(description) = &self.description {
            body["description"] = json!(description);
        }
        if !self.tags.is_empty() {
            body["resourceTags"] = Value::Array(
                self.tags
                    .iter()
                    .map(|(key, value)| json!({ "key": key, "value": value }))
                    .collect(),
            );
        }
        if let Some(expression) = &self.data_filter_expression {
            body["dataFilterExpression"] = expression.clone();
        }
        body
    }
}

#[tokio::test]
async fn a_created_view_is_reported_back_by_get_for_every_row() {
    let router = emulator();

    for (index, row) in view_rows().iter().enumerate() {
        let case = row.case;
        let token = format!("token-{index}");
        let (status, created) = call(&router, "CreateBillingView", &row.create_body(&token)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateBillingView to answer 200, got {status} {created:?}"
        );

        let arn = created["arn"].as_str().unwrap_or_default().to_string();
        assert!(
            arn.starts_with(&format!("arn:aws:billing::{ACCOUNT_ID}:billingview/")),
            "case {case:?}: expected a minted billing-view ARN, got {arn:?}"
        );
        assert!(
            created["createdAt"].is_number(),
            "case {case:?}: expected a createdAt timestamp, got {:?}",
            created["createdAt"]
        );

        let (status, got) = call(&router, "GetBillingView", &json!({ "arn": arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetBillingView to answer 200, got {status} {got:?}"
        );
        let view = &got["billingView"];

        assert_eq!(
            view["arn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the stored ARN, got {:?}",
            view["arn"]
        );
        assert_eq!(
            view["name"].as_str(),
            Some(row.name.as_str()),
            "case {case:?}: expected the stored name, got {:?}",
            view["name"]
        );
        assert_eq!(
            view["description"].as_str(),
            row.description.as_deref(),
            "case {case:?}: expected the stored description, got {:?}",
            view["description"]
        );
        assert_eq!(
            view["billingViewType"].as_str(),
            Some("CUSTOM"),
            "case {case:?}: a view created through CreateBillingView is CUSTOM, got {:?}",
            view["billingViewType"]
        );
        assert_eq!(
            view["ownerAccountId"].as_str(),
            Some(ACCOUNT_ID),
            "case {case:?}: expected the caller account as owner, got {:?}",
            view["ownerAccountId"]
        );
        assert_eq!(
            view["sourceViewCount"].as_u64(),
            Some(row.source_views.len() as u64),
            "case {case:?}: sourceViewCount must count the stored source views, got {:?}",
            view["sourceViewCount"]
        );
        assert_eq!(
            view["derivedViewCount"].as_u64(),
            Some(0),
            "case {case:?}: no view sources this one yet, got {:?}",
            view["derivedViewCount"]
        );
        assert_eq!(
            view["healthStatus"]["statusCode"].as_str(),
            Some("HEALTHY"),
            "case {case:?}: expected a HEALTHY status code, got {:?}",
            view["healthStatus"]
        );
        assert_eq!(
            view["createdAt"], created["createdAt"],
            "case {case:?}: GetBillingView must report the createdAt the create returned, got {:?}",
            view["createdAt"]
        );
        assert_eq!(
            view.get("updatedAt"),
            None,
            "case {case:?}: a view that was never updated has no updatedAt, got {:?}",
            view.get("updatedAt")
        );
        assert_eq!(
            view["dataFilterExpression"],
            row.data_filter_expression.clone().unwrap_or(Value::Null),
            "case {case:?}: expected the stored data filter expression, got {:?}",
            view["dataFilterExpression"]
        );
    }
}

#[tokio::test]
async fn every_created_view_appears_in_the_list_and_derived_counts_follow_the_sources() {
    let router = emulator();
    let rows = view_rows();
    let mut created_arns = Vec::new();

    for (index, row) in rows.iter().enumerate() {
        let (status, created) =
            call(&router, "CreateBillingView", &row.create_body(&format!("tok-{index}"))).await;
        assert_eq!(status, 200, "case {:?}: create failed: {created:?}", row.case);
        created_arns.push(created["arn"].as_str().unwrap_or_default().to_string());
    }

    let (status, listed) = call(&router, "ListBillingViews", &json!({})).await;
    assert_eq!(status, 200, "ListBillingViews must answer 200, got {status} {listed:?}");
    let arns: Vec<&str> = listed["billingViews"]
        .as_array()
        .expect("billingViews is a list")
        .iter()
        .map(|view| view["arn"].as_str().unwrap_or_default())
        .collect();

    assert!(
        arns.contains(&PRIMARY_ARN),
        "case \"default PRIMARY view\": every account has one; listed {arns:?}"
    );
    for (row, arn) in rows.iter().zip(&created_arns) {
        assert!(
            arns.contains(&arn.as_str()),
            "case {:?}: a created view must appear in ListBillingViews; listed {arns:?}",
            row.case
        );
    }
    assert_eq!(
        arns.len(),
        created_arns.len() + 1,
        "case \"no filters\": expected the PRIMARY view plus every created one, got {arns:?}"
    );
    let mut sorted = arns.clone();
    sorted.sort_unstable();
    assert_eq!(
        arns, sorted,
        "case \"no filters\": views are listed in ARN order, got {arns:?}"
    );

    // Three of the four rows name the PRIMARY view as a source, and the PRIMARY
    // view is the one the service manages: it reports no source/derived counts
    // at all, so the rule that a count follows the stored records is asserted on
    // a view a caller created and this test then makes a source of another.
    let (_, primary) = call(&router, "GetBillingView", &json!({ "arn": PRIMARY_ARN })).await;
    for member in ["createdAt", "sourceViewCount", "derivedViewCount"] {
        assert!(
            primary["billingView"].get(member).is_none(),
            "case \"service-managed PRIMARY\": expected no {member:?}, got {:?}",
            primary["billingView"]
        );
    }
    let source = created_arns[0].clone();
    let (status, derived) = call(
        &router,
        "CreateBillingView",
        &json!({
            "name": "derives from a custom view",
            "sourceViews": [source.clone()],
            "clientToken": "tok-derived",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"derivedViewCount\": the harness create must answer 200, got {status} {derived:?}"
    );
    let (_, sourced) = call(&router, "GetBillingView", &json!({ "arn": source })).await;
    assert_eq!(
        sourced["billingView"]["derivedViewCount"].as_u64(),
        Some(1),
        "case \"derivedViewCount\": expected the one view that sources it, got {:?}",
        sourced["billingView"]["derivedViewCount"]
    );
}

#[tokio::test]
async fn the_list_honours_each_filter() {
    let router = emulator();
    let rows = view_rows();
    let mut created_arns = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let (status, created) =
            call(&router, "CreateBillingView", &row.create_body(&format!("f-{index}"))).await;
        assert_eq!(status, 200, "case {:?}: create failed: {created:?}", row.case);
        created_arns.push(created["arn"].as_str().unwrap_or_default().to_string());
    }

    // (case, request, the ARNs the filter must select)
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "arns selects exactly the listed views",
            json!({ "arns": [created_arns[0], created_arns[2]] }),
            vec![created_arns[0].clone(), created_arns[2].clone()],
        ),
        (
            "arns with one entry",
            json!({ "arns": [created_arns[1]] }),
            vec![created_arns[1].clone()],
        ),
        (
            "an empty arns list filters nothing out",
            json!({ "arns": [] }),
            {
                let mut all = created_arns.clone();
                all.push(PRIMARY_ARN.to_string());
                all.sort();
                all
            },
        ),
        (
            "billingViewTypes selects PRIMARY",
            json!({ "billingViewTypes": ["PRIMARY"] }),
            vec![PRIMARY_ARN.to_string()],
        ),
        (
            "billingViewTypes selects CUSTOM",
            json!({ "billingViewTypes": ["CUSTOM"] }),
            {
                let mut all = created_arns.clone();
                all.sort();
                all
            },
        ),
        (
            "billingViewTypes selects a type no view carries",
            json!({ "billingViewTypes": ["BILLING_GROUP"] }),
            vec![],
        ),
        (
            "names STARTS_WITH matches the common prefix",
            json!({ "names": [{ "searchOption": "STARTS_WITH", "searchValue": "demo" }] }),
            {
                let mut selected: Vec<String> = rows
                    .iter()
                    .zip(&created_arns)
                    .filter(|(row, _)| row.name.starts_with("demo"))
                    .map(|(_, arn)| arn.clone())
                    .collect();
                selected.sort();
                selected
            },
        ),
        (
            "names STARTS_WITH matches the whole name",
            json!({ "names": [{ "searchOption": "STARTS_WITH", "searchValue": "Primary View" }] }),
            vec![PRIMARY_ARN.to_string()],
        ),
        (
            "names STARTS_WITH matching nothing",
            json!({ "names": [{ "searchOption": "STARTS_WITH", "searchValue": "nothing" }] }),
            vec![],
        ),
        (
            "ownerAccountId selects the caller's views",
            json!({ "ownerAccountId": ACCOUNT_ID }),
            {
                let mut all = created_arns.clone();
                all.push(PRIMARY_ARN.to_string());
                all.sort();
                all
            },
        ),
        (
            "ownerAccountId of another account selects nothing",
            json!({ "ownerAccountId": "123456789012" }),
            vec![],
        ),
        (
            // "only billing views that use the specified account as a source":
            // every row of `view_rows` names a source view of the caller's own
            // account, so the caller's account selects them all — and the
            // PRIMARY view, which draws from nothing, none of them.
            // `source_account_id.rs` is the suite that spans this filter.
            "sourceAccountId selects the views drawing on that account's views",
            json!({ "sourceAccountId": ACCOUNT_ID }),
            {
                let mut all = created_arns.clone();
                all.sort();
                all
            },
        ),
        (
            "sourceAccountId of an account no view draws on selects nothing",
            json!({ "sourceAccountId": "123456789012" }),
            vec![],
        ),
        (
            // The modeled `activeTimeRange` doc: the account's PRIMARY view "is
            // always listed", so a window ending before any view existed still
            // lists it and nothing else.
            "activeTimeRange ending before every created view",
            json!({ "activeTimeRange": { "activeAfterInclusive": 0, "activeBeforeInclusive": 1 } }),
            vec![PRIMARY_ARN.to_string()],
        ),
    ];

    for (case, request, expected) in cases {
        let (status, listed) = call(&router, "ListBillingViews", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from ListBillingViews, got {status} {listed:?}"
        );
        let got: Vec<String> = listed["billingViews"]
            .as_array()
            .expect("billingViews is a list")
            .iter()
            .map(|view| view["arn"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            got, expected,
            "case {case:?}: expected {expected:?}, got {got:?}"
        );
    }
}

#[tokio::test]
async fn pagination_returns_every_view_exactly_once_for_every_page_size() {
    let router = emulator();
    for (index, row) in view_rows().iter().enumerate() {
        let (status, created) =
            call(&router, "CreateBillingView", &row.create_body(&format!("p-{index}"))).await;
        assert_eq!(status, 200, "case {:?}: create failed: {created:?}", row.case);
    }

    let (_, all) = call(&router, "ListBillingViews", &json!({})).await;
    let expected: Vec<String> = all["billingViews"]
        .as_array()
        .expect("billingViews is a list")
        .iter()
        .map(|view| view["arn"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        expected.len(),
        5,
        "case \"unpaginated\": four created views plus the PRIMARY one, got {expected:?}"
    );

    // Every page size from 1 to one past the result count, plus the modeled
    // `maxResults` maximum: the pages must concatenate to the unpaginated
    // answer, in the same order, with no repeats.
    for page_size in [1_usize, 2, 3, 4, 5, 6, 100] {
        let case = format!("maxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "maxResults": page_size });
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let (status, listed) = call(&router, "ListBillingViews", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from ListBillingViews, got {status} {listed:?}"
            );
            let page: Vec<String> = listed["billingViews"]
                .as_array()
                .expect("billingViews is a list")
                .iter()
                .map(|view| view["arn"].as_str().unwrap_or_default().to_string())
                .collect();
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must not exceed maxResults, got {page:?}"
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: pagination must terminate, saw {pages} pages of {seen:?}"
            );
            match listed["nextToken"].as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: the pages must concatenate to the full list, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_replayed_create_with_the_same_client_token_creates_no_second_view() {
    let router = emulator();
    let row = &view_rows()[0];
    let body = row.create_body("replayed-token");

    let (status, first) = call(&router, "CreateBillingView", &body).await;
    assert_eq!(status, 200, "case \"first create\": got {status} {first:?}");

    for attempt in 1..=3 {
        let case = format!("replay {attempt}");
        let (status, again) = call(&router, "CreateBillingView", &body).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {again:?}");
        assert_eq!(
            again["arn"], first["arn"],
            "case {case:?}: an idempotent replay returns the first ARN, got {:?}",
            again["arn"]
        );
        assert_eq!(
            again["createdAt"], first["createdAt"],
            "case {case:?}: an idempotent replay returns the first createdAt, got {:?}",
            again["createdAt"]
        );
    }

    let (_, listed) = call(&router, "ListBillingViews", &json!({})).await;
    let views = listed["billingViews"].as_array().expect("billingViews is a list");
    assert_eq!(
        views.len(),
        2,
        "case \"after three replays\": only the PRIMARY view and one created view exist, got {views:?}"
    );

    // A different token with the same attributes is a different view.
    let (status, other) = call(&router, "CreateBillingView", &row.create_body("other-token")).await;
    assert_eq!(status, 200, "case \"different token\": got {status} {other:?}");
    assert_ne!(
        other["arn"], first["arn"],
        "case \"different token\": a new token mints a new view, got {:?}",
        other["arn"]
    );
}

#[tokio::test]
async fn update_changes_only_the_members_the_request_carries() {
    let router = emulator();

    // (case, update request members, expected name, expected description)
    let cases: Vec<(&str, Value, &str, Option<&str>)> = vec![
        ("name only", json!({ "name": "renamed" }), "renamed", Some("first view")),
        (
            "description only",
            json!({ "description": "second description" }),
            "demo-view",
            Some("second description"),
        ),
        (
            "name and description",
            json!({ "name": "both changed", "description": "both too" }),
            "both changed",
            Some("both too"),
        ),
        ("no member at all", json!({}), "demo-view", Some("first view")),
        (
            "data filter expression only",
            json!({ "dataFilterExpression": { "dimensions": { "key": "LINKED_ACCOUNT", "values": ["000000000000"] } } }),
            "demo-view",
            Some("first view"),
        ),
    ];

    for (index, (case, members, expected_name, expected_description)) in
        cases.into_iter().enumerate()
    {
        let row = &view_rows()[0];
        let (status, created) =
            call(&router, "CreateBillingView", &row.create_body(&format!("u-{index}"))).await;
        assert_eq!(status, 200, "case {case:?}: create failed: {created:?}");
        let arn = created["arn"].as_str().unwrap_or_default().to_string();

        let mut request = members;
        request["arn"] = json!(arn);
        let (status, updated) = call(&router, "UpdateBillingView", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateBillingView to answer 200, got {status} {updated:?}"
        );
        assert_eq!(
            updated["arn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the updated view's ARN, got {:?}",
            updated["arn"]
        );
        assert!(
            updated["updatedAt"].is_number(),
            "case {case:?}: expected an updatedAt timestamp, got {:?}",
            updated["updatedAt"]
        );

        let (status, got) = call(&router, "GetBillingView", &json!({ "arn": arn })).await;
        assert_eq!(status, 200, "case {case:?}: get after update failed: {got:?}");
        let view = &got["billingView"];
        assert_eq!(
            view["name"].as_str(),
            Some(expected_name),
            "case {case:?}: expected name {expected_name:?}, got {:?}",
            view["name"]
        );
        assert_eq!(
            view["description"].as_str(),
            expected_description,
            "case {case:?}: expected description {expected_description:?}, got {:?}",
            view["description"]
        );
        assert_eq!(
            view["updatedAt"], updated["updatedAt"],
            "case {case:?}: the stored updatedAt is the one the update returned, got {:?}",
            view["updatedAt"]
        );
        assert_eq!(
            view["createdAt"], created["createdAt"],
            "case {case:?}: an update must not move createdAt, got {:?}",
            view["createdAt"]
        );
        assert_eq!(
            view["sourceViewCount"].as_u64(),
            Some(row.source_views.len() as u64),
            "case {case:?}: an update must not change the source views, got {:?}",
            view["sourceViewCount"]
        );
    }
}

#[tokio::test]
async fn a_deleted_view_is_gone_for_the_next_caller() {
    let router = emulator();
    let row = &view_rows()[1];
    let (status, created) = call(&router, "CreateBillingView", &row.create_body("d-0")).await;
    assert_eq!(status, 200, "case \"create\": got {status} {created:?}");
    let arn = created["arn"].as_str().unwrap_or_default().to_string();

    let (status, deleted) = call(&router, "DeleteBillingView", &json!({ "arn": arn })).await;
    assert_eq!(status, 200, "case \"delete\": got {status} {deleted:?}");
    assert_eq!(
        deleted["arn"].as_str(),
        Some(arn.as_str()),
        "case \"delete\": expected the deleted ARN, got {:?}",
        deleted["arn"]
    );

    let (status, got) = call(&router, "GetBillingView", &json!({ "arn": arn })).await;
    assert_eq!(
        (status, error_code(&got).as_str()),
        (404, "ResourceNotFoundException"),
        "case \"get after delete\": got {status} {got:?}"
    );

    let (_, listed) = call(&router, "ListBillingViews", &json!({})).await;
    let arns: Vec<&str> = listed["billingViews"]
        .as_array()
        .expect("billingViews is a list")
        .iter()
        .map(|view| view["arn"].as_str().unwrap_or_default())
        .collect();
    assert!(
        !arns.contains(&arn.as_str()),
        "case \"list after delete\": the deleted view must be gone, listed {arns:?}"
    );

    // `DeleteBillingView` models no ResourceNotFoundException, so a second
    // delete is refused as invalid input.
    let (status, again) = call(&router, "DeleteBillingView", &json!({ "arn": arn })).await;
    assert_eq!(
        (status, error_code(&again).as_str()),
        (400, "ValidationException"),
        "case \"delete twice\": got {status} {again:?}"
    );
}

#[tokio::test]
async fn deleting_a_source_view_needs_force() {
    let router = emulator();
    // A CUSTOM view, and a second CUSTOM view that draws from it: the first now
    // has a derived view, so deleting it needs `force`. (The account's PRIMARY
    // view cannot stand in for the source here — no caller may delete it.)
    let (status, source) =
        call(&router, "CreateBillingView", &view_rows()[0].create_body("c-0")).await;
    assert_eq!(status, 200, "case \"create source\": got {status} {source:?}");
    let source_arn = source["arn"].as_str().unwrap_or_default().to_string();

    let (status, derived) = call(
        &router,
        "CreateBillingView",
        &json!({ "name": "derived-view", "sourceViews": [source_arn], "clientToken": "c-1" }),
    )
    .await;
    assert_eq!(status, 200, "case \"create derived\": got {status} {derived:?}");

    let (status, refused) =
        call(&router, "DeleteBillingView", &json!({ "arn": source_arn })).await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (409, "ConflictException"),
        "case \"force absent\": a view another view sources is a conflict, got {status} {refused:?}"
    );
    let (status, still_there) =
        call(&router, "GetBillingView", &json!({ "arn": source_arn })).await;
    assert_eq!(
        status, 200,
        "case \"force absent\": the refused delete must not remove the view, got {still_there:?}"
    );

    let (status, refused) =
        call(&router, "DeleteBillingView", &json!({ "arn": source_arn, "force": false })).await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (409, "ConflictException"),
        "case \"force=false\": got {status} {refused:?}"
    );

    let (status, forced) =
        call(&router, "DeleteBillingView", &json!({ "arn": source_arn, "force": true })).await;
    assert_eq!(
        status, 200,
        "case \"force=true\": expected the delete to go through, got {status} {forced:?}"
    );
    let (status, gone) = call(&router, "GetBillingView", &json!({ "arn": source_arn })).await;
    assert_eq!(
        (status, error_code(&gone).as_str()),
        (404, "ResourceNotFoundException"),
        "case \"force=true\": the view must be gone, got {status} {gone:?}"
    );
}

#[tokio::test]
async fn an_arn_no_view_was_stored_under_is_refused_with_the_modeled_error() {
    let router = emulator();

    // (case, operation, request, expected status, expected error code)
    let cases: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "get an absent view",
            "GetBillingView",
            json!({ "arn": ABSENT_ARN }),
            404,
            "ResourceNotFoundException",
        ),
        (
            // Only a CUSTOM view may be edited, and eligibility is decided
            // before existence, so an absent ARN is the same 400 as the
            // account's PRIMARY view.
            "update an absent view",
            "UpdateBillingView",
            json!({ "arn": ABSENT_ARN, "name": "whatever" }),
            400,
            "ValidationException",
        ),
        (
            "delete an absent view",
            "DeleteBillingView",
            json!({ "arn": ABSENT_ARN }),
            400,
            "ValidationException",
        ),
        (
            // The account boundary is enforced before any existence lookup, so
            // a foreign-account ARN never reveals whether a view exists there.
            "get a view in another account",
            "GetBillingView",
            json!({ "arn": "arn:aws:billing::123456789012:billingview/primary" }),
            403,
            "AccessDeniedException",
        ),
    ];

    for (case, operation, request, expected_status, expected_code) in cases {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {body:?}"
        );
    }
}

#[tokio::test]
async fn a_request_outside_a_modeled_constraint_is_refused_and_stores_nothing() {
    let router = emulator();

    // Each row breaks exactly one modeled constraint; the bounds come from the
    // model (name len 1..128 pattern ^[ a-zA-Z0-9_\+=\.\-@]+$, sourceViews
    // len 1..10, maxResults range 1..100, the BillingViewArn pattern).
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "name outside its pattern",
            "CreateBillingView",
            json!({ "name": "bad/name!", "sourceViews": [PRIMARY_ARN] }),
        ),
        (
            "name over its length maximum",
            "CreateBillingView",
            json!({ "name": "n".repeat(129), "sourceViews": [PRIMARY_ARN] }),
        ),
        (
            "name under its length minimum",
            "CreateBillingView",
            json!({ "name": "", "sourceViews": [PRIMARY_ARN] }),
        ),
        (
            "sourceViews under its length minimum",
            "CreateBillingView",
            json!({ "name": "fine", "sourceViews": [] }),
        ),
        (
            "sourceViews over its length maximum",
            "CreateBillingView",
            json!({
                "name": "fine",
                "sourceViews": (0..11)
                    .map(|i| format!("arn:aws:billing::{ACCOUNT_ID}:billingview/source-{i}"))
                    .collect::<Vec<_>>(),
            }),
        ),
        (
            "a source view outside the ARN pattern",
            "CreateBillingView",
            json!({ "name": "fine", "sourceViews": ["not-an-arn"] }),
        ),
        (
            "maxResults below its range minimum",
            "ListBillingViews",
            json!({ "maxResults": 0 }),
        ),
        (
            "maxResults above its range maximum",
            "ListBillingViews",
            json!({ "maxResults": 101 }),
        ),
        (
            "nextToken this service never issued",
            "ListBillingViews",
            json!({ "nextToken": "zzzz" }),
        ),
        (
            "arn outside the ARN pattern",
            "GetBillingView",
            json!({ "arn": "arn:aws:billing::000000000000:not-a-billing-view" }),
        ),
        (
            "activeTimeRange whose start is after its end",
            "ListBillingViews",
            json!({ "activeTimeRange": { "activeAfterInclusive": 200, "activeBeforeInclusive": 100 } }),
        ),
    ];

    for (case, operation, request) in cases {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
    }

    // Nothing above was stored: only the account's default PRIMARY view exists.
    let (_, listed) = call(&router, "ListBillingViews", &json!({})).await;
    let arns: Vec<&str> = listed["billingViews"]
        .as_array()
        .expect("billingViews is a list")
        .iter()
        .map(|view| view["arn"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        arns,
        vec![PRIMARY_ARN],
        "case \"after every refusal\": a refused create must store nothing, listed {arns:?}"
    );
}

#[tokio::test]
async fn the_view_store_is_bounded_and_a_delete_reclaims_its_tokens() {
    use service_com_amazonaws_billing_awsbilling::state::{
        ComAmazonawsBillingAwsbillingState, MAX_BILLING_VIEWS,
    };

    let (router, engine) = emulator_with_engine();

    // A create/delete loop must reclaim what it allocated: the delete drops the
    // idempotency entry its create wrote, so the token store cannot outgrow the
    // view store however long a client loops.
    for round in 0..3 {
        let case = format!("create/delete round {round}");
        let body = json!({
            "name": format!("loop-view-{round}"),
            "sourceViews": [PRIMARY_ARN],
            "clientToken": format!("loop-{round}"),
        });
        let (status, created) = call(&router, "CreateBillingView", &body).await;
        assert_eq!(status, 200, "case {case:?}: create got {status} {created:?}");
        let arn = created["arn"].as_str().unwrap_or_default().to_string();
        {
            let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
            let state = state.lock();
            assert_eq!(
                state.idempotency_tokens.len(),
                1,
                "case {case:?}: the create writes exactly one token entry, found {:?}",
                state.idempotency_tokens
            );
        }

        let (status, deleted) = call(&router, "DeleteBillingView", &json!({ "arn": arn })).await;
        assert_eq!(status, 200, "case {case:?}: delete got {status} {deleted:?}");
        let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
        let state = state.lock();
        assert!(
            state.idempotency_tokens.is_empty(),
            "case {case:?}: the delete must reclaim its token, {:?} are left",
            state.idempotency_tokens
        );
        assert!(
            state.idempotency_tokens.len() <= state.billing_views.len(),
            "case {case:?}: the token store must never outgrow the view store, \
             {} tokens for {} views",
            state.idempotency_tokens.len(),
            state.billing_views.len()
        );
    }

    // The view store carries an explicit ceiling. The bound is about resident
    // records, so fill it to the cap in state and the next create must be
    // refused with the modeled quota error instead of growing it further.
    let filler_arn = {
        let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
        let mut state = state.lock();
        let template = state
            .billing_views
            .get(PRIMARY_ARN)
            .cloned()
            .expect("the account's PRIMARY view");
        let mut last = String::new();
        while state.billing_views.len() < MAX_BILLING_VIEWS {
            let arn = state.mint_view_arn();
            let mut filler = template.clone();
            filler.arn = arn.clone();
            filler.name = format!("filler-{}", state.billing_views.len());
            // The fillers stand in for views a caller created, so they are
            // CUSTOM: only a CUSTOM view may be deleted, and the last step of
            // this test frees one to prove the cap is a bound, not a latch.
            filler.billing_view_type = "CUSTOM".to_string();
            state.billing_views.insert(arn.clone(), filler);
            last = arn;
        }
        last
    };

    let (status, refused) = call(
        &router,
        "CreateBillingView",
        &json!({ "name": "one too many", "sourceViews": [PRIMARY_ARN] }),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (402, "ServiceQuotaExceededException"),
        "case \"at the cap\": expected the modeled quota error, got {status} {refused:?}"
    );

    // Freeing a record lets a create through again: the cap is a bound, not a latch.
    let (status, deleted) =
        call(&router, "DeleteBillingView", &json!({ "arn": filler_arn })).await;
    assert_eq!(
        status, 200,
        "case \"free one record\": got {status} {deleted:?}"
    );
    let (status, created) = call(
        &router,
        "CreateBillingView",
        &json!({ "name": "room again", "sourceViews": [PRIMARY_ARN] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"after freeing room\": got {status} {created:?}"
    );
}

#[tokio::test]
async fn reads_do_not_change_what_a_later_caller_sees() {
    let router = emulator();
    let (status, created) =
        call(&router, "CreateBillingView", &view_rows()[0].create_body("r-0")).await;
    assert_eq!(status, 200, "case \"create\": got {status} {created:?}");
    let arn = created["arn"].as_str().unwrap_or_default().to_string();

    let (_, first_get) = call(&router, "GetBillingView", &json!({ "arn": arn })).await;
    let (_, first_list) = call(&router, "ListBillingViews", &json!({})).await;
    for attempt in 1..=3 {
        let case = format!("repeat {attempt}");
        let (_, got) = call(&router, "GetBillingView", &json!({ "arn": arn })).await;
        assert_eq!(
            got, first_get,
            "case {case:?}: GetBillingView is a read, so it must answer identically, got {got:?}"
        );
        let (_, listed) = call(&router, "ListBillingViews", &json!({})).await;
        assert_eq!(
            listed, first_list,
            "case {case:?}: ListBillingViews is a read, so it must answer identically, got {listed:?}"
        );
    }
}
