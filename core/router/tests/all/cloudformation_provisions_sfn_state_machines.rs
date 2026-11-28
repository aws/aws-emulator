// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::StepFunctions::StateMachine` really creates
//! the state machine in AWS Step Functions, and the handle the stack hands back is the ARN
//! that service answers to.
//!
//! The rule under test: **a resource of a type this emulator provisions is created in the
//! service that OWNS it, through that service's own public API, and the identifier the stack
//! publishes for it is the one that type's published return values document — for a state
//! machine, "Ref returns the ARN of the state machine" — so `Ref` and `Fn::GetAtt` resolve
//! to a value `sfn DescribeStateMachine` resolves to the machine the template declared, with
//! the definition that template stated.** Three crates have to agree for that to hold
//! (CloudFormation makes the call, Step Functions serves it, and EventBridge is handed the
//! ARN as a rule's target), so the suite lives here rather than in any of them.
//!
//! The rows differ along what the rule generalises over: which form states the workflow
//! (`DefinitionString`, or the `Definition` template object), whether
//! `DefinitionSubstitutions` fills placeholders in it, whether the template NAMES the
//! machine or leaves CloudFormation to generate its name, which `StateMachineType` it
//! declares, and whether it declares `Tags`.
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except the GENERATED name of the row that declares none, which
//! is CloudFormation's to compute.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{elements, form_encode, opt_element};

/// Long enough for the stack's clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) to come
/// due, so the stack reports the template it deployed.
const PAST_STACK_SPAN: Duration = Duration::from_millis(900);

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The execution role every row's machine declares. No IAM is mounted here, so nothing is
/// known about it and the create is served — which is the documented reading of a role
/// provisioned outside the client that creates the resource.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/StatesExecutionRole";

/// The emulator with the services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way
/// (the credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_sfn_awsstepfunctions::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which
/// is what routes the request and what each handler reads its Region from. The signature is
/// never checked: this router runs with request authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one operation of an awsJson service over its own wire, signed for `region`.
async fn json_rpc(
    router: &MultiServiceRouter,
    region: &str,
    signing_name: &str,
    target: &str,
    json_version: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{region}.amazonaws.com"))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", target)
        .header("authorization", authorization(signing_name, region))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// Call one Step Functions operation (`sigv4Name: states`, service shape
/// `AWSStepFunctions`, `awsJson1_0`).
async fn sfn(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    json_rpc(
        router,
        region,
        "states",
        &format!("AWSStepFunctions.{operation}"),
        "1.0",
        body,
    )
    .await
}

/// Call one EventBridge operation (`sigv4Name: events`, service shape `AWSEvents`,
/// `awsJson1_1`).
async fn events(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    json_rpc(
        router,
        region,
        "events",
        &format!("AWSEvents.{operation}"),
        "1.1",
        body,
    )
    .await
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// The ARN a state machine of `name` carries in `region`: the same total function of the
/// Region, the account and the name both services compute it from.
fn state_machine_arn(region: &str, name: &str) -> String {
    format!("arn:aws:states:{region}:{ACCOUNT}:stateMachine:{name}")
}

/// A one-state definition that ends immediately, as the template states it.
fn pass_definition(result: &str) -> Value {
    json!({
        "StartAt": "Done",
        "States": { "Done": { "Type": "Pass", "Result": result, "End": true } },
    })
}

/// One row of the table: a template, and what the rule says the state machine it declares
/// must then be.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region the machine is created in and the
    /// one its ARN carries.
    region: &'static str,
    /// The template body.
    template: fn() -> String,
    /// The name the template declares, or `None` for a template that names nothing — whose
    /// machine carries the name CloudFormation generated.
    declared_name: Option<&'static str>,
    /// The definition `DescribeStateMachine` must report, which is the document the template
    /// stated with its placeholders filled in.
    expected_definition: fn() -> Value,
    /// The `type` that read must report.
    expected_type: &'static str,
    /// The tags `sfn ListTagsForResource` must report, beside the three CloudFormation
    /// records on every resource it provisions.
    expected_tags: &'static [(&'static str, &'static str)],
    /// The output keys whose value must be the machine's ARN, each written with a different
    /// intrinsic over the same resource.
    arn_outputs: &'static [&'static str],
    /// The output keys whose value must be the machine's NAME.
    name_outputs: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        case: "a named, tagged machine whose workflow is a DefinitionString",
        region: "us-east-1",
        template: || {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "a Step Functions state machine",
                "Resources": {
                    "Machine": {
                        "Type": "AWS::StepFunctions::StateMachine",
                        "Properties": {
                            "StateMachineName": "orders-workflow",
                            "RoleArn": ROLE_ARN,
                            "DefinitionString": pass_definition("ordered").to_string(),
                            "Tags": [
                                { "Key": "Owner", "Value": "orders" },
                                { "Key": "Tier", "Value": "prod" },
                            ],
                        },
                    },
                },
                "Outputs": {
                    "MachineRef": { "Value": { "Ref": "Machine" } },
                    "MachineArn": { "Value": { "Fn::GetAtt": ["Machine", "Arn"] } },
                    "MachineName": { "Value": { "Fn::GetAtt": ["Machine", "Name"] } },
                },
            })
            .to_string()
        },
        declared_name: Some("orders-workflow"),
        expected_definition: || pass_definition("ordered"),
        expected_type: "STANDARD",
        expected_tags: &[("Owner", "orders"), ("Tier", "prod")],
        arn_outputs: &["MachineRef", "MachineArn"],
        name_outputs: &["MachineName"],
    },
    Row {
        case: "an unnamed EXPRESS machine whose workflow is a Definition object",
        region: "us-west-2",
        template: || {
            json!({
                "Resources": {
                    "Machine": {
                        "Type": "AWS::StepFunctions::StateMachine",
                        "Properties": {
                            "RoleArn": ROLE_ARN,
                            "StateMachineType": "EXPRESS",
                            "Definition": pass_definition("expressed"),
                        },
                    },
                },
                "Outputs": {
                    "MachineRef": { "Value": { "Ref": "Machine" } },
                    "MachineName": { "Value": { "Fn::GetAtt": ["Machine", "Name"] } },
                },
            })
            .to_string()
        },
        declared_name: None,
        expected_definition: || pass_definition("expressed"),
        expected_type: "EXPRESS",
        expected_tags: &[],
        arn_outputs: &["MachineRef"],
        name_outputs: &["MachineName"],
    },
    Row {
        case: "a machine whose definition carries placeholders the substitutions fill",
        region: "eu-west-1",
        template: || {
            json!({
                "Resources": {
                    "Machine": {
                        "Type": "AWS::StepFunctions::StateMachine",
                        "Properties": {
                            "StateMachineName": "substituted-workflow",
                            "RoleArn": ROLE_ARN,
                            "StateMachineType": "STANDARD",
                            "DefinitionString": json!({
                                "StartAt": "Done",
                                "States": {
                                    "Done": {
                                        "Type": "Pass",
                                        "Result": "${Outcome}",
                                        "End": true,
                                    },
                                },
                            })
                            .to_string(),
                            "DefinitionSubstitutions": { "Outcome": "filled-in" },
                            "Tags": [{ "Key": "Owner", "Value": "platform" }],
                        },
                    },
                },
                "Outputs": { "MachineRef": { "Value": { "Ref": "Machine" } } },
            })
            .to_string()
        },
        declared_name: Some("substituted-workflow"),
        expected_definition: || pass_definition("filled-in"),
        expected_type: "STANDARD",
        expected_tags: &[("Owner", "platform")],
        arn_outputs: &["MachineRef"],
        name_outputs: &[],
    },
];

/// Every row: the ARN the stack reports for its `AWS::StepFunctions::StateMachine` is the
/// machine's own, `sfn DescribeStateMachine` resolves it to the machine the template
/// declared with the definition it stated, the tags CloudFormation gives the resource are on
/// it, and deleting the stack deletes it there.
#[tokio::test]
async fn a_stack_provisions_the_state_machine_its_template_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("sfn-machine-stack-{index}");
        let template = (row.template)();

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

        let (status, resources) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             body {resources}"
        );
        let physical_id = opt_element(&resources, "PhysicalResourceId")
            .unwrap_or_else(|| panic!("case {case:?}: no <PhysicalResourceId> in {resources}"));

        // "Ref returns the ARN of the state machine": the physical id is that ARN, and the
        // name inside it is the one the template asked for — or, for a template that named
        // nothing, the one CloudFormation generated.
        let name = match row.declared_name {
            Some(declared) => declared.to_string(),
            None => physical_id
                .rsplit(':')
                .next()
                .unwrap_or_default()
                .to_string(),
        };
        let arn = state_machine_arn(region, &name);
        assert_eq!(
            physical_id, arn,
            "case {case:?}: expected the physical id to be the machine's ARN {arn}, got \
             {physical_id:?}"
        );
        assert!(
            !name.is_empty() && name.len() <= 80,
            "case {case:?}: expected a name inside the modelled 1..80, got {name:?}"
        );

        let outputs = outputs_of(&described);
        for key in row.arn_outputs {
            assert_eq!(
                outputs.get(*key),
                Some(&arn),
                "case {case:?}: expected output {key:?} to be the machine's ARN {arn:?}, got {:?}",
                outputs.get(*key)
            );
        }
        for key in row.name_outputs {
            assert_eq!(
                outputs.get(*key),
                Some(&name),
                "case {case:?}: expected output {key:?} to be the machine's name {name:?}, got \
                 {:?}",
                outputs.get(*key)
            );
        }

        // And Step Functions itself resolves that ARN to the machine the template declared.
        let (status, machine) =
            sfn(&router, region, "DescribeStateMachine", &json!({ "stateMachineArn": arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStateMachine on the ARN the stack \
             reported, got {status} with body {machine}"
        );
        assert_eq!(
            machine.get("name").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case:?}: expected the describe to report the name {name:?}, got {machine}"
        );
        assert_eq!(
            machine.get("roleArn").and_then(Value::as_str),
            Some(ROLE_ARN),
            "case {case:?}: expected the declared role, got {machine}"
        );
        assert_eq!(
            machine.get("type").and_then(Value::as_str),
            Some(row.expected_type),
            "case {case:?}: expected the type {:?}, got {machine}",
            row.expected_type
        );
        let reported: Value = machine
            .get("definition")
            .and_then(Value::as_str)
            .map(|text| serde_json::from_str(text).expect("the definition is JSON"))
            .unwrap_or_else(|| panic!("case {case:?}: no definition in {machine}"));
        assert_eq!(
            reported,
            (row.expected_definition)(),
            "case {case:?}: expected the definition the template stated, got {reported}"
        );

        // The tags the declaration asked for, plus the three CloudFormation records on every
        // resource it provisions, read through Step Functions' own tagging call.
        let (status, listed) =
            sfn(&router, region, "ListTagsForResource", &json!({ "resourceArn": arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListTagsForResource, got {status} with body \
             {listed}"
        );
        let tags: BTreeMap<String, String> = listed
            .get("tags")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no 'tags' array in {listed}"))
            .iter()
            .filter_map(|tag| {
                Some((
                    tag.get("key").and_then(Value::as_str)?.to_string(),
                    tag.get("value").and_then(Value::as_str)?.to_string(),
                ))
            })
            .collect();
        for (key, value) in row.expected_tags {
            assert_eq!(
                tags.get(*key).map(String::as_str),
                Some(*value),
                "case {case:?}: expected tag {key:?} to be {value:?}, got {tags:?}"
            );
        }
        let logical_id = opt_element(&resources, "LogicalResourceId")
            .unwrap_or_else(|| panic!("case {case:?}: no <LogicalResourceId> in {resources}"));
        let stack_id = opt_element(&described, "StackId")
            .unwrap_or_else(|| panic!("case {case:?}: no <StackId> in {described}"));
        for (key, value) in [
            ("aws:cloudformation:stack-name", stack_name.as_str()),
            ("aws:cloudformation:stack-id", stack_id.as_str()),
            ("aws:cloudformation:logical-id", logical_id.as_str()),
        ] {
            assert_eq!(
                tags.get(key).map(String::as_str),
                Some(value),
                "case {case:?}: expected tag {key:?} to be {value:?}, got {tags:?}"
            );
        }

        // Deleting the stack deletes the machine Step Functions holds for it. The stack's
        // own work is done once that service reports the deletion as BEGUN, and how long the
        // record then stands is the OWNING service's span — an Express machine's deletion
        // "happens eventually (usually in less than a minute)" — so the read below waits for
        // the refusal to stop reporting the deletion as in progress.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (status, gone) = read_until_gone(&router, region, case, &arn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from DescribeStateMachine once the stack deleted \
             the machine, got {status} with body {gone}"
        );
        assert!(
            gone.get("__type")
                .and_then(Value::as_str)
                .is_some_and(|code| code.ends_with("StateMachineDoesNotExist")),
            "case {case:?}: expected StateMachineDoesNotExist once the machine was deleted, got \
             {gone}"
        );
    }
}

/// How long a read waits for Step Functions' own deletion to go through before giving up:
/// generous, because what the loop waits for is the ANSWER CHANGING and never a span.
const DELETION_DEADLINE: Duration = Duration::from_secs(90);

/// How long one read pauses before looking again.
const DELETION_POLL_PAUSE: Duration = Duration::from_millis(100);

/// `DescribeStateMachine` on `arn` until Step Functions no longer answers for the record,
/// and `(the status, the body)` it then answered.
///
/// A machine whose deletion has BEGUN still answers: the read reports it `DELETING`. That is
/// what the stack waited for, so the read that says the record has GONE is the next one.
async fn read_until_gone(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    arn: &str,
) -> (u16, Value) {
    let deadline = std::time::Instant::now() + DELETION_DEADLINE;
    loop {
        let (status, answered) =
            sfn(router, region, "DescribeStateMachine", &json!({ "stateMachineArn": arn })).await;
        if status != 200 {
            return (status, answered);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: expected DescribeStateMachine({arn:?}) to stop answering for a \
             record the stack deleted, got {status} with body {answered}"
        );
        tokio::time::sleep(DELETION_POLL_PAUSE).await;
    }
}

/// The reference architecture's own shape: one stack declaring the state machine AND the
/// EventBridge rule whose target is that machine, where the rule reads the machine's ARN with
/// `Fn::GetAtt`.
///
/// This is what a type whose return values resolve to nothing breaks: the rule's target
/// carries whatever `Fn::GetAtt … Arn` resolved to, so a stack can only deploy if that is the
/// ARN the owning service answers to.
#[tokio::test]
async fn a_rule_in_the_same_stack_targets_the_machine_by_its_published_arn() {
    let router = build_router();
    let case = "a state machine and the schedule rule that targets it";
    let region = "us-west-2";
    let stack_name = "sfn-scheduled-poller";
    let template = json!({
        "Resources": {
            "Machine": {
                "Type": "AWS::StepFunctions::StateMachine",
                "Properties": {
                    "StateMachineName": "scheduled-poller",
                    "RoleArn": ROLE_ARN,
                    "DefinitionString": pass_definition("polled").to_string(),
                },
            },
            "Schedule": {
                "Type": "AWS::Events::Rule",
                "Properties": {
                    "Name": "poller-schedule",
                    "ScheduleExpression": "rate(1 minute)",
                    "State": "ENABLED",
                    "Targets": [{
                        "Id": "machine",
                        "Arn": { "Fn::GetAtt": ["Machine", "Arn"] },
                        "RoleArn": ROLE_ARN,
                    }],
                },
            },
        },
        "Outputs": { "MachineArn": { "Value": { "Fn::GetAtt": ["Machine", "Arn"] } } },
    })
    .to_string();

    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;

    let (_, described) =
        cloudformation(&router, region, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
    );

    let arn = state_machine_arn(region, "scheduled-poller");
    assert_eq!(
        outputs_of(&described).get("MachineArn"),
        Some(&arn),
        "case {case:?}: expected the output to be the machine's ARN {arn:?}, got {described}"
    );

    // EventBridge holds the rule's target as that very ARN, so a firing of the rule reaches
    // the machine the same stack created.
    let (status, targets) = events(
        &router,
        region,
        "ListTargetsByRule",
        &json!({ "Rule": "poller-schedule" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListTargetsByRule, got {status} with body {targets}"
    );
    let arns: Vec<String> = targets
        .get("Targets")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no 'Targets' array in {targets}"))
        .iter()
        .filter_map(|target| target.get("Arn").and_then(Value::as_str).map(str::to_string))
        .collect();
    assert_eq!(
        arns,
        vec![arn.clone()],
        "case {case:?}: expected the rule's one target to be the machine's ARN {arn:?}, got \
         {targets}"
    );

    // And Step Functions answers for it.
    let (status, machine) =
        sfn(&router, region, "DescribeStateMachine", &json!({ "stateMachineArn": arn })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the machine the rule targets to resolve, got {status} with body \
         {machine}"
    );
}

/// A stack whose template declares a state machine Step Functions REFUSES does not report a
/// resource nothing created: the stack fails with that service's own refusal, and nothing is
/// left behind under the ARN the stack would have published.
#[tokio::test]
async fn a_machine_the_owning_service_refuses_fails_the_stack() {
    let router = build_router();
    let case = "a definition with no terminal state";
    let region = "us-east-2";
    let stack_name = "sfn-bad-definition";
    // A loop with no terminal state: a definition Step Functions refuses with
    // `InvalidDefinition`, so the resource fails rather than being reported created.
    let definition = json!({
        "StartAt": "Spin",
        "States": { "Spin": { "Type": "Wait", "Seconds": 1, "Next": "Spin" } },
    })
    .to_string();
    let template = json!({
        "Resources": {
            "Machine": {
                "Type": "AWS::StepFunctions::StateMachine",
                "Properties": {
                    "StateMachineName": "never-ends",
                    "RoleArn": ROLE_ARN,
                    "DefinitionString": definition,
                },
            },
        },
    })
    .to_string();

    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;

    let (_, described) =
        cloudformation(&router, region, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("ROLLBACK_COMPLETE"),
        "case {case:?}: expected the failed create to be rolled back, got {described}"
    );
    let reason = opt_element(&described, "StackStatusReason").unwrap_or_default();
    assert!(
        reason.contains("Machine"),
        "case {case:?}: expected the failure to name the resource, got {reason:?}"
    );

    let (status, gone) = sfn(
        &router,
        region,
        "DescribeStateMachine",
        &json!({ "stateMachineArn": state_machine_arn(region, "never-ends") }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected nothing to answer for the ARN the stack would have published, \
         got {status} with body {gone}"
    );
}
