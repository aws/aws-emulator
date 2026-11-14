// SPDX-License-Identifier: CC0-1.0

//! A CI topology DEPLOYED BY CLOUDFORMATION runs end to end: the pipeline the stack
//! provisioned reaches the terminal status the build its CodeBuild project ran decided.
//!
//! RULE under test: **a build's commands run in a build environment that has a working tree
//! of its own, so a project whose buildspec writes a file and reads it back builds to
//! success — which settles the pipeline's `CodeBuild` Build action, and the pipeline
//! execution with it, `Succeeded`; a buildspec whose command the environment cannot perform
//! fails the build, and the action then reports AWS's own job-failure code with the state
//! that build ended in, naming the build it ran and nothing about this emulator.** The
//! topology is deployed by template, so the pipeline, the project and the two service roles
//! a client gets are the records their owning services hold: the role the stack publishes
//! round-trips through `iam:GetRole` with the trust document the template declared, and the
//! execution of the published pipeline is what exercises it.
//!
//! Five crates have to agree for that to hold (CloudFormation deploys the estate,
//! CodePipeline runs the execution, CodeBuild runs the build, IAM holds the two roles and
//! their trust documents, and Amazon S3 holds the source and artifact buckets), so this
//! suite lives here: no crate-local test can see both ends of it.
//!
//! The rows differ along what the rule generalises over: how the buildspec writes the file
//! (from the start, or added to), and whether the path it reads is one the build wrote at
//! all — which is what decides the build's status and so the execution's.
//!
//! A second table pins the same rule over the two counts it also generalises over, because a
//! deployed pipeline is one a client drives repeatedly and a template may declare more than
//! one build: its rows declare one, two and three `AWS::CodeBuild::Project` resources with a
//! Build stage each, and start the published pipeline `STARTS` times, one settled execution
//! after another. Every start reaches `Succeeded` with every declared Build stage having run
//! a build of its own, of the project that stage's own `ProjectName` configuration names.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{elements, form_encode, opt_element};

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// How often to look while waiting for work the clock owes — a stack settling, an execution
/// reaching a terminal status — and how many looks to take. The spans are the owning
/// services' own, so nothing here sleeps for a fixed span expecting work to be done, and the
/// deadline is generous so a loaded host cannot decide the outcome.
const LOOK: Duration = Duration::from_millis(50);
const LOOKS: usize = 2_400;

/// CodePipeline's `X-Amz-Target` prefix, from that service's own model (service shape
/// `CodePipeline_20150709`).
const CODEPIPELINE_TARGET_PREFIX: &str = "CodePipeline_20150709";

/// CodeBuild's `X-Amz-Target` prefix, from that service's own model (service shape
/// `CodeBuild_20161006`).
const CODEBUILD_TARGET_PREFIX: &str = "CodeBuild_20161006";

/// The `PipelineExecutionStatus` values an execution never leaves
/// (`smithy_query(query="shape", shape="com.amazonaws.codepipeline#PipelineExecutionStatus")`).
const TERMINAL: [&str; 5] = ["Succeeded", "Failed", "Stopped", "Superseded", "Cancelled"];

/// The service principals the two declared roles are trusted by.
const CODEBUILD_PRINCIPAL: &str = "codebuild.amazonaws.com";
const CODEPIPELINE_PRINCIPAL: &str = "codepipeline.amazonaws.com";

/// The error code AWS reports for a Build action whose build did not succeed.
const JOB_FAILED: &str = "JobFailed";

/// The emulator as it is ASSEMBLED, on one engine: every call one service makes while
/// running this topology is dispatched back into this same router.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One awsQuery call of `service`, answered with the HTTP status and the XML body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let host = format!("{service}.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One awsJson1_1 call of `service`, answered with the HTTP status and the decoded body.
async fn json_rpc(
    router: &MultiServiceRouter,
    service: &str,
    prefix: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let host = format!("{service}.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{prefix}.{operation}"))
        .header("authorization", authorization(service))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One CodePipeline operation that has to succeed.
async fn codepipeline_ok(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, answer) =
        json_rpc(router, "codepipeline", CODEPIPELINE_TARGET_PREFIX, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: codepipeline {operation} must answer 200, got {status} with {answer}"
    );
    answer
}

/// One request against S3's REST surface, path-style.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &[u8]) -> (u16, String) {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("authorization", authorization("s3"))
        .header("content-type", "application/octet-stream")
        .body(Bytes::from(body.to_vec()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (
        status,
        String::from_utf8_lossy(response.body()).into_owned(),
    )
}

/// The versioned source bucket the pipeline's Source action reads, with one object at
/// `source.zip` — the bucket a client creates before it deploys the topology.
async fn source_bucket(router: &MultiServiceRouter, case: &str, bucket: &str) {
    let (status, body) = s3(router, "PUT", &format!("/{bucket}"), b"").await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateBucket({bucket}) must answer 200, got {status} with {body}"
    );
    let (status, body) = s3(
        router,
        "PUT",
        &format!("/{bucket}?versioning"),
        br#"<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>"#,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutBucketVersioning({bucket}) must answer 200, got {status} with {body}"
    );
    let (status, body) = s3(
        router,
        "PUT",
        &format!("/{bucket}/source.zip"),
        b"the source archive",
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutObject({bucket}/source.zip) must answer 200, got {status} with {body}"
    );
}

/// Whether a stack status is one a deployment STOPS in, so polling it again tells a client
/// nothing new.
fn is_terminal_stack(status: &str) -> bool {
    status.ends_with("_COMPLETE") || status.ends_with("_FAILED")
}

/// Poll `DescribeStacks` until the stack settles in a terminal status, and answer that status
/// with the body it was last described with.
async fn settled_stack(router: &MultiServiceRouter, case: &str, stack: &str) -> (String, String) {
    let mut described = String::new();
    for _ in 0..LOOKS {
        let (status, body) = query(
            router,
            "cloudformation",
            "2010-05-15",
            "DescribeStacks",
            &[("StackName", stack)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeStacks({stack:?}) must be answered, got {status} with {body}"
        );
        described = body;
        let reported = opt_element(&described, "StackStatus").unwrap_or_default();
        if is_terminal_stack(&reported) {
            return (reported, described);
        }
        tokio::time::sleep(LOOK).await;
    }
    panic!(
        "case {case:?}: the deployment must settle in a terminal status rather than leaving a \
         client polling, last described {described}"
    );
}

/// One stack output's value, or a panic naming what the stack published.
fn output(case: &str, described: &str, key: &str) -> String {
    let outputs: Vec<(String, String)> = elements(described, "OutputKey")
        .into_iter()
        .zip(elements(described, "OutputValue"))
        .collect();
    outputs
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.clone())
        .unwrap_or_else(|| {
            panic!("case {case:?}: the stack must publish {key}, got {outputs:?}")
        })
}

/// The trust document IAM holds for the role `arn` names, as `GetRole` reports it — the
/// JSON a client reads, with the XML escaping of the answer decoded.
async fn trust_document(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let role = arn.rsplit('/').next().unwrap_or_default().to_string();
    let (status, body) = query(
        router,
        "iam",
        "2010-05-08",
        "GetRole",
        &[("RoleName", role.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetRole({role:?}) must answer 200, got {status} with {body}"
    );
    let document = elements(&body, "AssumeRolePolicyDocument")
        .into_iter()
        .next()
        .unwrap_or_default();
    // IAM reports a trust policy as a document, which a client may also have handed over
    // URL-encoded, so either form reads back here.
    let decoded = ::smithy_server_runtime::percent_decode(&document).to_string();
    serde_json::from_str(&decoded).unwrap_or_else(|_| {
        panic!("case {case:?}: the trust document must be readable JSON, got {decoded:?}")
    })
}

/// Whether `document` allows `sts:AssumeRole` for the service principal `principal`.
fn allows_assume_role(document: &Value, principal: &str) -> bool {
    document["Statement"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .any(|statement| {
            let allowed = statement["Effect"] == json!("Allow");
            let action = statement["Action"] == json!("sts:AssumeRole")
                || statement["Action"]
                    .as_array()
                    .is_some_and(|actions| actions.contains(&json!("sts:AssumeRole")));
            let service = &statement["Principal"]["Service"];
            let trusted = service == &json!(principal)
                || service
                    .as_array()
                    .is_some_and(|services| services.contains(&json!(principal)));
            allowed && action && trusted
        })
}

/// Poll `GetPipelineExecution` until the execution reaches a status it never leaves, and
/// answer it.
async fn settled_execution(
    router: &MultiServiceRouter,
    case: &str,
    pipeline: &str,
    execution: &str,
) -> String {
    let request = json!({ "pipelineName": pipeline, "pipelineExecutionId": execution });
    let mut last = Value::Null;
    for _ in 0..LOOKS {
        let answer = codepipeline_ok(router, case, "GetPipelineExecution", &request).await;
        let status = answer["pipelineExecution"]["status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if TERMINAL.contains(&status.as_str()) {
            return status;
        }
        last = answer;
        tokio::time::sleep(LOOK).await;
    }
    panic!(
        "case {case:?}: the execution {execution} must reach a terminal status, last read {last}"
    );
}

/// The `Build` action's own result in `execution`, as `ListActionExecutions` reports it.
async fn build_action_result(
    router: &MultiServiceRouter,
    case: &str,
    pipeline: &str,
    execution: &str,
) -> Value {
    action_result(router, case, pipeline, execution, "Build").await
}

/// The result the action of the stage `stage` reported in `execution`, as
/// `ListActionExecutions` reports it — one read for any declared stage, so a topology whose
/// pipeline builds in several stages is read exactly like one that builds in a single stage.
async fn action_result(
    router: &MultiServiceRouter,
    case: &str,
    pipeline: &str,
    execution: &str,
    stage: &str,
) -> Value {
    let answer = codepipeline_ok(
        router,
        case,
        "ListActionExecutions",
        &json!({
            "pipelineName": pipeline,
            "filter": { "pipelineExecutionId": execution },
        }),
    )
    .await;
    answer["actionExecutionDetails"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|detail| detail["stageName"] == json!(stage))
        .map(|detail| detail["output"]["executionResult"].clone())
        .unwrap_or_else(|| {
            panic!("case {case:?}: the execution must report its {stage} action, got {answer}")
        })
}

/// The build CodeBuild holds under `id`.
async fn build(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, answer) = json_rpc(
        router,
        "codebuild",
        CODEBUILD_TARGET_PREFIX,
        "BatchGetBuilds",
        &json!({ "ids": [id] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: BatchGetBuilds({id}) must answer 200, got {status} with {answer}"
    );
    answer["builds"][0].clone()
}

/// The CI template a client deploys: the artifact bucket, the role CodeBuild assumes with
/// the inline policy its build needs, the build project whose source and artifacts are
/// CodePipeline's, the role CodePipeline assumes, and the two-stage pipeline that reads the
/// source object and builds it. The `Outputs` are the handles a client drives it by.
fn template(source: &str, buildspec: &str) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "ArtifactBucket": { "Type": "AWS::S3::Bucket" },
            "CodeBuildRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": { "Service": CODEBUILD_PRINCIPAL },
                            "Action": "sts:AssumeRole",
                        }],
                    },
                    "Policies": [{
                        "PolicyName": "build",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [
                                {
                                    "Effect": "Allow",
                                    "Action": [
                                        "logs:CreateLogGroup",
                                        "logs:CreateLogStream",
                                        "logs:PutLogEvents",
                                    ],
                                    "Resource": "*",
                                },
                                {
                                    "Effect": "Allow",
                                    "Action": [
                                        "s3:GetObject",
                                        "s3:GetObjectVersion",
                                        "s3:PutObject",
                                    ],
                                    "Resource": "*",
                                },
                            ],
                        },
                    }],
                },
            },
            "CodeBuildProject": {
                "Type": "AWS::CodeBuild::Project",
                "Properties": {
                    "Artifacts": { "Type": "CODEPIPELINE" },
                    "Source": { "Type": "CODEPIPELINE", "BuildSpec": buildspec },
                    "Environment": {
                        "Type": "LINUX_CONTAINER",
                        "ComputeType": "BUILD_GENERAL1_SMALL",
                        "Image": "aws/codebuild/amazonlinux2-x86_64-standard:5.0",
                    },
                    "ServiceRole": { "Fn::GetAtt": ["CodeBuildRole", "Arn"] },
                },
            },
            "PipelineRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": { "Service": CODEPIPELINE_PRINCIPAL },
                            "Action": "sts:AssumeRole",
                        }],
                    },
                    "Policies": [{
                        "PolicyName": "pipeline",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [
                                {
                                    "Effect": "Allow",
                                    "Action": ["s3:Get*", "s3:PutObject"],
                                    "Resource": "*",
                                },
                                {
                                    "Effect": "Allow",
                                    "Action": [
                                        "codebuild:StartBuild",
                                        "codebuild:BatchGetBuilds",
                                    ],
                                    "Resource": { "Fn::GetAtt": ["CodeBuildProject", "Arn"] },
                                },
                            ],
                        },
                    }],
                },
            },
            "Pipeline": {
                "Type": "AWS::CodePipeline::Pipeline",
                "Properties": {
                    "RoleArn": { "Fn::GetAtt": ["PipelineRole", "Arn"] },
                    "ArtifactStore": {
                        "Type": "S3",
                        "Location": { "Ref": "ArtifactBucket" },
                    },
                    "Stages": [
                        {
                            "Name": "Source",
                            "Actions": [{
                                "Name": "Source",
                                "ActionTypeId": {
                                    "Category": "Source",
                                    "Owner": "AWS",
                                    "Provider": "S3",
                                    "Version": "1",
                                },
                                "Configuration": {
                                    "S3Bucket": source,
                                    "S3ObjectKey": "source.zip",
                                    "PollForSourceChanges": "false",
                                },
                                "OutputArtifacts": [{ "Name": "SourceOutput" }],
                                "RunOrder": 1,
                            }],
                        },
                        {
                            "Name": "Build",
                            "Actions": [{
                                "Name": "Build",
                                "ActionTypeId": {
                                    "Category": "Build",
                                    "Owner": "AWS",
                                    "Provider": "CodeBuild",
                                    "Version": "1",
                                },
                                "Configuration": {
                                    "ProjectName": { "Ref": "CodeBuildProject" },
                                },
                                "InputArtifacts": [{ "Name": "SourceOutput" }],
                                "RunOrder": 1,
                            }],
                        },
                    ],
                },
            },
        },
        "Outputs": {
            "PipelineName": { "Value": { "Ref": "Pipeline" } },
            "ProjectName": { "Value": { "Ref": "CodeBuildProject" } },
            "CodeBuildRoleArn": { "Value": { "Fn::GetAtt": ["CodeBuildRole", "Arn"] } },
            "PipelineRoleArn": { "Value": { "Fn::GetAtt": ["PipelineRole", "Arn"] } },
        },
    })
    .to_string()
}

/// One row of the table: the case, the project's buildspec, the status the build it runs
/// ends in, and the status the pipeline execution then reaches.
struct Row {
    case: &'static str,
    buildspec: &'static str,
    build_status: &'static str,
    execution_status: &'static str,
}

/// The rows: each deploys the same topology with a different buildspec.
const ROWS: &[Row] = &[
    Row {
        case: "a buildspec that writes its artifact file and reads it back",
        buildspec: "version: 0.2\nphases:\n  build:\n    commands:\n      \
                    - 'echo built > output.txt'\n      - 'cat output.txt'\n",
        build_status: "SUCCEEDED",
        execution_status: "Succeeded",
    },
    Row {
        case: "a buildspec that adds to the file over two phases and reads it back",
        buildspec: "version: 0.2\nphases:\n  pre_build:\n    commands:\n      \
                    - 'echo first > output.txt'\n  build:\n    commands:\n      \
                    - 'echo second >> output.txt'\n      - 'cat output.txt'\n",
        build_status: "SUCCEEDED",
        execution_status: "Succeeded",
    },
    Row {
        case: "a buildspec that reads a path the build never wrote",
        buildspec: "version: 0.2\nphases:\n  build:\n    commands:\n      \
                    - 'cat output.txt'\n",
        build_status: "FAILED",
        execution_status: "Failed",
    },
];

/// Every row: the stack converges, the roles it published round-trip through IAM with the
/// trust documents the template declared, and an execution of the published pipeline reaches
/// the terminal status the build its project ran decided.
#[tokio::test]
async fn a_deployed_ci_topology_runs_its_pipeline_to_the_status_its_build_decided() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let stack = format!("ci-topology-{index}");
        let source = format!("ci-topology-source-{index}");
        source_bucket(&router, case, &source).await;

        let (status, body) = query(
            &router,
            "cloudformation",
            "2010-05-15",
            "CreateStack",
            &[
                ("StackName", stack.as_str()),
                ("TemplateBody", &template(&source, row.buildspec)),
                ("Capabilities.member.1", "CAPABILITY_IAM"),
                ("Capabilities.member.2", "CAPABILITY_NAMED_IAM"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStack must answer 200, got {status} with {body}"
        );

        let (stack_status, described) = settled_stack(&router, case, &stack).await;
        assert_eq!(
            stack_status, "CREATE_COMPLETE",
            "case {case:?}: the CI template must deploy, got {stack_status} with reason {:?}",
            opt_element(&described, "StackStatusReason")
        );

        // The deployed topology's handles are the ones the stack published.
        let pipeline = output(case, &described, "PipelineName");
        let project = output(case, &described, "ProjectName");
        let codebuild_role = output(case, &described, "CodeBuildRoleArn");
        let pipeline_role = output(case, &described, "PipelineRoleArn");

        // Each declared role is a role IAM really holds, with the trust the template stated.
        for (arn, principal) in [
            (&codebuild_role, CODEBUILD_PRINCIPAL),
            (&pipeline_role, CODEPIPELINE_PRINCIPAL),
        ] {
            let document = trust_document(&router, case, arn).await;
            assert!(
                allows_assume_role(&document, principal),
                "case {case:?}: the role {arn} must be assumable by {principal}, got {document}"
            );
        }

        // The pipeline the stack published runs, and the execution settles where the build
        // its project ran put it.
        let started = codepipeline_ok(
            &router,
            case,
            "StartPipelineExecution",
            &json!({ "name": pipeline }),
        )
        .await;
        let execution = started["pipelineExecutionId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_execution_id(&execution),
            "case {case:?}: the execution id must have the modelled shape, got {execution:?}"
        );

        let settled = settled_execution(&router, case, &pipeline, &execution).await;
        assert_eq!(
            settled, row.execution_status,
            "case {case:?}: expected the execution to be {}, got {settled:?}",
            row.execution_status
        );

        // The action reports the build it ran, which is a build CodeBuild holds for the
        // project the stack created, in the state that decided the action.
        let result = build_action_result(&router, case, &pipeline, &execution).await;
        let external = result["externalExecutionId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !external.is_empty(),
            "case {case:?}: the Build action must name the build it ran, got {result}"
        );
        let ran = build(&router, case, &external).await;
        assert_eq!(
            ran["projectName"].as_str(),
            Some(project.as_str()),
            "case {case:?}: the build must belong to the project the stack created, got {ran}"
        );
        assert_eq!(
            ran["buildStatus"].as_str(),
            Some(row.build_status),
            "case {case:?}: expected the build to be {}, got {ran}",
            row.build_status
        );

        let detail = result["errorDetails"]["message"]
            .as_str()
            .or_else(|| result["externalExecutionSummary"].as_str())
            .unwrap_or_default()
            .to_string();
        if row.build_status == "SUCCEEDED" {
            assert!(
                result["errorDetails"].is_null(),
                "case {case:?}: a succeeded action carries no error, got {result}"
            );
        } else {
            assert_eq!(
                result["errorDetails"]["code"].as_str(),
                Some(JOB_FAILED),
                "case {case:?}: a build that did not succeed is a job failure, got {result}"
            );
            assert_eq!(
                detail,
                format!("Build terminated with state: {}", row.build_status),
                "case {case:?}: the detail must be the state the build ended in, got {detail:?}"
            );
        }
        assert!(
            !detail.contains("emulator"),
            "case {case:?}: the detail a client reads must not announce the emulator, got \
             {detail:?}"
        );
    }
}

/// Whether `id` has the shape the model gives a pipeline execution id
/// (`PipelineExecutionId`, `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`).
fn is_execution_id(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(width, group)| {
                group.len() == *width
                    && group
                        .chars()
                        .all(|character| character.is_ascii_digit() || ('a'..='f').contains(&character))
            })
}

/// The `Build$buildStatus` a build that ran to success reports
/// (`smithy_query(query="shape", shape="com.amazonaws.codebuild#StatusType")`).
const BUILD_SUCCEEDED: &str = "SUCCEEDED";

/// The terminal `PipelineExecutionStatus` of an execution whose every action succeeded.
const EXECUTION_SUCCEEDED: &str = "Succeeded";

/// Buildspecs that each write a file of their own and read it back, so every build the
/// second table runs is one that succeeds. A row declares a different NUMBER of them, which
/// is the number of projects and Build stages its template then declares.
const BUILDSPECS: [&str; 3] = [
    "version: 0.2\nphases:\n  build:\n    commands:\n      - 'echo first > first.txt'\n      \
     - 'cat first.txt'\n",
    "version: 0.2\nphases:\n  pre_build:\n    commands:\n      - 'echo second > second.txt'\n  \
     build:\n    commands:\n      - 'cat second.txt'\n",
    "version: 0.2\nphases:\n  build:\n    commands:\n      - 'echo third > third.txt'\n      \
     - 'cat third.txt'\n      - 'cat third.txt'\n",
];

/// How many times each row starts the pipeline the stack published, one settled execution
/// after another: a client that drives a deployed topology comes back to it, and the rule
/// holds for every start rather than only the first.
const STARTS: usize = 2;

/// The logical id — and so the `Ref` — of the project the Build stage at `position` names.
fn project_logical_id(position: usize) -> String {
    format!("CodeBuildProject{}", position + 1)
}

/// The name of the Build stage at `position`.
fn build_stage_name(position: usize) -> String {
    format!("Build{}", position + 1)
}

/// The stack output publishing the name of the project the Build stage at `position` names.
fn project_output_key(position: usize) -> String {
    format!("ProjectName{}", position + 1)
}

/// The CI template a client deploys when its pipeline builds in SEVERAL stages: the estate
/// [`template`] declares, with one `AWS::CodeBuild::Project` and one Build stage per
/// buildspec — each stage naming its own project through its `ProjectName` configuration and
/// consuming the artifact the Source stage published. The `Outputs` publish the pipeline's
/// name, each project's name and the two role ARNs, which are the handles a client drives
/// the deployed topology by.
fn staged_template(source: &str, buildspecs: &[&str]) -> String {
    let projects: Vec<String> = (0..buildspecs.len()).map(project_logical_id).collect();
    let project_arns: Vec<Value> = projects
        .iter()
        .map(|logical| json!({ "Fn::GetAtt": [logical, "Arn"] }))
        .collect();

    let mut resources = serde_json::Map::new();
    resources.insert("ArtifactBucket".to_string(), json!({ "Type": "AWS::S3::Bucket" }));
    resources.insert(
        "CodeBuildRole".to_string(),
        json!({
            "Type": "AWS::IAM::Role",
            "Properties": {
                "AssumeRolePolicyDocument": {
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": { "Service": CODEBUILD_PRINCIPAL },
                        "Action": "sts:AssumeRole",
                    }],
                },
                "Policies": [{
                    "PolicyName": "build",
                    "PolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [
                            {
                                "Effect": "Allow",
                                "Action": [
                                    "logs:CreateLogGroup",
                                    "logs:CreateLogStream",
                                    "logs:PutLogEvents",
                                ],
                                "Resource": "*",
                            },
                            {
                                "Effect": "Allow",
                                "Action": ["s3:GetObject", "s3:GetObjectVersion", "s3:PutObject"],
                                "Resource": "*",
                            },
                        ],
                    },
                }],
            },
        }),
    );
    resources.insert(
        "PipelineRole".to_string(),
        json!({
            "Type": "AWS::IAM::Role",
            "Properties": {
                "AssumeRolePolicyDocument": {
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": { "Service": CODEPIPELINE_PRINCIPAL },
                        "Action": "sts:AssumeRole",
                    }],
                },
                "Policies": [{
                    "PolicyName": "pipeline",
                    "PolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [
                            {
                                "Effect": "Allow",
                                "Action": ["s3:Get*", "s3:PutObject"],
                                "Resource": "*",
                            },
                            {
                                "Effect": "Allow",
                                "Action": ["codebuild:StartBuild", "codebuild:BatchGetBuilds"],
                                "Resource": project_arns,
                            },
                        ],
                    },
                }],
            },
        }),
    );

    let mut outputs = serde_json::Map::new();
    outputs.insert(
        "PipelineName".to_string(),
        json!({ "Value": { "Ref": "Pipeline" } }),
    );
    outputs.insert(
        "CodeBuildRoleArn".to_string(),
        json!({ "Value": { "Fn::GetAtt": ["CodeBuildRole", "Arn"] } }),
    );
    outputs.insert(
        "PipelineRoleArn".to_string(),
        json!({ "Value": { "Fn::GetAtt": ["PipelineRole", "Arn"] } }),
    );

    let mut stages = vec![json!({
        "Name": "Source",
        "Actions": [{
            "Name": "Source",
            "ActionTypeId": {
                "Category": "Source",
                "Owner": "AWS",
                "Provider": "S3",
                "Version": "1",
            },
            "Configuration": {
                "S3Bucket": source,
                "S3ObjectKey": "source.zip",
                "PollForSourceChanges": "false",
            },
            "OutputArtifacts": [{ "Name": "SourceOutput" }],
            "RunOrder": 1,
        }],
    })];
    for (position, buildspec) in buildspecs.iter().enumerate() {
        let logical = project_logical_id(position);
        resources.insert(
            logical.clone(),
            json!({
                "Type": "AWS::CodeBuild::Project",
                "Properties": {
                    "Artifacts": { "Type": "CODEPIPELINE" },
                    "Source": { "Type": "CODEPIPELINE", "BuildSpec": buildspec },
                    "Environment": {
                        "Type": "LINUX_CONTAINER",
                        "ComputeType": "BUILD_GENERAL1_SMALL",
                        "Image": "aws/codebuild/amazonlinux2-x86_64-standard:5.0",
                    },
                    "ServiceRole": { "Fn::GetAtt": ["CodeBuildRole", "Arn"] },
                },
            }),
        );
        stages.push(json!({
            "Name": build_stage_name(position),
            "Actions": [{
                "Name": build_stage_name(position),
                "ActionTypeId": {
                    "Category": "Build",
                    "Owner": "AWS",
                    "Provider": "CodeBuild",
                    "Version": "1",
                },
                "Configuration": { "ProjectName": { "Ref": logical } },
                "InputArtifacts": [{ "Name": "SourceOutput" }],
                "RunOrder": 1,
            }],
        }));
        outputs.insert(
            project_output_key(position),
            json!({ "Value": { "Ref": logical } }),
        );
    }

    resources.insert(
        "Pipeline".to_string(),
        json!({
            "Type": "AWS::CodePipeline::Pipeline",
            "Properties": {
                "RoleArn": { "Fn::GetAtt": ["PipelineRole", "Arn"] },
                "ArtifactStore": { "Type": "S3", "Location": { "Ref": "ArtifactBucket" } },
                "Stages": stages,
            },
        }),
    );

    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": Value::Object(resources),
        "Outputs": Value::Object(outputs),
    })
    .to_string()
}

/// One row of the second table: how many Build stages — and so how many projects — the
/// deployed template declares.
struct StagedRow {
    case: &'static str,
    buildspecs: &'static [&'static str],
}

/// The rows: the same topology deployed with one, two and three Build stages.
const STAGED_ROWS: &[StagedRow] = &[
    StagedRow {
        case: "a deployed pipeline that builds in one stage",
        buildspecs: &[BUILDSPECS[0]],
    },
    StagedRow {
        case: "a deployed pipeline that builds in two stages",
        buildspecs: &[BUILDSPECS[0], BUILDSPECS[1]],
    },
    StagedRow {
        case: "a deployed pipeline that builds in three stages",
        buildspecs: &[BUILDSPECS[0], BUILDSPECS[1], BUILDSPECS[2]],
    },
];

/// Every row: the stack converges, the roles it published round-trip through IAM, and each
/// of the `STARTS` executions of the published pipeline reaches `Succeeded` having run every
/// declared Build stage as a build of its own — of the project that stage's configuration
/// names — rather than one execution succeeding and a later one stalling.
#[tokio::test]
async fn a_deployed_ci_topology_runs_every_declared_build_stage_on_every_start() {
    let router = build_router();

    for (index, row) in STAGED_ROWS.iter().enumerate() {
        let case = row.case;
        let stack = format!("ci-staged-{index}");
        let source = format!("ci-staged-source-{index}");
        source_bucket(&router, case, &source).await;

        let (status, body) = query(
            &router,
            "cloudformation",
            "2010-05-15",
            "CreateStack",
            &[
                ("StackName", stack.as_str()),
                ("TemplateBody", &staged_template(&source, row.buildspecs)),
                ("Capabilities.member.1", "CAPABILITY_IAM"),
                ("Capabilities.member.2", "CAPABILITY_NAMED_IAM"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStack must answer 200, got {status} with {body}"
        );

        let (stack_status, described) = settled_stack(&router, case, &stack).await;
        assert_eq!(
            stack_status, "CREATE_COMPLETE",
            "case {case:?}: the CI template must deploy, got {stack_status} with reason {:?}",
            opt_element(&described, "StackStatusReason")
        );

        let pipeline = output(case, &described, "PipelineName");
        let projects: Vec<String> = (0..row.buildspecs.len())
            .map(|position| output(case, &described, &project_output_key(position)))
            .collect();

        // Each declared role is a role IAM really holds, with the trust the template stated.
        for (key, principal) in [
            ("CodeBuildRoleArn", CODEBUILD_PRINCIPAL),
            ("PipelineRoleArn", CODEPIPELINE_PRINCIPAL),
        ] {
            let arn = output(case, &described, key);
            let document = trust_document(&router, case, &arn).await;
            assert!(
                allows_assume_role(&document, principal),
                "case {case:?}: the role {arn} must be assumable by {principal}, got {document}"
            );
        }

        let mut executions: Vec<String> = Vec::new();
        let mut builds: Vec<String> = Vec::new();
        for attempt in 1..=STARTS {
            let attempt_case = format!("{case}, start {attempt}");
            let case = attempt_case.as_str();

            let started = codepipeline_ok(
                &router,
                case,
                "StartPipelineExecution",
                &json!({ "name": pipeline }),
            )
            .await;
            let execution = started["pipelineExecutionId"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                is_execution_id(&execution),
                "case {case:?}: the execution id must have the modelled shape, got {execution:?}"
            );
            assert!(
                !executions.contains(&execution),
                "case {case:?}: every start is its own execution, got {execution} again"
            );

            let settled = settled_execution(&router, case, &pipeline, &execution).await;
            assert_eq!(
                settled, EXECUTION_SUCCEEDED,
                "case {case:?}: expected the execution to be {EXECUTION_SUCCEEDED}, got \
                 {settled:?}"
            );

            // Each declared Build stage ran a build of its own, of the project it names.
            for (position, project) in projects.iter().enumerate() {
                let stage = build_stage_name(position);
                let result = action_result(&router, case, &pipeline, &execution, &stage).await;
                assert!(
                    result["errorDetails"].is_null(),
                    "case {case:?}: the succeeded stage {stage} carries no error, got {result}"
                );
                let external = result["externalExecutionId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    !external.is_empty(),
                    "case {case:?}: the {stage} action must name the build it ran, got {result}"
                );
                assert!(
                    !builds.contains(&external),
                    "case {case:?}: the {stage} action must run a build of its own, got \
                     {external} again"
                );
                let ran = build(&router, case, &external).await;
                assert_eq!(
                    ran["projectName"].as_str(),
                    Some(project.as_str()),
                    "case {case:?}: the {stage} build must belong to {project}, got {ran}"
                );
                assert_eq!(
                    ran["buildStatus"].as_str(),
                    Some(BUILD_SUCCEEDED),
                    "case {case:?}: expected the {stage} build to be {BUILD_SUCCEEDED}, got {ran}"
                );
                builds.push(external);
            }
            executions.push(execution);
        }

        // Every start left a record of its own on the pipeline the stack published.
        let listed = codepipeline_ok(
            &router,
            case,
            "ListPipelineExecutions",
            &json!({ "pipelineName": pipeline }),
        )
        .await;
        let summaries = listed["pipelineExecutionSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for execution in &executions {
            let summary = summaries
                .iter()
                .find(|summary| summary["pipelineExecutionId"] == json!(execution))
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: the pipeline must report the execution {execution}, got \
                         {listed}"
                    )
                });
            assert_eq!(
                summary["status"].as_str(),
                Some(EXECUTION_SUCCEEDED),
                "case {case:?}: expected {execution} to be listed {EXECUTION_SUCCEEDED}, got \
                 {summary}"
            );
        }
    }
}
