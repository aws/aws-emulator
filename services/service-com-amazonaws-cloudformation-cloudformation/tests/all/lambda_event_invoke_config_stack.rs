// SPDX-License-Identifier: CC0-1.0

//! A stack that declares the asynchronous-invocation configuration of a function:
//! `AWS::Lambda::EventInvokeConfig`.
//!
//! RULE: a resource type a template declares is CREATED in the service that owns it, so what
//! that service reports about it is what the declaration asked for — for this type, the
//! configuration `PutFunctionEventInvokeConfig` writes on the qualifier the declaration names
//! ("Configures options for asynchronous invocation on a function, version, or alias"), read
//! back through `GetFunctionEventInvokeConfig`. A declaration that is merely recorded by the
//! stack is not a configuration: nothing would ever deliver an invocation record to the
//! destinations it names.
//!
//! The rows differ along what the rule generalises over: which destination slots the
//! declaration states (`OnFailure` alone, `OnSuccess` alone, both), and the retry count and
//! event age it states beside them — each at a published bound of its own member
//! (`MaximumRetryAttempts` `@range(min: 0, max: 2)`, `MaximumEventAgeInSeconds`
//! `@range(min: 60, max: 21600)`) and absent altogether.
//!
//! Beside the table, two tests over the rest of that configuration's life: a revision of the
//! template is applied to the configuration in force, and deleting the stack takes it away —
//! so a destination the template no longer declares stops receiving records.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    acknowledging_iam, call, create_stack, describe_stack, member, router,
    wait_for_the_transition, xml_text, REGION,
};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// The function every stack below declares, which is the one the configuration is held on.
const FUNCTION: &str = "async-destination-fn";

/// The qualifier every declaration names: the unpublished version, as the model spells it.
const QUALIFIER: &str = "$LATEST";

/// The error code AWS Lambda answers for a configuration it does not hold
/// (`ResourceNotFoundException`, httpError 404), which is what says the delete took it away.
const NOT_FOUND: &str = "ResourceNotFoundException";

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
    let status = response.status().as_u16();
    let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, answered)
}

/// `GetFunctionEventInvokeConfig` on the qualifier every declaration names: the status AWS
/// Lambda answered and the document it reported.
async fn held_configuration(router: &MultiServiceRouter) -> (u16, Value) {
    lambda(
        router,
        "GET",
        &format!("/2019-09-25/functions/{FUNCTION}/event-invoke-config?Qualifier={QUALIFIER}"),
    )
    .await
}

/// A template declaring the function, two topics for a configuration to point at, and ONE
/// `AWS::Lambda::EventInvokeConfig` whose properties are `configured`.
///
/// The function's role is granted `sns:Publish`, because Lambda delivers an invocation record
/// ACTING AS that role and refuses a configuration naming a destination the role may not
/// write — and the configuration is declared as depending on that grant, so the grant is in
/// place before the configuration is put.
fn template(configured: &str) -> String {
    format!(
        r#"{{"Resources":{{
              "HandlerRole":{{"Type":"AWS::IAM::Role","Properties":{{
                 "AssumeRolePolicyDocument":{{"Version":"2012-10-17","Statement":[{{
                   "Effect":"Allow","Principal":{{"Service":"lambda.amazonaws.com"}},
                   "Action":"sts:AssumeRole"}}]}}}}}},
              "HandlerPolicy":{{"Type":"AWS::IAM::RolePolicy","Properties":{{
                 "PolicyName":"publish-invocation-records",
                 "RoleName":{{"Ref":"HandlerRole"}},
                 "PolicyDocument":{{"Version":"2012-10-17","Statement":[{{
                   "Effect":"Allow","Action":"sns:Publish","Resource":"*"}}]}}}}}},
              "Handler":{{"Type":"AWS::Lambda::Function","Properties":{{
                 "FunctionName":"{FUNCTION}","Runtime":"python3.12",
                 "Handler":"index.handler",
                 "Role":{{"Fn::GetAtt":["HandlerRole","Arn"]}},
                 "Code":{{"ZipFile":"def handler(event, context):\n    return {{}}\n"}}}}}},
              "Succeeded":{{"Type":"AWS::SNS::Topic","Properties":{{
                 "TopicName":"async-destination-succeeded"}}}},
              "Failed":{{"Type":"AWS::SNS::Topic","Properties":{{
                 "TopicName":"async-destination-failed"}}}},
              "Spare":{{"Type":"AWS::SNS::Topic","Properties":{{
                 "TopicName":"async-destination-spare"}}}},
              "AsyncConfig":{{"Type":"AWS::Lambda::EventInvokeConfig",
                 "DependsOn":["HandlerPolicy"],"Properties":{{
                 "FunctionName":{{"Ref":"Handler"}},"Qualifier":"{QUALIFIER}",{configured}}}}}}}}}"#
    )
}

/// `CreateStack` of `body`, waited out and asserted to have completed.
async fn given_stack(case: &str, router: &MultiServiceRouter, stack: &str, body: &str) {
    let (code, answered) = create_stack(router, stack, body, &acknowledging_iam(&[])).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected CreateStack to be accepted, got {code} {answered}"
    );
    wait_for_the_transition().await;
    let (_, described) = describe_stack(router, stack).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "case {case:?}: expected the create to complete, got {described}"
    );
}

/// `UpdateStack` of `body`, waited out; answers the status the stack reports and its reason.
async fn deployed_revision(
    case: &str,
    router: &MultiServiceRouter,
    stack: &str,
    body: &str,
) -> (String, String) {
    let (code, answered) = call(
        router,
        "UpdateStack",
        &acknowledging_iam(&[member("StackName", stack), member("TemplateBody", body)]),
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected UpdateStack to be accepted, got {code} {answered}"
    );
    wait_for_the_transition().await;
    let (_, described) = describe_stack(router, stack).await;
    (
        xml_text(&described, "StackStatus").unwrap_or_default(),
        xml_text(&described, "StackStatusReason").unwrap_or_default(),
    )
}

/// The ARN of the topic `name` names, as every ARN this emulator mints spells one — the value
/// a `Ref` to an `AWS::SNS::Topic` resolves to, computed here rather than read back out of
/// the configuration under test.
fn topic_arn(name: &str) -> String {
    format!("arn:aws:sns:{REGION}:000000000000:{name}")
}

/// One row of the declaration table: what the resource declares, and what AWS Lambda must
/// then report for the configuration.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The properties the resource declares beside the function and the qualifier.
    declared: &'static str,
    /// The topic the `OnSuccess` slot of the configuration must point at, if any.
    on_success: Option<&'static str>,
    /// The topic the `OnFailure` slot must point at, if any.
    on_failure: Option<&'static str>,
    /// The `MaximumRetryAttempts` it must carry, if any.
    retries: Option<i64>,
    /// The `MaximumEventAgeInSeconds` it must carry, if any.
    age: Option<i64>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a failure destination and no retries",
            stack: "async-config-on-failure",
            declared: r#""MaximumRetryAttempts":0,
                 "DestinationConfig":{"OnFailure":{"Destination":{"Ref":"Failed"}}}"#,
            on_success: None,
            on_failure: Some("async-destination-failed"),
            retries: Some(0),
            age: None,
        },
        Row {
            case: "a success destination and the shortest event age the member admits",
            stack: "async-config-on-success",
            declared: r#""MaximumEventAgeInSeconds":60,
                 "DestinationConfig":{"OnSuccess":{"Destination":{"Ref":"Succeeded"}}}"#,
            on_success: Some("async-destination-succeeded"),
            on_failure: None,
            retries: None,
            age: Some(60),
        },
        Row {
            case: "both destinations, with each bound at the top of its range",
            stack: "async-config-both",
            declared: r#""MaximumRetryAttempts":2,"MaximumEventAgeInSeconds":21600,
                 "DestinationConfig":{
                   "OnSuccess":{"Destination":{"Ref":"Succeeded"}},
                   "OnFailure":{"Destination":{"Ref":"Failed"}}}"#,
            on_success: Some("async-destination-succeeded"),
            on_failure: Some("async-destination-failed"),
            retries: Some(2),
            age: Some(21_600),
        },
        Row {
            case: "a destination alone, with neither bound declared",
            stack: "async-config-bare",
            declared: r#""DestinationConfig":{"OnFailure":{"Destination":{"Ref":"Failed"}}}"#,
            on_success: None,
            on_failure: Some("async-destination-failed"),
            retries: None,
            age: None,
        },
    ]
}

/// The destination one slot of the reported configuration points at, or `None` for a slot it
/// names none for.
fn slot_destination(held: &Value, slot: &str) -> Option<String> {
    held.pointer(&format!("/DestinationConfig/{slot}/Destination"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[tokio::test]
async fn a_declared_asynchronous_invocation_configuration_is_put_in_lambda() {
    for row in rows() {
        let Row {
            case,
            stack,
            declared,
            on_success,
            on_failure,
            retries,
            age,
        } = row;
        let router = router();
        given_stack(case, &router, stack, &template(declared)).await;

        let (code, held) = held_configuration(&router).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected AWS Lambda to report the configuration the stack \
             declared, got {code} {held}"
        );
        for (slot, expected) in [("OnSuccess", on_success), ("OnFailure", on_failure)] {
            let wanted = expected.map(topic_arn);
            let answered = slot_destination(&held, slot);
            assert_eq!(
                answered, wanted,
                "case {case:?}: expected the {slot} destination {wanted:?}, got {held}"
            );
        }
        assert_eq!(
            held.get("MaximumRetryAttempts").and_then(Value::as_i64),
            retries,
            "case {case:?}: expected MaximumRetryAttempts {retries:?}, got {held}"
        );
        assert_eq!(
            held.get("MaximumEventAgeInSeconds").and_then(Value::as_i64),
            age,
            "case {case:?}: expected MaximumEventAgeInSeconds {age:?}, got {held}"
        );
    }
}

#[tokio::test]
async fn a_revised_declaration_is_applied_to_the_configuration_in_force() {
    let case = "the destination the configuration points at, revised";
    let stack = "async-config-revised";
    let router = router();
    let created = r#""MaximumRetryAttempts":0,
         "DestinationConfig":{"OnFailure":{"Destination":{"Ref":"Failed"}}}"#;
    given_stack(case, &router, stack, &template(created)).await;

    // The revision points the configuration at the other topic the template declares and
    // raises the retry count: `PutFunctionEventInvokeConfig` writes the whole configuration of
    // a qualifier, so what is in force afterwards is exactly what the revision declared.
    let revised = r#""MaximumRetryAttempts":2,
         "DestinationConfig":{"OnSuccess":{"Destination":{"Ref":"Spare"}}}"#;
    let (reported, reason) = deployed_revision(case, &router, stack, &template(revised)).await;
    assert_eq!(
        reported,
        status::UPDATE_COMPLETE,
        "case {case:?}: expected the revision to complete, got {reported:?} ({reason:?})"
    );

    let (code, held) = held_configuration(&router).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected the configuration to still be held, got {code} {held}"
    );
    assert_eq!(
        slot_destination(&held, "OnSuccess"),
        Some(topic_arn("async-destination-spare")),
        "case {case:?}: expected the revised destination to be the one in force, got {held}"
    );
    assert_eq!(
        slot_destination(&held, "OnFailure"),
        None,
        "case {case:?}: expected the slot the revision dropped to name no destination, got \
         {held}"
    );
    assert_eq!(
        held.get("MaximumRetryAttempts").and_then(Value::as_i64),
        Some(2),
        "case {case:?}: expected the revised retry count to be in force, got {held}"
    );
}

#[tokio::test]
async fn deleting_the_stack_takes_the_configuration_away() {
    let case = "the stack that declared the configuration, deleted";
    let stack = "async-config-deleted";
    let router = router();
    let declared = r#""DestinationConfig":{"OnFailure":{"Destination":{"Ref":"Failed"}}}"#;
    given_stack(case, &router, stack, &template(declared)).await;
    let (code, _) = held_configuration(&router).await;
    assert_eq!(
        code, 200,
        "case {case:?}: the configuration must be in force before the delete"
    );

    let (code, answered) = call(&router, "DeleteStack", &[member("StackName", stack)]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected DeleteStack to be accepted, got {code} {answered}"
    );
    wait_for_the_transition().await;

    let (code, held) = held_configuration(&router).await;
    assert_eq!(
        code, 404,
        "case {case:?}: expected the configuration to be gone, got {code} {held}"
    );
    let reported = held
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        reported.ends_with(NOT_FOUND),
        "case {case:?}: expected the refusal {NOT_FOUND}, got {reported:?} in {held}"
    );
}
