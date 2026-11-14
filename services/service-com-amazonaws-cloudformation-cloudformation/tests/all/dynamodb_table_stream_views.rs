// SPDX-License-Identifier: CC0-1.0

//! The DynamoDB Streams stream an `AWS::DynamoDB::Table` declares, as the TYPE declares it,
//! and the stack output over the ARN Amazon DynamoDB minted for it.
//!
//! RULE: a resource type's property whose shape or spelling differs from the owning API's
//! member of the same name is TRANSLATED into that member before the call rather than passed
//! through — `AWS::DynamoDB::Table`'s `StreamSpecification` states what the stream captures
//! and declaring the property is what asks for a stream, while the API's own specification
//! carries a required flag beside the view type — and a change to what an ENABLED stream
//! captures, which no single call of that service expresses, is applied by turning the stream
//! in force off and a stream capturing what the declaration now asks for on, so the table's
//! stream ARN is a new one and every `Fn::GetAtt` over it follows.
//!
//! The rows differ along what the rule generalises over: each `StreamViewType` the model
//! declares, both spellings of the declaration (the type's own, and the API's with the flag
//! stated), and — for the change — which view type the declaration moves from and to.
//!
//! Every case drives the real request path for both services through one router: the
//! CloudFormation calls over the awsQuery wire and the DynamoDB reads as a client's SDK makes
//! them, so what is asserted is what a client that deploys a streamed table by template and
//! then reads it observes.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::dynamodb_table_resources::{describe_table, failure_reasons};
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, output_value, router, xml_text,
};

/// How often a case asks the stack whether it has settled, and how long it goes on asking.
///
/// The budget covers the spans DynamoDB owes a new table and each change of one several times
/// over — a view-type change is two changes of the table, one after the other — so a loaded
/// machine is slower than those spans without the rule being any different, and a case that
/// never settles reports rather than running on.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const POLLS: usize = 600;

/// The `StreamViewType` values the model declares, in the order `StreamSpecification`'s own
/// enum states them (`smithy_query(query="shape",shape="StreamViewType")`).
const STREAM_VIEW_TYPES: &[&str] = &["NEW_IMAGE", "OLD_IMAGE", "NEW_AND_OLD_IMAGES", "KEYS_ONLY"];

/// The template a case deploys: one table whose stream captures `view_type`, declared in the
/// form `declared_flag` asks for, and the outputs a client reads the table through.
///
/// `declared_flag` is the API's own `StreamEnabled`, which the TYPE does not publish at all:
/// a template written against the type states the view type alone, and one written against
/// the API's spelling states the flag too. Both ask for the same stream.
fn template(view_type: &str, declared_flag: bool) -> String {
    let mut specification = json!({"StreamViewType": view_type});
    if declared_flag {
        specification
            .as_object_mut()
            .expect("the specification is an object")
            .insert("StreamEnabled".to_string(), json!(true));
    }
    json!({
        "Resources": {"Table": {"Type": "AWS::DynamoDB::Table", "Properties": {
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
            "StreamSpecification": specification,
        }}},
        "Outputs": {
            "TableName": {"Value": {"Ref": "Table"}},
            "StreamArn": {"Value": {"Fn::GetAtt": ["Table", "StreamArn"]}},
        },
    })
    .to_string()
}

/// Poll `stack` until it reports a status the deployment has settled on, and answer that
/// status with the `DescribeStacks` answer it was read from.
async fn settled(router: &MultiServiceRouter, stack: &str) -> (String, String) {
    for _ in 0..POLLS {
        let (code, body) = describe_stack(router, stack).await;
        assert_eq!(
            code, 200,
            "DescribeStacks {stack:?}: expected HTTP 200, got {code} with body {body}"
        );
        let status = xml_text(&body, "StackStatus").unwrap_or_default();
        if !status.ends_with("_IN_PROGRESS") {
            return (status, body);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let (_, body) = describe_stack(router, stack).await;
    (
        xml_text(&body, "StackStatus").unwrap_or_default(),
        body.clone(),
    )
}

/// What DynamoDB reports for the table `name`: the stream ARN it last minted, and the view
/// type its stream captures in.
async fn reported_stream(
    router: &MultiServiceRouter,
    name: &str,
) -> (Option<String>, Option<String>) {
    let Some(table) = describe_table(router, name).await else {
        return (None, None);
    };
    let arn = table
        .get("LatestStreamArn")
        .and_then(Value::as_str)
        .map(str::to_string);
    let view_type = table
        .get("StreamSpecification")
        .and_then(|specification| specification.get("StreamViewType"))
        .and_then(Value::as_str)
        .map(str::to_string);
    (arn, view_type)
}

/// A table declared with the type's own `StreamSpecification` is created WITH its stream, and
/// the stack's output over `Fn::GetAtt … StreamArn` carries the ARN DynamoDB minted for it.
#[tokio::test]
async fn a_declared_stream_publishes_the_minted_arn_for_every_view_type() {
    // One row per view type the model declares, each in the type's own form; the last row
    // states the API's flag as well, so the rule is not read off one spelling.
    let mut rows: Vec<(String, &str, bool)> = STREAM_VIEW_TYPES
        .iter()
        .map(|view_type| ((*view_type).to_string(), *view_type, false))
        .collect();
    rows.push((
        "the API's own spelling".to_string(),
        "NEW_AND_OLD_IMAGES",
        true,
    ));
    for (index, (case, view_type, declared_flag)) in rows.iter().enumerate() {
        let router = router();
        let stack = format!("stream-view-{index}");
        let (code, body) = create_stack(&router, &stack, &template(view_type, *declared_flag), &[])
            .await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {code} {body}"
        );
        let (status, described) = settled(&router, &stack).await;
        assert_eq!(
            status, "CREATE_COMPLETE",
            "case {case:?}: expected the streamed table stack to deploy, got {status:?} \
             because {:?}",
            failure_reasons(&router, &stack).await
        );

        let published = output_value(&described, "StreamArn");
        assert!(
            published.as_deref().is_some_and(|arn| !arn.is_empty()),
            "case {case:?}: the StreamArn output carries no value at all ({described})"
        );
        let name = output_value(&described, "TableName").unwrap_or_default();
        let (minted, captured) = reported_stream(&router, &name).await;
        assert_eq!(
            published, minted,
            "case {case:?}: expected the output to be the stream ARN DynamoDB minted, got \
             {published:?} against {minted:?}"
        );
        assert_eq!(
            captured.as_deref(),
            Some(*view_type),
            "case {case:?}: expected the table to capture in {view_type:?}, got {captured:?}"
        );
    }
}

/// Changing what the stream captures mints a NEW stream, and the stack's output follows it:
/// no call of that service changes an enabled stream's view type, so the declaration is
/// applied by turning the stream in force off and the declared one on.
#[tokio::test]
async fn a_view_type_change_mints_a_new_stream_and_the_output_follows() {
    // Each row moves between a different pair of the declared view types, in the type's own
    // form on both sides.
    let rows: &[(&str, &str, &str)] = &[
        ("NEW_IMAGE -> OLD_IMAGE", "NEW_IMAGE", "OLD_IMAGE"),
        (
            "OLD_IMAGE -> NEW_AND_OLD_IMAGES",
            "OLD_IMAGE",
            "NEW_AND_OLD_IMAGES",
        ),
        (
            "NEW_AND_OLD_IMAGES -> KEYS_ONLY",
            "NEW_AND_OLD_IMAGES",
            "KEYS_ONLY",
        ),
    ];
    for (index, (case, before, after)) in rows.iter().enumerate() {
        let router = router();
        let stack = format!("stream-view-change-{index}");
        let (code, body) = create_stack(&router, &stack, &template(before, false), &[]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {code} {body}"
        );
        let (status, described) = settled(&router, &stack).await;
        assert_eq!(
            status, "CREATE_COMPLETE",
            "case {case:?}: expected the streamed table stack to deploy, got {status:?} \
             because {:?}",
            failure_reasons(&router, &stack).await
        );
        let name = output_value(&described, "TableName").unwrap_or_default();
        let first = output_value(&described, "StreamArn");
        assert!(
            first.is_some(),
            "case {case:?}: expected the deployed stack to publish a stream ARN, got none \
             ({described})"
        );

        let (code, body) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", &stack),
                member("TemplateBody", &template(after, false)),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected HTTP 200 from UpdateStack, got {code} {body}"
        );
        let (status, described) = settled(&router, &stack).await;
        assert_eq!(
            status, "UPDATE_COMPLETE",
            "case {case:?}: expected the view-type change to be applied, got {status:?} \
             because {:?}",
            failure_reasons(&router, &stack).await
        );

        // The table captures what the revision declared, and the stream it captures into is a
        // new one — a stream's identity is its ARN, and the one in force was turned off.
        let (minted, captured) = reported_stream(&router, &name).await;
        assert_eq!(
            captured.as_deref(),
            Some(*after),
            "case {case:?}: expected the table to capture in {after:?} after the change, got \
             {captured:?}"
        );
        let published = output_value(&described, "StreamArn");
        assert!(
            published.as_deref().is_some_and(|arn| !arn.is_empty()),
            "case {case:?}: the StreamArn output carries no value after the change \
             ({described})"
        );
        assert_eq!(
            published, minted,
            "case {case:?}: expected the output to be the stream ARN DynamoDB now reports, \
             got {published:?} against {minted:?}"
        );
        assert_ne!(
            published, first,
            "case {case:?}: expected a new stream ARN for the stream the change minted, got \
             the one the create published ({published:?})"
        );
    }
}

/// A member of the type's `StreamSpecification` this emulator puts into force nowhere fails
/// the resource rather than being dropped from a stack that reports success: the stream's own
/// resource-based policy is a call on the stream nothing here makes.
#[tokio::test]
async fn an_unapplied_stream_member_fails_the_resource() {
    let router = router();
    let case = "a resource policy over the stream";
    let stack = "stream-unapplied-member";
    let body = json!({
        "Resources": {"Table": {"Type": "AWS::DynamoDB::Table", "Properties": {
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
            "StreamSpecification": {
                "StreamViewType": "KEYS_ONLY",
                "ResourcePolicy": {"PolicyDocument": {"Version": "2012-10-17"}},
            },
        }}},
    })
    .to_string();
    let (code, answer) = create_stack(&router, stack, &body, &[]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {code} {answer}"
    );
    let (status, described) = settled(&router, stack).await;
    assert_eq!(
        status, "ROLLBACK_COMPLETE",
        "case {case:?}: expected the failed create to be rolled back rather than report a \
         stream configured with less than the declaration asked for, got {status:?} in \
         {described}"
    );
    let reasons = failure_reasons(&router, stack).await;
    assert!(
        reasons
            .iter()
            .any(|reason| reason.contains("StreamSpecification.ResourcePolicy")),
        "case {case:?}: expected a failure naming the member, got {reasons:?}"
    );
}
