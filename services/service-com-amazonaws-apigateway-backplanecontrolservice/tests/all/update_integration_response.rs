// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateIntegrationResponse`, read back through
//! `GetIntegrationResponse` and through the `GetIntegration` that owns the response.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi`/`CreateResource`/`PutMethod`/`PutIntegration`/`PutIntegrationResponse`
//! helpers are this crate's existing harness, reused from `rest_api_lifecycle`,
//! `method_integration`, `method_response` and the sibling `integration_response` suite.
//!
//! The RULE under test: an `UpdateIntegrationResponse` applies every entry of
//! `patchOperations` to the stored `IntegrationResponse` its
//! `(restApiId, resourceId, httpMethod, statusCode)` names, in request order, and reports
//! the updated record — so a later read reports exactly what the patch set — while a patch
//! naming a member the resource has no updateable member for, an op such a member does not
//! support, or a value outside the member's own documented form is refused with a modelled
//! error and changes nothing. The updateable members are the record's
//! `selectionPattern`, its `contentHandling` and its two caller-supplied maps
//! `responseParameters` and `responseTemplates`
//! (`smithy_query(query="operation", operation="UpdateIntegrationResponse")` types them
//! `String`, `ContentHandlingStrategy` and `MapOfStringToString`); `statusCode` is the
//! record's identity — the URI label addressing it — and is not patchable.
//!
//! The tables span what the rule generalises over: which member the path names, the entry's
//! kind (single-valued string, enum, map entry), the op (`add`, `replace`, `remove`), how
//! many ops one request carries, a map key carrying an escaped `/` pointer token, the
//! status code (values across the modelled `^[1-5]\d\d$` pattern, each published bound
//! included), the method's verb and the spellings of it the resource holds no method
//! under, and which of the four
//! identifiers is the unknown one.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::put_integration_response::{
    invalid_integration_response_mapping, INTEGRATION_RESPONSE_BODY_PREFIX,
    INTEGRATION_RESPONSE_HEADER_PREFIX,
};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::put_method_response::invalid_response_parameter;
use service_com_amazonaws_apigateway_backplanecontrolservice::state::invalid_patch_path;

use crate::integration_response::{
    get_integration_response, put_integration_response, resource_with_an_integration,
};
use crate::method_integration::get_integration;
use crate::method_response::response_header;
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

/// An `integration.response.header.{name}` mapping expression, built from the prefix the
/// handler publishes rather than spelled out per case.
fn integration_header(name: &str) -> String {
    format!("{INTEGRATION_RESPONSE_HEADER_PREFIX}{name}")
}

/// An `integration.response.body.{JSON-expression}` mapping expression; see
/// [`integration_header`].
fn integration_body(expression: &str) -> String {
    format!("{INTEGRATION_RESPONSE_BODY_PREFIX}{expression}")
}

/// `UpdateIntegrationResponse` for `status` on the integration of the `verb` method of
/// `resource`, with `verb` sent exactly as the caller spelled it.
async fn update_integration_response(
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
            &format!(
                "/restapis/{api}/resources/{resource}/methods/{verb}/integration/responses/{status}"
            ),
            Some(json!({ "patchOperations": Value::Array(operations) })),
        )
        .await
}

/// Send `operations`, asserting the modelled 200 (the `http` code of
/// `smithy_query(query="operation", operation="UpdateIntegrationResponse")`) and returning
/// the reported body.
async fn update_ok(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    status: &str,
    case: &str,
    operations: Vec<Value>,
) -> Value {
    let answer = update_integration_response(gateway, api, resource, verb, status, operations).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected the modelled 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// Create the integration response `status` names, asserting the modelled 201 of
/// `PutIntegrationResponse`: a patch needs a stored record to apply to.
async fn seed(gateway: &Gateway, api: &str, resource: &str, verb: &str, status: &str, body: Value) {
    let created = put_integration_response(gateway, api, resource, verb, status, body).await;
    assert_eq!(
        created.status, 201,
        "case PutIntegrationResponse({verb}/{status}): expected 201, got {} with {:?}",
        created.status, created.body
    );
}

/// Assert the updated record is what a later reader sees: `GetIntegrationResponse` and the
/// `integrationResponses` of the owning `GetIntegration` both report `expected`.
async fn assert_read_back(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    status: &str,
    case: &str,
    expected: &Value,
) {
    let read = get_integration_response(gateway, api, resource, verb, status).await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected 200 from GetIntegrationResponse, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        &read.body, expected,
        "case {case}: GetIntegrationResponse must report the updated response {expected:?}, got {:?}",
        read.body
    );
    let integration = get_integration(gateway, api, resource, verb).await;
    assert_eq!(
        &integration.body["integrationResponses"][status], expected,
        "case {case}: the owning integration must carry {expected:?} at {status}, got {:?}",
        integration.body["integrationResponses"]
    );
}

#[tokio::test]
async fn a_patched_integration_response_is_reported_and_read_back_member_for_member() {
    // Rows differ along what the rule generalises over: which member the path names, the
    // op, how many ops one request carries, whether the map entry is a new key or one the
    // record already held, a key carrying an escaped `/` pointer token, the status code
    // (values across the modelled `^[1-5]\d\d$` pattern, its published bounds included)
    // and the method's verb.
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, &str, &str, &str, Value, Vec<Value>, Value)> = vec![
        (
            "replace the template the record already holds for a content type",
            "replace-template",
            "GET",
            "200",
            json!({ "responseTemplates": { "application/json": "{\"old\": true}" } }),
            vec![replace(
                "/responseTemplates/application~1json",
                "{\"new\": true}",
            )],
            json!({
                "statusCode": "200",
                "responseTemplates": { "application/json": "{\"new\": true}" }
            }),
        ),
        (
            "add a template for a second content type",
            "add-template",
            "POST",
            "201",
            json!({ "responseTemplates": { "application/json": "{}" } }),
            vec![op(
                "add",
                "/responseTemplates/text~1plain",
                Some("$input.body"),
            )],
            json!({
                "statusCode": "201",
                "responseTemplates": { "application/json": "{}", "text/plain": "$input.body" }
            }),
        ),
        (
            "remove the record's last template, which drops the whole map",
            "remove-template",
            "PUT",
            "100",
            json!({ "responseTemplates": { "application/json": "{}" } }),
            vec![op("remove", "/responseTemplates/application~1json", None)],
            json!({ "statusCode": "100" }),
        ),
        (
            "replace the selection pattern",
            "replace-pattern",
            "DELETE",
            "400",
            json!({ "selectionPattern": "4\\d{2}" }),
            vec![replace("/selectionPattern", "4\\d\\d")],
            json!({ "statusCode": "400", "selectionPattern": "4\\d\\d" }),
        ),
        (
            "remove the selection pattern, making this the default response",
            "remove-pattern",
            "HEAD",
            "500",
            json!({ "selectionPattern": "5\\d{2}" }),
            vec![op("remove", "/selectionPattern", None)],
            json!({ "statusCode": "500" }),
        ),
        (
            "add a selection pattern the record did not carry",
            "add-pattern",
            "OPTIONS",
            "599",
            json!({}),
            vec![op("add", "/selectionPattern", Some(".+"))],
            json!({ "statusCode": "599", "selectionPattern": ".+" }),
        ),
        (
            "set the content handling and then replace it, in request order",
            "content-handling",
            "PATCH",
            "199",
            json!({}),
            vec![
                op("add", "/contentHandling", Some("CONVERT_TO_TEXT")),
                replace("/contentHandling", "CONVERT_TO_BINARY"),
            ],
            json!({ "statusCode": "199", "contentHandling": "CONVERT_TO_BINARY" }),
        ),
        (
            "remove the content handling, passing the payload through unmodified",
            "remove-content-handling",
            "ANY",
            "302",
            json!({ "contentHandling": "CONVERT_TO_BINARY" }),
            vec![op("remove", "/contentHandling", None)],
            json!({ "statusCode": "302" }),
        ),
        (
            "add a response parameter mapping a static value",
            "static-parameter",
            "GET",
            "204",
            json!({}),
            vec![op(
                "add",
                &format!("/responseParameters/{}", response_header("X-Static")),
                Some("'fixed'"),
            )],
            json!({
                "statusCode": "204",
                "responseParameters": { response_header("X-Static"): "'fixed'" }
            }),
        ),
        (
            "replace a response parameter with an integration body expression",
            "body-parameter",
            "POST",
            "303",
            json!({
                "responseParameters": {
                    response_header("Location"): integration_header("Location")
                }
            }),
            vec![replace(
                &format!("/responseParameters/{}", response_header("Location")),
                &integration_body("redirect.url"),
            )],
            json!({
                "statusCode": "303",
                "responseParameters": {
                    response_header("Location"): integration_body("redirect.url")
                }
            }),
        ),
        (
            "remove one response parameter and leave the other",
            "remove-parameter",
            "PUT",
            "404",
            json!({
                "responseParameters": {
                    response_header("X-Kept"): "'kept'",
                    response_header("X-Dropped"): "'dropped'"
                }
            }),
            vec![op(
                "remove",
                &format!("/responseParameters/{}", response_header("X-Dropped")),
                None,
            )],
            json!({
                "statusCode": "404",
                "responseParameters": { response_header("X-Kept"): "'kept'" }
            }),
        ),
        (
            "one request touching every updateable member at once",
            "every-member",
            "DELETE",
            "502",
            json!({
                "selectionPattern": "5\\d{2}",
                "responseTemplates": { "application/json": "{\"old\": true}" }
            }),
            vec![
                replace("/selectionPattern", "502"),
                replace("/responseTemplates/application~1json", "{\"new\": true}"),
                op("add", "/responseTemplates/text~1plain", Some("error")),
                op("add", "/contentHandling", Some("CONVERT_TO_TEXT")),
                op(
                    "add",
                    &format!("/responseParameters/{}", response_header("Retry-After")),
                    Some(&integration_header("Retry-After")),
                ),
            ],
            json!({
                "statusCode": "502",
                "selectionPattern": "502",
                "contentHandling": "CONVERT_TO_TEXT",
                "responseTemplates": {
                    "application/json": "{\"new\": true}",
                    "text/plain": "error"
                },
                "responseParameters": {
                    response_header("Retry-After"): integration_header("Retry-After")
                }
            }),
        ),
        (
            "a request carrying no operations at all reports the record unchanged",
            "no-operations",
            "GET",
            "301",
            json!({ "selectionPattern": "3\\d{2}" }),
            vec![],
            json!({ "statusCode": "301", "selectionPattern": "3\\d{2}" }),
        ),
    ];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "update-integration-response-round-trip").await;

    for (case, path_part, verb, status, seeded, operations, expected) in &cases {
        let resource = resource_with_an_integration(&gateway, &api, &root, path_part, verb).await;
        seed(&gateway, &api, &resource, verb, status, seeded.clone()).await;

        let reported = update_ok(
            &gateway,
            &api,
            &resource,
            verb,
            status,
            case,
            operations.clone(),
        )
        .await;
        assert_eq!(
            &reported, expected,
            "case {case}: expected the updated response {expected:?}, got {reported:?}"
        );
        assert_read_back(&gateway, &api, &resource, verb, status, case, expected).await;
    }
}

#[tokio::test]
async fn an_integration_response_is_reached_only_by_the_verb_its_method_was_put_under() {
    // A method's verb is the IDENTIFIER the request names, so a patch reaches the record
    // only under the spelling the method was created with — the rule
    // `PutIntegrationResponse` and `GetIntegrationResponse` already apply to the same
    // label.
    let unresolved = ["get", "Get", "gEt", "POST"];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "update-integration-response-verb-case").await;
    let resource = resource_with_an_integration(&gateway, &api, &root, "pets", "GET").await;
    seed(&gateway, &api, &resource, "GET", "200", json!({})).await;
    let patched = json!({
        "statusCode": "200",
        "responseTemplates": { "application/json": "{\"verb\": \"GET\"}" }
    });
    let reported = update_ok(
        &gateway,
        &api,
        &resource,
        "GET",
        "200",
        "a patch sent with the verb the method was put under",
        vec![replace(
            "/responseTemplates/application~1json",
            "{\"verb\": \"GET\"}",
        )],
    )
    .await;
    assert_eq!(
        reported, patched,
        "case a patch sent with the verb the method was put under: expected {patched:?}, got \
         {reported:?}"
    );

    for spelling in unresolved {
        let case = format!("a patch sent with the verb spelled {spelling:?}");
        let answer = update_integration_response(
            &gateway,
            &api,
            &resource,
            spelling,
            "200",
            vec![replace(
                "/responseTemplates/application~1json",
                &format!("{{\"verb\": \"{spelling}\"}}"),
            )],
        )
        .await;
        assert_failure(&answer, 404, "NotFoundException", &case);
        // Refused, so the record the method DOES hold is the patched one, unchanged.
        assert_read_back(&gateway, &api, &resource, "GET", "200", &case, &patched).await;
    }
}

#[tokio::test]
async fn a_patch_this_record_cannot_take_is_refused_and_changes_nothing() {
    // Rows differ along WHY the patch cannot be applied: a member the record has no
    // updateable member for, the record's own identity, a pointer reaching into a
    // single-valued member, a map member named without a key, an op such a member does not
    // support, an add or replace carrying no value, a value outside a modelled enum's
    // value set, and each side of a response parameter mapping outside its documented
    // form.
    let seeded = json!({
        "selectionPattern": "2\\d{2}",
        "contentHandling": "CONVERT_TO_TEXT",
        "responseTemplates": { "application/json": "{\"kept\": true}" },
        "responseParameters": { response_header("X-Kept"): "'kept'" }
    });
    let stored = json!({
        "statusCode": "200",
        "selectionPattern": "2\\d{2}",
        "contentHandling": "CONVERT_TO_TEXT",
        "responseTemplates": { "application/json": "{\"kept\": true}" },
        "responseParameters": { response_header("X-Kept"): "'kept'" }
    });

    let bad_parameter_key = "X-No-Prefix";
    let bad_parameter_value = "integration.response.cookie.Session";
    let cases: Vec<(&str, Vec<Value>, String)> = vec![
        (
            "a member this resource does not have",
            vec![replace("/notAMember", "x")],
            invalid_patch_path("/notAMember"),
        ),
        (
            "the record's own identity, which the URI addresses it by",
            vec![replace("/statusCode", "201")],
            invalid_patch_path("/statusCode"),
        ),
        (
            "a pointer reaching into a single-valued member",
            vec![replace("/selectionPattern/first", "x")],
            invalid_patch_path("/selectionPattern/first"),
        ),
        (
            "the templates map named without a content type",
            vec![replace("/responseTemplates", "{}")],
            invalid_patch_path("/responseTemplates"),
        ),
        (
            "the parameters map named without a header",
            vec![replace("/responseParameters", "'x'")],
            invalid_patch_path("/responseParameters"),
        ),
        (
            "a move, which names nothing on these members",
            vec![json!({
                "op": "move",
                "from": "/responseTemplates/application~1json",
                "path": "/responseTemplates/text~1plain"
            })],
            "Invalid patch operation specified. Must be 'add'|'remove'|'replace'".to_string(),
        ),
        (
            "a copy of a template entry",
            vec![json!({
                "op": "copy",
                "from": "/responseTemplates/application~1json",
                "path": "/responseTemplates/text~1plain"
            })],
            "Invalid patch operation specified. Must be 'add'|'remove'|'replace'".to_string(),
        ),
        (
            "a test of the selection pattern",
            vec![op("test", "/selectionPattern", Some("2\\d{2}"))],
            "Invalid patch operation specified. Must be 'add'|'remove'|'replace'".to_string(),
        ),
        (
            "a replace of a template carrying no value",
            vec![op("replace", "/responseTemplates/application~1json", None)],
            "a replace of /responseTemplates/application~1json requires a value".to_string(),
        ),
        (
            "a content handling outside the modelled value set",
            vec![replace("/contentHandling", "CONVERT_TO_XML")],
            "/contentHandling takes one of CONVERT_TO_BINARY, CONVERT_TO_TEXT, got \"CONVERT_TO_XML\""
                .to_string(),
        ),
        (
            "a response parameter key that is not a method response header",
            vec![op(
                "add",
                &format!("/responseParameters/{bad_parameter_key}"),
                Some("'x'"),
            )],
            invalid_response_parameter(bad_parameter_key),
        ),
        (
            "a response parameter value in none of the documented forms",
            vec![op(
                "add",
                &format!("/responseParameters/{}", response_header("X-New")),
                Some(bad_parameter_value),
            )],
            invalid_integration_response_mapping(bad_parameter_value),
        ),
        (
            "a list whose LAST op is refused, leaving the ops before it unapplied",
            vec![
                replace("/responseTemplates/application~1json", "{\"new\": true}"),
                replace("/contentHandling", "CONVERT_TO_XML"),
            ],
            "/contentHandling takes one of CONVERT_TO_BINARY, CONVERT_TO_TEXT, got \"CONVERT_TO_XML\""
                .to_string(),
        ),
    ];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "update-integration-response-refusals").await;
    let resource = resource_with_an_integration(&gateway, &api, &root, "pets", "GET").await;
    seed(&gateway, &api, &resource, "GET", "200", seeded).await;

    for (case, operations, message) in &cases {
        let answer =
            update_integration_response(&gateway, &api, &resource, "GET", "200", operations.clone())
                .await;
        assert_failure(&answer, 400, "BadRequestException", case);
        assert_eq!(
            answer.body["message"].as_str(),
            Some(message.as_str()),
            "case {case}: expected the message {message:?}, got {:?}",
            answer.body["message"]
        );
        // A refused patch changes nothing: the record a later reader sees is the one that
        // was stored before the call.
        assert_read_back(&gateway, &api, &resource, "GET", "200", case, &stored).await;
    }
}

#[tokio::test]
async fn a_patch_naming_a_record_that_does_not_exist_is_refused_as_not_found() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "update-integration-response-identifiers").await;
    // One resource carrying the record every row resolves against, one carrying a method
    // with no integration, and one carrying no method at all.
    let resource = resource_with_an_integration(&gateway, &api, &root, "pets", "GET").await;
    seed(&gateway, &api, &resource, "GET", "200", json!({})).await;
    let without_integration = crate::method_integration::resource_with_a_method(
        &gateway,
        &api,
        &root,
        "bare",
        "GET",
        json!({}),
    )
    .await;

    // Rows differ along WHICH identifier of the four names nothing: the API, the resource,
    // the verb, the integration the verb's method does not carry, and the status code.
    let cases: Vec<(&str, &str, &str, &str, &str)> = vec![
        (
            "an unknown API id",
            "qqqqqqqqqq",
            resource.as_str(),
            "GET",
            "200",
        ),
        (
            "an unknown resource id",
            api.as_str(),
            "zzzzzzzzzz",
            "GET",
            "200",
        ),
        (
            "a verb the resource has no method for",
            api.as_str(),
            resource.as_str(),
            "DELETE",
            "200",
        ),
        (
            "a method carrying no integration",
            api.as_str(),
            resource.as_str(),
            "GET",
            "200",
        ),
        (
            "a status code the integration has no response for",
            api.as_str(),
            resource.as_str(),
            "GET",
            "404",
        ),
    ];

    for (index, (case, api_id, resource_id, verb, status)) in cases.iter().enumerate() {
        // The "method carrying no integration" row is the one addressing the bare resource.
        let resource_id = if index == 3 {
            without_integration.as_str()
        } else {
            *resource_id
        };
        let answer = update_integration_response(
            &gateway,
            api_id,
            resource_id,
            verb,
            status,
            vec![replace("/selectionPattern", ".+")],
        )
        .await;
        assert_failure(&answer, 404, "NotFoundException", case);
    }
}
