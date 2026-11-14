// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `CreateOTAUpdate` and `GetOTAUpdate`: an OTA update is
//! a RECORD plus the AWS IoT job it deploys through, and targeting a device with it is
//! what creates that device's job execution.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 `POST /otaUpdates/{otaUpdateId}` and `GET /otaUpdates/{otaUpdateId}`
//! signed for the `iot` signing name — then reads the update back through
//! `GetOTAUpdate` and the shared job-execution store the IoT Jobs data plane serves
//! (`service_common::iot_job_executions`), which is where the executions a job creates
//! live.
//!
//! The rules under test:
//!
//! * a `CreateOTAUpdate` registers the update under the `otaUpdateId` the request
//!   names, mints its `otaUpdateArn` scoped to the signed region, deploys it through
//!   the job the OTA Update Manager derives from that id, answers the non-terminal
//!   creation status, and hands every member back unchanged from `GetOTAUpdate`;
//! * every DISTINCT thing the update targets holds exactly one `QUEUED` execution of
//!   that job afterwards;
//! * a request it refuses (a target that names no device, a target ARN of another
//!   resource type, a thing group, a file naming no location, an id already in use, or
//!   a fan-out past a ceiling) writes nothing at all;
//! * the creation the clock owes completes on its own, at the instant it was due;
//! * a `DeleteOTAUpdate` removes the update, the job it deployed and that job's
//!   executions, and refuses while the job is still running unless the request forces
//!   it.
//!
//! Each table spans the class the rule generalises over — how many targets and files
//! one update carries and in what form, which optional members the request carries,
//! and the region it is signed for — rather than one recorded request.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::{
    missing_ota_update_message, CREATED_OTA_UPDATE_STATUS, MAX_JOB_TARGETS, MAX_OTA_UPDATE_FILES,
    MAX_OTA_UPDATE_ID_LEN, NEW_OTA_UPDATE_STATUS, OTA_UPDATE_CREATION_SPAN,
    OTA_UPDATE_JOB_ID_PREFIX,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use service_common::iot_job_executions::{JobExecutionStore, MAX_THING_NAME_LEN, STATUS_QUEUED};
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for unless a case names another.
const REGION: &str = "us-east-1";
/// An IAM role ARN of the shape `CreateOTAUpdate$roleArn` is modelled with (len
/// 20..2048); the operation neither assumes nor resolves it.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/ota-update-role";
/// The terminal `JobExecutionStatus` a device reports when it finished the job it was
/// handed (`smithy_query(query="shape",shape="JobExecutionStatus")`: QUEUED |
/// IN_PROGRESS | SUCCEEDED | FAILED | TIMED_OUT | REJECTED | REMOVED | CANCELED).
const STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name
/// and `region`: the scope is how the router picks the claiming service, and how the
/// service picks the region its ARNs are scoped to.
fn signed_for_iot(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{region}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    fn text(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    fn message(&self) -> String {
        self.text("message").unwrap_or_default()
    }

    /// The `otaUpdateInfo` a `GetOTAUpdate` reply carries.
    fn info(&self) -> &Value {
        self.body
            .get("otaUpdateInfo")
            .unwrap_or_else(|| panic!("a GetOTAUpdate reply carries otaUpdateInfo: {:?}", self.body))
    }

    /// One member of the reported `otaUpdateInfo`.
    fn info_member(&self, name: &str) -> Option<&Value> {
        self.info().get(name)
    }
}

/// One stored job execution, reduced to what a reader of the store sees.
#[derive(Debug, PartialEq, Eq)]
struct StoredExecution {
    job_id: String,
    status: String,
    version_number: i64,
    execution_number: i64,
    document: Option<String>,
    in_progress_timeout_minutes: Option<i64>,
    started: bool,
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it. The
/// engine handle is kept so a case can read the SHARED job-execution store the control
/// plane writes into.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>, region: &str) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{region}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot(region))
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    async fn create_thing(&self, thing_name: &str) -> Reply {
        self.call(
            "POST",
            &format!("/things/{thing_name}"),
            Some(&json!({})),
            REGION,
        )
        .await
    }


    async fn create_ota_update(&self, ota_update_id: &str, request: &Value) -> Reply {
        self.create_ota_update_in(ota_update_id, request, REGION)
            .await
    }

    async fn create_ota_update_in(
        &self,
        ota_update_id: &str,
        request: &Value,
        region: &str,
    ) -> Reply {
        self.call(
            "POST",
            &format!("/otaUpdates/{ota_update_id}"),
            Some(request),
            region,
        )
        .await
    }

    async fn get_ota_update(&self, ota_update_id: &str) -> Reply {
        self.get_ota_update_in(ota_update_id, REGION).await
    }

    async fn get_ota_update_in(&self, ota_update_id: &str, region: &str) -> Reply {
        self.call("GET", &format!("/otaUpdates/{ota_update_id}"), None, region)
            .await
    }

    /// The document of the AWS IoT job the update deploys through, read through the
    /// job surface a device's `GetJobDocument` call uses.
    async fn job_document(&self, job_id: &str, region: &str) -> Reply {
        self.call(
            "GET",
            &format!("/jobs/{job_id}/job-document"),
            None,
            region,
        )
        .await
    }

    /// `DeleteOTAUpdate` with both flags left out (their modelled default, `false`) or
    /// spelled out.
    async fn delete_ota_update(
        &self,
        ota_update_id: &str,
        force_delete_aws_job: Option<bool>,
        delete_stream: Option<bool>,
    ) -> Reply {
        let mut query: Vec<String> = Vec::new();
        if let Some(force) = force_delete_aws_job {
            query.push(format!("forceDeleteAWSJob={force}"));
        }
        if let Some(delete_stream) = delete_stream {
            query.push(format!("deleteStream={delete_stream}"));
        }
        let query = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };
        self.call(
            "DELETE",
            &format!("/otaUpdates/{ota_update_id}{query}"),
            None,
            REGION,
        )
        .await
    }

    /// Finish `thing_name`'s execution of `job_id` in the SHARED store, which is what a
    /// device reporting the job done through the Jobs data plane leaves behind.
    fn finish_execution(&self, thing_name: &str, job_id: &str) {
        let store = self.engine.state::<JobExecutionStore>();
        let mut store = store.lock();
        let index = store
            .execution_index(thing_name, job_id, None)
            .unwrap_or_else(|| panic!("{thing_name} must hold an execution of {job_id}"));
        store
            .record_mut(thing_name, index)
            .expect("the index came from this store")
            .status = STATUS_SUCCEEDED.to_string();
    }

    /// What the SHARED store holds for `thing_name`, in enqueue order.
    fn executions_for(&self, thing_name: &str) -> Vec<StoredExecution> {
        let store = self.engine.state::<JobExecutionStore>();
        let store = store.lock();
        store
            .executions_for(thing_name)
            .iter()
            .map(|record| StoredExecution {
                job_id: record.job_id.clone(),
                status: record.status.clone(),
                version_number: record.version_number,
                execution_number: record.execution_number,
                document: record.job_document.clone(),
                in_progress_timeout_minutes: record.in_progress_timeout_minutes,
                started: record.started_at.is_some(),
            })
            .collect()
    }
}

/// The thing ARN a target entry names a device by.
fn thing_arn(thing_name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thing/{thing_name}")
}

/// The thing group ARN a target entry names a group by.
fn thing_group_arn(group_name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thinggroup/{group_name}")
}

/// A file located in S3, the form `OTAUpdateFile$fileLocation.s3Location` names.
fn s3_file(file_name: &str, file_version: &str) -> Value {
    json!({
        "fileName": file_name,
        "fileType": 0,
        "fileVersion": file_version,
        "fileLocation": {
            "s3Location": {"bucket": "firmware-images", "key": file_name, "version": "1"}
        }
    })
}

/// A file located in an IoT stream, the other form `fileLocation` names.
fn stream_file(file_name: &str, stream_id: &str, file_id: i64) -> Value {
    json!({
        "fileName": file_name,
        "fileLocation": {"stream": {"streamId": stream_id, "fileId": file_id}}
    })
}

/// One row of the create table.
struct CreateCase {
    case: &'static str,
    ota_update_id: &'static str,
    /// The things the update targets, each as a `thing/…` ARN; a name repeated here is
    /// ONE device targeted twice.
    targets: &'static [&'static str],
    /// The `files` member, always at least one (modelled `len 1..`).
    files: Vec<Value>,
    /// The `protocols` member, when the row carries one.
    protocols: Option<Vec<&'static str>>,
    /// `targetSelection`, when the row carries one.
    target_selection: Option<&'static str>,
    /// The `description` member, when the row carries one.
    description: Option<&'static str>,
    /// `awsJobTimeoutConfig.inProgressTimeoutInMinutes`, when the row carries one.
    in_progress_timeout_minutes: Option<i64>,
    /// `additionalParameters`, when the row carries any.
    additional_parameters: Option<Value>,
    /// `awsJobExecutionsRolloutConfig`, when the row carries one.
    rollout_config: Option<Value>,
    /// The region the row is signed for; its ARNs must be scoped to it.
    region: &'static str,
}

impl CreateCase {
    /// The request body this row sends.
    fn request(&self) -> Value {
        let mut request = json!({
            "targets": self.targets.iter().map(|name| thing_arn(name)).collect::<Vec<String>>(),
            "files": self.files.clone(),
            "roleArn": ROLE_ARN,
        });
        if let Some(protocols) = &self.protocols {
            request["protocols"] = json!(protocols);
        }
        if let Some(selection) = self.target_selection {
            request["targetSelection"] = json!(selection);
        }
        if let Some(description) = self.description {
            request["description"] = json!(description);
        }
        if let Some(minutes) = self.in_progress_timeout_minutes {
            request["awsJobTimeoutConfig"] = json!({"inProgressTimeoutInMinutes": minutes});
        }
        if let Some(parameters) = &self.additional_parameters {
            request["additionalParameters"] = parameters.clone();
        }
        if let Some(rollout) = &self.rollout_config {
            request["awsJobExecutionsRolloutConfig"] = rollout.clone();
        }
        request
    }
}

/// RULE: a `CreateOTAUpdate` records the update under the id the request names, deploys
/// it through the AWS IoT job derived from that id, and every distinct thing it targets
/// holds exactly one `QUEUED` execution of that job afterwards — while `GetOTAUpdate`
/// reports the stored update member for member.
#[tokio::test]
async fn an_ota_update_is_recorded_and_deployed_to_every_targeted_thing() {
    let emulator = Emulator::new();

    // The row at the `files` ceiling: the bound is a count, so it is exercised AT the
    // published maximum, with one past it covered by the refusal table below.
    let many_files: Vec<Value> = (0..MAX_OTA_UPDATE_FILES)
        .map(|index| s3_file(&format!("image-{index}.bin"), "1.0.0"))
        .collect();

    let cases: Vec<CreateCase> = vec![
        CreateCase {
            case: "one target, one S3 file, no optional member",
            ota_update_id: "ota-minimal",
            targets: &["sensor-a"],
            files: vec![s3_file("firmware.bin", "1.0.0")],
            protocols: None,
            target_selection: None,
            description: None,
            in_progress_timeout_minutes: None,
            additional_parameters: None,
            rollout_config: None,
            region: REGION,
        },
        CreateCase {
            case: "two targets, two files, every optional member",
            ota_update_id: "ota-full",
            targets: &["sensor-b", "sensor-c"],
            files: vec![
                s3_file("firmware.bin", "2.0.0"),
                stream_file("bootloader.bin", "stream-one", 1),
            ],
            protocols: Some(vec!["HTTP", "MQTT"]),
            target_selection: Some("CONTINUOUS"),
            description: Some("install firmware 2.0.0"),
            in_progress_timeout_minutes: Some(30),
            additional_parameters: Some(json!({"channel": "beta", "wave": "1"})),
            rollout_config: Some(json!({"maximumPerMinute": 10})),
            region: REGION,
        },
        CreateCase {
            case: "one device targeted twice is one execution",
            ota_update_id: "ota-duplicate-target",
            targets: &["sensor-d", "sensor-d"],
            files: vec![stream_file("firmware.bin", "stream-two", 0)],
            protocols: Some(vec!["MQTT"]),
            target_selection: Some("SNAPSHOT"),
            description: None,
            in_progress_timeout_minutes: Some(1),
            additional_parameters: None,
            rollout_config: Some(json!({
                "exponentialRate": {
                    "baseRatePerMinute": 1,
                    "incrementFactor": 2.0,
                    "rateIncreaseCriteria": {"numberOfNotifiedThings": 1}
                }
            })),
            region: REGION,
        },
        CreateCase {
            case: "a file carrying code-signing material",
            ota_update_id: "ota_signed_1",
            targets: &["sensor-e"],
            files: vec![json!({
                "fileName": "firmware.bin",
                "fileType": 255,
                "fileLocation": {
                    "s3Location": {"bucket": "firmware-images", "key": "signed/firmware.bin"}
                },
                "codeSigning": {"awsSignerJobId": "signer-job-1"},
                "attributes": {"board": "rev-c"}
            })],
            protocols: Some(vec!["HTTP"]),
            target_selection: None,
            description: Some("signed image"),
            in_progress_timeout_minutes: None,
            additional_parameters: Some(json!({"only": ""})),
            rollout_config: None,
            region: REGION,
        },
        CreateCase {
            case: "three targets, signed for another region",
            ota_update_id: "ota-another-region",
            targets: &["sensor-f", "sensor-g", "sensor-h"],
            files: vec![s3_file("firmware.bin", "3.0.0")],
            protocols: Some(vec!["MQTT", "HTTP"]),
            target_selection: Some("SNAPSHOT"),
            description: None,
            in_progress_timeout_minutes: Some(120),
            additional_parameters: None,
            rollout_config: None,
            region: "eu-west-1",
        },
        CreateCase {
            case: "files at the ceiling one update may carry",
            ota_update_id: "ota-many-files",
            targets: &["sensor-i"],
            files: many_files,
            protocols: None,
            target_selection: None,
            description: None,
            in_progress_timeout_minutes: None,
            additional_parameters: None,
            rollout_config: None,
            region: REGION,
        },
    ];

    for case in &cases {
        for thing_name in case.targets {
            let created = emulator.create_thing(thing_name).await;
            assert_eq!(
                created.status, 200,
                "case {:?}: the target device must be registrable, got {} {:?}",
                case.case, created.status, created.body
            );
        }

        let request = case.request();
        let reply = emulator
            .create_ota_update_in(case.ota_update_id, &request, case.region)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            case.case, reply.status, reply.body
        );

        let expected_job_id = format!("{OTA_UPDATE_JOB_ID_PREFIX}{}", case.ota_update_id);
        assert_eq!(
            reply.text("otaUpdateId").as_deref(),
            Some(case.ota_update_id),
            "case {:?}: expected the otaUpdateId it was created under, got {:?}",
            case.case,
            reply.body
        );
        assert_eq!(
            reply.text("otaUpdateArn"),
            Some(format!(
                "arn:aws:iot:{}:{ACCOUNT_ID}:otaupdate/{}",
                case.region, case.ota_update_id
            )),
            "case {:?}: expected the OTA update ARN scoped to the signed region, got {:?}",
            case.case,
            reply.body
        );
        assert_eq!(
            reply.text("awsIotJobId").as_deref(),
            Some(expected_job_id.as_str()),
            "case {:?}: expected the job id derived from the update id, got {:?}",
            case.case,
            reply.body
        );
        assert_eq!(
            reply.text("awsIotJobArn"),
            Some(format!(
                "arn:aws:iot:{}:{ACCOUNT_ID}:job/{expected_job_id}",
                case.region
            )),
            "case {:?}: expected the deployed job's ARN scoped to the signed region, got {:?}",
            case.case,
            reply.body
        );
        assert_eq!(
            reply.text("otaUpdateStatus").as_deref(),
            Some(NEW_OTA_UPDATE_STATUS),
            "case {:?}: expected the creation status an update is answered in, got {:?}",
            case.case,
            reply.body
        );

        // ROUND TRIP: every member the create supplied comes back from the reader,
        // together with the identity the create minted.
        let read = emulator.get_ota_update_in(case.ota_update_id, case.region).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected the created update to be readable, got {} {:?}",
            case.case, read.status, read.body
        );
        assert_eq!(
            read.info_member("otaUpdateId"),
            Some(&json!(case.ota_update_id)),
            "case {:?}: expected the stored id, got {:?}",
            case.case,
            read.body
        );
        assert_eq!(
            read.info_member("targets"),
            Some(&request["targets"]),
            "case {:?}: expected the targets it was created with, got {:?}",
            case.case,
            read.body
        );
        assert_eq!(
            read.info_member("otaUpdateFiles"),
            Some(&request["files"]),
            "case {:?}: expected the files it was created with, got {:?}",
            case.case,
            read.body
        );
        assert_eq!(
            read.info_member("protocols"),
            request.get("protocols"),
            "case {:?}: expected the protocols it was created with, got {:?}",
            case.case,
            read.body
        );
        assert_eq!(
            read.info_member("targetSelection"),
            request.get("targetSelection"),
            "case {:?}: expected the targetSelection it was created with, got {:?}",
            case.case,
            read.body
        );
        assert_eq!(
            read.info_member("description"),
            request.get("description"),
            "case {:?}: expected the description it was created with, got {:?}",
            case.case,
            read.body
        );
        assert_eq!(
            read.info_member("additionalParameters"),
            request.get("additionalParameters"),
            "case {:?}: expected the additionalParameters it was created with, got {:?}",
            case.case,
            read.body
        );
        assert_eq!(
            read.info_member("awsJobExecutionsRolloutConfig"),
            request.get("awsJobExecutionsRolloutConfig"),
            "case {:?}: expected the rollout config it was created with, got {:?}",
            case.case,
            read.body
        );
        assert_eq!(
            read.info_member("awsIotJobId"),
            Some(&json!(expected_job_id)),
            "case {:?}: expected the deployed job's id, got {:?}",
            case.case,
            read.body
        );
        assert!(
            read.info_member("creationDate").is_some_and(Value::is_number),
            "case {:?}: expected a creationDate timestamp, got {:?}",
            case.case,
            read.body
        );
        assert!(
            read.info_member("errorInfo").is_none(),
            "case {:?}: a creation that did not fail reports no errorInfo, got {:?}",
            case.case,
            read.body
        );

        // The deployed job carries the document the OTA service generated for it, so
        // the job's own reader reports one naming every file the request asked to be
        // streamed; what it reports is what the executions below hand the device (the
        // document's structure is `ota_update_job_document`'s own table).
        let reported = emulator.job_document(&expected_job_id, case.region).await;
        let document = reported.text("document");
        assert!(
            document.as_deref().is_some_and(|document| !document.is_empty()),
            "case {:?}: expected the deployed job to carry a document, got {} {:?}",
            case.case,
            reported.status,
            reported.body
        );
        let document_text = document.clone().unwrap_or_default();
        for file in &case.files {
            let file_name = file
                .get("fileName")
                .and_then(Value::as_str)
                .expect("every file of a create case names itself");
            assert!(
                document_text.contains(file_name),
                "case {:?}: expected the generated document to name the file {file_name:?} \
                 the request asked to be streamed, got {document_text:?}",
                case.case
            );
        }

        // The update's targets ARE its job's: one QUEUED execution per DISTINCT thing,
        // carrying the per-device timeout the request named and the job's document.
        let mut distinct: Vec<&str> = case.targets.to_vec();
        distinct.sort_unstable();
        distinct.dedup();
        for thing_name in distinct {
            assert_eq!(
                emulator.executions_for(thing_name),
                vec![StoredExecution {
                    job_id: expected_job_id.clone(),
                    status: STATUS_QUEUED.to_string(),
                    version_number: 1,
                    execution_number: 1,
                    document: document.clone(),
                    in_progress_timeout_minutes: case.in_progress_timeout_minutes,
                    started: false,
                }],
                "case {:?}: expected one queued execution of {expected_job_id} on \
                 {thing_name}, got {:?}",
                case.case,
                emulator.executions_for(thing_name)
            );
        }
    }
}

/// One row of the refusal table.
struct RefusalCase {
    case: &'static str,
    ota_update_id: String,
    /// The `targets` member, verbatim.
    targets: Value,
    /// The `files` member, verbatim.
    files: Value,
    status: u16,
    code: &'static str,
    /// A fragment the refusal's message must carry, so the caller learns which
    /// constraint it violated.
    message_carries: String,
}

/// RULE: a `CreateOTAUpdate` this operation refuses writes NOTHING — no update, no job
/// and no execution for the targets that did resolve.
#[tokio::test]
async fn a_refused_create_records_no_update_and_no_execution() {
    let emulator = Emulator::new();

    let one_file = json!([s3_file("firmware.bin", "1.0.0")]);
    let too_long_name = "x".repeat(MAX_THING_NAME_LEN + 1);
    let over_target_limit: Vec<String> = (0..=MAX_JOB_TARGETS)
        .map(|index| thing_arn(&format!("sensor-over-{index}")))
        .collect();
    let over_file_limit: Vec<Value> = (0..=MAX_OTA_UPDATE_FILES)
        .map(|index| s3_file(&format!("image-{index}.bin"), "1.0.0"))
        .collect();

    let cases: Vec<RefusalCase> = vec![
        RefusalCase {
            case: "a target ARN of another resource type",
            ota_update_id: "ota-bad-type".to_string(),
            targets: json!([format!(
                "arn:aws:iot:{REGION}:{ACCOUNT_ID}:billinggroup/fleet"
            )]),
            files: one_file.clone(),
            status: 400,
            code: "InvalidRequestException",
            message_carries: "billinggroup".to_string(),
        },
        RefusalCase {
            case: "a target that is not an ARN at all",
            ota_update_id: "ota-not-an-arn".to_string(),
            targets: json!(["sensor-a"]),
            files: one_file.clone(),
            status: 400,
            code: "InvalidRequestException",
            message_carries: "is not an IoT thing or thing group ARN".to_string(),
        },
        RefusalCase {
            case: "a thing target whose name no request could address",
            ota_update_id: "ota-unaddressable".to_string(),
            targets: json!([thing_arn(&too_long_name)]),
            files: one_file.clone(),
            status: 400,
            code: "InvalidRequestException",
            message_carries: format!("1..{MAX_THING_NAME_LEN}"),
        },
        RefusalCase {
            case: "a thing group the registry does not hold",
            ota_update_id: "ota-unknown-group".to_string(),
            targets: json!([thing_group_arn("fleet-unknown")]),
            files: one_file.clone(),
            status: 404,
            code: "ResourceNotFoundException",
            message_carries: format!("ThingGroup {ACCOUNT_ID}:fleet-unknown not found."),
        },
        RefusalCase {
            case: "a file naming no location at all",
            ota_update_id: "ota-no-location".to_string(),
            targets: json!([thing_arn("sensor-a")]),
            files: json!([{"fileName": "firmware.bin"}]),
            status: 400,
            code: "InvalidRequestException",
            message_carries: "files[0].fileLocation".to_string(),
        },
        RefusalCase {
            case: "a later file whose location names neither a stream nor an s3Location",
            ota_update_id: "ota-empty-location".to_string(),
            targets: json!([thing_arn("sensor-a")]),
            files: json!([s3_file("firmware.bin", "1.0.0"), {"fileLocation": {}}]),
            status: 400,
            code: "InvalidRequestException",
            message_carries: "files[1].fileLocation".to_string(),
        },
        RefusalCase {
            case: "one target past the ceiling one update may name",
            ota_update_id: "ota-over-targets".to_string(),
            targets: json!(over_target_limit),
            files: one_file.clone(),
            status: 410,
            code: "LimitExceededException",
            message_carries: format!("at most {MAX_JOB_TARGETS} targets"),
        },
        RefusalCase {
            case: "one file past the ceiling one update may carry",
            ota_update_id: "ota-over-files".to_string(),
            targets: json!([thing_arn("sensor-a")]),
            files: json!(over_file_limit),
            status: 410,
            code: "LimitExceededException",
            message_carries: format!("at most {MAX_OTA_UPDATE_FILES} files"),
        },
        RefusalCase {
            case: "an id one character past what the deployed job's id may be",
            ota_update_id: "o".repeat(MAX_OTA_UPDATE_ID_LEN + 1),
            targets: json!([thing_arn("sensor-a")]),
            files: one_file.clone(),
            status: 400,
            code: "InvalidRequestException",
            message_carries: format!("at most {MAX_OTA_UPDATE_ID_LEN} characters"),
        },
    ];

    for case in &cases {
        let request = json!({
            "targets": case.targets.clone(),
            "files": case.files.clone(),
            "roleArn": ROLE_ARN,
        });
        let reply = emulator.create_ota_update(&case.ota_update_id, &request).await;
        assert_eq!(
            reply.status, case.status,
            "case {:?}: expected {}, got {} {:?}",
            case.case, case.status, reply.status, reply.body
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some(case.code),
            "case {:?}: expected {}, got {:?} {:?}",
            case.case,
            case.code,
            reply.error_code,
            reply.body
        );
        assert!(
            reply.message().contains(&case.message_carries),
            "case {:?}: expected a message carrying {:?}, got {:?}",
            case.case,
            case.message_carries,
            reply.body
        );

        // Nothing was written: the update does not exist and no device was targeted.
        let read = emulator.get_ota_update(&case.ota_update_id).await;
        assert_eq!(
            read.status, 404,
            "case {:?}: the refused create must record no update, got {} {:?}",
            case.case, read.status, read.body
        );
        assert_eq!(
            emulator.executions_for("sensor-a").len(),
            0,
            "case {:?}: the refused create must queue no execution, got {:?}",
            case.case,
            emulator.executions_for("sensor-a")
        );
    }
}

/// RULE: the `otaUpdateId` identifies the update, so a second create under a live id is
/// a conflict and leaves the recorded update exactly as it was.
#[tokio::test]
async fn a_second_create_under_a_live_id_conflicts_and_changes_nothing() {
    let emulator = Emulator::new();
    let ota_update_id = "ota-conflict";

    let first = json!({
        "targets": [thing_arn("sensor-first")],
        "files": [s3_file("firmware.bin", "1.0.0")],
        "roleArn": ROLE_ARN,
    });
    assert_eq!(
        emulator.create_ota_update(ota_update_id, &first).await.status,
        200,
        "the first create of {ota_update_id} must be accepted"
    );

    let second = json!({
        "targets": [thing_arn("sensor-second")],
        "files": [s3_file("other.bin", "9.9.9")],
        "roleArn": ROLE_ARN,
    });
    let reply = emulator.create_ota_update(ota_update_id, &second).await;
    assert_eq!(
        reply.status, 409,
        "expected 409 for a repeat create, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code.as_deref(),
        Some("ResourceAlreadyExistsException"),
        "expected ResourceAlreadyExistsException, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.body.get("resourceId"),
        Some(&json!(ota_update_id)),
        "expected the conflict to name the resource that exists, got {:?}",
        reply.body
    );

    let read = emulator.get_ota_update(ota_update_id).await;
    assert_eq!(
        read.info_member("targets"),
        Some(&first["targets"]),
        "expected the FIRST update's targets to survive the conflict, got {:?}",
        read.body
    );
    assert_eq!(
        emulator.executions_for("sensor-second").len(),
        0,
        "expected the refused create to target nothing, got {:?}",
        emulator.executions_for("sensor-second")
    );
    let unknown = emulator.get_ota_update("ota-never-created").await;
    assert_eq!(
        unknown.status, 404,
        "expected 404 for an id nothing created, got {} {:?}",
        unknown.status, unknown.body
    );
}

/// RULE: an update answered in the non-terminal creation status is owed its completion
/// by the clock — every reader after the due instant reports `CREATE_COMPLETE`, stamped
/// with the instant of the transition rather than the instant of the read.
#[tokio::test]
async fn the_creation_the_clock_owes_completes_on_its_own() {
    let emulator = Emulator::new();
    let ota_update_id = "ota-transition";

    let request = json!({
        "targets": [thing_arn("sensor-transition")],
        "files": [s3_file("firmware.bin", "1.0.0")],
        "roleArn": ROLE_ARN,
    });
    let started = Instant::now();
    let created = emulator.create_ota_update(ota_update_id, &request).await;
    assert_eq!(
        created.status, 200,
        "expected 200, got {} {:?}",
        created.status, created.body
    );

    // Before the span is up the update is still being created, so a caller acting
    // straight after the create sees the transient status.
    let early = emulator.get_ota_update(ota_update_id).await;
    if started.elapsed() < OTA_UPDATE_CREATION_SPAN {
        assert_eq!(
            early.info_member("otaUpdateStatus"),
            Some(&json!(NEW_OTA_UPDATE_STATUS)),
            "expected the transient creation status inside the span, got {:?}",
            early.body
        );
    }

    tokio::time::sleep(OTA_UPDATE_CREATION_SPAN + Duration::from_millis(250)).await;

    let settled = emulator.get_ota_update(ota_update_id).await;
    assert_eq!(
        settled.info_member("otaUpdateStatus"),
        Some(&json!(CREATED_OTA_UPDATE_STATUS)),
        "expected the completed creation status past the span, got {:?}",
        settled.body
    );
    let last_modified = settled.info_member("lastModifiedDate").cloned();
    let creation_date = settled.info_member("creationDate").cloned();
    assert!(
        last_modified
            .as_ref()
            .zip(creation_date.as_ref())
            .is_some_and(|(modified, created)| {
                modified.as_f64().unwrap_or_default() >= created.as_f64().unwrap_or_default()
            }),
        "expected the transition to move lastModifiedDate on from creationDate, got {:?}",
        settled.body
    );

    // The transition happened once: a second read reports the same instant, not the
    // instant of the read.
    let again = emulator.get_ota_update(ota_update_id).await;
    assert_eq!(
        again.info_member("lastModifiedDate").cloned(),
        last_modified,
        "expected a stable lastModifiedDate across reads, got {:?}",
        again.body
    );
    assert_eq!(
        again.info_member("otaUpdateStatus"),
        Some(&json!(CREATED_OTA_UPDATE_STATUS)),
        "expected the completed status to stay, got {:?}",
        again.body
    );
}

/// One row of the deletion table.
struct DeletionCase {
    case: &'static str,
    ota_update_id: &'static str,
    /// The things the update targets.
    targets: &'static [&'static str],
    /// Whether every targeted device has finished its execution before the delete.
    finished: bool,
    /// `forceDeleteAWSJob`, when the row spells it out; left out is the modelled
    /// default (`false`).
    force_delete_aws_job: Option<bool>,
    /// `deleteStream`, when the row spells it out.
    delete_stream: Option<bool>,
    /// The status the delete answers with.
    status: u16,
    /// The error code it answers with, for a refusal.
    code: Option<&'static str>,
}

/// RULE: deleting an OTA update removes the three records its create wrote — the
/// update, the AWS IoT job it deployed and that job's per-thing executions — and a job
/// that is still running is deleted only when the request forces it; a refused delete
/// removes nothing.
#[tokio::test]
async fn deleting_an_ota_update_removes_the_job_it_deployed_and_its_executions() {
    let emulator = Emulator::new();

    let cases: Vec<DeletionCase> = vec![
        DeletionCase {
            case: "every device finished, both flags left out",
            ota_update_id: "ota-delete-finished",
            targets: &["delete-sensor-a"],
            finished: true,
            force_delete_aws_job: None,
            delete_stream: None,
            status: 200,
            code: None,
        },
        DeletionCase {
            case: "two devices finished, the stream the request supplied is kept",
            ota_update_id: "ota-delete-two-finished",
            targets: &["delete-sensor-b", "delete-sensor-c"],
            finished: true,
            force_delete_aws_job: Some(false),
            delete_stream: Some(true),
            status: 200,
            code: None,
        },
        DeletionCase {
            case: "a device still has the job queued, and the delete does not force it",
            ota_update_id: "ota-delete-running",
            targets: &["delete-sensor-d"],
            finished: false,
            force_delete_aws_job: None,
            delete_stream: None,
            status: 400,
            code: Some("InvalidRequestException"),
        },
        DeletionCase {
            case: "a device still has the job queued, and the delete forces it",
            ota_update_id: "ota-delete-forced",
            targets: &["delete-sensor-e", "delete-sensor-f"],
            finished: false,
            force_delete_aws_job: Some(true),
            delete_stream: Some(false),
            status: 200,
            code: None,
        },
    ];

    for case in &cases {
        let request = json!({
            "targets": case.targets.iter().map(|name| thing_arn(name)).collect::<Vec<String>>(),
            "files": [s3_file("firmware.bin", "1.0.0")],
            "protocols": ["HTTP"],
            "roleArn": ROLE_ARN,
        });
        assert_eq!(
            emulator
                .create_ota_update(case.ota_update_id, &request)
                .await
                .status,
            200,
            "case {:?}: the update must be creatable",
            case.case
        );
        let job_id = format!("{OTA_UPDATE_JOB_ID_PREFIX}{}", case.ota_update_id);
        if case.finished {
            for thing_name in case.targets {
                emulator.finish_execution(thing_name, &job_id);
            }
        }

        let reply = emulator
            .delete_ota_update(
                case.ota_update_id,
                case.force_delete_aws_job,
                case.delete_stream,
            )
            .await;
        assert_eq!(
            reply.status, case.status,
            "case {:?}: expected {}, got {} {:?}",
            case.case, case.status, reply.status, reply.body
        );
        assert_eq!(
            reply.error_code.as_deref(),
            case.code,
            "case {:?}: expected {:?}, got {:?} {:?}",
            case.case,
            case.code,
            reply.error_code,
            reply.body
        );

        if case.code.is_some() {
            // A refused delete removes nothing: the update, its job's document and
            // every execution are still there.
            assert!(
                reply.message().contains(&job_id),
                "case {:?}: expected the refusal to name the job that is still running, \
                 got {:?}",
                case.case,
                reply.body
            );
            assert_eq!(
                emulator.get_ota_update(case.ota_update_id).await.status,
                200,
                "case {:?}: the refused delete must keep the update",
                case.case
            );
            assert_eq!(
                emulator.job_document(&job_id, REGION).await.status,
                200,
                "case {:?}: the refused delete must keep the deployed job",
                case.case
            );
            for thing_name in case.targets {
                assert_eq!(
                    emulator.executions_for(thing_name).len(),
                    1,
                    "case {:?}: the refused delete must keep {thing_name}'s execution",
                    case.case
                );
            }
            continue;
        }

        let read = emulator.get_ota_update(case.ota_update_id).await;
        assert_eq!(
            read.status, 404,
            "case {:?}: expected the deleted update to be gone, got {} {:?}",
            case.case, read.status, read.body
        );
        let document = emulator.job_document(&job_id, REGION).await;
        assert_eq!(
            document.status, 404,
            "case {:?}: expected the deployed job and its document to be gone, got {} {:?}",
            case.case, document.status, document.body
        );
        for thing_name in case.targets {
            assert_eq!(
                emulator.executions_for(thing_name),
                Vec::new(),
                "case {:?}: expected {thing_name} to hold no execution of the deleted \
                 job, got {:?}",
                case.case,
                emulator.executions_for(thing_name)
            );
        }
    }

    // An id the account holds no update under is the modelled 404, not an empty 200.
    for ota_update_id in ["ota-delete-finished", "ota-never-existed"] {
        let reply = emulator.delete_ota_update(ota_update_id, Some(true), None).await;
        assert_eq!(
            reply.status, 404,
            "case {ota_update_id:?}: expected 404, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {ota_update_id:?}: expected ResourceNotFoundException, got {:?}",
            reply.error_code
        );
        assert_eq!(
            reply.message(),
            missing_ota_update_message(ota_update_id),
            "case {ota_update_id:?}: expected the refusal to name the update, got {:?}",
            reply.body
        );
    }
}
