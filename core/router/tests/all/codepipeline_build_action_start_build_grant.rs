// SPDX-License-Identifier: CC0-1.0

//! A CodePipeline `Build` action is authorized against the role it is PERFORMED AS before
//! any build exists, and a build it does start runs its project's buildspec.
//!
//! The rule under test: **CodePipeline performs a `Build` action by calling CodeBuild's
//! `StartBuild` under the role the action runs as — its own `roleArn` when the declaration
//! states one, and the pipeline's otherwise — so that call is decided by the identity-based
//! policies in force on THAT role before anything is started: a role those policies do not
//! allow the action gets no build at all (the action is `Failed` with
//! `ActionExecutionResult$errorDetails.code` `JobFailed`, no `externalExecutionId`, and
//! CodeBuild holds no build of the project), while a role they allow gets a real build, which
//! then runs the project's buildspec to a terminal `buildStatus` of its own.**
//!
//! Two services have to agree for that to hold (IAM holds the role and its policies,
//! CodePipeline takes the decision, CodeBuild would hold the build), so this suite lives
//! here.
//!
//! The rows differ along what the rule generalises over: which role the action is performed
//! as, and the SHAPE of the grant in force on it — the project's own ARN, every project of
//! the account, every action of the service, another project's ARN, a statement that denies
//! the action beside one that allows it, a policy set naming the service not at all, and no
//! identity-based policy whatsoever (the state a role provisioned out of band is in, which
//! says nothing about what it may do).
//!
//! A build the authorized rows DO start is held to the second half of the rule: it ends in
//! the status its buildspec's commands answered, with no phase reporting CodeBuild's own
//! `FAULT` — the status reserved for a condition the caller did not cause — and nothing in
//! what a client reads announcing this emulator.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every call of this suite is signed for.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = ::engine::EMULATOR_ACCOUNT_ID;

/// CodePipeline's `X-Amz-Target` prefix, from that service's own model.
const CODEPIPELINE_TARGET_PREFIX: &str = "CodePipeline_20150709";

/// CodeBuild's `X-Amz-Target` prefix, from that service's own model.
const CODEBUILD_TARGET_PREFIX: &str = "CodeBuild_20161006";

/// The IAM action CodePipeline performs a `CodeBuild` action's work with ("Actions defined by
/// AWS CodeBuild", IAM Service Authorization Reference).
const START_BUILD_ACTION: &str = "codebuild:StartBuild";

/// The `FailureType` a run that failed at its own work reports
/// (`smithy_query(query="shape",shape="FailureType")`), which is what an action nothing was
/// started for carries.
const JOB_FAILED: &str = "JobFailed";

/// The `PipelineExecutionStatus` values an execution never leaves.
const TERMINAL: [&str; 5] = [
    "Succeeded",
    "Failed",
    "Stopped",
    "Superseded",
    "Cancelled",
];

/// How long a wait for one execution's next state may take before the suite calls it
/// stalled. Generous: every row of the table waits under this one budget, and none of them
/// is testing a deadline.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(60);

/// How often a wait re-reads the reporting operation it polls.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The trust policy a pipeline role carries.
const PIPELINE_TRUST: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
"Principal":{"Service":"codepipeline.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The trust policy a build project's service role carries.
const CODEBUILD_TRUST: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
"Principal":{"Service":"codebuild.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The buildspec every project of this suite runs: one command that answers success, so a
/// build that was really started ends `SUCCEEDED`.
const BUILDSPEC: &str = "version: 0.2\nphases:\n  build:\n    commands:\n      - echo building\n";

/// One inline identity-based policy document, written for the project the action names.
type Document = fn(&str) -> String;

/// A policy set that names the artifact store and nothing of the build service: it STATES
/// what the role may do, and `codebuild:StartBuild` is not in it.
fn only_the_artifact_store(_project: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": ["s3:GetObject", "s3:GetObjectVersion", "s3:PutObject"],
            "Resource": "*",
        }],
    })
    .to_string()
}

/// A grant of the action on the project's OWN ARN.
fn the_projects_own_arn(project: &str) -> String {
    grant(START_BUILD_ACTION, &project_arn(project))
}

/// A grant of the action on every project of this account.
fn every_project_of_the_account(_project: &str) -> String {
    grant(START_BUILD_ACTION, &project_arn("*"))
}

/// A grant of every action of the build service on the project.
fn every_action_of_the_service(project: &str) -> String {
    grant("codebuild:*", &project_arn(project))
}

/// A grant of the action on a DIFFERENT project's ARN: a statement about another resource
/// says nothing about this one.
fn another_projects_arn(project: &str) -> String {
    grant(START_BUILD_ACTION, &project_arn(&format!("{project}-other")))
}

/// A statement that DENIES the action on the project beside one that allows it everywhere.
fn a_deny_beside_a_grant(project: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            { "Effect": "Allow", "Action": START_BUILD_ACTION, "Resource": "*" },
            {
                "Effect": "Deny",
                "Action": START_BUILD_ACTION,
                "Resource": project_arn(project),
            },
        ],
    })
    .to_string()
}

/// One `Allow` of `action` on `resource`.
fn grant(action: &str, resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": action, "Resource": resource }],
    })
    .to_string()
}

/// The ARN of the CodeBuild project `name` in this account and Region
/// (`arn:<partition>:codebuild:<region>:<account>:project/<name>`, "Resource types defined by
/// Amazon CodeBuild"), which is the resource the start is authorized on.
fn project_arn(name: &str) -> String {
    format!("arn:aws:codebuild:{REGION}:{ACCOUNT}:project/{name}")
}

/// One row of the table: the case, the policy in force on the pipeline's role, the policy in
/// force on the action's own role when the declaration states one, and whether the start is
/// authorized.
struct Row {
    case: &'static str,
    pipeline_policy: Option<Document>,
    action_policy: Option<Document>,
    authorized: bool,
}

/// The emulator this suite drives: the whole router over one engine, and the runtime its
/// requests are blocked on.
struct Emulator {
    router: MultiServiceRouter,
    runtime: tokio::runtime::Runtime,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        let mut router = build_router(engine);
        router.set_request_authentication(false);
        Self {
            router,
            runtime: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("a multi-thread runtime"),
        }
    }

    fn send(
        &self,
        signing_name: &str,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> (u16, Vec<u8>) {
        let host = format!("{signing_name}.{REGION}.amazonaws.com");
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{uri}"))
            .header("host", host)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/\
                     aws4_request, SignedHeaders=host, Signature=deadbeef"
                ),
            );
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(bytes::Bytes::from(body))
            .expect("a well-formed request");
        let response = self.runtime.block_on(self.router.dispatch(request));
        let status = response.status().as_u16();
        (status, response.into_body().to_vec())
    }

    /// One awsJson1_1 operation of `signing_name`, under `prefix`.
    fn json_rpc(
        &self,
        signing_name: &str,
        prefix: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let (status, payload) = self.send(
            signing_name,
            "POST",
            "/",
            &[
                ("content-type", "application/x-amz-json-1.1"),
                ("x-amz-target", &format!("{prefix}.{operation}")),
            ],
            serde_json::to_vec(body).expect("the request body serializes"),
        );
        let parsed = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&payload).unwrap_or_else(|_| {
                json!({ "unparsed": String::from_utf8_lossy(&payload).into_owned() })
            })
        };
        (status, parsed)
    }

    /// One CodePipeline operation that has to succeed.
    fn cp_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, answer) =
            self.json_rpc("codepipeline", CODEPIPELINE_TARGET_PREFIX, operation, body);
        assert_eq!(
            status, 200,
            "case {case:?}: codepipeline {operation} must answer 200, got {status} with {answer}"
        );
        answer
    }

    /// One CodeBuild operation that has to succeed.
    fn cb_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, answer) =
            self.json_rpc("codebuild", CODEBUILD_TARGET_PREFIX, operation, body);
        assert_eq!(
            status, 200,
            "case {case:?}: codebuild {operation} must answer 200, got {status} with {answer}"
        );
        answer
    }

    /// One IAM action (awsQuery, API version 2010-05-08).
    fn iam(&self, action: &str, members: &[(&str, &str)]) -> (u16, String) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (key, value) in members {
            body.push('&');
            body.push_str(&form_encode(key));
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let (status, payload) = self.send(
            "iam",
            "POST",
            "/",
            &[("content-type", "application/x-www-form-urlencoded")],
            body.into_bytes(),
        );
        (status, String::from_utf8_lossy(&payload).into_owned())
    }

    /// Create a role carrying `trust`, and answer its ARN.
    fn role(&self, name: &str, trust: &str) -> String {
        let (status, body) = self.iam(
            "CreateRole",
            &[("RoleName", name), ("AssumeRolePolicyDocument", trust)],
        );
        assert_eq!(
            status, 200,
            "setup CreateRole {name}: expected 200, got {status} with body {body}"
        );
        element(&body, "Arn")
    }

    /// Put `document` in force on the role `name` as its inline policy, the way
    /// `iam:PutRolePolicy` stores one.
    fn role_policy(&self, name: &str, document: &str) {
        let (status, body) = self.iam(
            "PutRolePolicy",
            &[
                ("RoleName", name),
                ("PolicyName", "granted"),
                ("PolicyDocument", document),
            ],
        );
        assert_eq!(
            status, 200,
            "setup PutRolePolicy {name}: expected 200, got {status} with body {body}"
        );
    }

    /// Create a bucket and stage, at `key`, the ZIP archive a build can run on.
    ///
    /// The object a pipeline's source action delivers to a build IS an archive — "the input
    /// artifact must be a ZIP file", which CodeBuild unpacks into the build's working tree
    /// before the first command runs — so the suite stages one rather than bytes no build
    /// could run on. It is written by a build of its own, whose `ZIP` artifacts are exactly
    /// that one object, so nothing here writes an archive by hand.
    fn staged_source(&self, case: &str, bucket: &str, key: &str, role_arn: &str) {
        let (status, answer) = self.send("s3", "PUT", &format!("/{bucket}"), &[], Vec::new());
        assert_eq!(
            status,
            200,
            "setup CreateBucket {bucket}: expected 200, got {status} with {:?}",
            String::from_utf8_lossy(&answer)
        );
        let project = format!("stage-{bucket}");
        self.cb_ok(
            case,
            "CreateProject",
            &json!({
                "name": project,
                "serviceRole": role_arn,
                "source": {
                    "type": "NO_SOURCE",
                    "buildspec": "version: 0.2\nphases:\n  build:\n    commands:\n      \
                                  - \"printf 'the source payload' > app.txt\"\nartifacts:\n  \
                                  files:\n    - app.txt\n",
                },
                "artifacts": {
                    "type": "S3",
                    "location": bucket,
                    "name": key,
                    "packaging": "ZIP",
                },
                "environment": {
                    "type": "LINUX_CONTAINER",
                    "image": "aws/codebuild/standard:7.0",
                    "computeType": "BUILD_GENERAL1_SMALL",
                },
            }),
        );
        let started = self.cb_ok(case, "StartBuild", &json!({ "projectName": project }));
        let id = started["build"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("setup StartBuild must mint a build, got {started}"))
            .to_string();
        let staged = settle(case, "the staged source archive", || {
            let build = self.build(case, &id);
            if build["buildComplete"] == Value::Bool(true) {
                Ok(build)
            } else {
                Err(build.to_string())
            }
        });
        assert_eq!(
            staged["buildStatus"].as_str(),
            Some("SUCCEEDED"),
            "setup: the build that stages {key} must succeed, got {staged}"
        );
    }

    /// The builds CodeBuild holds for `project`.
    fn builds_of(&self, case: &str, project: &str) -> Vec<String> {
        let answer = self.cb_ok(
            case,
            "ListBuildsForProject",
            &json!({ "projectName": project }),
        );
        answer["ids"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|id| id.as_str().map(str::to_string))
            .collect()
    }

    /// One build as `BatchGetBuilds` reports it.
    fn build(&self, case: &str, id: &str) -> Value {
        let answer = self.cb_ok(case, "BatchGetBuilds", &json!({ "ids": [id] }));
        answer["builds"]
            .as_array()
            .and_then(|builds| builds.first())
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: CodeBuild must hold the build {id}"))
    }

    /// The state of one action of one stage, as `GetPipelineState` reports it.
    fn action_state(&self, case: &str, pipeline: &str, stage: &str, action: &str) -> Value {
        let answer = self.cp_ok(case, "GetPipelineState", &json!({ "name": pipeline }));
        answer["stageStates"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|state| state["stageName"].as_str() == Some(stage))
            .and_then(|state| {
                state["actionStates"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .find(|state| state["actionName"].as_str() == Some(action))
            })
            .unwrap_or_else(|| panic!("case {case:?}: no state for {stage}/{action}"))
    }

    /// The one record `ListActionExecutions` reports for `action` of `pipeline`.
    fn record_of(&self, case: &str, pipeline: &str, action: &str) -> Value {
        let answer = self.cp_ok(
            case,
            "ListActionExecutions",
            &json!({ "pipelineName": pipeline }),
        );
        let found: Vec<Value> = answer["actionExecutionDetails"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|detail| detail["actionName"].as_str() == Some(action))
            .collect();
        assert_eq!(
            found.len(),
            1,
            "case {case:?}: {action} must have exactly one action execution, got {answer}"
        );
        found[0].clone()
    }

    /// Wait until every execution of `pipeline` has settled, and answer their statuses.
    fn settled_executions(&self, case: &str, pipeline: &str) -> Vec<String> {
        settle(case, "every execution settled", || {
            let answer = self.cp_ok(
                case,
                "ListPipelineExecutions",
                &json!({ "pipelineName": pipeline }),
            );
            let statuses: Vec<String> = answer["pipelineExecutionSummaries"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|summary| summary["status"].as_str().unwrap_or_default().to_string())
                .collect();
            if !statuses.is_empty()
                && statuses
                    .iter()
                    .all(|status| TERMINAL.contains(&status.as_str()))
            {
                Ok(statuses)
            } else {
                Err(format!("{statuses:?}"))
            }
        })
    }
}

/// Poll `read` until it answers `Ok`, or fail naming the case and the last state observed.
fn settle<T>(case: &str, what: &str, mut read: impl FnMut() -> Result<T, String>) -> T {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    loop {
        match read() {
            Ok(value) => return value,
            Err(observed) => {
                assert!(
                    Instant::now() < deadline,
                    "case {case:?}: {what} was not reached within {SETTLE_TIMEOUT:?}; last \
                     observed {observed}"
                );
                std::thread::sleep(POLL_INTERVAL);
            }
        }
    }
}

/// A pipeline whose Source stage reads one object and whose Build stage invokes `project`,
/// with `action_role` as the Build action's own `roleArn` when it declares one.
fn declaration(
    name: &str,
    role_arn: &str,
    bucket: &str,
    project: &str,
    action_role: Option<&str>,
) -> Value {
    let mut build = json!({
        "name": "Build",
        "actionTypeId": {
            "category": "Build",
            "owner": "AWS",
            "provider": "CodeBuild",
            "version": "1",
        },
        "configuration": { "ProjectName": project },
        "inputArtifacts": [{ "name": "Src" }],
        "runOrder": 1,
    });
    if let Some(action_role) = action_role {
        build["roleArn"] = Value::String(action_role.to_string());
    }
    json!({
        "name": name,
        "roleArn": role_arn,
        "artifactStore": { "type": "S3", "location": bucket },
        "stages": [
            {
                "name": "Source",
                "actions": [{
                    "name": "Src",
                    "actionTypeId": {
                        "category": "Source", "owner": "AWS", "provider": "S3", "version": "1",
                    },
                    "configuration": {
                        "S3Bucket": bucket,
                        "S3ObjectKey": "source.zip",
                        "PollForSourceChanges": "false",
                    },
                    "outputArtifacts": [{ "name": "Src" }],
                    "runOrder": 1,
                }],
            },
            { "name": "Build", "actions": [build] },
        ],
    })
}

/// RULE: the `StartBuild` a `Build` action is performed by is decided by the identity-based
/// policies in force on the role the action runs as, before any build exists — a role those
/// policies do not allow the action gets no build and an action `Failed` with `JobFailed`,
/// no external execution and nothing created in CodeBuild, and a role they allow gets a real
/// build that runs the project's buildspec to a terminal status of its own.
#[test]
fn a_build_action_starts_a_build_only_where_its_role_is_allowed_to() {
    on_a_sized_thread(|| {
        // (case, the pipeline role's policy, the action role's policy, authorized)
        let rows = [
            Row {
                case: "a pipeline role carrying no identity-based policy at all",
                pipeline_policy: None,
                action_policy: None,
                authorized: true,
            },
            Row {
                case: "a pipeline role whose policy names the artifact store only",
                pipeline_policy: Some(only_the_artifact_store),
                action_policy: None,
                authorized: false,
            },
            Row {
                case: "a grant of the action on the project's own ARN",
                pipeline_policy: Some(the_projects_own_arn),
                action_policy: None,
                authorized: true,
            },
            Row {
                case: "a grant of the action on every project of the account",
                pipeline_policy: Some(every_project_of_the_account),
                action_policy: None,
                authorized: true,
            },
            Row {
                case: "a grant of every action of the service on the project",
                pipeline_policy: Some(every_action_of_the_service),
                action_policy: None,
                authorized: true,
            },
            Row {
                case: "a grant of the action on another project's ARN",
                pipeline_policy: Some(another_projects_arn),
                action_policy: None,
                authorized: false,
            },
            Row {
                case: "a statement denying the action beside one allowing it",
                pipeline_policy: Some(a_deny_beside_a_grant),
                action_policy: None,
                authorized: false,
            },
            Row {
                case: "an action role that is allowed the action, beside a pipeline role \
                       that is not",
                pipeline_policy: Some(only_the_artifact_store),
                action_policy: Some(the_projects_own_arn),
                authorized: true,
            },
            Row {
                case: "an action role that is not allowed the action, beside a pipeline role \
                       that is",
                pipeline_policy: Some(every_project_of_the_account),
                action_policy: Some(only_the_artifact_store),
                authorized: false,
            },
        ];

        let emulator = Emulator::new();
        let bucket = "cp-grant-source";
        // The project's own service role, which is not what this suite varies: the question
        // here is what the role the ACTION runs as may do.
        let project_role = emulator.role("cp-grant-project-role", CODEBUILD_TRUST);
        emulator.staged_source("setup", bucket, "source.zip", &project_role);

        for (index, row) in rows.iter().enumerate() {
            let case = row.case;
            let project = format!("cp-grant-project-{index}");
            let pipeline = format!("cp-grant-pipeline-{index}");
            let pipeline_role_name = format!("cp-grant-pipeline-role-{index}");
            let action_role_name = format!("cp-grant-action-role-{index}");

            emulator.cb_ok(
                case,
                "CreateProject",
                &json!({
                    "name": project,
                    "serviceRole": project_role,
                    "source": { "type": "CODEPIPELINE", "buildspec": BUILDSPEC },
                    "artifacts": { "type": "CODEPIPELINE" },
                    "environment": {
                        "type": "LINUX_CONTAINER",
                        "image": "aws/codebuild/amazonlinux2-x86_64-standard:5.0",
                        "computeType": "BUILD_GENERAL1_SMALL",
                    },
                }),
            );

            let pipeline_role = emulator.role(&pipeline_role_name, PIPELINE_TRUST);
            if let Some(document) = row.pipeline_policy {
                emulator.role_policy(&pipeline_role_name, &document(&project));
            }
            let action_role = row.action_policy.map(|document| {
                let arn = emulator.role(&action_role_name, PIPELINE_TRUST);
                emulator.role_policy(&action_role_name, &document(&project));
                arn
            });
            // The role the action is PERFORMED AS: its own where it declares one, and the
            // pipeline's otherwise.
            let performed_as = action_role.clone().unwrap_or_else(|| pipeline_role.clone());

            emulator.cp_ok(
                case,
                "CreatePipeline",
                &json!({
                    "pipeline": declaration(
                        &pipeline,
                        &pipeline_role,
                        bucket,
                        &project,
                        action_role.as_deref(),
                    ),
                }),
            );

            // One execution runs when the pipeline is created, and it is the one judged
            // here: nothing else starts an execution of it.
            let statuses = emulator.settled_executions(case, &pipeline);
            assert_eq!(
                statuses.len(),
                1,
                "case {case:?}: exactly one execution runs for a created pipeline, got \
                 {statuses:?}"
            );
            let status = statuses[0].clone();
            let state = emulator.action_state(case, &pipeline, "Build", "Build");
            let latest = &state["latestExecution"];
            let record = emulator.record_of(case, &pipeline, "Build");
            let result = &record["output"]["executionResult"];
            let builds = emulator.builds_of(case, &project);

            if row.authorized {
                assert_eq!(
                    builds.len(),
                    1,
                    "case {case:?}: a role allowed the start must get one build of the \
                     project, got {builds:?}"
                );
                let external = result["externalExecutionId"].as_str().unwrap_or_default();
                assert_eq!(
                    external,
                    builds[0],
                    "case {case:?}: the action must report the build it started as its \
                     external execution, got {record}"
                );
                // The build really ran: its buildspec's one command answered success, so
                // that is the status it ended in — and no phase reports CodeBuild's own
                // fault status, which is reserved for a condition the caller did not cause.
                let build = emulator.build(case, external);
                assert_eq!(
                    build["buildStatus"].as_str(),
                    Some("SUCCEEDED"),
                    "case {case:?}: a buildspec whose every command succeeds must end \
                     SUCCEEDED, got {build}"
                );
                let faults: Vec<&Value> = build["phases"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|phase| phase["phaseStatus"].as_str() == Some("FAULT"))
                    .collect();
                assert!(
                    faults.is_empty(),
                    "case {case:?}: no phase of a build the caller configured correctly may \
                     report FAULT, got {faults:?}"
                );
                assert_eq!(
                    latest["status"].as_str(),
                    Some("Succeeded"),
                    "case {case:?}: a succeeded build settles the action Succeeded, got \
                     {latest}"
                );
                assert_eq!(
                    status, "Succeeded",
                    "case {case:?}: and the execution with it, got {status:?}"
                );
            } else {
                assert!(
                    builds.is_empty(),
                    "case {case:?}: no build may be created for a role that is not allowed \
                     to start one, got {builds:?}"
                );
                assert_eq!(
                    latest["status"].as_str(),
                    Some("Failed"),
                    "case {case:?}: the action must fail, got {latest}"
                );
                assert_eq!(
                    status, "Failed",
                    "case {case:?}: and the execution with it, got {status:?}"
                );
                assert_eq!(
                    latest["errorDetails"]["code"].as_str(),
                    Some(JOB_FAILED),
                    "case {case:?}: a call the role may not make is the run's own failure, \
                     got {latest}"
                );
                assert_eq!(
                    result["errorDetails"]["code"].as_str(),
                    Some(JOB_FAILED),
                    "case {case:?}: and the history reports the same code, got {record}"
                );
                assert_eq!(
                    result["externalExecutionId"].as_str(),
                    None,
                    "case {case:?}: an action that started nothing names no external \
                     execution, got {record}"
                );
                assert_eq!(
                    latest["externalExecutionId"].as_str(),
                    None,
                    "case {case:?}: nor does the state projection, got {latest}"
                );
                // The refusal says who was refused, what was asked for and on which
                // resource — the sentence IAM states for a call no identity-based policy
                // allows.
                let message = latest["errorDetails"]["message"]
                    .as_str()
                    .unwrap_or_default();
                for named in [
                    performed_as.as_str(),
                    START_BUILD_ACTION,
                    &project_arn(&project),
                    "is not authorized to perform",
                ] {
                    assert!(
                        message.contains(named),
                        "case {case:?}: the refusal must name {named:?}, got {message:?}"
                    );
                }
                assert!(
                    !message.contains("emulator"),
                    "case {case:?}: the detail a client reads must not announce the \
                     emulator, got {message:?}"
                );
            }
        }
    });
}

/// RULE, the reason clause: a statement that DENIES the action is an explicit deny whichever
/// else the role carries, and a policy set that merely does not mention the action is IAM's
/// default deny — so the two refusals name different rules of IAM's.
#[test]
fn the_refusal_names_the_rule_of_iam_that_withheld_the_grant() {
    on_a_sized_thread(|| {
        // (case, the pipeline role's policy, the reason clause the refusal carries)
        let rows: [(&str, Document, &str); 2] = [
            (
                "a policy set that does not mention the action",
                only_the_artifact_store,
                "because no identity-based policy allows the codebuild:StartBuild action",
            ),
            (
                "a statement that denies the action",
                a_deny_beside_a_grant,
                "with an explicit deny in an identity-based policy",
            ),
        ];

        let emulator = Emulator::new();
        let bucket = "cp-reason-source";
        let project_role = emulator.role("cp-reason-project-role", CODEBUILD_TRUST);
        emulator.staged_source("setup", bucket, "source.zip", &project_role);

        for (index, (case, document, expected)) in rows.iter().enumerate() {
            let project = format!("cp-reason-project-{index}");
            let pipeline = format!("cp-reason-pipeline-{index}");
            let role_name = format!("cp-reason-role-{index}");
            emulator.cb_ok(
                case,
                "CreateProject",
                &json!({
                    "name": project,
                    "serviceRole": project_role,
                    "source": { "type": "CODEPIPELINE", "buildspec": BUILDSPEC },
                    "artifacts": { "type": "CODEPIPELINE" },
                    "environment": {
                        "type": "LINUX_CONTAINER",
                        "image": "aws/codebuild/amazonlinux2-x86_64-standard:5.0",
                        "computeType": "BUILD_GENERAL1_SMALL",
                    },
                }),
            );
            let role_arn = emulator.role(&role_name, PIPELINE_TRUST);
            emulator.role_policy(&role_name, &document(&project));
            emulator.cp_ok(
                case,
                "CreatePipeline",
                &json!({
                    "pipeline": declaration(&pipeline, &role_arn, bucket, &project, None),
                }),
            );
            emulator.settled_executions(case, &pipeline);

            let latest = emulator.action_state(case, &pipeline, "Build", "Build")["latestExecution"]
                .clone();
            let message = latest["errorDetails"]["message"]
                .as_str()
                .unwrap_or_default();
            assert_eq!(
                message,
                format!(
                    "User: {role_arn} is not authorized to perform: {START_BUILD_ACTION} on \
                     resource: {} {expected}",
                    project_arn(&project)
                ),
                "case {case:?}: the refusal must name the rule that withheld the grant, got \
                 {message:?}"
            );
        }
    });
}

/// Run `body` on a thread with the stack the whole-router assembly needs.
fn on_a_sized_thread(body: impl FnOnce() + Send + 'static) {
    let handle = std::thread::Builder::new()
        .stack_size(BOOT_STACK_SIZE)
        .spawn(body)
        .expect("the sized thread starts");
    handle.join().expect("the suite's thread does not panic");
}
