// SPDX-License-Identifier: CC0-1.0

//! The STAGES a SAM-expanded REST API is deployed with, read back through Amazon API Gateway
//! itself.
//!
//! RULE: the `AWS::ApiGateway::Deployment` the `AWS::Serverless-2016-10-31` transform
//! generates for a REST API declares a `StageName` of its own — "AWS SAM creates a stage
//! called `Stage` by default" — and `CreateDeployment$stageName` is "the name of the Stage
//! resource for the Deployment resource to create", so the api the stack deploys carries that
//! stage BESIDE the one its generated `AWS::ApiGateway::Stage` serves, under the api id API
//! Gateway minted; stating `OpenApiVersion` on the declaration (or in the `Globals` section
//! that stands in for it) is what suppresses the extra stage, since "setting this property to
//! any valid value will prevent the creation of the stage `Stage`".
//!
//! The rows differ along what the rule generalises over: whether the API is the IMPLICIT one
//! the transform synthesizes for `Api` events that name no `RestApiId` or one an
//! `AWS::Serverless::Api` declaration spells, which stage name the declaration states (the
//! `Prod` the macro names for the implicit API, or a name of its own), how many functions are
//! routed through the one API, and whether the extra stage is suppressed — stated on the
//! declaration or in `Globals`, as either OpenAPI version the property admits, against a
//! property written as `null`, which states no version at all.
//!
//! The recorded trace this suite was written for is ONE row: a function with an implicit `Api`
//! event, whose deployed API a client then drives through the API Gateway SDK.
//!
//! Everything goes over the wire each service speaks through the one router `harness.rs`
//! builds — the CloudFormation calls over its awsQuery wire, the api and stage reads over API
//! Gateway's own `restJson1` wire — so what is asserted is what a client that deploys the
//! template and then calls API Gateway observes.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::apigateway_vpc_link_resources::{apigateway, is_api_gateway_id, member_text};
use crate::change_set_lifecycle::settle_the_store;
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, kv, member, router_with_engine, xml_text,
};
use crate::stack_resources::wire_resources;
use crate::stack_resource_provisioning::stage_code_objects;

/// How many characters a REST API id carries, and which they may be: API Gateway assigns an
/// api ten lowercase alphanumeric characters, the width of every identifier of the account
/// that service mints.
const API_ID_CHARACTERS: usize = 10;

/// The resource types the transform's API trio is made of.
const REST_API_TYPE: &str = "AWS::ApiGateway::RestApi";
const DEPLOYMENT_TYPE: &str = "AWS::ApiGateway::Deployment";
const STAGE_TYPE: &str = "AWS::ApiGateway::Stage";

/// The stage the generated deployment declares, which is the one no stack resource owns.
const EXTRA_STAGE: &str = "Stage";

/// How many reads a row gives a stack to reach its terminal status in: the stack is owed that
/// status once the services that own its resources report them ready, and only a read asks
/// them about it, so a client polls — which is what this is.
const POLL_ATTEMPTS: usize = 20;

/// One `AWS::Serverless::Function` whose code lives in Amazon S3, with the events it is
/// triggered by.
fn function(logical_id: &str, events: Value) -> Value {
    json!({
        "Type": "AWS::Serverless::Function",
        "Properties": {
            "Handler": "index.handler",
            "Runtime": "python3.12",
            "CodeUri": format!("s3://code-bucket/{logical_id}.zip"),
            "Events": events
        }
    })
}

/// One `Api` event. `api` names the `AWS::Serverless::Api` it is routed through; an event that
/// names none is routed through the IMPLICIT api the transform synthesizes.
fn api_event(path: &str, method: &str, api: Option<&str>) -> Value {
    let mut properties = json!({ "Path": path, "Method": method });
    if let Some(api) = api {
        properties["RestApiId"] = json!({ "Ref": api });
    }
    json!({ "Type": "Api", "Properties": properties })
}

/// One `AWS::Serverless::Api` declaration: the stage it names, and the OpenAPI version it
/// states where the row states one.
fn serverless_api(stage_name: &str, open_api_version: Option<Value>) -> Value {
    let mut properties = json!({ "StageName": stage_name });
    if let Some(version) = open_api_version {
        properties["OpenApiVersion"] = version;
    }
    json!({ "Type": "AWS::Serverless::Api", "Properties": properties })
}

/// A SAM template body: the transform, the resources, and the `Globals` section where the row
/// states one.
fn template(resources: Value, globals: Option<Value>) -> String {
    let mut document = json!({
        "Transform": "AWS::Serverless-2016-10-31",
        "Resources": resources
    });
    if let Some(globals) = globals {
        document["Globals"] = globals;
    }
    document.to_string()
}

/// One row of the table.
struct Row {
    case: &'static str,
    stack: &'static str,
    template: String,
    /// The logical id the expanded api carries, which is what a client reads its id out of.
    api: &'static str,
    /// Every stage name the DEPLOYED api must carry, sorted.
    stages: Vec<&'static str>,
    /// The stage name the stack's own `AWS::ApiGateway::Stage` resource is created under.
    declared_stage: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the implicit api of one function's Api event: the Prod stage the macro \
                   names and the Stage one its deployment declares",
            stack: "sam-stages-implicit",
            template: template(
                json!({ "Hello": function("Hello", json!({ "Get": api_event("/hello", "get", None) })) }),
                None,
            ),
            api: "ServerlessRestApi",
            stages: vec!["Prod", "Stage"],
            declared_stage: "Prod",
        },
        Row {
            case: "two functions routed through the one implicit api: one deployment, so the \
                   same two stages",
            stack: "sam-stages-implicit-two",
            template: template(
                json!({
                    "Hello": function("Hello", json!({ "Get": api_event("/hello", "get", None) })),
                    "Bye": function("Bye", json!({ "Post": api_event("/bye/{id}", "post", None) }))
                }),
                None,
            ),
            api: "ServerlessRestApi",
            stages: vec!["Prod", "Stage"],
            declared_stage: "Prod",
        },
        Row {
            case: "a declared AWS::Serverless::Api naming a stage of its own",
            stack: "sam-stages-declared",
            template: template(
                json!({
                    "Web": serverless_api("live", None),
                    "Place": function("Place", json!({ "Post": api_event("/orders", "post", Some("Web")) }))
                }),
                None,
            ),
            api: "Web",
            stages: vec!["Stage", "live"],
            declared_stage: "live",
        },
        Row {
            case: "a declared api stating OpenApiVersion 3.0.1: the extra stage is suppressed",
            stack: "sam-stages-openapi-three",
            template: template(
                json!({
                    "Web": serverless_api("live", Some(json!("3.0.1"))),
                    "Place": function("Place", json!({ "Post": api_event("/orders", "post", Some("Web")) }))
                }),
                None,
            ),
            api: "Web",
            stages: vec!["live"],
            declared_stage: "live",
        },
        Row {
            case: "the implicit api under a Globals section stating OpenApiVersion 2.0: the \
                   extra stage is suppressed there too",
            stack: "sam-stages-globals-swagger",
            template: template(
                json!({ "Hello": function("Hello", json!({ "Get": api_event("/hello", "get", None) })) }),
                Some(json!({ "Api": { "OpenApiVersion": "2.0" } })),
            ),
            api: "ServerlessRestApi",
            stages: vec!["Prod"],
            declared_stage: "Prod",
        },
        Row {
            case: "a declared api whose OpenApiVersion is written as null: no version is \
                   stated, so the extra stage stands",
            stack: "sam-stages-openapi-null",
            template: template(
                json!({
                    "Web": serverless_api("live", Some(Value::Null)),
                    "Place": function("Place", json!({ "Post": api_event("/orders", "post", Some("Web")) }))
                }),
                None,
            ),
            api: "Web",
            stages: vec!["Stage", "live"],
            declared_stage: "live",
        },
    ]
}

/// Whether `value` is an identifier of the form API Gateway mints for a REST API: exactly
/// [`API_ID_CHARACTERS`] of them.
fn is_rest_api_id(value: &str) -> bool {
    is_api_gateway_id(value, API_ID_CHARACTERS)
}

/// Deploy one row's template and poll the stack to a terminal status, answering that read's
/// `DescribeStacks` body.
///
/// Each poll pulls the transitions the store owes into the past and lets an ordinary read
/// materialise them, so the row waits for the stack's CONDITION rather than for the clock.
async fn deploy(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    row: &Row,
) -> String {
    stage_code_objects(router, row.case, &row.template).await;
    let (status, body) = create_stack(
        router,
        row.stack,
        &row.template,
        &kv(&[
            ("Capabilities.member.1", "CAPABILITY_IAM"),
            ("Capabilities.member.2", "CAPABILITY_AUTO_EXPAND"),
        ]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected CreateStack to be accepted, got {status} {body}",
        row.case
    );
    let mut described = String::new();
    for _ in 0..POLL_ATTEMPTS {
        settle_the_store(router, engine).await;
        let (status, body) = describe_stack(router, row.stack).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected DescribeStacks to answer, got {status} {body}",
            row.case
        );
        described = body;
        if !xml_text(&described, "StackStatus")
            .unwrap_or_default()
            .ends_with("_IN_PROGRESS")
        {
            break;
        }
    }
    described
}

/// The `(logical id, type, physical id)` triples `DescribeStackResources` reports for a
/// deployed stack.
async fn deployed_resources(
    router: &MultiServiceRouter,
    row: &Row,
) -> Vec<(String, String, String)> {
    let (status, body) = call(
        router,
        "DescribeStackResources",
        &[member("StackName", row.stack)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected DescribeStackResources to answer, got {status} {body}",
        row.case
    );
    wire_resources(&body)
        .into_iter()
        .map(|resource| {
            (
                resource.logical_id.unwrap_or_default(),
                resource.resource_type.unwrap_or_default(),
                resource.physical_id.unwrap_or_default(),
            )
        })
        .collect()
}

/// The physical id of the one resource of `resource_type` the stack holds.
fn only_resource_of(
    case: &str,
    resources: &[(String, String, String)],
    resource_type: &str,
) -> String {
    let held: Vec<&(String, String, String)> = resources
        .iter()
        .filter(|(_, held_type, _)| held_type == resource_type)
        .collect();
    assert_eq!(
        held.len(),
        1,
        "case {case:?}: expected exactly one {resource_type} in the stack, got {held:?}"
    );
    held[0].2.clone()
}

/// `GetStages` (`GET /restapis/{restApiId}/stages`): the stages the api holds, as the
/// collection's own `item` member carries them.
async fn deployed_stages(router: &MultiServiceRouter, case: &str, api_id: &str) -> Vec<Value> {
    let (status, document) =
        apigateway(router, "GET", &format!("/restapis/{api_id}/stages")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetStages({api_id}) to answer 200, got {status} with {document}"
    );
    document
        .get("item")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Every row: the api the transform expanded exists in API Gateway under the id the stack
/// publishes, and `GetStages` on that id answers exactly the stages the rule says the
/// expansion deployed — each of them serving the deployment the stack published.
#[tokio::test]
async fn a_sam_expanded_rest_api_is_deployed_with_the_stages_the_transform_declares() {
    let (router, engine) = router_with_engine();
    for row in rows() {
        let case = row.case;
        let described = deploy(&router, &engine, &row).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the create to complete, got {described}"
        );

        let resources = deployed_resources(&router, &row).await;
        let api_id = resources
            .iter()
            .find(|(logical_id, held_type, _)| logical_id == row.api && held_type == REST_API_TYPE)
            .map(|(_, _, physical_id)| physical_id.clone())
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a {REST_API_TYPE} named {:?}, got {resources:?}", row.api)
            });
        assert!(
            is_rest_api_id(&api_id),
            "case {case:?}: expected {:?} to report an api id of the form API Gateway mints, \
             got {api_id:?}",
            row.api
        );

        // The id the stack publishes is one API Gateway itself answers for, which is what
        // makes every later leg (the stages below, the invoke URL) addressable at all.
        let (status, read) = apigateway(&router, "GET", &format!("/restapis/{api_id}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetRestApi({api_id}) to answer 200, got {status} with {read}"
        );
        assert_eq!(
            member_text(&read, "id").as_deref(),
            Some(api_id.as_str()),
            "case {case:?}: expected the api to report the published id, got {read}"
        );

        let stages = deployed_stages(&router, case, &api_id).await;
        let mut names: Vec<String> = stages
            .iter()
            .filter_map(|stage| member_text(stage, "stageName"))
            .collect();
        names.sort();
        let expected: Vec<String> = row.stages.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            names, expected,
            "case {case:?}: expected the deployed api to carry the stages {expected:?}, got \
             {names:?} in {stages:?}"
        );

        // Every stage the expansion deployed serves the deployment the stack published: the
        // one its stage resource points at, and the one the deployment's own `StageName`
        // created.
        let deployment_id = only_resource_of(case, &resources, DEPLOYMENT_TYPE);
        for stage in &stages {
            assert_eq!(
                member_text(stage, "deploymentId").as_deref(),
                Some(deployment_id.as_str()),
                "case {case:?}: expected every stage to serve the published deployment \
                 {deployment_id:?}, got {stage:?}"
            );
        }

        // The stack owns exactly ONE stage — the one its declaration names — so the extra
        // stage is the deployment's and no resource of the stack's own.
        let declared = only_resource_of(case, &resources, STAGE_TYPE);
        assert_eq!(
            declared, row.declared_stage,
            "case {case:?}: expected the stack's stage resource to be {:?}, got {declared:?}",
            row.declared_stage
        );
        let owned_extra: Vec<&(String, String, String)> = resources
            .iter()
            .filter(|(_, held_type, physical_id)| {
                held_type == STAGE_TYPE && physical_id == EXTRA_STAGE
            })
            .collect();
        assert!(
            owned_extra.is_empty(),
            "case {case:?}: expected the {EXTRA_STAGE} stage to be the deployment's rather \
             than a resource of the stack, got {owned_extra:?}"
        );
    }
}
