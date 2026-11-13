// SPDX-License-Identifier: CC0-1.0

//! The template pipeline and the events it causes, end to end over the awsQuery wire.
//!
//! Three rules are asserted here, each over a TABLE of inputs rather than one recorded
//! request:
//!
//! 1. `GetTemplate` serves the stage the request asks for: `Original` is the submitted
//!    body byte for byte, `Processed` is that body with its declared transforms applied
//!    (and the `Transform` member gone). A transform this emulator cannot apply — a macro
//!    backed by a Lambda function — has no processed stage, and the read says so with 501
//!    instead of answering the unprocessed document. A CHANGE SET is planned from, serves
//!    and deploys that same stage, so the plan a caller reviews and the deployment its
//!    execution carries out are one document.
//! 2. A template that declares a transform may only be deployed by a request that
//!    acknowledges `CAPABILITY_AUTO_EXPAND`.
//! 3. A stack operation reports an event for every RESOURCE it acts on, between its own
//!    opening and closing events, each carrying that resource's type, logical id and
//!    properties — and every event of one operation carries that operation's client
//!    request token.
//! 4. The expansion is BOUNDED: a template whose directives this service cannot resolve,
//!    that nests deeper than the expander descends, or whose loops would expand past the
//!    expansion ceiling has no processed stage (501), and one that would deploy more than
//!    the documented per-stack resource quota is refused.
//!
//! The wire harness itself — the router, the form-encoded request, the XML readers — is
//! the one `stack_lifecycle.rs` exports, the way `error_envelope.rs` reuses it: this suite
//! adds only the helpers its own rules need.

use service_com_amazonaws_cloudformation_cloudformation::state::{
    change_set_status, execution_status, status, template, CAPABILITY_AUTO_EXPAND,
    MAX_EVENT_PROPERTIES_BYTES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::change_set_lifecycle::{
    create_change_set, execute_change_set, settle_the_store, settled_change_set,
};
use crate::harness::{
    call, create_stack, get_template, member, router, router_with_engine, wait_for_the_transition,
    xml_text, xml_texts,
};
use crate::stack_lifecycle::{acknowledging_iam, describe_stack, iam};

/// The resource statuses one stack-level transition reports, as the model's
/// `ResourceStatus` spells them.
const CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";
const UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";
const DELETE_IN_PROGRESS: &str = "DELETE_IN_PROGRESS";
const DELETE_COMPLETE: &str = "DELETE_COMPLETE";

/// The resource type CloudFormation reports on a stack's own event.
const STACK_RESOURCE_TYPE: &str = "AWS::CloudFormation::Stack";

/// The reason CloudFormation puts on a resource's opening create event, spelled here as
/// the wire spells it rather than taken from the crate under test, so a changed message is
/// a failing assertion.
const RESOURCE_CREATION_INITIATED: &str = "Resource creation Initiated";

/// The terminal statuses an operation this service cannot carry out ends in.
const CREATE_FAILED: &str = "CREATE_FAILED";
const UPDATE_FAILED: &str = "UPDATE_FAILED";

/// Assert the stack reports the terminal status an operation this service could not carry
/// out leaves it in, with a reason that says why.
async fn assert_failed_with_a_reason(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    status: &str,
) {
    let (code, body) = describe_stack(router, stack_name).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected DescribeStacks to succeed, got {code} {body}"
    );
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some(status),
        "case {case:?}: an operation this service cannot carry out ends in {status}, got {body}"
    );
    let reason = xml_text(&body, "StackStatusReason").unwrap_or_default();
    assert!(
        !reason.is_empty(),
        "case {case:?}: the failure names what this service could not do, got {body}"
    );
}

/// `CreateStack` acknowledging the macro capability, for a template that declares a
/// transform.
async fn create_stack_acknowledging_macros(
    router: &MultiServiceRouter,
    name: &str,
    template: &str,
) -> (u16, String) {
    create_stack(
        router,
        name,
        template,
        &[member("Capabilities.member.1", CAPABILITY_AUTO_EXPAND)],
    )
    .await
}

/// The `TemplateBody` of a `GetTemplate` response, parsed as JSON so a comparison is about
/// the DOCUMENT rather than the order its members happen to be written in.
fn template_body_json(case: &str, body: &str) -> serde_json::Value {
    let template = xml_text(body, "TemplateBody")
        .unwrap_or_else(|| panic!("case {case:?}: GetTemplate answered no TemplateBody: {body}"));
    serde_json::from_str(&template).unwrap_or_else(|error| {
        panic!("case {case:?}: expected a JSON template, got {template:?} ({error})")
    })
}

/// One `StackEvent` of a `DescribeStackEvents` response, in the members this suite reads.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Event {
    logical_resource_id: Option<String>,
    resource_type: Option<String>,
    resource_status: Option<String>,
    resource_status_reason: Option<String>,
    resource_properties: Option<String>,
    client_request_token: Option<String>,
    physical_resource_id: Option<String>,
    event_id: Option<String>,
}

/// The events a `DescribeStackEvents` response carries, newest first.
fn events(xml: &str) -> Vec<Event> {
    xml.split("<member")
        .skip(1)
        .filter_map(|member| {
            // The serializer writes a list's first entry as `<member xmlns="...">`, so the
            // tag's attributes are skipped before reading its content.
            let member = member.split_once('>')?.1;
            let member = member.split("</member>").next().unwrap_or_default();
            Some(Event {
                logical_resource_id: xml_text(member, "LogicalResourceId"),
                resource_type: xml_text(member, "ResourceType"),
                resource_status: xml_text(member, "ResourceStatus"),
                resource_status_reason: xml_text(member, "ResourceStatusReason"),
                resource_properties: xml_text(member, "ResourceProperties"),
                client_request_token: xml_text(member, "ClientRequestToken"),
                physical_resource_id: xml_text(member, "PhysicalResourceId"),
                event_id: xml_text(member, "EventId"),
            })
        })
        .collect()
}

async fn describe_stack_events(router: &MultiServiceRouter, name: &str) -> Vec<Event> {
    let (status, body) = call(router, "DescribeStackEvents", &[member("StackName", name)]).await;
    assert_eq!(
        status, 200,
        "expected DescribeStackEvents({name}) to succeed, got {status} {body}"
    );
    events(&body)
}

/// The `<Status, LogicalResourceId>` pair of each event, which is what an ordering
/// assertion reads.
fn status_and_logical_id(events: &[Event]) -> Vec<(String, String)> {
    events
        .iter()
        .map(|event| {
            (
                event.resource_status.clone().unwrap_or_default(),
                event.logical_resource_id.clone().unwrap_or_default(),
            )
        })
        .collect()
}

/// The events ONE stack operation reported: the newest run of events sharing one client
/// request token, newest first.
///
/// "All events triggered by a given stack operation are assigned the same client request
/// token", and an operation whose caller named none is given one of its own — so the token
/// is what separates one operation's events from the operations before it.
fn one_operation(events: &[Event]) -> Vec<Event> {
    let token = events
        .first()
        .and_then(|event| event.client_request_token.clone());
    events
        .iter()
        .take_while(|event| event.client_request_token == token)
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// Rule 1: the two template stages.
// ---------------------------------------------------------------------------

/// One row of the processed-stage table: a template whose declared transform this service
/// applies, and the document the Processed stage must answer.
struct StageCase {
    case: &'static str,
    stack_name: &'static str,
    template: &'static str,
    /// The processed document, as JSON text (compared parsed, not byte for byte).
    processed: &'static str,
    /// The logical ids the processed template declares, in logical-id order — which is the
    /// order the stack provisions them in.
    resources: &'static [&'static str],
}

/// The class rule 1 covers: the `AWS::LanguageExtensions` directives are rewrites of the
/// template document (`Fn::ForEach` loops over a collection, `Fn::Length` counts a list,
/// `Fn::ToJsonString` renders one), so the rows differ in the directive used, the shape and
/// size of what it ranges over, and how the loop identifier is read. The issue's own case
/// is the first row.
fn stage_cases() -> Vec<StageCase> {
    vec![
        StageCase {
            case: "a loop over two strings, and a length in the outputs",
            stack_name: "foreach-two",
            template: r#"{"AWSTemplateFormatVersion":"2010-09-09","Transform":"AWS::LanguageExtensions","Resources":{"Fn::ForEach::Handles":["Suffix",["Alpha","Beta"],{"Handle${Suffix}":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}]},"Outputs":{"HandleCount":{"Value":{"Fn::Length":["a","b","c"]}}}}"#,
            processed: r#"{"AWSTemplateFormatVersion":"2010-09-09","Outputs":{"HandleCount":{"Value":3}},"Resources":{"HandleAlpha":{"Type":"AWS::CloudFormation::WaitConditionHandle"},"HandleBeta":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
            resources: &["HandleAlpha", "HandleBeta"],
        },
        StageCase {
            case: "a loop over three strings whose properties read the identifier with Ref",
            stack_name: "foreach-ref",
            template: r#"{"Transform":["AWS::LanguageExtensions"],"Resources":{"Fn::ForEach::Params":["Name",["one","two","three"],{"Param${Name}":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":{"Ref":"Name"}}}}]}}"#,
            processed: r#"{"Resources":{"Paramone":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":"one"}},"Paramtwo":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":"two"}},"Paramthree":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":"three"}}}}"#,
            resources: &["Paramone", "Paramthree", "Paramtwo"],
        },
        StageCase {
            case: "a loop over numbers, read through an Fn::Sub template string",
            stack_name: "foreach-numbers",
            template: r#"{"Transform":"AWS::LanguageExtensions","Resources":{"Fn::ForEach::Shards":["Index",[1,2],{"Shard${Index}":{"Type":"AWS::SSM::Parameter","Properties":{"Name":{"Fn::Sub":"shard-${Index}"},"Type":"String","Value":"x"}}}]}}"#,
            processed: r#"{"Resources":{"Shard1":{"Type":"AWS::SSM::Parameter","Properties":{"Name":{"Fn::Sub":"shard-1"},"Type":"String","Value":"x"}},"Shard2":{"Type":"AWS::SSM::Parameter","Properties":{"Name":{"Fn::Sub":"shard-2"},"Type":"String","Value":"x"}}}}"#,
            resources: &["Shard1", "Shard2"],
        },
        StageCase {
            // `Fn::Sub`'s other documented form: `["<template>", {<variables>}]`, whose
            // template string is the first element.
            case: "a loop read through the two-argument Fn::Sub form",
            stack_name: "foreach-sub-list",
            template: r#"{"Transform":"AWS::LanguageExtensions","Resources":{"Fn::ForEach::Nodes":["Tier",["web","db"],{"Node${Tier}":{"Type":"AWS::SSM::Parameter","Properties":{"Name":{"Fn::Sub":["${Zone}-${Tier}",{"Zone":{"Ref":"Tier"}}]},"Type":"String","Value":"x"}}}]}}"#,
            processed: r#"{"Resources":{"Nodeweb":{"Type":"AWS::SSM::Parameter","Properties":{"Name":{"Fn::Sub":["${Zone}-web",{"Zone":"web"}]},"Type":"String","Value":"x"}},"Nodedb":{"Type":"AWS::SSM::Parameter","Properties":{"Name":{"Fn::Sub":["${Zone}-db",{"Zone":"db"}]},"Type":"String","Value":"x"}}}}"#,
            resources: &["Nodedb", "Nodeweb"],
        },
        StageCase {
            case: "no loop at all: a length of an empty list, of five items, and a rendered document",
            stack_name: "lengths",
            template: r#"{"Transform":"AWS::LanguageExtensions","Outputs":{"Empty":{"Value":{"Fn::Length":[]}},"Five":{"Value":{"Fn::Length":[1,2,3,4,5]}},"Rendered":{"Value":{"Fn::ToJsonString":{"b":2,"a":[1,"x"]}}}}}"#,
            processed: r#"{"Outputs":{"Empty":{"Value":0},"Five":{"Value":5},"Rendered":{"Value":"{\"a\":[1,\"x\"],\"b\":2}"}}}"#,
            resources: &[],
        },
        StageCase {
            case: "a loop inside a loop",
            stack_name: "foreach-nested",
            template: r#"{"Transform":"AWS::LanguageExtensions","Resources":{"Fn::ForEach::Zones":["Zone",["a","b"],{"Fn::ForEach::Tiers${Zone}":["Tier",["web"],{"Node${Zone}${Tier}":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}]}]}}"#,
            processed: r#"{"Resources":{"Nodeaweb":{"Type":"AWS::CloudFormation::WaitConditionHandle"},"Nodebweb":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
            resources: &["Nodeaweb", "Nodebweb"],
        },
        StageCase {
            case: "the transform is declared but the template uses none of its directives",
            stack_name: "transform-only",
            template: r#"{"Transform":"AWS::LanguageExtensions","Resources":{"Plain":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
            processed: r#"{"Resources":{"Plain":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
            resources: &["Plain"],
        },
        StageCase {
            // The SPELLING a template is submitted in is not what decides whether it can be
            // processed: "You can write CloudFormation templates in JSON or YAML", and the
            // directives are rewrites of the document either way. The processed stage is
            // the expanded DOCUMENT, served as JSON.
            case: "a yaml template declaring the transform and using none of its directives",
            stack_name: "yaml-transform-only",
            template: "Transform: AWS::LanguageExtensions\nResources:\n  Plain:\n    Type: AWS::CloudFormation::WaitConditionHandle\n",
            processed: r#"{"Resources":{"Plain":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
            resources: &["Plain"],
        },
        StageCase {
            case: "a yaml loop over two strings, and a length in the outputs",
            stack_name: "yaml-foreach",
            template: "Transform: AWS::LanguageExtensions\nResources:\n  Fn::ForEach::Handles:\n    - Suffix\n    - [Alpha, Beta]\n    - Handle${Suffix}:\n        Type: AWS::CloudFormation::WaitConditionHandle\nOutputs:\n  HandleCount:\n    Value:\n      Fn::Length: [a, b, c]\n",
            processed: r#"{"Outputs":{"HandleCount":{"Value":3}},"Resources":{"HandleAlpha":{"Type":"AWS::CloudFormation::WaitConditionHandle"},"HandleBeta":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
            resources: &["HandleAlpha", "HandleBeta"],
        },
    ]
}

#[tokio::test]
async fn get_template_answers_the_submitted_body_as_original_and_the_expanded_one_as_processed() {
    let router = router();
    let cases = stage_cases();

    for case in &cases {
        let (status, body) =
            create_stack_acknowledging_macros(&router, case.stack_name, case.template).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateStack to succeed, got {status} {body}",
            case.case
        );
    }

    for case in &cases {
        // "To get the user-submitted template, specify Original" — byte for byte.
        let (status, body) = get_template(&router, case.stack_name, Some("Original")).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected GetTemplate(Original) to succeed, got {status} {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "TemplateBody").as_deref(),
            Some(case.template),
            "case {:?}: the Original stage is the submitted body, got {body}",
            case.case
        );

        let expected: serde_json::Value = serde_json::from_str(case.processed)
            .unwrap_or_else(|error| panic!("case {:?}: bad row ({error})", case.case));
        // "To get the template after CloudFormation has processed all transforms, specify
        // Processed", and "By default, CloudFormation specifies Processed".
        for stage in [Some("Processed"), None] {
            let (status, body) = get_template(&router, case.stack_name, stage).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected GetTemplate({stage:?}) to succeed, got {status} {body}",
                case.case
            );
            let processed = template_body_json(case.case, &body);
            assert_eq!(
                processed, expected,
                "case {:?}: GetTemplate({stage:?}) must answer the expanded template, got \
                 {processed}",
                case.case
            );
            assert!(
                processed.get("Transform").is_none(),
                "case {:?}: the applied transform does not survive into the processed \
                 template, got {processed}",
                case.case
            );
        }
        assert_eq!(
            xml_texts(&body, "member"),
            vec!["Original".to_string(), "Processed".to_string()],
            "case {:?}: both stages are available for a stack, got {body}",
            case.case
        );
    }

    // The processed template is also the document the stack deploys: the resources it
    // provisions are the EXPANDED ones, which its events are what report.
    wait_for_the_transition().await;
    for case in &cases {
        let events = describe_stack_events(&router, case.stack_name).await;
        let mut created: Vec<String> = events
            .iter()
            .filter(|event| {
                event.resource_status.as_deref() == Some(CREATE_COMPLETE)
                    && event.resource_type.as_deref() != Some(STACK_RESOURCE_TYPE)
            })
            .map(|event| event.logical_resource_id.clone().unwrap_or_default())
            .collect();
        created.sort();
        let mut expected: Vec<String> = case.resources.iter().map(|id| (*id).to_string()).collect();
        expected.sort();
        assert_eq!(
            created, expected,
            "case {:?}: the stack provisions the processed template's resources, got {events:?}",
            case.case
        );
    }
}

/// One row of the macro table: a transform declaration this emulator cannot apply, because
/// applying it means running a macro's Lambda function.
struct MacroCase {
    case: &'static str,
    stack_name: &'static str,
    template: &'static str,
}

/// The class: every transform that is NOT one this service applies ITSELF — the language
/// extensions, whose directives are document rewrites, and the SAM macro, which
/// `state::sam` expands — is a macro backed by a Lambda function that runs outside this
/// emulator, including the CloudFormation-hosted `AWS::Include` one named in
/// `CreateStackInput$Capabilities`. The rows span each of the forms `Transform` can be
/// written in.
fn macro_cases() -> Vec<MacroCase> {
    vec![
        MacroCase {
            case: "the include transform",
            stack_name: "macro-include",
            template: r#"{"Transform":{"Name":"AWS::Include","Parameters":{"Location":"s3://bucket/snippet.json"}}}"#,
        },
        MacroCase {
            case: "a macro of the caller's own",
            stack_name: "macro-custom",
            template: r#"{"Transform":"MyOwnMacro","Resources":{"Plain":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
        },
        MacroCase {
            case: "a list that mixes the language extensions with a macro",
            stack_name: "macro-mixed",
            template: r#"{"Transform":["AWS::LanguageExtensions","MyOwnMacro"],"Resources":{"Plain":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
        },
        MacroCase {
            // One transform this service applies and one it cannot: the stage is a function
            // of EVERY declaration, so a single macro in the list is enough to leave the
            // template with no processed stage.
            case: "a list that mixes the serverless transform with a macro",
            stack_name: "macro-mixed-sam",
            template: r#"{"Transform":["AWS::Serverless-2016-10-31","MyOwnMacro"],"Resources":{"Plain":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
        },
        MacroCase {
            // A YAML template's top-level `Transform:` declares one just as the JSON member
            // does, and the declaration is read from the PARSED body: a macro is a macro in
            // either spelling.
            case: "a yaml template declaring a macro",
            stack_name: "macro-yaml",
            template: "Transform: MyOwnMacro\nResources:\n  Plain:\n    Type: AWS::CloudFormation::WaitConditionHandle\n",
        },
        MacroCase {
            // The same member with its key quoted and a space before the colon — the
            // declaration is read from the PARSED body, so any form the reader accepts is
            // seen.
            case: "a yaml template whose transform key is quoted",
            stack_name: "macro-yaml-quoted",
            template: "\"Transform\" : MyOwnMacro\nResources:\n  Plain:\n    Type: AWS::CloudFormation::WaitConditionHandle\n",
        },
        MacroCase {
            // A YAML FLOW mapping: not valid JSON, and the transform is not on a line of
            // its own — the reader still sees it.
            case: "a yaml flow mapping declaring a macro",
            stack_name: "macro-yaml-flow",
            template: "{Transform: MyOwnMacro, Resources: {Plain: {Type: AWS::CloudFormation::WaitConditionHandle}}}",
        },
    ]
}

#[tokio::test]
async fn a_template_whose_transform_is_a_macro_has_an_original_stage_but_no_processed_one() {
    let router = router();
    let cases = macro_cases();

    for case in &cases {
        let (status, body) =
            create_stack_acknowledging_macros(&router, case.stack_name, case.template).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateStack to succeed, got {status} {body}",
            case.case
        );

        let (status, body) = get_template(&router, case.stack_name, Some("Original")).await;
        assert_eq!(
            status, 200,
            "case {:?}: the submitted template is still readable, got {status} {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "TemplateBody").as_deref(),
            Some(case.template),
            "case {:?}: the Original stage is the submitted body, got {body}",
            case.case
        );

        // 501, never the unprocessed body answered as if it had been processed — and the
        // stage the read cannot serve is not advertised as available either.
        for stage in [Some("Processed"), None] {
            let (status, body) = get_template(&router, case.stack_name, stage).await;
            assert_eq!(
                status, 501,
                "case {:?}: GetTemplate({stage:?}) cannot run a macro, got {status} {body}",
                case.case
            );
        }
        let (_, body) = get_template(&router, case.stack_name, Some("Original")).await;
        assert_eq!(
            xml_texts(&body, "member"),
            vec!["Original".to_string()],
            "case {:?}: only the stage this read can serve is advertised, got {body}",
            case.case
        );
    }

    // Nothing was provisioned from a document this emulator never processed, and the
    // create it opened ends in the modelled failure state rather than reporting a stack
    // whose resources nothing created.
    wait_for_the_transition().await;
    for case in &cases {
        let events = describe_stack_events(&router, case.stack_name).await;
        assert!(
            events
                .iter()
                .all(|event| event.resource_type.as_deref() == Some(STACK_RESOURCE_TYPE)),
            "case {:?}: an unprocessed template provisions no resources, got {events:?}",
            case.case
        );
        assert_failed_with_a_reason(&router, case.case, case.stack_name, CREATE_FAILED).await;
    }
}

// ---------------------------------------------------------------------------
// Rule 1, continued: a CHANGE SET is planned from, and serves, the same stage its
// execution deploys.
// ---------------------------------------------------------------------------

/// `GetTemplate` for a change set, at one stage or at the service's default.
async fn get_change_set_template(
    router: &MultiServiceRouter,
    stack_name: &str,
    change_set_name: &str,
    stage: Option<&str>,
) -> (u16, String) {
    let mut members = vec![
        member("ChangeSetName", change_set_name),
        member("StackName", stack_name),
    ];
    if let Some(stage) = stage {
        members.push(member("TemplateStage", stage));
    }
    call(router, "GetTemplate", &members).await
}

/// The logical ids the `Resources` section of a processed template declares, sorted.
fn declared_logical_ids(case: &str, processed: &serde_json::Value) -> Vec<String> {
    let mut ids: Vec<String> = processed
        .get("Resources")
        .and_then(serde_json::Value::as_object)
        .map(|resources| resources.keys().cloned().collect())
        .unwrap_or_else(|| panic!("case {case:?}: the processed template declares no Resources"));
    ids.sort();
    ids
}

/// `ids` sorted, for a comparison that is about the SET of resources rather than the order
/// a response happens to list them in.
fn sorted(ids: &[&str]) -> Vec<String> {
    let mut sorted: Vec<String> = ids.iter().map(|id| (*id).to_string()).collect();
    sorted.sort();
    sorted
}

/// One row of the change-set planning table: the template the change set is created from
/// and the resources the stage it would deploy declares.
struct PlanCase {
    case: &'static str,
    stack_name: &'static str,
    change_set_name: &'static str,
    template: &'static str,
    /// The logical ids the PROCESSED stage declares — which is what the plan must name and
    /// what the execution must deploy.
    resources: &'static [&'static str],
}

/// The class: a change set is a plan over the resources its execution would deploy, so the
/// rows differ in where those resources come from — a directive alone, a directive beside a
/// plainly declared resource, and no directive at all.
fn plan_cases() -> Vec<PlanCase> {
    vec![
        PlanCase {
            case: "every resource comes from a loop",
            stack_name: "plan-foreach-only",
            change_set_name: "plan-foreach-only-cs",
            template: r#"{"Transform":"AWS::LanguageExtensions","Resources":{"Fn::ForEach::Handles":["Suffix",["One","Two"],{"Handle${Suffix}":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}]}}"#,
            resources: &["HandleOne", "HandleTwo"],
        },
        PlanCase {
            case: "a plainly declared resource beside a looped one",
            stack_name: "plan-mixed",
            change_set_name: "plan-mixed-cs",
            template: r#"{"Transform":"AWS::LanguageExtensions","Resources":{"Plain":{"Type":"AWS::CloudFormation::WaitConditionHandle"},"Fn::ForEach::Params":["Name",["alpha"],{"Param${Name}":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":"v"}}}]}}"#,
            resources: &["Paramalpha", "Plain"],
        },
        PlanCase {
            case: "no transform at all: the two stages are one document",
            stack_name: "plan-plain",
            change_set_name: "plan-plain-cs",
            template: r#"{"Resources":{"Solo":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":"v"}}}}"#,
            resources: &["Solo"],
        },
    ]
}

#[tokio::test]
async fn a_change_set_is_planned_from_the_stage_its_execution_deploys() {
    let (router, engine) = router_with_engine();
    let cases = plan_cases();

    for case in &cases {
        let (code, body) = create_change_set(
            &router,
            case.stack_name,
            case.change_set_name,
            &[
                member("ChangeSetType", "CREATE"),
                member("TemplateBody", case.template),
                member("Capabilities.member.1", CAPABILITY_AUTO_EXPAND),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {:?}: expected CreateChangeSet to succeed, got {code} {body}",
            case.case
        );

        // The plan is over the resources the execution would deploy: a resource a
        // directive expands is one of them.
        let body = settled_change_set(
            &router,
            &engine,
            case.change_set_name,
            Some(case.stack_name),
        )
        .await;
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(change_set_status::CREATE_COMPLETE),
            "case {:?}: a change set with changes completes, got {body}",
            case.case
        );
        let mut planned = xml_texts(&body, "LogicalResourceId");
        planned.sort();
        assert_eq!(
            planned,
            sorted(case.resources),
            "case {:?}: the plan names the resources the deployed stage declares, got {body}",
            case.case
        );

        // And the change set serves those same two stages.
        let (code, body) =
            get_change_set_template(&router, case.stack_name, case.change_set_name, Some("Original"))
                .await;
        assert_eq!(
            code, 200,
            "case {:?}: the change set's submitted body is readable, got {code} {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "TemplateBody").as_deref(),
            Some(case.template),
            "case {:?}: the Original stage is the submitted body, got {body}",
            case.case
        );
        for stage in [Some("Processed"), None] {
            let (code, body) =
                get_change_set_template(&router, case.stack_name, case.change_set_name, stage).await;
            assert_eq!(
                code, 200,
                "case {:?}: expected GetTemplate({stage:?}) to succeed, got {code} {body}",
                case.case
            );
            let processed = template_body_json(case.case, &body);
            assert_eq!(
                declared_logical_ids(case.case, &processed),
                sorted(case.resources),
                "case {:?}: GetTemplate({stage:?}) answers the stage the plan was computed \
                 from, got {processed}",
                case.case
            );
            assert!(
                processed.get("Transform").is_none(),
                "case {:?}: the applied transform does not survive into the processed \
                 template, got {processed}",
                case.case
            );
        }

        let (code, body) =
            execute_change_set(&router, case.change_set_name, Some(case.stack_name)).await;
        assert_eq!(
            code, 200,
            "case {:?}: expected ExecuteChangeSet to succeed, got {code} {body}",
            case.case
        );
    }

    // The execution deploys exactly what the plan named: the resources the reviewed change
    // list carried are the ones the stack's events report provisioned.
    settle_the_store(&router, &engine).await;
    for case in &cases {
        let events = describe_stack_events(&router, case.stack_name).await;
        let mut created: Vec<String> = events
            .iter()
            .filter(|event| {
                event.resource_status.as_deref() == Some(CREATE_COMPLETE)
                    && event.resource_type.as_deref() != Some(STACK_RESOURCE_TYPE)
            })
            .map(|event| event.logical_resource_id.clone().unwrap_or_default())
            .collect();
        created.sort();
        assert_eq!(
            created,
            sorted(case.resources),
            "case {:?}: the execution deploys the resources the plan named, got {events:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn the_two_requests_that_select_a_change_sets_processed_stage_answer_the_same_thing() {
    let router = router();
    // "To get the template after CloudFormation has processed all transforms, specify
    // Processed. […] By default, CloudFormation specifies Processed": naming the stage and
    // naming none select the SAME stage, so the two requests cannot answer differently —
    // whatever stage the change set has reached. The rows are the two things that stage can
    // be: the submitted body itself ("if the template doesn't include transforms, Original
    // and Processed return the same template") and a document the expansion computes.
    let cases: [(&str, &str, &str, &str); 2] = [
        (
            "no transform: the two stages are one document",
            "both-ways-plain",
            "both-ways-plain-cs",
            r#"{"Resources":{"Solo":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":"v"}}}}"#,
        ),
        (
            "a transform the service applies",
            "both-ways-transformed",
            "both-ways-transformed-cs",
            r#"{"Transform":"AWS::LanguageExtensions","Resources":{"Fn::ForEach::Handles":["Suffix",["One"],{"Handle${Suffix}":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}]}}"#,
        ),
    ];

    for (case, stack_name, change_set_name, template) in cases {
        let (code, body) = create_change_set(
            &router,
            stack_name,
            change_set_name,
            &[
                member("ChangeSetType", "CREATE"),
                member("TemplateBody", template),
                member("Capabilities.member.1", CAPABILITY_AUTO_EXPAND),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected CreateChangeSet to succeed, got {code} {body}"
        );

        let named =
            get_change_set_template(&router, stack_name, change_set_name, Some("Processed")).await;
        let defaulted = get_change_set_template(&router, stack_name, change_set_name, None).await;
        // Compared on what the response SAYS — its status, the body it serves and the
        // stages it advertises — not on the request id every response mints one of.
        let answer = |(code, body): &(u16, String)| {
            (
                *code,
                xml_text(body, "TemplateBody"),
                xml_texts(body, "member"),
            )
        };
        assert_eq!(
            answer(&named),
            answer(&defaulted),
            "case {case:?}: naming Processed and naming no stage select the same stage, so \
             they answer the same thing, got {named:?} and {defaulted:?}"
        );
        if template.contains("Transform") {
            // A transform this service CAN apply has a processed stage; the change set's
            // creation is what makes it available ("After CloudFormation finishes creating
            // the change set, the Processed template becomes available"). So the read is
            // either answered that stage, or REFUSED as a request for something not
            // available yet — never 501, which is the answer for a stage no transform of
            // this service computes at all.
            let (code, body) = named;
            assert!(
                code == 200 || code == 400,
                "case {case:?}: a stage this service computes is served or refused as not \
                 available yet, got {code} {body}"
            );
            continue;
        }
        // A change set whose template declares no transform has that one document as BOTH
        // stages, so it is served at once rather than withheld until the creation settles —
        // and both stages are ADVERTISED, because the stage set a response reports and the
        // stage it serves cannot disagree.
        let (code, body) = defaulted;
        assert_eq!(
            code, 200,
            "case {case:?}: the one document both stages are is served, got {code} {body}"
        );
        assert_eq!(
            xml_text(&body, "TemplateBody").as_deref(),
            Some(template),
            "case {case:?}: that document is the submitted body, got {body}"
        );
        assert_eq!(
            xml_texts(&body, "member"),
            vec!["Original".to_string(), "Processed".to_string()],
            "case {case:?}: the stage served is advertised as available, got {body}"
        );
    }
}

#[tokio::test]
async fn a_change_set_whose_transform_cannot_be_applied_fails_and_serves_no_processed_stage() {
    let (router, engine) = router_with_engine();
    // The same class as `macro_cases`: a transform this emulator cannot apply, so there is
    // no stage to plan from — and a plan over a document nobody processed is never offered.
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a macro of the caller's own",
            "plan-macro-custom",
            "plan-macro-custom-cs",
            r#"{"Transform":"MyOwnMacro","Resources":{"Plain":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
        ),
        (
            "the include transform",
            "plan-macro-include",
            "plan-macro-include-cs",
            r#"{"Transform":{"Name":"AWS::Include","Parameters":{"Location":"s3://bucket/snippet.json"}},"Resources":{"Plain":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
        ),
        (
            "a list that mixes the language extensions with a macro",
            "plan-macro-mixed",
            "plan-macro-mixed-cs",
            r#"{"Transform":["AWS::LanguageExtensions","MyOwnMacro"],"Resources":{"Plain":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
        ),
    ];

    for (case, stack_name, change_set_name, template) in &cases {
        let (code, body) = create_change_set(
            &router,
            stack_name,
            change_set_name,
            &[
                member("ChangeSetType", "CREATE"),
                member("TemplateBody", template),
                member("Capabilities.member.1", CAPABILITY_AUTO_EXPAND),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected CreateChangeSet to succeed, got {code} {body}"
        );

        let body = settled_change_set(&router, &engine, change_set_name, Some(stack_name)).await;
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(change_set_status::FAILED),
            "case {case:?}: a change set this service cannot plan fails, got {body}"
        );
        assert!(
            xml_text(&body, "StatusReason").is_some_and(|reason| !reason.is_empty()),
            "case {case:?}: the failure names what could not be applied, got {body}"
        );
        assert_eq!(
            xml_text(&body, "ExecutionStatus").as_deref(),
            Some(execution_status::UNAVAILABLE),
            "case {case:?}: a failed change set is not executable, got {body}"
        );

        // The submitted body is still readable; the stage nobody could compute is neither
        // served nor advertised.
        let (code, body) =
            get_change_set_template(&router, stack_name, change_set_name, Some("Original")).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the submitted template is still readable, got {code} {body}"
        );
        assert_eq!(
            xml_text(&body, "TemplateBody").as_deref(),
            Some(*template),
            "case {case:?}: the Original stage is the submitted body, got {body}"
        );
        assert_eq!(
            xml_texts(&body, "member"),
            vec!["Original".to_string()],
            "case {case:?}: only the stage this read can serve is advertised, got {body}"
        );
        for stage in [Some("Processed"), None] {
            let (code, body) =
                get_change_set_template(&router, stack_name, change_set_name, stage).await;
            assert_eq!(
                code, 501,
                "case {case:?}: GetTemplate({stage:?}) cannot run a macro, got {code} {body}"
            );
        }

        let (code, body) = execute_change_set(&router, change_set_name, Some(stack_name)).await;
        assert_eq!(
            code, 400,
            "case {case:?}: a failed change set cannot be executed, got {code} {body}"
        );
    }
}

#[tokio::test]
async fn an_update_change_set_this_service_cannot_plan_stores_no_changes() {
    let case = "an UPDATE change set whose template declares a macro";
    let (router, engine) = router_with_engine();
    // A stack that already HOLDS resources, so there is something a plan could claim to
    // remove.
    let (code, body) = create_stack(
        &router,
        "plan-update-macro",
        r#"{"Resources":{"Keep":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":"v"}}}}"#,
        &[],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected CreateStack to succeed, got {code} {body}"
    );
    settle_the_store(&router, &engine).await;

    let (code, body) = create_change_set(
        &router,
        "plan-update-macro",
        "plan-update-macro-cs",
        &[
            member(
                "TemplateBody",
                r#"{"Transform":"MyOwnMacro","Resources":{"Other":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
            ),
            member("Capabilities.member.1", CAPABILITY_AUTO_EXPAND),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected CreateChangeSet to succeed, got {code} {body}"
    );

    let body = settled_change_set(
        &router,
        &engine,
        "plan-update-macro-cs",
        Some("plan-update-macro"),
    )
    .await;
    assert_eq!(
        xml_text(&body, "Status").as_deref(),
        Some(change_set_status::FAILED),
        "case {case:?}: a change set this service cannot plan fails, got {body}"
    );
    assert_eq!(
        xml_text(&body, "ExecutionStatus").as_deref(),
        Some(execution_status::UNAVAILABLE),
        "case {case:?}: a failed change set is not executable, got {body}"
    );
    // The plan is EMPTY, not a removal of what the stack holds: nothing was computed from
    // any document, so the change set claims no change.
    assert!(
        xml_texts(&body, "Action").is_empty(),
        "case {case:?}: no change is claimed for a plan nothing could compute, got {body}"
    );
    assert!(
        xml_texts(&body, "LogicalResourceId").is_empty(),
        "case {case:?}: no resource is claimed for a plan nothing could compute, got {body}"
    );

    // And the stack is untouched: a change set that failed to be created changed nothing.
    let (code, body) = describe_stack(&router, "plan-update-macro").await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected DescribeStacks to succeed, got {code} {body}"
    );
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some(CREATE_COMPLETE),
        "case {case:?}: the stack keeps the status its create left it in, got {body}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: the macro capability.
// ---------------------------------------------------------------------------

/// One row of the capability table.
struct CapabilityCase {
    case: &'static str,
    template: &'static str,
    acknowledged: &'static [&'static str],
    /// Whether the deployment is accepted.
    accepted: bool,
}

/// The class: a template that declares ANY transform needs `CAPABILITY_AUTO_EXPAND`
/// acknowledged, and one that declares none needs nothing — whatever else the request
/// acknowledges.
fn capability_cases() -> Vec<CapabilityCase> {
    vec![
        CapabilityCase {
            case: "no transform, nothing acknowledged",
            template: r#"{"Resources":{"Plain":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
            acknowledged: &[],
            accepted: true,
        },
        CapabilityCase {
            case: "the language extensions, nothing acknowledged",
            template: r#"{"Transform":"AWS::LanguageExtensions","Resources":{"Plain":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
            acknowledged: &[],
            accepted: false,
        },
        CapabilityCase {
            case: "the language extensions, acknowledged",
            template: r#"{"Transform":"AWS::LanguageExtensions","Resources":{"Plain":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
            acknowledged: &[CAPABILITY_AUTO_EXPAND],
            accepted: true,
        },
        CapabilityCase {
            case: "a macro, only the IAM capability acknowledged",
            template: r#"{"Transform":"AWS::Serverless-2016-10-31","Resources":{"Plain":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
            acknowledged: &["CAPABILITY_IAM"],
            accepted: false,
        },
        CapabilityCase {
            case: "a macro, both capabilities acknowledged",
            template: r#"{"Transform":"AWS::Serverless-2016-10-31","Resources":{"Plain":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
            acknowledged: &["CAPABILITY_IAM", CAPABILITY_AUTO_EXPAND],
            accepted: true,
        },
        CapabilityCase {
            case: "a macro named in the object form, nothing acknowledged",
            template: r#"{"Transform":{"Name":"MyOwnMacro"},"Resources":{"Plain":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
            acknowledged: &[],
            accepted: false,
        },
        CapabilityCase {
            // A YAML template's top-level `Transform:` needs the same acknowledgement.
            case: "a yaml template declaring a transform, nothing acknowledged",
            template: "Transform: AWS::Serverless-2016-10-31\nResources:\n  Plain:\n    Type: AWS::CloudFormation::WaitConditionHandle\n",
            acknowledged: &[],
            accepted: false,
        },
        CapabilityCase {
            case: "a yaml template declaring a transform, acknowledged",
            template: "Transform: AWS::Serverless-2016-10-31\nResources:\n  Plain:\n    Type: AWS::CloudFormation::WaitConditionHandle\n",
            acknowledged: &[CAPABILITY_AUTO_EXPAND],
            accepted: true,
        },
        CapabilityCase {
            // A YAML FLOW mapping: not valid JSON, and the transform is not on a line of
            // its own. The declaration comes from the parser, so it needs the same
            // acknowledgement.
            case: "a yaml flow mapping declaring a transform, nothing acknowledged",
            template: "{Transform: AWS::Serverless-2016-10-31, Resources: {Plain: {Type: AWS::CloudFormation::WaitConditionHandle}}}",
            acknowledged: &[],
            accepted: false,
        },
        CapabilityCase {
            // A YAML template that declares none needs nothing: its two stages are the
            // same document, as they are for a JSON template with no transform.
            case: "a yaml template with no transform, nothing acknowledged",
            template: "Resources:\n  Plain:\n    Type: AWS::CloudFormation::WaitConditionHandle\n",
            acknowledged: &[],
            accepted: true,
        },
    ]
}

/// The `Capabilities.member.N` form members for a request's acknowledgements.
fn capability_members(acknowledged: &[&str]) -> Vec<(String, String)> {
    acknowledged
        .iter()
        .enumerate()
        .map(|(index, capability)| {
            member(&format!("Capabilities.member.{}", index + 1), capability)
        })
        .collect()
}

#[tokio::test]
async fn deploying_a_template_that_declares_a_transform_needs_the_macro_capability() {
    let router = router();

    for (index, case) in capability_cases().into_iter().enumerate() {
        let name = format!("caps-{index}");
        let (status, body) = create_stack(
            &router,
            &name,
            case.template,
            &capability_members(case.acknowledged),
        )
        .await;
        if case.accepted {
            assert_eq!(
                status, 200,
                "case {:?}: expected CreateStack to succeed, got {status} {body}",
                case.case
            );
        } else {
            assert_eq!(
                status, 400,
                "case {:?}: expected CreateStack to be refused, got {status} {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "Code").as_deref(),
                Some("InsufficientCapabilitiesException"),
                "case {:?}: expected the modelled capability refusal, got {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "Message").as_deref(),
                Some(format!("Requires capabilities : [{CAPABILITY_AUTO_EXPAND}]").as_str()),
                "case {:?}: the refusal names the capability to acknowledge, got {body}",
                case.case
            );
        }
    }

    // The same rule on the update path: the template an update deploys is the one whose
    // transform has to be acknowledged.
    let (status, body) = create_stack(
        &router,
        "caps-update",
        r#"{"Resources":{"Plain":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"update: the stack to update\": expected success, got {status} {body}"
    );
    wait_for_the_transition().await;

    let transformed = r#"{"Transform":"AWS::LanguageExtensions","Resources":{"Fn::ForEach::Handles":["Suffix",["Alpha"],{"Handle${Suffix}":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}]}}"#;
    let (status, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "caps-update"),
            member("TemplateBody", transformed),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"update: a transform nobody acknowledged\": expected a refusal, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("InsufficientCapabilitiesException"),
        "case \"update: a transform nobody acknowledged\": expected the modelled refusal, got {body}"
    );

    let mut members = vec![
        member("StackName", "caps-update"),
        member("TemplateBody", transformed),
    ];
    members.extend(capability_members(&[CAPABILITY_AUTO_EXPAND]));
    let (status, body) = call(&router, "UpdateStack", &members).await;
    assert_eq!(
        status, 200,
        "case \"update: the transform acknowledged\": expected success, got {status} {body}"
    );
    wait_for_the_transition().await;
    let (status, body) = get_template(&router, "caps-update", Some("Processed")).await;
    assert_eq!(
        status, 200,
        "case \"update: the transform acknowledged\": the processed stage is served, got \
         {status} {body}"
    );
    let processed = template_body_json("update: the transform acknowledged", &body);
    assert_eq!(
        processed,
        serde_json::json!({
            "Resources": {"HandleAlpha": {"Type": "AWS::CloudFormation::WaitConditionHandle"}}
        }),
        "case \"update: the transform acknowledged\": the update's template is processed too, \
         got {processed}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: an event per resource the operation acts on.
// ---------------------------------------------------------------------------

/// One row of the resource-events table: a template, and the resources it declares in the
/// order the stack provisions them (logical-id order), with each one's type and the
/// properties document its events must carry.
struct ResourceCase {
    case: &'static str,
    stack_name: &'static str,
    template: &'static str,
    /// `(logical id, type, properties)` per resource, in provisioning order.
    resources: &'static [(&'static str, &'static str, Option<&'static str>)],
    /// The `ClientRequestToken` the request names, or `None` to leave it to the service.
    token: Option<&'static str>,
}

/// The class: a create reports the stack's own two events plus, for EVERY resource the
/// template declares, that resource's three create events — whatever the number of
/// resources, their types, or whether they carry properties. The issue's own case (one
/// `AWS::SSM::Parameter` named `ProbeParam`) is the second row.
fn resource_cases() -> Vec<ResourceCase> {
    vec![
        ResourceCase {
            case: "a template that declares no resources at all",
            stack_name: "events-none",
            template: r#"{"Description":"nothing to provision"}"#,
            resources: &[],
            token: Some("token-for-the-empty-stack"),
        },
        ResourceCase {
            case: "one parameter resource with properties",
            stack_name: "events-one",
            template: r#"{"Resources":{"ProbeParam":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":"probe-value"}}}}"#,
            resources: &[(
                "ProbeParam",
                "AWS::SSM::Parameter",
                Some(r#"{"Type":"String","Value":"probe-value"}"#),
            )],
            token: Some("probe-token-1"),
        },
        ResourceCase {
            case: "three resources of three types, one of them without properties",
            stack_name: "events-three",
            template: r#"{"Resources":{"Alpha":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":"a"}},"Beta":{"Type":"AWS::CloudFormation::WaitConditionHandle"},"Gamma":{"Type":"AWS::SNS::Topic","Properties":{"DisplayName":"g","FifoTopic":false}}}}"#,
            resources: &[
                ("Alpha", "AWS::SSM::Parameter", Some(r#"{"Type":"String","Value":"a"}"#)),
                ("Beta", "AWS::CloudFormation::WaitConditionHandle", None),
                (
                    "Gamma",
                    "AWS::SNS::Topic",
                    Some(r#"{"DisplayName":"g","FifoTopic":false}"#),
                ),
            ],
            // No token on the request: the service assigns the operation one of its own.
            token: None,
        },
    ]
}

#[tokio::test]
async fn a_create_reports_an_event_for_every_resource_it_provisions() {
    let router = router();
    let cases = resource_cases();

    for case in &cases {
        let extra = case
            .token
            .map(|token| vec![member("ClientRequestToken", token)])
            .unwrap_or_default();
        let (status, body) = create_stack(&router, case.stack_name, case.template, &extra).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateStack to succeed, got {status} {body}",
            case.case
        );
    }
    wait_for_the_transition().await;

    for case in &cases {
        let events = describe_stack_events(&router, case.stack_name).await;

        // "Returns all stack related events for a specified stack in reverse chronological
        // order": the stack's terminal event, every resource's events, then the stack's
        // opening event. The resources' events sit BETWEEN the stack's two; which resource
        // the service happens to act on first is not a rule, so the events in between are
        // compared as a multiset.
        let pairs = status_and_logical_id(&events);
        let (newest, rest) = pairs
            .split_first()
            .unwrap_or_else(|| panic!("case {:?}: no events at all", case.case));
        let (oldest, in_between) = rest
            .split_last()
            .unwrap_or_else(|| panic!("case {:?}: expected the stack's two own events", case.case));
        assert_eq!(
            newest,
            &(CREATE_COMPLETE.to_string(), case.stack_name.to_string()),
            "case {:?}: the newest event completes the stack's own create, got {events:?}",
            case.case
        );
        assert_eq!(
            oldest,
            &(CREATE_IN_PROGRESS.to_string(), case.stack_name.to_string()),
            "case {:?}: the oldest event opens the stack's own create, got {events:?}",
            case.case
        );
        let mut expected: Vec<(String, String)> = Vec::new();
        for (logical_id, _, _) in case.resources {
            expected.push((CREATE_COMPLETE.to_string(), (*logical_id).to_string()));
            expected.push((CREATE_IN_PROGRESS.to_string(), (*logical_id).to_string()));
            expected.push((CREATE_IN_PROGRESS.to_string(), (*logical_id).to_string()));
        }
        expected.sort();
        let mut reported = in_between.to_vec();
        reported.sort();
        assert_eq!(
            reported, expected,
            "case {:?}: expected every resource's events between the stack's own, got {events:?}",
            case.case
        );

        // Each resource's events carry that resource's type, properties and a physical id.
        for (logical_id, resource_type, properties) in case.resources {
            let of_resource: Vec<&Event> = events
                .iter()
                .filter(|event| event.logical_resource_id.as_deref() == Some(*logical_id))
                .collect();
            assert_eq!(
                of_resource.len(),
                3,
                "case {:?}: expected three events for {logical_id}, got {of_resource:?}",
                case.case
            );
            // ONE resource's own events are ordered: its create completes after it opened.
            assert_eq!(
                of_resource
                    .iter()
                    .map(|event| event.resource_status.clone().unwrap_or_default())
                    .collect::<Vec<String>>(),
                vec![
                    CREATE_COMPLETE.to_string(),
                    CREATE_IN_PROGRESS.to_string(),
                    CREATE_IN_PROGRESS.to_string(),
                ],
                "case {:?}: {logical_id}'s own events run newest-first from its completion, \
                 got {of_resource:?}",
                case.case
            );
            for event in &of_resource {
                assert_eq!(
                    event.resource_type.as_deref(),
                    Some(*resource_type),
                    "case {:?}: {logical_id}'s events report its type, got {event:?}",
                    case.case
                );
                assert_eq!(
                    event.resource_properties.as_deref(),
                    *properties,
                    "case {:?}: {logical_id}'s events report the properties it was created \
                     with, got {event:?}",
                    case.case
                );
            }
            // `PhysicalResourceId` is "the name or unique identifier associated with the
            // physical instance of the resource": the event that OPENS the create (the
            // oldest of the three) has no instance to name yet, so it reports none, and the
            // ones after it name the instance CloudFormation generated a name for.
            let (opening, once_created) = of_resource
                .split_last()
                .expect("three events, asserted just above");
            assert!(
                opening
                    .physical_resource_id
                    .as_deref()
                    .is_none_or(str::is_empty),
                "case {:?}: {logical_id} has no physical instance when its create opens, got \
                 {opening:?}",
                case.case
            );
            for event in once_created {
                let physical = event.physical_resource_id.clone().unwrap_or_default();
                // The name CloudFormation generates for a resource the template does not
                // name is `<stack>-<logical id>-<unique>`; a type whose `Ref` returns an
                // ARN or a URL carries that name inside it.
                assert!(
                    physical.contains(&format!("{}-{logical_id}-", case.stack_name)),
                    "case {:?}: {logical_id} has a generated physical id once created, got \
                     {physical:?}",
                    case.case
                );
            }
            // "CREATE_IN_PROGRESS with ResourceStatusReason 'Resource creation Initiated'",
            // once per resource.
            let initiated = of_resource
                .iter()
                .filter(|event| {
                    event.resource_status_reason.as_deref() == Some(RESOURCE_CREATION_INITIATED)
                })
                .count();
            assert_eq!(
                initiated, 1,
                "case {:?}: exactly one of {logical_id}'s events reports the create was \
                 initiated, got {of_resource:?}",
                case.case
            );
        }

        // "All events triggered by a given stack operation are assigned the same client
        // request token": the caller's when it named one, the service's otherwise — on
        // every event, resource-level and stack-level alike.
        let tokens: Vec<String> = events
            .iter()
            .map(|event| event.client_request_token.clone().unwrap_or_default())
            .collect();
        match case.token {
            Some(token) => assert!(
                tokens.iter().all(|carried| carried == token),
                "case {:?}: every event carries the request's token {token:?}, got {tokens:?}",
                case.case
            ),
            None => {
                let first = tokens.first().cloned().unwrap_or_default();
                assert!(
                    !first.is_empty() && tokens.iter().all(|carried| *carried == first),
                    "case {:?}: every event carries the one token the service assigned the \
                     operation, got {tokens:?}",
                    case.case
                );
            }
        }

        let mut event_ids: Vec<String> = events
            .iter()
            .map(|event| event.event_id.clone().unwrap_or_default())
            .collect();
        let total = event_ids.len();
        event_ids.sort();
        event_ids.dedup();
        assert_eq!(
            event_ids.len(),
            total,
            "case {:?}: every EventId is distinct, got {events:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn an_update_and_a_delete_report_events_only_for_the_resources_they_act_on() {
    let router = router();
    const FIRST: &str = r#"{"Resources":{"Kept":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":"keep"}},"Changed":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":"before"}}}}"#;
    // `Kept` is untouched, `Changed` has new properties, `Added` is new, and nothing is
    // removed yet.
    const SECOND: &str = r#"{"Resources":{"Kept":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":"keep"}},"Changed":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":"after"}},"Added":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#;
    // `Changed` and `Added` are gone, `Kept` is still untouched.
    const THIRD: &str = r#"{"Resources":{"Kept":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":"keep"}}}}"#;

    let (status, body) = create_stack(&router, "churn", FIRST, &[]).await;
    assert_eq!(
        status, 200,
        "case \"the stack to churn\": expected success, got {status} {body}"
    );
    wait_for_the_transition().await;

    for (case, template, expected) in [
        (
            "an update that changes one resource and adds another",
            SECOND,
            vec![
                (UPDATE_COMPLETE.to_string(), "Changed".to_string()),
                (UPDATE_IN_PROGRESS.to_string(), "Changed".to_string()),
                (CREATE_COMPLETE.to_string(), "Added".to_string()),
                (CREATE_IN_PROGRESS.to_string(), "Added".to_string()),
                (CREATE_IN_PROGRESS.to_string(), "Added".to_string()),
            ],
        ),
        (
            // `Added` is an `AWS::CloudFormation::WaitConditionHandle`, which no service
            // this emulator calls holds: its removal asks nothing of anybody, so the
            // operation reports the deletion it really carried out and not one it did not.
            "an update that removes two resources",
            THIRD,
            vec![
                (DELETE_COMPLETE.to_string(), "Changed".to_string()),
                (DELETE_IN_PROGRESS.to_string(), "Changed".to_string()),
            ],
        ),
    ] {
        let (status, body) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", "churn"),
                member("TemplateBody", template),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateStack to succeed, got {status} {body}"
        );
        wait_for_the_transition().await;

        // THIS operation's events: the newest run of events sharing one client request
        // token ("All events triggered by a given stack operation are assigned the same
        // client request token" — and every operation here was given one of its own).
        let events = describe_stack_events(&router, "churn").await;
        let this_operation = one_operation(&events);
        let pairs = status_and_logical_id(&this_operation);
        let (newest, rest) = pairs
            .split_first()
            .unwrap_or_else(|| panic!("case {case:?}: no events at all"));
        let (oldest, in_between) = rest
            .split_last()
            .unwrap_or_else(|| panic!("case {case:?}: expected the stack's two own events"));
        assert_eq!(
            newest,
            &(UPDATE_COMPLETE.to_string(), "churn".to_string()),
            "case {case:?}: the newest event completes the stack's update, got {this_operation:?}"
        );
        assert_eq!(
            oldest,
            &(UPDATE_IN_PROGRESS.to_string(), "churn".to_string()),
            "case {case:?}: the oldest event opens the stack's update, got {this_operation:?}"
        );
        let mut expected = expected;
        expected.sort();
        let mut reported = in_between.to_vec();
        reported.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: only the resources the update acted on report events, got \
             {this_operation:?}"
        );
        assert!(
            !pairs.iter().any(|(_, logical_id)| logical_id == "Kept"),
            "case {case:?}: an unchanged resource is not acted on, so it reports no event, got \
             {this_operation:?}"
        );
    }

    let (status, body) = call(&router, "DeleteStack", &[member("StackName", "churn")]).await;
    assert_eq!(
        status, 200,
        "case \"a delete\": expected DeleteStack to succeed, got {status} {body}"
    );
    let stack_id = {
        let (_, body) = call(
            &router,
            "DescribeStackEvents",
            &[member("StackName", "churn")],
        )
        .await;
        xml_text(&body, "StackId").expect("the stack's id")
    };
    wait_for_the_transition().await;

    let events = describe_stack_events(&router, &stack_id).await;
    assert_eq!(
        status_and_logical_id(&one_operation(&events)),
        vec![
            (DELETE_COMPLETE.to_string(), "churn".to_string()),
            (DELETE_COMPLETE.to_string(), "Kept".to_string()),
            (DELETE_IN_PROGRESS.to_string(), "Kept".to_string()),
            (DELETE_IN_PROGRESS.to_string(), "churn".to_string()),
        ],
        "case \"a delete\": deleting the stack deletes the resources it provisioned, got \
         {events:?}"
    );
}

#[tokio::test]
async fn an_events_properties_document_is_reported_only_while_it_fits_the_per_event_bound() {
    let router = router();
    // `StackEvent$ResourceProperties` is caller-sized, and the log holds up to
    // `MAX_EVENTS_PER_STACK` of them, so ONE event reports the document only while it fits
    // `MAX_EVENT_PROPERTIES_BYTES`. The rows straddle that ceiling: a document well under
    // it is reported verbatim, and one over it is reported as ABSENT — a document cut in
    // half is not the document, and the member is optional.
    let cases: [(&str, &str, usize, bool); 3] = [
        ("well under the per-event ceiling", "props-small", 16, true),
        (
            "just under the per-event ceiling",
            "props-at-the-edge",
            MAX_EVENT_PROPERTIES_BYTES - 64,
            true,
        ),
        (
            "past the per-event ceiling",
            "props-too-big",
            MAX_EVENT_PROPERTIES_BYTES + 64,
            false,
        ),
    ];

    for (case, stack_name, value_bytes, reported) in cases {
        let template = serde_json::json!({
            "Resources": {
                "Probe": {
                    "Type": "AWS::SSM::Parameter",
                    "Properties": {"Type": "String", "Value": "v".repeat(value_bytes)}
                }
            }
        })
        .to_string();
        let (status, body) = create_stack(&router, stack_name, &template, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateStack to succeed, got {status} {body}"
        );
        wait_for_the_transition().await;

        let events = describe_stack_events(&router, stack_name).await;
        let of_resource: Vec<&Event> = events
            .iter()
            .filter(|event| event.logical_resource_id.as_deref() == Some("Probe"))
            .collect();
        assert!(
            !of_resource.is_empty(),
            "case {case:?}: the resource reports its own events whatever its size, got \
             {events:?}"
        );
        for event in &of_resource {
            let carried = event.resource_properties.as_deref();
            assert_eq!(
                carried.is_some(),
                reported,
                "case {case:?}: a properties document of {value_bytes} bytes is reported \
                 ({reported}), got {carried:?}"
            );
            if let Some(carried) = carried {
                assert!(
                    carried.len() <= MAX_EVENT_PROPERTIES_BYTES,
                    "case {case:?}: a reported document is within the per-event bound, got \
                     {} bytes",
                    carried.len()
                );
                assert!(
                    serde_json::from_str::<serde_json::Value>(carried).is_ok(),
                    "case {case:?}: a reported document is a document, got {carried:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn every_event_of_a_change_set_execution_carries_the_operations_token() {
    let router = router();
    const TEMPLATE: &str = r#"{"Resources":{"Probe":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":"probe"}}}}"#;

    // A change set execution opens a stack operation, so the rule holds for it too:
    // "All events triggered by a given stack operation are assigned the same client
    // request token". This caller names none, so the service assigns the operation one.
    let (status, body) = call(
        &router,
        "CreateChangeSet",
        &[
            member("StackName", "executed"),
            member("ChangeSetName", "first"),
            member("ChangeSetType", "CREATE"),
            member("TemplateBody", TEMPLATE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a CREATE change set\": expected success, got {status} {body}"
    );
    wait_for_the_transition().await;

    let (status, body) = call(
        &router,
        "ExecuteChangeSet",
        &[
            member("StackName", "executed"),
            member("ChangeSetName", "first"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"executing it\": expected success, got {status} {body}"
    );
    wait_for_the_transition().await;

    let events = describe_stack_events(&router, "executed").await;
    let operation = one_operation(&events);
    assert!(
        operation.len() >= 2,
        "case \"executing it\": the execution reports its own events, got {events:?}"
    );
    let tokens: Vec<String> = operation
        .iter()
        .map(|event| event.client_request_token.clone().unwrap_or_default())
        .collect();
    let first = tokens.first().cloned().unwrap_or_default();
    assert!(
        !first.is_empty() && tokens.iter().all(|carried| *carried == first),
        "case \"executing it\": every event of the execution carries the one token the \
         service assigned it, got {tokens:?}"
    );
    // The resource the change set's template declares was provisioned by the execution,
    // and its events carry that same token.
    assert!(
        operation
            .iter()
            .any(|event| event.logical_resource_id.as_deref() == Some("Probe")),
        "case \"executing it\": the execution provisions the template's resources, got \
         {operation:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: the expansion is bounded.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_whose_template_cannot_be_deployed_fails_and_acts_on_no_resource() {
    let router = router();
    const DEPLOYED: &str = r#"{"Resources":{"Keeper":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":"keep"}}}}"#;
    // A macro template: this service cannot compute its processed stage, so the resources
    // the update would deploy are UNKNOWN — not "none".
    const UNPROCESSABLE: &str = r#"{"Transform":"MyOwnMacro","Resources":{"Keeper":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":"keep"}}}}"#;

    let (status, body) = create_stack(&router, "unknowable", DEPLOYED, &[]).await;
    assert_eq!(
        status, 200,
        "case \"the stack to update\": expected success, got {status} {body}"
    );
    wait_for_the_transition().await;

    let mut members = vec![
        member("StackName", "unknowable"),
        member("TemplateBody", UNPROCESSABLE),
    ];
    members.extend(capability_members(&[CAPABILITY_AUTO_EXPAND]));
    let (status, body) = call(&router, "UpdateStack", &members).await;
    assert_eq!(
        status, 200,
        "case \"an update whose processed stage is unavailable\": expected success, got \
         {status} {body}"
    );
    wait_for_the_transition().await;

    // The update acted on no resource and ended in the modelled failure state: an unknown
    // resource set is not an empty one, so nothing is reported deleted that the request
    // never asked to delete, and nothing is reported complete that was not deployed.
    let events = describe_stack_events(&router, "unknowable").await;
    assert_eq!(
        status_and_logical_id(&one_operation(&events)),
        vec![
            (UPDATE_FAILED.to_string(), "unknowable".to_string()),
            (UPDATE_IN_PROGRESS.to_string(), "unknowable".to_string()),
        ],
        "case \"an update whose processed stage is unavailable\": the stack's own two events, \
         ending in the failure, got {events:?}"
    );
    assert_failed_with_a_reason(
        &router,
        "an update whose processed stage is unavailable",
        "unknowable",
        UPDATE_FAILED,
    )
    .await;

    // And the resource the first template deployed is still there: deploying a template
    // that declares the same resource acts on nothing. The stack's update FAILED and was
    // left standing, so this one asks for that state to go on being preserved — which is
    // what such a stack takes an update from.
    let (status, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "unknowable"),
            member("TemplateBody", DEPLOYED),
            member("DisableRollback", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"an update back to the deployed template\": expected success, got {status} {body}"
    );
    wait_for_the_transition().await;
    let events = describe_stack_events(&router, "unknowable").await;
    assert_eq!(
        status_and_logical_id(&one_operation(&events)),
        vec![
            (UPDATE_COMPLETE.to_string(), "unknowable".to_string()),
            (UPDATE_IN_PROGRESS.to_string(), "unknowable".to_string()),
        ],
        "case \"an update back to the deployed template\": the resource was kept, so \
         nothing is acted on, got {events:?}"
    );

    // A YAML body declares the same resource, and this service's reader reads YAML: the
    // update deploys the same resource set, so it acts on no resource either — what is
    // asserted here is that re-spelling a template changes nothing about its resources.
    const YAML: &str =
        "Resources:\n  Keeper:\n    Type: AWS::SSM::Parameter\n    Properties:\n      Type: String\n      Value: keep\n";
    let (status, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "unknowable"),
            member("TemplateBody", YAML),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"an update to a yaml template\": expected success, got {status} {body}"
    );
    wait_for_the_transition().await;
    let events = describe_stack_events(&router, "unknowable").await;
    assert_eq!(
        status_and_logical_id(&one_operation(&events)),
        vec![
            (UPDATE_COMPLETE.to_string(), "unknowable".to_string()),
            (UPDATE_IN_PROGRESS.to_string(), "unknowable".to_string()),
        ],
        "case \"an update to a yaml template\": the same resources, differently spelled, \
         are not resources to act on, got {events:?}"
    );
}

/// `levels` nested `Fn::ForEach` loops, each over `items` literal items, around one
/// resource: the shape whose cost is the PRODUCT of the nesting rather than the size of
/// the body that asks for it.
fn nested_loops(levels: usize, items: usize) -> String {
    // The leaf's logical id reads EVERY enclosing loop's identifier, so each iteration
    // emits a distinct key: what refuses this template is the expansion ceiling itself,
    // not the duplicate-key refusal a constant key would trip first.
    let suffix: String = (0..levels).map(|level| format!("${{I{level}}}")).collect();
    let mut inner =
        format!(r#"{{"Leaf{suffix}":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}}"#);
    for level in (0..levels).rev() {
        let collection: Vec<String> = (0..items).map(|item| format!("\"i{item}\"")).collect();
        inner = format!(
            r#"{{"Fn::ForEach::L{level}":["I{level}",[{}],{inner}]}}"#,
            collection.join(",")
        );
    }
    format!(r#"{{"Transform":"AWS::LanguageExtensions","Resources":{inner}}}"#)
}

/// A template declaring `depth` levels of plain nested objects inside one resource's
/// properties — deeper than the expander descends, but shallow enough for the JSON parser.
fn deeply_nested_properties(depth: usize) -> String {
    let mut properties = r#""leaf""#.to_string();
    for _ in 0..depth {
        properties = format!(r#"{{"a":{properties}}}"#);
    }
    format!(
        r#"{{"Transform":"AWS::LanguageExtensions","Resources":{{"Deep":{{"Type":"AWS::SSM::Parameter","Properties":{properties}}}}}}}"#
    )
}

/// `levels` nested `Fn::ToJsonString` directives around one string: each level renders the
/// level below INTO a JSON string, which escapes it, so the text doubles per level while
/// the body that asks for it grows by twenty bytes.
fn nested_to_json_string(levels: usize) -> String {
    let mut value = r#""payload""#.to_string();
    for _ in 0..levels {
        value = format!(r#"{{"Fn::ToJsonString":{value}}}"#);
    }
    format!(
        r#"{{"Transform":"AWS::LanguageExtensions","Outputs":{{"Rendered":{{"Value":{value}}}}}}}"#
    )
}

/// One row of the unprocessable table: a template that declares the transform this service
/// applies but asks for something it cannot resolve or cannot afford.
struct UnprocessableCase {
    case: &'static str,
    stack_name: String,
    template: String,
}

/// The class: the processed stage exists only when every directive could be applied within
/// the expander's stated bounds. The rows span the ways that fails — a malformed directive,
/// one whose argument is not resolvable from the template alone, a document deeper than the
/// expander descends, and loops whose cross-product passes the expansion ceiling. (A
/// directive whose ARGUMENT could never be what it takes is refused when the template is
/// submitted instead, so it has no stack to report a stage for: see
/// `template_validation.rs`.)
fn unprocessable_cases() -> Vec<UnprocessableCase> {
    let rows: Vec<(&'static str, &'static str, String)> = vec![
        (
            "a ForEach with the wrong number of parts",
            "bad-arity",
            r#"{"Transform":"AWS::LanguageExtensions","Resources":{"Fn::ForEach::Bad":["Suffix",["Alpha"]]}}"#.to_string(),
        ),
        (
            // "The collection ... can be a list or a reference to a list": a reference to a
            // parameter whose value is NOT a list resolves to a scalar, which is no
            // collection to iterate.
            "a ForEach whose collection references a parameter that is not a list",
            "bad-collection",
            r#"{"Transform":"AWS::LanguageExtensions","Parameters":{"Names":{"Type":"String","Default":"a,b"}},"Resources":{"Fn::ForEach::Params":["Name",{"Ref":"Names"},{"P${Name}":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}]}}"#.to_string(),
        ),
        (
            "a ForEach whose collection items are objects",
            "bad-items",
            r#"{"Transform":"AWS::LanguageExtensions","Resources":{"Fn::ForEach::Params":["Name",[{"a":1}],{"P${Name}":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}]}}"#.to_string(),
        ),
        (
            "a ForEach whose output map is not a map",
            "bad-output",
            r#"{"Transform":"AWS::LanguageExtensions","Resources":{"Fn::ForEach::Params":["Name",["one"],"not-a-map"]}}"#.to_string(),
        ),
        (
            "a document nested deeper than the expander descends",
            "too-deep",
            deeply_nested_properties(template::MAX_EXPANSION_DEPTH + 8),
        ),
        (
            "loops whose cross-product passes the expansion ceiling",
            "too-big",
            nested_loops(4, 50),
        ),
        (
            // Rendering a document into a JSON string escapes it, so one of these nested
            // inside another doubles its own input per level: the budget is what stops it.
            "Fn::ToJsonString nested far enough to amplify",
            "too-rendered",
            nested_to_json_string(40),
        ),
        (
            // `&{Identifier}` is ForEach's other documented reference form, which this
            // service does not apply: leaving it literal would emit one output key for
            // every item, so the stage is reported unavailable instead.
            "a loop reading the identifier through the &{} form",
            "foreach-alphanumeric",
            r#"{"Transform":"AWS::LanguageExtensions","Resources":{"Fn::ForEach::Names":["Name",["a-1","b-2"],{"Node&{Name}":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}]}}"#
                .to_string(),
        ),
        (
            // A loop whose output key does not vary per item would emit one key twice.
            // CloudFormation refuses a template that declares one logical id twice, so the
            // expansion is refused rather than one declaration quietly overwriting the
            // other.
            "a loop whose output key is the same for every item",
            "foreach-duplicate",
            r#"{"Transform":"AWS::LanguageExtensions","Resources":{"Fn::ForEach::Names":["Name",["a","b"],{"Same":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}]}}"#
                .to_string(),
        ),
        (
            // The same collision against a SIBLING declaration rather than another
            // iteration of the loop.
            "a loop whose entry collides with a plainly declared resource",
            "foreach-collides",
            r#"{"Transform":"AWS::LanguageExtensions","Resources":{"Plain":{"Type":"AWS::CloudFormation::WaitConditionHandle"},"Fn::ForEach::Names":["Name",["x"],{"Plain":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":"v"}}}]}}"#
                .to_string(),
        ),
    ];
    rows.into_iter()
        .map(|(case, stack_name, template)| UnprocessableCase {
            case,
            stack_name: stack_name.to_string(),
            template,
        })
        .collect()
}

#[tokio::test]
async fn a_template_this_service_cannot_expand_within_its_bounds_has_no_processed_stage() {
    let router = router();
    let cases = unprocessable_cases();

    for case in &cases {
        // Every row is well under the documented submitted-body maximum: it is the
        // EXPANSION, not the request, that the bounds are about.
        assert!(
            case.template.len() < 51_200,
            "case {:?}: the row's own body must be a small one, got {} bytes",
            case.case,
            case.template.len()
        );
        let (status, body) =
            create_stack_acknowledging_macros(&router, &case.stack_name, &case.template).await;
        assert_eq!(
            status, 200,
            "case {:?}: the stack is still created, got {status} {body}",
            case.case
        );

        let (status, body) = get_template(&router, &case.stack_name, Some("Original")).await;
        assert_eq!(
            status, 200,
            "case {:?}: the submitted template is still readable, got {status} {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "TemplateBody").as_deref(),
            Some(case.template.as_str()),
            "case {:?}: the Original stage is the submitted body, got {body}",
            case.case
        );

        for stage in [Some("Processed"), None] {
            let (status, body) = get_template(&router, &case.stack_name, stage).await;
            assert_eq!(
                status, 501,
                "case {:?}: GetTemplate({stage:?}) has nothing honest to answer, got \
                 {status} {body}",
                case.case
            );
        }
        let (_, body) = get_template(&router, &case.stack_name, Some("Original")).await;
        assert_eq!(
            xml_texts(&body, "member"),
            vec!["Original".to_string()],
            "case {:?}: only the stage this read can serve is advertised, got {body}",
            case.case
        );
    }

    // And nothing was provisioned from a document this service refused to expand: the
    // create ends in the modelled failure state with the reason instead.
    wait_for_the_transition().await;
    for case in &cases {
        let events = describe_stack_events(&router, &case.stack_name).await;
        assert!(
            events
                .iter()
                .all(|event| event.resource_type.as_deref() == Some(STACK_RESOURCE_TYPE)),
            "case {:?}: a template that was not expanded provisions no resources, got \
             {events:?}",
            case.case
        );
        assert_failed_with_a_reason(&router, case.case, &case.stack_name, CREATE_FAILED).await;
    }
}

/// A template whose loop declares `count` resources, in a body far smaller than the
/// resources it asks for.
fn template_declaring_resources(count: usize) -> String {
    let collection: Vec<String> = (0..count).map(|index| index.to_string()).collect();
    format!(
        r#"{{"Transform":"AWS::LanguageExtensions","Resources":{{"Fn::ForEach::Many":["N",[{}],{{"R${{N}}":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}}]}}}}"#,
        collection.join(",")
    )
}

#[tokio::test]
async fn the_resources_a_template_deploys_are_bounded_by_the_documented_quota() {
    let router = router();

    // The documented per-template resource quota, at the bound and one past it — measured
    // on the PROCESSED document, which is what a transform's expansion can grow. A
    // template past the quota is not one CloudFormation accepts, so this service reads it
    // as declaring nothing at all rather than deploying part of it.
    for (case, count, within_the_quota) in [
        ("one resource", 1, true),
        (
            "at the published quota",
            template::MAX_RESOURCES_PER_TEMPLATE,
            true,
        ),
        (
            "one past the published quota",
            template::MAX_RESOURCES_PER_TEMPLATE + 1,
            false,
        ),
    ] {
        let name = format!("quota-{count}");
        let (status, body) =
            create_stack_acknowledging_macros(&router, &name, &template_declaring_resources(count))
                .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateStack to succeed, got {status} {body}"
        );

        // The expansion itself is bounded well above the quota, so the processed stage is
        // served either way: the quota is about what the template DEPLOYS.
        let (status, body) = get_template(&router, &name, Some("Processed")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the processed stage is served, got {status} {body}"
        );
        let processed = template_body_json(case, &body);
        let declared = processed
            .get("Resources")
            .and_then(serde_json::Value::as_object)
            .map_or(0, serde_json::Map::len);
        assert_eq!(
            declared, count,
            "case {case:?}: the processed template declares {count} resources, got {declared}"
        );

        wait_for_the_transition().await;
        let events = describe_stack_events(&router, &name).await;
        let provisioned = events
            .iter()
            .filter(|event| {
                event.resource_status.as_deref() == Some(CREATE_COMPLETE)
                    && event.resource_type.as_deref() != Some(STACK_RESOURCE_TYPE)
            })
            .count();
        // One response carries one bounded page of events, so what this asserts is
        // whether the template deployed resources AT ALL — a template past the quota is
        // not one this service reads, so the create it opened ends in the modelled
        // failure state and provisions nothing.
        if within_the_quota {
            assert!(
                provisioned > 0,
                "case {case:?}: a template within the quota provisions its resources, got \
                 {events:?}"
            );
        } else {
            assert_eq!(
                provisioned, 0,
                "case {case:?}: a template past the quota provisions nothing, got {events:?}"
            );
            assert_failed_with_a_reason(&router, case, &name, CREATE_FAILED).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3, the other half: an operation reports resource events for the work it
// CARRIED OUT, so one that failed and was unwound reports no creation at all.
// ---------------------------------------------------------------------------

/// A trust policy IAM accepts, so the role in the template below is refused for its NAME
/// rather than for its document.
fn trust_policy() -> serde_json::Value {
    serde_json::json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "glue.amazonaws.com"},
            "Action": "sts:AssumeRole"
        }]
    })
}

/// A template whose FIRST resource this service can create and whose second one the owning
/// service refuses, so the create gets part-way and then fails: the logical ids are chosen
/// so the parameter is deployed before the role whichever order the reader yields.
fn template_whose_second_resource_is_refused(parameter: &str, role: &str) -> String {
    serde_json::json!({
        "Resources": {
            "AParam": {
                "Type": "AWS::SSM::Parameter",
                "Properties": {"Name": parameter, "Type": "String", "Value": "made"}
            },
            "ZRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": role,
                    "AssumeRolePolicyDocument": trust_policy()
                }
            }
        }
    })
    .to_string()
}

/// RULE: the resource events an operation reports are the ones its OUTCOME supports — one
/// record per transition a resource really went through, and none for a transition it did
/// not. A create whose second resource the owning service refuses reports the first one as
/// created (it is), the refused one as `CREATE_FAILED` with the refusal as its reason, and
/// never the refused one as created.
///
/// The rows differ along what the rule generalises over: whether the failed create is
/// unwound (the default rollback, which removes what it had made) or left as it failed
/// (`DisableRollback`), which are the two terminal states a refused create can end in while
/// keeping the stack.
#[tokio::test]
async fn a_create_that_failed_reports_no_resource_as_created() {
    for (case, stack, extra, terminal) in [
        (
            "the default rollback",
            "unwound",
            Vec::new(),
            status::ROLLBACK_COMPLETE,
        ),
        (
            "DisableRollback leaves the failed state",
            "left-as-it-failed",
            vec![member("DisableRollback", "true")],
            CREATE_FAILED,
        ),
    ] {
        let router = router();
        let role = format!("{stack}-role");
        // The clash: the role NAME is already taken in IAM, so the create's second
        // resource is refused by the service that owns it.
        let (status, body) = iam(
            &router,
            "CreateRole",
            &[
                ("RoleName", role.as_str()),
                ("AssumeRolePolicyDocument", &trust_policy().to_string()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the role that takes the name must be created, got {status} {body}"
        );

        let mut members = acknowledging_iam(&extra);
        members.push(member("StackName", stack));
        members.push(member(
            "TemplateBody",
            &template_whose_second_resource_is_refused(&format!("{stack}-param"), &role),
        ));
        members.push(member("ClientRequestToken", &format!("{stack}-token")));
        let (status, body) = call(&router, "CreateStack", &members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateStack to be accepted, got {status} {body}"
        );
        wait_for_the_transition().await;

        assert_failed_with_a_reason(&router, case, stack, terminal).await;

        let events = describe_stack_events(&router, stack).await;
        // The REFUSED resource is never reported as created: nothing of it exists in the
        // service that owns it.
        let claimed: Vec<&Event> = events
            .iter()
            .filter(|event| {
                event.logical_resource_id.as_deref() == Some("ZRole")
                    && event.resource_status.as_deref() == Some(CREATE_COMPLETE)
            })
            .collect();
        assert!(
            claimed.is_empty(),
            "case {case:?}: the resource the owning service refused is never reported as \
             created, got {claimed:?}"
        );
        // What it reports for that resource is the refusal, with the owning service's own
        // message as the reason.
        let refused: Vec<&Event> = events
            .iter()
            .filter(|event| {
                event.logical_resource_id.as_deref() == Some("ZRole")
                    && event.resource_status.as_deref() == Some(CREATE_FAILED)
            })
            .collect();
        assert!(
            refused
                .iter()
                .any(|event| event.resource_status_reason.as_deref().is_some_and(|reason| !reason.is_empty())),
            "case {case:?}: the refused resource's event says why it failed, got {events:?}"
        );
        // What it DOES report is the stack's own account of the failure, every event of the
        // operation carrying the token the request named.
        let operation = one_operation(&events);
        assert!(
            operation
                .iter()
                .any(|event| event.resource_status.as_deref() == Some(CREATE_FAILED)
                    || event.resource_status.as_deref() == Some(terminal)),
            "case {case:?}: the stack's events say the create failed, got {events:?}"
        );
        assert!(
            events
                .iter()
                .all(|event| event.client_request_token.is_some()),
            "case {case:?}: every event carries the operation's client request token, got \
             {events:?}"
        );
    }
}
