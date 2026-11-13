// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack provisions the SageMaker model package group its template
//! declares — with the resource policy declared for it — and deleting the stack deletes
//! the group again.
//!
//! The rule under test: **creating a stack calls the owning service's own create API once
//! per resource the template's `Resources` section declares, and a property that is not a
//! member of that create is applied through the operation that owns it.** An
//! `AWS::SageMaker::ModelPackageGroup` is the case where the second half bites:
//! `ModelPackageGroupName`, `ModelPackageGroupDescription` and `Tags` are members of
//! `CreateModelPackageGroup`, but `ModelPackageGroupPolicy` is not — SageMaker attaches a
//! model group's resource policy through `PutModelPackageGroupPolicy` — so a stack that
//! deploys a model registry has to make both calls for the group it declares to be
//! readable through `GetModelPackageGroupPolicy`.
//!
//! Two crates have to agree for that to hold (CloudFormation makes the calls; SageMaker
//! serves them), so the suite lives here rather than in either crate.
//!
//! The rows differ along what the rule generalises over: how the template spells the
//! policy (the JSON document itself, or the text of one), whether the group's name is a
//! literal, a `Ref` to a parameter or left for CloudFormation to generate, and which of the
//! type's other properties the template declares. Every expected value is computed here
//! from what the row declares, never read back out of the answer being checked.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery encoder and the XML/Outputs readers this suite needs already exist in the
// test binary's shared modules; they are shared rather than copied.
use crate::cloudformation_provisions_stack_resources::{outputs_of, xml_text};
use crate::cloudformation_stack_clock::settle_the_stacks;
use crate::query_wire::{elements, form_encode};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    failed_action, resource_failure_summary,
};

/// The region every request here is signed for, and so the region the stack's resources
/// and their ARNs belong to.
const REGION: &str = "us-west-2";

/// The account id every ARN this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// Long enough for the stack's clock-owed transition (`STATUS_TRANSITION_DELAY`, 500 ms)
/// to come due. Creating a model group is synchronous in SageMaker, so the stack's span is
/// the only one to wait for.
const PAST_STACK_SPAN: Duration = Duration::from_millis(900);

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    build_router_and_engine().0
}

/// [`build_router`] with the engine it was mounted on, for a case that drives the stack's own
/// CLOCK rather than waiting for it
/// ([`crate::cloudformation_stack_clock::settle_the_stacks`]).
fn build_router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: this router runs with request
/// authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire.
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
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one SageMaker operation over its awsJson1_1 wire.
async fn sagemaker(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("api.sagemaker.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("SageMaker.{operation}"))
        .header("authorization", authorization("sagemaker"))
        .body(Bytes::from(request.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// Create a stack from `template` and answer its id, failing the case with the response
/// body when the create is refused.
async fn create_stack(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
    parameters: &[(&str, &str)],
) -> String {
    let mut request: Vec<(&str, &str)> =
        vec![("StackName", stack_name), ("TemplateBody", template)];
    // `Parameters.member.N.ParameterKey` / `.ParameterValue`, as an SDK flattens the list.
    let flattened: Vec<(String, String)> = parameters
        .iter()
        .enumerate()
        .flat_map(|(index, (key, value))| {
            let position = index + 1;
            [
                (
                    format!("Parameters.member.{position}.ParameterKey"),
                    (*key).to_string(),
                ),
                (
                    format!("Parameters.member.{position}.ParameterValue"),
                    (*value).to_string(),
                ),
            ]
        })
        .collect();
    for (key, value) in &flattened {
        request.push((key.as_str(), value.as_str()));
    }
    let (status, body) = cloudformation(router, "CreateStack", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    xml_text(&body, "StackId").unwrap_or_else(|| panic!("case {case:?}: no <StackId> in {body}"))
}

/// The one stack `stack_name` names, as the response body.
async fn describe_stack(router: &MultiServiceRouter, case: &str, stack_name: &str) -> String {
    let (status, body) =
        cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    body
}

/// The ARN a SageMaker model package group of `name` carries in this emulator, as the
/// modelled `ModelPackageGroupArn` pattern spells it.
fn group_arn(name: &str) -> String {
    format!("arn:aws:sagemaker:{REGION}:{ACCOUNT_ID}:model-package-group/{name}")
}

/// A resource policy document for `name`, as the JSON VALUE a template declares under
/// `ModelPackageGroupPolicy`.
fn policy_document(name: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowDescribe",
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT_ID}:root") },
            "Action": ["sagemaker:DescribeModelPackage", "sagemaker:ListModelPackages"],
            "Resource": group_arn(name)
        }]
    })
}

/// One row of the table: a template, and what the rule says the stack must then have
/// created in SageMaker.
struct Row {
    case: &'static str,
    /// The template body, built from the stack name the row runs under.
    template: fn(&str) -> String,
    /// The parameters the create supplies.
    parameters: &'static [(&'static str, &'static str)],
    /// The group's name, where the row fixes it; `None` where the template leaves the name
    /// to CloudFormation and the stack's `GroupName` output is what names it.
    expected_name: fn(&str) -> Option<String>,
    /// The policy document the template declared, as the TEXT `GetModelPackageGroupPolicy`
    /// must report: the document's own serialization where the template spelled the
    /// document, and the exact text where it spelled text.
    expected_policy: fn(&str) -> String,
    /// The description the template declared, where it declared one.
    expected_description: Option<&'static str>,
}

/// The rows: one axis per thing the rule generalises over.
const ROWS: &[Row] = &[
    Row {
        case: "the policy as the JSON document itself, on a literally named group",
        template: |stack| {
            json!({
                "Resources": {
                    "Registry": {
                        "Type": "AWS::SageMaker::ModelPackageGroup",
                        "Properties": {
                            "ModelPackageGroupName": format!("{stack}-registry"),
                            "ModelPackageGroupDescription": "the row's own description",
                            "ModelPackageGroupPolicy": policy_document(&format!("{stack}-registry")),
                            "Tags": [{ "Key": "owner", "Value": "registry" }]
                        }
                    }
                },
                "Outputs": {
                    "GroupName": { "Value": { "Ref": "Registry" } },
                    "GroupArn": {
                        "Value": { "Fn::GetAtt": ["Registry", "ModelPackageGroupArn"] }
                    }
                }
            })
            .to_string()
        },
        parameters: &[],
        expected_name: |stack| Some(format!("{stack}-registry")),
        expected_policy: |stack| policy_document(&format!("{stack}-registry")).to_string(),
        expected_description: Some("the row's own description"),
    },
    Row {
        case: "the policy as the TEXT of a document, on a group named by a parameter",
        template: |_| {
            json!({
                "Parameters": { "GroupName": { "Type": "String" } },
                "Resources": {
                    "Registry": {
                        "Type": "AWS::SageMaker::ModelPackageGroup",
                        "Properties": {
                            "ModelPackageGroupName": { "Ref": "GroupName" },
                            "ModelPackageGroupPolicy": "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":\"*\",\"Action\":\"sagemaker:DescribeModelPackage\",\"Resource\":\"*\"}]}"
                        }
                    }
                },
                "Outputs": {
                    "GroupName": { "Value": { "Ref": "Registry" } },
                    "GroupArn": {
                        "Value": { "Fn::GetAtt": ["Registry", "ModelPackageGroupArn"] }
                    }
                }
            })
            .to_string()
        },
        parameters: &[("GroupName", "parameter-named-registry")],
        expected_name: |_| Some("parameter-named-registry".to_string()),
        expected_policy: |_| {
            "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":\"*\",\"Action\":\"sagemaker:DescribeModelPackage\",\"Resource\":\"*\"}]}"
                .to_string()
        },
        expected_description: None,
    },
    Row {
        case: "a group CloudFormation names, written in YAML with short-form intrinsics",
        template: |_| {
            // The YAML spelling, with the policy as a nested document: the name is left
            // out entirely, so CloudFormation generates it and the stack's output is the
            // only way to learn it.
            [
                "Resources:",
                "  Registry:",
                "    Type: AWS::SageMaker::ModelPackageGroup",
                "    Properties:",
                "      ModelPackageGroupPolicy:",
                "        Version: '2012-10-17'",
                "        Statement:",
                "          - Effect: Allow",
                "            Principal: '*'",
                "            Action: sagemaker:DescribeModelPackage",
                "            Resource: '*'",
                "Outputs:",
                "  GroupName:",
                "    Value: !Ref Registry",
                "  GroupArn:",
                "    Value: !GetAtt Registry.ModelPackageGroupArn",
                "",
            ]
            .join("\n")
        },
        parameters: &[],
        expected_name: |_| None,
        expected_policy: |_| {
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "sagemaker:DescribeModelPackage",
                    "Resource": "*"
                }]
            })
            .to_string()
        },
        expected_description: None,
    },
];

/// RULE: a stack's `AWS::SageMaker::ModelPackageGroup` is created in SageMaker, with the
/// resource policy the template declared attached to it, and the stack's deletion removes
/// both.
#[tokio::test]
async fn a_stack_provisions_the_model_package_group_and_its_policy() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let stack_name = format!("registry-stack-{index}");
        let template = (row.template)(&stack_name);
        let stack_id = create_stack(&router, case, &stack_name, &template, row.parameters).await;

        // The resources are created by the call itself; the stack reports its outputs once
        // the create COMPLETES, so the case waits past the stack's clock-owed span.
        tokio::time::sleep(PAST_STACK_SPAN).await;
        let body = describe_stack(&router, case, &stack_name).await;
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to complete, got {body}"
        );
        let outputs = outputs_of(&body);
        let name = outputs
            .get("GroupName")
            .unwrap_or_else(|| panic!("case {case:?}: no GroupName output in {body}"))
            .clone();
        if let Some(expected) = (row.expected_name)(&stack_name) {
            assert_eq!(
                name, expected,
                "case {case:?}: expected the stack to name the group {expected:?}, got {name:?}"
            );
        } else {
            let prefix = format!("{stack_name}-Registry-");
            assert!(
                name.starts_with(&prefix),
                "case {case:?}: expected a generated name starting {prefix:?}, got {name:?}"
            );
        }
        assert_eq!(
            outputs.get("GroupArn").map(String::as_str),
            Some(group_arn(&name).as_str()),
            "case {case:?}: expected the stack's GroupArn output to be the group's own ARN, \
             got {outputs:?}"
        );

        // The group the template declared exists in SageMaker, as SageMaker's own create
        // would have made it.
        let (status, described) = sagemaker(
            &router,
            "DescribeModelPackageGroup",
            &json!({ "ModelPackageGroupName": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeModelPackageGroup for {name:?}, \
             got {status} with body {described}"
        );
        assert_eq!(
            described["ModelPackageGroupArn"].as_str(),
            Some(group_arn(&name).as_str()),
            "case {case:?}: expected the group's ARN to name {name:?}, got {described}"
        );
        assert_eq!(
            described["ModelPackageGroupDescription"].as_str(),
            row.expected_description,
            "case {case:?}: expected the declared description, got {described}"
        );

        // ...and the policy the template declared for it is the document the reader
        // reports, byte for byte.
        let (status, policy) = sagemaker(
            &router,
            "GetModelPackageGroupPolicy",
            &json!({ "ModelPackageGroupName": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetModelPackageGroupPolicy for {name:?}, \
             got {status} with body {policy}"
        );
        assert_eq!(
            policy["ResourcePolicy"].as_str(),
            Some((row.expected_policy)(&stack_name).as_str()),
            "case {case:?}: expected the declared policy document, got {policy}"
        );

        // Deleting the stack deletes the group it created, and the policy goes with it.
        let (status, body) =
            cloudformation(&router, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let (status, gone) = sagemaker(
            &router,
            "DescribeModelPackageGroup",
            &json!({ "ModelPackageGroupName": name }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the group to be gone after the stack's deletion, got \
             {status} with body {gone}"
        );
        let (status, no_policy) = sagemaker(
            &router,
            "GetModelPackageGroupPolicy",
            &json!({ "ModelPackageGroupName": name }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected no policy to report for the deleted group, got {status} \
             with body {no_policy}"
        );

        tokio::time::sleep(PAST_STACK_SPAN).await;
        let body = describe_stack(&router, case, &stack_id).await;
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some("DELETE_COMPLETE"),
            "case {case:?}: expected the stack's deletion to complete, got {body}"
        );
    }
}

/// RULE: a group SageMaker refuses is never passed off as created — the stack fails with
/// the refusal the owning service gave, and what a client already had is untouched.
#[tokio::test]
async fn a_group_the_owning_service_refuses_fails_the_stack() {
    let (router, engine) = build_router_and_engine();

    // Rows: a name a client already took (SageMaker's own uniqueness rule), and a name
    // outside the modelled `EntityName` pattern (`^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`).
    let taken = "already-taken-registry";
    let client_policy = policy_document(taken).to_string();
    let (status, created) = sagemaker(
        &router,
        "CreateModelPackageGroup",
        &json!({ "ModelPackageGroupName": taken }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the client's own CreateModelPackageGroup to succeed, got {status} with \
         body {created}"
    );
    let (status, attached) = sagemaker(
        &router,
        "PutModelPackageGroupPolicy",
        &json!({ "ModelPackageGroupName": taken, "ResourcePolicy": client_policy }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the client's own PutModelPackageGroupPolicy to succeed, got {status} with \
         body {attached}"
    );

    for (case, group_name) in [
        ("a template naming a group that already exists", taken),
        ("a template naming a group the model does not admit", "not a name"),
    ] {
        let template = json!({
            "Resources": {
                "Registry": {
                    "Type": "AWS::SageMaker::ModelPackageGroup",
                    "Properties": {
                        "ModelPackageGroupName": group_name,
                        "ModelPackageGroupPolicy": policy_document(group_name)
                    }
                }
            },
            "Outputs": { "GroupName": { "Value": { "Ref": "Registry" } } }
        });
        let stack_name = format!("doomed-registry-{}", group_name.replace(' ', "-"));
        let (status, created) = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", &template.to_string()),
                // The refusal these rows are about is the OWNING SERVICE's, so the create has
                // to reach it: with the pre-deployment validations on, CloudFormation's own
                // existence check fails a create whose declared name that service already
                // holds before any resource is asked for.
                ("DisableValidation", "true"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {created}"
        );
        // The create failed, so the stack is being rolled back. Both transitions it owes are
        // on the stack's own clock, so the case drives that clock rather than waiting for it
        // ([`crate::cloudformation_stack_clock::settle_the_stacks`]).
        settle_the_stacks(&router, &engine, case).await;
        settle_the_stacks(&router, &engine, case).await;

        let body = describe_stack(&router, case, &stack_name).await;
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some("ROLLBACK_COMPLETE"),
            "case {case:?}: expected the failed create to be rolled back, got {body}"
        );
        // The STACK's reason summarises which resource failed; the service's own refusal
        // is reported on that resource's event.
        let reason = xml_text(&body, "StackStatusReason").unwrap_or_default();
        assert_eq!(
            reason,
            resource_failure_summary(failed_action::CREATE, ["Registry"]),
            "case {case:?}: expected the stack reason to summarise what failed, got \
             {reason:?}"
        );
        let (_, events) =
            cloudformation(&router, "DescribeStackEvents", &[("StackName", &stack_name)]).await;
        assert!(
            elements(&events, "ResourceStatusReason")
                .iter()
                .any(|reported| reported.contains("Resource handler returned message:")),
            "case {case:?}: expected the failing resource's event to carry the service's \
             refusal, got {events}"
        );

        // The stack created nothing, so its deletion deletes nothing: the group the client
        // made still carries the policy the client attached.
        let (status, body) =
            cloudformation(&router, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let (status, policy) = sagemaker(
            &router,
            "GetModelPackageGroupPolicy",
            &json!({ "ModelPackageGroupName": taken }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the client's own group to survive, got {status} with \
             body {policy}"
        );
        assert_eq!(
            policy["ResourcePolicy"].as_str(),
            Some(client_policy.as_str()),
            "case {case:?}: expected the client's own policy to be untouched, got {policy}"
        );
    }
}
