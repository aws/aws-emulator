// SPDX-License-Identifier: CC0-1.0

//! What a DEPLOYED SAM stack holds, read back over the awsQuery wire.
//!
//! The rule every row here holds the service to: **a template whose `Transform` section
//! names `AWS::Serverless-2016-10-31` is PROCESSED by that macro before the stack deploys
//! anything, so the resources the stack reports are the concrete ones the macro synthesized
//! (a function's `AWS::Lambda::Function`, the `AWS::IAM::Role` it runs as when it declares
//! none of its own, its `AWS::Lambda::Permission` per API event, an API's
//! `AWS::ApiGateway::RestApi`/`Deployment`/`Stage`, a simple table's
//! `AWS::DynamoDB::Table`) beside the resources the macro has nothing to do with, which
//! pass through under the type and the logical id the template gave them — and never an
//! `AWS::Serverless::*` type, which exists only as the macro's input.**
//!
//! `tests/all/sam_transform.rs` holds the macro to what it EXPANDS a declaration into (the
//! plan, the logical ids, the deployment's content hash) from one implicit-API template.
//! This suite is about the DEPLOYED stack a client then reads: the rows differ along which
//! serverless types the template declares, whether its API is explicit or implicit, whether
//! its function names a role of its own, whether resources the macro leaves alone are
//! declared beside them, and whether the body is spelled as JSON or as YAML. Both readers a
//! client has for a deployed stack are asserted (`DescribeStackResources` and
//! `ListStackResources`), because the defect this pins was visible through them, and so is
//! `GetTemplate`'s pair of stages.
//!
//! What this suite deliberately does NOT re-run, because `sam_transform.rs` owns it: the
//! LOGICAL IDS the macro synthesizes (the API trio's, the role's, the permission's), the
//! deployment logical id's content hash and its stability under an edited definition, what
//! a change set PLANS for a SAM template, and the serverless types this crate expands
//! beyond the three below (`HttpApi`, `LayerVersion`, `StateMachine`) or deliberately
//! leaves as declared. A row here states the resource-type SET its template deploys plus
//! the identities the template itself is entitled to keep, never a synthesized id.
//!
//! The recorded trace this suite was written for is ONE row of the table: an explicit
//! `AWS::Serverless::Api`, a function routed to it and a `AWS::Serverless::SimpleTable`,
//! beside CloudWatch and CloudWatch Logs resources the macro leaves alone.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use crate::change_set_lifecycle::settle_the_store;
use crate::stack_lifecycle::{
    call, create_stack, iam, kv, member, router_with_engine, xml_blocks, xml_text,
};
use crate::sam_transform::sam_template;
use crate::stack_resources::wire_resources;
use crate::stack_resource_provisioning::stage_code_objects;

/// The namespace a SAM resource type lives under: no resource of a DEPLOYED stack may
/// carry it, because the macro consumed every such declaration.
const SERVERLESS_TYPE_PREFIX: &str = "AWS::Serverless::";

/// One row of the deployed-resources table.
struct Row {
    case: &'static str,
    stack: &'static str,
    template: String,
    /// Every resource TYPE the deployed stack must hold — sorted and deduplicated, so a
    /// row states the type SET its template expands to rather than the synthesized logical
    /// ids (which `sam_transform.rs` owns).
    types: Vec<&'static str>,
    /// `(logical id, type)` pairs the deployed stack must still carry under the identity
    /// the TEMPLATE gave them: a resource the macro leaves alone, and the ones SAM keeps
    /// the declared logical id of.
    kept: Vec<(&'static str, &'static str)>,
    /// The execution role this row's template NAMES rather than declares, created in IAM
    /// before the stack deploys — an execution role provisioned outside the template, which
    /// is what a function that "names a role of its own" is given.
    external_role: Option<&'static str>,
}

/// A trust policy that lets AWS Lambda assume the role, which is what the service resolves
/// before it creates a function that runs as it.
fn lambda_trust_policy() -> String {
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
         "Principal":{"Service":"lambda.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#
        .to_string()
}

/// A function declaration whose `Api` event names `RestApiId`, which is what makes the API
/// it is served by the EXPLICIT one the template declares.
///
/// The counterpart of `sam_transform.rs`'s `implicit_api_function`, which is the same
/// declaration WITHOUT a `RestApiId` — the member is the whole difference between an
/// explicit API and the one the macro adds, so the two builders state it rather than one
/// taking a flag.
fn routed_function(logical_id: &str, api: &str, path: &str, method: &str) -> String {
    format!(
        r#""{logical_id}":{{"Type":"AWS::Serverless::Function","Properties":{{
             "Handler":"index.handler","Runtime":"python3.12",
             "CodeUri":"s3://code-bucket/{logical_id}.zip",
             "Events":{{"Route":{{"Type":"Api","Properties":{{
                "Path":"{path}","Method":"{method}","RestApiId":{{"Ref":"{api}"}}}}}}}}}}}}"#
    )
}

/// The resources a serverless application is monitored by, which the macro has nothing to
/// do with: they are part of the processed template exactly as the client wrote them.
///
/// `group` names the log group, because the rows below deploy against ONE emulator: a log
/// group is really created in Amazon CloudWatch Logs now
/// (`state::provisioning::logs_resources`), and two stacks cannot both create the group of
/// the same name — the second create is refused, exactly as it is in an account.
fn monitoring_resources(group: &str) -> String {
    format!(
        r#""Failures":{{"Type":"AWS::Logs::LogGroup",
         "Properties":{{"LogGroupName":"{group}","RetentionInDays":7}}}},
       "FailureCount":{{"Type":"AWS::Logs::MetricFilter",
         "Properties":{{"LogGroupName":{{"Ref":"Failures"}},"FilterPattern":"ERROR",
           "MetricTransformations":[{{"MetricName":"Failures","MetricNamespace":"Orders",
             "MetricValue":"1"}}]}}}},
       "FailureAlarm":{{"Type":"AWS::CloudWatch::Alarm",
         "Properties":{{"AlarmName":"orders-failures","ComparisonOperator":"GreaterThanThreshold",
           "EvaluationPeriods":1,"Threshold":1,"MetricName":"Failures","Namespace":"Orders",
           "Period":60,"Statistic":"Sum"}}}}"#
    )
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an explicit Api, a function routed to it and a simple table, beside \
                   resources the macro leaves alone",
            stack: "sam-deployed-explicit-api",
            template: sam_template(&format!(
                r#"{{"OrdersApi":{{"Type":"AWS::Serverless::Api",
                     "Properties":{{"StageName":"live"}}}},
                   "Orders":{{"Type":"AWS::Serverless::SimpleTable",
                     "Properties":{{"PrimaryKey":{{"Name":"orderId","Type":"String"}}}}}},
                   {},{}}}"#,
                routed_function("Place", "OrdersApi", "/orders", "post"),
                monitoring_resources("/aws/lambda/orders")
            )),
            types: vec![
                "AWS::ApiGateway::Deployment",
                "AWS::ApiGateway::RestApi",
                "AWS::ApiGateway::Stage",
                "AWS::CloudWatch::Alarm",
                "AWS::DynamoDB::Table",
                "AWS::IAM::Role",
                "AWS::Lambda::Function",
                "AWS::Lambda::Permission",
                "AWS::Logs::LogGroup",
                "AWS::Logs::MetricFilter",
            ],
            kept: vec![
                // SAM keeps the declared logical id of the resource it expands a
                // declaration into, so these are the ids the template itself states.
                ("OrdersApi", "AWS::ApiGateway::RestApi"),
                ("Orders", "AWS::DynamoDB::Table"),
                ("Place", "AWS::Lambda::Function"),
                // Untouched by the macro.
                ("Failures", "AWS::Logs::LogGroup"),
                ("FailureCount", "AWS::Logs::MetricFilter"),
                ("FailureAlarm", "AWS::CloudWatch::Alarm"),
            ],
            external_role: None,
        },
        Row {
            case: "a simple table alone, the one serverless type that expands to a single \
                   resource",
            stack: "sam-deployed-table",
            template: sam_template(
                r#"{"Sessions":{"Type":"AWS::Serverless::SimpleTable",
                     "Properties":{"PrimaryKey":{"Name":"sessionId","Type":"String"}}}}"#,
            ),
            types: vec!["AWS::DynamoDB::Table"],
            kept: vec![("Sessions", "AWS::DynamoDB::Table")],
            external_role: None,
        },
        Row {
            case: "a function that names a role of its own: no role is synthesized for it",
            stack: "sam-deployed-own-role",
            template: sam_template(
                r#"{"Worker":{"Type":"AWS::Serverless::Function","Properties":{
                     "Handler":"index.handler","Runtime":"python3.12",
                     "CodeUri":"s3://code-bucket/worker.zip",
                     "Role":"arn:aws:iam::000000000000:role/declared-by-the-template"}}}"#,
            ),
            types: vec!["AWS::Lambda::Function"],
            kept: vec![("Worker", "AWS::Lambda::Function")],
            external_role: Some("declared-by-the-template"),
        },
        Row {
            case: "the YAML spelling of a function with an implicit Api event and a table",
            stack: "sam-deployed-yaml",
            template: [
                "Transform: AWS::Serverless-2016-10-31",
                "Resources:",
                "  Greeter:",
                "    Type: AWS::Serverless::Function",
                "    Properties:",
                "      Handler: index.handler",
                "      Runtime: python3.12",
                "      CodeUri: s3://code-bucket/greeter.zip",
                "      Events:",
                "        Hello:",
                "          Type: Api",
                "          Properties:",
                "            Path: /hello",
                "            Method: get",
                "  Visits:",
                "    Type: AWS::Serverless::SimpleTable",
                "",
            ]
            .join("\n"),
            types: vec![
                "AWS::ApiGateway::Deployment",
                "AWS::ApiGateway::RestApi",
                "AWS::ApiGateway::Stage",
                "AWS::DynamoDB::Table",
                "AWS::IAM::Role",
                "AWS::Lambda::Function",
                "AWS::Lambda::Permission",
            ],
            kept: vec![
                ("Greeter", "AWS::Lambda::Function"),
                ("Visits", "AWS::DynamoDB::Table"),
                // The API no declaration names: the one the macro adds for the events that
                // name no `RestApiId`.
                ("ServerlessRestApi", "AWS::ApiGateway::RestApi"),
            ],
            external_role: None,
        },
        Row {
            case: "the macro declared over resources it expands nothing of",
            stack: "sam-deployed-nothing-to-expand",
            template: sam_template(&format!(
                "{{{}}}",
                monitoring_resources("/aws/lambda/orders-unexpanded")
            )),
            types: vec![
                "AWS::CloudWatch::Alarm",
                "AWS::Logs::LogGroup",
                "AWS::Logs::MetricFilter",
            ],
            kept: vec![
                ("Failures", "AWS::Logs::LogGroup"),
                ("FailureCount", "AWS::Logs::MetricFilter"),
                ("FailureAlarm", "AWS::CloudWatch::Alarm"),
            ],
            external_role: None,
        },
    ]
}

/// Deploy one row and answer the `(logical id, type)` pairs `DescribeStackResources`
/// reports for the settled stack.
async fn deployed_resources(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    row: &Row,
) -> Vec<(String, String)> {
    if let Some(role) = row.external_role {
        let (code, body) = iam(
            router,
            "CreateRole",
            &[
                ("RoleName", role),
                ("AssumeRolePolicyDocument", &lambda_trust_policy()),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {:?}: expected the execution role the template names to be created in IAM, \
             got {code} {body}",
            row.case
        );
    }
    stage_code_objects(router, row.case, &row.template).await;
    let (code, body) = create_stack(
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
        code, 200,
        "case {:?}: expected CreateStack to be accepted, got {code} {body}",
        row.case
    );
    settle_the_store(router, engine).await;

    let (code, body) = call(router, "DescribeStacks", &[member("StackName", row.stack)]).await;
    assert_eq!(
        code, 200,
        "case {:?}: expected DescribeStacks to answer, got {code} {body}",
        row.case
    );
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {:?}: expected the stack to deploy what the transform synthesized, got {body}",
        row.case
    );

    let (code, body) = call(
        router,
        "DescribeStackResources",
        &[member("StackName", row.stack)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {:?}: expected DescribeStackResources to answer, got {code} {body}",
        row.case
    );
    wire_resources(&body)
        .into_iter()
        .map(|resource| {
            (
                resource.logical_id.unwrap_or_default(),
                resource.resource_type.unwrap_or_default(),
            )
        })
        .collect()
}

/// The `(logical id, type)` pairs `ListStackResources` reports — the other reader a client
/// has for a deployed stack's resources, and the one the recorded trace read.
async fn listed_resources(router: &MultiServiceRouter, row: &Row) -> Vec<(String, String)> {
    let (code, body) = call(
        router,
        "ListStackResources",
        &[member("StackName", row.stack)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {:?}: expected ListStackResources to answer, got {code} {body}",
        row.case
    );
    xml_blocks(&body, "member")
        .iter()
        .map(|entry| {
            (
                xml_text(entry, "LogicalResourceId").unwrap_or_default(),
                xml_text(entry, "ResourceType").unwrap_or_default(),
            )
        })
        .collect()
}

/// The sorted, deduplicated resource types of a `(logical id, type)` list.
fn type_set(resources: &[(String, String)]) -> Vec<String> {
    let mut types: Vec<String> = resources
        .iter()
        .map(|(_, resource_type)| resource_type.clone())
        .collect();
    types.sort();
    types.dedup();
    types
}

/// The resource types a template BODY declares, read out of its `Resources` map.
///
/// Read as a document rather than searched for as text, so a type that appears inside a
/// generated OpenAPI definition or an `Fn::Sub` string cannot be mistaken for a resource
/// the template declares.
fn declared_types(case: &str, body: &str) -> Vec<String> {
    let document: ::serde_json::Value = ::serde_json::from_str(body)
        .unwrap_or_else(|error| panic!("case {case:?}: expected a JSON template, got {error}"));
    let resources = document
        .get("Resources")
        .and_then(::serde_json::Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?}: expected a Resources map, got {document}"));
    let mut types: Vec<String> = resources
        .values()
        .filter_map(|declaration| {
            declaration
                .get("Type")
                .and_then(::serde_json::Value::as_str)
                .map(ToString::to_string)
        })
        .collect();
    types.sort();
    types.dedup();
    types
}

#[tokio::test]
async fn a_deployed_sam_stack_holds_the_synthesized_resources_and_no_serverless_type() {
    let (router, engine) = router_with_engine();
    for row in rows() {
        let deployed = deployed_resources(&router, &engine, &row).await;

        let expected: Vec<String> = row.types.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            type_set(&deployed),
            expected,
            "case {:?}: expected exactly the types the transform leaves behind, got \
             {deployed:?}",
            row.case
        );

        let untransformed: Vec<&(String, String)> = deployed
            .iter()
            .filter(|(_, resource_type)| resource_type.starts_with(SERVERLESS_TYPE_PREFIX))
            .collect();
        assert!(
            untransformed.is_empty(),
            "case {:?}: expected no {SERVERLESS_TYPE_PREFIX}* type in the deployed stack, \
             got {untransformed:?}",
            row.case
        );

        for (logical_id, resource_type) in &row.kept {
            let found = deployed
                .iter()
                .find(|(deployed_id, _)| deployed_id == logical_id);
            assert_eq!(
                found.map(|(_, deployed_type)| deployed_type.as_str()),
                Some(*resource_type),
                "case {:?}: expected {logical_id} to be deployed as a {resource_type}, got \
                 {found:?} in {deployed:?}",
                row.case
            );
        }

        // Both readers report one deployed stack, so they report the same resources.
        let mut listed = listed_resources(&router, &row).await;
        let mut described = deployed.clone();
        listed.sort();
        described.sort();
        assert_eq!(
            listed, described,
            "case {:?}: expected ListStackResources and DescribeStackResources to report \
             the same resources, got {listed:?} and {described:?}",
            row.case
        );
    }
}

/// `GetTemplate` answers the SUBMITTED body for the `Original` stage and the macro's output
/// for `Processed` (the default): "To get the user-submitted template, specify Original. To
/// get the template after CloudFormation has processed all transforms, specify Processed …
/// By default, CloudFormation specifies Processed."
#[tokio::test]
async fn get_template_answers_the_submitted_body_and_the_processed_one() {
    let (router, engine) = router_with_engine();
    for row in rows() {
        let _ = deployed_resources(&router, &engine, &row).await;

        let (code, body) = call(
            &router,
            "GetTemplate",
            &[
                member("StackName", row.stack),
                member("TemplateStage", "Original"),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {:?}: expected GetTemplate to answer, got {code} {body}",
            row.case
        );
        assert_eq!(
            xml_text(&body, "TemplateBody")
                .map(|template| crate::stack_lifecycle::xml_unescape(&template)),
            Some(row.template.clone()),
            "case {:?}: expected the submitted body for the Original stage, got {body}",
            row.case
        );

        let (code, body) = call(&router, "GetTemplate", &[member("StackName", row.stack)]).await;
        assert_eq!(
            code, 200,
            "case {:?}: expected GetTemplate to answer, got {code} {body}",
            row.case
        );
        let processed = xml_text(&body, "TemplateBody")
            .map(|template| crate::stack_lifecycle::xml_unescape(&template))
            .unwrap_or_default();
        // The types the PROCESSED body declares are read out of its `Resources` map, so
        // this says what the template declares rather than what its text happens to
        // mention. The set equality below is what rules a macro input type out — no row's
        // expected set carries one — so it is asserted once, here.
        let expected: Vec<String> = row.types.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            declared_types(row.case, &processed),
            expected,
            "case {:?}: expected the processed template to declare exactly the types the \
             transform leaves behind, and no {SERVERLESS_TYPE_PREFIX}* one, got {processed}",
            row.case
        );
    }
}
