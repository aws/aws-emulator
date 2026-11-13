// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack provisions the `AWS::ApiGateway::BasePathMapping` resources its
//! template declares, in the Amazon API Gateway REST control plane that owns them.
//!
//! The rule under test: **a stack reports a resource created only once the service that owns
//! its type really created it, through that service's own create API — so a client that
//! deployed a template declaring a custom domain name and a base-path mapping of it reads
//! that mapping back with `apigateway GetBasePathMapping`, addressed by the domain name and
//! the base path the template stated, reporting the REST API and the stage the template
//! named; and the mapping stops existing when the stack is deleted.** Two crates have to
//! agree for that to hold (CloudFormation makes the calls, API Gateway serves them), so the
//! suite lives here rather than in either of them.
//!
//! The rows differ along what the rule generalises over: the base path declared (a plain
//! one, a hyphenated one, none at all and one declared BLANK — the two ways of asking for
//! the mapping a caller reaches by naming the domain and nothing after it, which API Gateway
//! addresses as `(none)`), how the mapping names its domain (a `Ref` to the stack's own
//! domain resource and the literal name), the stage name, and the language the template is
//! written in (JSON, and YAML with the short-form intrinsic tags). Every expected value is
//! computed here from what the row declares or read from the owning service, never from the
//! answer being checked.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery call, the stack readers and the output readers this test crate writes once
// (`tests/all/cloudformation_apigatewayv2_resources.rs`, with
// `tests/all/cloudformation_provisions_stack_resources.rs` for the XML readers).
use crate::cloudformation_apigatewayv2_resources::{
    cloudformation, create_stack, describe_stack, every_output_has_a_value, member, REGION,
};
use crate::cloudformation_provisions_stack_resources::xml_text;
use crate::rds_docdb_neptune_signing_name_routing::authorization;

/// How often the suite asks the stack whether it has completed, and how long it goes on
/// asking. The FIRST observation that reports the stack complete is the one every assertion
/// is made about. The budget is long enough that a loaded machine is slower than the spans
/// these services choose without the rule being any different.
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const POLLS: usize = 400;

/// The `StackStatus` values the rule is stated in.
const CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The base path API Gateway holds the mapping reached by naming the domain and nothing
/// after it under: "specify '(none)' if you do not want callers to specify any base path
/// name after the domain name" (`GetBasePathMapping$basePath`).
const NONE_BASE_PATH: &str = "(none)";

/// What API Gateway answers for a record it does not hold: the modelled 404 of every read
/// here.
const NOT_FOUND: &str = "NotFoundException";

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(engine),
    );
    router
}

/// Call one Amazon API Gateway operation over its restJson1 wire: this service signs as
/// `apigateway` and binds its custom-domain operations under `/domainnames`.
///
/// Answers the HTTP status, the `x-amzn-errortype` a refusal carries (the wire error code a
/// client reads) and the answered document.
async fn apigateway(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Option<String>, Value) {
    let payload = match &body {
        Some(document) => Bytes::from(document.to_string()),
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("apigateway.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("apigateway"))
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(['/', ':'])
                .next()
                .unwrap_or(value)
                .trim()
                .to_string()
        });
    let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, error_type, answered)
}

/// The certificate the row's custom domain presents. A literal ARN: what is under test is
/// the mapping, and API Gateway takes the certificate ARN as configuration it reports back.
fn certificate_arn() -> String {
    format!("arn:aws:acm:{REGION}:000000000000:certificate/8f4c1b1e-2a1d-4a57-9c0e-1f7b2a3d4e5f")
}

/// The domain name a row declares: one DNS name per stack, so two rows never collide.
fn domain_of(stack: &str) -> String {
    format!("{stack}.example.com")
}

/// What one row declares and what the stack must then have created.
struct Row {
    case: &'static str,
    /// The template body, built from the stack name the row runs under (which is what makes
    /// the declared domain name unique per row).
    template: fn(&str) -> String,
    /// The `BasePath` the row's mapping declares, or `None` where it declares none — which
    /// a row may also declare BLANK, the other way of asking for the mapping API Gateway
    /// holds under `(none)`.
    base_path: Option<&'static str>,
    /// The stage the row's mapping names, which is the stage its own template deploys.
    stage: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        case: "a mapping of a plain base path onto a Ref'd domain and stage",
        template: |stack| template(stack, Some("v1"), "prod", DomainReference::Ref),
        base_path: Some("v1"),
        stage: "prod",
    },
    Row {
        case: "a mapping that declares no base path, onto the domain named literally",
        template: |stack| template(stack, None, "live", DomainReference::Literal),
        base_path: None,
        stage: "live",
    },
    Row {
        // "Leave this blank if you do not want callers to specify a base path name after the
        // domain name": the same mapping as the row above, asked for the other way.
        case: "a mapping whose base path is declared blank",
        template: |stack| template(stack, Some(""), "prod", DomainReference::Ref),
        base_path: None,
        stage: "prod",
    },
    Row {
        case: "a mapping of a hyphenated base path, in YAML with the short-form tags",
        template: |stack| yaml_template(stack, "orders-v2", "staging"),
        base_path: Some("orders-v2"),
        stage: "staging",
    },
];

/// How a mapping's declaration names the domain it belongs to: the two spellings a template
/// addresses a resource of its own by.
enum DomainReference {
    /// `{ "Ref": "CustomDomain" }` — the domain resource of this very stack.
    Ref,
    /// The domain name itself, as the declaration of the domain states it.
    Literal,
}

/// A JSON template declaring a REST API with one mocked method, a deployment, a stage, a
/// custom domain name and the base-path mapping under test.
fn template(stack: &str, base_path: Option<&str>, stage: &str, domain: DomainReference) -> String {
    let declared_domain = domain_of(stack);
    let named_domain = match domain {
        DomainReference::Ref => json!({ "Ref": "CustomDomain" }),
        DomainReference::Literal => json!(declared_domain),
    };
    let mut mapping = json!({
        "DomainName": named_domain,
        "RestApiId": { "Ref": "RestApi" },
        "Stage": { "Ref": "ApiStage" }
    });
    if let Some(base_path) = base_path {
        mapping["BasePath"] = json!(base_path);
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "RestApi": {
                "Type": "AWS::ApiGateway::RestApi",
                "Properties": { "Name": format!("{stack}-api") }
            },
            "Greeting": {
                "Type": "AWS::ApiGateway::Resource",
                "Properties": {
                    "RestApiId": { "Ref": "RestApi" },
                    "ParentId": { "Fn::GetAtt": ["RestApi", "RootResourceId"] },
                    "PathPart": "hello"
                }
            },
            "Method": {
                "Type": "AWS::ApiGateway::Method",
                "Properties": {
                    "RestApiId": { "Ref": "RestApi" },
                    "ResourceId": { "Ref": "Greeting" },
                    "HttpMethod": "GET",
                    "AuthorizationType": "NONE",
                    "Integration": { "Type": "MOCK" }
                }
            },
            "Deployment": {
                "Type": "AWS::ApiGateway::Deployment",
                "DependsOn": ["Method"],
                "Properties": { "RestApiId": { "Ref": "RestApi" } }
            },
            "ApiStage": {
                "Type": "AWS::ApiGateway::Stage",
                "Properties": {
                    "RestApiId": { "Ref": "RestApi" },
                    "DeploymentId": { "Ref": "Deployment" },
                    "StageName": stage
                }
            },
            "CustomDomain": {
                "Type": "AWS::ApiGateway::DomainName",
                "Properties": {
                    "DomainName": declared_domain,
                    "RegionalCertificateArn": certificate_arn(),
                    "EndpointConfiguration": { "Types": ["REGIONAL"] }
                }
            },
            "Mapping": {
                "Type": "AWS::ApiGateway::BasePathMapping",
                "Properties": mapping
            }
        },
        "Outputs": {
            "RestApiId": { "Value": { "Ref": "RestApi" } },
            "StageName": { "Value": { "Ref": "ApiStage" } },
            "DomainName": { "Value": { "Ref": "CustomDomain" } }
        }
    })
    .to_string()
}

/// The same estate as [`template`], written in YAML with the short-form intrinsic tags.
fn yaml_template(stack: &str, base_path: &str, stage: &str) -> String {
    format!(
        "\
AWSTemplateFormatVersion: '2010-09-09'
Description: a custom domain whose base path maps onto this stack's own stage
Resources:
  RestApi:
    Type: AWS::ApiGateway::RestApi
    Properties:
      Name: {stack}-api
  Greeting:
    Type: AWS::ApiGateway::Resource
    Properties:
      RestApiId: !Ref RestApi
      ParentId: !GetAtt RestApi.RootResourceId
      PathPart: hello
  Method:
    Type: AWS::ApiGateway::Method
    Properties:
      RestApiId: !Ref RestApi
      ResourceId: !Ref Greeting
      HttpMethod: GET
      AuthorizationType: NONE
      Integration:
        Type: MOCK
  Deployment:
    Type: AWS::ApiGateway::Deployment
    DependsOn:
      - Method
    Properties:
      RestApiId: !Ref RestApi
  ApiStage:
    Type: AWS::ApiGateway::Stage
    Properties:
      RestApiId: !Ref RestApi
      DeploymentId: !Ref Deployment
      StageName: {stage}
  CustomDomain:
    Type: AWS::ApiGateway::DomainName
    Properties:
      DomainName: {domain}
      RegionalCertificateArn: {certificate}
      EndpointConfiguration:
        Types:
          - REGIONAL
  Mapping:
    Type: AWS::ApiGateway::BasePathMapping
    Properties:
      DomainName: !Ref CustomDomain
      RestApiId: !Ref RestApi
      Stage: !Ref ApiStage
      BasePath: {base_path}
Outputs:
  RestApiId:
    Value: !Ref RestApi
  StageName:
    Value: !Ref ApiStage
  DomainName:
    Value: !Ref CustomDomain
",
        domain = domain_of(stack),
        certificate = certificate_arn(),
    )
}

/// The stack's body at the FIRST observation that reports it complete, which is what every
/// assertion about a provisioned resource is made against.
async fn awaited_complete(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let mut last = String::new();
    for _ in 0..POLLS {
        let body = describe_stack(router, case, stack).await;
        match xml_text(&body, "StackStatus").as_deref() {
            Some(CREATE_COMPLETE) => return body,
            Some(CREATE_IN_PROGRESS) => {}
            other => panic!(
                "case {case:?}: expected the stack to be creating or complete, got {other:?} \
                 in {body}"
            ),
        }
        last = body;
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: the stack never completed; the last answer was {last}")
}

/// The base-path mapping API Gateway holds, read the way a client reads it: by the domain
/// name and the base path.
async fn get_base_path_mapping(
    router: &MultiServiceRouter,
    domain_name: &str,
    base_path: &str,
) -> (u16, Option<String>, Value) {
    apigateway(
        router,
        "GET",
        &format!("/domainnames/{domain_name}/basepathmappings/{base_path}"),
        None,
    )
    .await
}

#[tokio::test]
async fn a_stack_creates_the_base_path_mappings_it_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let stack_name = format!("apigw-mapping-{index}");
        let domain = domain_of(&stack_name);
        // The base path the mapping is ADDRESSED by: the one the row declares, and the
        // `(none)` spelling for the mapping that declares none.
        let addressed = row.base_path.unwrap_or(NONE_BASE_PATH);
        create_stack(&router, case, &stack_name, &(row.template)(&stack_name)).await;

        let body = awaited_complete(&router, case, &stack_name).await;
        let outputs = every_output_has_a_value(case, &body);
        let api_id = outputs
            .get("RestApiId")
            .unwrap_or_else(|| panic!("case {case:?}: no RestApiId output in {outputs:?}"))
            .clone();
        assert_eq!(
            outputs.get("DomainName"),
            Some(&domain),
            "case {case:?}: expected the Ref of the domain to be the declared name, got \
             {outputs:?}"
        );

        let (status, error_type, mapping) =
            get_base_path_mapping(&router, &domain, addressed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetBasePathMapping for {addressed:?} of \
             {domain:?}, got {status} {error_type:?} {mapping}"
        );
        assert_eq!(
            member(case, &mapping, "basePath"),
            addressed,
            "case {case:?}: expected the base path the mapping was created under, got {mapping}"
        );
        // The api and the stage the mapping reports are the stack's own: the identifier API
        // Gateway minted for the template's `AWS::ApiGateway::RestApi`, and the stage its
        // `AWS::ApiGateway::Stage` deployed.
        assert_eq!(
            member(case, &mapping, "restApiId"),
            api_id,
            "case {case:?}: expected the mapping to name the stack's own api {api_id:?}, got \
             {mapping}"
        );
        assert_eq!(
            member(case, &mapping, "stage"),
            row.stage,
            "case {case:?}: expected the mapping to name the stage the stack deployed, got \
             {mapping}"
        );

        // The same mapping is one of the domain's own collection, which is the read a client
        // enumerating a domain's base paths makes.
        let (status, _, listed) = apigateway(
            &router,
            "GET",
            &format!("/domainnames/{domain}/basepathmappings"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetBasePathMappings, got {status} {listed}"
        );
        let paths: Vec<&str> = listed["item"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("basePath").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            paths,
            vec![addressed],
            "case {case:?}: expected the domain to hold exactly the declared mapping, got \
             {listed}"
        );
    }
}

/// Deleting the stack deletes what it created: the mapping stops resolving in API Gateway,
/// which is how a client can tell the stack really owned it.
#[tokio::test]
async fn deleting_the_stack_deletes_the_mapping_it_created() {
    let router = build_router();
    let case = "a deleted api gateway base-path-mapping stack";
    let stack_name = "apigw-mapping-deleted";
    let domain = domain_of(stack_name);
    let template = template(stack_name, Some("v1"), "prod", DomainReference::Ref);
    create_stack(&router, case, stack_name, &template).await;
    awaited_complete(&router, case, stack_name).await;

    let (status, _, mapping) = get_base_path_mapping(&router, &domain, "v1").await;
    assert_eq!(
        status, 200,
        "case {case:?}: the stack's mapping must resolve before the stack is deleted, got \
         {status} {mapping}"
    );

    let (status, deleted) =
        cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {deleted}"
    );
    // The stack's deletion is the clock's to make due, so the read a client makes next is
    // asked until it answers for a mapping nothing holds.
    let mut answer = get_base_path_mapping(&router, &domain, "v1").await;
    for _ in 0..POLLS {
        if answer.0 != 200 {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
        answer = get_base_path_mapping(&router, &domain, "v1").await;
    }
    let (status, error_type, gone) = answer;
    assert_eq!(
        status, 404,
        "case {case:?}: expected GetBasePathMapping to report the deleted mapping gone, got \
         {status} {gone}"
    );
    assert_eq!(
        error_type.as_deref(),
        Some(NOT_FOUND),
        "case {case:?}: expected the modelled not-found code, got {error_type:?} {gone}"
    );
}
