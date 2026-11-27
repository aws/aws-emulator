// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHO a domain's REST data plane serves: the caller a request
//! authenticates as.
//!
//! The rule under test: a request to the address a domain published is served only when
//! it presents a credential the emulator verified; one that presents none authenticates
//! as nobody, which no domain admits, so it is refused HTTP 403 naming the action it
//! asked for (`es:ESHttp<Method>`, the actions AWS names this plane's requests by) and
//! NOTHING it asked for happens — a write leaves no document behind and a delete leaves
//! the one that is there.
//!
//! The table spans what the rule generalises over: the HTTP method (which decides the
//! action named), the route (cluster banner, index, document, search) and whether the
//! request reads or writes.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_elasticsearchservice_amazonelasticsearchservice2015::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call is signed for.
const REGION: &str = "us-east-1";

/// The domain the suite drives.
const DOMAIN: &str = "auth-plane-domain";

/// How long a poll waits for the cluster to come up before it gives up. Generous: the
/// condition ends the loop, not the clock.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// How long one turn of that poll waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// One answer: the status and the parsed body.
struct Answer {
    status: u16,
    body: Value,
}

/// A router carrying only this service, over one shared engine.
fn router() -> MultiServiceRouter {
    let engine = Arc::new(::engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// One request, optionally carrying a SigV4 credential.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<&Value>,
    signed: bool,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://localhost{path}"))
        .header("content-type", "application/json");
    if signed {
        builder = builder.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/es/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).to_string()))
    };
    Answer { status, body }
}

/// The published path of a data-plane request against [`DOMAIN`].
fn plane_path(request_path: &str) -> String {
    format!("/_aws/es-2015-01-01/{DOMAIN}{request_path}")
}

/// Create the domain and wait for its cluster to answer.
async fn domain_with_a_live_cluster() -> MultiServiceRouter {
    let router = router();
    let created = dispatch(
        &router,
        "POST",
        "/2015-01-01/es/domain",
        Some(&json!({
            "DomainName": DOMAIN,
            "ElasticsearchVersion": "7.10",
            "ElasticsearchClusterConfig": {"InstanceType": "t2.small.elasticsearch", "InstanceCount": 1}
        })),
        true,
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the domain must be created, got {} {}",
        created.status, created.body
    );

    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let banner = dispatch(&router, "GET", &plane_path("/"), None.as_ref(), true).await;
        if banner.status == 200 {
            return router;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the cluster never came up; last answer {} {}",
            banner.status,
            banner.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One data-plane request, and the action its method is named by.
struct Case {
    label: &'static str,
    method: &'static str,
    path: &'static str,
    body: Option<Value>,
    /// The IAM action the refusal names — `es:ESHttp<Method>` over the request's method.
    action: &'static str,
}

/// Rows differ along the method (which decides the action), the route and whether the
/// request reads or writes.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "the cluster banner",
            method: "GET",
            path: "/",
            body: None,
            action: "es:ESHttpGet",
        },
        Case {
            label: "a HEAD of an index",
            method: "HEAD",
            path: "/books",
            body: None,
            action: "es:ESHttpHead",
        },
        Case {
            label: "creating an index",
            method: "PUT",
            path: "/journals",
            body: Some(json!({})),
            action: "es:ESHttpPut",
        },
        Case {
            label: "writing a document",
            method: "POST",
            path: "/books/_doc/7",
            body: Some(json!({"title": "written by nobody"})),
            action: "es:ESHttpPost",
        },
        Case {
            label: "deleting a document",
            method: "DELETE",
            path: "/books/_doc/1",
            body: None,
            action: "es:ESHttpDelete",
        },
        Case {
            label: "a search",
            method: "POST",
            path: "/books/_search",
            body: Some(json!({"query": {"match_all": {}}})),
            action: "es:ESHttpPost",
        },
    ]
}

/// RULE: a data-plane request presenting no credential is refused 403, naming the action
/// it asked for, and changes nothing.
#[tokio::test]
async fn a_data_plane_request_with_no_credential_is_refused_and_changes_nothing() {
    let router = domain_with_a_live_cluster().await;

    // One document to delete and to search for, written by a caller the emulator
    // verified.
    let written = dispatch(
        &router,
        "PUT",
        &plane_path("/books/_doc/1"),
        Some(&json!({"title": "written by a signed caller"})),
        true,
    )
    .await;
    assert_eq!(
        written.status, 201,
        "the signed write must be served, got {} {}",
        written.status, written.body
    );

    for case in cases() {
        let label = case.label;
        let refused = dispatch(
            &router,
            case.method,
            &plane_path(case.path),
            case.body.as_ref(),
            false,
        )
        .await;
        assert_eq!(
            refused.status, 403,
            "case {label:?}: an unsigned data-plane request is refused 403, got {} {}",
            refused.status, refused.body
        );
        if case.method != "HEAD" {
            assert_eq!(
                refused.body,
                json!({
                    "Message": format!(
                        "User: anonymous is not authorized to perform: {}",
                        case.action
                    )
                }),
                "case {label:?}: the refusal names the caller and the action, got {}",
                refused.body
            );
        }
    }

    // Nothing the refused requests asked for happened: the document a signed caller
    // wrote is still there, the one an unsigned caller wrote never was, and the index an
    // unsigned caller asked for does not exist.
    let read = dispatch(&router, "GET", &plane_path("/books/_doc/1"), None.as_ref(), true).await;
    assert_eq!(
        read.status, 200,
        "the document the unsigned delete asked for is still there, got {} {}",
        read.status, read.body
    );
    let absent = dispatch(&router, "GET", &plane_path("/books/_doc/7"), None.as_ref(), true).await;
    assert_eq!(
        absent.status, 404,
        "the document the unsigned write asked for was never stored, got {} {}",
        absent.status, absent.body
    );
    let index = dispatch(&router, "GET", &plane_path("/journals"), None.as_ref(), true).await;
    assert_eq!(
        index.status, 404,
        "the index the unsigned create asked for was never created, got {} {}",
        index.status, index.body
    );
}

/// RULE, the other half: the same requests, signed, are served — so the refusal above is
/// about the credential and not about the route.
#[tokio::test]
async fn the_same_requests_are_served_for_a_verified_caller() {
    let router = domain_with_a_live_cluster().await;
    dispatch(
        &router,
        "PUT",
        &plane_path("/books/_doc/1"),
        Some(&json!({"title": "written by a signed caller"})),
        true,
    )
    .await;

    for case in cases() {
        let label = case.label;
        let served = dispatch(
            &router,
            case.method,
            &plane_path(case.path),
            case.body.as_ref(),
            true,
        )
        .await;
        assert!(
            served.status < 400,
            "case {label:?}: a signed data-plane request is served, got {} {}",
            served.status,
            served.body
        );
    }
}
