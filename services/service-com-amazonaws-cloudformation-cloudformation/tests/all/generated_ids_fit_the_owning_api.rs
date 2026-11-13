// SPDX-License-Identifier: CC0-1.0

//! The identifier CloudFormation GENERATES for a resource whose template names none, where
//! the identifier is not a resource NAME but the member an owning API's create takes it as.
//!
//! RULE: the `<stack name>-<logical id>-<unique id>` identifier CloudFormation generates for
//! a resource a template leaves unnamed is FITTED to the constraint the service that owns
//! the resource type publishes for the member that resource is created under — its maximum
//! length and its permitted character set — by shortening the stack-name and logical-id
//! segments while the unique segment is kept whole, so the owning service accepts the call
//! and the stack that declared the resource reaches `CREATE_COMPLETE` instead of failing on
//! that service's refusal and rolling back.
//!
//! The rows differ along what the rule generalises over:
//!
//! * the MEMBER the identifier is sent as — a statement id (`AWS::Lambda::Permission`, whose
//!   type publishes no id property at all, so the identifier IS the statement id), a parent's
//!   name (`AWS::Lambda::LayerVersion`, published under the layer the identifier names) and a
//!   resource's own name (`AWS::StepFunctions::StateMachine`);
//! * the BOUND — 100, 140 and 80 characters, each taken from the owning service's own Smithy
//!   model (the source is named per row), which also makes one row of every size class fitted
//!   and another left whole;
//! * the SIZE of what is fitted — a stack name and logical ids well inside every bound, a
//!   CDK-sized stack name with the long logical id a construct generates, and the longest
//!   stack name CloudFormation accepts with a 150-character logical id, so each type is
//!   exercised both inside its bound and far past it.
//!
//! Each resource is read back through the OWNING service's own wire under the identifier the
//! stack reports — AWS Lambda's `GetPolicy` and `GetLayerVersion`, Step Functions'
//! `DescribeStateMachine` — so what the rows assert is that the service really holds the
//! resource under the fitted identifier, not merely that CloudFormation reported one.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::change_set_lifecycle::settle_the_store;
use crate::hotswapped_function_code::package;
use crate::stack_lifecycle::{
    acknowledging_iam, call, create_stack, describe_stack, member, router_with_engine, xml_blocks,
    xml_text, ACCOUNT_ID, REGION,
};
use crate::stack_resource_provisioning::s3;
use crate::step_functions_state_machine_replacement::sfn_json;
use service_com_amazonaws_cloudformation_cloudformation::state::{status, STACK_NAME_MAX_LEN};

/// The bucket the layer declaration below names its archive in, and the key it names in it.
const CODE_BUCKET: &str = "code-bucket";
const LAYER_KEY: &str = "deps.zip";

/// Stage the layer archive in Amazon S3, the way a client uploads it before deploying: AWS
/// Lambda READS the archive out of the object the `Content` names, so the object has to be
/// there and has to be a readable .zip, or the publish is refused and the stack rolls back.
async fn stage_archive(router: &MultiServiceRouter, case: &str) {
    let created = s3(router, "PUT", &format!("/{CODE_BUCKET}"), b"").await;
    assert!(
        (200..300).contains(&created.status),
        "case {case:?}: the code bucket must be created first, got {} {}",
        created.status,
        created.body
    );
    let archive = package("python/helpers.py", "VALUE = 1\n");
    let staged = s3(router, "PUT", &format!("/{CODE_BUCKET}/{LAYER_KEY}"), &archive).await;
    assert!(
        (200..300).contains(&staged.status),
        "case {case:?}: the archive {LAYER_KEY} must be staged first, got {} {}",
        staged.status,
        staged.body
    );
}

/// How many characters the unique segment of a generated identifier carries
/// (`<stack name>-<logical id>-<unique id>`, 12 hexadecimal characters).
const UNIQUE_SEGMENT_CHARACTERS: usize = 12;

/// The two hyphens that join the three segments of a generated identifier.
const SEPARATORS: usize = 2;

/// Which resource of the stack a row's assertions are about — and so which member its
/// generated identifier reaches its owning service as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// The statement id `AddPermission` adds the grant under.
    Permission,
    /// The layer name `PublishLayerVersion` publishes the version under.
    LayerVersion,
    /// The name `CreateStateMachine` creates the machine under.
    StateMachine,
}

/// One type whose generated identifier the owning API bounds.
struct Bounded {
    kind: Kind,
    resource_type: &'static str,
    /// The start of the logical id the template declares it under, which the rows then pad
    /// to the length they are testing.
    logical_prefix: &'static str,
    /// The longest identifier the owning service accepts for the member the resource is
    /// created under — see the comment on each row for where the bound comes from.
    limit: usize,
}

/// The types this table spans, each with the bound its owning service publishes.
const BOUNDED: &[Bounded] = &[
    // AWS Lambda `AddPermission.StatementId`: `StatementId`, `@length(min: 1, max: 100)`
    // with pattern `^([a-zA-Z0-9-_]+)$` (AWS Lambda's own Smithy model). The type publishes
    // no statement-id property, so the statement is always added under the identifier
    // CloudFormation generated.
    Bounded {
        kind: Kind::Permission,
        resource_type: "AWS::Lambda::Permission",
        logical_prefix: "ScheduleAllowEventRule",
        limit: 100,
    },
    // The same service's `PublishLayerVersion.LayerName`: `LayerName`,
    // `@length(min: 1, max: 140)`, whose bare-name branch is `[a-zA-Z0-9-_]+` (that same
    // model). `LayerName` is not a required property of the type, so a declaration that
    // states none publishes the version under the generated identifier.
    Bounded {
        kind: Kind::LayerVersion,
        resource_type: "AWS::Lambda::LayerVersion",
        logical_prefix: "ReportDeps",
        limit: 140,
    },
    // AWS Step Functions `CreateStateMachine.name`: `Name`, `@length(min: 1, max: 80)`
    // (that service's own Smithy model), whose member doc forbids white space, brackets,
    // wildcards, its listed special characters, control characters and surrogates — none of
    // which a generated identifier carries.
    Bounded {
        kind: Kind::StateMachine,
        resource_type: "AWS::StepFunctions::StateMachine",
        logical_prefix: "ReportWorkflow",
        limit: 80,
    },
];

/// One row: the stack the resources are declared in, and how long their logical ids are.
struct Row {
    case: &'static str,
    stack: String,
    /// How many characters every logical id of this row carries.
    logical_characters: usize,
    /// The name the row's function is declared under, which is NOT generated: the grant is
    /// read back through that function's own policy.
    function: &'static str,
}

/// The logical id a row declares `prefix` under: the prefix padded with alphanumerics to
/// `characters`, which is what a CDK construct's own logical ids look like.
fn logical_id(prefix: &str, characters: usize) -> String {
    let padding = "ScheduledReportStackReportFunctionHandlerResource".repeat(8);
    let mut id = prefix.to_string();
    let held = id.chars().count();
    assert!(
        characters >= held,
        "a row must ask for at least the {held} characters of {prefix:?}, got {characters}"
    );
    id.push_str(&padding[..characters - held]);
    id
}

/// The stack the longest row uses: the longest name CloudFormation accepts ("A stack name
/// can contain only alphanumeric characters (case-sensitive) and hyphens … and can't be
/// longer than 128 characters").
fn longest_stack_name() -> String {
    let head = "platform-production-scheduled-report-email-workflow-stack-";
    let mut name = head.to_string();
    name.push_str(&"a".repeat(STACK_NAME_MAX_LEN - head.len()));
    name
}

/// The rows, from one inside every bound to one far past all three.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a short stack name and short logical ids",
            stack: "report-stack".to_string(),
            logical_characters: 24,
            function: "report-fn-short",
        },
        Row {
            case: "a CDK stack name and the long logical ids a construct generates",
            stack: "cdk-scheduled-report-email-workflow-stack".to_string(),
            logical_characters: 70,
            function: "report-fn-cdk",
        },
        Row {
            case: "the longest stack name and a 150-character logical id",
            stack: longest_stack_name(),
            logical_characters: 150,
            function: "report-fn-longest",
        },
    ]
}

/// The template a row deploys: the execution role and the function the grant is made on,
/// beside one unnamed resource of every row of [`BOUNDED`].
fn template(row: &Row) -> String {
    let mut resources = serde_json::Map::new();
    resources.insert(
        "HandlerRole".to_string(),
        json!({
            "Type": "AWS::IAM::Role",
            "Properties": {
                "AssumeRolePolicyDocument": {
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": {"Service": "lambda.amazonaws.com"},
                        "Action": "sts:AssumeRole"
                    }]
                }
            }
        }),
    );
    resources.insert(
        "Handler".to_string(),
        json!({
            "Type": "AWS::Lambda::Function",
            "Properties": {
                "FunctionName": row.function,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Role": {"Fn::GetAtt": ["HandlerRole", "Arn"]},
                "Code": {"ZipFile": "def handler(event, context):\n    return {}\n"}
            }
        }),
    );
    for bounded in BOUNDED {
        let properties = match bounded.kind {
            // The grant the scheduled-report workflow needs: EventBridge invoking the
            // function the same stack declares. The type publishes no statement id.
            Kind::Permission => json!({
                "FunctionName": {"Fn::GetAtt": ["Handler", "Arn"]},
                "Action": "lambda:InvokeFunction",
                "Principal": "events.amazonaws.com",
                "SourceArn": format!(
                    "arn:aws:events:{REGION}:{ACCOUNT_ID}:rule/report-schedule"
                ),
            }),
            // No `LayerName`, so the version is published under the generated identifier.
            Kind::LayerVersion => json!({
                "Content": {"S3Bucket": CODE_BUCKET, "S3Key": LAYER_KEY},
                "CompatibleRuntimes": ["python3.12"],
            }),
            // No `StateMachineName`, so the machine is created under the generated one.
            Kind::StateMachine => json!({
                "DefinitionString":
                    r#"{"StartAt":"Done","States":{"Done":{"Type":"Pass","End":true}}}"#,
                "RoleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/StepFunctionsRole"),
            }),
        };
        resources.insert(
            logical_id(bounded.logical_prefix, row.logical_characters),
            json!({"Type": bounded.resource_type, "Properties": properties}),
        );
    }
    json!({"AWSTemplateFormatVersion": "2010-09-09", "Resources": resources}).to_string()
}

/// Call one AWS Lambda operation over its restJson1 wire, signed for this suite's Region.
async fn lambda(router: &MultiServiceRouter, method: &str, uri: &str) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/lambda/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::new())
        .expect("a well-formed Lambda request");
    let response = router.dispatch(request).await;
    let code = response.status().as_u16();
    let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (code, answered)
}

/// Deploy a row's stack, settle the store and answer the `(logical id, physical id)` pairs
/// `DescribeStackResources` reports for it.
async fn deployed(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    row: &Row,
) -> Vec<(String, String)> {
    stage_archive(router, row.case).await;
    let (code, body) =
        create_stack(router, &row.stack, &template(row), &acknowledging_iam(&[])).await;
    assert_eq!(
        code, 200,
        "case {:?}: expected CreateStack to be accepted, got {code} {body}",
        row.case
    );
    settle_the_store(router, engine).await;

    // The issue's own observable: the stack COMPLETES. Before the fix the generated
    // statement id was past AWS Lambda's 100 characters, Lambda refused the grant, and the
    // stack rolled back.
    let (code, described) = describe_stack(router, &row.stack).await;
    assert_eq!(
        code, 200,
        "case {:?}: expected DescribeStacks to answer, got {code} {described}",
        row.case
    );
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "case {:?}: expected the stack to reach CREATE_COMPLETE, got {:?} ({:?})",
        row.case,
        xml_text(&described, "StackStatus"),
        xml_text(&described, "StackStatusReason")
    );

    let (code, body) = call(
        router,
        "DescribeStackResources",
        &[member("StackName", &row.stack)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {:?}: expected DescribeStackResources to answer, got {code} {body}",
        row.case
    );
    xml_blocks(&body, "member")
        .iter()
        .map(|entry| {
            (
                xml_text(entry, "LogicalResourceId").unwrap_or_default(),
                xml_text(entry, "PhysicalResourceId").unwrap_or_default(),
            )
        })
        .collect()
}

/// The physical id the stack published for one of its logical resources.
fn physical_id(case: &str, reported: &[(String, String)], logical_id: &str) -> String {
    reported
        .iter()
        .find(|(logical, _)| logical == logical_id)
        .map(|(_, physical)| physical.clone())
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the stack to report {logical_id:?}, got {reported:?}")
        })
}

/// The `(layer, version)` a layer version ARN this emulator published names.
fn layer_of(case: &str, arn: &str) -> (String, String) {
    let prefix = format!("arn:aws:lambda:{REGION}:{ACCOUNT_ID}:layer:");
    let rest = arn.strip_prefix(&prefix).unwrap_or_else(|| {
        panic!("case {case:?}: expected a layer version ARN under {prefix:?}, got {arn:?}")
    });
    let (layer, version) = rest.rsplit_once(':').unwrap_or_else(|| {
        panic!("case {case:?}: expected the ARN to end in a version number, got {arn:?}")
    });
    (layer.to_string(), version.to_string())
}

/// The name a state machine ARN this emulator published names.
fn machine_name_of(case: &str, arn: &str) -> String {
    let prefix = format!("arn:aws:states:{REGION}:{ACCOUNT_ID}:stateMachine:");
    arn.strip_prefix(&prefix)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a state machine ARN under {prefix:?}, got {arn:?}")
        })
        .to_string()
}

/// The identifier the owning service was called with, read out of the physical id the stack
/// published.
fn generated_identifier(case: &str, kind: Kind, physical: &str) -> String {
    match kind {
        Kind::Permission => physical.to_string(),
        Kind::LayerVersion => layer_of(case, physical).0,
        Kind::StateMachine => machine_name_of(case, physical),
    }
}

/// `(the stack-name segment, the logical-id segment, the unique segment)` of a generated
/// identifier: the unique segment is the last, and a logical id carries no hyphen, so the
/// two hyphens that matter are the last two.
fn segments(identifier: &str) -> Option<(&str, &str, &str)> {
    let (head, unique) = identifier.rsplit_once('-')?;
    let (stack, logical) = head.rsplit_once('-')?;
    Some((stack, logical, unique))
}

/// The whole rule, for every type of [`BOUNDED`] in one row's stack.
async fn identifiers_are_fitted_to_the_owning_api(row: &Row) {
    let (router, engine) = router_with_engine();
    let reported = deployed(&router, &engine, row).await;

    for bounded in BOUNDED {
        let logical = logical_id(bounded.logical_prefix, row.logical_characters);
        let case = format!("{}/{}", row.case, bounded.resource_type);
        let physical = physical_id(&case, &reported, &logical);
        let identifier = generated_identifier(&case, bounded.kind, &physical);
        let characters = identifier.chars().count();

        // Inside the bound the owning API publishes.
        assert!(
            characters <= bounded.limit,
            "case {case:?}: expected an identifier within the {} characters its owning \
             service accepts, got {characters} ({identifier:?})",
            bounded.limit
        );
        // And the room is USED: nothing is given up that the bound did not require, beyond
        // the at most two hyphens a cut segment gives up so the identifier carries no
        // doubled separator.
        let unfitted = row.stack.chars().count()
            + logical.chars().count()
            + UNIQUE_SEGMENT_CHARACTERS
            + SEPARATORS;
        let expected = unfitted.min(bounded.limit);
        let floor = expected - SEPARATORS;
        assert!(
            (floor..=expected).contains(&characters),
            "case {case:?}: expected an identifier of {floor}..={expected} characters \
             (unfitted {unfitted}), got {characters} ({identifier:?})"
        );

        // The FORM: three segments, the unique one kept whole, the other two prefixes of the
        // stack name and the logical id they were shortened from.
        let (stack_segment, logical_segment, unique) = segments(&identifier)
            .unwrap_or_else(|| panic!("case {case:?}: {identifier:?} must carry three segments"));
        assert!(
            unique.chars().count() == UNIQUE_SEGMENT_CHARACTERS
                && unique
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase()),
            "case {case:?}: expected a {UNIQUE_SEGMENT_CHARACTERS}-character hexadecimal \
             unique segment, got {unique:?} in {identifier:?}"
        );
        assert!(
            row.stack.starts_with(stack_segment.trim_end_matches('-')),
            "case {case:?}: expected the first segment to be a prefix of the stack name, got \
             {stack_segment:?} in {identifier:?}"
        );
        assert!(
            !logical_segment.is_empty() && logical.starts_with(logical_segment),
            "case {case:?}: expected the second segment to be a prefix of the logical id, got \
             {logical_segment:?} in {identifier:?}"
        );
        // Nothing is fitted away where the whole identifier is within the bound.
        if unfitted <= bounded.limit {
            assert_eq!(
                identifier,
                format!("{}-{logical}-{unique}", row.stack),
                "case {case:?}: an identifier inside its bound must be the whole \
                 <stack name>-<logical id>-<unique id>"
            );
        }

        // The CHARACTER SET every one of these members admits (letters, digits, the hyphen
        // and the underscore), with no doubled separator and an alphanumeric at each end.
        assert!(
            identifier
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                && identifier.starts_with(|c: char| c.is_ascii_alphanumeric())
                && identifier.ends_with(|c: char| c.is_ascii_alphanumeric())
                && !identifier.contains("--"),
            "case {case:?}: expected an identifier of alphanumerics and single separators, \
             beginning and ending with an alphanumeric, got {identifier:?}"
        );

        // And the OWNING service really holds the resource under it.
        match bounded.kind {
            Kind::Permission => {
                let (code, answered) = lambda(
                    &router,
                    "GET",
                    &format!("/2015-03-31/functions/{}/policy", row.function),
                )
                .await;
                assert_eq!(
                    code, 200,
                    "case {case:?}: expected AWS Lambda to report the function's policy, got \
                     {code} {answered}"
                );
                let document: Value = answered
                    .get("Policy")
                    .and_then(Value::as_str)
                    .and_then(|policy| serde_json::from_str(policy).ok())
                    .unwrap_or(Value::Null);
                let held: Vec<String> = document
                    .get("Statement")
                    .and_then(Value::as_array)
                    .map(|statements| {
                        statements
                            .iter()
                            .filter_map(|statement| statement.get("Sid"))
                            .filter_map(Value::as_str)
                            .map(ToString::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                assert!(
                    held.contains(&identifier),
                    "case {case:?}: expected the function's policy to hold the statement \
                     {identifier:?}, got {held:?}"
                );
            }
            Kind::LayerVersion => {
                let (layer, version) = layer_of(&case, &physical);
                let (code, answered) = lambda(
                    &router,
                    "GET",
                    &format!("/2018-10-31/layers/{layer}/versions/{version}"),
                )
                .await;
                assert_eq!(
                    code, 200,
                    "case {case:?}: expected AWS Lambda to hold the published version, got \
                     {code} {answered}"
                );
                assert_eq!(
                    answered.get("LayerVersionArn").and_then(Value::as_str),
                    Some(physical.as_str()),
                    "case {case:?}: expected the version to report {physical:?}, got {answered}"
                );
            }
            Kind::StateMachine => {
                let (code, answered) = sfn_json(
                    &router,
                    "DescribeStateMachine",
                    &json!({"stateMachineArn": physical}),
                )
                .await;
                assert_eq!(
                    code, 200,
                    "case {case:?}: expected Step Functions to hold the machine, got {code} \
                     {answered}"
                );
                assert_eq!(
                    answered.get("name").and_then(Value::as_str),
                    Some(identifier.as_str()),
                    "case {case:?}: expected the machine to be named {identifier:?}, got \
                     {answered}"
                );
            }
        }
    }

    // Two resources of one stack never collide, however much the fitting shortened them —
    // which is why the unique segment is what survives it.
    let mut published: Vec<&String> = reported.iter().map(|(_, physical)| physical).collect();
    let before = published.len();
    published.sort();
    published.dedup();
    assert_eq!(
        published.len(),
        before,
        "case {:?}: every identifier of one stack must be distinct, got {published:?}",
        row.case
    );

    // A READ does not change them: the identifiers a second DescribeStackResources reports
    // are the ones the first did.
    let (code, body) = call(
        &router,
        "DescribeStackResources",
        &[member("StackName", &row.stack)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {:?}: expected the second read to answer, got {code} {body}",
        row.case
    );
    let again: Vec<(String, String)> = xml_blocks(&body, "member")
        .iter()
        .map(|entry| {
            (
                xml_text(entry, "LogicalResourceId").unwrap_or_default(),
                xml_text(entry, "PhysicalResourceId").unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        again, reported,
        "case {:?}: the published identifiers must be stable across reads",
        row.case
    );
}

#[tokio::test]
async fn a_generated_identifier_is_fitted_to_the_bound_its_owning_api_publishes() {
    for row in rows() {
        identifiers_are_fitted_to_the_owning_api(&row).await;
    }
}
