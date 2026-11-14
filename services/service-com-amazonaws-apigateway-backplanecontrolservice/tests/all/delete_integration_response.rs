// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `DeleteIntegrationResponse` is SCOPED to: which stored
//! record its identifiers name, and which records a delete must leave alone.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi`/`PutMethod`/`PutIntegration`/`PutIntegrationResponse`/
//! `PutMethodResponse` helpers are this crate's existing harness, reused from
//! `rest_api_lifecycle`, `method_integration`, `integration_response` and
//! `method_response`.
//!
//! The RULE under test: an integration response is keyed by the whole
//! (api, resource, method verb, status code) tuple, the verb exactly as the method was
//! created under, so a
//! delete removes exactly the one record that tuple names — a response at the same status
//! code on another verb, another resource or another API, and the method response keyed by
//! that same status code, are all untouched — and once the record is gone its status code
//! names nothing, so the same status code may be put again rather than conflicting.
//!
//! The sibling `control_plane_deletes` suite asserts the delete/read round trip and the
//! not-found refusals of this operation over a table of status codes on ONE method; these
//! tables span the other identifiers of the key: two APIs, two resources per API, two
//! verbs per resource, and a spelling of the verb the resource holds no method under.

use serde_json::json;

use crate::integration_response::{
    delete_integration_response, get_integration_response, put_integration_response,
};
use crate::method_integration::{
    get_integration, get_method, put_integration, put_method, resource_with_a_method,
};
use crate::method_response::{get_method_response, put_method_response};
use crate::rest_api_lifecycle::{assert_failure, create_api, Gateway};

/// The status codes every method of the fixture carries a response for, spanning the
/// classes the modelled `StatusCode` pattern (`^[1-5]\d\d$`) admits.
const STATUSES: [&str; 3] = ["200", "404", "500"];

/// The verbs each resource of the fixture carries a method for. Two of the verbs
/// `PutMethod` accepts, so one resource holds two independently keyed integrations.
const VERBS: [&str; 2] = ["GET", "POST"];

/// One method of the fixture: where it lives, what verb addresses it, and a label that
/// names it in an assertion.
#[derive(Clone, Debug)]
struct MethodUnderTest {
    label: String,
    api: String,
    resource: String,
    verb: &'static str,
}

/// Two APIs, two resources under each, two methods on each resource, a MOCK integration
/// on each method, and on every integration one response per [`STATUSES`] entry beside a
/// METHOD response at the same status code — the record keyed by the same status code that
/// a delete of an integration response must not touch.
async fn methods_with_integration_responses(gateway: &Gateway) -> Vec<MethodUnderTest> {
    let mut methods = Vec::new();
    for api_name in ["first-api", "second-api"] {
        let (api, root) = create_api(gateway, api_name).await;
        for path_part in ["pets", "owners"] {
            let resource =
                resource_with_a_method(gateway, &api, &root, path_part, VERBS[0], json!({})).await;
            for verb in VERBS {
                let label = format!("{api_name}/{path_part}/{verb}");
                if verb != VERBS[0] {
                    put_method(gateway, &api, &resource, verb).await;
                }
                let created =
                    put_integration(gateway, &api, &resource, verb, json!({ "type": "MOCK" })).await;
                assert_eq!(
                    created.status, 201,
                    "case PutIntegration({label}): expected 201, got {} with {:?}",
                    created.status, created.body
                );
                for status in STATUSES {
                    let put =
                        put_integration_response(gateway, &api, &resource, verb, status, json!({}))
                            .await;
                    assert_eq!(
                        put.status, 201,
                        "case PutIntegrationResponse({label}/{status}): expected 201, \
                         got {} with {:?}",
                        put.status, put.body
                    );
                    let put =
                        put_method_response(gateway, &api, &resource, verb, status, json!({})).await;
                    assert_eq!(
                        put.status, 201,
                        "case PutMethodResponse({label}/{status}): expected 201, got {} with {:?}",
                        put.status, put.body
                    );
                }
                methods.push(MethodUnderTest {
                    label,
                    api: api.clone(),
                    resource: resource.clone(),
                    verb,
                });
            }
        }
    }
    methods
}

/// Assert every `(method, status)` pair of the fixture OTHER than `deleted` still reports
/// its integration response, both through `GetIntegrationResponse` and through the
/// integration that owns it.
async fn assert_siblings_intact(
    gateway: &Gateway,
    methods: &[MethodUnderTest],
    deleted: (usize, &str),
    case: &str,
) {
    let (deleted_method, deleted_status) = deleted;
    for (index, method) in methods.iter().enumerate() {
        let integration =
            get_integration(gateway, &method.api, &method.resource, method.verb).await;
        assert_eq!(
            integration.status, 200,
            "case {case}: expected 200 from GetIntegration({}), got {} with {:?}",
            method.label, integration.status, integration.body
        );
        for status in STATUSES {
            if index == deleted_method && status == deleted_status {
                continue;
            }
            let read =
                get_integration_response(gateway, &method.api, &method.resource, method.verb, status)
                    .await;
            assert_eq!(
                read.status, 200,
                "case {case}: the sibling response {}/{status} must be untouched, \
                 got {} with {:?}",
                method.label, read.status, read.body
            );
            assert_eq!(
                integration.body["integrationResponses"][status]["statusCode"].as_str(),
                Some(status),
                "case {case}: the integration of {} must still report {status}, got {:?}",
                method.label,
                integration.body["integrationResponses"]
            );
        }
    }
}

#[tokio::test]
async fn deleting_an_integration_response_removes_only_the_record_its_identifiers_name() {
    let gateway = Gateway::new();
    let methods = methods_with_integration_responses(&gateway).await;
    // The fixture's methods, in the order they were built: two per resource, two resources
    // per API, two APIs.
    assert_eq!(
        methods.len(),
        8,
        "case fixture: expected eight methods, got {:?}",
        methods.iter().map(|m| m.label.clone()).collect::<Vec<_>>()
    );

    // Rows differ along what the rule generalises over: which identifier of the key
    // distinguishes the target from its siblings (the status code, the verb on one
    // resource, the resource under one API, the API itself) and whether the request is
    // first made under a spelling of the verb the resource holds no method under, which
    // reaches no record at all.
    let rows: [(&str, usize, &str, bool); 5] = [
        ("the success code of the first method", 0, "200", false),
        ("a client error on the sibling verb of one resource", 1, "404", true),
        ("a server error on another resource of the same api", 2, "500", false),
        ("the success code on another api", 4, "200", true),
        ("a client error on the last method", 7, "404", false),
    ];

    for (case, index, status, probe_lowercase_verb) in rows {
        let method = methods[index].clone();
        if probe_lowercase_verb {
            let lowercase = method.verb.to_ascii_lowercase();
            assert_failure(
                &delete_integration_response(
                    &gateway,
                    &method.api,
                    &method.resource,
                    &lowercase,
                    status,
                )
                .await,
                404,
                "NotFoundException",
                &format!("{case} / DeleteIntegrationResponse({lowercase})"),
            );
        }
        let verb = method.verb.to_string();

        let deleted =
            delete_integration_response(&gateway, &method.api, &method.resource, &verb, status)
                .await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected the modelled 204, got {} with {:?}",
            deleted.status, deleted.body
        );
        assert!(
            deleted.body.is_null(),
            "case {case}: expected an empty body, got {:?}",
            deleted.body
        );

        // The named record is gone: the read that reported it refuses, and so does the
        // owning integration's map — read through both the integration and the method that
        // carries it.
        assert_failure(
            &get_integration_response(
                &gateway,
                &method.api,
                &method.resource,
                method.verb,
                status,
            )
            .await,
            404,
            "NotFoundException",
            &format!("GetIntegrationResponse after the delete / {case}"),
        );
        let integration =
            get_integration(&gateway, &method.api, &method.resource, method.verb).await;
        assert!(
            integration.body["integrationResponses"][status].is_null(),
            "case {case}: the integration of {} must stop reporting {status}, got {:?}",
            method.label,
            integration.body["integrationResponses"]
        );
        let read = get_method(&gateway, &method.api, &method.resource, method.verb).await;
        assert!(
            read.body["methodIntegration"]["integrationResponses"][status].is_null(),
            "case {case}: the method of {} must stop reporting {status}, got {:?}",
            method.label,
            read.body["methodIntegration"]
        );

        // The METHOD response keyed by the same status code is a different record, so it
        // is untouched.
        let method_response =
            get_method_response(&gateway, &method.api, &method.resource, method.verb, status).await;
        assert_eq!(
            method_response.status, 200,
            "case {case}: the method response {}/{status} must be untouched, \
             got {} with {:?}",
            method.label, method_response.status, method_response.body
        );

        // Every other response of the fixture — the same status code on the sibling verb,
        // the sibling resource and the other API included — is untouched.
        assert_siblings_intact(&gateway, &methods, (index, status), case).await;

        // With the record gone its status code names nothing, so putting it again is a
        // create (201) rather than the conflict a status code that already names a
        // response is — and the restored record reads back.
        let put = put_integration_response(
            &gateway,
            &method.api,
            &method.resource,
            method.verb,
            status,
            json!({}),
        )
        .await;
        assert_eq!(
            put.status, 201,
            "case {case}: expected 201 from PutIntegrationResponse after the delete, \
             got {} with {:?}",
            put.status, put.body
        );
        let read =
            get_integration_response(&gateway, &method.api, &method.resource, method.verb, status)
                .await;
        assert_eq!(
            read.body["statusCode"].as_str(),
            Some(status),
            "case {case}: the re-put response must read back, got {:?}",
            read.body
        );
    }
}

#[tokio::test]
async fn emptying_one_integrations_responses_leaves_every_other_integrations_map_whole() {
    let gateway = Gateway::new();
    let methods = methods_with_integration_responses(&gateway).await;

    // Rows differ along which integration is emptied: one in the middle of the fixture and
    // the last one, so the map's disappearance is not keyed to a position.
    for index in [1usize, 7] {
        let method = methods[index].clone();
        for status in STATUSES {
            let deleted = delete_integration_response(
                &gateway,
                &method.api,
                &method.resource,
                method.verb,
                status,
            )
            .await;
            assert_eq!(
                deleted.status, 204,
                "case emptying {}: expected 204 deleting {status}, got {} with {:?}",
                method.label, deleted.status, deleted.body
            );
        }

        // Its last response gone, the emptied integration reports no map at all, while the
        // integration itself and the method's own responses stay.
        let integration =
            get_integration(&gateway, &method.api, &method.resource, method.verb).await;
        assert!(
            integration.body["integrationResponses"].is_null(),
            "case emptying {}: expected no integrationResponses map, got {:?}",
            method.label,
            integration.body
        );
        assert_eq!(
            integration.body["type"].as_str(),
            Some("MOCK"),
            "case emptying {}: the integration itself must be untouched, got {:?}",
            method.label,
            integration.body
        );
        for status in STATUSES {
            let method_response =
                get_method_response(&gateway, &method.api, &method.resource, method.verb, status)
                    .await;
            assert_eq!(
                method_response.status, 200,
                "case emptying {}: the method response at {status} must be untouched, \
                 got {} with {:?}",
                method.label, method_response.status, method_response.body
            );
        }

        // Every integration this emptying did not name still reports all of its responses.
        for (other, sibling) in methods.iter().enumerate() {
            if other == index {
                continue;
            }
            let integration =
                get_integration(&gateway, &sibling.api, &sibling.resource, sibling.verb).await;
            for status in STATUSES {
                assert_eq!(
                    integration.body["integrationResponses"][status]["statusCode"].as_str(),
                    Some(status),
                    "case emptying {}: the integration of {} must still report {status}, \
                     got {:?}",
                    method.label,
                    sibling.label,
                    integration.body["integrationResponses"]
                );
            }
        }

        // Put the emptied integration's responses back, so the next row starts from the
        // same fixture: a status code freed by a delete is a create again.
        for status in STATUSES {
            let put = put_integration_response(
                &gateway,
                &method.api,
                &method.resource,
                method.verb,
                status,
                json!({}),
            )
            .await;
            assert_eq!(
                put.status, 201,
                "case emptying {}: expected 201 re-putting {status}, got {} with {:?}",
                method.label, put.status, put.body
            );
        }
    }
}

#[tokio::test]
async fn deleting_the_responses_of_an_integration_leaves_the_integration_itself_deletable() {
    // Deleting an integration response removes the response alone, so the integration
    // record that owned it is still there to be deleted afterwards — whether its responses
    // were removed one at a time first or not at all.
    let rows: [(&str, &str, &str, bool); 4] = [
        ("responses removed first, on a GET", "responses-first", "GET", true),
        ("responses left in place, on a GET", "responses-left", "GET", false),
        ("responses removed first, on a POST", "post-first", "POST", true),
        ("responses left in place, on a POST", "post-left", "POST", false),
    ];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-delete-after-responses").await;
    for (case, path_part, verb, remove_responses) in rows {
        let resource =
            resource_with_a_method(&gateway, &api, &root, path_part, verb, json!({})).await;
        let created =
            put_integration(&gateway, &api, &resource, verb, json!({ "type": "MOCK" })).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from PutIntegration, got {} with {:?}",
            created.status, created.body
        );
        for status in STATUSES {
            let put =
                put_integration_response(&gateway, &api, &resource, verb, status, json!({})).await;
            assert_eq!(
                put.status, 201,
                "case {case}: expected 201 from PutIntegrationResponse({status}), \
                 got {} with {:?}",
                put.status, put.body
            );
        }
        if remove_responses {
            for status in STATUSES {
                let deleted =
                    delete_integration_response(&gateway, &api, &resource, verb, status).await;
                assert_eq!(
                    deleted.status, 204,
                    "case {case}: expected 204 deleting the response at {status}, \
                     got {} with {:?}",
                    deleted.status, deleted.body
                );
            }
        }

        let deleted = gateway
            .call(
                "DELETE",
                &format!("/restapis/{api}/resources/{resource}/methods/{verb}/integration"),
                None,
            )
            .await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected 204 from DeleteIntegration, got {} with {:?}",
            deleted.status, deleted.body
        );
        // The integration and every response that hung off it are gone with it.
        assert_failure(
            &get_integration(&gateway, &api, &resource, verb).await,
            404,
            "NotFoundException",
            &format!("GetIntegration after DeleteIntegration / {case}"),
        );
        for status in STATUSES {
            assert_failure(
                &get_integration_response(&gateway, &api, &resource, verb, status).await,
                404,
                "NotFoundException",
                &format!("GetIntegrationResponse({status}) after DeleteIntegration / {case}"),
            );
        }
    }
}
