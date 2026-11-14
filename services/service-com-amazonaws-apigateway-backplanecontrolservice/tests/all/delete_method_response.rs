// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteMethodResponse`, read back through `GetMethodResponse`
//! and through the `GetMethod` that owns the response.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi`/`CreateResource`/`PutMethod`/`PutMethodResponse` helpers are this
//! crate's existing harness, reused from `rest_api_lifecycle`, `method_integration` and
//! the sibling `method_response` suite.
//!
//! The rule under test: a `DeleteMethodResponse` removes exactly the one method response
//! its `(restApiId, resourceId, httpMethod, statusCode)` names, answering the modelled
//! 204 with an empty body, so every later read of THAT response answers the modelled
//! `NotFoundException` (404) while every other stored response is untouched and the
//! status code is free to carry a new response again; a request naming no stored record
//! removes nothing and is refused 404.
//!
//! The tables span what the rule generalises over: the status code (values across the
//! modelled `^[1-5]\d\d$` pattern, each published bound and one past it), the method's
//! verb and a spelling of it the resource holds no method under, how many responses the
//! method carries, which of
//! the two caller-supplied maps each response carries, and which identifier of the four
//! is the unknown one.

use serde_json::{json, Value};

use crate::method_integration::{get_method, resource_with_a_method};
use crate::method_response::{get_method_response, put_method_response, response_header};
use crate::rest_api_lifecycle::{assert_failure, create_api, Answer, Gateway};

/// One row of the delete table: its case name, the path part of the resource to stand
/// up, the verb of the method, and the `(statusCode, request)` pairs of the responses to
/// put on it.
type DeleteCase<'a> = (&'a str, &'a str, &'a str, Vec<(&'a str, Value)>);

/// `DeleteMethodResponse` for `status` on the `verb` method of `resource`.
async fn delete_method_response(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    status: &str,
) -> Answer {
    gateway
        .call(
            "DELETE",
            &format!("/restapis/{api}/resources/{resource}/methods/{verb}/responses/{status}"),
            None,
        )
        .await
}

/// The status codes the `verb` method of `resource` reports in its `methodResponses`, in
/// sorted order.
async fn reported_status_codes(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    case: &str,
) -> Vec<String> {
    let method = get_method(gateway, api, resource, verb).await;
    assert_eq!(
        method.status, 200,
        "case {case}: expected 200 from GetMethod, got {} with {:?}",
        method.status, method.body
    );
    let mut codes: Vec<String> = method.body["methodResponses"]
        .as_object()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    codes.sort();
    codes
}

/// Put every `(status, request)` of `responses` on the `verb` method of `resource`,
/// asserting the modelled 201 for each.
async fn put_all(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    responses: &[(&str, Value)],
) {
    for (status, request) in responses {
        let created =
            put_method_response(gateway, api, resource, verb, status, request.clone()).await;
        assert_eq!(
            created.status, 201,
            "case PutMethodResponse({verb}/{status}): expected 201, got {} with {:?}",
            created.status, created.body
        );
    }
}

#[tokio::test]
async fn deleting_a_method_response_removes_that_status_code_and_leaves_the_others() {
    // Rows differ along what the rule generalises over: the verb of the method the
    // responses hang off, how many responses it carries, the status codes themselves
    // (values across the modelled `^[1-5]\d\d$` pattern, its lowest and highest), and
    // which of the two caller-supplied maps each response carries.
    let cases: Vec<DeleteCase> = vec![
        (
            "one bare response on a GET",
            "only-one",
            "GET",
            vec![("200", json!({}))],
        ),
        (
            "two responses on a POST, one with a model",
            "two-responses",
            "POST",
            vec![
                ("201", json!({ "responseModels": { "application/json": "Empty" } })),
                ("400", json!({})),
            ],
        ),
        (
            "the pattern's bounds on a HEAD",
            "pattern-bounds",
            "HEAD",
            vec![("100", json!({})), ("599", json!({}))],
        ),
        (
            "responses across every status class on an ANY",
            "every-class",
            "ANY",
            vec![
                ("100", json!({})),
                (
                    "200",
                    json!({ "responseParameters": { response_header("Content-Type"): true } }),
                ),
                (
                    "302",
                    json!({ "responseParameters": { response_header("Location"): false } }),
                ),
                ("404", json!({ "responseModels": { "application/json": "Error" } })),
                (
                    "500",
                    json!({
                        "responseParameters": { response_header("Retry-After"): false },
                        "responseModels": { "text/plain": "Empty" }
                    }),
                ),
            ],
        ),
    ];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "method-response-deletes").await;

    for (case, path_part, verb, responses) in &cases {
        let resource =
            resource_with_a_method(&gateway, &api, &root, path_part, verb, json!({})).await;
        put_all(&gateway, &api, &resource, verb, responses).await;

        let mut remaining: Vec<String> = responses
            .iter()
            .map(|(status, _)| (*status).to_string())
            .collect();
        for (index, (status, _)) in responses.iter().enumerate() {
            // Rows alternate whether a spelling of the verb the resource holds no method
            // under is tried first: a method is addressed by the verb it was created
            // with, so such a request reaches no response to delete.
            if index % 2 == 1 {
                let lowercase = verb.to_ascii_lowercase();
                assert_failure(
                    &delete_method_response(&gateway, &api, &resource, &lowercase, status).await,
                    404,
                    "NotFoundException",
                    &format!("{case} / DeleteMethodResponse({lowercase}/{status})"),
                );
            }
            let addressed = (*verb).to_string();
            let probe = format!("{case} / DeleteMethodResponse({addressed}/{status})");

            let deleted =
                delete_method_response(&gateway, &api, &resource, &addressed, status).await;
            assert_eq!(
                deleted.status, 204,
                "case {probe}: expected the modelled 204, got {} with {:?}",
                deleted.status, deleted.body
            );
            assert!(
                deleted.body.is_null() || deleted.body == json!({}),
                "case {probe}: expected an empty body, got {:?}",
                deleted.body
            );

            // The response is gone for a reader, and a spelling of the verb the resource
            // holds no method under reaches nothing either, and
            // deleting it again finds nothing to delete.
            for verb_probe in [addressed.clone(), verb.to_ascii_lowercase()] {
                assert_failure(
                    &get_method_response(&gateway, &api, &resource, &verb_probe, status).await,
                    404,
                    "NotFoundException",
                    &format!("{probe} / GetMethodResponse({verb_probe})"),
                );
            }
            assert_failure(
                &delete_method_response(&gateway, &api, &resource, &addressed, status).await,
                404,
                "NotFoundException",
                &format!("{probe} / DeleteMethodResponse again"),
            );

            // Every response not yet deleted is untouched, member for member, and the
            // owning method reports exactly those status codes.
            remaining.retain(|kept| kept != status);
            for kept in &remaining {
                let read = get_method_response(&gateway, &api, &resource, verb, kept).await;
                assert_eq!(
                    read.status, 200,
                    "case {probe}: the {kept} response must survive, got {} with {:?}",
                    read.status, read.body
                );
                let method = get_method(&gateway, &api, &resource, verb).await;
                assert_eq!(
                    method.body["methodResponses"][kept], read.body,
                    "case {probe}: the method must still carry {kept}, got {:?}",
                    method.body["methodResponses"]
                );
            }
            let mut expected = remaining.clone();
            expected.sort();
            let reported = reported_status_codes(&gateway, &api, &resource, verb, &probe).await;
            assert_eq!(
                reported, expected,
                "case {probe}: expected methodResponses {expected:?}, got {reported:?}"
            );
        }

        // The method itself survives losing every response, and each freed status code
        // carries a new response again: nothing stale is left to conflict with it.
        let method = get_method(&gateway, &api, &resource, verb).await;
        assert_eq!(
            method.status, 200,
            "case {case}: the method must survive its responses, got {} with {:?}",
            method.status, method.body
        );
        for (status, _) in responses {
            let recreated = put_method_response(
                &gateway,
                &api,
                &resource,
                verb,
                status,
                json!({ "responseModels": { "application/json": "Empty" } }),
            )
            .await;
            assert_eq!(
                recreated.status, 201,
                "case {case} / PutMethodResponse({status}) after the delete: expected 201, \
                 got {} with {:?}",
                recreated.status, recreated.body
            );
            let read = get_method_response(&gateway, &api, &resource, verb, status).await;
            assert_eq!(
                read.body["responseModels"],
                json!({ "application/json": "Empty" }),
                "case {case} / GetMethodResponse({status}) after the delete: expected the new \
                 response, got {:?}",
                read.body
            );
        }
    }
}

#[tokio::test]
async fn a_delete_touches_only_the_response_its_four_identifiers_name() {
    // The same status code on several methods, resources and APIs: a delete is keyed by
    // all four identifiers, so only one of these records goes.
    let gateway = Gateway::new();
    let (first_api, first_root) = create_api(&gateway, "method-response-delete-scope-a").await;
    let (second_api, second_root) = create_api(&gateway, "method-response-delete-scope-b").await;
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

    // Each row is one (api, resource, verb) holding a 200 response; the first row is the
    // one deleted, every other row must survive.
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        ("the named method", &first_api, &first_resource, "GET"),
        ("another verb, same resource", &first_api, &first_resource, "POST"),
        ("another resource, same API", &first_api, &second_resource, "GET"),
        ("another API", &second_api, &other_api_resource, "GET"),
    ];
    for (case, api, resource, verb) in &rows {
        put_all(&gateway, api, resource, verb, &[("200", json!({}))]).await;
        let read = get_method_response(&gateway, api, resource, verb, "200").await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetMethodResponse, got {} with {:?}",
            read.status, read.body
        );
    }

    let deleted =
        delete_method_response(&gateway, &first_api, &first_resource, "GET", "200").await;
    assert_eq!(
        deleted.status, 204,
        "case DeleteMethodResponse(the named method): expected 204, got {} with {:?}",
        deleted.status, deleted.body
    );

    for (index, (case, api, resource, verb)) in rows.iter().enumerate() {
        let read = get_method_response(&gateway, api, resource, verb, "200").await;
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
    let (api, root) = create_api(&gateway, "method-response-delete-notfound").await;
    let resource = resource_with_a_method(&gateway, &api, &root, "pets", "GET", json!({})).await;
    put_all(&gateway, &api, &resource, "GET", &[("200", json!({}))]).await;
    let missing = "zzzzzzzzzz";

    // Rows span each of the four identifiers this operation resolves, one unknown at a
    // time: the API, the resource, the verb naming no method on the resource, and a
    // status code naming no response of the method.
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
        assert_failure(
            &gateway.call("DELETE", uri, None).await,
            404,
            "NotFoundException",
            case,
        );
    }

    // A status code the modelled `StatusCode` pattern `^[1-5]\d\d$` does not admit — one
    // past each published bound, and the lengths it refuses — never reaches the records:
    // the constraint layer in front of the handler refuses it with a 400, under its own
    // error class rather than the operation's `BadRequestException`.
    for status in ["099", "600", "000", "999", "20", "2000", "abc"] {
        assert_failure(
            &delete_method_response(&gateway, &api, &resource, "GET", status).await,
            400,
            "ValidationException",
            &format!("DeleteMethodResponse({status})"),
        );
    }

    // Every refused call removed nothing: the stored response is still there.
    let read = get_method_response(&gateway, &api, &resource, "GET", "200").await;
    assert_eq!(
        read.status, 200,
        "case GetMethodResponse after the refused deletes: expected 200, got {} with {:?}",
        read.status, read.body
    );
    let reported =
        reported_status_codes(&gateway, &api, &resource, "GET", "after the refused deletes").await;
    assert_eq!(
        reported,
        vec!["200".to_string()],
        "case GetMethod after the refused deletes: expected only the 200 response, got {reported:?}"
    );

    // The responses of a method on a deleted API go with it, so a delete naming one
    // afterwards finds nothing.
    let deleted = gateway.call("DELETE", &format!("/restapis/{api}"), None).await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_failure(
        &delete_method_response(&gateway, &api, &resource, "GET", "200").await,
        404,
        "NotFoundException",
        "DeleteMethodResponse after DeleteRestApi",
    );
}
