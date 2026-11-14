// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteIntegration`, read back through `GetIntegration`,
//! `GetIntegrationResponse` and the `GetMethod` that owns the integration.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi`/`CreateResource`/`PutMethod`/`PutIntegration` helpers are this crate's
//! existing harness, reused from `rest_api_lifecycle`, `method_integration` and
//! `method_response`.
//!
//! The rule under test: a `DeleteIntegration` removes exactly the one integration its
//! `(restApiId, resourceId, httpMethod)` names — and the integration responses stored on
//! it — answering the modelled 204 with an empty body, so every later read of that
//! integration or of one of its responses answers the modelled `NotFoundException` (404)
//! while the owning method, its method responses and every other stored integration are
//! untouched and the method is free to carry a new integration again; a request naming no
//! stored record removes nothing and is refused 404.
//!
//! The tables span what the rule generalises over: the integration type (every value of
//! the modelled `IntegrationType`), the method's verb and a spelling of it the resource
//! holds no method under, how
//! many integration responses hang off the integration, and which identifier of the three
//! is the unknown one.

use serde_json::{json, Value};

use crate::integration_response::{get_integration_response, put_integration_response};
use crate::method_integration::{
    aws_integration_uri, delete_integration, get_integration, get_method, put_integration,
    resource_with_a_method,
};
use crate::method_response::{get_method_response, put_method_response};
use crate::rest_api_lifecycle::{assert_failure, create_api, Gateway};

/// One row of the delete table: its case name, the path part of the resource to stand up,
/// the verb of the method, the `PutIntegration` request, and the status codes of the
/// integration responses to put on the integration.
type DeleteCase<'a> = (&'a str, &'a str, &'a str, Value, Vec<&'a str>);

/// Stand the `verb` method of a fresh `path_part` resource up with `request` as its
/// integration and one integration response per entry of `responses`, answering the
/// resource id.
///
/// The sibling `integration_response` suite's own `resource_with_an_integration` always
/// puts a bare MOCK integration and no responses; this one takes the integration request
/// and the responses the table row calls for, which is what these cases vary, and reuses
/// that suite's `PutIntegrationResponse` helper for each response.
async fn resource_with_an_integration_and_responses(
    gateway: &Gateway,
    api: &str,
    root: &str,
    path_part: &str,
    verb: &str,
    request: Value,
    responses: &[&str],
) -> String {
    let resource = resource_with_a_method(gateway, api, root, path_part, verb, json!({})).await;
    let created = put_integration(gateway, api, &resource, verb, request).await;
    assert_eq!(
        created.status, 201,
        "case PutIntegration({path_part}/{verb}): expected 201, got {} with {:?}",
        created.status, created.body
    );
    for status in responses {
        let response =
            put_integration_response(gateway, api, &resource, verb, status, json!({})).await;
        assert_eq!(
            response.status, 201,
            "case PutIntegrationResponse({path_part}/{verb}/{status}): expected 201, got {} \
             with {:?}",
            response.status, response.body
        );
    }
    resource
}

#[tokio::test]
async fn deleting_an_integration_removes_it_and_the_responses_stored_on_it() {
    // Rows differ along what the rule generalises over: the integration type (every value
    // of the modelled `IntegrationType`), the members the request carries, the verb of the
    // method the integration hangs off, and how many integration responses it owns.
    let cases: Vec<DeleteCase> = vec![
        (
            "a bare mock integration with no responses",
            "mock",
            "GET",
            json!({ "type": "MOCK" }),
            vec![],
        ),
        (
            "a mock integration with one response",
            "mock-one-response",
            "POST",
            json!({
                "type": "MOCK",
                "requestTemplates": { "application/json": "{\"statusCode\": 200}" }
            }),
            vec!["200"],
        ),
        (
            "an http integration with responses across the status classes",
            "http",
            "PUT",
            json!({
                "type": "HTTP",
                "httpMethod": "GET",
                "uri": "https://example.com/pets"
            }),
            vec!["200", "400", "500"],
        ),
        (
            "an http proxy integration",
            "http-proxy",
            "HEAD",
            json!({
                "type": "HTTP_PROXY",
                "httpMethod": "HEAD",
                "uri": "http://example.com/pets"
            }),
            vec!["204"],
        ),
        (
            "an aws service action with credentials",
            "aws",
            "ANY",
            json!({
                "type": "AWS",
                "httpMethod": "POST",
                "uri": aws_integration_uri("dynamodb:action/PutItem"),
                "credentials": "arn:aws:iam::123456789012:role/apigateway-dynamodb"
            }),
            vec!["200", "599"],
        ),
        (
            "a lambda proxy integration",
            "lambda",
            "DELETE",
            json!({
                "type": "AWS_PROXY",
                "httpMethod": "POST",
                "uri": aws_integration_uri(
                    "lambda:path/2015-03-31/functions/arn:aws:lambda:us-east-1:123456789012:function:pets/invocations"
                )
            }),
            vec![],
        ),
    ];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-deletes").await;

    for (index, (case, path_part, verb, request, responses)) in cases.iter().enumerate() {
        let resource = resource_with_an_integration_and_responses(
            &gateway,
            &api,
            &root,
            path_part,
            verb,
            request.clone(),
            responses,
        )
        .await;
        // A method response lives on the METHOD, not on the integration, so it must
        // survive the integration going.
        let method_response =
            put_method_response(&gateway, &api, &resource, verb, "200", json!({})).await;
        assert_eq!(
            method_response.status, 201,
            "case {case}: expected 201 from PutMethodResponse, got {} with {:?}",
            method_response.status, method_response.body
        );

        // Rows alternate whether a spelling the resource holds no method under is tried
        // first: a method is addressed by the verb it was created with, so such a request
        // reaches no integration to delete.
        if index % 2 == 1 {
            let lowercase = verb.to_ascii_lowercase();
            assert_failure(
                &delete_integration(&gateway, &api, &resource, &lowercase).await,
                404,
                "NotFoundException",
                &format!("{case} / DeleteIntegration({lowercase})"),
            );
        }
        let addressed = verb.to_ascii_uppercase();
        let deleted = delete_integration(&gateway, &api, &resource, &addressed).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected the modelled 204 from DeleteIntegration({addressed}), got {} \
             with {:?}",
            deleted.status, deleted.body
        );
        assert!(
            deleted.body.is_null() || deleted.body == json!({}),
            "case {case}: expected an empty body, got {:?}",
            deleted.body
        );

        // The integration is gone for a reader, and a spelling of the verb the resource
        // holds no method under reaches nothing either, and
        // deleting it again finds nothing to delete.
        for verb_probe in [verb.to_ascii_uppercase(), verb.to_ascii_lowercase()] {
            assert_failure(
                &get_integration(&gateway, &api, &resource, &verb_probe).await,
                404,
                "NotFoundException",
                &format!("{case} / GetIntegration({verb_probe})"),
            );
            assert_failure(
                &delete_integration(&gateway, &api, &resource, &verb_probe).await,
                404,
                "NotFoundException",
                &format!("{case} / DeleteIntegration({verb_probe}) again"),
            );
        }

        // Every integration response stored on it went with it.
        for status in responses {
            assert_failure(
                &get_integration_response(&gateway, &api, &resource, verb, status).await,
                404,
                "NotFoundException",
                &format!("{case} / GetIntegrationResponse({status})"),
            );
        }

        // The owning method survives, reports no `methodIntegration`, and keeps its own
        // method response.
        let method = get_method(&gateway, &api, &resource, verb).await;
        assert_eq!(
            method.status, 200,
            "case {case}: the method must survive its integration, got {} with {:?}",
            method.status, method.body
        );
        assert!(
            method.body["methodIntegration"].is_null(),
            "case {case}: expected no methodIntegration after the delete, got {:?}",
            method.body["methodIntegration"]
        );
        assert_eq!(
            method.body["methodResponses"]["200"]["statusCode"],
            json!("200"),
            "case {case}: the method's own response must survive, got {:?}",
            method.body["methodResponses"]
        );
        let kept = get_method_response(&gateway, &api, &resource, verb, "200").await;
        assert_eq!(
            kept.status, 200,
            "case {case}: expected 200 from GetMethodResponse after the delete, got {} with {:?}",
            kept.status, kept.body
        );

        // The method carries a new integration again: nothing stale is left behind.
        let recreated =
            put_integration(&gateway, &api, &resource, verb, json!({ "type": "MOCK" })).await;
        assert_eq!(
            recreated.status, 201,
            "case {case}: expected 201 from PutIntegration after the delete, got {} with {:?}",
            recreated.status, recreated.body
        );
        let read = get_integration(&gateway, &api, &resource, verb).await;
        assert_eq!(
            read.body, recreated.body,
            "case {case}: GetIntegration must report the new integration, got {:?} after {:?}",
            read.body, recreated.body
        );
        // The freed integration carries no response of the deleted one.
        for status in responses {
            assert_failure(
                &get_integration_response(&gateway, &api, &resource, verb, status).await,
                404,
                "NotFoundException",
                &format!("{case} / GetIntegrationResponse({status}) on the new integration"),
            );
        }
    }
}

#[tokio::test]
async fn a_delete_touches_only_the_integration_its_three_identifiers_name() {
    // The same verb carrying an integration on several methods, resources and APIs: a
    // delete is keyed by all three identifiers, so only one of these records goes.
    let gateway = Gateway::new();
    let (first_api, first_root) = create_api(&gateway, "integration-delete-scope-a").await;
    let (second_api, second_root) = create_api(&gateway, "integration-delete-scope-b").await;
    let first_resource =
        resource_with_a_method(&gateway, &first_api, &first_root, "pets", "GET", json!({})).await;
    let second_resource =
        resource_with_a_method(&gateway, &first_api, &first_root, "owners", "GET", json!({})).await;
    let other_api_resource =
        resource_with_a_method(&gateway, &second_api, &second_root, "pets", "GET", json!({})).await;
    // A second verb on the first resource, so the verb is varied too.
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

    // Each row is one (api, resource, verb) carrying a MOCK integration; the first row is
    // the one deleted, every other row must survive.
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        ("the named method", &first_api, &first_resource, "GET"),
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
    for (case, api, resource, verb) in &rows {
        let created = put_integration(&gateway, api, resource, verb, json!({ "type": "MOCK" })).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from PutIntegration, got {} with {:?}",
            created.status, created.body
        );
    }

    let deleted = delete_integration(&gateway, &first_api, &first_resource, "GET").await;
    assert_eq!(
        deleted.status, 204,
        "case DeleteIntegration(the named method): expected 204, got {} with {:?}",
        deleted.status, deleted.body
    );

    for (index, (case, api, resource, verb)) in rows.iter().enumerate() {
        let read = get_integration(&gateway, api, resource, verb).await;
        if index == 0 {
            assert_failure(&read, 404, "NotFoundException", case);
        } else {
            assert_eq!(
                read.status, 200,
                "case {case}: must be untouched by the delete, got {} with {:?}",
                read.status, read.body
            );
        }
    }
}

#[tokio::test]
async fn an_unknown_identifier_is_a_not_found_and_removes_nothing() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-delete-notfound").await;
    let resource = resource_with_an_integration_and_responses(
        &gateway,
        &api,
        &root,
        "pets",
        "GET",
        json!({ "type": "MOCK" }),
        &["200"],
    )
    .await;
    // A second method on the same resource, standing with no integration on it.
    let bare = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/POST"),
            Some(json!({ "authorizationType": "NONE" })),
        )
        .await;
    assert_eq!(
        bare.status, 201,
        "case PutMethod(POST): expected 201, got {} with {:?}",
        bare.status, bare.body
    );
    let missing = "zzzzzzzzzz";

    // Rows span each of the three identifiers this operation resolves, one unresolvable
    // at a time: the API, the resource, a verb naming no method on the resource, a verb
    // outside the catalog, and a method that exists but carries no integration.
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
            "the root resource, which carries no method",
            format!("/restapis/{api}/resources/{root}/methods/GET/integration"),
        ),
        (
            "a verb naming no method",
            format!("/restapis/{api}/resources/{resource}/methods/PATCH/integration"),
        ),
        (
            "a verb outside the catalog",
            format!("/restapis/{api}/resources/{resource}/methods/TRACE/integration"),
        ),
        (
            "a method carrying no integration",
            format!("/restapis/{api}/resources/{resource}/methods/POST/integration"),
        ),
    ];
    for (case, uri) in &cases {
        assert_failure(
            &gateway.call("DELETE", uri, None).await,
            404,
            "NotFoundException",
            case,
        );
    }

    // Every refused call removed nothing: the stored integration and its response are
    // still there.
    let read = get_integration(&gateway, &api, &resource, "GET").await;
    assert_eq!(
        read.status, 200,
        "case GetIntegration after the refused deletes: expected 200, got {} with {:?}",
        read.status, read.body
    );
    let response = get_integration_response(&gateway, &api, &resource, "GET", "200").await;
    assert_eq!(
        response.status, 200,
        "case GetIntegrationResponse after the refused deletes: expected 200, got {} with {:?}",
        response.status, response.body
    );

    // The integration of a method on a deleted API goes with it, so a delete naming one
    // afterwards finds nothing.
    let deleted = gateway
        .call("DELETE", &format!("/restapis/{api}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_failure(
        &delete_integration(&gateway, &api, &resource, "GET").await,
        404,
        "NotFoundException",
        "DeleteIntegration after DeleteRestApi",
    );
}
