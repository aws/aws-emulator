// SPDX-License-Identifier: CC0-1.0

//! The container CI/CD chain's build half, across the services it spans: what a CodeBuild
//! build's own commands DO, what Amazon ECR holds afterwards at the address it published, and
//! the Amazon ECS task that then runs that very image by digest.
//!
//! RULE under test: `StartBuild` of a project whose environment and buildspec are valid RUNS
//! the build. The move out of `IN_PROGRESS` is owed by the clock, and what the build reports
//! once it has settled is what its own commands answered: it walks the phases
//! `BuildPhaseType` names, in that enum's own order, each `phaseStatus` `SUCCEEDED` where
//! that phase's work succeeded — `PROVISIONING` among them, because a build environment this
//! emulator creates in process is one it provisioned — and `buildStatus` is `SUCCEEDED`
//! exactly when every command of every phase exited zero. `FAULT` is CodeBuild's code for an
//! internal failure, so no phase of a well-formed build carries it. A build whose commands
//! push a container image to a repository of this account leaves that image IN the registry,
//! addressable by the `sha256:` digest Amazon ECR reports for the tag pushed — so a task
//! definition pinning `<repositoryUri>@<digest>` is an image an Amazon ECS task really pulls,
//! and the task it places starts.
//!
//! Five crates have to agree for that to hold, which is why this suite lives here: IAM holds
//! the policies of the role the build RUNS AS and of the role the task PULLS AS, Amazon ECR
//! holds the registry the push uploads to and the digest the pull resolves, CodeBuild runs
//! the build, CloudWatch Logs holds the account of it the build's own log is, and Amazon ECS
//! pulls the image and places the task. Everything is driven through the real operations on
//! one shared engine, so what is asserted is what each service persisted — the push is read
//! back through Amazon ECR's own reads, never out of the build record, the log through
//! `GetLogEvents` and the pull through `DescribeTasks`.
//!
//! The rows differ along what the rule generalises over: which command phases the buildspec
//! declares (the walk the build reports is the modelled one either way), whether the
//! Dockerfile resolves the build argument the command passes, the body baked into the image,
//! the tag the push addresses and the repository it addresses.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The ONE percent-encoder and element reader this test crate keeps for awsQuery suites
// (`all/query_wire.rs`: "Nothing new should write another one — call this module").
use crate::query_wire::{form_encode, opt_element};

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// The network the task is placed in, held in the account's own Amazon EC2 records because
/// the placement names it literally.
const VPC: &str = "vpc-0123456789abcdef0";
const SUBNET: &str = "subnet-0123456789abcdef0";

/// The cluster every task below runs in.
const CLUSTER: &str = "cicdchain";

/// The container of every task definition below.
const CONTAINER: &str = "app";

/// The trust policy of the role a build RUNS AS: CodeBuild assumes it for the build.
const CODEBUILD_TRUST: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
"Principal":{"Service":"codebuild.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The trust policy of the role a TASK pulls as (`TaskDefinition$executionRoleArn`).
const ECS_TASKS_TRUST: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
"Principal":{"Service":"ecs-tasks.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The actions AWS documents a role needs to PUSH an image to a repository (Amazon ECR User
/// Guide, the pushing permissions it lists for a repository), beside the login the push
/// presents.
const PUSH_ACTIONS: [&str; 6] = [
    "ecr:GetAuthorizationToken",
    "ecr:BatchCheckLayerAvailability",
    "ecr:InitiateLayerUpload",
    "ecr:UploadLayerPart",
    "ecr:CompleteLayerUpload",
    "ecr:PutImage",
];

/// The two calls a task's image PULL makes, which its execution role has to be allowed.
const PULL_ACTIONS: [&str; 2] = ["ecr:GetAuthorizationToken", "ecr:BatchGetImage"];

/// The actions AWS documents a build's service role needs to deliver its log events (the
/// CodeBuild service-role policy its User Guide publishes).
const LOG_ACTIONS: [&str; 3] = [
    "logs:CreateLogGroup",
    "logs:CreateLogStream",
    "logs:PutLogEvents",
];

/// The group the chain's builds deliver their logs to, created through CloudWatch Logs' own
/// API before the first build runs.
const BUILD_LOG_GROUP: &str = "cicd-chain-build-logs";

/// `BuildPhaseType`'s own members, in the order that enum lists them — which is the order a
/// build walks its phases in
/// (`smithy_query(query="shape", shape="com.amazonaws.codebuild#BuildPhaseType")`).
const PHASE_WALK: [&str; 11] = [
    "SUBMITTED",
    "QUEUED",
    "PROVISIONING",
    "DOWNLOAD_SOURCE",
    "INSTALL",
    "PRE_BUILD",
    "BUILD",
    "POST_BUILD",
    "UPLOAD_ARTIFACTS",
    "FINALIZING",
    "COMPLETED",
];

/// The `StatusType` value a build, and a phase of it, that succeeded carries, and the one a
/// build that has not settled carries
/// (`smithy_query(query="shape", shape="com.amazonaws.codebuild#StatusType")`).
const SUCCEEDED: &str = "SUCCEEDED";
const IN_PROGRESS: &str = "IN_PROGRESS";

/// The terminal marker phase of a build, which is open while the build completes and so
/// carries no `phaseStatus` of its own.
const PHASE_COMPLETED: &str = "COMPLETED";

/// How long a wait for a clock-owed move may take before the suite calls it stalled, and how
/// long it leaves before looking again. Generous on purpose: the wait ends on the CONDITION,
/// and the deadline is only there so a move that never happens fails the test instead of
/// hanging it.
const DEADLINE: Duration = Duration::from_secs(60);
const LOOK_AGAIN_AFTER: Duration = Duration::from_millis(50);

/// One emulator carrying the five services this chain spans over ONE shared engine, with the
/// network the task placement names held in the account's Amazon EC2 records.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    crate::ec2_network::register_network(&engine, VPC, &[SUBNET], &format!("{REGION}a"));
    // The placement below is given a public address on a subnet that routes to an internet
    // gateway, which is the path Amazon ECS reads off these records before it starts a task.
    crate::ec2_network::register_internet_route(&engine, VPC);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_codebuild_codebuild_20161006::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(engine),
    );
    router
}

/// One awsJson1_1 call to the service `signing_name` serves, answered with the HTTP status and
/// the parsed body.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}\
                 /aws4_request, SignedHeaders=host;x-amz-target, Signature=abc"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the test body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let decoded: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    (status, decoded)
}

/// One CodeBuild call, answered with the HTTP status and the parsed body.
async fn codebuild(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    rpc(router, "codebuild", "CodeBuild_20161006", operation, body).await
}

/// One CodeBuild call that has to succeed.
async fn codebuild_ok(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, answer) = codebuild(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: codebuild {operation} must answer 200, got {status} with {answer}"
    );
    answer
}

/// One Amazon ECR call that has to succeed.
async fn ecr_ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, answer) = rpc(
        router,
        "ecr",
        "AmazonEC2ContainerRegistry_V20150921",
        operation,
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ecr {operation} must answer 200, got {status} with {answer}"
    );
    answer
}

/// One Amazon ECS call that has to succeed.
async fn ecs_ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, answer) = rpc(
        router,
        "ecs",
        "AmazonEC2ContainerServiceV20141113",
        operation,
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ecs {operation} must answer 200, got {status} with {answer}"
    );
    answer
}

/// The messages the stream `stream` of the group `group` holds, as CloudWatch Logs' own read
/// reports them — and `None` when it holds no such stream at all.
async fn delivered_messages(
    router: &MultiServiceRouter,
    group: &str,
    stream: &str,
) -> Option<Vec<String>> {
    let (status, read) = rpc(
        router,
        "logs",
        "Logs_20140328",
        "GetLogEvents",
        &json!({"logGroupName": group, "logStreamName": stream}),
    )
    .await;
    if status != 200 {
        return None;
    }
    Some(
        read["events"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|event| event["message"].as_str().map(str::to_string))
            .collect(),
    )
}

/// One awsQuery call to IAM, answered with the HTTP status and the XML body.
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Create the role `name`, trusted by `trust`, whose one inline policy allows `actions`, and
/// answer its ARN — through IAM's own API, so the permissions the two services read are the
/// ones that service persisted.
async fn role_allowing(
    router: &MultiServiceRouter,
    name: &str,
    trust: &str,
    actions: &[&str],
) -> String {
    let (status, created) = iam(
        router,
        "CreateRole",
        &[("RoleName", name), ("AssumeRolePolicyDocument", trust)],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateRole {name:?}: expected the role to be persisted, got {status}: {created}"
    );
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": actions, "Resource": "*"}],
    })
    .to_string();
    let (status, attached) = iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", name),
            ("PolicyName", "chain-permissions"),
            ("PolicyDocument", &document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "PutRolePolicy {name:?}: expected the policy to be persisted, got {status}: {attached}"
    );
    opt_element(&created, "Arn").unwrap_or_else(|| panic!("no <Arn> in {created}"))
}

/// One row of the table: the Dockerfile the build materialises, the body it bakes in, the
/// repository the push addresses, the tag it pushes, and whether the buildspec declares an
/// `install` section — a build walks the phase CodeBuild runs the installs in whether or not
/// its specification declares commands for it.
struct Row {
    case: &'static str,
    dockerfile: &'static str,
    body: &'static str,
    repository: &'static str,
    tag: &'static str,
    installs: bool,
}

/// A Dockerfile that RESOLVES the build argument into an instruction of the image, so the
/// body the build passes is part of what the image declares.
const USES_THE_ARGUMENT: &str =
    "FROM scratch\nARG BODY\nENV SERVED_BODY=$BODY\nEXPOSE 80\nCMD [\"/serve\"]\n";

/// One that declares no `ARG`, so the image it composes is the same whatever the build passes.
/// It still declares the program its own image runs (`CMD`): a container runs the argument
/// vector its definition or its image names, so an image declaring neither is one no container
/// can be started out of — which is a rule of its own
/// (`service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::image_pull`) and not
/// what this row is about.
const DECLARES_NO_ARGUMENT: &str =
    "FROM scratch\nENV SERVED_BODY=fixed\nEXPOSE 8080\nCMD [\"/sidecar\"]\n";

const ROWS: [Row; 3] = [
    Row {
        case: "the chain's own shape: a Dockerfile resolving the baked body, every command \
               phase declared",
        dockerfile: USES_THE_ARGUMENT,
        body: "token-one build-one v1",
        repository: "chain-app",
        tag: "v1",
        installs: true,
    },
    Row {
        case: "another body and tag of the same repository, with no install phase declared",
        dockerfile: USES_THE_ARGUMENT,
        body: "token-two build-two v2",
        repository: "chain-app",
        tag: "v2",
        installs: false,
    },
    Row {
        case: "a document that resolves no build argument, pushed to another repository",
        dockerfile: DECLARES_NO_ARGUMENT,
        body: "token-three build-three v3",
        repository: "chain-sidecar",
        tag: "latest",
        installs: true,
    },
];

/// The buildspec of a container-image build, as the chain's own commands write it: the
/// Dockerfile travels as a Base64-encoded environment variable and is decoded into the
/// working tree, the build logs in to the registry with the password Amazon ECR issues,
/// builds the image with the body as a build argument and pushes it.
fn buildspec(installs: bool) -> String {
    let install = if installs {
        "  install:\n    commands:\n      - 'echo \"installing\"'\n"
    } else {
        ""
    };
    format!(
        "version: 0.2\nphases:\n{install}  pre_build:\n    commands:\n      - 'echo \
         \"$DOCKERFILE_B64\" | base64 -d > Dockerfile'\n      - 'aws ecr get-login-password \
         --region $AWS_REGION | docker login --username AWS --password-stdin $REGISTRY'\n  \
         build:\n    commands:\n      - 'docker build --build-arg \"BODY=$BODY\" -t \
         \"$REPO_URI:$IMAGE_TAG\" .'\n  post_build:\n    commands:\n      - 'docker push \
         \"$REPO_URI:$IMAGE_TAG\"'\n"
    )
}

/// The `(phaseType, phaseStatus)` pairs a build reports, in the order it reports them.
fn phases_of(build: &Value) -> Vec<(String, Option<String>)> {
    build["phases"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|phase| {
            (
                phase["phaseType"].as_str().unwrap_or_default().to_string(),
                phase["phaseStatus"].as_str().map(str::to_string),
            )
        })
        .collect()
}

/// Look again at one build until the transition the clock owes it has been materialised, and
/// answer the settled record `BatchGetBuilds` reports.
async fn settled_build(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let read = codebuild_ok(router, case, "BatchGetBuilds", &json!({"ids": [id]})).await;
        let build = read["builds"][0].clone();
        if build["buildComplete"] == json!(true) {
            return build;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the build {id} to settle, last read {build}"
        );
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}

/// Look again at one task until it has STARTED — a task that reports `startedAt` is one whose
/// image pull was served and whose containers were launched — and answer that record.
async fn started_task(router: &MultiServiceRouter, case: &str, task_arn: &str) -> Value {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let read = ecs_ok(
            router,
            case,
            "DescribeTasks",
            &json!({"cluster": CLUSTER, "tasks": [task_arn]}),
        )
        .await;
        let task = read["tasks"][0].clone();
        if task["startedAt"].is_null() {
            assert_ne!(
                task["lastStatus"].as_str(),
                Some("STOPPED"),
                "case {case:?}: the task must pull the image the build pushed and start, it \
                 stopped instead: {task}"
            );
        } else {
            return task;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the task to start, last read {task}"
        );
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}

/// RULE: a build whose commands build and push a container image settles `SUCCEEDED` with
/// every phase it walked `SUCCEEDED`, the registry then holds that image under the tag pushed
/// with a `sha256:` digest of its own, and an Amazon ECS task definition pinning that digest
/// is an image a task really pulls and starts.
#[tokio::test]
async fn a_builds_pushed_image_walks_the_build_phases_and_then_runs_on_ecs() {
    let router = emulator();
    let suite = "the suite's own records";

    // The repositories, created through Amazon ECR's own API. The URI each one publishes
    // names the registry that holds it, so it is the reference the build's commands push to
    // and the one the task definition pins.
    let mut uris: BTreeMap<&str, String> = BTreeMap::new();
    for repository in ["chain-app", "chain-sidecar"] {
        let created = ecr_ok(
            &router,
            suite,
            "CreateRepository",
            &json!({"repositoryName": repository}),
        )
        .await;
        let uri = created["repository"]["repositoryUri"]
            .as_str()
            .unwrap_or_else(|| panic!("case {suite:?}: the create reports a repositoryUri"))
            .to_string();
        let (authority, _path) = uri
            .split_once('/')
            .unwrap_or_else(|| panic!("case {suite:?}: {uri:?} names no repository path"));
        assert!(
            !authority.is_empty()
                && !authority.to_ascii_lowercase().ends_with(".amazonaws.com")
                && uri.ends_with(&format!("/{repository}")),
            "case {suite:?}: a push and a pull DIAL the published URI, so it names an address \
             this emulator answers registry requests at and the repository itself, and never \
             a host AWS serves, which resolves nowhere near this process, got {uri:?}"
        );
        uris.insert(repository, uri);
    }

    // The role the build RUNS AS carries both halves of what its commands do: the registry
    // calls the push makes, and the delivery of the build's own log.
    let build_actions: Vec<&str> = PUSH_ACTIONS.iter().chain(LOG_ACTIONS.iter()).copied().collect();
    let build_role = role_allowing(&router, "cicd-build", CODEBUILD_TRUST, &build_actions).await;
    let execution_role =
        role_allowing(&router, "cicd-task", ECS_TASKS_TRUST, &PULL_ACTIONS).await;
    let (status, created) = rpc(
        &router,
        "logs",
        "Logs_20140328",
        "CreateLogGroup",
        &json!({"logGroupName": BUILD_LOG_GROUP}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {suite:?}: the build log group must be created, got {status} with {created}"
    );
    ecs_ok(
        &router,
        suite,
        "CreateCluster",
        &json!({"clusterName": CLUSTER}),
    )
    .await;

    let mut digests: Vec<String> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let repository_uri = uris
            .get(row.repository)
            .unwrap_or_else(|| panic!("case {case:?}: the row's repository was created"))
            .clone();
        let registry = repository_uri
            .split('/')
            .next()
            .unwrap_or(&repository_uri)
            .to_string();

        let project = format!("cicd-chain-{index}");
        codebuild_ok(
            &router,
            case,
            "CreateProject",
            &json!({
                "name": project,
                "serviceRole": build_role,
                "source": {"type": "NO_SOURCE", "buildspec": buildspec(row.installs)},
                "artifacts": {"type": "NO_ARTIFACTS"},
                "environment": {
                    "type": "LINUX_CONTAINER",
                    "image": "aws/codebuild/amazonlinux2-x86_64-standard:5.0",
                    "computeType": "BUILD_GENERAL1_SMALL",
                    "privilegedMode": true,
                },
                "logsConfig": {"cloudWatchLogs": {
                    "status": "ENABLED",
                    "groupName": BUILD_LOG_GROUP,
                }},
            }),
        )
        .await;

        // The values the chain hands the build at START time, which is where the client's own
        // token, tag and Dockerfile travel.
        let started = codebuild_ok(
            &router,
            case,
            "StartBuild",
            &json!({
                "projectName": project,
                "environmentVariablesOverride": [
                    {"name": "REGISTRY", "value": registry, "type": "PLAINTEXT"},
                    {"name": "REPO_URI", "value": repository_uri, "type": "PLAINTEXT"},
                    {"name": "IMAGE_TAG", "value": row.tag, "type": "PLAINTEXT"},
                    {"name": "BODY", "value": row.body, "type": "PLAINTEXT"},
                    {
                        "name": "DOCKERFILE_B64",
                        "value": aws_smithy_types::base64::encode(row.dockerfile),
                        "type": "PLAINTEXT",
                    },
                ],
            }),
        )
        .await;
        assert_eq!(
            started["build"]["buildStatus"].as_str(),
            Some(IN_PROGRESS),
            "case {case:?}: the start answers a build that is under way, got {started}"
        );
        let id = started["build"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the start reports a build id, got {started}"))
            .to_string();

        // What the build came to, read back once the clock owed its terminal transition.
        let build = settled_build(&router, case, &id).await;
        assert_eq!(
            build["buildStatus"].as_str(),
            Some(SUCCEEDED),
            "case {case:?}: every command of the build exited zero, so the build succeeded, \
             got {build}"
        );
        let phases = phases_of(&build);
        let walked: Vec<&str> = phases.iter().map(|(phase, _)| phase.as_str()).collect();
        assert_eq!(
            walked, PHASE_WALK,
            "case {case:?}: expected the build to walk the phases BuildPhaseType names, in that \
             enum's own order, got {phases:?}"
        );
        for (phase, status) in &phases {
            if phase == PHASE_COMPLETED {
                continue;
            }
            assert_eq!(
                status.as_deref(),
                Some(SUCCEEDED),
                "case {case:?}: the {phase} phase of a build whose commands succeeded is \
                 SUCCEEDED, got {phases:?}"
            );
        }

        // The build's own LOG is its account of what it did, delivered to the group the
        // project configured and the stream the record names: the agent's phase lines are in
        // it, so a client reads there that the build's commands ran and how each phase ended.
        let group = build["logs"]["groupName"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the build reports its log group, got {build}"))
            .to_string();
        let stream = build["logs"]["streamName"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the build reports its log stream, got {build}")
            })
            .to_string();
        assert_eq!(
            group, BUILD_LOG_GROUP,
            "case {case:?}: the build's log goes to the group its project configured, got \
             {group:?}"
        );
        let messages = delivered_messages(&router, &group, &stream)
            .await
            .unwrap_or_else(|| {
                panic!("case {case:?}: CloudWatch Logs must hold the stream {group}/{stream}")
            });
        assert!(
            messages
                .iter()
                .any(|message| message == "Phase complete: BUILD State: SUCCEEDED"),
            "case {case:?}: the build's log carries how its BUILD phase ended, got {messages:?}"
        );

        // What the REGISTRY holds is what says the push happened: read through Amazon ECR's
        // own read of the tag the push addressed.
        let described = ecr_ok(
            &router,
            case,
            "DescribeImages",
            &json!({
                "repositoryName": row.repository,
                "imageIds": [{"imageTag": row.tag}],
            }),
        )
        .await;
        let detail = described["imageDetails"][0].clone();
        let digest = detail["imageDigest"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the registry reports the pushed image's digest, got {described}")
            })
            .to_string();
        assert!(
            digest.starts_with("sha256:"),
            "case {case:?}: an image is addressed by the digest of its manifest, got {digest:?}"
        );
        assert!(
            detail["imageSizeInBytes"].as_i64().unwrap_or_default() > 0,
            "case {case:?}: the image the push uploaded carries bytes, got {detail}"
        );
        digests.push(digest.clone());

        // And that digest is an image Amazon ECS pulls: the revision pins it by digest, and
        // the task placed on it starts rather than failing its pull.
        let family = format!("cicd-chain-{index}");
        let image = format!("{repository_uri}@{digest}");
        ecs_ok(
            &router,
            case,
            "RegisterTaskDefinition",
            &json!({
                "family": family,
                "networkMode": "awsvpc",
                "requiresCompatibilities": ["FARGATE"],
                "cpu": "256",
                "memory": "512",
                "executionRoleArn": execution_role,
                "containerDefinitions": [{
                    "name": CONTAINER,
                    "image": image,
                    "essential": true,
                    "portMappings": [{"containerPort": 80, "protocol": "tcp"}],
                }],
            }),
        )
        .await;
        let run = ecs_ok(
            &router,
            case,
            "RunTask",
            &json!({
                "cluster": CLUSTER,
                "taskDefinition": family,
                "launchType": "FARGATE",
                "networkConfiguration": {
                    "awsvpcConfiguration": {
                        "subnets": [SUBNET],
                        "assignPublicIp": "ENABLED",
                    }
                },
            }),
        )
        .await;
        let task_arn = run["tasks"][0]["taskArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: RunTask places one task, got {run}"))
            .to_string();
        let task = started_task(&router, case, &task_arn).await;
        assert!(
            task["pullStoppedAt"].as_f64().is_some() || task["pullStoppedAt"].is_string(),
            "case {case:?}: a task that started pulled its image, so it reports when the \
             fetch ended, got {task}"
        );
        let container = task["containers"][0].clone();
        assert_eq!(
            container["image"].as_str(),
            Some(image.as_str()),
            "case {case:?}: the container runs the image the revision pinned, got {container}"
        );
    }

    // Two builds that passed different bodies to a Dockerfile that resolves one composed
    // different images, and the one that resolves none composed its own: the digests the
    // registry reports are its answer, not the suite's.
    assert_ne!(
        digests[0], digests[1],
        "a build argument the Dockerfile resolves changes the image, so the two digests \
         differ, got {digests:?}"
    );
}
