// SPDX-License-Identifier: CC0-1.0

//! The Partner Central Selling CustomerAccount resource through the ROUTER: the one
//! path a client walks — create, wait for the asynchronous matching outcome, find the
//! account with a search, count it with the statistics call, delete it — where every
//! service is mounted together on the one shared engine, because the router is what a
//! client actually talks to.
//!
//! The rule tables — every create payload, every filter and sort form, paging, the
//! refusals, the idempotency token, the duplicate-customer outcome and the store's
//! ceiling — are the owning crate's own suite,
//! `services/service-com-amazonaws-partnercentralselling-awspartnercentralselling/tests/all/opportunity_lifecycle.rs`.
//!
//! The matching step is waited for by POLLING the account until it leaves `PENDING`,
//! never by sleeping for its span: the span is the service's, and a test that assumed
//! it would fail on a loaded host.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The endpoint host; its first label is the route key this service claims.
const HOST: &str = "partnercentral-selling.us-east-1.amazonaws.com";

/// The region every call signs for, as the SigV4 credential scope the router resolves
/// the service from.
const REGION: &str = "us-east-1";

/// The catalog the walk runs in. "Valid values: AWS: Production. Sandbox: Testing
/// environments."
const CATALOG: &str = "AWS";

/// How long the poll waits for a matching outcome before giving up. Generous on
/// purpose: it bounds a hang, it does not measure the service's span.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How often the poll looks again.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The emulator under test: this service mounted on the router over one shared engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // This suite exercises operation behaviour, not SigV4: the signature a real client
    // sends is verified by the emulator, never by a service.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_partnercentralselling_awspartnercentralselling::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Send one `awsJson1_0` call — `POST /` with an `X-Amz-Target` header — and read the
/// status and parsed body back.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/partnercentral-selling/aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{HOST}/"))
        .header("host", HOST)
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("AWSPartnerCentralSelling.{operation}"),
        )
        .header("authorization", authorization)
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the test body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One call that must have succeeded, with the step named for the assertion.
async fn ok(router: &MultiServiceRouter, step: &str, operation: &str, body: &Value) -> Value {
    let (status, answer) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "step {step:?}: {operation} expected HTTP 200, got {status} with body {answer}"
    );
    answer
}

/// A string member of a body.
fn member(step: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("step {step:?}: expected a string `{name}`, got {body}"))
        .to_string()
}

/// The whole path, through the assembled router: an account created on it is matched,
/// found by a search over the matching outcome, counted by the statistics call, and
/// gone from both after it is deleted.
#[tokio::test]
async fn a_customer_account_lifecycle_runs_on_the_assembled_router() {
    let router = router();

    let created = ok(
        &router,
        "create",
        "CreateCustomerAccount",
        &json!({
            "Catalog": CATALOG,
            "Account": { "CompanyName": "Router Walk Inc", "Duns": "456456456" },
            "PartnerAccountIdentifier": "crm-router-1"
        }),
    )
    .await;
    let id = member("create", &created, "Id");
    assert_eq!(
        member("create", &created, "Status"),
        "PENDING",
        "step \"create\": a new account starts as PENDING, got {created}"
    );

    // Matching runs asynchronously, so the outcome is waited for by looking again.
    let deadline = Instant::now() + SETTLE_DEADLINE;
    let settled = loop {
        let read = ok(
            &router,
            "read",
            "GetCustomerAccount",
            &json!({ "Catalog": CATALOG, "Identifier": id }),
        )
        .await;
        if member("read", &read, "EnrichmentStatus") != "PENDING" {
            break read;
        }
        assert!(
            Instant::now() < deadline,
            "step \"read\": the account never left PENDING; last read was {read}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    let status = member("read", &settled, "EnrichmentStatus");
    assert_eq!(
        member("read", &settled, "Id"),
        id,
        "step \"read\": the read answers the account created, got {settled}"
    );
    assert_eq!(
        settled
            .get("Account")
            .and_then(|account| account.get("CompanyName"))
            .and_then(Value::as_str),
        Some("Router Walk Inc"),
        "step \"read\": the read reports the customer details the create carried, got \
         {settled}"
    );

    // The search selects on the stored outcome, so the account is found by the status
    // matching gave it and not by the one it was created with.
    let found = ok(
        &router,
        "search",
        "SearchCustomerAccounts",
        &json!({
            "Catalog": CATALOG,
            "Filter": { "EqualsTo": { "Key": "ENRICHMENT_STATUS", "Value": { "StringValue": status } } }
        }),
    )
    .await;
    assert_eq!(
        found
            .get("CustomerAccountSummaries")
            .and_then(Value::as_array)
            .map(|summaries| summaries
                .iter()
                .filter_map(|summary| summary.get("Id").and_then(Value::as_str))
                .collect::<Vec<_>>()),
        Some(vec![id.as_str()]),
        "step \"search\": the account is found by its settled status, got {found}"
    );

    let counted = ok(
        &router,
        "statistics",
        "GetCustomerAccountStatistics",
        &json!({
            "Catalog": CATALOG,
            "Aggregations": [{ "GroupBy": ["ENRICHMENT_STATUS"] }]
        }),
    )
    .await;
    assert_eq!(
        counted.get("TotalCount").and_then(Value::as_i64),
        Some(1),
        "step \"statistics\": the one stored account is counted, got {counted}"
    );
    assert_eq!(
        counted
            .get("Aggregations")
            .and_then(Value::as_array)
            .and_then(|results| results.first())
            .and_then(|result| result.get("Buckets"))
            .and_then(Value::as_array)
            .map(|buckets| buckets
                .iter()
                .filter_map(|bucket| {
                    let value = bucket
                        .get("Values")
                        .and_then(Value::as_array)
                        .and_then(|values| values.first())
                        .and_then(Value::as_str)?;
                    let count = bucket.get("Count").and_then(Value::as_i64)?;
                    Some((value.to_string(), count))
                })
                .collect::<Vec<_>>()),
        Some(vec![(status.clone(), 1)]),
        "step \"statistics\": the account is grouped under the status it carries, got \
         {counted}"
    );

    ok(
        &router,
        "delete",
        "DeleteCustomerAccount",
        &json!({ "Catalog": CATALOG, "Identifier": id }),
    )
    .await;

    let (read_status, read_body) = call(
        &router,
        "GetCustomerAccount",
        &json!({ "Catalog": CATALOG, "Identifier": id }),
    )
    .await;
    assert_eq!(
        read_status, 404,
        "step \"delete\": the deleted account is gone, got HTTP {read_status} with body \
         {read_body}"
    );
    let emptied = ok(
        &router,
        "delete",
        "GetCustomerAccountStatistics",
        &json!({ "Catalog": CATALOG }),
    )
    .await;
    assert_eq!(
        emptied.get("TotalCount").and_then(Value::as_i64),
        Some(0),
        "step \"delete\": the deleted account is counted nowhere, got {emptied}"
    );
}
