// SPDX-License-Identifier: CC0-1.0

//! Amazon Inspector's SCAN CONFIGURATION routes on the ASSEMBLED emulator, beside the
//! other service of the Inspector family that shares its path space.
//!
//! RULE under test: **a request reaches the service its SigV4 credential scope names, and
//! only that service** — so `POST /configuration/update` and `POST /configuration/get` are
//! answered by `inspector2` when it is mounted beside `inspector-scan`, the Amazon Machine
//! Image setting an update writes through the assembled router is the one a read there
//! reports, and a request carrying `inspector-scan`'s own signing name reaches neither
//! route.
//!
//! Why it lives beside the router's suites rather than in one service crate: the invariant
//! is about which of two services mounted in ONE router claims a path, which no
//! single-service test target can observe. The configuration behaviour itself — every
//! member's round trip, the span a change is applied over and a member account's
//! inheritance — is owned by
//! `services/service-com-amazonaws-inspector2-inspector2/tests/all/filters_and_enablement.rs`.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// `UpdateConfiguration`'s route.
/// Source: `smithy_query(query="operation", operation="UpdateConfiguration")` — `POST
/// /configuration/update`.
const UPDATE_URI: &str = "/configuration/update";

/// `GetConfiguration`'s route.
/// Source: `smithy_query(query="operation", operation="GetConfiguration")` — `POST
/// /configuration/get`.
const GET_URI: &str = "/configuration/get";

/// Amazon Inspector's SigV4 signing name.
/// Source: `smithy_query(query="service_info")` of its model — `sigv4Name` `inspector2`.
const INSPECTOR2: &str = "inspector2";

/// The signing name of the sibling service mounted beside it, whose own model serves a
/// single operation at `POST /scan/sbom`.
/// Source: `smithy_query(query="service_info")` of
/// `services/service-com-amazonaws-inspectorscan-inspectorscan` — `sigv4Name`
/// `inspector-scan`.
const INSPECTOR_SCAN: &str = "inspector-scan";

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

/// The update this suite writes: targeted AMI scanning, restricted to the images created
/// within a published window.
/// Source: `smithy_query(query="shape", shape="AmiConfiguration,AmiScanMode,AmiScanDuration")`.
fn ami_update() -> Value {
    json!({
        "amiConfiguration": {
            "createdWithin": "DAYS_60",
            "scanModeConfiguration": {
                "scanMode": "TARGET_SCANNING",
                "tags": [{ "key": "Fleet", "value": "assembled" }],
            },
        }
    })
}

#[tokio::test]
async fn the_scan_configuration_routes_are_claimed_by_inspector2_alone() {
    let router = emulator();

    // Signed for Amazon Inspector: the write is served, and the read reports what it wrote.
    let (status, written) = post(&router, INSPECTOR2, UPDATE_URI, &ami_update()).await;
    assert_eq!(
        status, 200,
        "UpdateConfiguration signed for {INSPECTOR2}: expected 200, got {status} / {written}"
    );

    let (status, read) = post(&router, INSPECTOR2, GET_URI, &json!({})).await;
    assert_eq!(
        status, 200,
        "GetConfiguration signed for {INSPECTOR2}: expected 200, got {status} / {read}"
    );
    let ami = &read["amiConfiguration"];
    assert_eq!(
        ami["createdWithin"],
        json!("DAYS_60"),
        "expected the window the update wrote, got {read}"
    );
    assert_eq!(
        ami["scanModeConfiguration"]["scanMode"],
        json!("TARGET_SCANNING"),
        "expected the mode the update wrote, got {read}"
    );
    assert_eq!(
        ami["scanModeConfiguration"]["tags"],
        json!([{ "key": "Fleet", "value": "assembled" }]),
        "expected the tag filter the update wrote, got {read}"
    );

    // Signed for the sibling service, which serves neither route: neither is answered from
    // Amazon Inspector's store, and the configuration it holds is unchanged.
    for (route, body) in [(UPDATE_URI, ami_update()), (GET_URI, json!({}))] {
        let (status, answered) = post(&router, INSPECTOR_SCAN, route, &body).await;
        assert!(
            !(200..300).contains(&status),
            "case {route}: a request scoped to {INSPECTOR_SCAN} must not be answered from \
             {INSPECTOR2}'s store, got {status} / {answered}"
        );
    }

    let (status, after) = post(&router, INSPECTOR2, GET_URI, &json!({})).await;
    assert_eq!(
        status, 200,
        "GetConfiguration after the unclaimed requests: expected 200, got {status} / {after}"
    );
    // The values in force, not the status the setting reports: a change is applied over a
    // span, so comparing whole answers would compare instants rather than values.
    for member in ["createdWithin", "scanModeConfiguration"] {
        assert_eq!(
            after["amiConfiguration"][member], ami[member],
            "a request no service claimed must change nothing, got {after} after {read}"
        );
    }
}
