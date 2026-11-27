// SPDX-License-Identifier: CC0-1.0

//! What a resource provider of the API Gateway REST control plane reports for a create whose
//! IDENTIFIER the owning service already holds, end to end over the awsQuery wire.
//!
//! RULE under test: **a resource provider checks the already-exists precondition ITSELF, so a
//! create whose identifier is already taken fails the resource with CloudFormation's own
//! already-exists sentence — `Resource of type '<type>' with identifier '<identifier>' already
//! exists.`, whose identifier is every part of that type's primary identifier labelled with the
//! property the declaration states it under — quoted in the provider envelope under the
//! `AlreadyExists` `HandlerErrorCode`, with no SDK diagnostic of a called service inside it,
//! and the resource that was already there is left exactly as it was.** The owning service's own
//! conflict exception is not what the event carries: it names neither the type nor the colliding
//! identifier, and the class its code falls in says a conflict a retry of the same request might
//! clear.
//!
//! Why it matters: a client triaging a rolled-back deployment (a CDK deploy, the CLI's output,
//! any rollback triage) reads the reason to learn WHICH resource identifier collided, and
//! branches on the `HandlerErrorCode` to tell a name collision — which no retry of the same
//! template clears, so the resource has to be renamed — from a transient conflict, which is
//! retried as it stands. A reason carrying the raw service exception names neither, so the
//! triage takes the wrong branch.
//!
//! The rows differ along what the rule generalises over: WHICH type collides — a stage, a method
//! and a model, whose primary identifiers are two and three properties long — and WHO holds the
//! identifier (a sibling resource of the same stack, and a resource created outside
//! CloudFormation altogether, through the owning service's own wire).
//!
//! `apigateway_deployment_stage_teardown.rs` owns what becomes of the stack around the recorded
//! collision (the rollback, and the stage the deployment created going with it); this suite is
//! about what the failed resource REPORTS.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::apigateway_vpc_link_resources::{apigateway, apigateway_with_body};
use crate::harness::{
    envelope_parts, failed_resource_reason, is_a_request_id, physical_resource_id,
    settled_stack_status, REGION,
};
use crate::stack_lifecycle::{create_stack, member, router};

/// The resource status a create its provider refused records, and the stack statuses the two
/// failure handlings settle in: the create kept as it failed, and the rollback of it.
const CREATE_FAILED: &str = "CREATE_FAILED";
const ROLLBACK_COMPLETE: &str = "ROLLBACK_COMPLETE";

/// The `HandlerErrorCode` class a create refused because the resource is already there falls
/// in, from the model's own `HandlerErrorCode` enum
/// (`smithy_query(query="shape",shape="HandlerErrorCode")`).
const ALREADY_EXISTS: &str = "AlreadyExists";

/// The types the rows collide on.
const STAGE_TYPE: &str = "AWS::ApiGateway::Stage";
const METHOD_TYPE: &str = "AWS::ApiGateway::Method";
const MODEL_TYPE: &str = "AWS::ApiGateway::Model";

/// The properties each of those types is identified by, in the order its own schema declares
/// its primary identifier in — which is also the order every call of API Gateway addresses one
/// of those resources by (`GetStage` takes `restApiId` beside `stageName`, `GetMethod` takes
/// `restApiId`, `resourceId` and `httpMethod`, `GetModel` takes `restApiId` beside `modelName`).
const REST_API_ID: &str = "RestApiId";
const STAGE_NAME: &str = "StageName";
const RESOURCE_ID: &str = "ResourceId";
const HTTP_METHOD: &str = "HttpMethod";
const NAME: &str = "Name";

/// The verb the method rows declare, which is part of a method's identity.
const VERB: &str = "GET";

/// The stage name the sibling-deployment row collides on, and the one the row whose stage was
/// created outside CloudFormation collides on.
const STAGE_MINTED_BY_THE_DEPLOYMENT: &str = "dup";
const STAGE_HELD_OUTSIDE: &str = "held";

/// The model name the model row collides on, in the alphabet `CreateModel$name` admits ("must
/// be alphanumeric").
const MODEL_NAME: &str = "EchoRequest";

/// The content type the declared and the held model are each the schema for.
const JSON: &str = "application/json";

/// The description the model created OUTSIDE the stack carries, against the one the stack's own
/// declaration states: a create that had been carried out would leave the declaration's there,
/// so the two say whether anything was taken over.
const HELD_MODEL_DESCRIPTION: &str = "the model a client created";
const DECLARED_MODEL_DESCRIPTION: &str = "the model the stack declares";

/// The `OperationName` each of the two method declarations states, for the same reason.
const FIRST_OPERATION: &str = "listItems";
const SECOND_OPERATION: &str = "replaceItems";

/// Which collision one row deploys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Collision {
    /// An `AWS::ApiGateway::Stage` whose `StageName` the sibling `AWS::ApiGateway::Deployment`
    /// of the same template already minted (a deployment declaring a `StageName` creates that
    /// stage). The recorded case.
    StageAfterItsSiblingDeployment,
    /// An `AWS::ApiGateway::Stage` whose name a stage created through API Gateway's own wire —
    /// outside CloudFormation altogether — already holds.
    StageHeldOutsideTheStack,
    /// A second `AWS::ApiGateway::Method` on the resource and verb the first declaration of the
    /// same template already answers.
    MethodAfterItsSibling,
    /// An `AWS::ApiGateway::Model` whose name a model created outside CloudFormation holds.
    ModelHeldOutsideTheStack,
}

/// One row: the collision, the stack that deploys it, and the resource the failure is about.
struct Row {
    case: &'static str,
    stack: &'static str,
    collision: Collision,
    /// The type of the resource that fails, and the logical id its template declares it under.
    resource_type: &'static str,
    logical_id: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        case: "a stage whose name its sibling deployment already minted",
        stack: "apigw-already-exists-stage-sibling",
        collision: Collision::StageAfterItsSiblingDeployment,
        resource_type: STAGE_TYPE,
        logical_id: "Stage",
    },
    Row {
        case: "a stage whose name a stage created outside CloudFormation holds",
        stack: "apigw-already-exists-stage-outside",
        collision: Collision::StageHeldOutsideTheStack,
        resource_type: STAGE_TYPE,
        logical_id: "Stage",
    },
    Row {
        case: "a method on the resource and verb its sibling already answers",
        stack: "apigw-already-exists-method-sibling",
        collision: Collision::MethodAfterItsSibling,
        resource_type: METHOD_TYPE,
        logical_id: "Second",
    },
    Row {
        case: "a model whose name a model created outside CloudFormation holds",
        stack: "apigw-already-exists-model-outside",
        collision: Collision::ModelHeldOutsideTheStack,
        resource_type: MODEL_TYPE,
        logical_id: "Model",
    },
];

/// CloudFormation's own sentence for a resource that cannot be created because it is already
/// there: fixed text apart from the declared type and the identifier that was taken.
fn already_exists_sentence(resource_type: &str, identifier: &str) -> String {
    format!("Resource of type '{resource_type}' with identifier '{identifier}' already exists.")
}

/// The identifier that sentence names for a type identified by MORE THAN ONE property: every
/// part labelled with the property the declaration states it under, joined by a single space.
fn identifier(parts: &[(&str, &str)]) -> String {
    parts
        .iter()
        .map(|(label, value)| format!("{label}: {value}"))
        .collect::<Vec<String>>()
        .join(" ")
}

/// The api, the method under its root, the deployment that mints a stage and the stage resource
/// that declares the same name: the recorded template, and the one the rollback test deploys
/// too.
fn stage_after_deployment_template(stack: &str) -> String {
    json!({
        "Resources": {
            "Api": {
                "Type": "AWS::ApiGateway::RestApi",
                "Properties": {
                    "Name": format!("{stack}-api"),
                    "EndpointConfiguration": { "Types": ["REGIONAL"] },
                },
            },
            "Root": {
                "Type": "AWS::ApiGateway::Method",
                "Properties": {
                    "RestApiId": { "Ref": "Api" },
                    "ResourceId": { "Fn::GetAtt": ["Api", "RootResourceId"] },
                    "HttpMethod": VERB,
                    "AuthorizationType": "NONE",
                    "Integration": {
                        "Type": "MOCK",
                        "RequestTemplates": { JSON: "{\"statusCode\":200}" },
                    },
                },
            },
            "Deployment": {
                "Type": "AWS::ApiGateway::Deployment",
                "DependsOn": ["Root"],
                "Properties": {
                    "RestApiId": { "Ref": "Api" },
                    "StageName": STAGE_MINTED_BY_THE_DEPLOYMENT,
                },
            },
            "Stage": {
                "Type": "AWS::ApiGateway::Stage",
                "Properties": {
                    "RestApiId": { "Ref": "Api" },
                    "DeploymentId": { "Ref": "Deployment" },
                    "StageName": STAGE_MINTED_BY_THE_DEPLOYMENT,
                },
            },
        },
    })
    .to_string()
}

/// What one row needs in API Gateway before its stack is deployed, and the template it deploys.
struct Prepared {
    template: String,
    /// The api the row's resources live in, where the ROW created it outside the stack; `None`
    /// where the template declares the api itself.
    api_id: Option<String>,
    /// The deployment the stage held outside the stack serves, for the row that has one.
    held_deployment: Option<String>,
    /// Whether the request must state `DisableValidation`: an identifier the OWNING SERVICE
    /// already holds fails CloudFormation's pre-deployment existence check for a type whose
    /// name the template states, before anything is provisioned
    /// (`state::early_validation`), so a row that collides with a resource created outside the
    /// stack reaches the handler only with that stage off. A collision a SIBLING of the same
    /// stack mints is invisible to it — nothing holds the name when the template is weighed —
    /// so those rows leave the validations on.
    disable_validation: bool,
}

/// Create one api through API Gateway's own wire, with one `GET` method on its root so a
/// deployment of it is a snapshot of something, and answer its id.
async fn api_outside_the_stack(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (code, api) = apigateway_with_body(
        router,
        "POST",
        "/restapis",
        Some(&json!({ "name": name })),
    )
    .await;
    assert_eq!(
        code, 201,
        "case {case:?}: CreateRestApi must answer 201, got {code} with {api}"
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
    let (code, method) = apigateway_with_body(
        router,
        "PUT",
        &format!("/restapis/{api_id}/resources/{root_id}/methods/{VERB}"),
        Some(&json!({ "authorizationType": "NONE" })),
    )
    .await;
    assert_eq!(
        code, 201,
        "case {case:?}: PutMethod must answer 201, got {code} with {method}"
    );
    // The method's integration, which is what a stage serves a matched request with: an
    // api holding a method with none cannot be deployed at all. `MOCK` is the integration
    // type that needs no backend.
    let (code, integration) = apigateway_with_body(
        router,
        "PUT",
        &format!("/restapis/{api_id}/resources/{root_id}/methods/{VERB}/integration"),
        Some(&json!({ "type": "MOCK" })),
    )
    .await;
    assert_eq!(
        code, 201,
        "case {case:?}: PutIntegration must answer 201, got {code} with {integration}"
    );
    api_id
}

/// Create one deployment of `api_id` through that service's own wire, and answer its id.
async fn deployment_outside_the_stack(
    router: &MultiServiceRouter,
    case: &str,
    api_id: &str,
) -> String {
    let (code, deployment) = apigateway_with_body(
        router,
        "POST",
        &format!("/restapis/{api_id}/deployments"),
        Some(&json!({})),
    )
    .await;
    assert_eq!(
        code, 201,
        "case {case:?}: CreateDeployment must answer 201, got {code} with {deployment}"
    );
    deployment
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: CreateDeployment answered no id: {deployment}"))
        .to_string()
}

/// Set a row up and answer the template it deploys.
async fn prepare(router: &MultiServiceRouter, row: &Row) -> Prepared {
    match row.collision {
        Collision::StageAfterItsSiblingDeployment => Prepared {
            template: stage_after_deployment_template(row.stack),
            api_id: None,
            held_deployment: None,
            disable_validation: false,
        },
        Collision::StageHeldOutsideTheStack => {
            let api_id =
                api_outside_the_stack(router, row.case, &format!("{}-api", row.stack)).await;
            // The stage that holds the name, serving a deployment of its own; the stack's own
            // declaration points at a SECOND deployment, so a create that had been carried out
            // would leave the held stage serving that one instead.
            let held_deployment = deployment_outside_the_stack(router, row.case, &api_id).await;
            let declared_deployment = deployment_outside_the_stack(router, row.case, &api_id).await;
            let (code, created) = apigateway_with_body(
                router,
                "POST",
                &format!("/restapis/{api_id}/stages"),
                Some(&json!({
                    "stageName": STAGE_HELD_OUTSIDE,
                    "deploymentId": held_deployment,
                })),
            )
            .await;
            assert_eq!(
                code, 201,
                "case {:?}: CreateStage must answer 201, got {code} with {created}",
                row.case
            );
            let template = json!({
                "Resources": {
                    "Stage": {
                        "Type": STAGE_TYPE,
                        "Properties": {
                            "RestApiId": api_id,
                            "DeploymentId": declared_deployment,
                            "StageName": STAGE_HELD_OUTSIDE,
                        },
                    },
                },
            })
            .to_string();
            Prepared {
                template,
                api_id: Some(api_id),
                held_deployment: Some(held_deployment),
                disable_validation: true,
            }
        }
        Collision::MethodAfterItsSibling => {
            let template = json!({
                "Resources": {
                    "Api": {
                        "Type": "AWS::ApiGateway::RestApi",
                        "Properties": {
                            "Name": format!("{}-api", row.stack),
                            "EndpointConfiguration": { "Types": ["REGIONAL"] },
                        },
                    },
                    "Items": {
                        "Type": "AWS::ApiGateway::Resource",
                        "Properties": {
                            "RestApiId": { "Ref": "Api" },
                            "ParentId": { "Fn::GetAtt": ["Api", "RootResourceId"] },
                            "PathPart": "items",
                        },
                    },
                    "First": {
                        "Type": METHOD_TYPE,
                        "Properties": {
                            "RestApiId": { "Ref": "Api" },
                            "ResourceId": { "Ref": "Items" },
                            "HttpMethod": VERB,
                            "AuthorizationType": "NONE",
                            "OperationName": FIRST_OPERATION,
                            "Integration": { "Type": "MOCK" },
                        },
                    },
                    // The colliding declaration, ordered after the first so the collision is
                    // the second create rather than a race between two independent ones.
                    "Second": {
                        "Type": METHOD_TYPE,
                        "DependsOn": ["First"],
                        "Properties": {
                            "RestApiId": { "Ref": "Api" },
                            "ResourceId": { "Ref": "Items" },
                            "HttpMethod": VERB,
                            "AuthorizationType": "NONE",
                            "OperationName": SECOND_OPERATION,
                            "Integration": { "Type": "MOCK" },
                        },
                    },
                },
            })
            .to_string();
            Prepared {
                template,
                api_id: None,
                held_deployment: None,
                disable_validation: false,
            }
        }
        Collision::ModelHeldOutsideTheStack => {
            let api_id =
                api_outside_the_stack(router, row.case, &format!("{}-api", row.stack)).await;
            let (code, created) = apigateway_with_body(
                router,
                "POST",
                &format!("/restapis/{api_id}/models"),
                Some(&json!({
                    "name": MODEL_NAME,
                    "contentType": JSON,
                    "description": HELD_MODEL_DESCRIPTION,
                    "schema": "{\"type\":\"object\"}",
                })),
            )
            .await;
            assert_eq!(
                code, 201,
                "case {:?}: CreateModel must answer 201, got {code} with {created}",
                row.case
            );
            let template = json!({
                "Resources": {
                    "Model": {
                        "Type": MODEL_TYPE,
                        "Properties": {
                            "RestApiId": api_id,
                            "Name": MODEL_NAME,
                            "ContentType": JSON,
                            "Description": DECLARED_MODEL_DESCRIPTION,
                            "Schema": { "type": "object", "required": ["marker"] },
                        },
                    },
                },
            })
            .to_string();
            Prepared {
                template,
                api_id: Some(api_id),
                held_deployment: None,
                disable_validation: true,
            }
        }
    }
}

/// The identifier the row's failed resource must be reported under, read off what the
/// deployment really created: the api the resources live in, and — for the method row — the id
/// API Gateway assigned the resource the two declarations share.
async fn expected_identifier(router: &MultiServiceRouter, row: &Row, api_id: &str) -> String {
    match row.collision {
        Collision::StageAfterItsSiblingDeployment => identifier(&[
            (REST_API_ID, api_id),
            (STAGE_NAME, STAGE_MINTED_BY_THE_DEPLOYMENT),
        ]),
        Collision::StageHeldOutsideTheStack => {
            identifier(&[(REST_API_ID, api_id), (STAGE_NAME, STAGE_HELD_OUTSIDE)])
        }
        Collision::MethodAfterItsSibling => {
            let resource_id = physical_resource_id(router, row.case, row.stack, "Items").await;
            identifier(&[
                (REST_API_ID, api_id),
                (RESOURCE_ID, &resource_id),
                (HTTP_METHOD, VERB),
            ])
        }
        Collision::ModelHeldOutsideTheStack => {
            identifier(&[(REST_API_ID, api_id), (NAME, MODEL_NAME)])
        }
    }
}

/// Hold the resource that was ALREADY there to being exactly as it was: the create was refused,
/// so nothing of the declaration reached it.
async fn assert_the_held_resource_is_untouched(
    router: &MultiServiceRouter,
    row: &Row,
    prepared: &Prepared,
    api_id: &str,
) {
    match row.collision {
        Collision::StageAfterItsSiblingDeployment => {
            let (code, stage) = apigateway(
                router,
                "GET",
                &format!("/restapis/{api_id}/stages/{STAGE_MINTED_BY_THE_DEPLOYMENT}"),
            )
            .await;
            assert_eq!(
                code, 200,
                "case {:?}: expected the stage the deployment minted to still be there, got \
                 {code} with {stage}",
                row.case
            );
        }
        Collision::StageHeldOutsideTheStack => {
            let (code, stage) = apigateway(
                router,
                "GET",
                &format!("/restapis/{api_id}/stages/{STAGE_HELD_OUTSIDE}"),
            )
            .await;
            assert_eq!(
                code, 200,
                "case {:?}: expected the held stage to still be there, got {code} with {stage}",
                row.case
            );
            assert_eq!(
                stage.get("deploymentId").and_then(Value::as_str),
                prepared.held_deployment.as_deref(),
                "case {:?}: expected the held stage to still serve the deployment it was \
                 created for, got {stage}",
                row.case
            );
        }
        Collision::MethodAfterItsSibling => {
            let resource_id = physical_resource_id(router, row.case, row.stack, "Items").await;
            let (code, method) = apigateway(
                router,
                "GET",
                &format!("/restapis/{api_id}/resources/{resource_id}/methods/{VERB}"),
            )
            .await;
            assert_eq!(
                code, 200,
                "case {:?}: expected the method the first declaration put to still be there, \
                 got {code} with {method}",
                row.case
            );
            assert_eq!(
                method.get("operationName").and_then(Value::as_str),
                Some(FIRST_OPERATION),
                "case {:?}: expected the first declaration's method to be left as it is, got \
                 {method}",
                row.case
            );
        }
        Collision::ModelHeldOutsideTheStack => {
            let (code, model) = apigateway(
                router,
                "GET",
                &format!("/restapis/{api_id}/models/{MODEL_NAME}"),
            )
            .await;
            assert_eq!(
                code, 200,
                "case {:?}: expected the held model to still be there, got {code} with {model}",
                row.case
            );
            assert_eq!(
                model.get("description").and_then(Value::as_str),
                Some(HELD_MODEL_DESCRIPTION),
                "case {:?}: expected the held model to be left as it is, got {model}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn a_create_whose_identifier_is_already_held_reports_the_handlers_already_exists() {
    for row in ROWS {
        let router = router();
        let prepared = prepare(&router, row).await;
        // `DO_NOTHING` keeps the failed create as it stands, so the identifier the reason names
        // can be read off the resources the deployment really created.
        let mut extra = vec![member("OnFailure", "DO_NOTHING")];
        if prepared.disable_validation {
            extra.push(member("DisableValidation", "true"));
        }
        let (code, created) = create_stack(&router, row.stack, &prepared.template, &extra).await;
        assert_eq!(
            code, 200,
            "case {:?}: CreateStack must answer 200, got {code} {created}",
            row.case
        );

        let reason = failed_resource_reason(
            row.case,
            &router,
            REGION,
            row.stack,
            row.logical_id,
            CREATE_FAILED,
        )
        .await;
        let (quoted, token, class) = envelope_parts(row.case, &reason);
        let api_id = match &prepared.api_id {
            Some(api_id) => api_id.clone(),
            None => physical_resource_id(&router, row.case, row.stack, "Api").await,
        };
        let expected = already_exists_sentence(
            row.resource_type,
            &expected_identifier(&router, row, &api_id).await,
        );
        assert_eq!(
            quoted, expected,
            "case {:?}: expected the handler's own already-exists report {expected:?}, got \
             {quoted:?} in {reason:?}",
            row.case
        );
        assert_eq!(
            class, ALREADY_EXISTS,
            "case {:?}: expected the {ALREADY_EXISTS} class, got {class:?} in {reason:?}",
            row.case
        );
        assert!(
            is_a_request_id(&token),
            "case {:?}: expected the resource request's own token, got {token:?}",
            row.case
        );
        // The refusal is the HANDLER's: no call of the owning service answered it, so there is
        // no SDK diagnostic of one inside the quotes.
        assert!(
            !quoted.contains("(Service: "),
            "case {:?}: expected no SDK diagnostic in the handler's own report, got {quoted:?}",
            row.case
        );

        let settled = settled_stack_status(row.case, &router, REGION, row.stack).await;
        assert_eq!(
            settled, CREATE_FAILED,
            "case {:?}: expected the failed create to be kept as it stands, got {settled:?}",
            row.case
        );
        assert_the_held_resource_is_untouched(&router, row, &prepared, &api_id).await;
    }
}

/// The recorded trace, with the failure handled as CloudFormation handles it by default: the
/// stack rolls back, and the stage's `CREATE_FAILED` event carries the same report. The api the
/// identifier names is the stack's own and goes with the rollback, so what is asserted around it
/// is the sentence's fixed text and the stage name the declaration stated.
#[tokio::test]
async fn the_colliding_stage_reports_the_same_reason_when_the_stack_rolls_back() {
    let case = "a colliding stage on a stack that rolls back";
    let stack = "apigw-already-exists-stage-rolled-back";
    let router = router();
    let (code, created) =
        create_stack(&router, stack, &stage_after_deployment_template(stack), &[]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: CreateStack must answer 200, got {code} {created}"
    );

    let reason =
        failed_resource_reason(case, &router, REGION, stack, "Stage", CREATE_FAILED).await;
    let (quoted, token, class) = envelope_parts(case, &reason);
    let opening = format!("Resource of type '{STAGE_TYPE}' with identifier '{REST_API_ID}: ");
    let closing = format!(" {STAGE_NAME}: {STAGE_MINTED_BY_THE_DEPLOYMENT}' already exists.");
    assert!(
        quoted.starts_with(&opening) && quoted.ends_with(&closing),
        "case {case:?}: expected the already-exists sentence naming the type and the colliding \
         identifier ({opening:?} … {closing:?}), got {quoted:?}"
    );
    assert_eq!(
        class, ALREADY_EXISTS,
        "case {case:?}: expected the {ALREADY_EXISTS} class, got {class:?} in {reason:?}"
    );
    assert!(
        is_a_request_id(&token),
        "case {case:?}: expected the resource request's own token, got {token:?}"
    );
    let settled = settled_stack_status(case, &router, REGION, stack).await;
    assert_eq!(
        settled, ROLLBACK_COMPLETE,
        "case {case:?}: expected the failed create to roll back, got {settled:?}"
    );
}
