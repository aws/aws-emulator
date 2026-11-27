// SPDX-License-Identifier: CC0-1.0

//! What a pipeline's `CodeBuild` Build action COMES TO when the project it names really runs
//! its buildspec — driven over the wire each service speaks, with CodePipeline, CodeBuild,
//! Amazon S3, Amazon ECR and IAM mounted on one engine the way the router assembles them.
//!
//! RULE under test: a Build action's outcome is the outcome of the build it started, so a
//! project whose every buildspec command the build environment performs settles the action
//! `Succeeded` and the execution `Succeeded` — the handoff in both directions being real
//! state: the action's input artifact is unpacked into the build's working tree, and what the
//! build's `artifacts` section declares is stored at the object the action's OUTPUT artifact
//! occupies, which is what `ListActionExecutions` publishes for that artifact and what the
//! actions after it read. A command the environment cannot perform fails the build, and the
//! action and the execution then report that failure instead.
//!
//! The rows differ along what the rule generalises over: which files the source artifact
//! carries, which commands the buildspec transforms them with, where the artifact section
//! names the produced files, and — for the container rows — how the buildspec starts and
//! probes the container daemon, and whether the role the build runs as is granted the registry
//! actions its own commands take.
//!
//! Nothing here sleeps for a build or an execution to finish: it POLLS the reporting
//! operations until the CONDITION holds, exactly as a client does.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix OPERATIONS.md records for this service.
const TARGET_PREFIX: &str = "CodePipeline_20150709";

/// The `X-Amz-Target` prefix CodeBuild's OPERATIONS.md records.
const CODEBUILD_TARGET: &str = "CodeBuild_20161006";

/// The `X-Amz-Target` prefix Amazon ECR's OPERATIONS.md records.
const ECR_TARGET: &str = "AmazonEC2ContainerRegistry_V20150921";

/// The Region every request of this suite is signed for.
const REGION: &str = "us-west-2";

/// The access key id every request presents.
const ACCOUNT_KEY: &str = "AKIDEXAMPLE";

/// The trust policy a pipeline's service role carries.
const PIPELINE_TRUST: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"codepipeline.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The trust policy a build project's service role carries.
const CODEBUILD_TRUST: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"codebuild.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The actions AWS documents a role needs to deliver a build's own output to CloudWatch Logs
/// (CodeBuild User Guide, the build's log-delivery permissions).
const LOG_ACTIONS: [&str; 3] = [
    "logs:CreateLogGroup",
    "logs:CreateLogStream",
    "logs:PutLogEvents",
];

/// The actions AWS documents a role needs to read a pipeline's artifacts out of the artifact
/// store and put its own back (CodePipeline User Guide, the CodeBuild service-role policy).
const ARTIFACT_ACTIONS: [&str; 3] = ["s3:GetObject", "s3:GetObjectVersion", "s3:PutObject"];

/// The actions AWS documents a role needs to PUSH an image to a repository (Amazon ECR User
/// Guide, the pushing permissions it lists), beside the login the push presents.
const PUSH_ACTIONS: [&str; 6] = [
    "ecr:GetAuthorizationToken",
    "ecr:BatchCheckLayerAvailability",
    "ecr:InitiateLayerUpload",
    "ecr:UploadLayerPart",
    "ecr:CompleteLayerUpload",
    "ecr:PutImage",
];

/// The actions a pipeline's own role takes to perform a `CodeBuild` action (that same guide,
/// the CodePipeline service-role policy).
const BUILD_ACTIONS: [&str; 2] = ["codebuild:StartBuild", "codebuild:BatchGetBuilds"];

/// The `PipelineExecutionStatus` values an execution never leaves.
const TERMINAL: [&str; 5] = [
    "Succeeded",
    "Failed",
    "Stopped",
    "Superseded",
    "Cancelled",
];

/// How long a wait for an execution or a build to settle may take before the suite calls it
/// stalled. Generous on purpose: the wait ends on the CONDITION, and the deadline is only
/// there so something that never settles fails the test instead of hanging it.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long a wait leaves before it looks again.
const LOOK_AGAIN_AFTER: Duration = Duration::from_millis(25);

/// One emulator: this service, CodeBuild, Amazon S3, Amazon ECR and IAM over ONE shared
/// engine, the way the router mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_codepipeline_codepipeline_20150709::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_codebuild_codebuild_20161006::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 credential scope naming `signing_name`, in [`REGION`].
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host;x-amz-date, Signature=00"
    )
}

/// One awsJson1_1 call to the service `signing_name` claims, under `target`.
async fn json_call(
    router: &MultiServiceRouter,
    signing_name: &str,
    target: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header("x-amz-target", format!("{target}.{operation}"))
        .header("content-type", "application/x-amz-json-1.1")
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the harness builds a well-formed request");
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
async fn cp_ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, answer) = json_call(router, "codepipeline", TARGET_PREFIX, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: codepipeline {operation} must answer 200, got {status} with {answer}"
    );
    answer
}

/// One CodeBuild operation that has to succeed.
async fn cb_ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, answer) = json_call(router, "codebuild", CODEBUILD_TARGET, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: codebuild {operation} must answer 200, got {status} with {answer}"
    );
    answer
}

/// One Amazon ECR operation that has to succeed.
async fn ecr_ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, answer) = json_call(router, "ecr", ECR_TARGET, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ecr {operation} must answer 200, got {status} with {answer}"
    );
    answer
}

/// One S3 request, path style, the way the emulated S3 service routes it.
async fn s3(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, Vec<u8>) {
    let request = ::http::Request::builder()
        .method(method)
        .uri(format!("https://s3.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", authorization("s3"))
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("the harness builds a well-formed request");
    let response = router.dispatch(request).await;
    let (parts, payload) = response.into_parts();
    (parts.status.as_u16(), payload.to_vec())
}

/// Create a bucket with versioning enabled, which is what a pipeline's artifact store and its
/// S3 source bucket are.
async fn versioned_bucket(router: &MultiServiceRouter, name: &str) {
    let configuration = format!(
        "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
         <LocationConstraint>{REGION}</LocationConstraint></CreateBucketConfiguration>"
    );
    let (status, body) = s3(router, "PUT", &format!("/{name}"), configuration.as_bytes()).await;
    assert_eq!(
        status,
        200,
        "the bucket {name:?} must be created, got {status} with {}",
        String::from_utf8_lossy(&body)
    );
    let (status, body) = s3(
        router,
        "PUT",
        &format!("/{name}?versioning"),
        b"<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>",
    )
    .await;
    assert_eq!(
        status,
        200,
        "versioning must be enabled on {name:?}, got {status} with {}",
        String::from_utf8_lossy(&body)
    );
}

/// One awsQuery call against the IAM service mounted beside this one.
async fn iam_call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&crate::pipeline_lifecycle::form_encode(name));
        body.push('=');
        body.push_str(&crate::pipeline_lifecycle::form_encode(value));
    }
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("authorization", authorization("iam"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(bytes::Bytes::from(body))
        .expect("the harness builds a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8(response.into_body().to_vec()).expect("a UTF-8 body");
    (status, payload)
}

/// Create a role carrying `trust` with an inline policy allowing `actions` on `resources`, and
/// answer the ARN IAM holds it under.
async fn role_allowing(
    router: &MultiServiceRouter,
    name: &str,
    trust: &str,
    grants: &[(&[&str], &str)],
) -> String {
    let (status, body) = iam_call(
        router,
        "CreateRole",
        &[("RoleName", name), ("AssumeRolePolicyDocument", trust)],
    )
    .await;
    assert_eq!(status, 200, "CreateRole {name} must succeed: {body}");
    let statements: Vec<Value> = grants
        .iter()
        .map(|(actions, resource)| {
            json!({"Effect": "Allow", "Action": actions, "Resource": resource})
        })
        .collect();
    let document = json!({"Version": "2012-10-17", "Statement": statements}).to_string();
    let (status, body) = iam_call(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", name),
            ("PolicyName", "permissions"),
            ("PolicyDocument", &document),
        ],
    )
    .await;
    assert_eq!(status, 200, "PutRolePolicy {name} must succeed: {body}");
    format!(
        "arn:aws:iam::{}:role/{name}",
        ::engine::emulator_account_id()
    )
}

/// Wait until every execution of `pipeline` has reached a terminal status, and answer the
/// newest one's id and status.
async fn settled_execution(
    router: &MultiServiceRouter,
    case: &str,
    pipeline: &str,
) -> (String, String) {
    let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let listed = cp_ok(
            router,
            case,
            "ListPipelineExecutions",
            &json!({"pipelineName": pipeline}),
        )
        .await;
        let summaries = listed["pipelineExecutionSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let settled = !summaries.is_empty()
            && summaries
                .iter()
                .all(|summary| TERMINAL.contains(&summary["status"].as_str().unwrap_or_default()));
        if settled {
            return (
                summaries[0]["pipelineExecutionId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                summaries[0]["status"].as_str().unwrap_or_default().to_string(),
            );
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: every execution of {pipeline} must settle, last read {listed}"
        );
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}

/// The action execution `ListActionExecutions` reports for `action` of `execution`.
async fn action_execution(
    router: &MultiServiceRouter,
    case: &str,
    pipeline: &str,
    execution: &str,
    action: &str,
) -> Value {
    let answer = cp_ok(
        router,
        case,
        "ListActionExecutions",
        &json!({
            "pipelineName": pipeline,
            "filter": {"pipelineExecutionId": execution},
        }),
    )
    .await;
    answer["actionExecutionDetails"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|detail| detail["actionName"].as_str() == Some(action))
        .unwrap_or_else(|| panic!("case {case:?}: no action execution for {action}, got {answer}"))
}

/// The build CodeBuild holds for `id`, read once it reports itself complete.
async fn settled_build(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let read = cb_ok(router, case, "BatchGetBuilds", &json!({"ids": [id]})).await;
        let build = read["builds"][0].clone();
        if build["buildComplete"] == json!(true) {
            return build;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: the build {id} must settle, last read {build}"
        );
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}

/// A pipeline whose Source stage reads `source_key` out of `bucket`, whose Build stage builds
/// `project` into the artifact `BuildOutput`, and whose Deploy stage extracts that artifact
/// under `prefix` of the same bucket — so what the build produced is readable as objects.
fn source_build_deploy(
    name: &str,
    role_arn: &str,
    bucket: &str,
    source_key: &str,
    project: &str,
    prefix: &str,
) -> Value {
    json!({"pipeline": {
        "name": name,
        "roleArn": role_arn,
        "artifactStore": {"type": "S3", "location": bucket},
        "stages": [
            {
                "name": "Source",
                "actions": [{
                    "name": "Source",
                    "actionTypeId": {
                        "category": "Source", "owner": "AWS", "provider": "S3", "version": "1",
                    },
                    "configuration": {
                        "S3Bucket": bucket,
                        "S3ObjectKey": source_key,
                        "PollForSourceChanges": "false",
                    },
                    "outputArtifacts": [{"name": "SourceArtifact"}],
                }],
            },
            {
                "name": "Build",
                "actions": [{
                    "name": "Build",
                    "actionTypeId": {
                        "category": "Build", "owner": "AWS", "provider": "CodeBuild",
                        "version": "1",
                    },
                    "configuration": {"ProjectName": project},
                    "inputArtifacts": [{"name": "SourceArtifact"}],
                    "outputArtifacts": [{"name": "BuildOutput"}],
                }],
            },
            {
                "name": "Deploy",
                "actions": [{
                    "name": "Deploy",
                    "actionTypeId": {
                        "category": "Deploy", "owner": "AWS", "provider": "S3", "version": "1",
                    },
                    "configuration": {
                        "BucketName": bucket,
                        "ObjectKey": prefix,
                        "Extract": "true",
                    },
                    "inputArtifacts": [{"name": "BuildOutput"}],
                }],
            },
        ],
    }})
}

/// One row of the artifact table: the case, the files the source archive carries, the
/// buildspec the project runs over them, and the files the output artifact must hold.
struct ArtifactRow {
    case: &'static str,
    staged: &'static [(&'static str, &'static str)],
    buildspec: &'static str,
    produced: &'static [(&'static str, &'static str)],
}

/// RULE: a Build action whose project's buildspec commands all run settles the execution
/// `Succeeded`, and the files the buildspec's `artifacts` section declares are the output
/// artifact the action publishes.
#[tokio::test]
async fn a_build_action_transforms_its_input_artifact_into_the_output_it_publishes() {
    let rows: &[ArtifactRow] = &[
        ArtifactRow {
            case: "one file copied to another name at the root",
            staged: &[("input.txt", "cp-marker-one")],
            buildspec: "version: 0.2\nphases:\n  build:\n    commands:\n      \
                        - cp input.txt output.txt\nartifacts:\n  files:\n    - output.txt\n",
            produced: &[("output.txt", "cp-marker-one")],
        },
        ArtifactRow {
            case: "a file copied into a directory the build made",
            staged: &[("input.txt", "cp-marker-two")],
            buildspec: "version: 0.2\nphases:\n  build:\n    commands:\n      \
                        - mkdir -p dist\n      - cp input.txt dist/output.txt\n\
                        artifacts:\n  files:\n    - dist/output.txt\n",
            produced: &[("dist/output.txt", "cp-marker-two")],
        },
        ArtifactRow {
            case: "two files of the source tree, both copied, declared by one pattern",
            staged: &[
                ("input.txt", "cp-marker-three"),
                ("nested/input.txt", "cp-marker-four"),
            ],
            buildspec: "version: 0.2\nphases:\n  build:\n    commands:\n      \
                        - cp input.txt first.out\n      - cp nested/input.txt second.out\n\
                        artifacts:\n  files:\n    - '*.out'\n",
            produced: &[
                ("first.out", "cp-marker-three"),
                ("second.out", "cp-marker-four"),
            ],
        },
    ];

    let router = emulator();
    let bucket = "pipeline-driven-build-artifacts";
    versioned_bucket(&router, bucket).await;
    let bucket_objects = format!("arn:aws:s3:::{bucket}/*");
    let build_role = role_allowing(
        &router,
        "pdb-build-role",
        CODEBUILD_TRUST,
        &[
            (&LOG_ACTIONS, "*"),
            (&ARTIFACT_ACTIONS, bucket_objects.as_str()),
        ],
    )
    .await;
    let pipeline_role = role_allowing(
        &router,
        "pdb-pipeline-role",
        PIPELINE_TRUST,
        &[
            (&BUILD_ACTIONS, "*"),
            (&ARTIFACT_ACTIONS, bucket_objects.as_str()),
        ],
    )
    .await;

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let source_key = format!("source-{index}.zip");
        let project = format!("pdb-project-{index}");
        let pipeline = format!("pdb-pipeline-{index}");
        let prefix = format!("deployed-{index}");

        let entries: Vec<(&str, &[u8])> = row
            .staged
            .iter()
            .map(|(path, contents)| (*path, contents.as_bytes()))
            .collect();
        let (status, body) = s3(
            &router,
            "PUT",
            &format!("/{bucket}/{source_key}"),
            &crate::execution_engine::zip_of(&entries),
        )
        .await;
        assert_eq!(
            status,
            200,
            "case {case:?}: the source object must be put, got {status} with {}",
            String::from_utf8_lossy(&body)
        );

        cb_ok(
            &router,
            case,
            "CreateProject",
            &json!({
                "name": project,
                "serviceRole": build_role,
                "source": {"type": "CODEPIPELINE", "buildspec": row.buildspec},
                "artifacts": {"type": "CODEPIPELINE"},
                "environment": {
                    "type": "LINUX_CONTAINER",
                    "image": "aws/codebuild/standard:7.0",
                    "computeType": "BUILD_GENERAL1_SMALL",
                },
            }),
        )
        .await;
        cp_ok(
            &router,
            case,
            "CreatePipeline",
            &source_build_deploy(
                &pipeline,
                &pipeline_role,
                bucket,
                &source_key,
                &project,
                &prefix,
            ),
        )
        .await;

        let (execution, status) = settled_execution(&router, case, &pipeline).await;
        let detail = action_execution(&router, case, &pipeline, &execution, "Build").await;
        assert_eq!(
            status, "Succeeded",
            "case {case:?}: every command of the buildspec runs, so the execution must \
             succeed, got {status} with the Build action reporting {detail}"
        );

        // What the action published for its output artifact, as a later action addresses it.
        let location = detail["output"]["outputArtifacts"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|artifact| artifact["name"].as_str() == Some("BuildOutput"))
            .map(|artifact| artifact["s3location"].clone())
            .unwrap_or(Value::Null);
        assert_eq!(
            location["bucket"].as_str(),
            Some(bucket),
            "case {case:?}: the output artifact must be published in the artifact store, \
             got {detail}"
        );
        assert!(
            location["key"].as_str().is_some_and(|key| !key.is_empty()),
            "case {case:?}: the output artifact must be published at a key, got {detail}"
        );

        // And the bytes behind it are the files the buildspec declared, as the Deploy action
        // extracted them.
        for (path, contents) in row.produced {
            let (status, bytes) = s3(&router, "GET", &format!("/{bucket}/{prefix}/{path}"), b"").await;
            assert_eq!(
                status, 200,
                "case {case:?}: the artifact must carry {path:?}, got {status}"
            );
            assert_eq!(
                String::from_utf8_lossy(&bytes),
                *contents,
                "case {case:?}: {path:?} must hold what the build wrote"
            );
        }
    }
}

/// The buildspec of the container rows: the daemon is started from the install phase the way
/// AWS documents a privileged build starting it, the Dockerfile is written out of the source
/// artifact the pipeline delivered, and the image is built and pushed to the destination
/// repository the environment names.
///
/// `bootstrap` is how the row starts and probes the daemon, which is the only part of it a row
/// varies.
fn container_buildspec(bootstrap: &[&str]) -> String {
    let commands: String = bootstrap
        .iter()
        .map(|command| {
            let quoted = command.replace('\\', "\\\\").replace('"', "\\\"");
            format!("      - \"{quoted}\"\n")
        })
        .collect();
    format!(
        "version: 0.2\nphases:\n  install:\n    commands:\n{commands}  pre_build:\n    \
         commands:\n      - \"printf 'FROM scratch\\\\nENV SOURCED=1\\\\n' > Dockerfile\"\n      \
         - \"aws ecr get-login-password --region $AWS_REGION | docker login --username AWS \
         --password-stdin $REGISTRY\"\n  build:\n    commands:\n      \
         - \"docker build -t $REPO_URI:$IMAGE_TAG .\"\n  post_build:\n    commands:\n      \
         - \"docker push $REPO_URI:$IMAGE_TAG\"\n"
    )
}

/// RULE: a Build action whose project runs a privileged container buildspec is the build's
/// outcome too — the image the commands pushed is in the destination registry and the
/// execution succeeded — and a role the registry actions are withheld from fails the build,
/// the action and the execution.
#[tokio::test]
async fn a_privileged_build_action_pushes_the_image_its_commands_built() {
    /// One row: the case, how the buildspec starts and probes the container daemon, whether
    /// the build's role is granted the registry actions, and the status the execution must
    /// reach.
    struct Row {
        case: &'static str,
        bootstrap: &'static [&'static str],
        granted: bool,
        execution_status: &'static str,
    }
    let rows = [
        Row {
            // The install phase AWS documents for a container build: the daemon started in
            // the background, and then the wait that polls it until it answers.
            case: "the documented bootstrap, under a role granted the registry actions",
            bootstrap: &[
                "nohup dockerd --storage-driver=overlay2 > /tmp/dockerd.log 2>&1 &",
                "timeout 15 sh -c \"until docker info; do sleep 1; done\"",
            ],
            granted: true,
            execution_status: "Succeeded",
        },
        Row {
            case: "the daemon started and asked about itself directly",
            bootstrap: &["dockerd &", "docker info"],
            granted: true,
            execution_status: "Succeeded",
        },
        Row {
            case: "the documented bootstrap, under a role those actions are withheld from",
            bootstrap: &[
                "nohup dockerd --storage-driver=overlay2 > /tmp/dockerd.log 2>&1 &",
                "timeout 15 sh -c \"until docker info; do sleep 1; done\"",
            ],
            granted: false,
            execution_status: "Failed",
        },
    ];

    let router = emulator();
    let bucket = "pipeline-driven-image-artifacts";
    versioned_bucket(&router, bucket).await;
    let bucket_objects = format!("arn:aws:s3:::{bucket}/*");
    let pipeline_role = role_allowing(
        &router,
        "pdi-pipeline-role",
        PIPELINE_TRUST,
        &[
            (&BUILD_ACTIONS, "*"),
            (&ARTIFACT_ACTIONS, bucket_objects.as_str()),
        ],
    )
    .await;

    // The source the pipeline's Source action delivers to the build: one image of a
    // repository, at the tag the action names.
    ecr_ok(
        &router,
        "setup",
        "CreateRepository",
        &json!({"repositoryName": "pdi-source"}),
    )
    .await;
    ecr_ok(
        &router,
        "setup",
        "PutImage",
        &json!({
            "repositoryName": "pdi-source",
            "imageManifest": json!({"schemaVersion": 2, "marker": "pdi"}).to_string(),
            "imageTag": "latest",
        }),
    )
    .await;

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let destination = format!("pdi-destination-{index}");
        let project = format!("pdi-project-{index}");
        let pipeline = format!("pdi-pipeline-{index}");
        let created = ecr_ok(
            &router,
            case,
            "CreateRepository",
            &json!({"repositoryName": destination}),
        )
        .await;
        let repository_uri = created["repository"]["repositoryUri"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the repository reports its URI, got {created}"))
            .to_string();
        let registry = repository_uri
            .split('/')
            .next()
            .unwrap_or(&repository_uri)
            .to_string();

        let mut grants: Vec<(&[&str], &str)> = vec![(&LOG_ACTIONS, "*")];
        grants.push((&ARTIFACT_ACTIONS, bucket_objects.as_str()));
        if row.granted {
            grants.push((&PUSH_ACTIONS, "*"));
        }
        let build_role = role_allowing(
            &router,
            &format!("pdi-build-role-{index}"),
            CODEBUILD_TRUST,
            &grants,
        )
        .await;

        cb_ok(
            &router,
            case,
            "CreateProject",
            &json!({
                "name": project,
                "serviceRole": build_role,
                "source": {
                    "type": "CODEPIPELINE",
                    "buildspec": container_buildspec(row.bootstrap),
                },
                "artifacts": {"type": "CODEPIPELINE"},
                "environment": {
                    "type": "LINUX_CONTAINER",
                    "image": "aws/codebuild/standard:7.0",
                    "computeType": "BUILD_GENERAL1_SMALL",
                    "privilegedMode": true,
                    "environmentVariables": [
                        {"name": "REGISTRY", "value": registry, "type": "PLAINTEXT"},
                        {"name": "REPO_URI", "value": repository_uri, "type": "PLAINTEXT"},
                        {"name": "IMAGE_TAG", "value": "pipeline-build", "type": "PLAINTEXT"},
                    ],
                },
            }),
        )
        .await;
        cp_ok(
            &router,
            case,
            "CreatePipeline",
            &json!({"pipeline": {
                "name": pipeline,
                "roleArn": pipeline_role,
                "artifactStore": {"type": "S3", "location": bucket},
                "stages": [
                    {
                        "name": "Source",
                        "actions": [{
                            "name": "Source",
                            "actionTypeId": {
                                "category": "Source", "owner": "AWS", "provider": "ECR",
                                "version": "1",
                            },
                            "configuration": {
                                "RepositoryName": "pdi-source",
                                "ImageTag": "latest",
                            },
                            "outputArtifacts": [{"name": "SourceArtifact"}],
                        }],
                    },
                    {
                        "name": "Build",
                        "actions": [{
                            "name": "Build",
                            "actionTypeId": {
                                "category": "Build", "owner": "AWS", "provider": "CodeBuild",
                                "version": "1",
                            },
                            "configuration": {"ProjectName": project},
                            "inputArtifacts": [{"name": "SourceArtifact"}],
                        }],
                    },
                ],
            }}),
        )
        .await;

        let (execution, status) = settled_execution(&router, case, &pipeline).await;
        let detail = action_execution(&router, case, &pipeline, &execution, "Build").await;
        let build_id = detail["output"]["executionResult"]["externalExecutionId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the action must report the build it started, got {detail}")
            })
            .to_string();
        let build = settled_build(&router, case, &build_id).await;
        assert_eq!(
            status, row.execution_status,
            "case {case:?}: expected the execution to reach {}, got {status}; the build \
             reported {build}",
            row.execution_status
        );

        let (_, listed) = json_call(
            &router,
            "ecr",
            ECR_TARGET,
            "DescribeImages",
            &json!({"repositoryName": destination}),
        )
        .await;
        let tags: Vec<String> = listed["imageDetails"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .flat_map(|detail| {
                detail["imageTags"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|tag| tag.as_str().map(str::to_string))
            })
            .collect();
        if row.granted {
            assert_eq!(
                build["buildStatus"].as_str(),
                Some("SUCCEEDED"),
                "case {case:?}: every command of the buildspec runs, so the build succeeds, \
                 got {build}"
            );
            assert_eq!(
                tags,
                vec!["pipeline-build".to_string()],
                "case {case:?}: the image the build pushed must be in {destination}, got \
                 {listed}"
            );
        } else {
            assert_eq!(
                build["buildStatus"].as_str(),
                Some("FAILED"),
                "case {case:?}: a command the role may not make fails the build, got {build}"
            );
            assert!(
                tags.is_empty(),
                "case {case:?}: no image may reach {destination}, got {listed}"
            );
        }
    }
}

/// The artifact a pipeline's Build action occupies in these rows.
const BUILD_ARTIFACT: &str = "BuildOutput";

/// The signature every ZIP archive ENDS with ("end of central directory record", ZIP APPNOTE
/// 4.3.16), which is the whole of an archive that carries no entry.
const ZIP_END_SIGNATURE: &[u8] = b"PK\x05\x06";

/// The signature a ZIP archive's entry begins with ("local file header signature", ZIP
/// APPNOTE 4.3.7): an archive carrying no entry has none of them.
const ZIP_ENTRY_SIGNATURE: &[u8] = b"PK\x03\x04";

/// A pipeline whose Source stage reads `source_key` out of `bucket` and whose Build stage
/// builds `project` into the artifact [`BUILD_ARTIFACT`] — with no stage after it, so what the
/// Build action published is read as the object it published rather than through a later
/// action.
fn source_and_build(
    name: &str,
    role_arn: &str,
    bucket: &str,
    source_key: &str,
    project: &str,
) -> Value {
    json!({"pipeline": {
        "name": name,
        "roleArn": role_arn,
        "artifactStore": {"type": "S3", "location": bucket},
        "stages": [
            {
                "name": "Source",
                "actions": [{
                    "name": "Source",
                    "actionTypeId": {
                        "category": "Source", "owner": "AWS", "provider": "S3", "version": "1",
                    },
                    "configuration": {
                        "S3Bucket": bucket,
                        "S3ObjectKey": source_key,
                        "PollForSourceChanges": "false",
                    },
                    "outputArtifacts": [{"name": "SourceArtifact"}],
                }],
            },
            {
                "name": "Build",
                "actions": [{
                    "name": "Build",
                    "actionTypeId": {
                        "category": "Build",
                        "owner": "AWS",
                        "provider": "CodeBuild",
                        "version": "1",
                    },
                    "configuration": {"ProjectName": project},
                    "inputArtifacts": [{"name": "SourceArtifact"}],
                    "outputArtifacts": [{"name": BUILD_ARTIFACT}],
                }],
            },
        ],
    }})
}

/// Whether `bytes` carries `wanted` anywhere in it.
fn carries(bytes: &[u8], wanted: &[u8]) -> bool {
    bytes.windows(wanted.len()).any(|window| window == wanted)
}

/// RULE: the destination a Build action hands the build is the object the PIPELINE assigned
/// that action's output artifact, and the action publishes that object whatever its project
/// builds — so a project whose buildspec declares no `artifacts` section at all settles the
/// execution `Succeeded` with the EMPTY archive published for the artifact, while one that
/// declares a section publishes the files it names. A section whose paths match nothing the
/// build produced is the build's own client error, which the action and the execution report
/// as the failure it is.
///
/// The rows differ along what the rule generalises over: whether the buildspec declares an
/// `artifacts` section at all, which files it declares, and whether what it declares is among
/// what the build produced.
#[tokio::test]
async fn a_build_action_publishes_its_output_artifact_whatever_the_buildspec_declares() {
    /// One row: the case, the buildspec the project runs, the status the execution settles
    /// at, and the entries the published archive carries.
    struct Row {
        case: &'static str,
        buildspec: &'static str,
        execution_status: &'static str,
        entries: &'static [&'static str],
    }

    let rows: &[Row] = &[
        Row {
            case: "a buildspec that declares no artifacts section at all",
            buildspec: "version: 0.2\nphases:\n  build:\n    commands:\n      - echo built\n",
            execution_status: "Succeeded",
            entries: &[],
        },
        Row {
            case: "a buildspec that declares the file its commands produced",
            buildspec: "version: 0.2\nphases:\n  build:\n    commands:\n      \
                        - cp input.txt output.txt\nartifacts:\n  files:\n    - output.txt\n",
            execution_status: "Succeeded",
            entries: &["output.txt"],
        },
        Row {
            case: "a buildspec that declares a file its commands never produced",
            buildspec: "version: 0.2\nphases:\n  build:\n    commands:\n      - echo built\n\
                        artifacts:\n  files:\n    - never-written.txt\n",
            execution_status: "Failed",
            entries: &[],
        },
    ];

    let router = emulator();
    let bucket = "pipeline-published-artifacts";
    versioned_bucket(&router, bucket).await;
    let bucket_objects = format!("arn:aws:s3:::{bucket}/*");
    let build_role = role_allowing(
        &router,
        "ppa-build-role",
        CODEBUILD_TRUST,
        &[
            (&LOG_ACTIONS, "*"),
            (&ARTIFACT_ACTIONS, bucket_objects.as_str()),
        ],
    )
    .await;
    let pipeline_role = role_allowing(
        &router,
        "ppa-pipeline-role",
        PIPELINE_TRUST,
        &[
            (&BUILD_ACTIONS, "*"),
            (&ARTIFACT_ACTIONS, bucket_objects.as_str()),
        ],
    )
    .await;

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let source_key = format!("ppa-source-{index}.zip");
        let project = format!("ppa-project-{index}");
        let pipeline = format!("ppa-pipeline-{index}");

        let entries: Vec<(&str, &[u8])> = vec![("input.txt", b"ppa-marker")];
        let (status, body) = s3(
            &router,
            "PUT",
            &format!("/{bucket}/{source_key}"),
            &crate::execution_engine::zip_of(&entries),
        )
        .await;
        assert_eq!(
            status,
            200,
            "case {case:?}: the source object must be put, got {status} with {}",
            String::from_utf8_lossy(&body)
        );

        cb_ok(
            &router,
            case,
            "CreateProject",
            &json!({
                "name": project,
                "serviceRole": build_role,
                "source": {"type": "CODEPIPELINE", "buildspec": row.buildspec},
                "artifacts": {"type": "CODEPIPELINE"},
                "environment": {
                    "type": "LINUX_CONTAINER",
                    "image": "aws/codebuild/standard:7.0",
                    "computeType": "BUILD_GENERAL1_SMALL",
                },
            }),
        )
        .await;
        cp_ok(
            &router,
            case,
            "CreatePipeline",
            &source_and_build(&pipeline, &pipeline_role, bucket, &source_key, &project),
        )
        .await;

        let (execution, status) = settled_execution(&router, case, &pipeline).await;
        let detail = action_execution(&router, case, &pipeline, &execution, "Build").await;
        assert_eq!(
            status, row.execution_status,
            "case {case:?}: expected the execution to reach {}, got {status} with the Build \
             action reporting {detail}",
            row.execution_status
        );

        // The build the action followed, whose own outcome the action reported.
        let build_id = detail["output"]["executionResult"]["externalExecutionId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the action must report the build it started, got {detail}")
            })
            .to_string();
        let build = settled_build(&router, case, &build_id).await;
        if row.execution_status == "Failed" {
            assert_eq!(
                build["buildStatus"].as_str(),
                Some("FAILED"),
                "case {case:?}: the action's failure is the build's own, got {build}"
            );
            continue;
        }
        assert_eq!(
            build["buildStatus"].as_str(),
            Some("SUCCEEDED"),
            "case {case:?}: every command of the buildspec runs, so the build succeeds, got \
             {build}"
        );

        // The object the action published for its output artifact, as a later action
        // addresses it — and the archive that object really holds.
        let location = detail["output"]["outputArtifacts"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|artifact| artifact["name"].as_str() == Some(BUILD_ARTIFACT))
            .map(|artifact| artifact["s3location"].clone())
            .unwrap_or(Value::Null);
        assert_eq!(
            location["bucket"].as_str(),
            Some(bucket),
            "case {case:?}: the output artifact must be published in the artifact store, got \
             {detail}"
        );
        let key = location["key"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the output artifact must be published at a key, got {detail}")
            })
            .to_string();
        let (status, bytes) = s3(&router, "GET", &format!("/{bucket}/{key}"), b"").await;
        assert_eq!(
            status, 200,
            "case {case:?}: the published artifact must be the object at {key}, got {status}"
        );
        assert!(
            carries(&bytes, ZIP_END_SIGNATURE),
            "case {case:?}: expected an archive at {key}, got {} bytes",
            bytes.len()
        );
        for entry in row.entries {
            assert!(
                carries(&bytes, entry.as_bytes()),
                "case {case:?}: expected the archive at {key} to carry {entry:?}, got {} bytes",
                bytes.len()
            );
        }
        if row.entries.is_empty() {
            assert!(
                !carries(&bytes, ZIP_ENTRY_SIGNATURE),
                "case {case:?}: a buildspec declaring no artifacts section publishes the EMPTY \
                 archive, got {} bytes at {key}",
                bytes.len()
            );
        }
    }
}
