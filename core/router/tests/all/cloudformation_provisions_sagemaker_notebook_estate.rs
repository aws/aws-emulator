// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack provisions the Amazon SageMaker AI estate its template declares —
//! the model, the notebook instance and the lifecycle configuration that instance runs — in
//! SageMaker itself, and deleting the stack takes each of them away again.
//!
//! The rule under test: **a stack reports a resource created only once the service that owns
//! the type created it, so every identifier the stack publishes for one resolves in that
//! service, and a resource whose owning service answers a non-terminal status holds the
//! stack's operation open until that service reports it ready.** An
//! `AWS::SageMaker::Model`, an `AWS::SageMaker::NotebookInstance` and an
//! `AWS::SageMaker::NotebookInstanceLifecycleConfig` are each addressed by their NAME in
//! every SageMaker call that reaches them, and `CreateNotebookInstance` answers the instance
//! `Pending` — so a stack that completed must leave a model `DescribeModel` resolves, a
//! configuration `DescribeNotebookInstanceLifecycleConfig` resolves, and an instance
//! `DescribeNotebookInstance` reports `InService`.
//!
//! Three crates have to agree for that to hold (CloudFormation makes the calls, SageMaker
//! serves them, IAM holds the role the notebook instance and the model run as), so the suite
//! lives here rather than in any one of them.
//!
//! The rows differ along what the rule generalises over: whether each resource's name is a
//! literal, a `Ref` to a parameter or left for CloudFormation to generate, whether the
//! template spells the numeric and boolean properties as JSON's own values or as the text a
//! parameter reference resolves to, whether the instance names a lifecycle configuration of
//! the same stack at all, and whether the template is JSON or YAML. Every expected value is
//! computed here from what the row declares, never read back out of the answer being checked.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery encoder and the XML/Outputs readers this suite needs already exist in the
// test binary's shared modules; they are shared rather than copied.
use crate::cloudformation_provisions_stack_resources::{outputs_of, xml_text};
use crate::query_wire::form_encode;

/// The region every request here is signed for, and so the region the stack's resources and
/// their ARNs belong to.
const REGION: &str = "us-west-2";

/// The account id every ARN this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// How long a case waits for a stack to reach the status the rule says it reaches.
///
/// GENEROUS on purpose: the only thing under test is WHICH status the stack reaches, never
/// how fast it gets there, so the deadline leaves room for a loaded host and the case polls
/// for the condition rather than sleeping for a span.
const STATUS_DEADLINE: Duration = Duration::from_secs(60);

/// How long the poll below waits between two reads of the stack.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them.
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
        service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(Arc::clone(&engine)),
    );
    // The hooks of a lifecycle configuration publish what they wrote to CloudWatch Logs, so
    // the service that holds those streams is part of the estate a stack's notebook instance
    // reaches.
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
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

/// Create a stack from `template` and answer its id, failing the case with the response body
/// when the create is refused.
async fn create_stack(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
    parameters: &[(&str, &str)],
) -> String {
    let mut members: Vec<(&str, &str)> = vec![
        ("StackName", stack_name),
        ("TemplateBody", template),
        // Every row declares an IAM role, and one of them names it: `CAPABILITY_NAMED_IAM`
        // is the stronger acknowledgement, which covers both.
        ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
    ];
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
        members.push((key.as_str(), value.as_str()));
    }
    let (status, body) = cloudformation(router, "CreateStack", &members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    xml_text(&body, "StackId").unwrap_or_else(|| panic!("case {case:?}: no <StackId> in {body}"))
}

/// The stack `stack` names, as the response body.
async fn describe_stack(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks for {stack:?}, got {status} with \
         body {body}"
    );
    body
}

/// Read the stack until it reports `wanted`, and answer the body that did.
///
/// The CONDITION is what is waited for, never a span: a stack whose resources are still
/// coming up is read again until it rests in the status the rule says it reaches or
/// [`STATUS_DEADLINE`] passes.
async fn stack_resting_in(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    wanted: &str,
) -> String {
    let deadline = Instant::now() + STATUS_DEADLINE;
    loop {
        let body = describe_stack(router, case, stack).await;
        let status = xml_text(&body, "StackStatus").unwrap_or_default();
        if status == wanted {
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the stack to reach {wanted:?}, got {status:?} with body \
             {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The ARN a notebook instance of `name` carries in this emulator, as the modelled
/// `NotebookInstanceArn` form spells it.
fn notebook_instance_arn(name: &str) -> String {
    format!("arn:aws:sagemaker:{REGION}:{ACCOUNT_ID}:notebook-instance/{name}")
}

/// The ARN a model of `name` carries, as the modelled `ModelArn` pattern spells it.
fn model_arn(name: &str) -> String {
    format!("arn:aws:sagemaker:{REGION}:{ACCOUNT_ID}:model/{name}")
}

/// The ARN a notebook instance lifecycle configuration of `name` carries.
fn lifecycle_config_arn(name: &str) -> String {
    format!(
        "arn:aws:sagemaker:{REGION}:{ACCOUNT_ID}:notebook-instance-lifecycle-config/{name}"
    )
}

/// The image a declared model container is pulled from.
const CONTAINER_IMAGE: &str = "000000000000.dkr.ecr.us-west-2.amazonaws.com/inference:1";

/// The base64 a declared lifecycle hook carries, which SageMaker hands back as written: the
/// `Content` member is "a base64-encoded string that contains a shell script".
const ON_CREATE_SCRIPT: &str = "ZWNobyBvbi1jcmVhdGU=";
const ON_START_SCRIPT: &str = "ZWNobyBvbi1zdGFydA==";

/// The trust document a role a notebook instance and a model run as carries: SageMaker
/// assumes the role, so its own service principal is the one admitted.
fn sagemaker_trust_document() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "sagemaker.amazonaws.com" },
            "Action": "sts:AssumeRole"
        }]
    })
}

/// One row of the table: a template, and what the rule says the stack must then have created
/// in SageMaker.
struct Row {
    case: &'static str,
    /// The template body, built from the stack name the row runs under.
    template: fn(&str) -> String,
    /// The parameters the create supplies.
    parameters: &'static [(&'static str, &'static str)],
    /// The notebook instance's name where the row fixes it, and `None` where the template
    /// leaves it for CloudFormation to generate — in which case the stack's own output is the
    /// only way to learn it.
    expected_notebook_name: fn(&str) -> Option<String>,
    /// The same for the model and for the lifecycle configuration, which the row leaves out
    /// entirely when its template declares none.
    expected_model_name: fn(&str) -> Option<String>,
    /// Whether the row's template declares a lifecycle configuration for the instance.
    declares_hooks: bool,
    /// The volume size the declaration asks for, whichever way it spells it.
    expected_volume_size: i64,
    /// Whether the model the row declares is network-isolated, whichever way it spells it.
    expected_isolated: bool,
    /// The instance type the declaration asks for.
    expected_instance_type: &'static str,
}

/// The rows: one axis per thing the rule generalises over.
const ROWS: &[Row] = &[
    Row {
        case: "literal names, a lifecycle configuration, and JSON's own number and boolean",
        template: |stack| {
            json!({
                "Resources": {
                    "NotebookRole": {
                        "Type": "AWS::IAM::Role",
                        "Properties": {
                            "RoleName": format!("{stack}-role"),
                            "AssumeRolePolicyDocument": sagemaker_trust_document()
                        }
                    },
                    "Hooks": {
                        "Type": "AWS::SageMaker::NotebookInstanceLifecycleConfig",
                        "Properties": {
                            "NotebookInstanceLifecycleConfigName": format!("{stack}-hooks"),
                            "OnCreate": [{ "Content": ON_CREATE_SCRIPT }],
                            "OnStart": [{ "Content": ON_START_SCRIPT }]
                        }
                    },
                    "Notebook": {
                        "Type": "AWS::SageMaker::NotebookInstance",
                        "Properties": {
                            "NotebookInstanceName": format!("{stack}-notebook"),
                            "InstanceType": "ml.t3.medium",
                            "RoleArn": { "Fn::GetAtt": ["NotebookRole", "Arn"] },
                            "LifecycleConfigName": {
                                "Fn::GetAtt": ["Hooks", "NotebookInstanceLifecycleConfigName"]
                            },
                            "VolumeSizeInGB": 20,
                            "Tags": [{ "Key": "owner", "Value": "notebooks" }]
                        }
                    },
                    "Inference": {
                        "Type": "AWS::SageMaker::Model",
                        "Properties": {
                            "ModelName": format!("{stack}-model"),
                            "ExecutionRoleArn": { "Fn::GetAtt": ["NotebookRole", "Arn"] },
                            "PrimaryContainer": { "Image": CONTAINER_IMAGE },
                            "EnableNetworkIsolation": true,
                            "Tags": [{ "Key": "owner", "Value": "inference" }]
                        }
                    }
                },
                "Outputs": {
                    "NotebookName": { "Value": { "Ref": "Notebook" } },
                    "HooksName": {
                        "Value": { "Fn::GetAtt": ["Hooks", "NotebookInstanceLifecycleConfigName"] }
                    },
                    "ModelName": { "Value": { "Fn::GetAtt": ["Inference", "ModelName"] } },
                    "RoleArn": { "Value": { "Fn::GetAtt": ["NotebookRole", "Arn"] } }
                }
            })
            .to_string()
        },
        parameters: &[],
        expected_notebook_name: |stack| Some(format!("{stack}-notebook")),
        expected_model_name: |stack| Some(format!("{stack}-model")),
        declares_hooks: true,
        expected_volume_size: 20,
        expected_isolated: true,
        expected_instance_type: "ml.t3.medium",
    },
    Row {
        case: "names from parameters, with the number and the boolean spelled as text",
        template: |_| {
            json!({
                "Parameters": {
                    "NotebookName": { "Type": "String" },
                    "ModelName": { "Type": "String" },
                    "VolumeSize": { "Type": "String" },
                    "Isolated": { "Type": "String" }
                },
                "Resources": {
                    "NotebookRole": {
                        "Type": "AWS::IAM::Role",
                        "Properties": {
                            "AssumeRolePolicyDocument": sagemaker_trust_document()
                        }
                    },
                    "Notebook": {
                        "Type": "AWS::SageMaker::NotebookInstance",
                        "Properties": {
                            "NotebookInstanceName": { "Ref": "NotebookName" },
                            "InstanceType": "ml.m5.xlarge",
                            "RoleArn": { "Fn::GetAtt": ["NotebookRole", "Arn"] },
                            "VolumeSizeInGB": { "Ref": "VolumeSize" },
                            "DirectInternetAccess": "Enabled",
                            "RootAccess": "Enabled"
                        }
                    },
                    "Inference": {
                        "Type": "AWS::SageMaker::Model",
                        "Properties": {
                            "ModelName": { "Ref": "ModelName" },
                            "ExecutionRoleArn": { "Fn::GetAtt": ["NotebookRole", "Arn"] },
                            "PrimaryContainer": { "Image": CONTAINER_IMAGE },
                            "EnableNetworkIsolation": { "Ref": "Isolated" }
                        }
                    }
                },
                "Outputs": {
                    "NotebookName": {
                        "Value": { "Fn::GetAtt": ["Notebook", "NotebookInstanceName"] }
                    },
                    "ModelName": { "Value": { "Ref": "Inference" } },
                    "RoleArn": { "Value": { "Fn::GetAtt": ["NotebookRole", "Arn"] } }
                }
            })
            .to_string()
        },
        parameters: &[
            ("NotebookName", "parameter-named-notebook"),
            ("ModelName", "parameter-named-model"),
            ("VolumeSize", "7"),
            ("Isolated", "false"),
        ],
        expected_notebook_name: |_| Some("parameter-named-notebook".to_string()),
        expected_model_name: |_| Some("parameter-named-model".to_string()),
        declares_hooks: false,
        expected_volume_size: 7,
        expected_isolated: false,
        expected_instance_type: "ml.m5.xlarge",
    },
    Row {
        case: "names CloudFormation generates, in YAML with short-form intrinsics",
        template: |_| {
            [
                "Resources:",
                "  NotebookRole:",
                "    Type: AWS::IAM::Role",
                "    Properties:",
                "      AssumeRolePolicyDocument:",
                "        Version: '2012-10-17'",
                "        Statement:",
                "          - Effect: Allow",
                "            Principal:",
                "              Service: sagemaker.amazonaws.com",
                "            Action: sts:AssumeRole",
                "  Hooks:",
                "    Type: AWS::SageMaker::NotebookInstanceLifecycleConfig",
                "    Properties:",
                "      OnStart:",
                "        - Content: ZWNobyBvbi1zdGFydA==",
                "  Notebook:",
                "    Type: AWS::SageMaker::NotebookInstance",
                "    Properties:",
                "      InstanceType: ml.t2.medium",
                "      RoleArn: !GetAtt NotebookRole.Arn",
                // The instance names the configuration by a `Ref` to it here, rather than
                // by the attribute the row above reads: both resolve to the name SageMaker
                // holds the configuration under.
                "      LifecycleConfigName: !Ref Hooks",
                "  Inference:",
                "    Type: AWS::SageMaker::Model",
                "    Properties:",
                "      ExecutionRoleArn: !GetAtt NotebookRole.Arn",
                "      PrimaryContainer:",
                "        Image: 000000000000.dkr.ecr.us-west-2.amazonaws.com/inference:1",
                "Outputs:",
                "  NotebookName:",
                "    Value: !Ref Notebook",
                "  HooksName:",
                "    Value: !GetAtt Hooks.NotebookInstanceLifecycleConfigName",
                "  ModelName:",
                "    Value: !GetAtt Inference.ModelName",
                "  RoleArn:",
                "    Value: !GetAtt NotebookRole.Arn",
                "",
            ]
            .join("\n")
        },
        parameters: &[],
        expected_notebook_name: |_| None,
        expected_model_name: |_| None,
        declares_hooks: true,
        // `VolumeSizeInGB`'s own doc: "the default value is 5 GB", which is what a
        // declaration that states none leaves the instance with.
        expected_volume_size: 5,
        // "If True, no inbound or outbound network calls can be made to or from the model
        // container": a model nothing asked to isolate is not isolated.
        expected_isolated: false,
        expected_instance_type: "ml.t2.medium",
    },
];

/// RULE: a stack's SageMaker resources are created in SageMaker, so every name the stack
/// publishes resolves there, the instance the stack reports created is `InService`, and the
/// stack's deletion takes all three away.
#[tokio::test]
async fn a_stack_provisions_its_sagemaker_estate() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let stack_name = format!("notebook-estate-{index}");
        let template = (row.template)(&stack_name);
        let stack_id = create_stack(&router, case, &stack_name, &template, row.parameters).await;

        // The notebook instance is answered `Pending`, so the stack's creation is still open
        // when the create answers: the case waits for the CONDITION the rule states.
        let body = stack_resting_in(&router, case, &stack_name, "CREATE_COMPLETE").await;
        let outputs = outputs_of(&body);

        // Every output the template declares carries its resolved value, which is what makes
        // the stack's own identifiers usable in a later call.
        let notebook = outputs
            .get("NotebookName")
            .unwrap_or_else(|| panic!("case {case:?}: no NotebookName output in {body}"))
            .clone();
        let model = outputs
            .get("ModelName")
            .unwrap_or_else(|| panic!("case {case:?}: no ModelName output in {body}"))
            .clone();
        let role_arn = outputs
            .get("RoleArn")
            .unwrap_or_else(|| panic!("case {case:?}: no RoleArn output in {body}"))
            .clone();
        match (row.expected_notebook_name)(&stack_name) {
            Some(expected) => assert_eq!(
                notebook, expected,
                "case {case:?}: expected the stack to name the instance {expected:?}, got \
                 {notebook:?}"
            ),
            None => {
                let prefix = format!("{stack_name}-Notebook-");
                assert!(
                    notebook.starts_with(&prefix),
                    "case {case:?}: expected a generated name starting {prefix:?}, got \
                     {notebook:?}"
                );
            }
        }
        match (row.expected_model_name)(&stack_name) {
            Some(expected) => assert_eq!(
                model, expected,
                "case {case:?}: expected the stack to name the model {expected:?}, got {model:?}"
            ),
            None => {
                let prefix = format!("{stack_name}-Inference-");
                assert!(
                    model.starts_with(&prefix),
                    "case {case:?}: expected a generated name starting {prefix:?}, got {model:?}"
                );
            }
        }

        // The notebook instance exists in SageMaker, is the one the declaration asked for,
        // and is the status the stack's own completion says it is in.
        let (status, described) = sagemaker(
            &router,
            "DescribeNotebookInstance",
            &json!({ "NotebookInstanceName": notebook }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeNotebookInstance for {notebook:?}, \
             got {status} with body {described}"
        );
        assert_eq!(
            described["NotebookInstanceArn"].as_str(),
            Some(notebook_instance_arn(&notebook).as_str()),
            "case {case:?}: expected the instance's own ARN, got {described}"
        );
        assert_eq!(
            described["NotebookInstanceStatus"].as_str(),
            Some("InService"),
            "case {case:?}: expected the completed stack to leave the instance InService, got \
             {described}"
        );
        assert_eq!(
            described["InstanceType"].as_str(),
            Some(row.expected_instance_type),
            "case {case:?}: expected the declared instance type, got {described}"
        );
        assert_eq!(
            described["RoleArn"].as_str(),
            Some(role_arn.as_str()),
            "case {case:?}: expected the instance to run as the role the stack created, got \
             {described}"
        );
        assert_eq!(
            described["VolumeSizeInGB"].as_i64(),
            Some(row.expected_volume_size),
            "case {case:?}: expected the declared volume size, got {described}"
        );

        // ...and the lifecycle configuration the instance names is one SageMaker holds, under
        // the name the stack published for it.
        if row.declares_hooks {
            let hooks = outputs
                .get("HooksName")
                .unwrap_or_else(|| panic!("case {case:?}: no HooksName output in {body}"))
                .clone();
            assert_eq!(
                described["NotebookInstanceLifecycleConfigName"].as_str(),
                Some(hooks.as_str()),
                "case {case:?}: expected the instance to name the stack's own configuration, \
                 got {described}"
            );
            let (status, config) = sagemaker(
                &router,
                "DescribeNotebookInstanceLifecycleConfig",
                &json!({ "NotebookInstanceLifecycleConfigName": hooks }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from \
                 DescribeNotebookInstanceLifecycleConfig for {hooks:?}, got {status} with body \
                 {config}"
            );
            assert_eq!(
                config["NotebookInstanceLifecycleConfigArn"].as_str(),
                Some(lifecycle_config_arn(&hooks).as_str()),
                "case {case:?}: expected the configuration's own ARN, got {config}"
            );
            assert_eq!(
                config["OnStart"],
                json!([{ "Content": ON_START_SCRIPT }]),
                "case {case:?}: expected the declared OnStart hook, got {config}"
            );
        } else {
            assert!(
                described["NotebookInstanceLifecycleConfigName"].is_null(),
                "case {case:?}: expected no configuration on an instance that names none, got \
                 {described}"
            );
        }

        // The model exists too, with the container and the role the declaration asked for.
        let (status, described_model) =
            sagemaker(&router, "DescribeModel", &json!({ "ModelName": model })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeModel for {model:?}, got {status} \
             with body {described_model}"
        );
        assert_eq!(
            described_model["ModelArn"].as_str(),
            Some(model_arn(&model).as_str()),
            "case {case:?}: expected the model's own ARN, got {described_model}"
        );
        assert_eq!(
            described_model["PrimaryContainer"]["Image"].as_str(),
            Some(CONTAINER_IMAGE),
            "case {case:?}: expected the declared container image, got {described_model}"
        );
        assert_eq!(
            described_model["ExecutionRoleArn"].as_str(),
            Some(role_arn.as_str()),
            "case {case:?}: expected the model to run as the role the stack created, got \
             {described_model}"
        );
        assert_eq!(
            described_model["EnableNetworkIsolation"].as_bool(),
            Some(row.expected_isolated),
            "case {case:?}: expected the declared network isolation, got {described_model}"
        );

        // Deleting the stack deletes what it created: a notebook instance SageMaker is
        // serving has to be stopped first, so the deletion is what the stack waits on.
        let (status, deleted) =
            cloudformation(&router, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {deleted}"
        );
        stack_resting_in(&router, case, &stack_id, "DELETE_COMPLETE").await;
        for (operation, request, subject) in [
            (
                "DescribeNotebookInstance",
                json!({ "NotebookInstanceName": notebook }),
                "the notebook instance",
            ),
            (
                "DescribeModel",
                json!({ "ModelName": model }),
                "the model",
            ),
        ] {
            let (status, gone) = sagemaker(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected {subject} to be gone after the stack's deletion, got \
                 {status} with body {gone}"
            );
        }
    }
}

/// RULE: a notebook instance whose role the owning service cannot assume fails the stack's
/// creation, and a create that asked for the failure to be KEPT
/// (`OnFailure=DO_NOTHING`) leaves what SageMaker created exactly as that service reports it.
#[tokio::test]
async fn a_notebook_instance_whose_role_cannot_be_assumed_fails_the_stack() {
    let router = build_router();

    let stack_name = "unassumable-role-notebook";
    let notebook_name = format!("{stack_name}-notebook");
    // A role ARN this account's IAM holds nothing under: SageMaker accepts the create and
    // the instance's own provisioning is what fails, which is the position the recorded
    // service puts such a create in.
    let template = json!({
        "Resources": {
            "Notebook": {
                "Type": "AWS::SageMaker::NotebookInstance",
                "Properties": {
                    "NotebookInstanceName": notebook_name,
                    "InstanceType": "ml.t3.medium",
                    "RoleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/absent-notebook-role")
                }
            }
        },
        "Outputs": { "NotebookName": { "Value": { "Ref": "Notebook" } } }
    });
    let (status, body) = cloudformation(
        &router,
        "CreateStack",
        &[
            ("StackName", stack_name),
            ("TemplateBody", &template.to_string()),
            ("OnFailure", "DO_NOTHING"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    let failed = stack_resting_in(&router, "an unassumable role", stack_name, "CREATE_FAILED").await;
    assert!(
        failed.contains("Notebook"),
        "expected the stack's failure to name the resource that did not come up, got {failed}"
    );

    let (status, described) = sagemaker(
        &router,
        "DescribeNotebookInstance",
        &json!({ "NotebookInstanceName": notebook_name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the instance SageMaker created to be left behind, got {status} with body \
         {described}"
    );
    assert_eq!(
        described["NotebookInstanceStatus"].as_str(),
        Some("Failed"),
        "expected SageMaker to report the instance Failed, got {described}"
    );

    // Deleting the stack takes the failed instance away: nothing is running for an instance
    // in that status, so SageMaker removes it without a stop.
    let (status, deleted) =
        cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from DeleteStack, got {status} with body {deleted}"
    );
    let stack_id = xml_text(&failed, "StackId")
        .unwrap_or_else(|| panic!("no <StackId> in the failed stack's description: {failed}"));
    stack_resting_in(&router, "an unassumable role", &stack_id, "DELETE_COMPLETE").await;
    let (status, gone) = sagemaker(
        &router,
        "DescribeNotebookInstance",
        &json!({ "NotebookInstanceName": notebook_name }),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected the failed instance to be gone with the stack, got {status} with body {gone}"
    );
}

/// RULE: a resource the owning service refuses is never passed off as created — the stack
/// fails with that service's own refusal and leaves nothing behind in SageMaker.
#[tokio::test]
async fn a_notebook_instance_sagemaker_refuses_fails_the_stack() {
    let router = build_router();

    // The rows: a placement SageMaker refuses (a subnet with no security group beside it,
    // which is that service's own rule for a notebook instance in a VPC), and a declaration
    // carrying a property no call this emulator makes applies.
    let rows: &[(&str, Value)] = &[
        (
            "a VPC placement SageMaker refuses",
            json!({
                "InstanceType": "ml.t3.medium",
                "RoleArn": { "Fn::GetAtt": ["NotebookRole", "Arn"] },
                "SubnetId": "subnet-0123456789abcdef0"
            }),
        ),
        (
            "a property no call applies",
            json!({
                "InstanceType": "ml.t3.medium",
                "RoleArn": { "Fn::GetAtt": ["NotebookRole", "Arn"] },
                "InstanceMetadataServiceVersion": "2"
            }),
        ),
    ];

    for (index, (case, properties)) in rows.iter().enumerate() {
        let stack_name = format!("doomed-notebook-{index}");
        let notebook_name = format!("{stack_name}-notebook");
        let mut declared = properties.clone();
        declared["NotebookInstanceName"] = json!(notebook_name);
        let template = json!({
            "Resources": {
                "NotebookRole": {
                    "Type": "AWS::IAM::Role",
                    "Properties": { "AssumeRolePolicyDocument": sagemaker_trust_document() }
                },
                "Notebook": {
                    "Type": "AWS::SageMaker::NotebookInstance",
                    "Properties": declared
                }
            },
            "Outputs": { "NotebookName": { "Value": { "Ref": "Notebook" } } }
        });
        create_stack(&router, case, &stack_name, &template.to_string(), &[]).await;
        stack_resting_in(&router, case, &stack_name, "ROLLBACK_COMPLETE").await;

        let (status, gone) = sagemaker(
            &router,
            "DescribeNotebookInstance",
            &json!({ "NotebookInstanceName": notebook_name }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the refused instance never to exist, got {status} with \
             body {gone}"
        );
    }
}
