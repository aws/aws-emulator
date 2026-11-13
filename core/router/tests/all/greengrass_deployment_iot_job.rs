// SPDX-License-Identifier: CC0-1.0

//! The IoT JOB a Greengrass V2 deployment is applied by, across the two services that
//! share it.
//!
//! `greengrassv2:CreateDeployment` answers an `iotJobId` and an `iotJobArn` — "the ID
//! of the IoT job that applies the deployment to target devices" — and a client that
//! drives deployments the way AWS does then tracks the rollout through the IoT jobs
//! plane: `iot:DescribeJob` on that id, and the per-device executions under it. The two
//! services are separate crates, so the job one of them MINTS is only resolvable in the
//! other because both reach the one shared jobs store.
//!
//! The rule these rows pin: an identifier an operation hands back addresses a resource
//! that exists in the plane that owns it. So the job a deployment reports is a real job
//! of the IoT jobs plane — `DescribeJob` answers 200 for it, with `job.jobArn` equal to
//! the `iotJobArn` the deployment reported and a non-terminal status while the
//! deployment is `ACTIVE` — a `jobId` nothing minted is that operation's modelled
//! `ResourceNotFoundException` (HTTP 404), and the job's lifecycle follows the
//! deployment's: cancelling the deployment cancels the job that applies it.
//!
//! The same rule runs the other way at the create: a deployment's TARGET is a record of
//! the IoT control plane, so each deployment below is preceded by the thing or thing
//! group it targets being created there.
//!
//! Rows span what the rule generalises over: both target forms a deployment may name (a
//! thing and a thing group), a deployment with and without an IoT job configuration,
//! and — for a job this plane creates itself — both `targetSelection` values, so a job
//! whose targets have all finished is reported finished exactly when the model says it
//! is finished by them.
//!
//! Only this configuration — Greengrass V2, the IoT control plane and the IoT Jobs data
//! plane mounted on ONE engine, which is what `core/router` ships — makes the join
//! observable at all.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";
/// The region every request here is signed for.
const REGION: &str = "us-east-1";

/// The three signing names this suite signs for, which is how the router picks the
/// service that serves each request.
const GREENGRASS: &str = "greengrass";
const IOT: &str = "iot";
const IOT_JOBS_DATA: &str = "iot-jobs-data";

/// Modelled `JobStatus` values (`IN_PROGRESS`, `CANCELED`, `COMPLETED`,
/// `DELETION_IN_PROGRESS`, `SCHEDULED`).
const JOB_IN_PROGRESS: &str = "IN_PROGRESS";
const JOB_CANCELED: &str = "CANCELED";
const JOB_COMPLETED: &str = "COMPLETED";

/// The `DeploymentStatus` a deployment that has just been created carries, and the one
/// a cancelled deployment reports.
const DEPLOYMENT_ACTIVE: &str = "ACTIVE";
const DEPLOYMENT_CANCELED: &str = "CANCELED";

type GreengrassV2 =
    service_com_amazonaws_greengrassv2_greengrassv2::EmulatedService<engine::InMemoryEngine>;
type Iot = service_com_amazonaws_iot_awsiotservice::EmulatedService<engine::InMemoryEngine>;
type IotJobsData =
    service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::EmulatedService<
        engine::InMemoryEngine,
    >;

/// One answer, reduced to what the rows assert on.
#[derive(Debug)]
struct Answer {
    status: u16,
    error_code: String,
    body: Value,
}

impl Answer {
    /// A string member, addressed by JSON pointer so a nested member is one call.
    fn text(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn list(&self, pointer: &str) -> Vec<String> {
        self.body
            .pointer(pointer)
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn number(&self, pointer: &str) -> Option<i64> {
        self.body.pointer(pointer).and_then(Value::as_i64)
    }
}

/// The three services, over ONE shared engine exactly as `router/src/lib.rs` mounts
/// them.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not request signing.
        router.set_request_authentication(false);
        router.register_service(GreengrassV2::new(Arc::clone(&engine)));
        router.register_service(Iot::new(Arc::clone(&engine)));
        router.register_service(IotJobsData::new(engine));
        Self { router }
    }

    /// Dispatch one request signed for `signing_name`.
    async fn call(&self, signing_name: &str, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/{signing_name}/aws4_request, \
             SignedHeaders=host, Signature=0000"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://localhost:9297{uri}"))
            .header("host", "localhost:9297")
            .header("authorization", authorization)
            .header("content-type", "application/json")
            .body(payload)
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let raw = response.into_body();
        let body: Value = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        let error_code = if error_code.is_empty() {
            body.get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        } else {
            error_code
        };
        Answer {
            status,
            error_code,
            body,
        }
    }

    async fn create_deployment(&self, request: Value) -> Answer {
        self.call(GREENGRASS, "POST", "/greengrass/v2/deployments", Some(request))
            .await
    }

    async fn get_deployment(&self, deployment_id: &str) -> Answer {
        self.call(
            GREENGRASS,
            "GET",
            &format!("/greengrass/v2/deployments/{deployment_id}"),
            None,
        )
        .await
    }

    async fn cancel_deployment(&self, deployment_id: &str) -> Answer {
        self.call(
            GREENGRASS,
            "POST",
            &format!("/greengrass/v2/deployments/{deployment_id}/cancel"),
            None,
        )
        .await
    }

    async fn describe_job(&self, job_id: &str) -> Answer {
        self.call(IOT, "GET", &format!("/jobs/{job_id}"), None).await
    }

    async fn create_job(&self, job_id: &str, request: Value) -> Answer {
        self.call(IOT, "PUT", &format!("/jobs/{job_id}"), Some(request))
            .await
    }

    /// `iot-jobs-data:UpdateJobExecution` — the device reporting what it made of the
    /// execution it was handed.
    async fn report_execution(&self, thing: &str, job_id: &str, status: &str) -> Answer {
        self.call(
            IOT_JOBS_DATA,
            "POST",
            &format!("/things/{thing}/jobs/{job_id}"),
            Some(json!({ "status": status })),
        )
        .await
    }

    /// Create the IoT thing or thing group `target_arn` names, in the plane that OWNS
    /// that record.
    ///
    /// The prerequisite every deployment has, here as on AWS: `CreateDeployment`
    /// resolves its target against the IoT control plane before it records anything, so
    /// a target that plane holds nothing under is refused. A target an earlier row
    /// already created answers `ResourceAlreadyExistsException`, which is the same
    /// prerequisite met.
    async fn ensure_target(&self, target_arn: &str) {
        let resource = target_arn
            .rsplit_once(':')
            .map(|(_, resource)| resource)
            .unwrap_or_default();
        let (space, name) = match resource.split_once('/') {
            Some(("thinggroup", name)) => ("/thing-groups", name),
            Some(("thing", name)) => ("/things", name),
            _ => panic!("setup: {target_arn:?} names no IoT thing or thing group"),
        };
        let created = self
            .call(IOT, "POST", &format!("{space}/{name}"), Some(json!({})))
            .await;
        assert!(
            created.status == 200 || created.error_code == "ResourceAlreadyExistsException",
            "setup: creating the target {target_arn:?} must succeed or report it already \
             exists, got {} {:?}",
            created.status,
            created.body
        );
    }
}

/// The ARN of a thing group, as a deployment names one.
fn thing_group_arn(name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT}:thinggroup/{name}")
}

/// The ARN of a thing, the other target form a deployment may name.
fn thing_arn(name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT}:thing/{name}")
}

/// One row: a deployment to create, and the in-progress timeout its IoT job
/// configuration carries (`None` for a request that sends no configuration at all).
struct DeploymentCase {
    case: &'static str,
    target_arn: String,
    deployment_name: &'static str,
    in_progress_timeout_minutes: Option<i64>,
}

/// RULE: the `iotJobId` / `iotJobArn` a deployment reports address a job that EXISTS in
/// the IoT jobs plane, which answers 200 for it while the deployment is ACTIVE; a
/// `jobId` nothing minted is that operation's modelled 404.
#[tokio::test]
async fn a_deployments_iot_job_is_resolvable_in_the_iot_jobs_plane() {
    let emulator = Emulator::new();

    let cases = vec![
        DeploymentCase {
            case: "a thing-group target, no job configuration",
            target_arn: thing_group_arn("MyGreengrassCoreGroup"),
            deployment_name: "group-deployment",
            in_progress_timeout_minutes: None,
        },
        DeploymentCase {
            case: "a thing target with an in-progress timeout",
            target_arn: thing_arn("MyGreengrassCore"),
            deployment_name: "device-deployment",
            in_progress_timeout_minutes: Some(10),
        },
        DeploymentCase {
            case: "a second revision for one target",
            target_arn: thing_group_arn("MyGreengrassCoreGroup"),
            deployment_name: "group-deployment-revised",
            in_progress_timeout_minutes: Some(1),
        },
    ];

    for case in &cases {
        // The deployment's target has to exist in the plane that owns it before the
        // deployment can be created at all.
        emulator.ensure_target(&case.target_arn).await;
        let mut request = json!({
            "targetArn": case.target_arn,
            "deploymentName": case.deployment_name,
            "components": { "com.example.App": { "componentVersion": "1.0.0" } },
        });
        if let Some(minutes) = case.in_progress_timeout_minutes {
            request["iotJobConfiguration"] =
                json!({ "timeoutConfig": { "inProgressTimeoutInMinutes": minutes } });
        }

        let created = emulator.create_deployment(request).await;
        assert_eq!(
            created.status, 201,
            "case {:?}: CreateDeployment must answer 201, got {} {:?}",
            case.case, created.status, created.body
        );
        let deployment_id = created.text("/deploymentId");
        let iot_job_id = created.text("/iotJobId");
        let iot_job_arn = created.text("/iotJobArn");
        assert!(
            !deployment_id.is_empty() && !iot_job_id.is_empty() && !iot_job_arn.is_empty(),
            "case {:?}: the create reports a deployment id and an IoT job identity, got {:?}",
            case.case,
            created.body
        );
        assert_eq!(
            iot_job_arn,
            format!("arn:aws:iot:{REGION}:{ACCOUNT}:job/{iot_job_id}"),
            "case {:?}: expected the modelled IoTJobARN form, got {iot_job_arn:?}",
            case.case
        );

        // The deployment itself is ACTIVE and reports the same job identity.
        let deployment = emulator.get_deployment(&deployment_id).await;
        assert_eq!(
            (
                deployment.status,
                deployment.text("/deploymentStatus"),
                deployment.text("/targetArn"),
                deployment.text("/iotJobId"),
            ),
            (
                200,
                DEPLOYMENT_ACTIVE.to_string(),
                case.target_arn.clone(),
                iot_job_id.clone(),
            ),
            "case {:?}: expected an ACTIVE deployment of the requested target, got {:?}",
            case.case,
            deployment.body
        );

        // …and the job that applies it is a job of the IoT jobs plane.
        let job = emulator.describe_job(&iot_job_id).await;
        assert_eq!(
            job.status, 200,
            "case {:?}: DescribeJob on the deployment's job must answer 200, got {} {:?}",
            case.case, job.status, job.body
        );
        assert_eq!(
            (job.text("/job/jobId"), job.text("/job/jobArn")),
            (iot_job_id.clone(), iot_job_arn.clone()),
            "case {:?}: expected the job identity the deployment reported, got {:?}",
            case.case,
            job.body
        );
        assert_eq!(
            job.text("/job/status"),
            JOB_IN_PROGRESS,
            "case {:?}: a job applying an ACTIVE deployment is non-terminal, got {:?}",
            case.case,
            job.body
        );
        assert_eq!(
            job.list("/job/targets"),
            vec![case.target_arn.clone()],
            "case {:?}: expected the job to target what the deployment targets, got {:?}",
            case.case,
            job.body
        );
        assert_eq!(
            job.text("/job/targetSelection"),
            "CONTINUOUS",
            "case {:?}: a Greengrass deployment is continuous, so its job is too, got {:?}",
            case.case,
            job.body
        );
        assert_eq!(
            job.number("/job/timeoutConfig/inProgressTimeoutInMinutes"),
            case.in_progress_timeout_minutes,
            "case {:?}: expected the configured in-progress timeout, got {:?}",
            case.case,
            job.body
        );
    }

    // A job id nothing minted: the modelled 404, not a 501 and not a success.
    for job_id in ["00000000-0000-4000-8000-000000000000", "no-such-job"] {
        let missing = emulator.describe_job(job_id).await;
        assert_eq!(
            missing.status, 404,
            "case {job_id:?}: expected 404, got {} {:?}",
            missing.status, missing.body
        );
        assert_eq!(
            missing.error_code, "ResourceNotFoundException",
            "case {job_id:?}: expected ResourceNotFoundException, got {:?}",
            missing.error_code
        );
    }
}

/// RULE: the job's lifecycle follows the deployment's — "cancels the deployment for
/// devices that haven't yet received it" is what cancelling its IoT job does, so the
/// job reads back terminal in the plane that owns it.
#[tokio::test]
async fn cancelling_a_deployment_cancels_the_job_that_applies_it() {
    let emulator = Emulator::new();

    let target_arn = thing_group_arn("CancelledGroup");
    emulator.ensure_target(&target_arn).await;
    let created = emulator
        .create_deployment(json!({
            "targetArn": target_arn,
            "deploymentName": "to-be-cancelled",
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "CreateDeployment must answer 201, got {} {:?}",
        created.status, created.body
    );
    let deployment_id = created.text("/deploymentId");
    let iot_job_id = created.text("/iotJobId");

    let before = emulator.describe_job(&iot_job_id).await;
    assert_eq!(
        before.text("/job/status"),
        JOB_IN_PROGRESS,
        "case \"before the cancel\": expected {JOB_IN_PROGRESS}, got {:?}",
        before.body
    );

    let cancelled = emulator.cancel_deployment(&deployment_id).await;
    assert_eq!(
        cancelled.status, 200,
        "CancelDeployment must answer 200, got {} {:?}",
        cancelled.status, cancelled.body
    );

    let deployment = emulator.get_deployment(&deployment_id).await;
    assert_eq!(
        deployment.text("/deploymentStatus"),
        DEPLOYMENT_CANCELED,
        "case \"the cancelled deployment\": expected {DEPLOYMENT_CANCELED}, got {:?}",
        deployment.body
    );
    let after = emulator.describe_job(&iot_job_id).await;
    assert_eq!(
        (after.status, after.text("/job/status")),
        (200, JOB_CANCELED.to_string()),
        "case \"the cancelled deployment's job\": expected 200 {JOB_CANCELED}, got {} {:?}",
        after.status,
        after.body
    );
}

/// RULE: a job's progress is counted from its executions, so a SNAPSHOT job — one that
/// "will be complete after all those things specified as targets have completed the
/// job" — reads back COMPLETED once its targets have, while a CONTINUOUS one, which
/// "will continue to run", does not.
#[tokio::test]
async fn a_jobs_progress_is_counted_from_the_executions_its_devices_report() {
    let emulator = Emulator::new();

    // (case, job id, targetSelection the create sends, the status after the device
    // reports SUCCEEDED)
    let rows: [(&str, &str, Option<&str>, &str); 3] = [
        (
            "a snapshot job every target finished",
            "snapshot-job",
            Some("SNAPSHOT"),
            JOB_COMPLETED,
        ),
        (
            "a continuous job keeps running",
            "continuous-job",
            Some("CONTINUOUS"),
            JOB_IN_PROGRESS,
        ),
        (
            "a job whose create named no selection",
            "unselected-job",
            None,
            JOB_IN_PROGRESS,
        ),
    ];

    for (case, job_id, target_selection, expected_after) in rows {
        let thing = format!("device-for-{job_id}");
        let mut request = json!({
            "targets": [thing_arn(&thing)],
            "document": r#"{"operation":"reboot"}"#,
        });
        if let Some(selection) = target_selection {
            request["targetSelection"] = json!(selection);
        }
        let created = emulator.create_job(job_id, request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateJob must answer 200, got {} {:?}",
            created.status, created.body
        );

        let queued = emulator.describe_job(job_id).await;
        assert_eq!(
            (
                queued.text("/job/status"),
                queued.number("/job/jobProcessDetails/numberOfQueuedThings"),
            ),
            (JOB_IN_PROGRESS.to_string(), Some(1)),
            "case {case:?}: a fresh job is rolling out to its one target, got {:?}",
            queued.body
        );

        // The DEVICE reports the execution finished, through the Jobs data plane.
        let reported = emulator.report_execution(&thing, job_id, "SUCCEEDED").await;
        assert_eq!(
            reported.status, 200,
            "case {case:?}: UpdateJobExecution must answer 200, got {} {:?}",
            reported.status, reported.body
        );

        let after = emulator.describe_job(job_id).await;
        assert_eq!(
            after.text("/job/status"),
            expected_after,
            "case {case:?}: expected {expected_after}, got {:?}",
            after.body
        );
        assert_eq!(
            (
                after.number("/job/jobProcessDetails/numberOfSucceededThings"),
                after.number("/job/jobProcessDetails/numberOfQueuedThings"),
                after.list("/job/jobProcessDetails/processingTargets"),
            ),
            (Some(1), None, Vec::new()),
            "case {case:?}: the counts follow the executions, got {:?}",
            after.body
        );
        assert_eq!(
            after.body.pointer("/job/completedAt").is_some(),
            expected_after == JOB_COMPLETED,
            "case {case:?}: completedAt is reported exactly for a completed job, got {:?}",
            after.body
        );
    }
}
