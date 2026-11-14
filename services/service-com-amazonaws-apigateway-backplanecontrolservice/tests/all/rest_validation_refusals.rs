// SPDX-License-Identifier: CC0-1.0

//! What this control plane REFUSES, and what it accepts, on four rules whose refusals a
//! client matches on:
//!
//! 1. a caller-supplied API key `value` outside its published length range is refused
//!    with the sentence that names the bound it broke, and nothing of the request;
//! 2. a `Method` is addressed by the HTTP verb it was CREATED with, so a request naming
//!    the verb in any other spelling names no method at all — on every operation that
//!    addresses a method by its `httpMethod` label;
//! 3. an `AWS`/`AWS_PROXY` integration's `uri` is an AWS integration ARN whose resource
//!    names one of the two documented forms, `path/…` or `action/…`, and one that names
//!    neither is refused when the integration is CONFIGURED;
//! 4. an `AWS_PROXY` integration's own HTTP method is any verb of the method catalog —
//!    which verb a Lambda backend wants of an invocation is not a rule this call holds
//!    the record to;
//! 5. a `stageName` outside the character set a stage name is spelled from is refused
//!    with the sentence that names that set, on every call that creates a stage.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi`/`CreateResource`/`PutMethod`/`PutIntegration` helpers are this crate's
//! existing harness, reused from `rest_api_lifecycle` and `method_integration`.
//!
//! Each table spans what its rule generalises over rather than one recorded call: the
//! length on each side of both published bounds, the spellings of a verb and the
//! operations that address one, both documented URI forms and four ways an ARN names
//! neither, every verb of the method catalog as an integration's own method, and the
//! characters inside and outside a stage name's alphabet.

use serde_json::json;
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::put_integration::{
    DEFAULT_PASSTHROUGH_BEHAVIOR, DEFAULT_RESPONSE_TRANSFER_MODE, DEFAULT_TIMEOUT_IN_MILLIS,
};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::put_method::HTTP_METHODS;

use crate::base_path_mapping::assert_refusal;
use crate::method_integration::{
    aws_integration_uri, get_integration, put_integration, resource_with_a_method,
};
use crate::rest_api_lifecycle::{
    assert_failure, create_api, create_resource, make_deployable, string_member, Gateway,
};

/// AWS's published length range for a caller-supplied API key value
/// (`CreateApiKey$value`), spelled out here rather than imported so the assertion fails if
/// the implementation's bound ever moves off it.
const MIN_VALUE_CHARS: usize = 20;

/// The upper end of that range; see [`MIN_VALUE_CHARS`].
const MAX_VALUE_CHARS: usize = 128;

/// The sentence a value under that range is refused with: fixed text naming the lower
/// bound, with nothing of the request in it.
fn too_short() -> String {
    format!("API Key value should be at least {MIN_VALUE_CHARS} characters")
}

/// The sentence a value over that range is refused with; see [`too_short`].
fn exceeds_maximum() -> String {
    format!("API Key value exceeds maximum size of {MAX_VALUE_CHARS} characters")
}

/// The sentence an AWS integration `uri` naming neither documented form is refused with,
/// spelled out here rather than imported for the reason given on [`MIN_VALUE_CHARS`].
const AWS_URI_WITHOUT_PATH_OR_ACTION: &str = "AWS ARN for integration must contain path or action";

/// The message for a `httpMethod` the addressed resource holds no method under, which is
/// this service's one sentence for an unresolved method identifier.
const NO_SUCH_METHOD: &str = "Invalid Method identifier specified";

/// The sentence a `stageName` outside its alphabet is refused with: fixed text naming the
/// set, with nothing of the request in it.
const INVALID_STAGE_NAME: &str = "Stage name only allows a-zA-Z0-9_";

/// The canonical Lambda proxy `uri`: the PATH form of an AWS integration ARN, whose
/// service path is Lambda's own synchronous-invocation resource.
fn lambda_proxy_uri() -> String {
    aws_integration_uri(
        "lambda:path/2015-03-31/functions/\
         arn:aws:lambda:us-east-1:123456789012:function:pets/invocations",
    )
}

/// Assert `answer` is the modelled failure `(status, code)` carrying exactly `message`:
/// this crate's one such assertion, reused from the `base_path_mapping` suite.

#[tokio::test]
async fn an_api_key_value_outside_its_published_range_is_refused_by_its_own_sentence() {
    // Rows differ in the length of the supplied value: inside the range, on each
    // published bound, one character past each, and far past the lower one. `None` is the
    // accepted row; `Some(sentence)` is the refusal that length earns.
    let cases: Vec<(usize, Option<String>)> = vec![
        (1, Some(too_short())),
        (MIN_VALUE_CHARS - 1, Some(too_short())),
        (MIN_VALUE_CHARS, None),
        (MIN_VALUE_CHARS + 1, None),
        (MAX_VALUE_CHARS, None),
        (MAX_VALUE_CHARS + 1, Some(exceeds_maximum())),
        (MAX_VALUE_CHARS * 2, Some(exceeds_maximum())),
    ];

    let gateway = Gateway::new();
    for (length, refusal) in &cases {
        let case = format!("CreateApiKey(value of {length} characters)");
        // Each row's value is distinct, so nothing is refused for being a duplicate.
        let value = format!("{length:0>width$}", width = length);
        let answer = gateway
            .call("POST", "/apikeys", Some(json!({ "value": value })))
            .await;
        match refusal {
            Some(message) => {
                assert_refusal(&answer, (400, "BadRequestException"), message, &case);
            }
            None => {
                assert_eq!(
                    answer.status, 201,
                    "case {case}: expected 201 inside the range, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.body["value"].as_str(),
                    Some(value.as_str()),
                    "case {case}: expected the supplied value back, got {:?}",
                    answer.body["value"]
                );
            }
        }
    }
}

#[tokio::test]
async fn a_method_is_addressed_only_by_the_verb_it_was_created_with() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "method-identifier").await;
    let resource = string_member(
        &create_resource(&gateway, &api, &root, "pets").await,
        "id",
        "method-identifier/pets",
    );
    // Two of the catalog's verbs, so a row that names neither cannot resolve by accident
    // and the `ANY` catch-all is addressed as the identifier it is.
    for verb in ["GET", "ANY"] {
        let answer = gateway
            .call(
                "PUT",
                &format!("/restapis/{api}/resources/{resource}/methods/{verb}"),
                Some(json!({ "authorizationType": "NONE" })),
            )
            .await;
        assert_eq!(
            answer.status, 201,
            "case PutMethod({verb}): expected 201, got {} with {:?}",
            answer.status, answer.body
        );
    }

    // Rows differ in the spelling the request addresses: the two stored identifiers, the
    // same verbs in three other letter cases, a catalog verb no method was put under, and
    // a token that is no verb of the catalog at all.
    let cases: Vec<(&str, bool)> = vec![
        ("GET", true),
        ("ANY", true),
        ("get", false),
        ("Get", false),
        ("gEt", false),
        ("any", false),
        ("POST", false),
        ("TRACE", false),
    ];

    for (spelling, resolves) in &cases {
        let case = format!("GetMethod({spelling})");
        let answer = gateway
            .call(
                "GET",
                &format!("/restapis/{api}/resources/{resource}/methods/{spelling}"),
                None,
            )
            .await;
        if *resolves {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.body["httpMethod"].as_str(),
                Some(*spelling),
                "case {case}: expected the method stored under {spelling:?}, got {:?}",
                answer.body["httpMethod"]
            );
        } else {
            assert_refusal(&answer, (404, "NotFoundException"), NO_SUCH_METHOD, &case);
        }
    }

    // The rule is the method IDENTIFIER's, not one operation's: every call that addresses
    // a method by its `httpMethod` label resolves it the same way. One row per such call,
    // each sent with a spelling the resource holds no method under.
    let unresolved: Vec<(&str, &str, String)> = vec![
        (
            "PutIntegration",
            "PUT",
            format!("/restapis/{api}/resources/{resource}/methods/get/integration"),
        ),
        (
            "GetIntegration",
            "GET",
            format!("/restapis/{api}/resources/{resource}/methods/get/integration"),
        ),
        (
            "PutMethodResponse",
            "PUT",
            format!("/restapis/{api}/resources/{resource}/methods/get/responses/200"),
        ),
        (
            "UpdateMethod",
            "PATCH",
            format!("/restapis/{api}/resources/{resource}/methods/get"),
        ),
        (
            "DeleteMethod",
            "DELETE",
            format!("/restapis/{api}/resources/{resource}/methods/get"),
        ),
        (
            "TestInvokeMethod",
            "POST",
            format!("/restapis/{api}/resources/{resource}/methods/get"),
        ),
    ];
    for (operation, verb, uri) in &unresolved {
        let case = format!("{operation} addressing the verb as \"get\"");
        let body = match *operation {
            "PutIntegration" => Some(json!({ "type": "MOCK" })),
            "UpdateMethod" => Some(json!({ "patchOperations": [] })),
            _ => None,
        };
        let answer = gateway.call(verb, uri, body).await;
        assert_refusal(&answer, (404, "NotFoundException"), NO_SUCH_METHOD, &case);
    }

    // And nothing was created under the other spelling: the resource still holds exactly
    // the two methods it was given.
    let read = gateway
        .call(
            "GET",
            &format!("/restapis/{api}/resources/{resource}"),
            None,
        )
        .await;
    let mut verbs: Vec<String> = read.body["resourceMethods"]
        .as_object()
        .map(|methods| methods.keys().cloned().collect())
        .unwrap_or_default();
    verbs.sort();
    assert_eq!(
        verbs,
        vec!["ANY".to_string(), "GET".to_string()],
        "case GetResource: expected exactly the two stored verbs, got {:?}",
        read.body["resourceMethods"]
    );
}

#[tokio::test]
async fn a_verb_outside_the_catalog_spelling_is_not_a_method_that_can_be_created() {
    // The catalog is `put_method::HTTP_METHODS`, the verbs API Gateway accepts for a
    // method: a value that is one of them in another letter case is not one of them, so
    // the method is refused rather than created under a spelling the request did not ask
    // for.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "method-catalog-spelling").await;
    let resource = string_member(
        &create_resource(&gateway, &api, &root, "pets").await,
        "id",
        "method-catalog-spelling/pets",
    );

    for verb in HTTP_METHODS {
        let spelled = verb.to_ascii_lowercase();
        let case = format!("PutMethod({spelled})");
        let answer = gateway
            .call(
                "PUT",
                &format!("/restapis/{api}/resources/{resource}/methods/{spelled}"),
                Some(json!({ "authorizationType": "NONE" })),
            )
            .await;
        assert_failure(&answer, 400, "BadRequestException", &case);
        // Refused, so the catalog spelling of it still names no method either.
        assert_refusal(
            &gateway
                .call(
                    "GET",
                    &format!("/restapis/{api}/resources/{resource}/methods/{verb}"),
                    None,
                )
                .await,
            (404, "NotFoundException"),
            NO_SUCH_METHOD,
            &format!("{case} / GetMethod({verb})"),
        );
    }
}

#[tokio::test]
async fn an_aws_integration_uri_must_name_a_path_or_an_action() {
    // Rows differ in the FORM the `uri` names: the two documented ones (`path/` and
    // `action/`, over both integration types of the AWS family and two integrated
    // services), and four ARNs that name neither — a service path with no form prefix, an
    // ARN that stops short of its resource field, another vendor's ARN, and the Lambda
    // path with the prefix dropped. The last row is the control: the rule is the AWS
    // family's, so an HTTP integration's URL is untouched by it.
    let cases: Vec<(&str, &str, &str, String, bool)> = vec![
        (
            "the canonical lambda proxy path form",
            "lambda-proxy",
            "AWS_PROXY",
            lambda_proxy_uri(),
            true,
        ),
        (
            "an aws service path form",
            "s3-path",
            "AWS",
            aws_integration_uri("s3:path/shelf/plain.txt"),
            true,
        ),
        (
            "an aws service action form",
            "dynamodb-action",
            "AWS",
            aws_integration_uri("dynamodb:action/PutItem"),
            true,
        ),
        (
            "an action form with parameters",
            "sqs-action",
            "AWS",
            aws_integration_uri("sqs:action/SendMessage&QueueUrl=/123456789012/orders"),
            true,
        ),
        (
            "a service path carrying neither form prefix",
            "neither-form",
            "AWS",
            aws_integration_uri("s3:shelf/plain.txt"),
            false,
        ),
        (
            "an arn that stops short of its resource field",
            "short-arn",
            "AWS",
            "arn:aws:apigateway:us-east-1:lambda".to_string(),
            false,
        ),
        (
            "another vendor's arn",
            "other-vendor",
            "AWS_PROXY",
            "arn:aws:lambda:us-east-1:123456789012:function:pets".to_string(),
            false,
        ),
        (
            "the lambda path with the form prefix dropped",
            "prefixless-lambda",
            "AWS_PROXY",
            aws_integration_uri(
                "lambda:2015-03-31/functions/\
                 arn:aws:lambda:us-east-1:123456789012:function:pets/invocations",
            ),
            false,
        ),
        (
            "an http integration's own url, which this rule does not reach",
            "http-url",
            "HTTP",
            "https://example.com/pets".to_string(),
            true,
        ),
    ];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "aws-integration-uri-forms").await;

    for (case, path_part, integration_type, uri, accepted) in &cases {
        let resource =
            resource_with_a_method(&gateway, &api, &root, path_part, "GET", json!({})).await;
        let answer = put_integration(
            &gateway,
            &api,
            &resource,
            "GET",
            json!({ "type": integration_type, "httpMethod": "POST", "uri": uri }),
        )
        .await;
        if *accepted {
            assert_eq!(
                answer.status, 201,
                "case {case}: expected 201, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.body["uri"].as_str(),
                Some(uri.as_str()),
                "case {case}: expected the stored uri {uri:?}, got {:?}",
                answer.body["uri"]
            );
            let read = get_integration(&gateway, &api, &resource, "GET").await;
            assert_eq!(
                read.body, answer.body,
                "case {case}: GetIntegration must report the stored integration, got {:?}",
                read.body
            );
        } else {
            assert_refusal(
                &answer,
                (400, "BadRequestException"),
                AWS_URI_WITHOUT_PATH_OR_ACTION,
                case,
            );
            // A refused request stores nothing, so the method still carries no
            // integration at all.
            assert_failure(
                &get_integration(&gateway, &api, &resource, "GET").await,
                404,
                "NotFoundException",
                &format!("{case} / GetIntegration after the refusal"),
            );
        }
    }

    // The rule is the stored RECORD's, so the patch that reaches the same record is held
    // to it too: an accepted integration whose `uri` is moved off both forms is refused,
    // and the stored record is left as it was.
    let resource = resource_with_a_method(&gateway, &api, &root, "patched", "GET", json!({})).await;
    let created = put_integration(
        &gateway,
        &api,
        &resource,
        "GET",
        json!({ "type": "AWS", "httpMethod": "POST", "uri": aws_integration_uri("dynamodb:action/PutItem") }),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case PutIntegration(AWS, action form): expected 201, got {} with {:?}",
        created.status, created.body
    );
    let patched = gateway
        .call(
            "PATCH",
            &format!("/restapis/{api}/resources/{resource}/methods/GET/integration"),
            Some(json!({
                "patchOperations": [{
                    "op": "replace",
                    "path": "/uri",
                    "value": aws_integration_uri("dynamodb:PutItem")
                }]
            })),
        )
        .await;
    assert_refusal(
        &patched,
        (400, "BadRequestException"),
        AWS_URI_WITHOUT_PATH_OR_ACTION,
        "UpdateIntegration moving the uri off both forms",
    );
    let read = get_integration(&gateway, &api, &resource, "GET").await;
    assert_eq!(
        read.body, created.body,
        "case UpdateIntegration moving the uri off both forms: expected the record unchanged, \
         got {:?}",
        read.body
    );
}

#[tokio::test]
async fn an_aws_proxy_integration_takes_any_verb_of_the_catalog_as_its_own_method() {
    // Rows span every verb of `put_method::HTTP_METHODS` as the integration's own method,
    // each on the canonical Lambda proxy `uri`, plus a lowercase spelling (a VALUE of the
    // record, so it is read onto the catalog's spelling) and a token that is no verb at
    // all. The method the integration hangs off is `GET` throughout, so no row turns on
    // the two matching.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "aws-proxy-integration-method").await;
    let uri = lambda_proxy_uri();

    for verb in HTTP_METHODS {
        let case = format!("PutIntegration(AWS_PROXY, httpMethod {verb})");
        let resource = resource_with_a_method(
            &gateway,
            &api,
            &root,
            &format!("proxy-{}", verb.to_ascii_lowercase()),
            "GET",
            json!({}),
        )
        .await;
        let answer = put_integration(
            &gateway,
            &api,
            &resource,
            "GET",
            json!({ "type": "AWS_PROXY", "httpMethod": verb, "uri": uri }),
        )
        .await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected 201, got {} with {:?}",
            answer.status, answer.body
        );
        // The created record carries the request's verb and the members the service fills
        // in for every integration.
        let expected = json!({
            "type": "AWS_PROXY",
            "httpMethod": verb,
            "uri": uri,
            "passthroughBehavior": DEFAULT_PASSTHROUGH_BEHAVIOR,
            "timeoutInMillis": DEFAULT_TIMEOUT_IN_MILLIS,
            "responseTransferMode": DEFAULT_RESPONSE_TRANSFER_MODE,
            "cacheKeyParameters": [],
            "cacheNamespace": resource.clone(),
        });
        for (member, value) in expected
            .as_object()
            .expect("the expected record is a JSON object")
        {
            assert_eq!(
                &answer.body[member], value,
                "case {case}: expected {member} {value:?}, got {:?}",
                answer.body[member]
            );
        }
        let read = get_integration(&gateway, &api, &resource, "GET").await;
        assert_eq!(
            read.body, answer.body,
            "case {case}: GetIntegration must report the stored integration, got {:?}",
            read.body
        );
    }

    // A verb sent in another letter case is stored as the catalog spells it.
    let lowercase = resource_with_a_method(&gateway, &api, &root, "proxy-lower", "GET", json!({}))
        .await;
    let answer = put_integration(
        &gateway,
        &api,
        &lowercase,
        "GET",
        json!({ "type": "AWS_PROXY", "httpMethod": "get", "uri": uri }),
    )
    .await;
    assert_eq!(
        answer.status, 201,
        "case PutIntegration(AWS_PROXY, httpMethod \"get\"): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body["httpMethod"].as_str(),
        Some("GET"),
        "case PutIntegration(AWS_PROXY, httpMethod \"get\"): expected the stored verb GET, got {:?}",
        answer.body["httpMethod"]
    );

    // And a value that is no verb of the catalog is still refused.
    let refused = resource_with_a_method(&gateway, &api, &root, "proxy-bad", "GET", json!({})).await;
    let answer = put_integration(
        &gateway,
        &api,
        &refused,
        "GET",
        json!({ "type": "AWS_PROXY", "httpMethod": "FETCH", "uri": uri }),
    )
    .await;
    assert_failure(
        &answer,
        400,
        "BadRequestException",
        "PutIntegration(AWS_PROXY, httpMethod FETCH)",
    );
}

#[tokio::test]
async fn a_stage_name_outside_its_alphabet_is_refused_by_its_own_sentence() {
    // Rows differ in the characters the name is spelled from: the alphabet the member
    // documents ("alphanumeric characters, hyphens, and underscores", `CreateStage$
    // stageName`) and one character outside it per row — the `$` of a reserved name, a
    // space, punctuation, a path separator, a dot and a non-ASCII letter.
    let accepted = ["prod", "Prod_1", "dev-1", "v2", "a"];
    let refused = ["$default", "prod stage", "prod!", "prod/v1", "stage.1", "prodé"];

    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "stage-name-alphabet").await;
    make_deployable(&gateway, &api).await;

    // The rule holds of every call that creates a stage: the deployment that creates one
    // and `CreateStage` itself. A deployment is created first, so `CreateStage` has one to
    // name.
    let deployment = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/deployments"),
            Some(json!({ "stageName": "seed" })),
        )
        .await;
    assert_eq!(
        deployment.status, 201,
        "case CreateDeployment(seed): expected 201, got {} with {:?}",
        deployment.status, deployment.body
    );
    let deployment_id = string_member(&deployment.body, "id", "CreateDeployment(seed)");

    for name in refused {
        for (operation, uri, body) in [
            (
                "CreateDeployment",
                format!("/restapis/{api}/deployments"),
                json!({ "stageName": name }),
            ),
            (
                "CreateStage",
                format!("/restapis/{api}/stages"),
                json!({ "stageName": name, "deploymentId": deployment_id }),
            ),
        ] {
            let case = format!("{operation}(stageName {name:?})");
            let answer = gateway.call("POST", &uri, Some(body)).await;
            assert_refusal(
                &answer,
                (400, "BadRequestException"),
                INVALID_STAGE_NAME,
                &case,
            );
        }
    }

    for name in accepted {
        let case = format!("CreateStage(stageName {name:?})");
        let answer = gateway
            .call(
                "POST",
                &format!("/restapis/{api}/stages"),
                Some(json!({ "stageName": name, "deploymentId": deployment_id })),
            )
            .await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected 201 inside the alphabet, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["stageName"].as_str(),
            Some(name),
            "case {case}: expected the created stage to carry the name, got {:?}",
            answer.body["stageName"]
        );
    }

    // The published maximum is a bound of its own, and a name past it is refused for its
    // LENGTH rather than for a character outside the alphabet.
    let longest = "s".repeat(128);
    let accepted_at_the_bound = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/stages"),
            Some(json!({ "stageName": longest, "deploymentId": deployment_id })),
        )
        .await;
    assert_eq!(
        accepted_at_the_bound.status, 201,
        "case CreateStage(a 128-character name): expected 201, got {} with {:?}",
        accepted_at_the_bound.status, accepted_at_the_bound.body
    );
    let past_the_bound = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/stages"),
            Some(json!({ "stageName": "s".repeat(129), "deploymentId": deployment_id })),
        )
        .await;
    assert_failure(
        &past_the_bound,
        400,
        "BadRequestException",
        "CreateStage(a 129-character name)",
    );
}
