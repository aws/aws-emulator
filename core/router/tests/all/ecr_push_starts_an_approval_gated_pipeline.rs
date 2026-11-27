// SPDX-License-Identifier: CC0-1.0

//! An image push to Amazon ECR starts the CodePipeline pipeline an EventBridge rule
//! targets even while that pipeline is already holding work, and the target ROLE's grant
//! is what decides whether it does.
//!
//! The rule under test: **a push that an ENABLED rule's `EventPattern` matches is
//! delivered to that rule's CodePipeline target by starting ONE execution of that
//! pipeline under the role the target names, whatever the pipeline's own stages are
//! holding at the time — an earlier execution parked on a manual approval included — so
//! the execution appears in `ListPipelineExecutions` outside the baseline; and the
//! delivery is made only where the documents in force on that role allow
//! `codepipeline:StartPipelineExecution` on that pipeline, so a role carrying another
//! action, or the same action on another pipeline, starts nothing.**
//!
//! Five crates have to agree for that to hold (Amazon ECR publishes the push, EventBridge
//! matches and delivers it, IAM holds the target role, CodePipeline runs the execution and
//! Amazon S3 holds its artifacts), so this suite lives here.
//!
//! Every push is made the way a registry client makes one — each blob the manifest
//! references goes up through `InitiateLayerUpload` -> `UploadLayerPart` ->
//! `CompleteLayerUpload` before `PutImage` stores the manifest — so the event under test is
//! the one a real push emits rather than one a manifest-only shortcut emits.
//!
//! The rows differ along what the rule generalises over: what the target role's documents
//! say about the invocation — the action on exactly that pipeline, the action on every
//! resource, an unrelated action, and the action on a DIFFERENT pipeline. Every "nothing
//! was started" row is paired with a permissioned sibling rule whose delivery IS observed
//! on the same wiring, so an absence is attributable to the role under test rather than to
//! a delivery path that carries nothing at all.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread. Nothing here sleeps for an execution: it POLLS the
//! reporting operations, exactly as a client does.

use std::collections::BTreeSet;
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
const ACCOUNT: &str = "000000000000";

/// CodePipeline's `X-Amz-Target` prefix, from that service's own model.
const CODEPIPELINE_TARGET_PREFIX: &str = "CodePipeline_20150709";

/// Amazon ECR's `X-Amz-Target` prefix, from that service's own model.
const ECR_TARGET_PREFIX: &str = "AmazonEC2ContainerRegistry_V20150921";

/// EventBridge's `X-Amz-Target` prefix, from that service's own model.
const EVENTS_TARGET_PREFIX: &str = "AWSEvents";

/// How long a wait for one execution to appear may take before the suite calls it stalled.
/// A generous ceiling, not an expectation.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(30);

/// How often a wait re-reads the reporting operation it polls.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How long a "nothing was started" assertion watches the pipeline. Longer than several of
/// the execution engine's per-step spans and of the delivery path, so an execution that was
/// going to be started would have been.
const NOTHING_STARTED_WINDOW: Duration = Duration::from_millis(1_500);

/// The trust policy a pipeline role carries.
const PIPELINE_TRUST: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
"Principal":{"Service":"codepipeline.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The trust policy the role an EventBridge target names carries: EventBridge invokes the
/// target AS that role, so the role has to let that service assume it.
const EVENTS_TRUST: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
"Principal":{"Service":"events.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The action a delivery to a CodePipeline target performs, which is what the target's role
/// has to be allowed to do.
const START_EXECUTION_ACTION: &str = "codepipeline:StartPipelineExecution";

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
            // A MULTI-thread runtime: a push delivers its event through in-process calls
            // into other services of this same router, and an action's work runs there
            // too.
            runtime: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("a multi-thread runtime"),
        }
    }

    /// Drive one request and answer its status and body.
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

    /// One Amazon ECR operation that has to succeed.
    fn ecr_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, answer) = self.json_rpc("ecr", ECR_TARGET_PREFIX, operation, body);
        assert_eq!(
            status, 200,
            "case {case:?}: ecr {operation} must answer 200, got {status} with {answer}"
        );
        answer
    }

    /// One EventBridge operation that has to succeed.
    fn events_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, answer) = self.json_rpc("events", EVENTS_TARGET_PREFIX, operation, body);
        assert_eq!(
            status, 200,
            "case {case:?}: events {operation} must answer 200, got {status} with {answer}"
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
            "CreateRole {name}: expected 200, got {status} with body {body}"
        );
        element(&body, "Arn")
    }

    /// Attach an inline policy to `role` that allows `action` on `resource`.
    fn allow(&self, role: &str, action: &str, resource: &str) {
        let document = json!({
            "Version": "2012-10-17",
            "Statement": [{"Effect": "Allow", "Action": action, "Resource": resource}],
        })
        .to_string();
        let (status, body) = self.iam(
            "PutRolePolicy",
            &[
                ("RoleName", role),
                ("PolicyName", "granted"),
                ("PolicyDocument", &document),
            ],
        );
        assert_eq!(
            status, 200,
            "PutRolePolicy {role}: expected 200, got {status} with body {body}"
        );
    }

    /// Create a bucket for a pipeline's artifact store.
    fn bucket(&self, name: &str) {
        let (status, payload) = self.send("s3", "PUT", &format!("/{name}"), &[], Vec::new());
        assert_eq!(
            status,
            200,
            "CreateBucket {name}: expected 200, got {status} with {:?}",
            String::from_utf8_lossy(&payload)
        );
    }

    /// Create an Amazon ECR repository.
    fn repository(&self, case: &str, name: &str) {
        self.ecr_ok(case, "CreateRepository", &json!({"repositoryName": name}));
    }

    /// Upload `content` into `repository` as one blob THE WAY A REGISTRY CLIENT DOES —
    /// `InitiateLayerUpload` -> `UploadLayerPart` -> `CompleteLayerUpload` — and answer the
    /// digest the registry registered it under.
    fn upload_blob(&self, case: &str, repository: &str, content: &[u8]) -> String {
        let digest = content_digest(content);
        let opened = self.ecr_ok(
            case,
            "InitiateLayerUpload",
            &json!({"repositoryName": repository}),
        );
        let upload_id = opened["uploadId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: InitiateLayerUpload must hand back an uploadId, got {opened}")
            })
            .to_string();
        let last = i64::try_from(content.len()).expect("a test blob fits in an i64") - 1;
        self.ecr_ok(
            case,
            "UploadLayerPart",
            &json!({
                "repositoryName": repository,
                "uploadId": upload_id,
                "partFirstByte": 0,
                "partLastByte": last,
                "layerPartBlob": aws_smithy_types::base64::encode(content),
            }),
        );
        let completed = self.ecr_ok(
            case,
            "CompleteLayerUpload",
            &json!({
                "repositoryName": repository,
                "uploadId": upload_id,
                "layerDigests": [&digest],
            }),
        );
        assert_eq!(
            completed["layerDigest"].as_str(),
            Some(digest.as_str()),
            "case {case:?}: the completed upload must register the blob under the digest of its \
             own bytes ({digest}), got {completed}"
        );
        digest
    }

    /// Push one image into `repository` under `tag` the way a registry client does: both
    /// blobs the manifest references go up through the layer-upload sequence, and the
    /// manifest is then stored by `PutImage`. Answers the digest the registry stored the
    /// manifest under.
    fn push(&self, case: &str, repository: &str, marker: &str, tag: &str) -> String {
        let layer = format!("layer bytes of {marker}").into_bytes();
        let config = format!("{{\"marker\":\"{marker}\"}}").into_bytes();
        let layer_digest = self.upload_blob(case, repository, &layer);
        let config_digest = self.upload_blob(case, repository, &config);
        let manifest = json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.docker.distribution.manifest.v2+json",
            "config": {
                "mediaType": "application/vnd.docker.container.image.v1+json",
                "size": config.len(),
                "digest": config_digest,
            },
            "layers": [{
                "mediaType": "application/vnd.docker.image.rootfs.diff.tar.gzip",
                "size": layer.len(),
                "digest": layer_digest,
            }],
        })
        .to_string();
        let pushed = self.ecr_ok(
            case,
            "PutImage",
            &json!({
                "repositoryName": repository,
                "imageManifest": manifest,
                "imageTag": tag,
            }),
        );
        pushed["image"]["imageId"]["imageDigest"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the push must report a digest, got {pushed}"))
            .to_string()
    }

    /// The ids of every execution of `pipeline`, as `ListPipelineExecutions` reports them.
    fn execution_ids(&self, case: &str, pipeline: &str) -> BTreeSet<String> {
        let answer = self.cp_ok(
            case,
            "ListPipelineExecutions",
            &json!({ "pipelineName": pipeline }),
        );
        answer["pipelineExecutionSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|summary| summary["pipelineExecutionId"].as_str().map(str::to_string))
            .collect()
    }

    /// Wait until an execution of `pipeline` appears outside `baseline`, and answer its id.
    fn started_execution(&self, case: &str, pipeline: &str, baseline: &BTreeSet<String>) -> String {
        settle(case, "a new pipeline execution", || {
            let ids = self.execution_ids(case, pipeline);
            match ids.difference(baseline).next() {
                Some(id) => Ok(id.clone()),
                None => Err(format!("only the baseline executions {ids:?}")),
            }
        })
    }

    /// One execution's summary, as `ListPipelineExecutions` reports it — which is where
    /// `sourceRevisions` lives.
    fn execution_summary(&self, case: &str, pipeline: &str, id: &str) -> Value {
        let answer = self.cp_ok(
            case,
            "ListPipelineExecutions",
            &json!({ "pipelineName": pipeline }),
        );
        answer["pipelineExecutionSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|summary| summary["pipelineExecutionId"].as_str() == Some(id))
            .unwrap_or_else(|| panic!("case {case:?}: no summary for the execution {id}"))
    }

    /// Create a pipeline whose Source action sources `repository` at `tag` and whose second
    /// stage is a MANUAL APPROVAL — the stage a client puts in front of a release, and the
    /// one that leaves an execution parked `InProgress` until somebody answers it.
    fn approval_gated_pipeline(
        &self,
        case: &str,
        name: &str,
        role: &str,
        bucket: &str,
        repository: &str,
        tag: &str,
    ) {
        let declaration = json!({
            "name": name,
            "roleArn": role,
            "artifactStore": { "type": "S3", "location": bucket },
            "stages": [
                {
                    "name": "Source",
                    "actions": [{
                        "name": "Image",
                        "actionTypeId": {
                            "category": "Source",
                            "owner": "AWS",
                            "provider": "ECR",
                            "version": "1",
                        },
                        "configuration": { "RepositoryName": repository, "ImageTag": tag },
                        "outputArtifacts": [{ "name": "ImageArt" }],
                        "runOrder": 1,
                    }],
                },
                {
                    "name": "Approve",
                    "actions": [{
                        "name": "Release",
                        "actionTypeId": {
                            "category": "Approval",
                            "owner": "AWS",
                            "provider": "Manual",
                            "version": "1",
                        },
                        "runOrder": 1,
                    }],
                },
            ],
        });
        self.cp_ok(case, "CreatePipeline", &json!({ "pipeline": declaration }));
    }

    /// A rule matching a PUSH to `repository` on the default bus, with `pipeline_arn` as
    /// its target under `role_arn` — the pattern a client writes for an image-driven
    /// trigger.
    fn pipeline_rule(
        &self,
        case: &str,
        rule: &str,
        repository: &str,
        pipeline_arn: &str,
        role_arn: &str,
    ) {
        let pattern = json!({
            "source": ["aws.ecr"],
            "detail-type": ["ECR Image Action"],
            "detail": {
                "action-type": ["PUSH"],
                "repository-name": [repository],
                "result": ["SUCCESS"],
            },
        })
        .to_string();
        self.events_ok(
            case,
            "PutRule",
            &json!({ "Name": rule, "EventPattern": pattern, "State": "ENABLED" }),
        );
        let targeted = self.events_ok(
            case,
            "PutTargets",
            &json!({
                "Rule": rule,
                "Targets": [{ "Id": "1", "Arn": pipeline_arn, "RoleArn": role_arn }],
            }),
        );
        assert_eq!(
            targeted["FailedEntryCount"].as_i64(),
            Some(0),
            "case {case:?}: the pipeline target must be accepted, got {targeted}"
        );
    }
}

/// Watch `pipeline` for the whole absence window and fail the moment an execution appears
/// beyond `baseline`.
fn nothing_started(
    emulator: &Emulator,
    case: &str,
    pipeline: &str,
    baseline: &BTreeSet<String>,
    what: &str,
) {
    let deadline = Instant::now() + NOTHING_STARTED_WINDOW;
    loop {
        let ids = emulator.execution_ids(case, pipeline);
        assert_eq!(
            &ids, baseline,
            "case {case:?}: {what} must start no execution of {pipeline}, got {ids:?} against \
             the baseline {baseline:?}"
        );
        if Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(POLL_INTERVAL);
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

/// The ARN a pipeline of `name` carries, which is what a rule's target names it by.
fn pipeline_arn(name: &str) -> String {
    format!("arn:aws:codepipeline:{REGION}:{ACCOUNT}:{name}")
}

/// The content digest of `content`: the `sha256:<hex>` form a registry addresses a blob by,
/// computed here independently of the services under test.
fn content_digest(content: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{}", hex::encode(Sha256::digest(content)))
}

/// The source revision the execution recorded for its Amazon ECR source action, as the
/// execution's SUMMARY reports it (`PipelineExecutionSummary$sourceRevisions` is "a list of
/// the source artifact revisions that initiated a pipeline execution").
fn source_revision(summary: &Value) -> Option<String> {
    summary["sourceRevisions"]
        .as_array()?
        .iter()
        .find_map(|revision| revision["revisionId"].as_str())
        .map(str::to_string)
}

/// A push of an image an ENABLED rule's pattern matches starts an execution of the
/// pipeline the rule targets even while that pipeline is parked on a manual approval, and
/// the execution that starts sources the image the push stored.
#[test]
fn a_matched_push_starts_the_pipeline_parked_on_a_manual_approval() {
    on_a_sized_thread(|| {
        let emulator = Emulator::new();
        let case = "a push onto an approval-gated pipeline";
        let pipeline = "approval-gated";
        let arn = pipeline_arn(pipeline);

        emulator.bucket("approval-artifacts");
        emulator.repository(case, "good-repo");
        let pipeline_role = emulator.role("approval-pipeline-role", PIPELINE_TRUST);
        emulator.approval_gated_pipeline(
            case,
            pipeline,
            &pipeline_role,
            "approval-artifacts",
            "good-repo",
            "latest",
        );

        let events_role = emulator.role("eb-role-good", EVENTS_TRUST);
        emulator.allow("eb-role-good", START_EXECUTION_ACTION, &arn);
        emulator.pipeline_rule(case, "on-good-push", "good-repo", &arn, &events_role);

        // The pipeline's FIRST execution is parked: nothing has answered the approval the
        // second stage waits on, so the pipeline is holding work when the push arrives.
        let baseline = emulator.execution_ids(case, pipeline);
        let pushed = emulator.push(case, "good-repo", "first", "latest");
        let started = emulator.started_execution(case, pipeline, &baseline);
        assert!(
            !baseline.contains(&started),
            "case {case:?}: the delivery must start an execution outside the baseline \
             {baseline:?}, got {started}"
        );
        // The execution the delivery started runs the image the push stored: its Amazon
        // ECR source action's revision is that image's digest.
        let revision = settle(case, "the execution's source revision", || {
            let summary = emulator.execution_summary(case, pipeline, &started);
            source_revision(&summary).ok_or_else(|| format!("the summary {summary}"))
        });
        assert_eq!(
            revision, pushed,
            "case {case:?}: the started execution must source the pushed image {pushed}, got \
             {revision}"
        );

        // A SECOND push of the same repository is a second event, and so a second
        // execution: the delivery is not a one-off that the first push consumed.
        let after_first = emulator.execution_ids(case, pipeline);
        emulator.push(case, "good-repo", "second", "latest");
        let again = emulator.started_execution(case, pipeline, &after_first);
        assert!(
            !after_first.contains(&again),
            "case {case:?}: a second matched push must start a further execution outside \
             {after_first:?}, got {again}"
        );
    });
}

/// One row of the grant table: what the role an EventBridge target names is allowed, and
/// whether a push the rule matches therefore starts an execution of the target pipeline.
#[derive(Debug)]
struct Grant {
    /// The row's name, which every assertion of it names.
    case: &'static str,
    /// The repository this row pushes to, and the one its rule filters on.
    repository: &'static str,
    /// The role this row's target names.
    role: &'static str,
    /// The rule this row's target hangs off.
    rule: &'static str,
    /// The action the role's inline document allows.
    action: &'static str,
    /// What that document allows the action on: `None` is every resource (`*`), `Some(name)`
    /// the ARN of the pipeline of that name.
    resource: Option<&'static str>,
    /// Whether a matched push under this role starts an execution of the pipeline.
    starts: bool,
}

/// The ROLE a rule's CodePipeline target names is what decides the delivery: a push the
/// rule matches starts an execution only where the documents in force on that role allow
/// `codepipeline:StartPipelineExecution` on that pipeline.
///
/// The rows differ along what those documents say — the action on exactly that pipeline,
/// the action on every resource, an unrelated action, and the action on another pipeline —
/// and every row that starts nothing is followed by a push onto a permissioned sibling
/// rule whose delivery IS observed, so an absence is attributable to the role under test.
#[test]
fn the_target_roles_grant_decides_whether_a_matched_push_starts_the_pipeline() {
    on_a_sized_thread(|| {
        let emulator = Emulator::new();
        let setup = "the grant table";
        let pipeline = "grant-gated";
        let arn = pipeline_arn(pipeline);

        // The rows this table walks. The unrelated action is the one the recorded
        // configuration withholds the start permission with; the resources are the target
        // pipeline's own ARN, every resource, and another pipeline's ARN.
        let rows = [
            Grant {
                case: "the start action on the target pipeline",
                repository: "repo-exact",
                role: "eb-role-exact",
                rule: "on-exact",
                action: START_EXECUTION_ACTION,
                resource: Some(pipeline),
                starts: true,
            },
            Grant {
                case: "the start action on every resource",
                repository: "repo-wildcard",
                role: "eb-role-wildcard",
                rule: "on-wildcard",
                action: START_EXECUTION_ACTION,
                resource: None,
                starts: true,
            },
            Grant {
                case: "an unrelated action only",
                repository: "repo-unrelated",
                role: "eb-role-unrelated",
                rule: "on-unrelated",
                action: "sqs:GetQueueAttributes",
                resource: None,
                starts: false,
            },
            Grant {
                case: "the start action on another pipeline",
                repository: "repo-elsewhere",
                role: "eb-role-elsewhere",
                rule: "on-elsewhere",
                action: START_EXECUTION_ACTION,
                resource: Some("another-pipeline"),
                starts: false,
            },
        ];

        emulator.bucket("grant-artifacts");
        emulator.repository(setup, "source-repo");
        let pipeline_role = emulator.role("grant-pipeline-role", PIPELINE_TRUST);
        emulator.approval_gated_pipeline(
            setup,
            pipeline,
            &pipeline_role,
            "grant-artifacts",
            "source-repo",
            "latest",
        );

        // The control wiring: a rule on its own repository under a role that IS allowed the
        // invocation, which every "nothing started" row then proves the path alive with.
        let control_role = emulator.role("eb-role-control", EVENTS_TRUST);
        emulator.allow("eb-role-control", START_EXECUTION_ACTION, &arn);
        emulator.repository(setup, "repo-control");
        emulator.pipeline_rule(setup, "on-control", "repo-control", &arn, &control_role);

        for row in &rows {
            let case = row.case;
            let role_arn = emulator.role(row.role, EVENTS_TRUST);
            let resource = row.resource.map_or_else(|| "*".to_string(), pipeline_arn);
            emulator.allow(row.role, row.action, &resource);
            emulator.repository(case, row.repository);
            emulator.pipeline_rule(case, row.rule, row.repository, &arn, &role_arn);

            let baseline = emulator.execution_ids(case, pipeline);
            emulator.push(case, row.repository, row.repository, "latest");
            if row.starts {
                let started = emulator.started_execution(case, pipeline, &baseline);
                assert!(
                    !baseline.contains(&started),
                    "case {case:?}: a role allowing {} on {resource} must start an execution \
                     outside {baseline:?}, got {started}",
                    row.action
                );
            } else {
                nothing_started(
                    &emulator,
                    case,
                    pipeline,
                    &baseline,
                    &format!("a role allowing only {} on {resource}", row.action),
                );
                // The same push onto the control rule, whose role IS allowed the
                // invocation, does start one: the absence above is this role's and not a
                // dead delivery path.
                let before_control = emulator.execution_ids(case, pipeline);
                emulator.push(case, "repo-control", row.repository, "latest");
                let started = emulator.started_execution(case, pipeline, &before_control);
                assert!(
                    !before_control.contains(&started),
                    "case {case:?}: the permissioned control rule must still start an \
                     execution outside {before_control:?}, got {started}"
                );
            }
        }
    });
}

/// Run `body` on a thread with the stack the whole-router assembly needs.
fn on_a_sized_thread(body: impl FnOnce() + Send + 'static) {
    let handle = std::thread::Builder::new()
        .stack_size(BOOT_STACK_SIZE)
        .spawn(body)
        .expect("a sized thread");
    if let Err(panic) = handle.join() {
        std::panic::resume_unwind(panic);
    }
}
