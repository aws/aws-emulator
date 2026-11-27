// SPDX-License-Identifier: CC0-1.0

//! What a product REST endpoint page reports about ITS OWN state, and what it never reports.
//!
//! RULE: **a status member of a resource representation describes the state THAT resource is in
//! as of the read, and never the message of a request the service refused.** A product REST
//! endpoint page reports a `StatusException` ("the status exception information") only for a
//! `Status` its content was not generated for, so the sentence a refusal is worded in is stated
//! in that refusal's OWN answer and in nothing any reader of the page serves. The same rule
//! holds of the other human-readable status member API Gateway serves, a VPC link's status
//! description, which `apigateway_vpc_link_status_message_surfaces` asserts it of.
//!
//! The pages under test are only reachable from here: a page documents a REST endpoint of the
//! API Gateway service that owns REST APIs, and a write naming an endpoint the account does not
//! have is refused there, so this suite mounts both control planes on ONE engine exactly as
//! `core/router/src/lib.rs` mounts them, and every endpoint a row documents is created through
//! the service that OWNS it.
//!
//! The rows differ along what the rule generalises over: WHICH refusal the sentence comes from
//! (an identifier naming a REST API the account holds nothing under, a stage the API it names
//! has none of, a display content naming neither of its two alternatives, a page id nothing
//! minted, an update of a page whose stage the account no longer has), which writer or reader
//! was refused and with which modelled error class, which reader reports the page back (the
//! create's own answer, the single-page read, the read that asks for the raw display content,
//! the update), what the page itself carries (its try-it state, the method and path it
//! documents, whether its display content was overridden), and which status the page is in when
//! it is read — the transient one the clock still owes and the settled one.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// The `Status` values a product REST endpoint page reports, from the modelled enum
/// (`smithy_query(query="shape", shape="com.amazonaws.apigatewayv2#Status")`: `AVAILABLE`,
/// `IN_PROGRESS`, `FAILED`).
const AVAILABLE: &str = "AVAILABLE";
const IN_PROGRESS: &str = "IN_PROGRESS";
const FAILED: &str = "FAILED";

/// The sentence API Gateway refuses a `RestEndpointIdentifier` that names no REST API, or no
/// stage of the API it names, with. Spelled out here rather than read off the service crate's
/// own constant, so a drift in either of them is caught: it is a REFUSAL's wording, and the
/// point of this suite is that no representation of a page states it.
const UNRESOLVED_ENDPOINT_SENTENCE: &str = "Invalid restEndpointIdentifier stage or restApiId";

/// A `RestApiId` in the form the owner of REST APIs mints (ten lowercase alphanumerics) that the
/// account holds nothing under, so what fails is the resolution and not the shape of the value.
const UNKNOWN_REST_API: &str = "nosuchapi0";

/// A `ProductRestEndpointPageId` of the modelled shape (`__stringMin10Max30PatternAZ09`) that
/// nothing minted.
const UNKNOWN_PAGE: &str = "nosuchpage0";

/// The portal-products collection URI; one product's pages hang off it as
/// `{PRODUCTS}/{id}/productrestendpointpages`.
const PRODUCTS: &str = "/v2/portalproducts";

/// The page status member this suite reads, as the wire names it
/// (`GetProductRestEndpointPageResponse$StatusException`, jsonName `statusException`).
const PAGE_EXCEPTION: &str = "statusException";

/// How long a wait for a CONDITION may take before the test gives up, and how long it waits
/// between readings. Generous: the transition is polled for rather than waited out, and the
/// deadline is only what stops a test whose premise never arrives.
const CONDITION_DEADLINE: Duration = Duration::from_secs(60);
const CONDITION_POLL: Duration = Duration::from_millis(25);

/// One answer: its HTTP status and its body as JSON (`Value::Null` when there is none).
struct Reply {
    status: u16,
    body: Value,
}

/// The emulator with the two control planes this suite needs, on ONE engine as the router mounts
/// them: this service, and the API Gateway service that owns the REST APIs a page documents.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(engine),
    );
    router
}

/// One API Gateway control-plane call over its restJson1 wire. Both surfaces answer to the one
/// signing name and are told apart by the path, as they are in the assembled emulator; the
/// signature is never checked.
async fn apigw(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Reply {
    let host = format!("apigateway.{REGION}.amazonaws.com");
    let payload = match &body {
        Some(document) => bytes::Bytes::from(document.to_string()),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", &host)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/apigateway\
                 /aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    Reply {
        status: response.status().as_u16(),
        body: serde_json::from_slice(response.body()).unwrap_or(Value::Null),
    }
}

/// One string member of a document, or the empty string when it carries none.
fn as_str(document: &Value, member: &str) -> String {
    document
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The error class a refusal carries.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Whether `member` carries nothing: absent, null, or an empty structure.
fn carries_nothing(document: &Value, member: &str) -> bool {
    document.get(member).is_none_or(|value| match value {
        Value::Null => true,
        Value::Object(map) => map.is_empty(),
        Value::Array(items) => items.is_empty(),
        _ => false,
    })
}

/// Read `path` until `settled` holds of the answer, and answer that reading.
///
/// The wait is for the CONDITION — the record is re-read until it reports what the case waits
/// for — never for a fixed time, so neither a fast nor a loaded host changes what a case
/// observes.
async fn read_until(
    router: &MultiServiceRouter,
    case: &str,
    path: &str,
    settled: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + CONDITION_DEADLINE;
    loop {
        let read = apigw(router, "GET", path, None).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GET {path}, got {} with {}",
            read.status, read.body
        );
        if settled(&read.body) {
            return read.body;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: {path} never reported what the case waits for, last read {}",
            read.body
        );
        tokio::time::sleep(CONDITION_POLL).await;
    }
}

/// One REST endpoint the ACCOUNT holds: a REST API with `stage` deployed on it, created through
/// the service that OWNS REST APIs, answered as the `RestApiId` that service minted.
///
/// A stage serves a DEPLOYMENT, and a deployment is the snapshot of the API's methods, so the API
/// gets one method on its root resource first — the shortest path a client takes to a stage it
/// can invoke. What this suite calls "an endpoint the account holds" is therefore what that
/// service reports, not an identifier a test made up.
async fn rest_endpoint(router: &MultiServiceRouter, name: &str, stage: &str) -> String {
    let created = apigw(router, "POST", "/restapis", Some(json!({ "name": name }))).await;
    assert_eq!(
        created.status, 201,
        "setup: expected 201 from CreateRestApi, got {} with {}",
        created.status, created.body
    );
    let rest_api_id = as_str(&created.body, "id");
    let root = as_str(&created.body, "rootResourceId");

    let method = apigw(
        router,
        "PUT",
        &format!("/restapis/{rest_api_id}/resources/{root}/methods/GET"),
        Some(json!({ "authorizationType": "NONE" })),
    )
    .await;
    assert_eq!(
        method.status, 201,
        "setup: expected 201 from PutMethod, got {} with {}",
        method.status, method.body
    );

    // The method's integration, which is what a stage serves a matched request with: an API
    // holding a method with none cannot be deployed at all. `MOCK` is the integration type
    // that needs no backend.
    let integration = apigw(
        router,
        "PUT",
        &format!("/restapis/{rest_api_id}/resources/{root}/methods/GET/integration"),
        Some(json!({ "type": "MOCK" })),
    )
    .await;
    assert_eq!(
        integration.status, 201,
        "setup: expected 201 from PutIntegration, got {} with {}",
        integration.status, integration.body
    );

    let deployment = apigw(
        router,
        "POST",
        &format!("/restapis/{rest_api_id}/deployments"),
        Some(json!({ "stageName": stage })),
    )
    .await;
    assert_eq!(
        deployment.status, 201,
        "setup: expected 201 from CreateDeployment, got {} with {}",
        deployment.status, deployment.body
    );
    rest_api_id
}

/// `IdentifierParts` as a request sends them: the REST API, the stage, the method and the path of
/// the endpoint a page documents.
fn parts(rest_api_id: &str, stage: &str, method: &str, path: &str) -> Value {
    json!({
        "restApiId": rest_api_id,
        "stage": stage,
        "method": method,
        "path": path
    })
}

/// A `CreateProductRestEndpointPage` body documenting `identifier_parts` and nothing else.
fn page_request(identifier_parts: Value) -> Value {
    json!({ "restEndpointIdentifier": { "identifierParts": identifier_parts } })
}

/// One call the service must REFUSE: what it was, how it is addressed, and the status and modelled
/// error class it comes back as.
struct Refused {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    status: u16,
    code: &'static str,
    /// The sentence this refusal is known to be worded in, where the service states one this
    /// suite can pin; `None` where the case needs only SOME sentence of the service's to then
    /// hold every representation against.
    sentence: Option<&'static str>,
}

/// Drive one refused call and answer the sentence it was refused with.
async fn refusal(router: &MultiServiceRouter, refused: &Refused) -> String {
    let case = refused.case;
    let answer = apigw(router, refused.method, &refused.uri, refused.body.clone()).await;
    assert_eq!(
        answer.status, refused.status,
        "case {case:?}: expected {} from {} {}, got {} with {}",
        refused.status, refused.method, refused.uri, answer.status, answer.body
    );
    assert_eq!(
        error_code(&answer.body),
        refused.code,
        "case {case:?}: expected {}, got {}",
        refused.code,
        answer.body
    );
    let sentence = as_str(&answer.body, "message");
    assert!(
        !sentence.is_empty(),
        "case {case:?}: expected the refusal to state why, got {}",
        answer.body
    );
    if let Some(expected) = refused.sentence {
        assert_eq!(
            sentence, expected,
            "case {case:?}: expected the refusal to be worded {expected:?}, got {sentence:?}"
        );
    }
    sentence
}

/// Assert what the rule says of the `Status`/`StatusException` pair `document` carries.
///
/// A page reporting a status its content was generated for has nothing to explain, so it carries
/// no exception at all; one reporting `FAILED` explains ITS OWN failure, so the exception it
/// carries says something. Either way neither the status member itself nor anything else the
/// representation carries repeats a sentence a refusal was worded in.
fn assert_page_status(case: &str, reader: &str, document: &Value, refusals: &[String]) {
    let status = as_str(document, "status");
    assert!(
        [AVAILABLE, IN_PROGRESS, FAILED].contains(&status.as_str()),
        "case {case:?} ({reader}): expected a modelled Status, got {status:?} {document}"
    );
    let exception = document
        .get(PAGE_EXCEPTION)
        .and_then(|exception| exception.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if status == FAILED {
        assert!(
            !exception.is_empty(),
            "case {case:?} ({reader}): expected a page reporting {FAILED} to explain its own \
             failure, got {document}"
        );
    } else {
        assert!(
            carries_nothing(document, PAGE_EXCEPTION),
            "case {case:?} ({reader}): expected a page reporting {status:?} to have nothing to \
             explain, got {document}"
        );
    }
    // The status member itself first — the one the rule is about — and then the whole
    // representation, so a refusal's wording cannot reach the client through any other member
    // either.
    for sentence in refusals {
        assert_ne!(
            exception, sentence,
            "case {case:?} ({reader}): expected {PAGE_EXCEPTION} to describe the page's own \
             state, got the refusal {sentence:?}"
        );
        let served = document.to_string();
        assert!(
            !served.contains(sentence.as_str()),
            "case {case:?} ({reader}): expected the representation to state nothing of the \
             refusal {sentence:?}, got {served}"
        );
    }
}

/// Every representation of the page at `uri` a reader serves, each asserted to have been served
/// at all: the single-page read, the read that asks for the page's raw display content, and the
/// answer of an update that re-states the try-it state the page already carries.
async fn page_representations(
    router: &MultiServiceRouter,
    case: &str,
    uri: &str,
    try_it_state: &str,
) -> Vec<(&'static str, Value)> {
    let raw_uri = format!("{uri}?includeRawDisplayContent=true");
    let mut served = Vec::new();
    for (reader, method, target, body) in [
        ("GetProductRestEndpointPage", "GET", uri, None),
        (
            "GetProductRestEndpointPage with the raw display content",
            "GET",
            raw_uri.as_str(),
            None,
        ),
        (
            "UpdateProductRestEndpointPage",
            "PATCH",
            uri,
            Some(json!({ "tryItState": try_it_state })),
        ),
    ] {
        let answer = apigw(router, method, target, body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 from {reader}, got {} with {}",
            answer.status, answer.body
        );
        served.push((reader, answer.body));
    }
    served
}

/// One page the account HOLDS, written against the REST endpoint the owner of REST APIs reports:
/// the rows differ along the try-it state, the method and path the page documents, and whether the
/// request overrode the display content API Gateway generates.
struct Held {
    case: &'static str,
    /// The create body, written against the minted REST API id and the deployed stage.
    request: fn(&str, &str) -> Value,
    try_it_state: &'static str,
}

fn held_pages() -> Vec<Held> {
    vec![
        Held {
            case: "the required members only",
            request: |rest_api_id, stage| page_request(parts(rest_api_id, stage, "GET", "/orders")),
            // The request names no `TryItState`, so the page carries the one the service applies.
            try_it_state: "ENABLED",
        },
        Held {
            case: "try-it disabled, documenting another method and path of the same stage",
            request: |rest_api_id, stage| {
                json!({
                    "restEndpointIdentifier": {
                        "identifierParts":
                            parts(rest_api_id, stage, "POST", "/accounts/{accountId}/orders")
                    },
                    "tryItState": "DISABLED"
                })
            },
            try_it_state: "DISABLED",
        },
        Held {
            case: "a page whose display content overrides what API Gateway generates",
            request: |rest_api_id, stage| {
                json!({
                    "restEndpointIdentifier": {
                        "identifierParts": parts(rest_api_id, stage, "DELETE", "/orders/{orderId}")
                    },
                    "displayContent": {
                        "overrides": {
                            "body": "# Orders\nThe orders of one account.\n",
                            "endpoint": "https://orders.example.com/prod",
                            "operationName": "DeleteOrder"
                        }
                    },
                    "tryItState": "ENABLED"
                })
            },
            try_it_state: "ENABLED",
        },
    ]
}

/// RULE: the sentence a refused page write is worded in is stated in that refusal's own answer and
/// in no representation of a page — whichever reader serves it, and whichever of the statuses the
/// clock moves the page through it is read in.
#[tokio::test]
async fn a_refused_page_write_states_its_sentence_only_in_its_own_answer() {
    let router = build_router();
    let created = apigw(
        &router,
        "POST",
        PRODUCTS,
        Some(json!({ "displayName": "Status members" })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "setup: expected 201 from CreatePortalProduct, got {} with {}",
        created.status, created.body
    );
    let collection = format!(
        "{PRODUCTS}/{}/productrestendpointpages",
        as_str(&created.body, "portalProductId")
    );
    let stage = "prod";
    let rest_api_id = rest_endpoint(&router, "status-members", stage).await;

    // Every sentence the service refused a request about this product's pages with. Every
    // representation read below is held against all of them.
    let mut refusals: Vec<String> = Vec::new();
    for refused in [
        Refused {
            case: "a create naming a REST API id the account holds nothing under",
            method: "POST",
            uri: collection.clone(),
            body: Some(page_request(parts(UNKNOWN_REST_API, stage, "GET", "/orders"))),
            status: 400,
            code: "BadRequestException",
            sentence: Some(UNRESOLVED_ENDPOINT_SENTENCE),
        },
        Refused {
            case: "a create naming a stage the account's REST API has none of",
            method: "POST",
            uri: collection.clone(),
            body: Some(page_request(parts(&rest_api_id, "beta", "GET", "/orders"))),
            status: 400,
            code: "BadRequestException",
            sentence: Some(UNRESOLVED_ENDPOINT_SENTENCE),
        },
        Refused {
            // `EndpointDisplayContent` carries the two alternatives `none` and `overrides`
            // (`smithy_query(query="shape",
            // shape="com.amazonaws.apigatewayv2#EndpointDisplayContent")`), so a display content
            // naming neither of them describes no content at all.
            case: "a create whose display content names neither of its alternatives",
            method: "POST",
            uri: collection.clone(),
            body: Some(json!({
                "restEndpointIdentifier": {
                    "identifierParts": parts(&rest_api_id, stage, "GET", "/orders")
                },
                "displayContent": {}
            })),
            status: 400,
            code: "BadRequestException",
            sentence: None,
        },
        Refused {
            case: "a read of a page id nothing minted",
            method: "GET",
            uri: format!("{collection}/{UNKNOWN_PAGE}"),
            body: None,
            status: 404,
            code: "NotFoundException",
            sentence: None,
        },
    ] {
        refusals.push(refusal(&router, &refused).await);
    }

    // The pages the account holds, each read through every reader in both of the statuses the
    // clock moves it through.
    let mut pages: Vec<(&'static str, String, &'static str)> = Vec::new();
    for row in held_pages() {
        let case = row.case;
        let answer = apigw(
            &router,
            "POST",
            &collection,
            Some((row.request)(&rest_api_id, stage)),
        )
        .await;
        assert_eq!(
            answer.status, 201,
            "case {case:?}: expected 201 from CreateProductRestEndpointPage, got {} with {}",
            answer.status, answer.body
        );
        let uri = format!(
            "{collection}/{}",
            as_str(&answer.body, "productRestEndpointPageId")
        );

        // The content is still being generated when the create answers, so the create's own
        // projection reports the transient status — and explains nothing.
        assert_page_status(
            case,
            "CreateProductRestEndpointPage",
            &answer.body,
            &refusals,
        );
        assert_eq!(
            as_str(&answer.body, "status"),
            IN_PROGRESS,
            "case {case:?}: expected the create to report {IN_PROGRESS}, got {}",
            answer.body
        );
        for (reader, document) in page_representations(&router, case, &uri, row.try_it_state).await
        {
            assert_page_status(case, reader, &document, &refusals);
        }

        // The generation is a transition the CLOCK owes: the read is repeated until the page
        // reports it has settled, never timed.
        let settled = read_until(&router, case, &uri, |document| {
            as_str(document, "status") != IN_PROGRESS
        })
        .await;
        assert_page_status(
            case,
            "GetProductRestEndpointPage once settled",
            &settled,
            &refusals,
        );
        assert_eq!(
            as_str(&settled, "status"),
            AVAILABLE,
            "case {case:?}: expected the page of an endpoint the account holds to settle \
             {AVAILABLE}, got {settled}"
        );
        pages.push((case, uri, row.try_it_state));
    }

    // And one more refusal, of a WRITE to a page that already exists: the stage those pages
    // document is taken away through the owner of REST APIs, which leaves an update with nothing
    // to re-generate the page's content from.
    let accepted = apigw(
        &router,
        "DELETE",
        &format!("/restapis/{rest_api_id}/stages/{stage}"),
        None,
    )
    .await;
    assert_eq!(
        accepted.status, 202,
        "setup: expected the owner of REST APIs to accept DeleteStage, got {} with {}",
        accepted.status, accepted.body
    );
    let (_, first_page, first_try_it_state) =
        pages.first().expect("the rows created at least one page");
    refusals.push(
        refusal(
            &router,
            &Refused {
                case: "an update of a page whose stage the account no longer has",
                method: "PATCH",
                uri: first_page.clone(),
                body: Some(json!({ "tryItState": first_try_it_state })),
                status: 400,
                code: "BadRequestException",
                sentence: Some(UNRESOLVED_ENDPOINT_SENTENCE),
            },
        )
        .await,
    );

    // Every page the account holds, read again now that every refusal has been answered: the
    // resolution belongs to the WRITE, so a stored page still reports what it was generated as,
    // and none of them has taken on a refusal's wording.
    for (case, uri, _) in &pages {
        let read = read_until(&router, case, uri, |document| {
            as_str(document, "status") != IN_PROGRESS
        })
        .await;
        assert_page_status(
            case,
            "GetProductRestEndpointPage after the refusals",
            &read,
            &refusals,
        );
    }
}
