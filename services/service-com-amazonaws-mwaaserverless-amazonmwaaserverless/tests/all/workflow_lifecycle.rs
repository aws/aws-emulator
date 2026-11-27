// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the MWAA Serverless workflow lifecycle slice:
//! `CreateWorkflow`, `GetWorkflow`, `ListWorkflows`, `StartWorkflowRun`,
//! `GetWorkflowRun` and `DeleteWorkflow` — and, at the end of the file,
//! `CreateAirflowUiUrl`, the launch URL a caller opens the Airflow UI of this account's
//! workflows with.
//!
//! Every test drives the real wire path — `MultiServiceRouter::dispatch` over an
//! `awsJson1_0` request — against ONE shared engine, so what a test asserts is what a
//! client sees. The tables below span the class each rule generalises over rather than
//! pinning one recorded request.

use std::sync::Arc;
use std::time::Duration;

use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    DUPLICATE_WORKFLOW_NAME_MESSAGE, RUN_TERMINAL_STATUSES, WORKFLOW_DELETED_AFTER,
    WORKFLOW_RESOURCE_TYPE,
};
use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::{
    ComAmazonawsMwaaserverlessAmazonmwaaserverlessState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The status the model gives `ConflictException`
/// (`smithy_query(query="shape", shape="ConflictException")`: `@httpError(409)`).
const CONFLICT_STATUS: u16 = 409;

/// The members `CreateWorkflowRequest` and `GetWorkflowResponse` share, from
/// `smithy_query(query="operation", operation="CreateWorkflow,GetWorkflow")`. A create
/// must be readable back through `GetWorkflow` on every one of them.
const ECHOED_MEMBERS: &[&str] = &[
    "Name",
    "Description",
    "DefinitionS3Location",
    "Code",
    "RoleArn",
    "EncryptionConfiguration",
    "LoggingConfiguration",
    "EngineVersion",
    "NetworkConfiguration",
    "Environment",
    "TriggerMode",
];

/// One emulator, reached exactly the way the production router reaches it.
///
/// `pub(crate)` so the other suites of this test target drive the same wire path
/// rather than each building a harness of its own.
pub(crate) struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    region: &'static str,
}

impl Harness {
    pub(crate) fn new(region: &'static str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self {
            router,
            engine,
            region,
        }
    }

    /// Call one operation over the wire and return its status and parsed body.
    pub(crate) async fn call(
        &self,
        operation: &str,
        body: serde_json::Value,
    ) -> (u16, serde_json::Value) {
        self.call_in(self.region, operation, body).await
    }

    /// The credential-scope region this harness signs for: the regional endpoint
    /// [`Harness::call`] reaches.
    pub(crate) fn region(&self) -> &'static str {
        self.region
    }

    /// Call one operation at `region`'s endpoint against the SAME store, so a suite can
    /// show what one region's endpoint does and does not serve. The credential scope is
    /// the only difference from [`Harness::call`].
    pub(crate) async fn call_in(
        &self,
        region: &str,
        operation: &str,
        body: serde_json::Value,
    ) -> (u16, serde_json::Value) {
        let payload = serde_json::to_vec(&body).expect("serialize the request body");
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("AmazonMWAAServerless.{operation}"))
            // The SigV4 credential scope is both the routing signal the core reads
            // (`resolve_route_key`) and the region a minted ARN carries.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20240101/{region}/\
                     airflow-serverless/aws4_request, SignedHeaders=host;x-amz-target, \
                     Signature=00"
                ),
            )
            .body(bytes::Bytes::from(payload))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = serde_json::from_slice(response.body()).unwrap_or(serde_json::Value::Null);
        (status, parsed)
    }

    /// How many runs the STORE holds for a workflow — the direct way to show that a
    /// repeated `ClientToken` created nothing, independent of what any listing reports.
    pub(crate) fn stored_run_count(&self, workflow_arn: &str) -> usize {
        self.engine
            .state::<ComAmazonawsMwaaserverlessAmazonmwaaserverlessState>()
            .lock()
            .workflows
            .get(workflow_arn)
            .map_or(0, |workflow| workflow.runs.len())
    }

    /// The engine this emulator reads and writes, so a suite can inspect or seed a
    /// record of its own — a run's clock fields, or a task instance carrying an
    /// execution this emulator never performs.
    pub(crate) fn engine(&self) -> &Arc<engine::InMemoryEngine> {
        &self.engine
    }

    /// The `(WorkflowVersion, IsLatestVersion)` pairs the STORE holds for a workflow,
    /// in the order the versions were appended — what the STORE holds, independent of
    /// what any listing reports.
    pub(crate) fn stored_versions(&self, workflow_arn: &str) -> Vec<(String, bool)> {
        self.engine
            .state::<ComAmazonawsMwaaserverlessAmazonmwaaserverlessState>()
            .lock()
            .workflows
            .get(workflow_arn)
            .map(|workflow| {
                workflow
                    .versions
                    .iter()
                    .map(|record| (record.version.clone(), record.is_latest))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// `arn:aws:airflow-serverless:<region>:<account>:workflow/<name>-<10 alnum>`, written
/// out from the modelled `WorkflowArn` pattern
/// (`smithy_query(query="shape", shape="WorkflowArn")`):
/// `^arn:aws…:airflow-serverless:([a-z]{2}-[a-z]+-[0-9]{1}):([0-9]{12}):workflow/([a-zA-Z0-9][a-zA-Z0-9\.\-_]{0,254}-[a-zA-Z0-9]{10})$`
///
/// `pub(crate)` so `region_isolation` checks a minted ARN the same way rather than
/// writing a second matcher.
pub(crate) fn is_workflow_arn(arn: &str, region: &str) -> bool {
    let prefix = format!("arn:aws:airflow-serverless:{region}:");
    let Some(rest) = arn.strip_prefix(&prefix) else {
        return false;
    };
    let Some((account, resource)) = rest.split_once(':') else {
        return false;
    };
    if account.len() != 12 || !account.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let Some(name_and_suffix) = resource.strip_prefix("workflow/") else {
        return false;
    };
    if name_and_suffix.len() < 12 {
        return false;
    }
    let (name, dash_suffix) = name_and_suffix.split_at(name_and_suffix.len() - 11);
    let Some(suffix) = dash_suffix.strip_prefix('-') else {
        return false;
    };
    suffix.len() == 10
        && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
        && (1..=255).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// The modelled `WorkflowVersion`: `^[a-z0-9]{32}$`, length 32..32.
fn is_workflow_version(version: &str) -> bool {
    version.len() == 32
        && version
            .bytes()
            .all(|b| b.is_ascii_lowercase() && b.is_ascii_alphabetic() || b.is_ascii_digit())
}

/// The instant a reported timestamp member names, as fractional epoch seconds, and `None`
/// for a member that is not a date-time string at all.
///
/// Every timestamp of this service targets `TimestampValue`, which carries
/// `@timestampFormat("date-time")` (`smithy_query(query="shape", shape="TimestampValue")`),
/// so on the wire it is an ISO 8601 / RFC 3339 STRING — parsed here with the framework's
/// own reader for that format, which is what a generated SDK client parses it with.
///
/// `pub(crate)` so every suite that compares two reported instants reads them the same
/// way rather than writing a second parser.
pub(crate) fn reported_instant(value: &serde_json::Value) -> Option<f64> {
    let text = value.as_str()?;
    ::aws_smithy_types::DateTime::from_str(text, ::aws_smithy_types::date_time::Format::DateTime)
        .ok()
        .map(|instant| instant.as_secs_f64())
}

/// Whether a reported member is a timestamp in the modelled date-time wire form.
pub(crate) fn is_date_time(value: &serde_json::Value) -> bool {
    reported_instant(value).is_some()
}

/// The configuration members the SERVICE itself defaults when a create or an update
/// supplies none, so a reader reports the value in force rather than nothing: the
/// CloudWatch Logs group this service names for the workflow, and the trigger mode its
/// active version's snapshotted definition decides.
///
/// `pub(crate)` beside [`assert_service_default`] so every suite that reads a
/// configuration back states the same two exceptions to "a member the request left out
/// reads back absent".
pub(crate) const SERVICE_DEFAULTED_MEMBERS: &[&str] = &["LoggingConfiguration", "TriggerMode"];

/// Assert that `fetched` — one `GetWorkflow` answer — reports the value THIS SERVICE
/// applies for `member`, which the request that wrote the version supplied no value for.
///
/// Both defaults are derived, never written out here: the log group from the workflow's
/// own id ([`service_log_group_name`]) and the trigger mode from whether the version's
/// definition declares a schedule, which is the same answer's own
/// `ScheduleConfiguration`.
pub(crate) fn assert_service_default(
    case: &str,
    member: &str,
    workflow_arn: &str,
    fetched: &serde_json::Value,
) {
    use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
        service_log_group_name, TRIGGER_MODE_MANUAL_ONLY, TRIGGER_MODE_SCHEDULED,
    };

    let expected = match member {
        "LoggingConfiguration" => {
            serde_json::json!({"LogGroupName": service_log_group_name(workflow_arn)})
        }
        "TriggerMode" => {
            let scheduled = !fetched["ScheduleConfiguration"].is_null();
            serde_json::Value::String(
                if scheduled {
                    TRIGGER_MODE_SCHEDULED
                } else {
                    TRIGGER_MODE_MANUAL_ONLY
                }
                .to_string(),
            )
        }
        other => panic!("case {case:?}: {other} is not a member this service defaults"),
    };
    assert_eq!(
        fetched[member], expected,
        "case {case:?}: the request supplied no {member}, so the service's own default \
         must be reported, got {:?}",
        fetched[member]
    );
}

/// How long one poll of a run waits before asking again, and how many times. The budget is
/// deliberately generous: it bounds a condition that has to HOLD — the run reporting a
/// status it never leaves — and not how fast the host runs, and the loop returns as soon as
/// that status is reported.
const RUN_POLL_INTERVAL: Duration = Duration::from_millis(50);
const RUN_POLLS: usize = 1_200;

/// Poll `GetWorkflowRun` until the run reports a status it never leaves
/// ([`RUN_TERMINAL_STATUSES`]), and answer that whole answer.
///
/// `pub(crate)` so a suite that has to let a run finish waits for the run rather than for a
/// span of the clock.
pub(crate) async fn settled_run(
    harness: &Harness,
    arn: &str,
    run_id: &str,
) -> serde_json::Value {
    let mut last = serde_json::Value::Null;
    for _ in 0..RUN_POLLS {
        let (status, report) = harness
            .call(
                "GetWorkflowRun",
                serde_json::json!({"WorkflowArn": arn, "RunId": run_id}),
            )
            .await;
        assert_eq!(
            status, 200,
            "expected GetWorkflowRun to succeed, got {status} {report:?}"
        );
        let state = report["RunDetail"]["RunState"].as_str().unwrap_or_default();
        if RUN_TERMINAL_STATUSES.contains(&state) {
            return report;
        }
        last = report;
        tokio::time::sleep(RUN_POLL_INTERVAL).await;
    }
    panic!("the run {run_id:?} never settled; last answer {last:?}")
}

/// The `OverrideParameters` a run of `workflow_arn` must report when its start supplied
/// `supplied`: the entry the service injects to name the run's workflow, with the
/// caller's own parameters merged over it.
///
/// `pub(crate)` so every suite that reads a run's parameter set states the rule the same
/// way, and so the injected key and value are named once — out of the service's own
/// constant and the workflow's own id, never written out by hand.
pub(crate) fn expected_run_parameters(
    workflow_arn: &str,
    supplied: &serde_json::Value,
) -> serde_json::Value {
    let mut expected = serde_json::Map::new();
    expected.insert(
        service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::RUN_WORKFLOW_ID_PARAMETER
            .to_string(),
        serde_json::Value::String(
            service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::workflow_resource_id(
                workflow_arn,
            )
            .to_string(),
        ),
    );
    if let Some(parameters) = supplied.as_object() {
        for (key, value) in parameters {
            expected.insert(key.clone(), value.clone());
        }
    }
    serde_json::Value::Object(expected)
}

/// The modelled `IdString` a `RunId` uses: `^[a-zA-Z0-9]+[a-zA-Z0-9\.\-_]*$`, length 1..255.
fn is_run_id(run_id: &str) -> bool {
    (1..=255).contains(&run_id.len())
        && run_id.starts_with(|c: char| c.is_ascii_alphanumeric())
        && run_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// A valid IAM role ARN for the emulator's own account, as `RoleARN` models it.
pub(crate) fn role_arn() -> String {
    "arn:aws:iam::000000000000:role/mwaa-serverless-exec".to_string()
}

/// The create requests the round-trip table covers. Rows differ along what the rule
/// generalises over: which optional members are present, the form and length of the
/// name, map and list sizes, and whether the optional `Code` union is supplied.
fn create_cases() -> Vec<(&'static str, serde_json::Value)> {
    vec![
        (
            "only the required members",
            serde_json::json!({
                "Name": "minimal",
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/minimal.yaml"},
                "RoleArn": role_arn(),
            }),
        ),
        (
            "every optional member, two tags, two subnets",
            serde_json::json!({
                "Name": "fully.specified-workflow_1",
                "DefinitionS3Location": {
                    "Bucket": "defs",
                    "ObjectKey": "dags/full.yaml",
                    "VersionId": "s3objectversion1",
                },
                "Code": {"S3Location": {"Bucket": "code", "ObjectKey": "src.zip"}},
                "RoleArn": role_arn(),
                "Description": "a workflow with every optional member supplied",
                "EncryptionConfiguration": {
                    "Type": "CUSTOMER_MANAGED_KEY",
                    "KmsKeyId": "arn:aws:kms:us-west-2:000000000000:key/abcd",
                },
                "LoggingConfiguration": {"LogGroupName": "/aws/mwaa-serverless/full"},
                "EngineVersion": 1,
                "NetworkConfiguration": {
                    "SecurityGroupIds": ["sg-1"],
                    "SubnetIds": ["subnet-1", "subnet-2"],
                },
                "Tags": {"team": "data", "env": "test"},
                "Environment": {"Variables": {"AIRFLOW_HOME": "/usr/local/airflow",
                                              "retries": "2"}},
                "TriggerMode": "SCHEDULED",
            }),
        ),
        (
            "empty collections, so a map and a list of size zero round-trip",
            serde_json::json!({
                "Name": "emptyCollections",
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/empty.yaml"},
                "RoleArn": role_arn(),
                "NetworkConfiguration": {"SecurityGroupIds": [], "SubnetIds": []},
                "Environment": {"Variables": {}},
                "Tags": {},
            }),
        ),
        (
            "a single-character name, the shortest NameString allows",
            serde_json::json!({
                "Name": "a",
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/a.yaml"},
                "RoleArn": role_arn(),
            }),
        ),
        (
            "a 255-character name, the longest NameString allows",
            serde_json::json!({
                "Name": format!("x{}", "9".repeat(254)),
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/long.yaml"},
                "RoleArn": role_arn(),
                "Description": "the maximum-length name still mints a pattern-valid ARN",
            }),
        ),
        (
            "an AWS_MANAGED_KEY encryption configuration with no key id",
            serde_json::json!({
                "Name": "awsManagedKey",
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/kms.yaml"},
                "RoleArn": role_arn(),
                "EncryptionConfiguration": {"Type": "AWS_MANAGED_KEY"},
                "TriggerMode": "ON_DEMAND",
            }),
        ),
    ]
}

#[tokio::test]
async fn create_mints_a_modelled_workflow_and_get_reads_every_member_back() {
    for (case, body) in create_cases() {
        // A fresh emulator per row, so one row's workflow cannot answer another's read.
        let harness = Harness::new("us-west-2");

        let (status, created) = harness.call("CreateWorkflow", body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateWorkflow to succeed, got {status} {created:?}"
        );

        let arn = created["WorkflowArn"].as_str().unwrap_or_default();
        assert!(
            is_workflow_arn(arn, "us-west-2"),
            "case {case:?}: expected a WorkflowArn matching the modelled pattern, got {arn:?}"
        );
        let version = created["WorkflowVersion"].as_str().unwrap_or_default();
        assert!(
            is_workflow_version(version),
            "case {case:?}: expected a 32-character lowercase-hex WorkflowVersion, got {version:?}"
        );
        assert_eq!(
            created["WorkflowStatus"], "READY",
            "case {case:?}: expected WorkflowStatus READY, got {:?}",
            created["WorkflowStatus"]
        );
        assert_eq!(
            created["IsLatestVersion"],
            serde_json::Value::Bool(true),
            "case {case:?}: expected IsLatestVersion true, got {:?}",
            created["IsLatestVersion"]
        );
        assert!(
            is_date_time(&created["CreatedAt"]),
            "case {case:?}: expected a CreatedAt date-time string, got {:?}",
            created["CreatedAt"]
        );
        assert!(
            created["RevisionId"].as_str().is_some_and(|id| !id.is_empty()),
            "case {case:?}: expected a RevisionId, got {:?}",
            created["RevisionId"]
        );

        // ROUND TRIP: read the resource back through the implemented reader.
        let (status, fetched) = harness
            .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetWorkflow to succeed, got {status} {fetched:?}"
        );
        assert_eq!(
            fetched["WorkflowArn"], created["WorkflowArn"],
            "case {case:?}: GetWorkflow reported a different ARN: {:?}",
            fetched["WorkflowArn"]
        );
        assert_eq!(
            fetched["WorkflowVersion"], created["WorkflowVersion"],
            "case {case:?}: GetWorkflow reported a different version: {:?}",
            fetched["WorkflowVersion"]
        );
        assert_eq!(
            fetched["WorkflowStatus"], created["WorkflowStatus"],
            "case {case:?}: GetWorkflow reported a different status: {:?}",
            fetched["WorkflowStatus"]
        );
        assert_eq!(
            fetched["CreatedAt"], created["CreatedAt"],
            "case {case:?}: GetWorkflow reported a different CreatedAt: {:?}",
            fetched["CreatedAt"]
        );
        for member in ECHOED_MEMBERS {
            let supplied = &body[member];
            if supplied.is_null() {
                continue;
            }
            assert_eq!(
                &fetched[member], supplied,
                "case {case:?}: GetWorkflow must report the {member} supplied at create, \
                 got {:?}",
                fetched[member]
            );
        }
        assert!(
            is_date_time(&fetched["ModifiedAt"]),
            "case {case:?}: expected a ModifiedAt date-time string, got {:?}",
            fetched["ModifiedAt"]
        );
        // The code artifacts are copied at the time of the request that supplies their
        // location, so a create that supplied one reports the copy instant of the
        // version it minted — the workflow's own creation instant — and a create that
        // supplied none reports no instant, as it reports no `Code`.
        if body["Code"].is_null() {
            assert!(
                fetched["CodeSnapshottedAt"].is_null(),
                "case {case:?}: the create supplied no Code, so no copy instant can be \
                 reported, got {:?}",
                fetched["CodeSnapshottedAt"]
            );
        } else {
            assert_eq!(
                fetched["CodeSnapshottedAt"], created["CreatedAt"],
                "case {case:?}: expected CodeSnapshottedAt to be the create's instant, got {:?}",
                fetched["CodeSnapshottedAt"]
            );
        }

        // The version the create reported selects the same stored record.
        let (status, by_version) = harness
            .call(
                "GetWorkflow",
                serde_json::json!({"WorkflowArn": arn, "WorkflowVersion": version}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a versioned GetWorkflow to succeed, got {status} \
             {by_version:?}"
        );
        assert_eq!(
            by_version["DefinitionS3Location"], fetched["DefinitionS3Location"],
            "case {case:?}: the stored version must carry the same definition, got {:?}",
            by_version["DefinitionS3Location"]
        );
    }
}

#[tokio::test]
async fn an_unknown_workflow_is_refused_and_a_duplicate_name_conflicts() {
    let harness = Harness::new("eu-west-1");

    // Pattern-valid ARNs the store has never held.
    let unknown = [
        "arn:aws:airflow-serverless:eu-west-1:000000000000:workflow/absent-0123456789",
        "arn:aws:airflow-serverless:eu-west-1:000000000000:workflow/other.one-abcdefghij",
    ];
    for arn in unknown {
        let (status, body) = harness
            .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 400,
            "case {arn:?}: expected a 400 lookup miss, got {status} {body:?}"
        );
        assert!(
            body["__type"]
                .as_str()
                .is_some_and(|t| t.contains("ResourceNotFoundException")),
            "case {arn:?}: expected a ResourceNotFoundException, got {:?}",
            body["__type"]
        );
    }

    // Names differing in form: a second create under a name already stored conflicts.
    for name in ["demo", "demo.two", "demo-three_4"] {
        let body = serde_json::json!({
            "Name": name,
            "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/d.yaml"},
            "RoleArn": role_arn(),
        });
        let (status, first) = harness.call("CreateWorkflow", body.clone()).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected the first CreateWorkflow to succeed, got {status} {first:?}"
        );

        let (status, conflict) = harness.call("CreateWorkflow", body).await;
        assert_eq!(
            status, 409,
            "case {name:?}: expected ConflictException (409) for a duplicate name, got \
             {status} {conflict:?}"
        );
        assert!(
            conflict["__type"]
                .as_str()
                .is_some_and(|t| t.contains("ConflictException")),
            "case {name:?}: expected a ConflictException, got {:?}",
            conflict["__type"]
        );
    }
}

/// RULE: a create whose `Name` a stored workflow already holds is refused with the
/// modelled `ConflictException`, whose `Message` is the ONE fixed sentence for that
/// condition — it carries nothing of the request — while the holder is reported by the
/// shape's own required `ResourceId`/`ResourceType`.
///
/// The rows differ along the FORM of the name a holder already has (plain, carrying each
/// punctuation character the member admits, the longest the member admits) and along the
/// STATE the holder is in (ready, and one whose teardown has started and so still holds
/// the name) — never merely along the name.
#[tokio::test]
async fn a_name_a_stored_workflow_holds_is_refused_with_one_fixed_sentence() {
    let harness = Harness::new("ap-south-1");

    // `NameString` is length 1..255, `^[a-zA-Z0-9]+[a-zA-Z0-9\.\-_]*$`
    // (`smithy_query(query="operation", operation="CreateWorkflow")`), so the longest
    // name a holder can have is 255 characters of it.
    let longest = format!("n{}", "a".repeat(254));
    let rows: Vec<(&str, String, bool)> = vec![
        ("a plain name", "holder".to_string(), false),
        (
            "a name carrying every punctuation character the member admits",
            "holder.two-three_4".to_string(),
            false,
        ),
        ("the longest name the member admits", longest, false),
        (
            "a holder whose teardown has started and still holds the name",
            "holderTearingDown".to_string(),
            true,
        ),
    ];

    for (case, name, tearing_down) in rows {
        let body = serde_json::json!({
            "Name": name,
            "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/d.yaml"},
            "RoleArn": role_arn(),
        });
        let (status, first) = harness.call("CreateWorkflow", body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first CreateWorkflow to succeed, got {status} {first:?}"
        );
        let holder = first["WorkflowArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        if tearing_down {
            let (status, deleted) = harness
                .call(
                    "DeleteWorkflow",
                    serde_json::json!({"WorkflowArn": holder.clone()}),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DeleteWorkflow to succeed, got {status} {deleted:?}"
            );
            // The removal deadline out of reach, so the create below meets the
            // `DELETING` phase rather than an already removed record.
            crate::workflow_deletion_teardown::with_workflow(&harness, &holder, |workflow| {
                workflow.deletion_due_at =
                    Some(std::time::SystemTime::now() + Duration::from_secs(3_600));
            });
        }

        let (status, conflict) = harness.call("CreateWorkflow", body).await;
        assert_eq!(
            status, CONFLICT_STATUS,
            "case {case:?}: expected ConflictException ({CONFLICT_STATUS}) for a name already \
             held, got {status} {conflict:?}"
        );
        assert_eq!(
            conflict["Message"].as_str(),
            Some(DUPLICATE_WORKFLOW_NAME_MESSAGE),
            "case {case:?}: expected the one fixed sentence for a name already held, got {:?}",
            conflict["Message"]
        );
        let message = conflict["Message"].as_str().unwrap_or_default();
        assert!(
            !message.contains(name.as_str()) && !message.contains(holder.as_str()),
            "case {case:?}: the sentence carries nothing of the request, got {message:?}"
        );
        assert_eq!(
            conflict["ResourceId"].as_str(),
            Some(holder.as_str()),
            "case {case:?}: expected the refusal to report the holder, got {:?}",
            conflict["ResourceId"]
        );
        assert_eq!(
            conflict["ResourceType"].as_str(),
            Some(WORKFLOW_RESOURCE_TYPE),
            "case {case:?}: expected the workflow resource type, got {:?}",
            conflict["ResourceType"]
        );

        // The refused create minted nothing: the holder still reports the one version it
        // was created with, and it is still the only workflow of that name.
        let (status, reported) = harness
            .call(
                "GetWorkflow",
                serde_json::json!({"WorkflowArn": holder.clone()}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the holder is still readable, got {status} {reported:?}"
        );
        let versions = harness.stored_versions(&holder);
        assert_eq!(
            versions.len(),
            1,
            "case {case:?}: a refused create appends no version, got {versions:?}"
        );
    }
}

#[tokio::test]
async fn a_repeated_client_token_replays_the_first_create() {
    let harness = Harness::new("us-east-1");

    // Tokens differing in form, each with its own request.
    for (index, token) in ["t", "Token-1", "~!@#$%^&*()_+"].into_iter().enumerate() {
        let body = serde_json::json!({
            "Name": format!("tokened{index}"),
            "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/t.yaml"},
            "RoleArn": role_arn(),
            "ClientToken": token,
        });
        let (status, first) = harness.call("CreateWorkflow", body.clone()).await;
        assert_eq!(
            status, 200,
            "case {token:?}: expected the first CreateWorkflow to succeed, got {status} {first:?}"
        );
        let (status, replay) = harness.call("CreateWorkflow", body).await;
        assert_eq!(
            status, 200,
            "case {token:?}: expected the token replay to succeed, got {status} {replay:?}"
        );
        assert_eq!(
            replay, first,
            "case {token:?}: a repeated ClientToken must replay the first response, got {replay:?}"
        );

        // The same token with a DIFFERENT request is a conflict, not a second workflow.
        let (status, mismatch) = harness
            .call(
                "CreateWorkflow",
                serde_json::json!({
                    "Name": format!("tokened{index}-other"),
                    "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/t.yaml"},
                    "RoleArn": role_arn(),
                    "ClientToken": token,
                }),
            )
            .await;
        assert_eq!(
            status, 409,
            "case {token:?}: expected ConflictException (409) for a reused token with a \
             different request, got {status} {mismatch:?}"
        );
    }

    // Exactly one workflow per token, never a duplicate.
    let (status, listed) = harness
        .call("ListWorkflows", serde_json::json!({"MaxResults": 100}))
        .await;
    assert_eq!(status, 200, "expected ListWorkflows to succeed, got {status} {listed:?}");
    assert_eq!(
        listed["Workflows"].as_array().map(Vec::len),
        Some(3),
        "expected one workflow per ClientToken, got {:?}",
        listed["Workflows"]
    );
}

#[tokio::test]
async fn list_workflows_pages_over_every_persisted_workflow_exactly_once() {
    let harness = Harness::new("ap-southeast-2");

    // Seven workflows, so the page sizes below divide the set evenly and unevenly.
    let mut expected: Vec<String> = Vec::new();
    for index in 0..7 {
        let (status, created) = harness
            .call(
                "CreateWorkflow",
                serde_json::json!({
                    "Name": format!("paged-{index}"),
                    "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/p.yaml"},
                    "RoleArn": role_arn(),
                    "Description": format!("workflow {index}"),
                }),
            )
            .await;
        assert_eq!(status, 200, "expected create {index} to succeed, got {created:?}");
        expected.push(
            created["WorkflowArn"]
                .as_str()
                .expect("a created workflow has an ARN")
                .to_string(),
        );
    }
    expected.sort();

    // `MaxResults` is modelled `@range(min: 1, max: 100)`: both published bounds plus
    // sizes smaller than, equal to and larger than the set.
    for page_size in [1_i64, 2, 3, 6, 7, 8, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = serde_json::json!({"MaxResults": page_size});
            if let Some(cursor) = token.as_deref() {
                request["NextToken"] = serde_json::Value::String(cursor.to_string());
            }
            let (status, page) = harness.call("ListWorkflows", request).await;
            assert_eq!(
                status, 200,
                "case MaxResults={page_size}: expected ListWorkflows to succeed, got \
                 {status} {page:?}"
            );
            let items = page["Workflows"]
                .as_array()
                .expect("Workflows is a required list")
                .clone();
            assert!(
                items.len() as i64 <= page_size,
                "case MaxResults={page_size}: a page carried {} items",
                items.len()
            );
            for item in &items {
                let arn = item["WorkflowArn"]
                    .as_str()
                    .expect("WorkflowArn is required on a summary");
                assert!(
                    item["Name"].as_str().is_some_and(|n| n.starts_with("paged-")),
                    "case MaxResults={page_size}: expected the stored Name on the summary, \
                     got {:?}",
                    item["Name"]
                );
                assert!(
                    item["Description"]
                        .as_str()
                        .is_some_and(|d| d.starts_with("workflow ")),
                    "case MaxResults={page_size}: expected the stored Description on the \
                     summary, got {:?}",
                    item["Description"]
                );
                assert_eq!(
                    item["WorkflowStatus"], "READY",
                    "case MaxResults={page_size}: expected the stored status, got {:?}",
                    item["WorkflowStatus"]
                );
                seen.push(arn.to_string());
            }
            pages += 1;
            assert!(
                pages <= 16,
                "case MaxResults={page_size}: pagination did not terminate"
            );
            match page["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(
            sorted, expected,
            "case MaxResults={page_size}: paging must yield every workflow exactly once, \
             got {seen:?}"
        );
    }

    // The default `MaxResults` (20) covers the whole set in one page.
    let (status, defaulted) = harness.call("ListWorkflows", serde_json::json!({})).await;
    assert_eq!(
        status, 200,
        "expected ListWorkflows with no MaxResults to succeed, got {status} {defaulted:?}"
    );
    assert_eq!(
        defaulted["Workflows"].as_array().map(Vec::len),
        Some(7),
        "the modelled default MaxResults of 20 must cover all seven, got {:?}",
        defaulted["Workflows"]
    );
    assert!(
        defaulted["NextToken"].is_null(),
        "a complete page must carry no NextToken, got {:?}",
        defaulted["NextToken"]
    );

    // A cursor this service did not issue is refused rather than silently ignored:
    // not hex at all, odd-length hex, hex of a NUL byte, and hex of "abc" — none of
    // which carries the marker the service's own cursors do.
    for bad in ["not-a-token", "zz", "00", "616263"] {
        let (status, body) = harness
            .call(
                "ListWorkflows",
                serde_json::json!({"MaxResults": 10, "NextToken": bad}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {bad:?}: expected ValidationException (400) for a foreign NextToken, got \
             {status} {body:?}"
        );
    }
}

/// The `OverrideParameters` documents the run table covers: a map<string, document>, so
/// the rows differ along the JSON type and nesting of the values.
fn override_parameter_cases() -> Vec<(&'static str, serde_json::Value)> {
    vec![
        ("absent", serde_json::Value::Null),
        ("an empty map", serde_json::json!({})),
        ("a string value", serde_json::json!({"env": "test"})),
        (
            "scalars of every JSON type",
            serde_json::json!({
                "retries": 3,
                "ratio": 0.5,
                "dry_run": true,
                "note": "mixed",
            }),
        ),
        (
            "a nested object and a list",
            serde_json::json!({
                "config": {"region": "us-east-1", "tags": ["a", "b"]},
                "partitions": [1, 2, 3],
            }),
        ),
    ]
}

#[tokio::test]
async fn a_started_run_is_persisted_and_read_back_with_its_override_parameters() {
    for (case, parameters) in override_parameter_cases() {
        let harness = Harness::new("us-east-1");
        let (status, created) = harness
            .call(
                "CreateWorkflow",
                serde_json::json!({
                    "Name": "runnable",
                    "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/r.yaml"},
                    "RoleArn": role_arn(),
                }),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: create failed: {created:?}");
        let arn = created["WorkflowArn"]
            .as_str()
            .expect("a created workflow has an ARN")
            .to_string();
        let workflow_version = created["WorkflowVersion"]
            .as_str()
            .expect("a created workflow has a version")
            .to_string();

        let mut start = serde_json::json!({"WorkflowArn": arn});
        if !parameters.is_null() {
            start["OverrideParameters"] = parameters.clone();
        }
        let (status, started) = harness.call("StartWorkflowRun", start).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected StartWorkflowRun to succeed, got {status} {started:?}"
        );
        let run_id = started["RunId"].as_str().unwrap_or_default();
        assert!(
            is_run_id(run_id),
            "case {case:?}: expected a RunId matching the modelled pattern, got {run_id:?}"
        );
        assert_eq!(
            started["Status"], "STARTING",
            "case {case:?}: a just-started run carries the status the model documents a \
             newly created run with, got {:?}",
            started["Status"]
        );
        assert!(
            is_date_time(&started["StartedAt"]),
            "case {case:?}: expected a StartedAt date-time string, got {:?}",
            started["StartedAt"]
        );

        // ROUND TRIP: the run is read back out of the store.
        let (status, run) = harness
            .call(
                "GetWorkflowRun",
                serde_json::json!({"WorkflowArn": arn, "RunId": run_id}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetWorkflowRun to succeed, got {status} {run:?}"
        );
        assert_eq!(
            run["RunId"], started["RunId"],
            "case {case:?}: GetWorkflowRun reported a different RunId: {:?}",
            run["RunId"]
        );
        assert_eq!(
            run["WorkflowArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the run's workflow ARN, got {:?}",
            run["WorkflowArn"]
        );
        assert_eq!(
            run["WorkflowVersion"].as_str(),
            Some(workflow_version.as_str()),
            "case {case:?}: a run defaults to the workflow's latest version, got {:?}",
            run["WorkflowVersion"]
        );
        assert_eq!(
            run["RunType"], "ON_DEMAND",
            "case {case:?}: a run started through StartWorkflowRun is ON_DEMAND, got {:?}",
            run["RunType"]
        );
        // The parameter set a run reports is the one it EXECUTES with: the entry the
        // service injects to name the run's workflow, with whatever the start supplied
        // merged over it. A start that supplied none, and one that supplied an empty map,
        // are the same absence and report just the injected entry.
        assert_eq!(
            run["OverrideParameters"],
            expected_run_parameters(&arn, &parameters),
            "case {case:?}: expected the injected workflow-id parameter with the supplied \
             overrides over it, got {:?}",
            run["OverrideParameters"]
        );
        let detail = &run["RunDetail"];
        assert_eq!(
            detail["RunId"], started["RunId"],
            "case {case:?}: RunDetail must carry the run's id, got {:?}",
            detail["RunId"]
        );
        assert_eq!(
            detail["RunType"], "ON_DEMAND",
            "case {case:?}: RunDetail must carry the run type, got {:?}",
            detail["RunType"]
        );
        assert_eq!(
            detail["RunState"], "STARTING",
            "case {case:?}: RunDetail's RunState must match the run's status, got {:?}",
            detail["RunState"]
        );
        assert!(
            is_date_time(&detail["CreatedAt"]) && is_date_time(&detail["StartedOn"]),
            "case {case:?}: RunDetail must carry CreatedAt and StartedOn as date-time \
             strings, got {detail:?}"
        );
        for member in ["CompletedOn", "Duration", "ErrorMessage", "TaskInstances"] {
            assert!(
                detail[member].is_null(),
                "case {case:?}: {member} is null while the run is not complete and its \
                 version's definition declares no task, got {:?}",
                detail[member]
            );
        }

        // An id the workflow does not hold resolves no run, whatever its form.
        for unknown in ["doesnotexist", "run-0000000000000000", "a.b-c_d"] {
            let (status, body) = harness
                .call(
                    "GetWorkflowRun",
                    serde_json::json!({"WorkflowArn": arn, "RunId": unknown}),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}/{unknown:?}: expected a 400 lookup miss, got \
                 {status} {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_run_leaves_its_non_terminal_status_when_the_clock_makes_the_change_due() {
    let harness = Harness::new("us-east-1");
    let (_, created) = harness
        .call(
            "CreateWorkflow",
            serde_json::json!({
                "Name": "settling",
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/s.yaml"},
                "RoleArn": role_arn(),
            }),
        )
        .await;
    let arn = created["WorkflowArn"]
        .as_str()
        .expect("a created workflow has an ARN")
        .to_string();
    let (_, started) = harness
        .call("StartWorkflowRun", serde_json::json!({"WorkflowArn": arn}))
        .await;
    let run_id = started["RunId"]
        .as_str()
        .expect("a started run has an id")
        .to_string();

    // Before the first change falls due the caller still observes the initial status.
    let (_, starting) = harness
        .call(
            "GetWorkflowRun",
            serde_json::json!({"WorkflowArn": arn, "RunId": run_id}),
        )
        .await;
    assert_eq!(
        starting["RunDetail"]["RunState"], "STARTING",
        "a run read back immediately is still STARTING, got {:?}",
        starting["RunDetail"]["RunState"]
    );

    // Each transition is owed by the clock, so it lands without anybody polling for
    // it. The whole schedule is covered row by row in `run_lifecycle_progression`; what
    // this test waits for is the CONDITION at the end of it — the run reporting a status
    // it never leaves.
    let settled = settled_run(&harness, &arn, &run_id).await;
    let detail = &settled["RunDetail"];
    assert_eq!(
        detail["RunState"], "SUCCESS",
        "every task this run's definition declared has finished, so the run completed \
         successfully, got {:?}",
        detail["RunState"]
    );
    assert!(
        detail["ErrorMessage"].is_null(),
        "a run that completed successfully reports no ErrorMessage, got {:?}",
        detail["ErrorMessage"]
    );
    assert!(
        is_date_time(&detail["CompletedOn"]),
        "a complete run carries CompletedOn as a date-time string, got {:?}",
        detail["CompletedOn"]
    );
    assert!(
        detail["Duration"].as_i64().is_some_and(|d| d >= 0),
        "a complete run carries a Duration, got {:?}",
        detail["Duration"]
    );

    // And the terminal status is stable across reads.
    let (_, again) = harness
        .call(
            "GetWorkflowRun",
            serde_json::json!({"WorkflowArn": arn, "RunId": run_id}),
        )
        .await;
    assert_eq!(
        again["RunDetail"], settled["RunDetail"],
        "a read must not change what a later caller sees, got {:?}",
        again["RunDetail"]
    );
}

#[tokio::test]
async fn starting_a_run_is_idempotent_and_an_unknown_workflow_is_refused() {
    let harness = Harness::new("us-east-1");
    let (_, created) = harness
        .call(
            "CreateWorkflow",
            serde_json::json!({
                "Name": "idempotentRuns",
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/i.yaml"},
                "RoleArn": role_arn(),
            }),
        )
        .await;
    let arn = created["WorkflowArn"]
        .as_str()
        .expect("a created workflow has an ARN")
        .to_string();

    let mut minted: Vec<String> = Vec::new();
    for token in ["run-token-a", "run-token-b"] {
        let request = serde_json::json!({
            "WorkflowArn": arn,
            "ClientToken": token,
            "OverrideParameters": {"token": token},
        });
        let (status, first) = harness.call("StartWorkflowRun", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {token:?}: expected StartWorkflowRun to succeed, got {status} {first:?}"
        );
        let (status, replay) = harness.call("StartWorkflowRun", request).await;
        assert_eq!(
            status, 200,
            "case {token:?}: expected the token replay to succeed, got {status} {replay:?}"
        );
        assert_eq!(
            replay["RunId"], first["RunId"],
            "case {token:?}: a repeated ClientToken must not start a second run, got {:?}",
            replay["RunId"]
        );
        minted.push(
            first["RunId"]
                .as_str()
                .expect("a started run has an id")
                .to_string(),
        );
    }
    assert_ne!(
        minted[0], minted[1],
        "two different ClientTokens must start two different runs, got {minted:?}"
    );
    assert_eq!(
        harness.stored_run_count(&arn),
        2,
        "the store must hold exactly one run per ClientToken, got {}",
        harness.stored_run_count(&arn)
    );

    // Pattern-valid ARNs the store never held, and a version the workflow never had.
    let (status, body) = harness
        .call(
            "StartWorkflowRun",
            serde_json::json!({
                "WorkflowArn":
                    "arn:aws:airflow-serverless:us-east-1:000000000000:workflow/ghost-0123456789",
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "expected a 400 lookup miss for an unknown workflow, got {status} {body:?}"
    );

    // A `WorkflowVersion` of the modelled form (`^[a-z0-9]{32}$`) the workflow never had.
    let (status, body) = harness
        .call(
            "StartWorkflowRun",
            serde_json::json!({"WorkflowArn": arn, "WorkflowVersion": "0".repeat(32)}),
        )
        .await;
    assert_eq!(
        status, 400,
        "expected a 400 lookup miss for an unknown version, got {status} {body:?}"
    );
}

#[tokio::test]
async fn delete_removes_the_workflow_its_versions_and_its_runs() {
    let harness = Harness::new("us-west-2");

    // Rows differ along whether the delete names the version explicitly.
    for (case, name_the_version) in [("version omitted", false), ("version named", true)] {
        let (_, created) = harness
            .call(
                "CreateWorkflow",
                serde_json::json!({
                    "Name": "deletable",
                    "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/d.yaml"},
                    "RoleArn": role_arn(),
                }),
            )
            .await;
        let arn = created["WorkflowArn"]
            .as_str()
            .expect("a created workflow has an ARN")
            .to_string();
        let version = created["WorkflowVersion"]
            .as_str()
            .expect("a created workflow has a version")
            .to_string();
        let (_, started) = harness
            .call("StartWorkflowRun", serde_json::json!({"WorkflowArn": arn}))
            .await;
        let run_id = started["RunId"]
            .as_str()
            .expect("a started run has an id")
            .to_string();

        let mut request = serde_json::json!({"WorkflowArn": arn});
        if name_the_version {
            request["WorkflowVersion"] = serde_json::Value::String(version.clone());
        }
        let (status, deleted) = harness.call("DeleteWorkflow", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteWorkflow to succeed, got {status} {deleted:?}"
        );
        assert_eq!(
            deleted["WorkflowArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: DeleteWorkflow must echo the ARN it deleted, got {:?}",
            deleted["WorkflowArn"]
        );
        assert_eq!(
            deleted["WorkflowVersion"].as_str(),
            Some(version.as_str()),
            "case {case:?}: DeleteWorkflow must echo the version it deleted, got {:?}",
            deleted["WorkflowVersion"]
        );

        // The delete starts the teardown the docs describe rather than dropping the
        // record inside the request, so the workflow reports the modelled `DELETING`
        // first and disappears when the removal the clock owes falls due. The
        // `DELETING` phase itself, and everything it refuses, is the subject of
        // `workflow_deletion_teardown`.
        tokio::time::sleep(WORKFLOW_DELETED_AFTER + Duration::from_millis(200)).await;

        // The workflow, its versions and its runs are all gone.
        let (status, _) = harness
            .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: GetWorkflow after a delete must be 400, got {status}"
        );
        let (status, _) = harness
            .call(
                "GetWorkflow",
                serde_json::json!({"WorkflowArn": arn, "WorkflowVersion": version}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a versioned GetWorkflow after a delete must be 400, got {status}"
        );
        let (status, _) = harness
            .call(
                "GetWorkflowRun",
                serde_json::json!({"WorkflowArn": arn, "RunId": run_id}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: GetWorkflowRun after a delete must be 400, got {status}"
        );
        let (status, listed) = harness
            .call("ListWorkflows", serde_json::json!({"MaxResults": 100}))
            .await;
        assert_eq!(status, 200, "case {case:?}: ListWorkflows failed: {listed:?}");
        assert_eq!(
            listed["Workflows"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a deleted workflow must not be listed, got {:?}",
            listed["Workflows"]
        );
        let (status, _) = harness
            .call("DeleteWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: deleting an unknown ARN must be 400, got {status}"
        );
        assert_eq!(
            harness.stored_run_count(&arn),
            0,
            "case {case:?}: the deleted workflow's runs must be gone too"
        );
    }
}

#[tokio::test]
async fn deleting_one_named_version_keeps_the_workflow_until_no_version_is_left() {
    use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::ops::types::DefinitionS3Location;
    use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
        WorkflowSnapshot, WorkflowVersionRecord,
    };

    // `DeleteWorkflowRequest.WorkflowVersion` is documented as "Optional. The specific
    // version of the workflow to delete. If not specified, all versions of the workflow
    // are deleted." The rule is defined over the SET of a workflow's versions, so each
    // row below has two of them. The second one is written straight into the store the
    // handlers read, rather than minted by `UpdateWorkflow`, so the row fixes both its
    // identifier and its `created_at` and can name the version it deletes exactly.
    let second = "b".repeat(32);
    for (case, delete_the_latest) in [("delete the latest", true), ("delete the older", false)] {
        let harness = Harness::new("us-west-2");
        let (_, created) = harness
            .call(
                "CreateWorkflow",
                serde_json::json!({
                    "Name": "versioned",
                    "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/v1.yaml"},
                    "RoleArn": role_arn(),
                }),
            )
            .await;
        let arn = created["WorkflowArn"]
            .as_str()
            .expect("a created workflow has an ARN")
            .to_string();
        let first = created["WorkflowVersion"]
            .as_str()
            .expect("a created workflow has a version")
            .to_string();
        {
            let state = harness
                .engine
                .state::<ComAmazonawsMwaaserverlessAmazonmwaaserverlessState>();
            let mut store = state.lock();
            // The append position the seeded version takes, from the same monotonic
            // counter a minted version takes it from, so it sorts after the created one.
            store.mint_seq += 1;
            let sequence = store.mint_seq;
            let workflow = store.workflows.get_mut(&arn).expect("the created workflow");
            let created_at = workflow.created_at + Duration::from_secs(1);
            for record in &mut workflow.versions {
                record.is_latest = false;
            }
            workflow.versions.push(WorkflowVersionRecord {
                version: second.clone(),
                created_at,
                is_latest: true,
                sequence,
                snapshot: WorkflowSnapshot {
                    definition_s3_location: DefinitionS3Location {
                        bucket: "defs".to_string(),
                        object_key: "dags/v2.yaml".to_string(),
                        version_id: None,
                    },
                    // Nothing staged an object at that location, so this seeded version
                    // froze no definition content and declares no schedule.
                    workflow_definition: None,
                    schedule_configuration: None,
                    code: None,
                    role_arn: role_arn(),
                    description: Some("the second version".to_string()),
                    encryption_configuration: None,
                    logging_configuration: None,
                    engine_version: None,
                    network_configuration: None,
                    environment: None,
                    trigger_mode: None,
                },
            });
            workflow.current_version = second.clone();
        }

        let (deleted, survivor) = if delete_the_latest {
            (second.clone(), first.clone())
        } else {
            (first.clone(), second.clone())
        };
        let (status, answered) = harness
            .call(
                "DeleteWorkflow",
                serde_json::json!({"WorkflowArn": arn, "WorkflowVersion": deleted}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a version-scoped delete to succeed, got {status} \
             {answered:?}"
        );
        assert_eq!(
            answered["WorkflowVersion"].as_str(),
            Some(deleted.as_str()),
            "case {case:?}: the response must echo the version deleted, got {:?}",
            answered["WorkflowVersion"]
        );

        // One version is not all of them, so the workflow is still there, reporting the
        // newest version left.
        let (status, fetched) = harness
            .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a workflow with a version left must still be readable, got \
             {status} {fetched:?}"
        );
        assert_eq!(
            fetched["WorkflowVersion"].as_str(),
            Some(survivor.as_str()),
            "case {case:?}: the workflow must report the newest version left, got {:?}",
            fetched["WorkflowVersion"]
        );
        let (status, _) = harness
            .call(
                "GetWorkflow",
                serde_json::json!({"WorkflowArn": arn, "WorkflowVersion": deleted}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the deleted version must be gone, got {status}"
        );

        // Deleting the last version is deleting all of them, so the WORKFLOW goes —
        // through the teardown the delete starts, which reports `DELETING` until the
        // removal the clock owes falls due.
        let (status, answered) = harness
            .call(
                "DeleteWorkflow",
                serde_json::json!({"WorkflowArn": arn, "WorkflowVersion": survivor}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the last version's delete to succeed, got {status} \
             {answered:?}"
        );
        tokio::time::sleep(WORKFLOW_DELETED_AFTER + Duration::from_millis(200)).await;
        let (status, _) = harness
            .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a workflow with no version left must be gone, got {status}"
        );
    }
}

#[tokio::test]
async fn the_caller_driven_stores_are_bounded() {
    use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
        CreatedWorkflow, IdempotentResult, MAX_IDEMPOTENCY_TOKENS, MAX_RUNS_PER_WORKFLOW,
        MAX_WORKFLOWS, WORKFLOW_STATUS_READY,
    };

    let harness = Harness::new("us-east-1");

    // Fill the workflow store to the emulator's own ceiling, then show the next create
    // is refused rather than growing the process without bound.
    for index in 0..MAX_WORKFLOWS {
        let (status, body) = harness
            .call(
                "CreateWorkflow",
                serde_json::json!({
                    "Name": format!("bounded{index}"),
                    "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/b.yaml"},
                    "RoleArn": role_arn(),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case workflow {index}: expected a create below the ceiling to succeed, got \
             {status} {body:?}"
        );
    }
    let (status, refused) = harness
        .call(
            "CreateWorkflow",
            serde_json::json!({
                "Name": "oneTooMany",
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/b.yaml"},
                "RoleArn": role_arn(),
            }),
        )
        .await;
    assert_eq!(
        status, 402,
        "case one past MAX_WORKFLOWS: expected ServiceQuotaExceededException (402), got \
         {status} {refused:?}"
    );
    assert!(
        refused["__type"]
            .as_str()
            .is_some_and(|t| t.contains("ServiceQuotaExceededException")),
        "case one past MAX_WORKFLOWS: expected a ServiceQuotaExceededException, got {:?}",
        refused["__type"]
    );

    // The same bound on one workflow's runs, so workflows x runs is a finite product.
    let runs = Harness::new("us-east-1");
    let (_, created) = runs
        .call(
            "CreateWorkflow",
            serde_json::json!({
                "Name": "boundedRuns",
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/b.yaml"},
                "RoleArn": role_arn(),
            }),
        )
        .await;
    let arn = created["WorkflowArn"]
        .as_str()
        .expect("a created workflow has an ARN")
        .to_string();
    for index in 0..MAX_RUNS_PER_WORKFLOW {
        let (status, body) = runs
            .call("StartWorkflowRun", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case run {index}: expected a run below the ceiling to start, got {status} {body:?}"
        );
    }
    let (status, refused) = runs
        .call("StartWorkflowRun", serde_json::json!({"WorkflowArn": arn}))
        .await;
    assert_eq!(
        status, 402,
        "case one past MAX_RUNS_PER_WORKFLOW: expected ServiceQuotaExceededException (402), \
         got {status} {refused:?}"
    );
    assert_eq!(
        runs.stored_run_count(&arn),
        MAX_RUNS_PER_WORKFLOW,
        "the run store must stop at its ceiling, got {}",
        runs.stored_run_count(&arn)
    );

    // The idempotency ledger is keyed on a caller-chosen token, so it evicts rather
    // than growing: recording far more tokens than the cap leaves it at the cap.
    let ledger = Harness::new("us-east-1");
    let state = ledger
        .engine
        .state::<ComAmazonawsMwaaserverlessAmazonmwaaserverlessState>();
    for index in 0..(MAX_IDEMPOTENCY_TOKENS * 3) {
        state.lock().idempotency.record(
            format!("CreateWorkflow:token-{index}"),
            IdempotentResult::CreateWorkflow {
                request_fingerprint: format!("fingerprint-{index}"),
                workflow_arn: format!("arn-{index}"),
                // The answer that token's first use gave, which the entry carries so a
                // repeat can replay it. What this bound is about is how many entries the
                // ledger keeps, so each one here carries an answer of its own.
                answered: CreatedWorkflow {
                    created_at: std::time::SystemTime::now(),
                    workflow_version: format!("version-{index}"),
                    revision_id: format!("revision-{index}"),
                    workflow_status: WORKFLOW_STATUS_READY.to_string(),
                    is_latest_version: true,
                },
            },
        );
    }
    let remembered = state.lock().idempotency.len();
    assert_eq!(
        remembered, MAX_IDEMPOTENCY_TOKENS,
        "the idempotency ledger must stay at its cap, got {remembered}"
    );
    assert!(
        state.lock().idempotency.get("CreateWorkflow:token-0").is_none(),
        "the oldest token must have been evicted first"
    );
    assert!(
        state
            .lock()
            .idempotency
            .get(&format!(
                "CreateWorkflow:token-{}",
                MAX_IDEMPOTENCY_TOKENS * 3 - 1
            ))
            .is_some(),
        "the newest token must still be remembered"
    );
}

#[tokio::test]
async fn inputs_outside_the_modelled_constraints_are_refused() {
    let harness = Harness::new("us-east-1");
    let good_definition = serde_json::json!({"Bucket": "defs", "ObjectKey": "dags/v.yaml"});

    // Each row breaks ONE modelled constraint, named in the case label.
    let refused: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "Name with a leading '-' breaks ^[a-zA-Z0-9]+...",
            "CreateWorkflow",
            serde_json::json!({
                "Name": "-leading-dash",
                "DefinitionS3Location": good_definition.clone(),
                "RoleArn": role_arn(),
            }),
        ),
        (
            "Name below the length minimum of 1",
            "CreateWorkflow",
            serde_json::json!({
                "Name": "",
                "DefinitionS3Location": good_definition.clone(),
                "RoleArn": role_arn(),
            }),
        ),
        (
            "Name one past the length maximum of 255",
            "CreateWorkflow",
            serde_json::json!({
                "Name": format!("x{}", "9".repeat(255)),
                "DefinitionS3Location": good_definition.clone(),
                "RoleArn": role_arn(),
            }),
        ),
        (
            "RoleArn that is not an IAM role ARN",
            "CreateWorkflow",
            serde_json::json!({
                "Name": "badRole",
                "DefinitionS3Location": good_definition.clone(),
                "RoleArn": "arn:aws:iam::000000000000:user/not-a-role",
            }),
        ),
        (
            "WorkflowArn that does not match the ARN pattern",
            "GetWorkflow",
            serde_json::json!({"WorkflowArn": "not-an-arn"}),
        ),
        (
            "WorkflowArn whose region segment is malformed",
            "GetWorkflow",
            serde_json::json!({
                "WorkflowArn":
                    "arn:aws:airflow-serverless:REGION:000000000000:workflow/demo-0123456789",
            }),
        ),
        (
            "MaxResults one below the range minimum of 1",
            "ListWorkflows",
            serde_json::json!({"MaxResults": 0}),
        ),
        (
            "MaxResults one above the range maximum of 100",
            "ListWorkflows",
            serde_json::json!({"MaxResults": 101}),
        ),
    ];
    for (case, operation, body) in refused {
        let (status, answered) = harness.call(operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected ValidationException (400), got {status} {answered:?}"
        );
        assert!(
            answered["__type"]
                .as_str()
                .is_some_and(|t| t.contains("ValidationException")),
            "case {case:?}: expected a ValidationException, got {:?}",
            answered["__type"]
        );
    }

    // Each published `MaxResults` bound itself is accepted.
    for page_size in [1, 100] {
        let (status, body) = harness
            .call("ListWorkflows", serde_json::json!({"MaxResults": page_size}))
            .await;
        assert_eq!(
            status, 200,
            "case MaxResults={page_size}: a published bound must be accepted, got \
             {status} {body:?}"
        );
    }
}

// ===========================================================================
// `CreateAirflowUiUrl`: the launch URL that opens the Airflow UI, and the
// session it starts
// ===========================================================================

/// One row of the launch-URL table: what the request asks for, and the spans that
/// request means.
struct UiUrlCase {
    label: &'static str,
    /// `ExpiresInSeconds`, or `None` for a request that names none.
    expires_in: Option<i32>,
    /// `SessionDurationInSeconds`, or `None` for a request that names none.
    session_duration: Option<i32>,
    /// How long the URL may be opened in, for the request above.
    url_validity: u64,
    /// How long the session it starts lasts, for the request above.
    session_lifetime: u64,
}

/// The launch-URL requests the table covers. Rows differ along which span the request
/// names and which it leaves to the service's documented default, and they sit on each
/// published bound of the two members: `ExpiresInSeconds` is `@range(5..300)` and
/// `SessionDurationInSeconds` is `@range(1..43200)`
/// (`smithy_query(query="operation", operation="CreateAirflowUiUrl")`).
fn ui_url_cases() -> Vec<UiUrlCase> {
    use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::airflow_ui_session::{
        DEFAULT_SESSION_DURATION, DEFAULT_URL_VALIDITY,
    };

    let default_url = DEFAULT_URL_VALIDITY.as_secs();
    let default_session = DEFAULT_SESSION_DURATION.as_secs();
    vec![
        UiUrlCase {
            label: "neither span named, so both documented defaults apply",
            expires_in: None,
            session_duration: None,
            url_validity: default_url,
            session_lifetime: default_session,
        },
        UiUrlCase {
            label: "both spans at their published minimum",
            expires_in: Some(5),
            session_duration: Some(1),
            url_validity: 5,
            session_lifetime: 1,
        },
        UiUrlCase {
            label: "both spans at their published maximum",
            expires_in: Some(300),
            session_duration: Some(43_200),
            url_validity: 300,
            session_lifetime: 43_200,
        },
        UiUrlCase {
            label: "a URL validity named and the session duration defaulted",
            expires_in: Some(60),
            session_duration: None,
            url_validity: 60,
            session_lifetime: default_session,
        },
        UiUrlCase {
            label: "a session duration named and the URL validity defaulted",
            expires_in: None,
            session_duration: Some(900),
            url_validity: default_url,
            session_lifetime: 900,
        },
    ]
}

/// The modelled `SessionIdString`: `^[a-z0-9]{32}$`, length 32..32
/// (`smithy_query(query="shape", shape="SessionIdString")`).
fn is_session_id(session_id: &str) -> bool {
    session_id.len() == 32
        && session_id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b.is_ascii_lowercase() && b.is_ascii_alphabetic()))
}

/// The `CreateAirflowUiUrl` body for one row.
fn ui_url_body(case: &UiUrlCase) -> serde_json::Value {
    let mut body = serde_json::Map::new();
    if let Some(seconds) = case.expires_in {
        body.insert("ExpiresInSeconds".to_string(), serde_json::json!(seconds));
    }
    if let Some(seconds) = case.session_duration {
        body.insert(
            "SessionDurationInSeconds".to_string(),
            serde_json::json!(seconds),
        );
    }
    serde_json::Value::Object(body)
}

/// RULE: `CreateAirflowUiUrl` mints a UI session and answers a launch URL that leads
/// back to THIS emulator, signed for exactly as long as the request allows the URL to be
/// opened in, while the session it starts is recorded at the caller's endpoint and ends
/// the span the request asked for after it was opened.
#[tokio::test]
async fn a_launch_url_is_signed_for_this_emulator_and_starts_a_recorded_session() {
    use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::airflow_ui_session::session_path;

    for case in ui_url_cases() {
        let label = case.label;
        // A fresh emulator per row, so one row's sessions cannot be read as another's.
        let harness = Harness::new("us-west-2");

        let (status, answered) = harness
            .call("CreateAirflowUiUrl", ui_url_body(&case))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected CreateAirflowUiUrl to succeed, got {status} {answered:?}"
        );

        let session_id = answered["SessionId"].as_str().unwrap_or_default();
        assert!(
            is_session_id(session_id),
            "case {label:?}: expected a 32-character lowercase-alphanumeric SessionId, got \
             {session_id:?}"
        );

        // The URL leads back to this emulator, at the session's own address, carrying a
        // SigV4 query signature that is valid for exactly the span asked for.
        let url = answered["AuthorizedUrl"].as_str().unwrap_or_default();
        assert!(
            (1..=2048).contains(&url.chars().count()),
            "case {label:?}: AuthorizedUrl must satisfy AuthorizedUrlString's length \
             1..2048, got {} characters",
            url.chars().count()
        );
        assert!(
            url.starts_with("https://"),
            "case {label:?}: expected an https URL on this emulator, got {url:?}"
        );
        let path = session_path(session_id);
        assert!(
            url.contains(&path),
            "case {label:?}: expected the URL to address {path:?}, got {url:?}"
        );
        assert!(
            url.contains("X-Amz-Signature=") && url.contains("X-Amz-Credential="),
            "case {label:?}: expected a SigV4 query-signed URL, got {url:?}"
        );
        let expires = format!("X-Amz-Expires={}", case.url_validity);
        assert!(
            url.contains(&expires),
            "case {label:?}: expected the signature to be valid for {expires:?}, got {url:?}"
        );

        assert!(
            is_date_time(&answered["UrlExpiresAt"]),
            "case {label:?}: expected a UrlExpiresAt date-time string, got {:?}",
            answered["UrlExpiresAt"]
        );
        assert!(
            is_date_time(&answered["SessionExpiresAt"]),
            "case {label:?}: expected a SessionExpiresAt date-time string, got {:?}",
            answered["SessionExpiresAt"]
        );

        // The session the URL starts is REAL: the store holds it at the endpoint the
        // call was signed for, ending the span the request asked for after it opened,
        // and that is the instant the answer reported.
        let state = harness
            .engine()
            .state::<ComAmazonawsMwaaserverlessAmazonmwaaserverlessState>();
        let store = state.lock();
        let session = store
            .airflow_ui_session_in(harness.region(), session_id)
            .unwrap_or_else(|| {
                panic!("case {label:?}: the store must hold the session the answer reported")
            });
        assert_eq!(
            session
                .expires_at
                .duration_since(session.created_at)
                .unwrap_or_default(),
            Duration::from_secs(case.session_lifetime),
            "case {label:?}: expected the session to last {} seconds from the instant it \
             opened, got {:?} to {:?}",
            case.session_lifetime,
            session.created_at,
            session.expires_at
        );
        let reported = reported_instant(&answered["SessionExpiresAt"]).unwrap_or_default();
        let stored = session
            .expires_at
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        assert!(
            (reported - stored).abs() < 0.002,
            "case {label:?}: SessionExpiresAt must name the instant the stored session \
             ends, got {reported} against {stored}"
        );
        // A session belongs to the endpoint it was opened through, so another region's
        // endpoint does not hold it.
        assert!(
            store
                .airflow_ui_session_in("eu-west-1", session_id)
                .is_none(),
            "case {label:?}: a session of {:?} must not be held at another region's \
             endpoint",
            harness.region()
        );
    }
}

/// RULE: two calls open two different sessions, and a session the clock has carried past
/// its end is gone from the store — the end the answer already promised, applied whether
/// or not anybody called again.
#[tokio::test]
async fn two_calls_open_two_sessions_and_an_ended_one_is_dropped() {
    let harness = Harness::new("eu-central-1");

    let mut opened = Vec::new();
    for round in 0..2 {
        let (status, answered) = harness
            .call(
                "CreateAirflowUiUrl",
                serde_json::json!({"SessionDurationInSeconds": 43_200}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case round {round}: expected CreateAirflowUiUrl to succeed, got {status} \
             {answered:?}"
        );
        opened.push(
            answered["SessionId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    assert_ne!(
        opened[0], opened[1],
        "case two calls: each call must open its own session, got {opened:?}"
    );

    let state = harness
        .engine()
        .state::<ComAmazonawsMwaaserverlessAmazonmwaaserverlessState>();
    for session_id in &opened {
        assert!(
            state
                .lock()
                .airflow_ui_session_in(harness.region(), session_id)
                .is_some(),
            "case two calls: the store must hold both sessions, {session_id:?} is missing"
        );
    }

    // Bring the FIRST session's end forward to the instant it opened, the way the clock
    // would have carried it there, and let the next call materialise what that owes.
    {
        let mut store = state.lock();
        let session = store
            .airflow_ui_sessions
            .get_mut(&opened[0])
            .expect("the first session is stored");
        session.expires_at = session.created_at;
    }
    let (status, answered) = harness
        .call("CreateAirflowUiUrl", serde_json::json!({}))
        .await;
    assert_eq!(
        status, 200,
        "case an ended session: expected CreateAirflowUiUrl to succeed, got {status} \
         {answered:?}"
    );

    let store = state.lock();
    assert!(
        store
            .airflow_ui_session_in(harness.region(), &opened[0])
            .is_none(),
        "case an ended session: a session past its end must be gone, {:?} is still held",
        opened[0]
    );
    assert!(
        store
            .airflow_ui_session_in(harness.region(), &opened[1])
            .is_some(),
        "case an ended session: a session still within its span must be kept, {:?} is \
         gone",
        opened[1]
    );
}
