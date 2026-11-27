// SPDX-License-Identifier: CC0-1.0

//! The ASSEMBLED emulator serves a CloudFormation stack that declares `AWS::SMSVOICE::*`
//! resources: each one is created in AWS End User Messaging SMS and Voice's own control
//! plane, the stack publishes what that service answered, an update that drops the resource
//! policy detaches it, and deleting the stack takes every resource away.
//!
//! The rule under test: **a resource type AWS publishes is one a template may declare, and a
//! declaration of it is DEPLOYED by calling the service that OWNS the type — so the handle
//! the stack publishes (`Ref`, `Fn::GetAtt Arn`) resolves in that service, and what that
//! service refuses is a FAILED RESOURCE carrying its own refusal rather than the `Template
//! format error: Unrecognized resource types` a type CloudFormation does not know is refused
//! with.**
//!
//! Three stacks, differing along what the rule generalises over: the CORE estate (a protect
//! configuration, an opt-out list, a configuration set linked to the protect configuration
//! and a resource policy on the list), the REGISTRATION estate (an RCS agent, a registration
//! and a registration attachment), and a REJECTED declaration (a configuration set naming a
//! protect configuration the service never minted), which rolls the stack back with that
//! service's own not-found sentence on the resource that failed.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::{block_of, blocks_of, form_encode, opt_element, outputs_of, xml_escape};

/// The region every request here is signed for, and so the region the stack's resources
/// belong to.
const REGION: &str = "us-east-1";

/// How long the suite is willing to wait for a stack operation to settle, and how often it
/// looks again. Generous: the suite waits for the CONDITION (the stack's status, the
/// resource's existence) rather than for a fixed time.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);
const LOOK_AGAIN_AFTER: Duration = Duration::from_millis(100);

/// The fixed text CloudFormation refuses a template declaring a type it does NOT know with,
/// which is what the defect this suite pins answered for types AWS publishes. No stack here
/// may ever see it.
const UNRECOGNIZED_TYPES: &str = "Unrecognized resource types";

/// The resource types under test.
const CONFIGURATION_SET: &str = "AWS::SMSVOICE::ConfigurationSet";
const OPT_OUT_LIST: &str = "AWS::SMSVOICE::OptOutList";
const PROTECT_CONFIGURATION: &str = "AWS::SMSVOICE::ProtectConfiguration";
const RESOURCE_POLICY: &str = "AWS::SMSVOICE::ResourcePolicy";
const RCS_AGENT: &str = "AWS::SMSVOICE::RcsAgent";
const REGISTRATION: &str = "AWS::SMSVOICE::Registration";
const REGISTRATION_ATTACHMENT: &str = "AWS::SMSVOICE::RegistrationAttachment";

/// The document `GetResourcePolicy` reports for a resource that carries no policy, which is
/// what a read answers once an update has dropped the policy resource.
const EMPTY_POLICY: &str = "{}";

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
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
        service_com_amazonaws_pinpointsmsvoicev2_pinpointsmsvoicev2::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation action over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
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
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one operation of AWS End User Messaging SMS and Voice over the `awsJson1_0` wire its
/// model declares (`POST /` with `X-Amz-Target: PinpointSMSVoiceV2.<Op>`), signed for
/// `sms-voice` — that model's own `sigv4Name`.
async fn sms_voice(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sms-voice.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("PinpointSMSVoiceV2.{operation}"))
        .header("authorization", authorization("sms-voice"))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, answered)
}

/// The `DescribeStacks` body once the stack rests in a status that is not in progress, or the
/// last one read when the deadline passes. Waits for the CONDITION, looking again until the
/// stack has settled.
async fn settled_stack(router: &MultiServiceRouter, case: &str, stack_name: &str) -> String {
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, body) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
        );
        let settled = opt_element(&body, "StackStatus")
            .is_some_and(|stack_status| !stack_status.ends_with("_IN_PROGRESS"));
        if settled || std::time::Instant::now() >= deadline {
            return body;
        }
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}

/// Create one stack and require that it was ACCEPTED: a type AWS publishes is one a template
/// may declare, so the refusal this suite exists to pin must not be the answer.
async fn create_stack(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &Value,
) {
    let body = template.to_string();
    let (status, created) = cloudformation(
        router,
        "CreateStack",
        &[
            ("StackName", stack_name),
            ("TemplateBody", &body),
            ("Tags.member.1.Key", "sim2real"),
            ("Tags.member.1.Value", stack_name),
        ],
    )
    .await;
    assert!(
        !created.contains(UNRECOGNIZED_TYPES),
        "case {case:?}: expected every declared type to be recognised, got {created}"
    );
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {created}"
    );
}

/// One output of a settled stack, or a panic naming what was missing.
fn output(case: &str, body: &str, name: &str) -> String {
    outputs_of(body)
        .get(name)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no {name} output in {body}"))
}

/// One string member of an answer document, or a panic naming what was missing.
fn member(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a member {name} in {body}"))
        .to_string()
}

/// The single record a narrowed read of that service reports, or `None` where it reports
/// none.
async fn reported(
    router: &MultiServiceRouter,
    operation: &str,
    selection: &str,
    handle: &str,
    reported_member: &str,
) -> Option<Value> {
    let (status, answered) = sms_voice(router, operation, json!({ selection: [handle] })).await;
    if status != 200 {
        return None;
    }
    answered
        .get(reported_member)
        .and_then(Value::as_array)
        .and_then(|records| records.first().cloned())
}

/// Wait until that service no longer holds the resource `handle` names, which is what makes a
/// stack's resources the stack's own rather than leftovers.
async fn released(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    selection: &str,
    handle: &str,
) {
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, answered) =
            sms_voice(router, operation, json!({ selection: [handle] })).await;
        // The wire error code is the last segment of the awsJson `__type`, which is what a
        // client branches on.
        let code = answered
            .get("__type")
            .and_then(Value::as_str)
            .map(|raw| raw.rsplit('#').next().unwrap_or(raw).to_string());
        if status == 400 && code.as_deref() == Some("ResourceNotFoundException") {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: expected {handle:?} to be released with the stack, {operation} still \
             answers {status} with {answered}"
        );
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}

#[tokio::test]
async fn the_assembled_emulator_deploys_the_core_smsvoice_estate_and_takes_it_away() {
    let router = build_router();
    let case = "a protect configuration, an opt-out list, a linked configuration set and a \
                resource policy";
    let stack_name = "smsvoice-core-router";
    let set_name = "smsvoice-core-set";
    let list_name = "smsvoice-core-list";
    let policy_document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "ShareTheList",
            "Effect": "Allow",
            "Principal": {"AWS": "123456789012"},
            "Action": "sms-voice:DescribeOptOutLists",
        }],
    });
    let resources = json!({
        "Protect": {
            "Type": PROTECT_CONFIGURATION,
            "Properties": {"DeletionProtectionEnabled": false},
        },
        "List": {
            "Type": OPT_OUT_LIST,
            "Properties": {"OptOutListName": list_name},
        },
        "Config": {
            "Type": CONFIGURATION_SET,
            "Properties": {
                "ConfigurationSetName": set_name,
                "ProtectConfigurationId": {"Ref": "Protect"},
            },
        },
        "Policy": {
            "Type": RESOURCE_POLICY,
            "Properties": {
                "ResourceArn": {"Fn::GetAtt": ["List", "Arn"]},
                "PolicyDocument": policy_document,
            },
        },
    });
    let outputs = json!({
        "ConfigRef": {"Value": {"Ref": "Config"}},
        "ConfigArn": {"Value": {"Fn::GetAtt": ["Config", "Arn"]}},
        "ListRef": {"Value": {"Ref": "List"}},
        "ListArn": {"Value": {"Fn::GetAtt": ["List", "Arn"]}},
        "ProtectRef": {"Value": {"Ref": "Protect"}},
        "ProtectArn": {"Value": {"Fn::GetAtt": ["Protect", "Arn"]}},
    });
    let template = json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": resources,
        "Outputs": outputs,
    });

    create_stack(&router, case, stack_name, &template).await;
    let body = settled_stack(&router, case, stack_name).await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the create to complete, got {body}"
    );

    // Every published handle resolves in the owning service, under the identity that service
    // answered: the two named resources by their names, the protect configuration by the id
    // that service minted.
    let config_ref = output(case, &body, "ConfigRef");
    let config_arn = output(case, &body, "ConfigArn");
    let list_ref = output(case, &body, "ListRef");
    let list_arn = output(case, &body, "ListArn");
    let protect_ref = output(case, &body, "ProtectRef");
    let protect_arn = output(case, &body, "ProtectArn");
    assert_eq!(
        (config_ref.as_str(), list_ref.as_str()),
        (set_name, list_name),
        "case {case:?}: expected each named resource's Ref to be its declared name, got {body}"
    );

    let reported_set = reported(
        &router,
        "DescribeConfigurationSets",
        "ConfigurationSetNames",
        &config_ref,
        "ConfigurationSets",
    )
    .await
    .unwrap_or_else(|| panic!("case {case:?}: the owning service holds no configuration set"));
    assert_eq!(
        (
            member(case, &reported_set, "ConfigurationSetArn"),
            member(case, &reported_set, "ProtectConfigurationId"),
        ),
        (config_arn.clone(), protect_ref.clone()),
        "case {case:?}: expected the set to carry the published ARN and the protect \
         configuration the template linked, got {reported_set}"
    );

    let reported_list = reported(
        &router,
        "DescribeOptOutLists",
        "OptOutListNames",
        &list_ref,
        "OptOutLists",
    )
    .await
    .unwrap_or_else(|| panic!("case {case:?}: the owning service holds no opt-out list"));
    assert_eq!(
        member(case, &reported_list, "OptOutListArn"),
        list_arn,
        "case {case:?}: expected the list to carry the published ARN, got {reported_list}"
    );

    let reported_protect = reported(
        &router,
        "DescribeProtectConfigurations",
        "ProtectConfigurationIds",
        &protect_ref,
        "ProtectConfigurations",
    )
    .await
    .unwrap_or_else(|| {
        panic!("case {case:?}: the owning service holds no protect configuration")
    });
    assert_eq!(
        member(case, &reported_protect, "ProtectConfigurationArn"),
        protect_arn,
        "case {case:?}: expected the protect configuration to carry the published ARN, got \
         {reported_protect}"
    );

    // The policy the stack attached is the policy that service holds against the list.
    let (status, policy) = sms_voice(
        &router,
        "GetResourcePolicy",
        json!({"ResourceArn": list_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the policy to be readable, got {status} with {policy}"
    );
    let attached = member(case, &policy, "Policy");
    assert!(
        attached.contains("ShareTheList"),
        "case {case:?}: expected the declared document to be attached, got {attached:?}"
    );

    // An update that DROPS the policy resource detaches the document, which the read then
    // answers as the empty one.
    let without_policy = json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Protect": resources["Protect"].clone(),
            "List": resources["List"].clone(),
            "Config": resources["Config"].clone(),
        },
        "Outputs": outputs,
    })
    .to_string();
    let (status, updated) = cloudformation(
        &router,
        "UpdateStack",
        &[
            ("StackName", stack_name),
            ("TemplateBody", &without_policy),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with {updated}"
    );
    let body = settled_stack(&router, case, stack_name).await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("UPDATE_COMPLETE"),
        "case {case:?}: expected the update to complete, got {body}"
    );
    let (_, policy) = sms_voice(
        &router,
        "GetResourcePolicy",
        json!({"ResourceArn": list_arn}),
    )
    .await;
    assert_eq!(
        member(case, &policy, "Policy"),
        EMPTY_POLICY,
        "case {case:?}: expected the empty document once the policy resource is dropped, got \
         {policy}"
    );

    // Deleting the stack takes every resource away, so the owning service answers its own
    // not-found for each of them.
    let (status, deleted) =
        cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with {deleted}"
    );
    released(
        &router,
        case,
        "DescribeConfigurationSets",
        "ConfigurationSetNames",
        &config_ref,
    )
    .await;
    released(
        &router,
        case,
        "DescribeOptOutLists",
        "OptOutListNames",
        &list_ref,
    )
    .await;
    released(
        &router,
        case,
        "DescribeProtectConfigurations",
        "ProtectConfigurationIds",
        &protect_ref,
    )
    .await;
}

#[tokio::test]
async fn the_assembled_emulator_deploys_the_registration_smsvoice_estate_and_takes_it_away() {
    let router = build_router();
    let case = "an RCS agent, a registration and a registration attachment";
    let stack_name = "smsvoice-registration-router";
    let template = json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Agent": {
                "Type": RCS_AGENT,
                "Properties": {
                    "DeletionProtectionEnabled": false,
                    "Tags": [{"Key": "estate", "Value": "registration"}],
                },
            },
            "Registration": {
                "Type": REGISTRATION,
                "Properties": {
                    "RegistrationType": "US_TOLL_FREE_REGISTRATION",
                    "Tags": [{"Key": "estate", "Value": "registration"}],
                },
            },
            "Attachment": {
                "Type": REGISTRATION_ATTACHMENT,
                "Properties": {
                    // The type declares the document's CONTENTS, which the owning API takes
                    // as a blob: the text a template writes is not base64 of its own.
                    "AttachmentBody": "supporting evidence for the registration",
                    "Tags": [{"Key": "estate", "Value": "registration"}],
                },
            },
        },
        "Outputs": {
            "AgentRef": {"Value": {"Ref": "Agent"}},
            "AgentArn": {"Value": {"Fn::GetAtt": ["Agent", "Arn"]}},
            "RegistrationRef": {"Value": {"Ref": "Registration"}},
            "RegistrationArn": {"Value": {"Fn::GetAtt": ["Registration", "Arn"]}},
            "AttachmentRef": {"Value": {"Ref": "Attachment"}},
            "AttachmentArn": {"Value": {"Fn::GetAtt": ["Attachment", "Arn"]}},
        },
    });

    create_stack(&router, case, stack_name, &template).await;
    let body = settled_stack(&router, case, stack_name).await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the create to complete, got {body}"
    );

    // Each of the three is read back through that service's own listing, by the identifier it
    // minted, and carries the ARN the stack published.
    let reads: [(&str, &str, &str, &str, &str, &str); 3] = [
        (
            "AgentRef",
            "AgentArn",
            "DescribeRcsAgents",
            "RcsAgentIds",
            "RcsAgents",
            "RcsAgentArn",
        ),
        (
            "RegistrationRef",
            "RegistrationArn",
            "DescribeRegistrations",
            "RegistrationIds",
            "Registrations",
            "RegistrationArn",
        ),
        (
            "AttachmentRef",
            "AttachmentArn",
            "DescribeRegistrationAttachments",
            "RegistrationAttachmentIds",
            "RegistrationAttachments",
            "RegistrationAttachmentArn",
        ),
    ];
    let mut handles = Vec::new();
    for (ref_output, arn_output, operation, selection, reported_member, arn_member) in reads {
        let handle = output(case, &body, ref_output);
        let arn = output(case, &body, arn_output);
        let record = reported(&router, operation, selection, &handle, reported_member)
            .await
            .unwrap_or_else(|| {
                panic!("case {case:?}: the owning service holds nothing for {handle:?}")
            });
        assert_eq!(
            member(case, &record, arn_member),
            arn,
            "case {case:?}: expected {operation} to report the published ARN, got {record}"
        );
        // The tags the resource carries are the declaration's own beside the ones
        // CloudFormation propagates from the stack, read back through that service's own tag
        // listing by the ARN the stack published.
        let (status, tags) = sms_voice(&router, "ListTagsForResource", json!({"ResourceArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the tags of {arn:?} to be readable, got {status} with {tags}"
        );
        let carried: Vec<(String, String)> = tags
            .get("Tags")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| (member(case, entry, "Key"), member(case, entry, "Value")))
                    .collect()
            })
            .unwrap_or_default();
        for expected in [
            ("estate".to_string(), "registration".to_string()),
            ("sim2real".to_string(), stack_name.to_string()),
        ] {
            assert!(
                carried.contains(&expected),
                "case {case:?}: expected {arn:?} to carry {expected:?}, got {carried:?}"
            );
        }
        handles.push((operation, selection, handle));
    }

    let (status, deleted) =
        cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with {deleted}"
    );
    for (operation, selection, handle) in &handles {
        released(&router, case, operation, selection, handle).await;
    }
}

#[tokio::test]
async fn a_configuration_set_naming_an_unminted_protect_configuration_rolls_the_stack_back() {
    let router = build_router();
    let case = "a configuration set linked to a protect configuration nobody created";
    let stack_name = "smsvoice-rejected-router";
    // An id of the form that service mints, which this account holds no configuration for.
    let unminted = "protect-configuration-00000000000000000000000000000001";
    let template = json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Config": {
                "Type": CONFIGURATION_SET,
                "Properties": {
                    "ConfigurationSetName": "smsvoice-rejected-set",
                    "ProtectConfigurationId": unminted,
                },
            },
        },
    });

    create_stack(&router, case, stack_name, &template).await;
    let body = settled_stack(&router, case, stack_name).await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("ROLLBACK_COMPLETE"),
        "case {case:?}: expected the stack to roll back, got {body}"
    );

    // The reason the failed resource carries is the owning service's OWN refusal, relayed as
    // a registry-handler reason: its not-found sentence naming the kind and the id the
    // template stated, the SDK's diagnostic for that service, and the handler class the
    // refusal falls in.
    let (status, events) =
        cloudformation(&router, "DescribeStackEvents", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStackEvents, got {status} with {events}"
    );
    let failed: Vec<String> = blocks_of(&events, "member")
        .into_iter()
        .filter(|event| {
            opt_element(event, "ResourceStatus").as_deref() == Some("CREATE_FAILED")
                && opt_element(event, "LogicalResourceId").as_deref() == Some("Config")
        })
        .filter_map(|event| opt_element(event, "ResourceStatusReason"))
        .collect();
    let reason = failed
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: no CREATE_FAILED event for Config in {events}"));
    let expected_sentence = xml_escape(&format!(
        "Resource Not Found - ResourceType=\"protect-configuration\" ResourceId=\"{unminted}\""
    ));
    assert!(
        reason.contains(&expected_sentence),
        "case {case:?}: expected the reason to quote {expected_sentence:?}, got {reason:?}"
    );
    for fragment in [
        "Resource handler returned message:",
        "(Service: PinpointSmsVoiceV2, Status Code: 400, Request ID:",
        "(SDK Attempt Count:",
        "HandlerErrorCode: NotFound",
    ] {
        assert!(
            reason.contains(fragment),
            "case {case:?}: expected the reason to carry {fragment:?}, got {reason:?}"
        );
    }

    // Nothing is left behind: the set the failed resource would have created is gone with the
    // rollback, so that service holds no configuration set at all.
    let (_, sets) = sms_voice(&router, "DescribeConfigurationSets", json!({})).await;
    assert_eq!(
        sets.get("ConfigurationSets")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0),
        0,
        "case {case:?}: expected the rollback to leave no configuration set, got {sets}"
    );
    // And the stack itself reports the rollback rather than a template-format error.
    assert!(
        block_of(&body, "Stacks").is_some(),
        "case {case:?}: expected a described stack, got {body}"
    );
}
