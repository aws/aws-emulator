// SPDX-License-Identifier: CC0-1.0

//! What the `AWS::Serverless-2016-10-31` (AWS SAM) transform expands a template into, end
//! to end over the awsQuery wire.
//!
//! The rule every case here holds the service to: **a template whose `Transform` section
//! names `AWS::Serverless-2016-10-31` is PROCESSED by that macro before anything reads its
//! `Resources`, so what a change set plans and what a stack deploys are the concrete
//! resources the macro synthesizes — an `AWS::Lambda::Function` with its execution role and
//! invoke permissions, the `AWS::ApiGateway::RestApi`/`Deployment`/`Stage` trio of each API
//! (exactly one API, and one deployment whose logical id carries a hash of that API's
//! definition, however many events route through it), a table, a layer version, a state
//! machine — and never an `AWS::Serverless::*` type, which exists only as the macro's
//! input.**
//!
//! The recorded trace this suite was written for is ONE row of the table below: the rows
//! span the serverless resource types the macro expands, the implicit and the explicit API,
//! one and several routes through the same implicit API, a declared and a synthesized
//! execution role, the JSON and the YAML spelling of the same template, and the two cases
//! the expansion deliberately leaves alone — a declaration whose template lives in the
//! Serverless Application Repository, and a serverless type in a template that declares no
//! transform at all. Every read goes through the real request path (form-encoded body in,
//! XML envelope out), so what is asserted is what a client sees.

use std::sync::Arc;

use service_com_amazonaws_cloudformation_cloudformation::state::change_action;
use smithy_server_runtime::MultiServiceRouter;

use crate::change_set_lifecycle::settle_the_store;
use crate::stack_lifecycle::{
    call, create_stack, get_template, member, router_with_engine, xml_blocks, xml_text, xml_texts,
};

/// How many hex characters a content-hash-suffixed logical id carries — the width AWS SAM
/// gives the hash it appends to a deployment's and a layer version's logical id.
const DIGEST_CHARS: usize = 10;

/// One resource a change set must plan: its logical id, either exactly or as a
/// content-hash-suffixed prefix, and its type.
#[derive(Debug, Clone, Copy)]
enum Planned {
    /// A logical id the expansion states outright.
    Exact(&'static str),
    /// A logical id that is a prefix plus a content hash.
    Hashed(&'static str),
}

impl Planned {
    /// Whether `actual` is the logical id this expects, and what to call it in a failure.
    fn matches(self, actual: &str) -> bool {
        match self {
            Planned::Exact(id) => actual == id,
            Planned::Hashed(prefix) => match actual.strip_prefix(prefix) {
                Some(hash) => {
                    hash.len() == DIGEST_CHARS
                        && hash
                            .chars()
                            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
                }
                None => false,
            },
        }
    }

    fn described(self) -> String {
        match self {
            Planned::Exact(id) => id.to_string(),
            Planned::Hashed(prefix) => format!("{prefix}<{DIGEST_CHARS} hex characters>"),
        }
    }
}

/// One row of the expansion table.
struct Case {
    case: &'static str,
    stack: &'static str,
    template: String,
    /// Every resource the change set must plan, and nothing else.
    planned: Vec<(Planned, &'static str)>,
}

/// `Transform` plus `Resources`, as a JSON template body.
///
/// `pub(crate)` because `sam_deployed_resources.rs` builds its rows with the same two
/// sections rather than copying this.
pub(crate) fn sam_template(resources: &str) -> String {
    format!(r#"{{"Transform":"AWS::Serverless-2016-10-31","Resources":{resources}}}"#)
}

/// The function declaration the implicit-API rows start from: a function with one `Api`
/// event and no `RestApiId`, which is what makes the API implicit.
fn implicit_api_function(logical_id: &str, event: &str, path: &str, method: &str) -> String {
    format!(
        r#""{logical_id}":{{"Type":"AWS::Serverless::Function","Properties":{{
             "Handler":"index.handler","Runtime":"python3.12",
             "CodeUri":"s3://code-bucket/{logical_id}.zip",
             "Events":{{"{event}":{{"Type":"Api","Properties":{{
                "Path":"{path}","Method":"{method}"}}}}}}}}}}"#
    )
}

/// The implicit API trio every implicit-API row plans.
fn implicit_api_trio() -> Vec<(Planned, &'static str)> {
    vec![
        (
            Planned::Exact("ServerlessRestApi"),
            "AWS::ApiGateway::RestApi",
        ),
        (
            Planned::Hashed("ServerlessRestApiDeployment"),
            "AWS::ApiGateway::Deployment",
        ),
        (
            Planned::Exact("ServerlessRestApiProdStage"),
            "AWS::ApiGateway::Stage",
        ),
    ]
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "one function with an implicit Api event",
            stack: "sam-implicit",
            template: sam_template(&format!(
                "{{{}}}",
                implicit_api_function("Hello", "Api", "/hello", "get")
            )),
            planned: [
                vec![
                    (Planned::Exact("Hello"), "AWS::Lambda::Function"),
                    (Planned::Exact("HelloRole"), "AWS::IAM::Role"),
                    (
                        Planned::Exact("HelloApiPermissionProd"),
                        "AWS::Lambda::Permission",
                    ),
                ],
                implicit_api_trio(),
            ]
            .concat(),
        },
        Case {
            case: "two functions routed through the same implicit Api",
            stack: "sam-implicit-two",
            template: sam_template(&format!(
                "{{{},{}}}",
                implicit_api_function("Hello", "Get", "/hello", "get"),
                implicit_api_function("Bye", "Post", "/bye/{id}", "post")
            )),
            planned: [
                vec![
                    (Planned::Exact("Bye"), "AWS::Lambda::Function"),
                    (Planned::Exact("ByeRole"), "AWS::IAM::Role"),
                    (
                        Planned::Exact("ByePostPermissionProd"),
                        "AWS::Lambda::Permission",
                    ),
                    (Planned::Exact("Hello"), "AWS::Lambda::Function"),
                    (Planned::Exact("HelloRole"), "AWS::IAM::Role"),
                    (
                        Planned::Exact("HelloGetPermissionProd"),
                        "AWS::Lambda::Permission",
                    ),
                ],
                implicit_api_trio(),
            ]
            .concat(),
        },
        Case {
            case: "an explicit Api, a declared execution role and an ANY method",
            stack: "sam-explicit",
            template: sam_template(
                r#"{"Web":{"Type":"AWS::Serverless::Api","Properties":{
                     "StageName":"dev",
                     "DefinitionBody":{"swagger":"2.0","paths":{}}}},
                   "Hello":{"Type":"AWS::Serverless::Function","Properties":{
                     "Handler":"index.handler","Runtime":"nodejs20.x",
                     "CodeUri":{"Bucket":"code-bucket","Key":"hello.zip"},
                     "Role":"arn:aws:iam::000000000000:role/given",
                     "Events":{"Get":{"Type":"Api","Properties":{
                        "Path":"/x","Method":"any","RestApiId":{"Ref":"Web"}}}}}}}"#,
            ),
            planned: vec![
                (Planned::Exact("Hello"), "AWS::Lambda::Function"),
                (
                    Planned::Exact("HelloGetPermissiondev"),
                    "AWS::Lambda::Permission",
                ),
                (Planned::Exact("Web"), "AWS::ApiGateway::RestApi"),
                (
                    Planned::Hashed("WebDeployment"),
                    "AWS::ApiGateway::Deployment",
                ),
                (Planned::Exact("WebdevStage"), "AWS::ApiGateway::Stage"),
            ],
        },
        Case {
            case: "a simple table and a layer version",
            stack: "sam-table-layer",
            template: sam_template(
                r#"{"Items":{"Type":"AWS::Serverless::SimpleTable","Properties":{
                     "PrimaryKey":{"Name":"sku","Type":"Number"}}},
                   "Shared":{"Type":"AWS::Serverless::LayerVersion","Properties":{
                     "LayerName":"shared","ContentUri":"s3://code-bucket/layer.zip",
                     "CompatibleRuntimes":["python3.12"]}}}"#,
            ),
            planned: vec![
                (Planned::Exact("Items"), "AWS::DynamoDB::Table"),
                (Planned::Hashed("Shared"), "AWS::Lambda::LayerVersion"),
            ],
        },
        Case {
            case: "a state machine with an inline definition",
            stack: "sam-state-machine",
            template: sam_template(
                r#"{"Flow":{"Type":"AWS::Serverless::StateMachine","Properties":{
                     "Definition":{"StartAt":"Done","States":{"Done":{"Type":"Succeed"}}},
                     "Type":"STANDARD"}}}"#,
            ),
            planned: vec![
                (
                    Planned::Exact("Flow"),
                    "AWS::StepFunctions::StateMachine",
                ),
                (Planned::Exact("FlowRole"), "AWS::IAM::Role"),
            ],
        },
        Case {
            case: "a function triggered by a queue",
            stack: "sam-queue-event",
            template: sam_template(
                r#"{"Pump":{"Type":"AWS::Serverless::Function","Properties":{
                     "Handler":"index.handler","Runtime":"python3.12",
                     "CodeUri":"s3://code-bucket/pump.zip",
                     "Events":{"Jobs":{"Type":"SQS","Properties":{
                        "Queue":"arn:aws:sqs:us-east-1:000000000000:jobs",
                        "BatchSize":10}}}}}}"#,
            ),
            planned: vec![
                (Planned::Exact("Pump"), "AWS::Lambda::Function"),
                (
                    Planned::Exact("PumpJobs"),
                    "AWS::Lambda::EventSourceMapping",
                ),
                (Planned::Exact("PumpRole"), "AWS::IAM::Role"),
            ],
        },
        Case {
            case: "the YAML spelling of the implicit-API template",
            stack: "sam-yaml",
            template: concat!(
                "Transform: AWS::Serverless-2016-10-31\n",
                "Resources:\n",
                "  Greet:\n",
                "    Type: AWS::Serverless::Function\n",
                "    Properties:\n",
                "      Handler: index.handler\n",
                "      Runtime: python3.12\n",
                "      CodeUri: s3://code-bucket/greet.zip\n",
                "      Events:\n",
                "        Web:\n",
                "          Type: Api\n",
                "          Properties:\n",
                "            Path: /greet\n",
                "            Method: get\n",
            )
            .to_string(),
            planned: [
                vec![
                    (Planned::Exact("Greet"), "AWS::Lambda::Function"),
                    (Planned::Exact("GreetRole"), "AWS::IAM::Role"),
                    (
                        Planned::Exact("GreetWebPermissionProd"),
                        "AWS::Lambda::Permission",
                    ),
                ],
                implicit_api_trio(),
            ]
            .concat(),
        },
        Case {
            case: "the transform beside resources it does not own",
            stack: "sam-passthrough",
            template: sam_template(r#"{"Jobs":{"Type":"AWS::SQS::Queue"}}"#),
            planned: vec![(Planned::Exact("Jobs"), "AWS::SQS::Queue")],
        },
        Case {
            // Declined, and asserted as declined: an application's template lives in the
            // AWS Serverless Application Repository, which this emulator makes no outbound
            // connection to, so the declaration is planned as it was written rather than as
            // resources nothing resolved.
            case: "an application, which the expansion leaves as declared",
            stack: "sam-application",
            template: sam_template(
                r#"{"Nested":{"Type":"AWS::Serverless::Application","Properties":{
                     "Location":{"ApplicationId":
                        "arn:aws:serverlessrepo:us-east-1:000000000000:applications/thing",
                       "SemanticVersion":"1.0.0"}}}}"#,
            ),
            planned: vec![(
                Planned::Exact("Nested"),
                "AWS::Serverless::Application",
            )],
        },
        Case {
            // The `Transform` declaration is what expands a serverless type: without it
            // the template declares the type itself, and nothing processes it.
            case: "a serverless type in a template that declares no transform",
            stack: "no-transform",
            template: format!(
                "{{\"Resources\":{{{}}}}}",
                implicit_api_function("Hello", "Api", "/hello", "get")
            ),
            planned: vec![(
                Planned::Exact("Hello"),
                "AWS::Serverless::Function",
            )],
        },
    ]
}

/// Create a `CREATE` change set for a stack that does not exist yet, settle the store and
/// answer what `DescribeChangeSet` reports.
async fn planned_change_set(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    stack: &str,
    template: &str,
) -> String {
    let (code, body) = call(
        router,
        "CreateChangeSet",
        &[
            member("StackName", stack),
            member("ChangeSetName", "plan"),
            member("ChangeSetType", "CREATE"),
            member("TemplateBody", template),
            member("Capabilities.member.1", "CAPABILITY_IAM"),
            member("Capabilities.member.2", "CAPABILITY_AUTO_EXPAND"),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: AWS creates this change set, got {code} {body}"
    );
    settle_the_store(router, engine).await;
    let (code, body) = call(
        router,
        "DescribeChangeSet",
        &[member("ChangeSetName", "plan"), member("StackName", stack)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: the change set must be describable, got {code} {body}"
    );
    body
}

/// Every `(LogicalResourceId, ResourceType, Action)` one `DescribeChangeSet` response
/// plans, sorted by logical id.
fn planned_resources(xml: &str) -> Vec<(String, String, String)> {
    let mut planned: Vec<(String, String, String)> = xml_blocks(xml, "ResourceChange")
        .iter()
        .map(|change| {
            (
                xml_text(change, "LogicalResourceId").unwrap_or_default(),
                xml_text(change, "ResourceType").unwrap_or_default(),
                xml_text(change, "Action").unwrap_or_default(),
            )
        })
        .collect();
    planned.sort();
    planned
}

#[tokio::test]
async fn a_sam_template_plans_the_resources_the_transform_synthesizes() {
    let (router, engine) = router_with_engine();
    for case in cases() {
        let name = case.case;
        let body =
            planned_change_set(&router, &engine, name, case.stack, &case.template).await;
        let planned = planned_resources(&body);

        let mut expected = case.planned.clone();
        expected.sort_by_key(|(id, _)| id.described());
        assert_eq!(
            planned.len(),
            expected.len(),
            "case {name:?}: expected {} planned resources ({:?}), got {:?}",
            expected.len(),
            expected
                .iter()
                .map(|(id, kind)| format!("{} ({kind})", id.described()))
                .collect::<Vec<String>>(),
            planned
        );
        for ((id, kind), (actual_id, actual_kind, action)) in expected.iter().zip(&planned) {
            assert!(
                id.matches(actual_id),
                "case {name:?}: expected the logical id {}, got {actual_id:?} in {planned:?}",
                id.described()
            );
            assert_eq!(
                actual_kind, kind,
                "case {name:?} resource {actual_id:?}: expected the type {kind:?}, got \
                 {actual_kind:?}"
            );
            // Nothing exists yet, so every resource the plan carries is an addition.
            assert_eq!(
                action,
                change_action::ADD,
                "case {name:?} resource {actual_id:?}: expected the {} action, got {action:?}",
                change_action::ADD
            );
        }
    }
}

/// The template one hash row submits: an implicit API whose route and whose function
/// configuration a row varies independently.
fn hashed_template(path: &str, method: &str, timeout: u32, extra_route: Option<&str>) -> String {
    let extra = match extra_route {
        Some(path) => format!(
            r#","Second":{{"Type":"Api","Properties":{{"Path":"{path}","Method":"get"}}}}"#
        ),
        None => String::new(),
    };
    sam_template(&format!(
        r#"{{"Hello":{{"Type":"AWS::Serverless::Function","Properties":{{
             "Handler":"index.handler","Runtime":"python3.12","Timeout":{timeout},
             "CodeUri":"s3://code-bucket/hello.zip",
             "Events":{{"First":{{"Type":"Api","Properties":{{
                "Path":"{path}","Method":"{method}"}}}}{extra}}}}}}}}}"#
    ))
}

/// One row of the deployment-hash table: the template it submits, and whether its
/// deployment's logical id must be the baseline's.
struct HashCase {
    case: &'static str,
    stack: &'static str,
    template: String,
    same_as_baseline: bool,
}

fn hash_cases() -> Vec<HashCase> {
    vec![
        HashCase {
            case: "the same template again",
            stack: "hash-same",
            template: hashed_template("/hello", "get", 3, None),
            same_as_baseline: true,
        },
        HashCase {
            case: "a change outside the API definition",
            stack: "hash-timeout",
            template: hashed_template("/hello", "get", 30, None),
            same_as_baseline: true,
        },
        HashCase {
            case: "a different path",
            stack: "hash-path",
            template: hashed_template("/goodbye", "get", 3, None),
            same_as_baseline: false,
        },
        HashCase {
            case: "a different method",
            stack: "hash-method",
            template: hashed_template("/hello", "post", 3, None),
            same_as_baseline: false,
        },
        HashCase {
            case: "one more route",
            stack: "hash-two-routes",
            template: hashed_template("/hello", "get", 3, Some("/also")),
            same_as_baseline: false,
        },
    ]
}

/// The single implicit-API deployment a plan carries.
fn deployment_id(case: &str, xml: &str) -> String {
    let deployments: Vec<String> = xml_blocks(xml, "ResourceChange")
        .iter()
        .filter(|change| {
            xml_text(change, "ResourceType").as_deref() == Some("AWS::ApiGateway::Deployment")
        })
        .filter_map(|change| xml_text(change, "LogicalResourceId"))
        .collect();
    assert_eq!(
        deployments.len(),
        1,
        "case {case:?}: the transform synthesizes exactly one deployment, got {deployments:?}"
    );
    let id = deployments[0].clone();
    assert!(
        Planned::Hashed("ServerlessRestApiDeployment").matches(&id),
        "case {case:?}: expected a content-hash-suffixed deployment id, got {id:?}"
    );
    id
}

#[tokio::test]
async fn the_implicit_apis_deployment_id_is_a_hash_of_the_apis_definition() {
    let (router, engine) = router_with_engine();
    let baseline = deployment_id(
        "the baseline",
        &planned_change_set(
            &router,
            &engine,
            "the baseline",
            "hash-baseline",
            &hashed_template("/hello", "get", 3, None),
        )
        .await,
    );
    for case in hash_cases() {
        let name = case.case;
        let body =
            planned_change_set(&router, &engine, name, case.stack, &case.template).await;
        let id = deployment_id(name, &body);
        if case.same_as_baseline {
            assert_eq!(
                id, baseline,
                "case {name:?}: the same API definition plans the same deployment, got {id:?}"
            );
        } else {
            assert_ne!(
                id, baseline,
                "case {name:?}: an edited API definition plans a different deployment, got \
                 {id:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_deployed_sam_stack_holds_the_synthesized_resources_and_keeps_its_own_template() {
    let (router, engine) = router_with_engine();
    let template = sam_template(&format!(
        "{{{}}}",
        implicit_api_function("Hello", "Api", "/hello", "get")
    ));
    crate::stack_resource_provisioning::stage_code_objects(
        &router,
        "a deployed SAM stack",
        &template,
    )
    .await;
    let (code, body) = create_stack(
        &router,
        "sam-deployed",
        &template,
        &[
            member("Capabilities.member.1", "CAPABILITY_IAM"),
            // "If your stack template contains one or more macros […] you must acknowledge
            // this capability. This includes the AWS::Include and AWS::Serverless
            // transforms, which are macros hosted by CloudFormation": the SAM transform is
            // one of them, so a request that DEPLOYS this template acknowledges it.
            member("Capabilities.member.2", "CAPABILITY_AUTO_EXPAND"),
        ],
    )
    .await;
    assert_eq!(code, 200, "the stack must be created, got {code} {body}");
    settle_the_store(&router, &engine).await;

    let (code, body) = call(
        &router,
        "DescribeStacks",
        &[member("StackName", "sam-deployed")],
    )
    .await;
    assert_eq!(code, 200, "the stack must be describable, got {code} {body}");
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "the stack deploys the resources the transform synthesized, got {body}"
    );

    let (code, body) = call(
        &router,
        "DescribeStackResources",
        &[member("StackName", "sam-deployed")],
    )
    .await;
    assert_eq!(code, 200, "the resources must be readable, got {code} {body}");
    let mut types = xml_texts(&body, "ResourceType");
    types.sort();
    types.dedup();
    assert_eq!(
        types,
        vec![
            "AWS::ApiGateway::Deployment".to_string(),
            "AWS::ApiGateway::RestApi".to_string(),
            "AWS::ApiGateway::Stage".to_string(),
            "AWS::IAM::Role".to_string(),
            "AWS::Lambda::Function".to_string(),
            "AWS::Lambda::Permission".to_string(),
        ],
        "the deployed stack holds the synthesized types and no AWS::Serverless::* one, got \
         {body}"
    );

    // The transform processes what a reader sees; the stack still STORES the body that was
    // submitted. "To get the user-submitted template, specify Original. To get the template
    // after CloudFormation has processed all transforms, specify Processed … By default,
    // CloudFormation specifies Processed."
    let (code, body) = call(
        &router,
        "GetTemplate",
        &[
            member("StackName", "sam-deployed"),
            member("TemplateStage", "Original"),
        ],
    )
    .await;
    assert_eq!(code, 200, "the template must be readable, got {code} {body}");
    assert_eq!(
        xml_text(&body, "TemplateBody").as_deref(),
        Some(template.as_str()),
        "the Original stage is the submitted body, got {body}"
    );

    let (code, body) = get_template(&router, "sam-deployed", None).await;
    assert_eq!(code, 200, "the template must be readable, got {code} {body}");
    let processed = xml_text(&body, "TemplateBody").unwrap_or_default();
    assert!(
        !processed.contains("AWS::Serverless::")
            && processed.contains("AWS::Lambda::Function")
            && processed.contains("AWS::ApiGateway::RestApi"),
        "the default stage is the PROCESSED template, got {processed:?}"
    );
    let mut stages = xml_texts(&body, "member");
    stages.sort();
    assert_eq!(
        stages,
        vec!["Original".to_string(), "Processed".to_string()],
        "both stages are available for a stack, got {body}"
    );
}
