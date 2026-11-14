// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateMethodResponse`, read back through `GetMethodResponse` and
//! through the `GetMethod` that owns the response.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi`/`CreateResource`/`PutMethod`/`PutMethodResponse` helpers are this
//! crate's existing harness, reused from `rest_api_lifecycle`, `method_integration` and
//! the sibling `method_response` suite.
//!
//! The RULE under test: an `UpdateMethodResponse` applies every entry of
//! `patchOperations` to the stored `MethodResponse` its
//! `(restApiId, resourceId, httpMethod, statusCode)` names, in request order, and reports
//! the updated record — so a later read reports exactly what the patch set — while a patch
//! this resource has no updateable member for, an op such a member does not support, or a
//! value outside the member's type is refused with a modelled error and changes nothing.
//! The two patchable members are the record's caller-supplied maps: `responseParameters`,
//! keyed by a `method.response.header.{name}` mapping expression with a boolean value, and
//! `responseModels`, keyed by a content type with a `Model` name as its value
//! (`smithy_query(query="operation", operation="UpdateMethodResponse")` types them
//! `MapOfStringToBoolean` and `MapOfStringToString`); `statusCode` is the record's
//! identity and is not patchable.
//!
//! The tables span what the rule generalises over: which map the path names, the entry's
//! type (boolean, string), the op (`add`, `replace`, `remove`), how many ops one request
//! carries, a key carrying an escaped `/` pointer token, the status code (values across
//! the modelled `^[1-5]\d\d$` pattern and each published bound), the method's verb and a
//! spelling of it the resource holds no method under, and which of the four identifiers is
//! the unknown one.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::put_method_response::invalid_response_parameter;

use crate::method_integration::{get_method, resource_with_a_method};
use crate::method_response::{get_method_response, put_method_response, response_header};
use crate::rest_api_lifecycle::{assert_failure, create_api, Answer, Gateway};

/// One `patchOperations` entry, as a client spells it on the wire.
fn op(kind: &str, path: &str, value: Option<&str>) -> Value {
    match value {
        Some(value) => json!({ "op": kind, "path": path, "value": value }),
        None => json!({ "op": kind, "path": path }),
    }
}

/// A `replace` entry, the op a client issues most.
fn replace(path: &str, value: &str) -> Value {
    op("replace", path, Some(value))
}

/// `UpdateMethodResponse` for `status` on the `verb` method of `resource`.
async fn update_method_response(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    status: &str,
    operations: Vec<Value>,
) -> Answer {
    gateway
        .call(
            "PATCH",
            &format!("/restapis/{api}/resources/{resource}/methods/{verb}/responses/{status}"),
            Some(json!({ "patchOperations": Value::Array(operations) })),
        )
        .await
}

/// Send `operations`, asserting the modelled 201 (`http` code of
/// `smithy_query(query="operation", operation="UpdateMethodResponse")`) and returning the
/// reported body.
async fn update_ok(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    status: &str,
    case: &str,
    operations: Vec<Value>,
) -> Value {
    let answer =
        update_method_response(gateway, api, resource, verb, status, operations).await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected the modelled 201, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// Create a method response at `status`, asserting the modelled 201 of `PutMethodResponse`.
async fn seed(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    status: &str,
    request: Value,
) {
    let created = put_method_response(gateway, api, resource, verb, status, request).await;
    assert_eq!(
        created.status, 201,
        "case PutMethodResponse({verb}/{status}): expected 201, got {} with {:?}",
        created.status, created.body
    );
}

/// Assert the updated record is what a later reader sees: `GetMethodResponse` and the
/// `methodResponses` of the owning `GetMethod` both report `expected`.
async fn assert_read_back(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    status: &str,
    case: &str,
    expected: &Value,
) {
    let read = get_method_response(gateway, api, resource, verb, status).await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected 200 from GetMethodResponse, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        &read.body, expected,
        "case {case}: GetMethodResponse must report the patched response {expected:?}, got {:?}",
        read.body
    );
    let method = get_method(gateway, api, resource, verb).await;
    assert_eq!(
        &method.body["methodResponses"][status], expected,
        "case {case}: the owning method must carry the patched response at {status}, got {:?}",
        method.body["methodResponses"]
    );
}

#[tokio::test]
async fn a_patched_method_response_member_is_stored_and_read_back() {
    // Rows differ along what the rule generalises over: which of the two maps the path
    // names, the entry's type (a boolean flag, a model name), the op, how many ops one
    // request carries, a key whose `/` is an escaped pointer token, and emptying a map
    // down to its last entry. Each row's expectation is the WHOLE record, so a patch that
    // changed anything else would fail the row.
    let cases: Vec<(&str, Vec<Value>, Value)> = vec![
        (
            "add one required response header",
            vec![op(
                "add",
                &format!("/responseParameters/{}", response_header("Content-Type")),
                Some("true"),
            )],
            json!({
                "statusCode": "200",
                "responseParameters": { response_header("Content-Type"): true }
            }),
        ),
        (
            "replace that header's flag with optional",
            vec![replace(
                &format!("/responseParameters/{}", response_header("Content-Type")),
                "false",
            )],
            json!({
                "statusCode": "200",
                "responseParameters": { response_header("Content-Type"): false }
            }),
        ),
        (
            "add two more headers in one request",
            vec![
                op(
                    "add",
                    &format!("/responseParameters/{}", response_header("Location")),
                    Some("true"),
                ),
                op(
                    "add",
                    &format!("/responseParameters/{}", response_header("X-Request-Id")),
                    Some("false"),
                ),
            ],
            json!({
                "statusCode": "200",
                "responseParameters": {
                    response_header("Content-Type"): false,
                    response_header("Location"): true,
                    response_header("X-Request-Id"): false
                }
            }),
        ),
        (
            "remove one header",
            vec![op(
                "remove",
                &format!("/responseParameters/{}", response_header("Location")),
                None,
            )],
            json!({
                "statusCode": "200",
                "responseParameters": {
                    response_header("Content-Type"): false,
                    response_header("X-Request-Id"): false
                }
            }),
        ),
        (
            "add a response model under an escaped content type",
            vec![op("add", "/responseModels/application~1json", Some("Empty"))],
            json!({
                "statusCode": "200",
                "responseParameters": {
                    response_header("Content-Type"): false,
                    response_header("X-Request-Id"): false
                },
                "responseModels": { "application/json": "Empty" }
            }),
        ),
        (
            "replace that response model",
            vec![replace("/responseModels/application~1json", "Pet")],
            json!({
                "statusCode": "200",
                "responseParameters": {
                    response_header("Content-Type"): false,
                    response_header("X-Request-Id"): false
                },
                "responseModels": { "application/json": "Pet" }
            }),
        ),
        (
            "patch both maps in one request",
            vec![
                op("add", "/responseModels/text~1plain", Some("Empty")),
                replace(
                    &format!("/responseParameters/{}", response_header("Content-Type")),
                    "true",
                ),
            ],
            json!({
                "statusCode": "200",
                "responseParameters": {
                    response_header("Content-Type"): true,
                    response_header("X-Request-Id"): false
                },
                "responseModels": { "application/json": "Pet", "text/plain": "Empty" }
            }),
        ),
        (
            "remove every response model",
            vec![
                op("remove", "/responseModels/application~1json", None),
                op("remove", "/responseModels/text~1plain", None),
            ],
            // An emptied caller-supplied map is reported absent, the way
            // `PutMethodResponse` reports one the request left empty.
            json!({
                "statusCode": "200",
                "responseParameters": {
                    response_header("Content-Type"): true,
                    response_header("X-Request-Id"): false
                }
            }),
        ),
        (
            "remove every response parameter too",
            vec![
                op(
                    "remove",
                    &format!("/responseParameters/{}", response_header("Content-Type")),
                    None,
                ),
                op(
                    "remove",
                    &format!("/responseParameters/{}", response_header("X-Request-Id")),
                    None,
                ),
            ],
            json!({ "statusCode": "200" }),
        ),
        (
            "removing a key the record never held changes nothing",
            vec![op(
                "remove",
                &format!("/responseParameters/{}", response_header("Never-Set")),
                None,
            )],
            json!({ "statusCode": "200" }),
        ),
    ];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "update-method-response-round-trip").await;
    let resource = resource_with_a_method(&gateway, &api, &root, "pets", "GET", json!({})).await;
    seed(&gateway, &api, &resource, "GET", "200", json!({})).await;

    for (case, operations, expected) in &cases {
        let updated = update_ok(
            &gateway,
            &api,
            &resource,
            "GET",
            "200",
            case,
            operations.clone(),
        )
        .await;
        assert_eq!(
            &updated, expected,
            "case {case}: UpdateMethodResponse must report {expected:?}, got {updated:?}"
        );
        assert_read_back(&gateway, &api, &resource, "GET", "200", case, expected).await;
    }
}

#[tokio::test]
async fn the_patch_rule_holds_for_every_status_code_and_verb() {
    // Rows differ along the status code — values across the modelled `^[1-5]\d\d$`
    // pattern, including its lowest and highest — the verb of the owning method, and
    // whether a spelling the resource holds no method under is probed first: a verb is
    // the identifier the request names, so another spelling of it reaches no record.
    let rows: Vec<(&str, &str, bool)> = vec![
        ("100", "GET", false),
        ("201", "POST", true),
        ("302", "PUT", true),
        ("404", "HEAD", true),
        ("500", "DELETE", false),
        ("599", "ANY", true),
    ];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "update-method-response-codes").await;

    for (status, verb, probe_other_spelling) in &rows {
        let resource = resource_with_a_method(
            &gateway,
            &api,
            &root,
            &format!("r{status}-{verb}"),
            verb,
            json!({}),
        )
        .await;
        seed(
            &gateway,
            &api,
            &resource,
            verb,
            status,
            json!({ "responseModels": { "application/json": "Empty" } }),
        )
        .await;

        let case = format!("UpdateMethodResponse({verb}/{status})");
        let key = response_header("X-Status");
        if *probe_other_spelling {
            let lowercase = verb.to_ascii_lowercase();
            let answer = update_method_response(
                &gateway,
                &api,
                &resource,
                &lowercase,
                status,
                vec![replace("/responseModels/application~1json", "Pet")],
            )
            .await;
            assert_failure(
                &answer,
                404,
                "NotFoundException",
                &format!("UpdateMethodResponse({lowercase}/{status})"),
            );
        }
        let updated = update_ok(
            &gateway,
            &api,
            &resource,
            verb,
            status,
            &case,
            vec![
                op("add", &format!("/responseParameters/{key}"), Some("true")),
                replace("/responseModels/application~1json", "Pet"),
            ],
        )
        .await;
        let expected = json!({
            "statusCode": status,
            "responseParameters": { key.clone(): true },
            "responseModels": { "application/json": "Pet" }
        });
        assert_eq!(
            updated, expected,
            "case {case}: expected {expected:?}, got {updated:?}"
        );
        // The patch does not move the record: it is still the response of THAT status
        // code, read back under the verb's own spelling.
        assert_read_back(&gateway, &api, &resource, verb, status, &case, &expected).await;
    }
}

#[tokio::test]
async fn a_patch_this_operation_cannot_apply_is_refused_and_changes_nothing() {
    // The baseline record every refusal is checked against.
    let baseline = json!({
        "statusCode": "200",
        "responseParameters": { response_header("Content-Type"): true },
        "responseModels": { "application/json": "Empty" }
    });
    let header = response_header("Content-Type");

    // Rows span the kinds of patch this resource cannot apply: a member it does not have,
    // its own identity, a map path naming no key, an op a map entry does not support, a
    // value outside the entry's type, an add carrying no value at all, and a response
    // parameter key outside the documented `method.response.header.{name}` form.
    let refused: Vec<(&str, Vec<Value>)> = vec![
        (
            "a member the resource does not have",
            vec![replace("/selectionPattern", "2\\d\\d")],
        ),
        (
            "the response's own status code",
            vec![replace("/statusCode", "201")],
        ),
        (
            "a sub-resource of the owning method",
            vec![replace("/methodIntegration/type", "MOCK")],
        ),
        (
            "a responseParameters path naming no key",
            vec![replace("/responseParameters", "true")],
        ),
        (
            "a responseModels path naming no key",
            vec![replace("/responseModels", "Empty")],
        ),
        (
            "a move of a response parameter",
            vec![json!({
                "op": "move",
                "from": format!("/responseParameters/{header}"),
                "path": format!("/responseParameters/{}", response_header("X-Moved"))
            })],
        ),
        (
            "a copy of a response model",
            vec![json!({
                "op": "copy",
                "from": "/responseModels/application~1json",
                "path": "/responseModels/text~1plain"
            })],
        ),
        (
            "a test of a response model",
            vec![op("test", "/responseModels/application~1json", Some("Empty"))],
        ),
        (
            "a non-boolean response parameter flag",
            vec![replace(&format!("/responseParameters/{header}"), "maybe")],
        ),
        (
            "an add of a response model with no value",
            vec![op("add", "/responseModels/text~1plain", None)],
        ),
        (
            "a path that is not a JSON Pointer",
            vec![replace("responseModels/application~1json", "Pet")],
        ),
        (
            "a bare header name as a response parameter key",
            vec![op("add", "/responseParameters/Content-Type", Some("true"))],
        ),
        (
            "a request-side mapping expression",
            vec![op(
                "add",
                "/responseParameters/method.request.header.Authorization",
                Some("true"),
            )],
        ),
        (
            "an integration-side mapping expression",
            vec![op(
                "add",
                "/responseParameters/integration.response.header.X-Custom",
                Some("true"),
            )],
        ),
        (
            "an empty header name",
            vec![op(
                "add",
                &format!("/responseParameters/{}", response_header("")),
                Some("true"),
            )],
        ),
        (
            "a header name outside the HTTP field-name token",
            vec![op(
                "add",
                &format!("/responseParameters/{}", response_header("X:Custom")),
                Some("true"),
            )],
        ),
        (
            // The ops before the refused one are not kept: the list is applied to a copy.
            "a valid op followed by a refused one",
            vec![
                op("add", "/responseModels/text~1plain", Some("Empty")),
                replace("/statusCode", "201"),
            ],
        ),
    ];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "update-method-response-refusals").await;
    let resource = resource_with_a_method(&gateway, &api, &root, "pets", "GET", json!({})).await;
    seed(
        &gateway,
        &api,
        &resource,
        "GET",
        "200",
        json!({
            "responseParameters": { header.clone(): true },
            "responseModels": { "application/json": "Empty" }
        }),
    )
    .await;

    for (case, operations) in &refused {
        let answer =
            update_method_response(&gateway, &api, &resource, "GET", "200", operations.clone())
                .await;
        assert_failure(&answer, 400, "BadRequestException", case);
        assert_read_back(&gateway, &api, &resource, "GET", "200", case, &baseline).await;
    }

    // The refusal for a path the resource has no updateable member for is the service's
    // shared `patchOperations` wording: the fixed sentence and the path the request sent.
    let answer =
        update_method_response(&gateway, &api, &resource, "GET", "200", vec![replace("/statusCode", "201")])
            .await;
    let message = answer.body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case the unsupported path: expected a message, got {:?}", answer.body))
        .to_owned();
    assert_eq!(
        message, "Invalid patch path /statusCode",
        "case the unsupported path: expected the fixed sentence and the path, got {message:?}"
    );

    // An `op` outside the modelled `Op` enum is a CONSTRAINT failure, not a patch this
    // resource cannot apply, so the boundary validator refuses it before the handler runs
    // — the service-wide rule the `validation_message_rules` suite pins.
    let answer = update_method_response(
        &gateway,
        &api,
        &resource,
        "GET",
        "200",
        vec![op("upsert", "/responseModels/text~1plain", Some("Empty"))],
    )
    .await;
    assert_failure(&answer, 400, "ValidationException", "an op nothing names");
    assert_read_back(
        &gateway,
        &api,
        &resource,
        "GET",
        "200",
        "an op nothing names",
        &baseline,
    )
    .await;

    // A response parameter key outside the documented form is stated the way
    // `PutMethodResponse` states it, so one rule has one wording.
    let answer = update_method_response(
        &gateway,
        &api,
        &resource,
        "GET",
        "200",
        vec![op("add", "/responseParameters/Content-Type", Some("true"))],
    )
    .await;
    assert_eq!(
        answer.body["message"].as_str(),
        Some(invalid_response_parameter("Content-Type").as_str()),
        "case a bare header name: expected the shared mapping-expression refusal, got {:?}",
        answer.body["message"]
    );
}

#[tokio::test]
async fn an_unknown_identifier_is_a_not_found_and_patches_nothing() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "update-method-response-notfound").await;
    let resource = resource_with_a_method(&gateway, &api, &root, "pets", "GET", json!({})).await;
    seed(&gateway, &api, &resource, "GET", "200", json!({})).await;
    let missing = "zzzzzzzzzz";

    // Rows span each of the four identifiers this operation resolves, one unknown at a
    // time: the API, the resource, the verb naming no method on the resource, a verb
    // outside the catalog, and a status code naming no response of the method.
    let cases: Vec<(&str, String)> = vec![
        (
            "an unknown restApiId",
            format!("/restapis/{missing}/resources/{resource}/methods/GET/responses/200"),
        ),
        (
            "an unknown resourceId",
            format!("/restapis/{api}/resources/{missing}/methods/GET/responses/200"),
        ),
        (
            "a verb naming no method",
            format!("/restapis/{api}/resources/{resource}/methods/DELETE/responses/200"),
        ),
        (
            "a verb outside the catalog",
            format!("/restapis/{api}/resources/{resource}/methods/TRACE/responses/200"),
        ),
        (
            "a status code naming no response",
            format!("/restapis/{api}/resources/{resource}/methods/GET/responses/404"),
        ),
    ];
    for (case, uri) in &cases {
        let answer = gateway
            .call(
                "PATCH",
                uri,
                Some(json!({ "patchOperations": [replace("/responseModels/application~1json", "Empty")] })),
            )
            .await;
        assert_failure(&answer, 404, "NotFoundException", case);
    }

    // A status code the modelled `StatusCode` pattern `^[1-5]\d\d$` does not admit — one
    // past each published bound, and the lengths it refuses — never reaches the records:
    // the CONSTRAINT layer refuses it with a 400 under its own error class before the
    // handler runs.
    for status in ["099", "600", "000", "999", "20", "2000", "abc"] {
        assert_failure(
            &update_method_response(
                &gateway,
                &api,
                &resource,
                "GET",
                status,
                vec![op("add", "/responseModels/text~1plain", Some("Empty"))],
            )
            .await,
            400,
            "ValidationException",
            &format!("UpdateMethodResponse({status})"),
        );
    }

    // Every refused call patched nothing: the seeded response is still the bare record.
    assert_read_back(
        &gateway,
        &api,
        &resource,
        "GET",
        "200",
        "after the refused patches",
        &json!({ "statusCode": "200" }),
    )
    .await;
}

#[tokio::test]
async fn a_patch_changes_only_the_response_its_four_identifiers_name() {
    let gateway = Gateway::new();
    let (first_api, first_root) = create_api(&gateway, "update-method-response-scope-a").await;
    let (second_api, second_root) = create_api(&gateway, "update-method-response-scope-b").await;
    let first_resource =
        resource_with_a_method(&gateway, &first_api, &first_root, "pets", "GET", json!({})).await;
    let second_resource = resource_with_a_method(
        &gateway,
        &first_api,
        &first_root,
        "owners",
        "GET",
        json!({}),
    )
    .await;
    let other_api_resource = resource_with_a_method(
        &gateway,
        &second_api,
        &second_root,
        "pets",
        "GET",
        json!({}),
    )
    .await;
    // A second verb on the first resource, so the verb varies too.
    let answer = gateway
        .call(
            "PUT",
            &format!("/restapis/{first_api}/resources/{first_resource}/methods/POST"),
            Some(json!({ "authorizationType": "NONE" })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case PutMethod(POST): expected 201, got {} with {:?}",
        answer.status, answer.body
    );

    // Each row is one (api, resource, verb) holding a 200 and a 404 response; only the
    // first row's 200 is patched, so every other record must stand.
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        ("the named response", &first_api, &first_resource, "GET"),
        (
            "another verb, same resource",
            &first_api,
            &first_resource,
            "POST",
        ),
        (
            "another resource, same API",
            &first_api,
            &second_resource,
            "GET",
        ),
        ("another API", &second_api, &other_api_resource, "GET"),
    ];
    for (_, api, resource, verb) in &rows {
        for status in ["200", "404"] {
            seed(&gateway, api, resource, verb, status, json!({})).await;
        }
    }

    let patched = update_ok(
        &gateway,
        &first_api,
        &first_resource,
        "GET",
        "200",
        "the named response",
        vec![op("add", "/responseModels/application~1json", Some("Pet"))],
    )
    .await;
    assert_eq!(
        patched["responseModels"],
        json!({ "application/json": "Pet" }),
        "case the named response: expected the patched model, got {patched:?}"
    );

    for (index, (case, api, resource, verb)) in rows.iter().enumerate() {
        for status in ["200", "404"] {
            let read = get_method_response(&gateway, api, resource, verb, status).await;
            let expected = if index == 0 && status == "200" {
                json!({ "statusCode": "200", "responseModels": { "application/json": "Pet" } })
            } else {
                json!({ "statusCode": status })
            };
            assert_eq!(
                read.body, expected,
                "case {case}/{status}: expected {expected:?}, got {:?}",
                read.body
            );
        }
    }
}
