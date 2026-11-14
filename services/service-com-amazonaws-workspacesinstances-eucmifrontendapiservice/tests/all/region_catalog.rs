// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListRegions`: the Region catalog this emulator supports.
//!
//! Every assertion goes through the router, over the wire this service really speaks:
//! awsJson1_0, `POST /`, `X-Amz-Target: EUCMIFrontendAPIService.<OperationName>`, with
//! the SigV4 signing name `workspaces-instances` in the credential scope.
//!
//! The rule under test is one sentence: the operation reports the service's whole
//! supported-Region catalog in a single response, in a stable order, whatever
//! `MaxResults` the caller asks for, and issues no pagination token — so the table
//! below spans the `MaxResults` values a caller can send rather than asserting one
//! recorded response. `MaxResults` is accepted inside its modelled `range 1..25` and
//! refused outside it, and a `NextToken` — which this operation never issues — is
//! refused as a token this service did not hand out.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "EUCMIFrontendAPIService";

/// The Region catalog this service reports, in the order `ListRegions` reports it.
///
/// Pinned here rather than read out of the crate, so the test measures what goes over
/// the wire against the documented set instead of against itself. SOURCE: AWS's
/// published Region availability for WorkSpaces Instances in the commercial
/// partition, recorded on `crate::state::SUPPORTED_REGIONS` with the same note.
const EXPECTED_REGIONS: &[&str] = &[
    "af-south-1",
    "ap-east-1",
    "ap-northeast-1",
    "ap-northeast-2",
    "ap-south-1",
    "ap-southeast-1",
    "ap-southeast-2",
    "ap-southeast-5",
    "ca-central-1",
    "eu-central-1",
    "eu-south-2",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "il-central-1",
    "me-central-1",
    "sa-east-1",
    "us-east-1",
    "us-east-2",
    "us-west-2",
];

/// A router carrying this service and the service that owns the EC2 instance a WorkSpace
/// Instance is, over one shared engine — the same `EmulatedService::new(engine)` seam
/// `core/router` mounts both through. Amazon EC2 is mounted because a create launches a
/// real instance there.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are the emulator's concern, not this service's; the tests drive
    // unsigned requests whose credential scope still names the service.
    router.set_request_authentication(false);
    router.register_service(wsinst::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_ec2_amazonec2::EmulatedService::new(Arc::clone(&engine)),
    );
    // The network and the images the create fixtures name, in the account's own EC2.
    crate::workspace_instance_lifecycle::seed_account_network(&engine);
    router
}

/// One awsJson1_0 call. Returns the HTTP status and the parsed JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "workspaces-instances.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/workspaces-instances/aws4_request, SignedHeaders=host, Signature=0",
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let payload = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or_else(|e| {
            panic!(
                "{operation} answered a body that is not JSON ({e}): {}",
                String::from_utf8_lossy(&raw)
            )
        })
    };
    (status, payload)
}

/// `call`, asserting a 200.
async fn ok(router: &MultiServiceRouter, case: &str, operation: &str, body: Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {operation}, got {status} with {payload}"
    );
    payload
}

/// The awsJson error code: the `__type` discriminator in the error envelope.
fn error_code(payload: &Value) -> String {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// `RegionName`'s modelled shape: `^[-0-9a-z]{1,31}$`
/// (`smithy_query(query="shape",shape="RegionName")`).
fn is_modelled_region_name(name: &str) -> bool {
    (1..=31).contains(&name.len())
        && name
            .chars()
            .all(|c| c == '-' || c.is_ascii_digit() || c.is_ascii_lowercase())
}

/// The `RegionName`s a `ListRegions` response reports, asserting on the way that the
/// response carries no pagination token and that every name has the modelled shape.
fn reported_regions(case: &str, payload: &Value) -> Vec<String> {
    assert!(
        payload.get("NextToken").is_none(),
        "case {case:?}: ListRegions reports the catalog whole and must return no \
         NextToken, got {payload}"
    );
    let regions: Vec<String> = payload
        .get("Regions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: ListRegions reported no Regions member: {payload}")
        })
        .iter()
        .map(|region| {
            region
                .get("RegionName")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a Region carried no RegionName: {region}")
                })
                .to_string()
        })
        .collect();
    for name in &regions {
        assert!(
            is_modelled_region_name(name),
            "case {case:?}: {name:?} is not shaped like the modelled RegionName"
        );
    }
    regions
}

/// `ListRegions` with the given request body.
async fn list_regions(router: &MultiServiceRouter, case: &str, body: Value) -> Vec<String> {
    let payload = ok(router, case, "ListRegions", body).await;
    reported_regions(case, &payload)
}

#[tokio::test]
async fn list_regions_reports_the_whole_catalog_whatever_max_results_asks_for() {
    let router = router();

    // The rows differ along the one thing a caller can vary: `MaxResults`. `None` is
    // the absent member, 1 and 25 are the published bounds of `MaxResults`
    // (`range 1..25`), and the rest sit inside, below, at and above the catalog's own
    // length — the real service truncates for none of them.
    let rows: Vec<(&str, Value)> = vec![
        ("no MaxResults", json!({})),
        (
            "MaxResults at the modelled minimum of 1",
            json!({ "MaxResults": 1 }),
        ),
        ("MaxResults 2", json!({ "MaxResults": 2 })),
        ("MaxResults 7, a third of the catalog", json!({ "MaxResults": 7 })),
        (
            "MaxResults 19, one short of the catalog",
            json!({ "MaxResults": 19 }),
        ),
        (
            "MaxResults 20, the catalog's own length",
            json!({ "MaxResults": 20 }),
        ),
        (
            "MaxResults 21, one past the catalog",
            json!({ "MaxResults": 21 }),
        ),
        (
            "MaxResults at the modelled maximum of 25",
            json!({ "MaxResults": 25 }),
        ),
    ];

    for (case, body) in rows {
        let reported = list_regions(&router, case, body).await;
        assert_eq!(
            reported, EXPECTED_REGIONS,
            "case {case:?}: ListRegions must report the whole catalog in ascending \
             RegionName order, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn the_catalog_holds_the_documented_regions_and_nothing_outside_the_partition() {
    let router = router();
    let case = "catalog membership";
    let reported = list_regions(&router, case, json!({})).await;

    // Entries the issue cited none of: it named no Region at all.
    for present in ["af-south-1", "eu-central-1", "il-central-1", "ap-southeast-2"] {
        assert!(
            reported.iter().any(|name| name == present),
            "case {case:?}: the catalog must report {present:?}, got {reported:?}"
        );
    }
    // The catalog is the commercial partition only, as its doc comment states.
    for absent in [
        "us-gov-west-1",
        "us-gov-east-1",
        "cn-north-1",
        "cn-northwest-1",
    ] {
        assert!(
            !reported.iter().any(|name| name == absent),
            "case {case:?}: {absent:?} is outside the commercial partition and must not \
             be reported, got {reported:?}"
        );
    }
    let mut sorted = reported.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted, reported,
        "case {case:?}: the catalog must be reported sorted and without duplicates, \
         got {reported:?}"
    );
}

#[tokio::test]
async fn max_results_outside_the_modelled_range_is_refused() {
    let router = router();

    // `MaxResults` is `range 1..25`: each published bound is accepted (the table above
    // covers both) and the value just past each one is refused at the boundary.
    for (case, max_results) in [("one below the minimum", 0), ("one above the maximum", 26)] {
        let (status, payload) =
            call(&router, "ListRegions", json!({ "MaxResults": max_results })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: MaxResults {max_results} must be refused with HTTP 400, \
             got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {payload}"
        );
    }
}

#[tokio::test]
async fn a_next_token_is_refused_because_this_operation_issues_none() {
    let router = router();

    // The catalog comes back whole, so the operation hands out no `NextToken`: every
    // token a caller could send is therefore one this service did not issue, whatever
    // it looks like. The rows span a well-formed Region name, a plausible opaque
    // token, and malformed bytes.
    let rows: Vec<(&str, String)> = vec![
        ("hex of a Region name in the catalog", hex::encode("us-east-1")),
        (
            "hex of a Region name outside the catalog",
            hex::encode("eu-north-1"),
        ),
        ("an opaque base64-looking token", "QUFBQUFBQUE=".to_string()),
        ("not hex at all", "not-a-token".to_string()),
        ("odd-length hex", "abc".to_string()),
        ("hex of bytes that are not UTF-8", "ff".to_string()),
        ("a single character, the modelled minimum", "a".to_string()),
        (
            "2048 characters, the modelled maximum",
            "a".repeat(2048),
        ),
    ];

    for (case, token) in rows {
        let (status, payload) = call(&router, "ListRegions", json!({ "NextToken": token })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a NextToken must be refused with HTTP 400, got {status} \
             with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {payload}"
        );
    }

    // And a token past the modelled `len 1..2048` is refused at the boundary, with the
    // same status and code.
    let case = "2049 characters, one past the modelled maximum";
    let (status, payload) = call(
        &router,
        "ListRegions",
        json!({ "NextToken": "a".repeat(2049) }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {payload}"
    );
}

#[tokio::test]
async fn listing_regions_changes_nothing_a_later_caller_sees() {
    let router = router();

    // `ListRegions` is `@readonly`: repeated calls answer the same catalog, and the
    // account's stored instances cannot move it either way.
    let first = list_regions(&router, "first call", json!({})).await;
    let second = list_regions(&router, "second call", json!({})).await;
    assert_eq!(
        second, first,
        "case \"repeat call\": a read must not change what a later caller sees, \
         got {second:?} after {first:?}"
    );

    let created = ok(
        &router,
        "create an instance",
        "CreateWorkspaceInstance",
        json!({
            "ManagedInstance": {
                "ImageId": "ami-0123456789abcdef0",
                "InstanceType": "t3.medium",
            },
        }),
    )
    .await;
    assert!(
        created
            .get("WorkspaceInstanceId")
            .and_then(Value::as_str)
            .is_some(),
        "case \"create an instance\": expected a minted WorkspaceInstanceId, got {created}"
    );

    let after = list_regions(&router, "after a create", json!({})).await;
    assert_eq!(
        after, first,
        "case \"after a create\": the Region catalog is a service fact and must not \
         depend on stored instances, got {after:?} after {first:?}"
    );
}
