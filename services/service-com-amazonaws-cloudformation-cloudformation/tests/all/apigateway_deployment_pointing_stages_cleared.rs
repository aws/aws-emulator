// SPDX-License-Identifier: CC0-1.0

//! Giving up an `AWS::ApiGateway::Deployment` while stages of its api still point at it, end
//! to end over the awsQuery wire.
//!
//! RULE under test: **a resource provider's delete clears the association the owning service
//! requires cleared before it will remove the resource. API Gateway refuses to delete a
//! deployment any stage still points at — "deleting a deployment will only succeed if there
//! are no Stage resources associated with it", whose refusal says the pointing stages "must
//! be moved or deleted" — so the deployment's delete takes down EVERY stage of that api still
//! serving it, whoever put the stage there, and the stack's deletion is carried out to the
//! end. A stage that serves another deployment is not part of that association and is left
//! exactly as it is.**
//!
//! Why it matters: a stack that cannot delete its deployment cannot be deleted at all — the
//! client is left with an undeletable stack and leaked API Gateway resources, and every later
//! "is it gone?" read is unreachable.
//!
//! The stages a deployment is addressed with come from that service's own listing, which
//! narrows an api's stages by the deployment they serve: `GetStages`' `deploymentId` query
//! parameter is "the stages' deployment identifiers"
//! (`smithy_query(query="operation",operation="GetStages",model_path="services/service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json")`).
//!
//! The first table's rows differ along what the rule generalises over: WHO left the stage
//! pointing at the deployment — the stack itself, through a stage resource replaced under
//! `UpdateReplacePolicy: Retain`, and a client calling `CreateStage` against the api outside
//! the stack — and HOW MANY such stages are left. The second case is the other half of the
//! rule: with the api created outside the stack, so that it outlives the stack's deletion,
//! the stage serving a different deployment is still there afterwards while the ones serving
//! the deleted deployment are gone.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::apigateway_vpc_link_resources::{apigateway, apigateway_with_body};
use crate::harness::{call, member, router, settled_stack, xml_text, xml_texts, REGION};
use crate::stack_lifecycle::resource_event_statuses;

/// The statuses this suite holds its stacks and their resources to.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";
const DELETE_COMPLETE: &str = "DELETE_COMPLETE";
const DELETE_FAILED: &str = "DELETE_FAILED";

/// The stage the stack's own stage resource is first declared with, and the one it is renamed
/// to — a stage keeps the name `UpdateStage` addresses it by, so restating the name replaces
/// the resource and leaves the first stage behind under `UpdateReplacePolicy: Retain`.
const FIRST_STAGE: &str = "first";
const SECOND_STAGE: &str = "second";

/// One row: the stack deployed, whether the stack leaves a stage of its own behind by
/// replacing its stage resource under `Retain`, and the stages a client creates against the
/// api outside the stack — each pointing at the stack's deployment.
struct Row {
    case: &'static str,
    stack: &'static str,
    retained_by_a_replacement: bool,
    created_outside: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        case: "a stage the stack retained when it replaced its stage resource",
        stack: "apigw-pointing-stage-retained",
        retained_by_a_replacement: true,
        created_outside: &[],
    },
    Row {
        case: "a stage another client created against the stack's deployment",
        stack: "apigw-pointing-stage-outside",
        retained_by_a_replacement: false,
        created_outside: &["adopted"],
    },
    Row {
        case: "a retained stage and two more created outside the stack",
        stack: "apigw-pointing-stages-many",
        retained_by_a_replacement: true,
        created_outside: &["adopted", "mirror"],
    },
];

/// The template a row deploys: the api, one `MOCK` method on its root so a deployment is a
/// snapshot of something, the deployment itself, and the stage resource that serves it —
/// declared `UpdateReplacePolicy: Retain`, so a row that renames the stage leaves the first
/// one in the api.
fn template(stack: &str, stage: &str) -> String {
    json!({
        "Resources": {
            "Api": {
                "Type": "AWS::ApiGateway::RestApi",
                "Properties": {
                    "Name": format!("{stack}-api"),
                    "EndpointConfiguration": { "Types": ["REGIONAL"] },
                },
            },
            "RootGet": {
                "Type": "AWS::ApiGateway::Method",
                "Properties": {
                    "RestApiId": { "Ref": "Api" },
                    "ResourceId": { "Fn::GetAtt": ["Api", "RootResourceId"] },
                    "HttpMethod": "GET",
                    "AuthorizationType": "NONE",
                    "Integration": {
                        "Type": "MOCK",
                        "RequestTemplates": { "application/json": "{\"statusCode\":200}" },
                    },
                },
            },
            "Deployment": {
                "Type": "AWS::ApiGateway::Deployment",
                "DependsOn": ["RootGet"],
                "Properties": { "RestApiId": { "Ref": "Api" } },
            },
            "Stage": {
                "Type": "AWS::ApiGateway::Stage",
                "UpdateReplacePolicy": "Retain",
                "Properties": {
                    "RestApiId": { "Ref": "Api" },
                    "DeploymentId": { "Ref": "Deployment" },
                    "StageName": stage,
                },
            },
        },
        "Outputs": {
            "ApiId": { "Value": { "Ref": "Api" } },
            "DeploymentId": { "Value": { "Ref": "Deployment" } },
        },
    })
    .to_string()
}

/// Deploy a template and answer the settled stack's own `DescribeStacks` body, holding the
/// operation to the status it is owed.
///
/// Nothing here waits for a span: a stack's terminal status is due at an instant this service
/// records, so the wait is for that status APPEARING ([`settled_stack`]).
async fn deploy(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    stack: &str,
    body: &str,
) -> String {
    let (code, answered) = call(
        router,
        action,
        &[member("StackName", stack), member("TemplateBody", body)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected {action} to answer HTTP 200, got {code} with body {answered}"
    );
    let described = settled_stack(case, router, REGION, stack).await;
    let settled = xml_text(&described, "StackStatus").unwrap_or_default();
    let wanted = if action == "CreateStack" {
        CREATE_COMPLETE
    } else {
        UPDATE_COMPLETE
    };
    if settled != wanted {
        let (_, events) = call(router, "DescribeStackEvents", &[member("StackName", stack)]).await;
        panic!(
            "case {case:?}: expected {action} to reach {wanted}, got {settled:?} with resource \
             failures {:?}",
            xml_texts(&events, "ResourceStatusReason")
        );
    }
    described
}

/// The value of the stack output `name`, or a panic naming what the stack published instead.
fn output(described: &str, name: &str, case: &str) -> String {
    xml_texts(described, "OutputKey")
        .into_iter()
        .zip(xml_texts(described, "OutputValue"))
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
        .unwrap_or_else(|| {
            panic!("case {case:?}: the stack published no {name} output: {described}")
        })
}

/// The stack id the settled stack reports, which is what addresses a stack whose DELETION has
/// completed: "information for stacks that have been deleted is kept", and a name no longer
/// resolves to one.
fn stack_id(described: &str, case: &str) -> String {
    xml_text(described, "StackId")
        .unwrap_or_else(|| panic!("case {case:?}: the stack reported no StackId: {described}"))
}

/// Create one stage of `api_id` through API Gateway's own `CreateStage`, serving
/// `deployment_id`: a stage the stack did not declare and does not know about.
async fn create_stage(
    router: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    stage: &str,
    deployment_id: &str,
) {
    let (code, created) = apigateway_with_body(
        router,
        "POST",
        &format!("/restapis/{api_id}/stages"),
        Some(&json!({ "stageName": stage, "deploymentId": deployment_id })),
    )
    .await;
    assert_eq!(
        code, 201,
        "case {case:?}: expected CreateStage({stage}) to answer 201, got {code} with {created}"
    );
}

/// What API Gateway reports for one stage: the HTTP status of its own read and the deployment
/// the stage serves.
async fn read_stage(
    router: &MultiServiceRouter,
    api_id: &str,
    stage: &str,
) -> (u16, Option<String>) {
    let (code, document) =
        apigateway(router, "GET", &format!("/restapis/{api_id}/stages/{stage}")).await;
    let serves = document
        .get("deploymentId")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    (code, serves)
}

/// Delete the stack and answer the status it SETTLED in, read off the stack id so a completed
/// deletion is reported rather than left unaddressable.
async fn delete_stack(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    stack_id: &str,
) -> String {
    let (code, body) = call(router, "DeleteStack", &[member("StackName", stack)]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected DeleteStack to answer HTTP 200, got {code} with body {body}"
    );
    let described = settled_stack(case, router, REGION, stack_id).await;
    let settled = xml_text(&described, "StackStatus").unwrap_or_default();
    assert_eq!(
        settled,
        DELETE_COMPLETE,
        "case {case:?}: expected the deletion to be carried out, got {settled:?} with reason \
         {:?}",
        xml_text(&described, "StackStatusReason")
    );
    settled
}

#[tokio::test]
async fn a_deployment_given_up_takes_every_stage_still_pointing_at_it() {
    for row in ROWS {
        let router = router();
        let described = deploy(
            &router,
            row.case,
            "CreateStack",
            row.stack,
            &template(row.stack, FIRST_STAGE),
        )
        .await;
        let stack = stack_id(&described, row.case);
        let api_id = output(&described, "ApiId", row.case);
        let deployment_id = output(&described, "DeploymentId", row.case);

        // The stages left pointing at the stack's deployment, each put there by a different
        // hand: the stack's own replacement under `Retain`, and a client's `CreateStage`.
        let mut pointing: Vec<String> = Vec::new();
        if row.retained_by_a_replacement {
            deploy(
                &router,
                row.case,
                "UpdateStack",
                row.stack,
                &template(row.stack, SECOND_STAGE),
            )
            .await;
            pointing.push(FIRST_STAGE.to_string());
        }
        for stage in row.created_outside {
            create_stage(&router, row.case, &api_id, stage, &deployment_id).await;
            pointing.push((*stage).to_string());
        }

        // What the deletion is up against really exists: every stage of the row serves the
        // deployment the stack is about to give up.
        for stage in &pointing {
            let (code, serves) = read_stage(&router, &api_id, stage).await;
            assert_eq!(
                code, 200,
                "case {:?}: expected API Gateway to hold the stage {stage:?}, got {code}",
                row.case
            );
            assert_eq!(
                serves.as_deref(),
                Some(deployment_id.as_str()),
                "case {:?}: expected {stage:?} to serve {deployment_id}, got {serves:?}",
                row.case
            );
        }

        delete_stack(&router, row.case, row.stack, &stack).await;

        // The deployment's own deletion was CARRIED OUT: a delete the owning service refused
        // is what records the failed one.
        let statuses = resource_event_statuses(&router, row.case, &stack, "Deployment").await;
        assert!(
            statuses.iter().any(|status| status == DELETE_COMPLETE)
                && !statuses.iter().any(|status| status == DELETE_FAILED),
            "case {:?}: expected the deployment's deletion to be carried out, got {statuses:?}",
            row.case
        );

        // ...and nothing of the api is left, the stages that pointed at the deployment
        // included.
        for stage in &pointing {
            let (code, serves) = read_stage(&router, &api_id, stage).await;
            assert_eq!(
                code, 404,
                "case {:?}: expected {stage:?} to be gone with the api, got {code} serving \
                 {serves:?}",
                row.case
            );
        }
        let (code, api) = apigateway(&router, "GET", &format!("/restapis/{api_id}")).await;
        assert_eq!(
            code, 404,
            "case {:?}: expected the api to be gone, got {code} with {api}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_stage_serving_another_deployment_is_left_as_it_is() {
    let case = "an api whose stages serve two deployments";
    let stack = "apigw-pointing-stage-other-deployment";
    // The stage the stack declares, one left behind pointing at the stack's deployment, and
    // one serving a deployment of its own.
    const SERVED: &str = "served";
    const STALE: &str = "stale";
    const OTHER: &str = "other";
    let router = router();

    // An api created through API Gateway rather than by the stack, so it OUTLIVES the stack's
    // deletion and what became of each of its stages can be read afterwards.
    let (code, api) = apigateway_with_body(
        &router,
        "POST",
        "/restapis",
        Some(&json!({ "name": format!("{stack}-api") })),
    )
    .await;
    assert_eq!(
        code, 201,
        "case {case:?}: expected CreateRestApi to answer 201, got {code} with {api}"
    );
    let api_id = api
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: CreateRestApi answered no id: {api}"))
        .to_string();
    let root_id = api
        .get("rootResourceId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: CreateRestApi answered no rootResourceId: {api}"))
        .to_string();

    // One method on the api's root, so a deployment of it is a snapshot of something.
    let (code, method) = apigateway_with_body(
        &router,
        "PUT",
        &format!("/restapis/{api_id}/resources/{root_id}/methods/GET"),
        Some(&json!({ "authorizationType": "NONE" })),
    )
    .await;
    assert_eq!(
        code, 201,
        "case {case:?}: expected PutMethod to answer 201, got {code} with {method}"
    );
    // And its integration, which is what a stage serves a matched request with: an api
    // holding a method with none cannot be deployed at all.
    let (code, integration) = apigateway_with_body(
        &router,
        "PUT",
        &format!("/restapis/{api_id}/resources/{root_id}/methods/GET/integration"),
        Some(&json!({ "type": "MOCK" })),
    )
    .await;
    assert_eq!(
        code, 201,
        "case {case:?}: expected PutIntegration to answer 201, got {code} with {integration}"
    );

    // The stack holds the deployment and the stage that serves it, against that api.
    let declared = json!({
        "Resources": {
            "Deployment": {
                "Type": "AWS::ApiGateway::Deployment",
                "Properties": { "RestApiId": api_id },
            },
            "Stage": {
                "Type": "AWS::ApiGateway::Stage",
                "Properties": {
                    "RestApiId": api_id,
                    "DeploymentId": { "Ref": "Deployment" },
                    "StageName": SERVED,
                },
            },
        },
        "Outputs": { "DeploymentId": { "Value": { "Ref": "Deployment" } } },
    })
    .to_string();
    let described = deploy(&router, case, "CreateStack", stack, &declared).await;
    let stack_id = stack_id(&described, case);
    let deployment_id = output(&described, "DeploymentId", case);

    // A second deployment of the same api with a stage of its own, and one more stage left
    // pointing at the stack's deployment.
    let (code, other) = apigateway_with_body(
        &router,
        "POST",
        &format!("/restapis/{api_id}/deployments"),
        Some(&json!({})),
    )
    .await;
    assert_eq!(
        code, 201,
        "case {case:?}: expected CreateDeployment to answer 201, got {code} with {other}"
    );
    let other_deployment = other
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: CreateDeployment answered no id: {other}"))
        .to_string();
    create_stage(&router, case, &api_id, OTHER, &other_deployment).await;
    create_stage(&router, case, &api_id, STALE, &deployment_id).await;

    delete_stack(&router, case, stack, &stack_id).await;

    // The stages that pointed at the deployment the stack gave up are gone — the association
    // API Gateway requires cleared was cleared...
    for stage in [SERVED, STALE] {
        let (code, serves) = read_stage(&router, &api_id, stage).await;
        assert_eq!(
            code, 404,
            "case {case:?}: expected {stage:?} to go with the deployment it served, got {code} \
             serving {serves:?}"
        );
    }
    // ...and the stage serving the OTHER deployment is left exactly as it is, on the api the
    // stack never owned.
    let (code, serves) = read_stage(&router, &api_id, OTHER).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected {OTHER:?} to be left as it is, got {code}"
    );
    assert_eq!(
        serves.as_deref(),
        Some(other_deployment.as_str()),
        "case {case:?}: expected {OTHER:?} to still serve {other_deployment}, got {serves:?}"
    );
    let (code, held) = apigateway(
        &router,
        "GET",
        &format!("/restapis/{api_id}/deployments/{other_deployment}"),
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected the other deployment to be left as it is, got {code} with \
         {held}"
    );
    let (code, held) = apigateway(&router, "GET", &format!("/restapis/{api_id}")).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected the api the stack never owned to be left as it is, got {code} \
         with {held}"
    );
}
