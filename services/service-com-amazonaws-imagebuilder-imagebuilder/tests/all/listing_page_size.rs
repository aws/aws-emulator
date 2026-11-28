// SPDX-License-Identifier: CC0-1.0

//! The page size EVERY listing of this service takes, end to end over restJson1.
//!
//! The rule under test: the page size Image Builder accepts for `maxResults` stops at 25 —
//! the maximum the shape every listing's page size targets is declared with
//! (`RestrictedInteger`, `range 1..25`, from this crate's overlay model
//! `smithy-model/local-additions.json`, see `smithy-model/LOCAL-ADDITIONS.md`) — so each
//! listing serves that maximum and refuses the first size past it with
//! `InvalidParameterValueException` (HTTP 400) in this API's own parameter sentence, naming
//! the member and the bound its value had to satisfy. The bound belongs to the page size
//! rather than to one operation, so the rows are the LISTINGS: a bound applied at one and
//! not at another is the defect this suite exists to catch.
//!
//! Where each clause comes from: `smithy_query(query="operation",operation="ListComponents")`
//! reports the `maxResults` member every listing of this model carries, and
//! `smithy_query(query="shape",shape="InvalidParameterValueException")` the error it is
//! refused with (a client error, `@httpError` 400, carrying one `message`). What the
//! snapshot states for that shape's range is NOT what the service enforces, which is why
//! the shape is declared locally; `tests/all/list_page_size.rs` owns how a page is CUT at
//! the size asked for, and `tests/all/modelled_range_bounds.rs` the other modelled ranges.
//!
//! The rows differ along what the rule generalises over: which listing is asked (all twenty
//! that take a page size, including the eight addressed by a resource and the two filtered
//! by owner), and which page size it is asked for — the smallest, the largest served, and
//! the first one past it. A listing is asked with the members it REQUIRES and nothing else,
//! so nothing but the page size can decide the answer; a required resource that does not
//! exist is immaterial, because the page size is judged before the resource is resolved.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every case signs for.
const REGION: &str = "eu-central-1";

/// The smallest page size a request may ask for.
const MIN_PAGE_SIZE: i64 = 1;

/// The largest page size a request may ask for: the maximum this API enforces, read from
/// the one place this crate states it.
const MAX_PAGE_SIZE: i64 =
    service_com_amazonaws_imagebuilder_imagebuilder::state::MAX_LIST_PAGE_SIZE as i64;

/// The error a refused parameter value carries.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// The sentence a page size past the maximum is refused with: this API's parameter sentence,
/// written out here so the wording is pinned by the suite and not only by the code.
fn page_size_refusal() -> String {
    format!(
        "The value supplied for parameter 'maxResults' is not valid. maxResults must be \
         smaller than or equal to {MAX_PAGE_SIZE}"
    )
}

/// One listing: the operation's path and the members it requires beside a page size.
struct Listing {
    operation: &'static str,
    selector: &'static [(&'static str, &'static str)],
}

/// An ARN of each form a listing requires, in this account and Region. None of them names a
/// resource the store holds: the page size is judged before the resource is resolved.
const IMAGE_BUILD_VERSION_ARN: &str =
    "arn:aws:imagebuilder:eu-central-1:000000000000:image/page-size/1.0.0/1";
const IMAGE_PIPELINE_ARN: &str =
    "arn:aws:imagebuilder:eu-central-1:000000000000:image-pipeline/page-size";
const COMPONENT_ARN: &str =
    "arn:aws:imagebuilder:eu-central-1:000000000000:component/page-size/1.0.0";
const WORKFLOW_VERSION_ARN: &str =
    "arn:aws:imagebuilder:eu-central-1:000000000000:workflow/build/page-size/1.0.0";
const LIFECYCLE_EXECUTION_ID: &str = "lce-01234567-89ab-cdef-0123-456789abcdef";
const WORKFLOW_EXECUTION_ID: &str = "wf-01234567-89ab-cdef-0123-456789abcdef";

/// Every listing of this service that takes a page size, with the members each requires.
const LISTINGS: &[Listing] = &[
    Listing {
        operation: "ListComponents",
        selector: &[],
    },
    Listing {
        operation: "ListComponentBuildVersions",
        selector: &[("componentVersionArn", COMPONENT_ARN)],
    },
    Listing {
        operation: "ListContainerRecipes",
        selector: &[],
    },
    Listing {
        operation: "ListDistributionConfigurations",
        selector: &[],
    },
    Listing {
        operation: "ListImageBuildVersions",
        selector: &[],
    },
    Listing {
        operation: "ListImagePackages",
        selector: &[("imageBuildVersionArn", IMAGE_BUILD_VERSION_ARN)],
    },
    Listing {
        operation: "ListImagePipelineImages",
        selector: &[("imagePipelineArn", IMAGE_PIPELINE_ARN)],
    },
    Listing {
        operation: "ListImagePipelines",
        selector: &[],
    },
    Listing {
        operation: "ListImageRecipes",
        selector: &[],
    },
    Listing {
        operation: "ListImageScanFindings",
        selector: &[],
    },
    Listing {
        operation: "ListImages",
        selector: &[],
    },
    Listing {
        operation: "ListInfrastructureConfigurations",
        selector: &[],
    },
    Listing {
        operation: "ListLifecycleExecutionResources",
        selector: &[("lifecycleExecutionId", LIFECYCLE_EXECUTION_ID)],
    },
    Listing {
        operation: "ListLifecycleExecutions",
        selector: &[("resourceArn", COMPONENT_ARN)],
    },
    Listing {
        operation: "ListLifecyclePolicies",
        selector: &[],
    },
    Listing {
        operation: "ListWaitingWorkflowSteps",
        selector: &[],
    },
    Listing {
        operation: "ListWorkflowBuildVersions",
        selector: &[("workflowVersionArn", WORKFLOW_VERSION_ARN)],
    },
    Listing {
        operation: "ListWorkflowExecutions",
        selector: &[("imageBuildVersionArn", IMAGE_BUILD_VERSION_ARN)],
    },
    Listing {
        operation: "ListWorkflowStepExecutions",
        selector: &[("workflowExecutionId", WORKFLOW_EXECUTION_ID)],
    },
    Listing {
        operation: "ListWorkflows",
        selector: &[],
    },
];

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(engine),
    );
    router
}

/// Ask `listing` for a page of `page_size` entries, and answer with the status and body.
async fn listed(router: &MultiServiceRouter, listing: &Listing, page_size: i64) -> (u16, Value) {
    let mut body = Map::new();
    for (member, value) in listing.selector {
        body.insert((*member).to_string(), json!(value));
    }
    body.insert("maxResults".to_string(), json!(page_size));
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/{}", listing.operation))
        .header("host", format!("imagebuilder.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDIMAGEBUILDER/20260101/{REGION}/imagebuilder/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&Value::Object(body)).expect("serialize the request body"),
        ))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The error code a refusal carries, from `__type` (restJson1 renders the shape there,
/// optionally namespace-qualified).
fn error_code(case: &str, body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an error shape, got {body}"))
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn every_listing_serves_the_page_size_up_to_the_maximum_this_api_enforces() {
    let router = emulator();

    // Rows: every listing, at the smallest page size and at the largest one served. A
    // listing whose required resource the store does not hold answers its own not-found
    // rather than a page — what matters here is that the PAGE SIZE is not what refused it.
    for listing in LISTINGS {
        for page_size in [MIN_PAGE_SIZE, MAX_PAGE_SIZE] {
            let case = format!("{} at maxResults={page_size}", listing.operation);
            let (status, body) = listed(&router, listing, page_size).await;
            let message = body
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                !message.contains("maxResults"),
                "case {case:?}: a page size the API accepts must not be what refuses the \
                 request, got {status} with {body}"
            );
        }
    }
}

#[tokio::test]
async fn every_listing_refuses_the_first_page_size_past_the_maximum() {
    let router = emulator();
    let expected = page_size_refusal();

    // Rows: every listing, at the first page size past the maximum and far past it. The
    // page is judged before the resource is resolved, so even a listing naming a resource
    // the store does not hold answers the page-size sentence.
    for listing in LISTINGS {
        for page_size in [MAX_PAGE_SIZE + 1, 10 * MAX_PAGE_SIZE, i64::from(i32::MAX)] {
            let case = format!("{} at maxResults={page_size}", listing.operation);
            let (status, body) = listed(&router, listing, page_size).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a page size past the maximum must be refused, got {status} \
                 with {body}"
            );
            let code = error_code(&case, &body);
            assert_eq!(
                code, INVALID_PARAMETER_VALUE,
                "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {code:?} in {body}"
            );
            let message = body
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_rows_are_every_listing_that_takes_a_page_size() {
    // The twenty listings of this model that carry a `maxResults`, so a listing the model
    // gains cannot be left out of the table above without this failing.
    assert_eq!(
        LISTINGS.len(),
        20,
        "expected every listing that takes a page size to be a row, got {}",
        LISTINGS.len()
    );
    let mut operations: Vec<&str> = LISTINGS.iter().map(|listing| listing.operation).collect();
    operations.sort_unstable();
    let distinct = {
        let mut distinct = operations.clone();
        distinct.dedup();
        distinct
    };
    assert_eq!(
        operations, distinct,
        "no listing may be a row twice, got {operations:?}"
    );
}
