// SPDX-License-Identifier: CC0-1.0

//! What a change set previews when a revision's CONDITIONS move a logical id into or out of
//! the stack's resource set, end to end over the awsQuery wire.
//!
//! RULE: a change set compares the resource SET each revision declares, and a declaration's
//! `Condition` is what decides whether that revision declares it at all — so a logical id
//! whose condition holds only on the stack's side LEAVES the set and is previewed
//! `Action=Remove`, one whose condition holds only on the submitted side ENTERS it and is
//! previewed `Action=Add`, and a resource both revisions declare is previewed `Action=Modify`
//! whose `Replacement` is read off its details as the model documents them ("if the
//! `RequiresRecreation` field is `Always` and the `Evaluation` field is `Static`,
//! `Replacement` is `True`"), so a property that must take a different resource's identifier
//! reports `True` rather than `Conditional`. Executing such a plan carries it out: the
//! resource that entered the set is created, the one that left is taken away in the owning
//! service, and the dependents are recreated inside the resource that replaced their parent.
//!
//! The rows differ along what the rule generalises over: WHY the condition's value differs
//! between the two revisions (a parameter value the condition reads, or the condition
//! expression itself rewritten in the submitted template), WHICH direction the pair flips in,
//! the resource types under the conditions (an HTTP api whose children name it, and an Amazon
//! SNS topic an alarm reads an attribute off), and the other side of the rule — a revision
//! that flips no condition previews no `Add` and no `Remove` at all.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudformation_cloudformation::state::change_action;
use service_com_amazonaws_cloudformation_cloudformation::state::change_plan::{
    replacement, requires_recreation,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    change_set_status, execution_status, status,
};

use crate::apigateway_vpc_link_resources::apigateway;
use crate::change_set_lifecycle::settle_the_store;
use crate::harness::physical_resource_id;
use crate::stack_lifecycle::{
    call, create_stack, member, router_with_engine, xml_blocks, xml_text, ACCOUNT_ID,
};

/// The two values the `Phase` parameter of the api template takes: the phase that declares
/// the first api, and the one that declares the api replacing it.
const CORS_PHASE: &str = "cors";
const REPLACED_PHASE: &str = "replaced";

/// A template whose two `AWS::ApiGatewayV2::Api` declarations are guarded by conditions over
/// `Phase`, with the integration, the route and the stage inside the api naming whichever of
/// them that phase declares.
///
/// `cors_phase` and `replaced_phase` are the values the two conditions compare `Phase` with,
/// so a row can flip the pair by changing the PARAMETER (leaving these as they are) or by
/// changing the CONDITION EXPRESSIONS (swapping these while the parameter stays put).
fn api_template(description: &str, cors_phase: &str, replaced_phase: &str) -> String {
    let api_id = json!({
        "Fn::If": ["ReplacedPhase", { "Ref": "ReplacementApi" }, { "Ref": "CorsApi" }]
    });
    json!({
        "Parameters": { "Phase": { "Type": "String", "Default": CORS_PHASE } },
        "Conditions": {
            "CorsPhase": { "Fn::Equals": [{ "Ref": "Phase" }, cors_phase] },
            "ReplacedPhase": { "Fn::Equals": [{ "Ref": "Phase" }, replaced_phase] },
        },
        "Resources": {
            "CorsApi": {
                "Type": "AWS::ApiGatewayV2::Api",
                "Condition": "CorsPhase",
                "Properties": {
                    "Name": "condition-flip-cors-api",
                    "ProtocolType": "HTTP",
                    "Description": description,
                    "CorsConfiguration": {
                        "AllowOrigins": ["https://allowed.example.com"],
                        "AllowMethods": ["GET"],
                        "MaxAge": 300,
                    },
                },
            },
            "ReplacementApi": {
                "Type": "AWS::ApiGatewayV2::Api",
                "Condition": "ReplacedPhase",
                "Properties": {
                    "Name": "condition-flip-replacement-api",
                    "ProtocolType": "HTTP",
                    "Description": description,
                },
            },
            "EchoIntegration": {
                "Type": "AWS::ApiGatewayV2::Integration",
                "Properties": {
                    "ApiId": api_id,
                    "IntegrationType": "HTTP_PROXY",
                    "IntegrationMethod": "GET",
                    "IntegrationUri": "https://backend.example.com/a",
                    "PayloadFormatVersion": "1.0",
                },
            },
            "RouteA": {
                "Type": "AWS::ApiGatewayV2::Route",
                "Properties": {
                    "ApiId": api_id,
                    "RouteKey": "GET /a",
                    "Target": { "Fn::Join": ["/", ["integrations", { "Ref": "EchoIntegration" }]] },
                },
            },
            "DefaultStage": {
                "Type": "AWS::ApiGatewayV2::Stage",
                "Properties": {
                    "ApiId": api_id,
                    "StageName": "$default",
                    "AutoDeploy": true,
                },
            },
        },
        "Outputs": { "ApiId": { "Value": api_id } }
    })
    .to_string()
}

/// A template whose two `AWS::SNS::Topic` declarations are guarded by conditions over
/// `Phase`, with an alarm reading the topic ARN of whichever of them that phase declares: the
/// same rule over another type family, where the dependent reads an ATTRIBUTE off the
/// condition-guarded resource rather than naming it.
///
/// `first_phase`/`second_phase` are the values the two conditions compare `Phase` with, as in
/// [`api_template`].
fn topic_template(stack: &str, first_phase: &str, second_phase: &str) -> String {
    let topic_arn = json!({
        "Fn::If": [
            "SecondPhase",
            { "Fn::GetAtt": ["SecondTopic", "TopicArn"] },
            { "Fn::GetAtt": ["FirstTopic", "TopicArn"] },
        ]
    });
    json!({
        "Parameters": { "Phase": { "Type": "String", "Default": "first" } },
        "Conditions": {
            "FirstPhase": { "Fn::Equals": [{ "Ref": "Phase" }, first_phase] },
            "SecondPhase": { "Fn::Equals": [{ "Ref": "Phase" }, second_phase] },
        },
        "Resources": {
            "FirstTopic": {
                "Type": "AWS::SNS::Topic",
                "Condition": "FirstPhase",
                "Properties": { "TopicName": format!("{stack}-first") },
            },
            "SecondTopic": {
                "Type": "AWS::SNS::Topic",
                "Condition": "SecondPhase",
                "Properties": { "TopicName": format!("{stack}-second") },
            },
            "Reader": {
                "Type": "AWS::CloudWatch::Alarm",
                "Properties": {
                    "AlarmName": format!("{stack}-reader"),
                    "Tags": [{ "Key": "topic", "Value": topic_arn }],
                },
            },
        }
    })
    .to_string()
}

/// One `ResourceChange` of a preview, as a row states it and as the response carries it: the
/// action, the `Replacement` and the `(property, RequiresRecreation)` pairs of its details.
#[derive(Debug, PartialEq, Eq)]
struct Change {
    action: String,
    replacement: String,
    details: Vec<(String, String)>,
}

/// The expected form of one change, spelled the way a row reads.
fn change(action: &str, replacement: &str, details: &[(&str, &str)]) -> Change {
    Change {
        action: action.to_string(),
        replacement: replacement.to_string(),
        details: details
            .iter()
            .map(|(name, recreation)| ((*name).to_string(), (*recreation).to_string()))
            .collect(),
    }
}

/// Every `ResourceChange` a `DescribeChangeSet` answer carries, keyed by logical id.
fn previewed(body: &str) -> BTreeMap<String, Change> {
    xml_blocks(body, "ResourceChange")
        .iter()
        .map(|entry| {
            let details = xml_blocks(entry, "Details")
                .first()
                .map(|details| {
                    xml_blocks(details, "member")
                        .iter()
                        .map(|detail| {
                            (
                                xml_text(detail, "Name").unwrap_or_default(),
                                xml_text(detail, "RequiresRecreation").unwrap_or_default(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            (
                xml_text(entry, "LogicalResourceId").unwrap_or_default(),
                Change {
                    action: xml_text(entry, "Action").unwrap_or_default(),
                    replacement: xml_text(entry, "Replacement").unwrap_or_default(),
                    details,
                },
            )
        })
        .collect()
}

/// One row of the preview table: the revision the stack is created from, the revision the
/// change set submits, and the whole plan it must answer.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The template the stack is created from, and the `Phase` value it carries.
    created: (String, &'static str),
    /// The template the change set submits, and the `Phase` value it submits.
    submitted: (String, &'static str),
    /// Every change the preview must carry, keyed by logical id.
    plan: Vec<(&'static str, Change)>,
}

fn rows() -> Vec<Row> {
    vec![
        // The recorded case: the same template body, and a PARAMETER value that flips which
        // of the two api declarations the revision makes.
        Row {
            case: "a parameter value that moves the api a route names",
            stack: "flip-by-parameter",
            created: (
                api_template("the api", CORS_PHASE, REPLACED_PHASE),
                CORS_PHASE,
            ),
            submitted: (
                api_template("the api", CORS_PHASE, REPLACED_PHASE),
                REPLACED_PHASE,
            ),
            plan: vec![
                ("CorsApi", change(change_action::REMOVE, "", &[])),
                ("ReplacementApi", change(change_action::ADD, "", &[])),
                // Each resource that lives INSIDE the api names it by `ApiId`, which no
                // api of API Gateway v2 can be moved between: the property is recreated
                // `Always`, the plan knows the new value, so `Replacement` is `True`.
                (
                    "EchoIntegration",
                    change(
                        change_action::MODIFY,
                        replacement::TRUE,
                        &[("ApiId", requires_recreation::ALWAYS)],
                    ),
                ),
                (
                    "DefaultStage",
                    change(
                        change_action::MODIFY,
                        replacement::TRUE,
                        &[("ApiId", requires_recreation::ALWAYS)],
                    ),
                ),
                (
                    "RouteA",
                    change(
                        change_action::MODIFY,
                        replacement::TRUE,
                        &[
                            ("ApiId", requires_recreation::ALWAYS),
                            // The route's target names the integration, which this plan
                            // recreates: a changed value that never recreates the route.
                            ("Target", requires_recreation::NEVER),
                        ],
                    ),
                ),
            ],
        },
        // The same flip the OTHER way round: the stack stands on the replacement api and the
        // submitted revision declares the first one again.
        Row {
            case: "the same pair of conditions flipped back",
            stack: "flip-back-by-parameter",
            created: (
                api_template("the api", CORS_PHASE, REPLACED_PHASE),
                REPLACED_PHASE,
            ),
            submitted: (
                api_template("the api", CORS_PHASE, REPLACED_PHASE),
                CORS_PHASE,
            ),
            plan: vec![
                ("ReplacementApi", change(change_action::REMOVE, "", &[])),
                ("CorsApi", change(change_action::ADD, "", &[])),
                (
                    "EchoIntegration",
                    change(
                        change_action::MODIFY,
                        replacement::TRUE,
                        &[("ApiId", requires_recreation::ALWAYS)],
                    ),
                ),
                (
                    "DefaultStage",
                    change(
                        change_action::MODIFY,
                        replacement::TRUE,
                        &[("ApiId", requires_recreation::ALWAYS)],
                    ),
                ),
                (
                    "RouteA",
                    change(
                        change_action::MODIFY,
                        replacement::TRUE,
                        &[
                            ("ApiId", requires_recreation::ALWAYS),
                            ("Target", requires_recreation::NEVER),
                        ],
                    ),
                ),
            ],
        },
        // The parameter value is the SAME on both sides; what differs is the condition
        // EXPRESSION the submitted template declares, so the same pair of logical ids leaves
        // and enters the resource set.
        Row {
            case: "a condition expression rewritten while the parameter stays put",
            stack: "flip-by-condition",
            created: (topic_template("flip-by-condition", "first", "second"), "first"),
            submitted: (topic_template("flip-by-condition", "second", "first"), "first"),
            plan: vec![
                ("FirstTopic", change(change_action::REMOVE, "", &[])),
                ("SecondTopic", change(change_action::ADD, "", &[])),
                // The alarm reads the topic ARN of whichever topic the revision declares:
                // a changed tag value, which never recreates an alarm.
                (
                    "Reader",
                    change(
                        change_action::MODIFY,
                        replacement::FALSE,
                        &[("Tags", requires_recreation::NEVER)],
                    ),
                ),
            ],
        },
        // The other side of the rule: a revision that flips NO condition previews neither an
        // `Add` nor a `Remove`, however many condition-guarded declarations the template
        // carries — only the property it really edits.
        Row {
            case: "a revision that edits a property and flips no condition",
            stack: "flip-nothing",
            created: (
                api_template("the api", CORS_PHASE, REPLACED_PHASE),
                CORS_PHASE,
            ),
            submitted: (
                api_template("the api, described again", CORS_PHASE, REPLACED_PHASE),
                CORS_PHASE,
            ),
            plan: vec![(
                "CorsApi",
                change(
                    change_action::MODIFY,
                    replacement::FALSE,
                    &[("Description", requires_recreation::NEVER)],
                ),
            )],
        },
    ]
}

/// The members that submit one `Phase` value.
fn phase(value: &str) -> Vec<(String, String)> {
    vec![
        member("Parameters.member.1.ParameterKey", "Phase"),
        member("Parameters.member.1.ParameterValue", value),
    ]
}

/// Settle the store until `stack` reports a status no operation is in flight in, and answer
/// that status with the reason beside it.
///
/// `settle_the_store` makes every transition the store owes due NOW and lets a read
/// materialise it, so a case waits on the emulator's own settling rather than on the wall
/// clock.
async fn settled(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    stack: &str,
) -> (String, String) {
    // Generous: the condition under test is the status the stack SETTLES on, never how long
    // the emulator takes to get there.
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut seen = Vec::new();
    loop {
        settle_the_store(router, engine).await;
        let (code, body) = call(router, "DescribeStacks", &[member("StackName", stack)]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected DescribeStacks to answer 200, got {code} {body}"
        );
        let reported = xml_text(&body, "StackStatus").unwrap_or_default();
        if !reported.is_empty() && !reported.ends_with("_IN_PROGRESS") {
            return (
                reported,
                xml_text(&body, "StackStatusReason").unwrap_or_default(),
            );
        }
        if seen.last().map(String::as_str) != Some(reported.as_str()) {
            seen.push(reported);
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {stack} to settle, saw {seen:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// `DescribeChangeSet` for a change set whose creation has settled.
async fn describe_change_set(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    stack: &str,
    change_set: &str,
) -> String {
    settle_the_store(router, engine).await;
    let (code, body) = call(
        router,
        "DescribeChangeSet",
        &[
            member("StackName", stack),
            member("ChangeSetName", change_set),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected DescribeChangeSet to answer 200, got {code} {body}"
    );
    body
}

#[tokio::test]
async fn a_condition_that_moves_a_logical_id_is_previewed_as_a_remove_and_an_add() {
    for row in rows() {
        let case = row.case;
        let stack = row.stack;
        // One engine per row: the topic rows name their topics, and a service holds one
        // resource per name.
        let (router, engine) = router_with_engine();

        let (code, body) = create_stack(&router, stack, &row.created.0, &phase(row.created.1)).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected CreateStack to be accepted, got {code} {body}"
        );
        let (reported, reason) = settled(&router, &engine, case, stack).await;
        assert_eq!(
            reported,
            status::CREATE_COMPLETE,
            "case {case:?}: expected the create to complete, got {reported:?} ({reason:?})"
        );

        let mut members = vec![
            member("StackName", stack),
            member("ChangeSetName", "flip"),
            member("TemplateBody", &row.submitted.0),
        ];
        members.extend(phase(row.submitted.1));
        let (code, body) = call(&router, "CreateChangeSet", &members).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected CreateChangeSet to be accepted, got {code} {body}"
        );

        let body = describe_change_set(&router, &engine, case, stack, "flip").await;
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(change_set_status::CREATE_COMPLETE),
            "case {case:?}: expected the change set to be computed, got {body}"
        );
        let expected: BTreeMap<String, Change> = row
            .plan
            .into_iter()
            .map(|(logical_id, change)| (logical_id.to_string(), change))
            .collect();
        let actual = previewed(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the preview {expected:?}, got {actual:?}"
        );
    }
}

/// The plan is not only previewed but carried out: executing it creates the api that entered
/// the resource set, recreates the route inside it, and takes the api that left away in API
/// Gateway v2 itself — which is what makes the old identifier answer for nothing.
#[tokio::test]
async fn executing_the_flip_creates_the_api_that_entered_and_removes_the_one_that_left() {
    let case = "a route repointed at the api a condition brought into the set";
    let stack = "flip-executed";
    let (router, engine) = router_with_engine();
    let template = api_template("the api", CORS_PHASE, REPLACED_PHASE);

    let (code, body) = create_stack(&router, stack, &template, &phase(CORS_PHASE)).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected CreateStack to be accepted, got {code} {body}"
    );
    let (reported, reason) = settled(&router, &engine, case, stack).await;
    assert_eq!(
        reported,
        status::CREATE_COMPLETE,
        "case {case:?}: expected the create to complete, got {reported:?} ({reason:?})"
    );
    let before = published_api(&router, case, stack).await;
    let route_before = physical_resource_id(&router, case, stack, "RouteA").await;
    // The api the stack created really is an api of API Gateway v2, read through that
    // service's own wire.
    let (code, held) = apigateway(&router, "GET", &format!("/v2/apis/{before}")).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected GetApi({before}) to answer 200, got {code} with {held}"
    );

    let mut members = vec![
        member("StackName", stack),
        member("ChangeSetName", "flip"),
        member("TemplateBody", &template),
    ];
    members.extend(phase(REPLACED_PHASE));
    let (code, body) = call(&router, "CreateChangeSet", &members).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected CreateChangeSet to be accepted, got {code} {body}"
    );
    let described = describe_change_set(&router, &engine, case, stack, "flip").await;
    assert_eq!(
        xml_text(&described, "ExecutionStatus").as_deref(),
        Some(execution_status::AVAILABLE),
        "case {case:?}: expected the change set to be executable, got {described}"
    );

    let (code, body) = call(
        &router,
        "ExecuteChangeSet",
        &[
            member("StackName", stack),
            member("ChangeSetName", "flip"),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected ExecuteChangeSet to be accepted, got {code} {body}"
    );
    let (reported, reason) = settled(&router, &engine, case, stack).await;
    assert_eq!(
        reported,
        status::UPDATE_COMPLETE,
        "case {case:?}: expected the update to complete, got {reported:?} ({reason:?})"
    );

    let after = published_api(&router, case, stack).await;
    assert_ne!(
        after, before,
        "case {case:?}: expected the api that entered the resource set to publish an ApiId of \
         its own, got {after:?}"
    );
    let route_after = physical_resource_id(&router, case, stack, "RouteA").await;
    assert_ne!(
        route_after, route_before,
        "case {case:?}: expected the route to be created inside the api that replaced the one \
         it named, got {route_after:?}"
    );
    // ...and that route is a route of the api the stack now publishes.
    let (code, held) = apigateway(
        &router,
        "GET",
        &format!("/v2/apis/{after}/routes/{route_after}"),
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected GetRoute({after}, {route_after}) to answer 200, got {code} \
         with {held}"
    );

    // The api that left the resource set is taken away in the update's cleanup, which runs
    // after the replacement is in force — so the old identifier is polled until API Gateway
    // v2 no longer answers for it, and the refusal is that service's own api-level one.
    let deadline = Instant::now() + Duration::from_secs(120);
    let last = loop {
        let answered = apigateway(&router, "GET", &format!("/v2/apis/{before}")).await;
        if answered.0 != 200 {
            break answered;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the api {before} that left the resource set to be gone \
             from API Gateway v2, it still answers {answered:?}"
        );
        settle_the_store(&router, &engine).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let (code, refused) = last;
    assert_eq!(
        code, 404,
        "case {case:?}: expected GetApi({before}) to be refused 404, got {code} with {refused}"
    );
    assert_eq!(
        refused.get("message").and_then(Value::as_str),
        Some(format!("Invalid API identifier specified {ACCOUNT_ID}:{before}").as_str()),
        "case {case:?}: expected API Gateway v2's own api-level refusal, got {refused}"
    );
}

/// The `ApiId` the stack publishes as its one output.
async fn published_api(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let (code, body) = call(router, "DescribeStacks", &[member("StackName", stack)]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected DescribeStacks to answer 200, got {code} {body}"
    );
    xml_text(&body, "OutputValue").unwrap_or_default()
}
