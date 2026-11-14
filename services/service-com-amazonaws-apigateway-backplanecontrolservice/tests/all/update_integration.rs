// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateIntegration`, read back through `GetIntegration` and
//! through the `GetMethod` that owns the integration.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi`/`CreateResource`/`PutMethod`/`PutIntegration` helpers are this crate's
//! existing harness, reused from `rest_api_lifecycle` and `method_integration`.
//!
//! The RULE under test: an `UpdateIntegration` applies every entry of `patchOperations`
//! to the stored `Integration` its `(restApiId, resourceId, httpMethod)` names, in request
//! order and as one unit, and reports the updated record — so a later read reports exactly
//! what the patch set — while a patch naming a path the `Integration` has no updateable
//! member for (its `type`, fixed when the record is created, included), an op such a member
//! does not support, or a value that leaves the record outside the rules its members
//! publish is refused with a modelled error and changes nothing.
//!
//! The tables span what the rule generalises over: the member's kind (optional string,
//! service-defaulted string, enum, bounded integer, nested structure, string map, string
//! collection), the op (`add`, `replace`, `remove`, and the `move`/`copy`/`test` these
//! members support for nothing), how many ops one request carries, the two spellings of a
//! collection entry, an escaped `/` in a map key, each published bound of
//! `timeoutInMillis` and one past it, the verb's letter case on the wire, and which of the
//! identifiers is the unknown one.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::put_integration::{
    DEFAULT_CONNECTION_TYPE, DEFAULT_PASSTHROUGH_BEHAVIOR, MAX_TIMEOUT_IN_MILLIS,
    MIN_TIMEOUT_IN_MILLIS, PASSTHROUGH_BEHAVIORS, VPC_LINK_CONNECTION_TYPE,
};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::types::{
    CONNECTION_TYPE_VALUES, CONTENT_HANDLING_STRATEGY_VALUES, RESPONSE_TRANSFER_MODE_VALUES,
};

use crate::method_integration::{
    get_integration, get_method, put_integration, resource_with_a_method,
};
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

/// The request parameter the seeded method declares, which is what a `cacheKeyParameters`
/// entry has to name ("these parameters must also be specified for Method
/// requestParameters").
const METHOD_REQUEST_PARAMETER: &str = "method.request.querystring.breed";

/// The integration every table row starts from: one member of each kind the patch paths
/// reach, so a row can set, replace or clear the member it is about.
fn seed_request() -> Value {
    json!({
        "type": "HTTP",
        "httpMethod": "GET",
        "uri": "https://example.com/pets",
        "credentials": "arn:aws:iam::123456789012:role/apigateway-pets",
        "cacheNamespace": "pets",
        "contentHandling": "CONVERT_TO_TEXT",
        "requestParameters": { "integration.request.querystring.breed": METHOD_REQUEST_PARAMETER },
        "requestTemplates": { "application/json": "{\"breed\": $input.params('breed')}" },
        "timeoutInMillis": 5_000
    })
}

/// `UpdateIntegration` for `verb` on `resource` of `api`.
async fn update_integration(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    operations: Vec<Value>,
) -> Answer {
    gateway
        .call(
            "PATCH",
            &format!("/restapis/{api}/resources/{resource}/methods/{verb}/integration"),
            Some(json!({ "patchOperations": Value::Array(operations) })),
        )
        .await
}

/// A fresh resource of `api` carrying one `verb` method with an integration, answering the
/// resource id and the stored integration as the wire reports it.
async fn seed(
    gateway: &Gateway,
    api: &str,
    root: &str,
    path_part: &str,
    verb: &str,
    request: Value,
) -> (String, Value) {
    let resource = resource_with_a_method(
        gateway,
        api,
        root,
        path_part,
        verb,
        json!({ METHOD_REQUEST_PARAMETER: true }),
    )
    .await;
    let created = put_integration(gateway, api, &resource, verb, request).await;
    assert_eq!(
        created.status, 201,
        "case PutIntegration({path_part}/{verb}): expected 201, got {} with {:?}",
        created.status, created.body
    );
    (resource, created.body)
}

/// `base` with `changes` applied: a member set to a value replaces it, a member set to
/// `null` is expected to be ABSENT from the reported record, the way an unset member is.
fn with_changes(base: &Value, changes: &Value) -> Value {
    let mut expected = base.clone();
    {
        let target = expected
            .as_object_mut()
            .expect("the stored integration is a JSON object");
        for (member, value) in changes
            .as_object()
            .expect("the expected changes are a JSON object")
        {
            if value.is_null() {
                target.remove(member);
            } else {
                target.insert(member.clone(), value.clone());
            }
        }
    }
    expected
}

/// Assert the updated record is what a later reader sees: `GetIntegration` and the
/// `methodIntegration` of the owning `GetMethod` both report `expected`.
async fn assert_read_back(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    case: &str,
    expected: &Value,
) {
    let read = get_integration(gateway, api, resource, verb).await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected 200 from GetIntegration, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        &read.body, expected,
        "case {case}: GetIntegration must report the updated integration {expected:?}, got {:?}",
        read.body
    );
    let method = get_method(gateway, api, resource, verb).await;
    assert_eq!(
        &method.body["methodIntegration"], expected,
        "case {case}: the owning method must carry the updated integration {expected:?}, got {:?}",
        method.body["methodIntegration"]
    );
}

#[tokio::test]
async fn a_patched_integration_is_persisted_and_read_back_member_for_member() {
    // Rows differ along what the rule generalises over: which member the path names and
    // of what kind, the op, the number of ops in one request, the two spellings of a
    // collection entry, an escaped `/` in a map key, and each published bound of the one
    // bounded integer member. Each row starts from its own copy of the seeded integration
    // and names the members it expects to differ afterwards.
    let cases: Vec<(&str, Vec<Value>, Value)> = vec![
        (
            "no ops at all leave the record as it was",
            vec![],
            json!({}),
        ),
        (
            "replace the uri",
            vec![replace("/uri", "https://example.com/pets/v2")],
            json!({ "uri": "https://example.com/pets/v2" }),
        ),
        (
            "replace the integration method, sent lowercase",
            vec![replace("/httpMethod", "post")],
            json!({ "httpMethod": "POST" }),
        ),
        (
            "replace the credentials",
            vec![replace(
                "/credentials",
                "arn:aws:iam::123456789012:role/apigateway-other",
            )],
            json!({ "credentials": "arn:aws:iam::123456789012:role/apigateway-other" }),
        ),
        (
            "remove the credentials, which an integration need not carry",
            vec![op("remove", "/credentials", None)],
            json!({ "credentials": null }),
        ),
        (
            "add an integration target",
            vec![op(
                "add",
                "/integrationTarget",
                Some("arn:aws:elasticloadbalancing:us-east-1:123456789012:listener/app/pets/1/2"),
            )],
            json!({
                "integrationTarget":
                    "arn:aws:elasticloadbalancing:us-east-1:123456789012:listener/app/pets/1/2"
            }),
        ),
        (
            "replace the cache namespace",
            vec![replace("/cacheNamespace", "shared-pets")],
            json!({ "cacheNamespace": "shared-pets" }),
        ),
        (
            "replace the passthrough behaviour",
            vec![replace("/passthroughBehavior", "NEVER")],
            json!({ "passthroughBehavior": "NEVER" }),
        ),
        (
            "replace the content handling",
            vec![replace("/contentHandling", "CONVERT_TO_BINARY")],
            json!({ "contentHandling": "CONVERT_TO_BINARY" }),
        ),
        (
            "remove the content handling, which leaves the payload passed through",
            vec![op("remove", "/contentHandling", None)],
            json!({ "contentHandling": null }),
        ),
        (
            "replace the response transfer mode",
            vec![replace("/responseTransferMode", "STREAM")],
            json!({ "responseTransferMode": "STREAM" }),
        ),
        (
            "replace the timeout with the published minimum",
            vec![replace("/timeoutInMillis", &MIN_TIMEOUT_IN_MILLIS.to_string())],
            json!({ "timeoutInMillis": MIN_TIMEOUT_IN_MILLIS }),
        ),
        (
            "replace the timeout with the published maximum",
            vec![replace("/timeoutInMillis", &MAX_TIMEOUT_IN_MILLIS.to_string())],
            json!({ "timeoutInMillis": MAX_TIMEOUT_IN_MILLIS }),
        ),
        (
            "add a mapping template under a second content type",
            vec![op(
                "add",
                "/requestTemplates/text~1xml",
                Some("<breed>$input.params('breed')</breed>"),
            )],
            json!({
                "requestTemplates": {
                    "application/json": "{\"breed\": $input.params('breed')}",
                    "text/xml": "<breed>$input.params('breed')</breed>"
                }
            }),
        ),
        (
            "remove the only mapping template",
            vec![op("remove", "/requestTemplates/application~1json", None)],
            json!({ "requestTemplates": null }),
        ),
        (
            "replace a request parameter mapping",
            vec![replace(
                "/requestParameters/integration.request.querystring.breed",
                "'labrador'",
            )],
            json!({
                "requestParameters": { "integration.request.querystring.breed": "'labrador'" }
            }),
        ),
        (
            "add a request parameter mapping and remove the one the record had",
            vec![
                op(
                    "add",
                    "/requestParameters/integration.request.header.x-breed",
                    Some(METHOD_REQUEST_PARAMETER),
                ),
                op(
                    "remove",
                    "/requestParameters/integration.request.querystring.breed",
                    None,
                ),
            ],
            json!({
                "requestParameters": {
                    "integration.request.header.x-breed": METHOD_REQUEST_PARAMETER
                }
            }),
        ),
        (
            "add a cache key parameter addressed by the path",
            vec![op(
                "add",
                &format!("/cacheKeyParameters/{METHOD_REQUEST_PARAMETER}"),
                None,
            )],
            json!({ "cacheKeyParameters": [METHOD_REQUEST_PARAMETER] }),
        ),
        (
            "add the same cache key parameter addressed by the op's value",
            vec![op(
                "add",
                "/cacheKeyParameters",
                Some(METHOD_REQUEST_PARAMETER),
            )],
            json!({ "cacheKeyParameters": [METHOD_REQUEST_PARAMETER] }),
        ),
        (
            "add a cache key parameter and remove it again",
            vec![
                op(
                    "add",
                    &format!("/cacheKeyParameters/{METHOD_REQUEST_PARAMETER}"),
                    None,
                ),
                op(
                    "remove",
                    &format!("/cacheKeyParameters/{METHOD_REQUEST_PARAMETER}"),
                    None,
                ),
            ],
            json!({ "cacheKeyParameters": [] }),
        ),
        (
            "set the nested tls configuration",
            vec![replace("/tlsConfig/insecureSkipVerification", "true")],
            json!({ "tlsConfig": { "insecureSkipVerification": true } }),
        ),
        (
            // The cross-member rule — a `connectionId` belongs only to a `VPC_LINK` — is a
            // property of the RESULTING record, so a list that reaches a valid record is
            // accepted whichever order its ops come in.
            "turn the integration private, the connection id after the connection type",
            vec![
                replace("/connectionType", VPC_LINK_CONNECTION_TYPE),
                op("add", "/connectionId", Some("vpce01234")),
            ],
            json!({ "connectionType": VPC_LINK_CONNECTION_TYPE, "connectionId": "vpce01234" }),
        ),
        (
            "turn the integration private, the connection id before the connection type",
            vec![
                op("add", "/connectionId", Some("vpce56789")),
                replace("/connectionType", VPC_LINK_CONNECTION_TYPE),
            ],
            json!({ "connectionType": VPC_LINK_CONNECTION_TYPE, "connectionId": "vpce56789" }),
        ),
        (
            "the last op of a list wins over an earlier one on the same member",
            vec![
                replace("/uri", "https://example.com/pets/first"),
                replace("/uri", "https://example.com/pets/last"),
            ],
            json!({ "uri": "https://example.com/pets/last" }),
        ),
    ];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-patch").await;

    for (index, (case, operations, changes)) in cases.iter().enumerate() {
        let (resource, stored) = seed(
            &gateway,
            &api,
            &root,
            &format!("pets-{index}"),
            "GET",
            seed_request(),
        )
        .await;
        let expected = with_changes(&stored, changes);

        let answer = update_integration(&gateway, &api, &resource, "GET", operations.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the modelled 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body, expected,
            "case {case}: expected the updated integration {expected:?}, got {:?}",
            answer.body
        );
        assert_read_back(&gateway, &api, &resource, "GET", case, &expected).await;
    }
}

#[tokio::test]
async fn every_published_value_of_a_patched_member_is_accepted() {
    // The catalogs are the model's own (`CONNECTION_TYPE_VALUES`,
    // `CONTENT_HANDLING_STRATEGY_VALUES`, `RESPONSE_TRANSFER_MODE_VALUES` of
    // `smithy_query(query="operation", operation="UpdateIntegration")`) and the three
    // values `Integration$passthroughBehavior` documents, so each row is an entry of a
    // published set rather than a value this suite chose.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-patch-values").await;
    let (resource, _) = seed(&gateway, &api, &root, "pets", "GET", seed_request()).await;

    let mut cases: Vec<(&str, String, Vec<Value>)> = Vec::new();
    for behavior in PASSTHROUGH_BEHAVIORS {
        cases.push((
            "passthroughBehavior",
            (*behavior).to_string(),
            vec![replace("/passthroughBehavior", behavior)],
        ));
    }
    for handling in CONTENT_HANDLING_STRATEGY_VALUES {
        cases.push((
            "contentHandling",
            (*handling).to_string(),
            vec![replace("/contentHandling", handling)],
        ));
    }
    for mode in RESPONSE_TRANSFER_MODE_VALUES {
        cases.push((
            "responseTransferMode",
            (*mode).to_string(),
            vec![replace("/responseTransferMode", mode)],
        ));
    }
    for connection in CONNECTION_TYPE_VALUES {
        // A private connection takes a `connectionId` and an internet one takes none, so
        // the row carries the op that keeps the record valid for the value it sets.
        let mut operations = vec![replace("/connectionType", connection)];
        if *connection == VPC_LINK_CONNECTION_TYPE {
            operations.push(op("add", "/connectionId", Some("vpce01234")));
        } else {
            operations.push(op("remove", "/connectionId", None));
        }
        cases.push(("connectionType", (*connection).to_string(), operations));
    }

    for (member, value, operations) in &cases {
        let case = format!("replace /{member} with {value}");
        let answer = update_integration(&gateway, &api, &resource, "GET", operations.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the modelled 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body[*member].as_str(),
            Some(value.as_str()),
            "case {case}: expected the value the patch set, got {:?}",
            answer.body[*member]
        );
        let read = get_integration(&gateway, &api, &resource, "GET").await;
        assert_eq!(
            read.body, answer.body,
            "case {case}: GetIntegration must report the updated integration, got {:?} after {:?}",
            read.body, answer.body
        );
    }
}

#[tokio::test]
async fn a_patch_outside_the_rules_the_integration_publishes_is_refused_and_changes_nothing() {
    // Rows span each way a patch can be outside the rules: a path the record has no
    // updateable member for (`/type`, fixed when the integration is created, and the
    // responses that are their own resource), a path reaching into a single-valued member
    // or naming no key of a map, an op these members support for nothing, a value outside
    // a published value set, a value that is not of the member's type, one past each
    // published bound, a `remove` of a member the service always reports a value for, and
    // a value that would leave the record inconsistent with its type.
    let cases: Vec<(&str, Vec<Value>)> = vec![
        (
            "replace the type, which is fixed when the integration is created",
            vec![replace("/type", "MOCK")],
        ),
        (
            "add a type",
            vec![op("add", "/type", Some("AWS"))],
        ),
        (
            "remove the type",
            vec![op("remove", "/type", None)],
        ),
        (
            // The case the rule is most often met in: one op the record takes and one it
            // does not, in a single request.
            "replace the uri and the type together",
            vec![
                replace("/uri", "https://example.com/pets/v2"),
                replace("/type", "MOCK"),
            ],
        ),
        (
            "replace a member the shape does not declare",
            vec![replace("/id", "abcdefghij")],
        ),
        (
            "replace an integration response, which is its own resource",
            vec![replace("/integrationResponses/200", "{}")],
        ),
        (
            "reach into a single-valued member",
            vec![replace("/uri/scheme", "https")],
        ),
        (
            "name a map but no key of it",
            vec![replace("/requestTemplates", "{}")],
        ),
        (
            "move a member",
            vec![json!({ "op": "move", "path": "/uri", "from": "/cacheNamespace" })],
        ),
        (
            "copy a member",
            vec![json!({ "op": "copy", "path": "/uri", "from": "/cacheNamespace" })],
        ),
        (
            "test a member",
            vec![json!({ "op": "test", "path": "/uri", "value": "https://example.com/pets" })],
        ),
        (
            "an entry with no op",
            vec![json!({ "path": "/uri", "value": "https://example.com/pets/v2" })],
        ),
        (
            "an entry with no path",
            vec![json!({ "op": "replace", "value": "https://example.com/pets/v2" })],
        ),
        (
            "a path that is not a JSON Pointer",
            vec![replace("uri", "https://example.com/pets/v2")],
        ),
        (
            "a replace with no value",
            vec![op("replace", "/uri", None)],
        ),
        (
            "a connection type outside the modelled value set",
            vec![replace("/connectionType", "SATELLITE")],
        ),
        (
            "a content handling outside the modelled value set",
            vec![replace("/contentHandling", "CONVERT_TO_JSON")],
        ),
        (
            "a response transfer mode outside the modelled value set",
            vec![replace("/responseTransferMode", "DRIBBLE")],
        ),
        (
            "a passthrough behaviour outside the documented value set",
            vec![replace("/passthroughBehavior", "SOMETIMES")],
        ),
        (
            "a timeout one below the published minimum",
            vec![replace(
                "/timeoutInMillis",
                &(MIN_TIMEOUT_IN_MILLIS - 1).to_string(),
            )],
        ),
        (
            "a timeout one above the published maximum",
            vec![replace(
                "/timeoutInMillis",
                &(MAX_TIMEOUT_IN_MILLIS + 1).to_string(),
            )],
        ),
        (
            "a timeout that is not an integer",
            vec![replace("/timeoutInMillis", "soon")],
        ),
        (
            "a tls flag that is not a boolean",
            vec![replace("/tlsConfig/insecureSkipVerification", "maybe")],
        ),
        (
            "a tls sub-member the structure does not declare",
            vec![replace("/tlsConfig/verifyEverything", "true")],
        ),
        (
            "remove the timeout, which the service always reports",
            vec![op("remove", "/timeoutInMillis", None)],
        ),
        (
            "remove the cache namespace, which the service always reports",
            vec![op("remove", "/cacheNamespace", None)],
        ),
        (
            "remove the passthrough behaviour, which the service always reports",
            vec![op("remove", "/passthroughBehavior", None)],
        ),
        (
            "remove the connection type, which the service always reports",
            vec![op("remove", "/connectionType", None)],
        ),
        (
            "empty the cache namespace",
            vec![replace("/cacheNamespace", "")],
        ),
        (
            "remove the uri an http integration needs",
            vec![op("remove", "/uri", None)],
        ),
        (
            "remove the integration method an http integration needs",
            vec![op("remove", "/httpMethod", None)],
        ),
        (
            "a uri that is not a formed HTTP(S) URL",
            vec![replace("/uri", "example.com/pets")],
        ),
        (
            "an empty uri",
            vec![replace("/uri", "   ")],
        ),
        (
            "an integration method outside the HTTP verbs",
            vec![replace("/httpMethod", "FETCH")],
        ),
        (
            "a connection id on an internet connection",
            vec![op("add", "/connectionId", Some("vpce01234"))],
        ),
        (
            "a private connection with no connection id",
            vec![replace("/connectionType", VPC_LINK_CONNECTION_TYPE)],
        ),
        (
            "a cache key parameter that is not a request parameter of the method",
            vec![op(
                "add",
                "/cacheKeyParameters/method.request.querystring.colour",
                None,
            )],
        ),
    ];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-patch-refusals").await;
    let (resource, stored) = seed(&gateway, &api, &root, "pets", "GET", seed_request()).await;

    for (case, operations) in &cases {
        let answer = update_integration(&gateway, &api, &resource, "GET", operations.clone()).await;
        assert_failure(&answer, 400, "BadRequestException", case);
        // A refused list is applied as one unit: the stored record is the one the seed
        // put, not a half-patched one.
        assert_read_back(&gateway, &api, &resource, "GET", case, &stored).await;
    }
}

#[tokio::test]
async fn a_mock_integration_takes_the_removals_its_type_allows() {
    // The same `remove` that an HTTP integration is refused — its type requires a `uri`
    // and an integration method — is accepted on a MOCK integration, whose `httpMethod`
    // "is optional" and which names no endpoint: the rule is about the resulting record's
    // type, not about the member.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-patch-mock").await;
    let (resource, stored) = seed(
        &gateway,
        &api,
        &root,
        "pets",
        "POST",
        json!({ "type": "MOCK", "httpMethod": "POST", "uri": "https://example.com/pets" }),
    )
    .await;
    assert_eq!(
        stored["type"].as_str(),
        Some("MOCK"),
        "case the seeded mock integration: expected a MOCK type, got {stored:?}"
    );

    let answer = update_integration(
        &gateway,
        &api,
        &resource,
        "POST",
        vec![op("remove", "/uri", None), op("remove", "/httpMethod", None)],
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case remove the uri and the integration method of a mock: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let expected = with_changes(&stored, &json!({ "uri": null, "httpMethod": null }));
    assert_eq!(
        answer.body, expected,
        "case remove the uri and the integration method of a mock: expected {expected:?}, got {:?}",
        answer.body
    );
    assert_read_back(
        &gateway,
        &api,
        &resource,
        "POST",
        "remove the uri and the integration method of a mock",
        &expected,
    )
    .await;
}

#[tokio::test]
async fn a_lambda_proxy_integration_takes_any_verb_of_the_catalog_as_its_method() {
    // "For Lambda integrations, you must set the integration method to POST"
    // (`Integration$httpMethod`) is what the BACKEND requires of an invocation, not a
    // rule this patch holds the record to: an `AWS_PROXY` integration's own method is
    // stored as any verb of the catalog, normalised, and only a value that is no verb at
    // all is refused.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-patch-lambda").await;
    let lambda_uri = "arn:aws:apigateway:us-east-1:lambda:path/2015-03-31/functions/\
                      arn:aws:lambda:us-east-1:123456789012:function:pets/invocations";
    let (resource, stored) = seed(
        &gateway,
        &api,
        &root,
        "pets",
        "ANY",
        json!({ "type": "AWS_PROXY", "httpMethod": "POST", "uri": lambda_uri }),
    )
    .await;

    // Rows differ in the verb the patch names and in its letter case; the expected stored
    // verb is the catalog's spelling of it.
    for (sent, expected) in [("GET", "GET"), ("get", "GET"), ("post", "POST")] {
        let case = format!("replace a lambda integration's method with {sent:?}");
        let accepted =
            update_integration(&gateway, &api, &resource, "ANY", vec![replace("/httpMethod", sent)])
                .await;
        assert_eq!(
            accepted.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            accepted.status, accepted.body
        );
        assert_eq!(
            accepted.body["httpMethod"].as_str(),
            Some(expected),
            "case {case}: expected the stored httpMethod {expected:?}, got {:?}",
            accepted.body["httpMethod"]
        );
        let expected_record = with_changes(&stored, &json!({ "httpMethod": expected }));
        assert_read_back(&gateway, &api, &resource, "ANY", &case, &expected_record).await;
    }

    // A value that is no verb of the catalog is still refused, and changes nothing.
    let refused = update_integration(
        &gateway,
        &api,
        &resource,
        "ANY",
        vec![replace("/httpMethod", "FETCH")],
    )
    .await;
    assert_failure(
        &refused,
        400,
        "BadRequestException",
        "replace a lambda integration's method with FETCH",
    );
    let expected_record = with_changes(&stored, &json!({ "httpMethod": "POST" }));
    assert_read_back(
        &gateway,
        &api,
        &resource,
        "ANY",
        "replace a lambda integration's method with FETCH",
        &expected_record,
    )
    .await;
}

#[tokio::test]
async fn an_unknown_identifier_is_a_not_found_on_update_integration() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-patch-notfound").await;
    let (resource, _) = seed(&gateway, &api, &root, "pets", "GET", seed_request()).await;
    // A second resource whose method carries no integration at all.
    let bare = resource_with_a_method(&gateway, &api, &root, "bare", "GET", json!({})).await;
    let missing = "zzzzzzzzzz";

    // Rows span each identifier the operation resolves, one unknown at a time, plus the
    // method that exists and carries no integration to patch.
    let cases: Vec<(&str, String)> = vec![
        (
            "an unknown restApiId",
            format!("/restapis/{missing}/resources/{resource}/methods/GET/integration"),
        ),
        (
            "an unknown resourceId",
            format!("/restapis/{api}/resources/{missing}/methods/GET/integration"),
        ),
        (
            "a verb with no method on the resource",
            format!("/restapis/{api}/resources/{resource}/methods/DELETE/integration"),
        ),
        (
            "a method carrying no integration",
            format!("/restapis/{api}/resources/{bare}/methods/GET/integration"),
        ),
    ];

    for (case, uri) in &cases {
        let answer = gateway
            .call(
                "PATCH",
                uri,
                Some(json!({
                    "patchOperations": [replace("/uri", "https://example.com/pets/v2")]
                })),
            )
            .await;
        assert_failure(&answer, 404, "NotFoundException", case);
    }

    // The integration of a method on a deleted API goes with it.
    let deleted = gateway
        .call("DELETE", &format!("/restapis/{api}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_failure(
        &update_integration(
            &gateway,
            &api,
            &resource,
            "GET",
            vec![replace("/uri", "https://example.com/pets/v2")],
        )
        .await,
        404,
        "NotFoundException",
        "UpdateIntegration after DeleteRestApi",
    );
}

#[tokio::test]
async fn the_service_filled_members_survive_a_patch_that_does_not_name_them() {
    // A patch is a MERGE, not a replacement: the members the service filled in at create
    // time keep their values through an update that names other members.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-patch-defaults").await;
    let (resource, stored) = seed(
        &gateway,
        &api,
        &root,
        "pets",
        "GET",
        json!({ "type": "HTTP", "httpMethod": "GET", "uri": "https://example.com/pets" }),
    )
    .await;
    assert_eq!(
        stored["connectionType"].as_str(),
        Some(DEFAULT_CONNECTION_TYPE),
        "case the seeded integration: expected the service's connection type, got {stored:?}"
    );

    let answer = update_integration(
        &gateway,
        &api,
        &resource,
        "GET",
        vec![replace("/uri", "https://example.com/pets/v2")],
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case replace the uri: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    for (member, expected) in [
        ("connectionType", json!(DEFAULT_CONNECTION_TYPE)),
        ("passthroughBehavior", json!(DEFAULT_PASSTHROUGH_BEHAVIOR)),
        ("cacheNamespace", json!(resource)),
        ("cacheKeyParameters", json!([])),
    ] {
        assert_eq!(
            answer.body[member], expected,
            "case replace the uri: expected {member} to stay {expected:?}, got {:?}",
            answer.body[member]
        );
    }

    // Reading the updated integration does not change what a later caller sees.
    let mut seen: Vec<Value> = Vec::new();
    for round in 0..3 {
        let read = get_integration(&gateway, &api, &resource, "GET").await;
        assert_eq!(
            read.status, 200,
            "case GetIntegration(round {round}): expected 200, got {} with {:?}",
            read.status, read.body
        );
        seen.push(read.body);
    }
    assert_eq!(
        seen[0], seen[2],
        "case GetIntegration repeated: a read must not change the record, got {:?} then {:?}",
        seen[0], seen[2]
    );
}
