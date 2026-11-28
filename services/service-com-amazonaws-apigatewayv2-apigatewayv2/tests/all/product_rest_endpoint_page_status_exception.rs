// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for WHAT a product REST endpoint page's `StatusException` says.
//!
//! The rule these cases assert, one level above the recorded lifecycle (one page whose
//! `RestEndpointIdentifier` names an api of this service, read back twice):
//!
//! > The `StatusException` a resource reports explains THAT RESOURCE'S OWN failure: a product
//! > REST endpoint page that could not be generated carries the class of the failure its own
//! > `RestEndpointIdentifier` validation hit (`exception`, "The exception") and this surface's
//! > sentence for that failure (`message`, "The error message") — the very class and sentence a
//! > write carrying an identifier that identifies no REST endpoint is refused with, since the
//! > page failed on that same validation of its own identifier. It never carries the refusal of
//! > an internal lookup the service made inside another component's records: such a refusal
//! > describes a record the caller never addressed and interpolates the account that component
//! > answers for, so a client reading the message to a user, or branching on the exception to
//! > repair the page's identifier, would be told about the wrong object.
//!
//! Both members are learned from THIS emulator's own wire behaviour rather than written out as
//! literals: each case first has a create refused for an identifier that identifies no endpoint
//! at all, and then asserts the page reports that refusal's own class and sentence, while
//! carrying neither the refusal of an internal api lookup nor this emulator's account. So the
//! cases pin the rule and not one recording's wording.
//!
//! The rows span what the rule generalises over: which control plane holds the api the
//! identifier names (an HTTP and a WEBSOCKET api of this service, each of which the account
//! really has and neither of which has REST API documentation to generate a page from), how
//! many stages that api carries and which of them is named, and the method and path the page
//! documents — which select WHICH endpoint of the api the page is of and so take no part in
//! whether its documentation exists. Every IMPLEMENTED reader of a page is asserted on, since
//! one record answers them all: the create, the single-page reader, that reader asking for the
//! raw content, and the patch. (`ListProductRestEndpointPages` is still a stub of its own and
//! is left to its own issue.)
//!
//! Every api a row names is created through the service that OWNS it, both mounted on one
//! shared engine exactly as the router mounts them, so no case passes off an identifier of its
//! own as one the account holds.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator_with_rest_apis, error_code, read_until};
use crate::product_rest_endpoint_page_lifecycle::{pages_of, product};

/// The status a page reports while its content is still being generated — the one status that
/// is not terminal, so it is what a case waits to stop seeing.
const IN_PROGRESS: &str = "IN_PROGRESS";

/// The terminal status of a page whose content could not be generated — the status the
/// modelled `StatusException` explains.
const FAILED: &str = "FAILED";

/// The modelled bounds of `StatusException$Exception` (`__stringMin1Max256`) and
/// `StatusException$Message` (`__stringMin1Max2048`), from
/// `smithy_query(query="shape", shape="StatusException")`.
const EXCEPTION_BOUNDS: (usize, usize) = (1, 256);
const MESSAGE_BOUNDS: (usize, usize) = (1, 2048);

/// The leading text of the sentence the api-level readers of this service refuse an identifier
/// they hold no api for with ("Invalid API identifier specified {account}:{apiId}",
/// `crate::state::invalid_api_identifier_message`), and the sentence the service that owns REST
/// APIs refuses an unknown REST API with. It explains a read of THAT record, so no page's
/// exception may carry it.
const INTERNAL_LOOKUP_SENTENCE: &str = "Invalid API identifier specified";

/// The account every ARN and every account-naming refusal of this emulator names. A page's
/// exception explains the page, so none of its members may carry it.
const EMULATOR_ACCOUNT: &str = "000000000000";

/// One row: the api the page's identifier names, the stages to put on it, which of them the
/// identifier names, and the method and path the page documents.
struct Row {
    case: &'static str,
    /// The `CreateApi` request that mints the api the identifier names. Such an api is one the
    /// account really has — it shares the account's api-id space — and it is not a REST API,
    /// so no REST API documentation can be generated for it.
    request: Value,
    stages: Vec<&'static str>,
    /// Which of those stages the identifier names, by index.
    stage_index: usize,
    method: &'static str,
    path: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the recorded case: an HTTP api of this service, which holds no REST API \
                   documentation",
            request: json!({ "name": "status-exception-http", "protocolType": "HTTP" }),
            stages: vec!["live"],
            stage_index: 0,
            method: "GET",
            path: "/items",
        },
        Row {
            case: "the third stage of an HTTP api of this service, not its first, and a \
                   greedy path",
            request: json!({ "name": "status-exception-stages", "protocolType": "HTTP" }),
            stages: vec!["dev", "beta", "prod"],
            stage_index: 2,
            method: "POST",
            path: "/accounts/{accountId}/orders/{proxy+}",
        },
        Row {
            case: "a WEBSOCKET api of this service",
            request: json!({
                "name": "status-exception-socket",
                "protocolType": "WEBSOCKET",
                "routeSelectionExpression": "$request.body.action"
            }),
            stages: vec!["ws"],
            stage_index: 0,
            method: "ANY",
            path: "/",
        },
    ]
}

/// The `(exception class, sentence)` this service refuses a `RestEndpointIdentifier` that
/// identifies NO REST endpoint of the account with, learned from the wire: a create naming an
/// api id nothing minted is refused, and that refusal's error code and message are the class
/// and the sentence of the identifier-validation failure a page hits on its OWN identifier —
/// the pair a page that could not be generated has to report.
async fn page_validation_failure(
    emulator: &MultiServiceRouter,
    collection: &str,
) -> (String, String) {
    let (status, refused) = call(
        emulator,
        "POST",
        collection,
        Some(json!({
            "restEndpointIdentifier": {
                "identifierParts": {
                    "restApiId": "zzzz999999",
                    "stage": "nostage",
                    "method": "GET",
                    "path": "/items"
                }
            }
        })),
    )
    .await;
    assert_eq!(
        status, 400,
        "setup: a create identifying no REST endpoint must be refused 400, got {status} \
         {refused}"
    );
    (error_code(&refused), as_str(&refused, "message"))
}

/// Create the api a row's page documents, with its stages, and answer `(apiId, the stage the
/// identifier names)` — the id this service MINTED, never one a case made up.
async fn held(emulator: &MultiServiceRouter, row: &Row) -> (String, String) {
    let (status, created) = call(emulator, "POST", "/v2/apis", Some(row.request.clone())).await;
    assert_eq!(
        status, 201,
        "case {:?}: expected 201 from CreateApi, got {status} {created}",
        row.case
    );
    let api_id = as_str(&created, "apiId");
    for stage in &row.stages {
        let (status, body) = call(
            emulator,
            "POST",
            &format!("/v2/apis/{api_id}/stages"),
            Some(json!({ "stageName": stage })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201 from CreateStage for {stage:?}, got {status} {body}",
            row.case
        );
    }
    (api_id, row.stages[row.stage_index].to_string())
}

/// Assert `page` reports the FAILED status with the `StatusException` the page's OWN failure
/// takes: `expected` is the `(class, sentence)` a write carrying an identifier that identifies
/// no REST endpoint is refused with — the page failed on exactly that validation of its own
/// identifier, so it is explained in the same class and the same words.
fn assert_page_failure(case: &str, what: &str, page: &Value, expected: &(String, String)) {
    let status = as_str(page, "status");
    assert_eq!(
        status, FAILED,
        "case {case:?}: expected {what} to report {FAILED}, got {status:?} in {page}"
    );
    let exception = page
        .pointer("/statusException/exception")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let message = page
        .pointer("/statusException/message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let (expected_exception, expected_message) = expected;
    assert_eq!(
        exception, expected_exception,
        "case {case:?}: expected {what} to name the class of the page's own identifier \
         validation failure ({expected_exception:?}), got {exception:?} in {page}"
    );
    assert_eq!(
        message, expected_message,
        "case {case:?}: expected {what} to state this surface's sentence for an identifier \
         that identifies no REST endpoint ({expected_message:?}), got {message:?} in {page}"
    );
    // Both members are within the modelled bounds, so what is reported is a value the model
    // admits rather than an empty or unbounded one.
    let (exception_min, exception_max) = EXCEPTION_BOUNDS;
    assert!(
        (exception_min..=exception_max).contains(&exception.chars().count()),
        "case {case:?}: expected {what} to populate statusException.exception within \
         {EXCEPTION_BOUNDS:?}, got {exception:?} in {page}"
    );
    let (message_min, message_max) = MESSAGE_BOUNDS;
    assert!(
        (message_min..=message_max).contains(&message.chars().count()),
        "case {case:?}: expected {what} to populate statusException.message within \
         {MESSAGE_BOUNDS:?}, got {message:?} in {page}"
    );
    // And nothing of the internal lookup the generation made travels in it: neither the
    // sentence an api-identifier read is refused with nor the account that read names.
    assert!(
        !message.contains(INTERNAL_LOOKUP_SENTENCE),
        "case {case:?}: {what} must not carry the refusal of an internal api lookup, got \
         {message:?} in {page}"
    );
    for member in [exception, message] {
        assert!(
            !member.contains(EMULATOR_ACCOUNT),
            "case {case:?}: {what} must not carry this emulator's own account, got \
             {member:?} in {page}"
        );
    }
}

/// Read `uri` until the page stopped reporting the transient status: the wait is for that
/// CONDITION, never for a fixed time.
async fn settled(emulator: &MultiServiceRouter, case: &str, uri: &str) -> Value {
    read_until(emulator, case, uri, |page| {
        page.get("status").and_then(Value::as_str) != Some(IN_PROGRESS)
    })
    .await
}

/// RULE: a page that could not be generated explains ITS OWN failure — the class of its
/// `RestEndpointIdentifier` validation failure and this surface's sentence for it — to every
/// reader of the page, and carries no refusal of an internal lookup.
#[tokio::test]
async fn a_failed_page_reports_its_own_identifier_validation_failure() {
    let emulator = emulator_with_rest_apis();
    let product_id = product(&emulator, "Explained pages").await;
    let collection = pages_of(&product_id);
    let expected = page_validation_failure(&emulator, &collection).await;

    for row in rows() {
        let case = row.case;
        let (api_id, stage) = held(&emulator, &row).await;

        // The create is SERVED: the account really has the api and stage the identifier names,
        // so what the page reports is the generation's own failure and not a refusal.
        let (status, created) = call(
            &emulator,
            "POST",
            &collection,
            Some(json!({
                "restEndpointIdentifier": {
                    "identifierParts": {
                        "restApiId": api_id,
                        "stage": stage,
                        "method": row.method,
                        "path": row.path
                    }
                }
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateProductRestEndpointPage, got {status} \
             {created}"
        );
        let page_id = as_str(&created, "productRestEndpointPageId");
        let uri = format!("{collection}/{page_id}");

        // Nothing is explained while the generation is still due: the exception travels with
        // the status that says the page could not be assembled.
        assert_eq!(
            as_str(&created, "status"),
            IN_PROGRESS,
            "case {case:?}: expected the create to report the transient status, got {created}"
        );

        // Every reader of the settled page reports one record, so each is told the same.
        let read = settled(&emulator, case, &uri).await;
        assert_page_failure(case, "GetProductRestEndpointPage", &read, &expected);

        let raw = settled(
            &emulator,
            case,
            &format!("{uri}?includeRawDisplayContent=true"),
        )
        .await;
        assert_page_failure(
            case,
            "GetProductRestEndpointPage(includeRawDisplayContent)",
            &raw,
            &expected,
        );

        let (status, patched) = call(
            &emulator,
            "PATCH",
            &uri,
            Some(json!({ "tryItState": "ENABLED" })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UpdateProductRestEndpointPage, got {status} \
             {patched}"
        );

        // A patch starts the generation over and settles it the same way, so the explanation
        // does not drift between a page's writes.
        let resettled = settled(&emulator, case, &uri).await;
        assert_page_failure(case, "the patched page", &resettled, &expected);
    }
}
