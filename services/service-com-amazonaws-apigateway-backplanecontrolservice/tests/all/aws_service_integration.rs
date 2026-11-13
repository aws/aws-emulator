// SPDX-License-Identifier: CC0-1.0

//! What a deployed method whose integration is of type `AWS` answers when the call its
//! `uri` names is one this emulator does NOT make.
//!
//! The rule: an `AWS` integration names another service's action
//! (`arn:aws:apigateway:{region}:{subdomain}:path|action/{service_api}`,
//! `Integration$uri`), and a call this emulator cannot make — because the `uri` places none,
//! or because this emulator mounts no such service to answer it — produces no backend answer,
//! so the method is answered as an integration that could not be performed, through the api's
//! own `API_CONFIGURATION_ERROR` gateway response
//! (`500` with `{"message":"Internal server error"}`), and NEVER with a status or a body
//! invented for a call nothing made. That is the other half of the rule the cross-service
//! suite `core/router/tests/all/apigateway_s3_service_integration.rs` pins: there the
//! action IS one this emulator makes, and what the stage answers is the answer the service
//! gave. Only this crate's emulator can pin the half below, because here no object store and
//! no table store is mounted at all.
//!
//! The rows vary what the rule generalises over: which part of the `uri` puts the call out
//! of reach (another service's subdomain, a form that is neither of the two documented
//! ones, an action of the named service this emulator does not perform, a verb that names
//! no action, a placeholder the integration maps nothing onto), and whether the service the
//! `uri` names is mounted at all.

use serde_json::{json, Value};

use crate::execute_api_data_plane::{
    deploy, integration_response, invoke, method_response, put_integration, put_method,
};
use crate::rest_api_lifecycle::{create_api, create_resource, string_member, Gateway};

/// The stage every row is invoked through.
const STAGE: &str = "live";

/// The status API Gateway answers an integration it could not perform with: the
/// `API_CONFIGURATION_ERROR` gateway response.
const INTEGRATION_FAILURE_STATUS: u16 = 500;

/// The sentence that response's default template carries.
const INTEGRATION_FAILURE_MESSAGE: &str = "Internal server error";

/// One row: the case, the integration's `uri`, the verb the integration calls with, and the
/// `requestParameters` it maps (none for most rows).
struct Row {
    case: &'static str,
    path_part: &'static str,
    uri: &'static str,
    integration_verb: &'static str,
    request_parameters: Option<Value>,
}

#[tokio::test]
async fn an_aws_integration_naming_a_call_this_emulator_cannot_make_is_a_configuration_error() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "aws-integration-unreachable").await;

    let rows = [
        Row {
            case: "an action of a service this emulator does not mount, so nothing answers the \
                   integration request",
            path_part: "another-service",
            uri: "arn:aws:apigateway:us-east-1:dynamodb:action/PutItem",
            integration_verb: "POST",
            request_parameters: None,
        },
        Row {
            case: "a form that is neither the path nor the action form",
            path_part: "neither-form",
            uri: "arn:aws:apigateway:us-east-1:s3:shelf/plain.txt",
            integration_verb: "GET",
            request_parameters: None,
        },
        Row {
            case: "an Amazon S3 action this emulator does not perform",
            path_part: "other-action",
            uri: "arn:aws:apigateway:us-east-1:s3:action/ListObjectsV2&Bucket=shelf",
            integration_verb: "GET",
            request_parameters: None,
        },
        Row {
            case: "a verb that names no Amazon S3 object action",
            path_part: "other-verb",
            uri: "arn:aws:apigateway:us-east-1:s3:path/shelf/plain.txt",
            integration_verb: "PATCH",
            request_parameters: None,
        },
        Row {
            case: "a placeholder the integration maps no value onto",
            path_part: "unmapped-placeholder",
            uri: "arn:aws:apigateway:us-east-1:s3:path/shelf/{object}",
            integration_verb: "GET",
            request_parameters: None,
        },
        Row {
            case: "a placeholder mapped from a source this emulator does not read",
            path_part: "unread-mapping",
            uri: "arn:aws:apigateway:us-east-1:s3:path/shelf/{object}",
            integration_verb: "GET",
            request_parameters: Some(json!({
                "integration.request.path.object": "context.requestId",
            })),
        },
        Row {
            case: "an object call in an emulator mounting no object store, so nothing \
                   answers the integration request",
            path_part: "no-object-store",
            uri: "arn:aws:apigateway:us-east-1:s3:path/shelf/plain.txt",
            integration_verb: "GET",
            request_parameters: None,
        },
    ];

    for row in &rows {
        let resource = string_member(
            &create_resource(&gateway, &api, &root, row.path_part).await,
            "id",
            row.case,
        );
        put_method(
            &gateway,
            &api,
            &resource,
            "GET",
            json!({ "authorizationType": "NONE" }),
        )
        .await;
        let mut integration = json!({
            "type": "AWS",
            // `integrationHttpMethod` travels under its `jsonName`, which is what
            // `smithy_query(query="operation",operation="PutIntegration")` reports.
            "httpMethod": row.integration_verb,
            "uri": row.uri,
        });
        if let Some(parameters) = &row.request_parameters {
            integration["requestParameters"] = parameters.clone();
        }
        put_integration(&gateway, &api, &resource, "GET", integration).await;
        // A method response and an integration response for the success the rows never
        // reach: what is asserted is that the stage answers the CONFIGURATION error rather
        // than this mapping, so the mapping has to be there to be answered through.
        method_response(&gateway, &api, &resource, "GET", "200", json!({})).await;
        integration_response(
            &gateway,
            &api,
            &resource,
            "GET",
            "200",
            json!({ "statusCode": "200" }),
        )
        .await;
    }
    deploy(&gateway, &api, STAGE).await;

    for row in &rows {
        let answered = invoke(
            &gateway,
            &api,
            "GET",
            &format!("/{STAGE}/{}", row.path_part),
            &[],
        )
        .await;
        assert_eq!(
            answered.status,
            INTEGRATION_FAILURE_STATUS,
            "case {:?}: expected the api's own integration-failure response, got {} with {:?}",
            row.case,
            answered.status,
            answered.body
        );
        assert_eq!(
            answered.message().as_deref(),
            Some(INTEGRATION_FAILURE_MESSAGE),
            "case {:?}: expected the API Gateway integration-failure message, got {:?}",
            row.case,
            answered.body
        );
        // The answer is the api's own, never the front door's refusal for a host no
        // service claims, and it carries nothing of the call that was not made.
        assert!(
            !answered.body.contains("UnknownService") && !answered.body.contains("shelf"),
            "case {:?}: the stage must answer for itself, got {:?}",
            row.case,
            answered.body
        );
    }
}

#[tokio::test]
async fn test_invoke_method_reports_the_same_configuration_error_the_stage_answers() {
    // `TestInvokeMethod` reports what the data plane performs FROM THE SAME RECORDS, so one
    // stored configuration cannot be answered two ways. The simulated invocation is driven
    // against the same api as the stage invoke below.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "aws-integration-parity").await;
    let resource = string_member(
        &create_resource(&gateway, &api, &root, "unreachable").await,
        "id",
        "CreateResource",
    );
    put_method(
        &gateway,
        &api,
        &resource,
        "GET",
        json!({ "authorizationType": "NONE" }),
    )
    .await;
    put_integration(
        &gateway,
        &api,
        &resource,
        "GET",
        json!({
            "type": "AWS",
            "httpMethod": "GET",
            "uri": "arn:aws:apigateway:us-east-1:dynamodb:action/GetItem",
        }),
    )
    .await;
    method_response(&gateway, &api, &resource, "GET", "200", json!({})).await;
    integration_response(
        &gateway,
        &api,
        &resource,
        "GET",
        "200",
        json!({ "statusCode": "200" }),
    )
    .await;
    deploy(&gateway, &api, STAGE).await;

    let simulated = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/resources/{resource}/methods/GET"),
            Some(json!({})),
        )
        .await;
    assert_eq!(
        simulated.status, 200,
        "TestInvokeMethod must answer its own 200 and report the simulation, got {} with {:?}",
        simulated.status, simulated.body
    );
    let reported = simulated
        .body
        .get("status")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    assert_eq!(
        reported,
        i64::from(INTEGRATION_FAILURE_STATUS),
        "TestInvokeMethod must report the status the stage answers, got {:?}",
        simulated.body
    );

    let answered = invoke(&gateway, &api, "GET", &format!("/{STAGE}/unreachable"), &[]).await;
    assert_eq!(
        answered.status, INTEGRATION_FAILURE_STATUS,
        "the stage must answer the status the simulation reported, got {} with {:?}",
        answered.status, answered.body
    );
}
