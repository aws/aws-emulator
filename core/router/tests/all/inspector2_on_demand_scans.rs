// SPDX-License-Identifier: CC0-1.0

//! Amazon Inspector's ON-DEMAND SCAN routes on the ASSEMBLED emulator, beside the other
//! service of the Inspector family that shares its path space.
//!
//! RULE under test: **a request reaches the service its SigV4 credential scope names, and
//! only that service** — so the three routes Amazon Inspector's on-demand scans are served
//! at (`POST /ondemand/scan`, `/ondemand/scan/status`, `/ondemand/scans`) are answered by
//! `inspector2` when it is mounted beside `inspector-scan`, and a request carrying
//! `inspector-scan`'s own signing name is not answered from `inspector2`'s store, even
//! though it names a path only `inspector2` serves.
//!
//! Why it lives beside the router's suites rather than in one service crate: the invariant
//! is about which of two services mounted in ONE router claims a path, which no
//! single-service test target can observe. The behaviour of the operations themselves —
//! what a started scan records, what a read reports and how the list pages — is owned by
//! `services/service-com-amazonaws-inspector2-inspector2/tests/all/filters_and_enablement.rs`.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// `StartOnDemandScan`'s route.
/// Source: `smithy_query(query="operation", operation="StartOnDemandScan")` — `POST
/// /ondemand/scan`, success status 202.
const START_URI: &str = "/ondemand/scan";

/// `GetOnDemandScanStatus`'s route.
/// Source: `smithy_query(query="operation", operation="GetOnDemandScanStatus")` — `POST
/// /ondemand/scan/status`.
const STATUS_URI: &str = "/ondemand/scan/status";

/// `ListOnDemandScans`'s route.
/// Source: `smithy_query(query="operation", operation="ListOnDemandScans")` — `POST
/// /ondemand/scans`.
const LIST_URI: &str = "/ondemand/scans";

/// Amazon Inspector's SigV4 signing name.
/// Source: `smithy_query(query="service_info")` of its model — `sigv4Name` `inspector2`.
const INSPECTOR2: &str = "inspector2";

/// The signing name of the sibling service mounted beside it, whose own model serves a
/// single operation at `POST /scan/sbom`.
/// Source: `smithy_query(query="service_info")` of
/// `services/service-com-amazonaws-inspectorscan-inspectorscan` — `sigv4Name`
/// `inspector-scan`.
const INSPECTOR_SCAN: &str = "inspector-scan";

/// The one `OnDemandResourceType` token.
/// Source: `smithy_query(query="shape", shape="OnDemandResourceType")` — `AMI`.
const RESOURCE_TYPE: &str = "AMI";

/// An instance id inside the modelled `ResourceId` (`@length` 10..1024, and a `@pattern`
/// admitting `i-` followed by 8 or 17 characters).
const RESOURCE_ID: &str = "i-0123456789abcdef0";

/// One router holding BOTH services of the family over one engine, mounted the way the
/// assembled emulator mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_inspector2_inspector2::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_inspectorscan_inspectorscan::EmulatedService::new(engine),
    );
    router
}

/// POST a restJson1 request whose credential scope names `signing_name`, which is the
/// signal the router routes on.
async fn post(
    router: &MultiServiceRouter,
    signing_name: &str,
    uri: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("http://localhost{uri}"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=TESTKEY/20240101/us-east-1/{signing_name}/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("a serializable request body"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// A `StartOnDemandScan` body.
fn start_body() -> Value {
    json!({ "resourceId": RESOURCE_ID, "resourceType": RESOURCE_TYPE })
}

/// A body for the two reads, which name the resource and, for the status read, the scan.
fn read_body(scan_id: Option<&str>) -> Value {
    let mut body = json!({ "resourceId": RESOURCE_ID, "resourceType": RESOURCE_TYPE });
    if let Some(scan_id) = scan_id {
        body["scanId"] = json!(scan_id);
    }
    body
}

#[tokio::test]
async fn the_on_demand_scan_routes_are_claimed_by_inspector2_alone() {
    let router = emulator();

    // Signed for Amazon Inspector: the route is served and the scan is recorded.
    let (status, started) = post(&router, INSPECTOR2, START_URI, &start_body()).await;
    assert_eq!(
        status, 202,
        "StartOnDemandScan signed for {INSPECTOR2}: expected 202, got {status} / {started}"
    );
    let scan_id = started["scanId"]
        .as_str()
        .unwrap_or_else(|| panic!("no scanId in {started}"))
        .to_string();

    let (status, read) = post(&router, INSPECTOR2, STATUS_URI, &read_body(Some(&scan_id))).await;
    assert_eq!(
        status, 200,
        "GetOnDemandScanStatus signed for {INSPECTOR2}: expected 200, got {status} / {read}"
    );
    assert_eq!(
        read["scanId"],
        json!(scan_id),
        "expected the scan the start recorded, got {read}"
    );

    let (status, listed) = post(&router, INSPECTOR2, LIST_URI, &read_body(None)).await;
    assert_eq!(
        status, 200,
        "ListOnDemandScans signed for {INSPECTOR2}: expected 200, got {status} / {listed}"
    );
    assert_eq!(
        listed["items"]
            .as_array()
            .map(|items| items
                .iter()
                .filter_map(|item| item["scanId"].as_str())
                .collect::<Vec<&str>>()),
        Some(vec![scan_id.as_str()]),
        "expected exactly the one recorded scan, got {listed}"
    );

    // Signed for the sibling service, which serves no such route: none of the three is
    // answered from Amazon Inspector's store, and nothing is recorded.
    for (route, body) in [
        (START_URI, start_body()),
        (STATUS_URI, read_body(Some(&scan_id))),
        (LIST_URI, read_body(None)),
    ] {
        let (status, body) = post(&router, INSPECTOR_SCAN, route, &body).await;
        assert!(
            !(200..300).contains(&status),
            "case {route}: a request scoped to {INSPECTOR_SCAN} must not be answered from \
             {INSPECTOR2}'s store, got {status} / {body}"
        );
    }

    let (status, after) = post(&router, INSPECTOR2, LIST_URI, &read_body(None)).await;
    assert_eq!(
        status, 200,
        "ListOnDemandScans after the unclaimed requests: expected 200, got {status} / {after}"
    );
    // The scans the store holds, not the status each reports: a scan's status follows the
    // clock, so comparing whole answers would compare instants rather than records.
    assert_eq!(
        after["items"]
            .as_array()
            .map(|items| items
                .iter()
                .filter_map(|item| item["scanId"].as_str())
                .collect::<Vec<&str>>()),
        Some(vec![scan_id.as_str()]),
        "a request no service claimed must record nothing, got {after}"
    );
}
