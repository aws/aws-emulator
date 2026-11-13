// SPDX-License-Identifier: CC0-1.0

//! THE REPORTS of a bulk thing provisioning task over the wire, with AWS IoT, Amazon S3 and
//! IAM mounted on ONE engine — the shape `core/router/src/lib.rs` mounts them in — so the
//! input file a task names is an object staged with `PutObject` and the role it names is a
//! role IAM actually holds.
//!
//! The RULE asserted here: `ListThingRegistrationTaskReports` reports, for the `reportType`
//! the request asks for, one link per Amazon S3 object of THAT report of the task's own run —
//! the `RESULTS` report holding the lines of the input file that provisioned a thing and the
//! thing each one provisioned, the `ERRORS` report the lines that provisioned none with the
//! reason, and for a run that could not be performed at all the one reason the file was never
//! read. The run is performed by the call that starts the task, so its report is there from
//! then on and does not change as the clock brings the task to its terminal status. Each link
//! is the presigned address of a real object of the emulator's own Amazon S3: following the
//! link and reading the object's bucket and key with `GetObject` answer the same bytes. The
//! links of one report are paged by `maxResults`/`nextToken`, and a `taskId` no caller started
//! is refused (HTTP 400 `InvalidRequestException`).
//!
//! The table spans what the rule generalises over rather than one recorded run: how many
//! lines the file has and which of them provision a thing, whether a line fails because the
//! LINE describes no thing or because the REGISTRY refused it, where the failing lines sit in
//! the file (so a row's `lineNumber` is the line of the file, blank lines included), a run
//! that provisions everything, one that provisions nothing, a run that could not be performed
//! at all, and a run long enough to be reported over more than one object.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::thing_registration_reports::{
    report_bucket, report_object_key, REPORT_OBJECT_ROWS,
};
use service_com_amazonaws_iot_awsiotservice::state::SERVICE_PRINCIPAL;
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `awsQuery` form encoder and the XML tag reader are this crate's test suite's own, in
/// `topic_rule_kinesis_action`, which puts roles in IAM over the same wire.
use crate::topic_rule_kinesis_action::{form_encode, xml_text};

/// The Region every call below is signed for.
const REGION: &str = "us-east-1";

/// The bucket every input file below is staged in.
const BUCKET: &str = "iot-bulk-registration-reports";

/// The modelled error a refused listing carries (`@httpError 400`).
const INVALID_REQUEST: &str = "InvalidRequestException";

/// The two values of the modelled `ReportType` enum
/// (`smithy_query(query="shape",shape="ReportType")`).
const RESULTS: &str = "RESULTS";
const ERRORS: &str = "ERRORS";

/// What a task's `ERRORS` report states for a line that describes no thing to provision,
/// spelled out here rather than taken from the code under test.
const UNPROVISIONABLE_LINE: &str =
    "The line provisions no thing: it is not a JSON object of parameter values the thing \
     resource of the provisioning template can be rendered from.";

/// What a run reports when the input file could not be read at all, spelled out for the same
/// reason.
const UNREADABLE_INPUT_FILE: &str =
    "Unable to read the input file from S3. Check the bucket, the key and the permissions \
     granted by the role.";

/// How this service states that a `taskId` names no thing-registration task.
const MISSING_TASK: &str = "Task doesn't exist";

/// The status a task holds while its run is still owed.
const IN_PROGRESS: &str = "InProgress";

/// How long a wait for a started task's completion may take before the test gives up.
///
/// Generous on purpose: the condition is polled, so a slow host takes longer rather than
/// failing, and nothing here asserts on elapsed time.
const COMPLETION_DEADLINE: Duration = Duration::from_secs(120);

/// How long the poll loop waits between two reads of a task's status.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// A string member of the body, or `None` when it is absent or not a string.
    fn text(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The `resourceLinks` member, or an empty list when it is absent.
    fn links(&self) -> Vec<String> {
        self.body
            .get("resourceLinks")
            .and_then(Value::as_array)
            .map(|links| {
                links
                    .iter()
                    .map(|link| link.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The emulator: the IoT control plane, Amazon S3 and IAM on ONE shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router }
    }

    /// One request through the real router, claimed by the service its credential scope
    /// names. Signature verification is off in this harness, so the signature itself is never
    /// checked.
    async fn send(
        &self,
        method: &str,
        path: &str,
        signing_name: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> Reply {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{signing_name}.{REGION}.amazonaws.com{path}"))
            .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDTESTIOT/20260101/{REGION}/{signing_name}/aws4_request, \
                     SignedHeaders=host, Signature=0000"
                ),
            )
            .header("content-type", content_type)
            .body(bytes::Bytes::from(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let raw = response.into_body();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    /// One IoT control-plane call: restJson1, the body carrying the operation's members.
    async fn iot(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        self.send(method, path, "iot", "application/json", payload)
            .await
    }

    /// One path-style request to the emulator's Amazon S3 front door.
    async fn s3(&self, method: &str, path: &str, body: Vec<u8>) -> u16 {
        self.send(method, path, "s3", "application/octet-stream", body)
            .await
            .status
    }

    /// One IAM call over the `awsQuery` protocol, answered with its XML body.
    async fn iam(&self, case: &str, action: &str, params: &[(&str, &str)]) -> String {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(&form_encode(name));
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://iam.{REGION}.amazonaws.com/"))
            .header("host", format!("iam.{REGION}.amazonaws.com"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDTESTIOT/20260101/{REGION}/iam/aws4_request, \
                     SignedHeaders=host, Signature=0000"
                ),
            )
            .header("content-type", "application/x-www-form-urlencoded")
            .body(bytes::Bytes::from(body.into_bytes()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(&response.into_body()).to_string();
        assert_eq!(
            status, 200,
            "case {case:?}: {action} answered {status}: {text}"
        );
        text
    }

    /// A GET of a link the service handed out, answered with its status and its body's bytes.
    ///
    /// The link is an absolute URL, so the ADDRESS it names is what is fetched: the path and
    /// query of the link go back through the very router that minted it — which is what makes
    /// the link one a caller can follow — and the credential scope the link carries is what
    /// names the service that answers it.
    async fn fetch(&self, link: &str) -> (u16, Vec<u8>) {
        let address = link_address(link);
        let request = http::Request::builder()
            .method("GET")
            .uri(format!("https://s3.{REGION}.amazonaws.com{address}"))
            .header("host", format!("s3.{REGION}.amazonaws.com"))
            .body(bytes::Bytes::new())
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, response.into_body().to_vec())
    }

    /// `GET /<bucket>/<key>` — `GetObject` — the object read as a client reads any other
    /// object of the emulator's Amazon S3, with no link involved.
    async fn get_object(&self, bucket: &str, key: &str) -> (u16, Vec<u8>) {
        let request = http::Request::builder()
            .method("GET")
            .uri(format!(
                "https://s3.{REGION}.amazonaws.com/{bucket}/{key}"
            ))
            .header("host", format!("s3.{REGION}.amazonaws.com"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDTESTIOT/20260101/{REGION}/s3/aws4_request, \
                     SignedHeaders=host, Signature=0000"
                ),
            )
            .body(bytes::Bytes::new())
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, response.into_body().to_vec())
    }

    /// `PUT /<bucket>` — `CreateBucket` — so the staged objects have a bucket to live in.
    async fn given_bucket(&self) {
        let status = self.s3("PUT", &format!("/{BUCKET}"), Vec::new()).await;
        assert_eq!(
            status, 200,
            "the input file bucket must be created first, got {status}"
        );
    }

    /// `PUT /<bucket>/<key>` — `PutObject` — staging `contents` as an input file.
    async fn given_object(&self, case: &str, key: &str, contents: &str) {
        let status = self
            .s3(
                "PUT",
                &format!("/{BUCKET}/{key}"),
                contents.as_bytes().to_vec(),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the input file must be staged, got {status}"
        );
    }

    /// A role trusting this service, answered with the ARN IAM persisted.
    async fn given_role(&self, case: &str, name: &str) -> String {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "Service": SERVICE_PRINCIPAL },
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string();
        let created = self
            .iam(
                case,
                "CreateRole",
                &[("RoleName", name), ("AssumeRolePolicyDocument", &trust)],
            )
            .await;
        xml_text(&created, "Arn")
            .unwrap_or_else(|| panic!("case {case:?}: CreateRole reported no Arn: {created}"))
    }

    /// `POST /things/{thingName}` — `CreateThing`, for a name a case registers before the
    /// task runs.
    async fn create_thing(&self, case: &str, thing_name: &str, request: &Value) {
        let reply = self
            .iot("POST", &format!("/things/{thing_name}"), Some(request))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: {thing_name} must be registered first, got {} {}",
            reply.status, reply.body
        );
    }

    /// `POST /thing-registration-tasks` — `StartThingRegistrationTask`, answered with the
    /// `taskId` it minted.
    async fn start_task(&self, case: &str, key: &str, role_arn: &str, template_body: &str) -> String {
        let reply = self
            .iot(
                "POST",
                "/thing-registration-tasks",
                Some(&json!({
                    "inputFileBucket": BUCKET,
                    "inputFileKey": key,
                    "roleArn": role_arn,
                    "templateBody": template_body,
                })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the task must start, got {} {}",
            reply.status, reply.body
        );
        reply
            .text("taskId")
            .unwrap_or_else(|| panic!("case {case:?}: expected a taskId, got {}", reply.body))
    }

    /// `GET /thing-registration-tasks/{taskId}` — `DescribeThingRegistrationTask`.
    async fn describe_task(&self, task_id: &str) -> Reply {
        self.iot("GET", &format!("/thing-registration-tasks/{task_id}"), None)
            .await
    }

    /// `GET /thing-registration-tasks/{taskId}/reports` —
    /// `ListThingRegistrationTaskReports`, with the query members a case names.
    async fn list_reports(
        &self,
        task_id: &str,
        report_type: &str,
        page: &[(&str, String)],
    ) -> Reply {
        let mut path = format!("/thing-registration-tasks/{task_id}/reports?reportType={report_type}");
        for (name, value) in page {
            path.push('&');
            path.push_str(name);
            path.push('=');
            path.push_str(value);
        }
        self.iot("GET", &path, None).await
    }

    /// Read the task until its run is no longer owed, and answer the status it settled in.
    ///
    /// The CONDITION is polled — the task's own reported status — rather than a span of the
    /// clock being waited out, so a slow host takes more passes rather than failing.
    async fn awaiting_completion(&self, case: &str, task_id: &str) -> String {
        let deadline = Instant::now() + COMPLETION_DEADLINE;
        loop {
            let described = self.describe_task(task_id).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: expected the task to read back, got {} {}",
                described.status, described.body
            );
            let status = described.text("status").unwrap_or_default();
            if status != IN_PROGRESS {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the task was still {IN_PROGRESS} when the wait ran out"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// The path and query of `link` — the address a caller follows it to.
fn link_address(link: &str) -> String {
    let after_scheme = link
        .split_once("://")
        .map_or(link, |(_, rest)| rest);
    let from_path = after_scheme
        .find('/')
        .unwrap_or_else(|| panic!("a link must carry a path, got {link:?}"));
    after_scheme[from_path..].to_string()
}

/// A provisioning template declaring `SerialNumber` and one `AWS::IoT::Thing` resource named
/// from it — the structure AWS documents for a provisioning template.
fn template() -> String {
    json!({
        "Parameters": { "SerialNumber": { "Type": "String" } },
        "Resources": {
            "thing": {
                "Type": "AWS::IoT::Thing",
                "Properties": {
                    "ThingName": { "Ref": "SerialNumber" },
                    "AttributePayload": { "model": "a1" },
                },
            },
        },
    })
    .to_string()
}

/// One line of an input file provisioning the thing `serial`.
fn line(serial: &str) -> String {
    json!({ "SerialNumber": serial }).to_string()
}

/// A `RESULTS` row: the line of the file, and the thing it provisioned.
fn result_row(line_number: usize, thing_name: &str) -> Value {
    json!({ "lineNumber": line_number, "thingName": thing_name })
}

/// An `ERRORS` row: the line of the file, and why it provisioned nothing.
fn error_row(line_number: usize, message: &str) -> Value {
    json!({ "lineNumber": line_number, "errorMessage": message })
}

/// One row of the report table: an input file, and the two reports its run must amount to.
struct Row {
    case: &'static str,
    /// The input file's own bytes — newline delimited JSON.
    input_file: String,
    /// Whether the object is staged at all: a key Amazon S3 holds nothing at is a run that
    /// could not be performed.
    staged: bool,
    /// The rows the `RESULTS` report must hold, in order.
    results: Vec<Value>,
    /// The rows the `ERRORS` report must hold, in order.
    errors: Vec<Value>,
}

/// Every row the report of `report_type` serves, read by following each link the listing
/// reports and splitting the object it serves into its lines.
async fn reported_rows(
    emulator: &Emulator,
    case: &str,
    task_id: &str,
    report_type: &str,
) -> (usize, Vec<Value>) {
    let listed = emulator.list_reports(task_id, report_type, &[]).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected the {report_type} report to list, got {} {}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.text("reportType").as_deref(),
        Some(report_type),
        "case {case:?}: the answer must name the report it is about, got {}",
        listed.body
    );
    assert_eq!(
        listed.text("nextToken"),
        None,
        "case {case:?}: a listing that reported every link must carry no nextToken, got {}",
        listed.body
    );
    let links = listed.links();
    let mut rows = Vec::new();
    for (index, link) in links.iter().enumerate() {
        let (status, bytes) = emulator.fetch(link).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the link {link:?} to serve its object, got {status}"
        );
        // The link is the address of a real object of the emulator's own Amazon S3: the same
        // bucket and key, read with `GetObject` and no link involved, answer the same bytes.
        let bucket = report_bucket(REGION);
        let key = report_object_key(task_id, report_type, index);
        assert_eq!(
            link_address(link).split('?').next(),
            Some(format!("/{bucket}/{key}").as_str()),
            "case {case:?}: expected the link to address {bucket}/{key}, got {link:?}"
        );
        let (held_status, held) = emulator.get_object(&bucket, &key).await;
        assert_eq!(
            held_status, 200,
            "case {case:?}: expected Amazon S3 to hold {key}, got {held_status}"
        );
        assert_eq!(
            held, bytes,
            "case {case:?}: the object the link serves and the object Amazon S3 holds must be \
             the same bytes"
        );
        let text = String::from_utf8(bytes)
            .unwrap_or_else(|_| panic!("case {case:?}: a report object must be text"));
        for row in text.lines().filter(|row| !row.trim().is_empty()) {
            rows.push(serde_json::from_str::<Value>(row).unwrap_or_else(|_| {
                panic!("case {case:?}: every report row must be JSON, got {row:?}")
            }));
        }
    }
    (links.len(), rows)
}

/// How many objects a report of `rows` rows is held in.
fn expected_objects(rows: usize) -> usize {
    rows.div_ceil(REPORT_OBJECT_ROWS)
}

/// A task's reports are the links to the run it performed, report type by report type.
#[tokio::test]
async fn a_tasks_reports_are_the_links_to_the_run_it_performed() {
    let emulator = Emulator::new();
    emulator.given_bucket().await;
    let role_arn = emulator.given_role("the report table", "bulk-report-reader").await;

    // A name registered with a DIFFERENT configuration before the run, so one row's line is
    // refused by the registry itself rather than by the reading of the line.
    emulator
        .create_thing(
            "a line the registry refuses",
            "taken-1",
            &json!({ "attributePayload": { "attributes": { "model": "z9" } } }),
        )
        .await;

    // A run long enough to be reported over more than one object: one line past the row count
    // one object holds.
    let long_run: Vec<String> = (0..=REPORT_OBJECT_ROWS)
        .map(|index| line(&format!("long-{index}")))
        .collect();
    let long_run_results: Vec<Value> = (0..=REPORT_OBJECT_ROWS)
        .map(|index| result_row(index + 1, &format!("long-{index}")))
        .collect();

    let rows: Vec<Row> = vec![
        Row {
            case: "every line provisions a thing",
            input_file: [line("rep-1"), line("rep-2"), line("rep-3")].join("\n"),
            staged: true,
            results: vec![
                result_row(1, "rep-1"),
                result_row(2, "rep-2"),
                result_row(3, "rep-3"),
            ],
            errors: vec![],
        },
        Row {
            case: "a line that describes no thing, between lines that do",
            input_file: [
                line("mixed-1"),
                "not json at all".to_string(),
                line("mixed-3"),
                json!({ "NotTheParameter": "x" }).to_string(),
            ]
            .join("\n"),
            staged: true,
            results: vec![result_row(1, "mixed-1"), result_row(3, "mixed-3")],
            errors: vec![
                error_row(2, UNPROVISIONABLE_LINE),
                error_row(4, UNPROVISIONABLE_LINE),
            ],
        },
        Row {
            case: "a blank line is no device, and the numbering stays the file's own",
            input_file: format!("{}\n\n   \n{}\n", line("blank-1"), line("blank-4")),
            staged: true,
            results: vec![result_row(1, "blank-1"), result_row(4, "blank-4")],
            errors: vec![],
        },
        Row {
            case: "a line the registry refuses carries the registry's own reason",
            input_file: [line("taken-1"), line("free-1")].join("\n"),
            staged: true,
            results: vec![result_row(2, "free-1")],
            errors: vec![error_row(
                1,
                "Thing taken-1 already exists with a different configuration",
            )],
        },
        Row {
            case: "no line provisions a thing",
            input_file: ["[1, 2]".to_string(), "\"text\"".to_string()].join("\n"),
            staged: true,
            results: vec![],
            errors: vec![
                error_row(1, UNPROVISIONABLE_LINE),
                error_row(2, UNPROVISIONABLE_LINE),
            ],
        },
        Row {
            case: "a run that could not be performed at all",
            input_file: line("never-read"),
            staged: false,
            results: vec![],
            errors: vec![json!({ "errorMessage": UNREADABLE_INPUT_FILE })],
        },
        Row {
            case: "a run reported over more than one object",
            input_file: long_run.join("\n"),
            staged: true,
            results: long_run_results,
            errors: vec![],
        },
    ];

    let template_body = template();
    let mut started = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let key = format!("devices/{index}.json");
        if row.staged {
            emulator.given_object(row.case, &key, &row.input_file).await;
        }
        let task_id = emulator
            .start_task(row.case, &key, &role_arn, &template_body)
            .await;

        // The report of a run is there as soon as the task is: the run is performed by the
        // call that started it. What the report holds is read BEFORE the clock has brought the
        // task to its terminal status and again after, and the two must agree — read under the
        // task's OWN reported status, so the assertion is about the state the task is in rather
        // than about how fast the host got here.
        let described = emulator.describe_task(&task_id).await;
        let while_running = (described.text("status").as_deref() == Some(IN_PROGRESS)).then_some(
            (
                reported_rows(&emulator, row.case, &task_id, RESULTS).await,
                reported_rows(&emulator, row.case, &task_id, ERRORS).await,
            ),
        );
        started.push((index, task_id, while_running));
    }

    for (index, task_id, while_running) in started {
        let row = &rows[index];
        let case = row.case;
        emulator.awaiting_completion(case, &task_id).await;

        for (report_type, expected) in [(RESULTS, &row.results), (ERRORS, &row.errors)] {
            let (links, reported) = reported_rows(&emulator, case, &task_id, report_type).await;
            assert_eq!(
                links,
                expected_objects(expected.len()),
                "case {case:?}: expected {} {report_type} link(s) for {} row(s), got {links}",
                expected_objects(expected.len()),
                expected.len()
            );
            assert_eq!(
                &reported, expected,
                "case {case:?}: expected the {report_type} report to hold {expected:?}, got \
                 {reported:?}"
            );
        }

        if let Some((running_results, running_errors)) = while_running {
            let settled_results = reported_rows(&emulator, case, &task_id, RESULTS).await;
            let settled_errors = reported_rows(&emulator, case, &task_id, ERRORS).await;
            assert_eq!(
                running_results, settled_results,
                "case {case:?}: the {RESULTS} report of a run must not change as the clock \
                 completes the task"
            );
            assert_eq!(
                running_errors, settled_errors,
                "case {case:?}: the {ERRORS} report of a run must not change as the clock \
                 completes the task"
            );
        }
    }
}

/// The links of one report are paged by `maxResults`, resumed by `nextToken`, and a token of
/// another listing is refused.
#[tokio::test]
async fn the_links_of_a_report_are_paged_by_max_results() {
    let emulator = Emulator::new();
    emulator.given_bucket().await;
    let role_arn = emulator.given_role("paging", "bulk-report-pager").await;

    // Two objects' worth of `RESULTS` rows, so a page of one link leaves a second to resume
    // to.
    let input_file: Vec<String> = (0..=REPORT_OBJECT_ROWS)
        .map(|index| line(&format!("page-{index}")))
        .collect();
    let key = "devices/paged.json";
    emulator
        .given_object("paging", key, &input_file.join("\n"))
        .await;
    let task_id = emulator
        .start_task("paging", key, &role_arn, &template())
        .await;
    emulator.awaiting_completion("paging", &task_id).await;

    let whole = emulator.list_reports(&task_id, RESULTS, &[]).await;
    let every_link = whole.links();
    assert_eq!(
        every_link.len(),
        expected_objects(input_file.len()),
        "case \"paging\": expected {} links in one page, got {}",
        expected_objects(input_file.len()),
        whole.body
    );

    // One link at a time, following each `nextToken` until the listing reports none: the pages
    // together are the whole listing, in the same order, with no link seen twice.
    let mut paged: Vec<String> = Vec::new();
    let mut page = vec![("maxResults", "1".to_string())];
    for _ in 0..every_link.len() {
        let listed = emulator.list_reports(&task_id, RESULTS, &page).await;
        assert_eq!(
            listed.status, 200,
            "case \"paging\": expected a page, got {} {}",
            listed.status, listed.body
        );
        let links = listed.links();
        assert_eq!(
            links.len(),
            1,
            "case \"paging\": maxResults=1 reports one link, got {}",
            listed.body
        );
        // The link is a presigned address minted per call, so the OBJECT a page names is what
        // is compared: its rows.
        let (status, bytes) = emulator.fetch(&links[0]).await;
        assert_eq!(
            status, 200,
            "case \"paging\": expected the paged link to serve its object, got {status}"
        );
        paged.push(String::from_utf8_lossy(&bytes).to_string());
        match listed.text("nextToken") {
            Some(token) => {
                page = vec![("maxResults", "1".to_string()), ("nextToken", token)];
            }
            None => {
                page = Vec::new();
                break;
            }
        }
    }
    assert!(
        page.is_empty(),
        "case \"paging\": the last page of {} links must carry no nextToken",
        every_link.len()
    );
    assert_eq!(
        paged.len(),
        every_link.len(),
        "case \"paging\": paging one link at a time must report every link, got {paged:?}"
    );
    let mut served = Vec::new();
    for link in &every_link {
        let (_, bytes) = emulator.fetch(link).await;
        served.push(String::from_utf8_lossy(&bytes).to_string());
    }
    assert_eq!(
        paged, served,
        "case \"paging\": the paged objects must be the whole listing's, in order"
    );

    // A token issued for the RESULTS listing is not a continuation of the ERRORS listing.
    let first = emulator
        .list_reports(&task_id, RESULTS, &[("maxResults", "1".to_string())])
        .await;
    let token = first
        .text("nextToken")
        .expect("a truncated listing reports a nextToken");
    let foreign = emulator
        .list_reports(&task_id, ERRORS, &[("nextToken", token.clone())])
        .await;
    assert_eq!(
        foreign.status, 400,
        "case \"a token of another report\": expected 400, got {} {}",
        foreign.status, foreign.body
    );
    assert_eq!(
        foreign.error_code.as_deref(),
        Some(INVALID_REQUEST),
        "case \"a token of another report\": expected {INVALID_REQUEST}, got {:?}",
        foreign.error_code
    );
    let garbled = emulator
        .list_reports(&task_id, RESULTS, &[("nextToken", "zzzz".to_string())])
        .await;
    assert_eq!(
        garbled.status, 400,
        "case \"a token this listing never issued\": expected 400, got {} {}",
        garbled.status, garbled.body
    );
}

/// The reports of a task nobody started are refused, and so is a link to an object no report
/// holds.
#[tokio::test]
async fn the_reports_of_a_task_nobody_started_are_refused() {
    let emulator = Emulator::new();

    for task_id in ["00000000-0000-4000-8000-000000000000", "not-a-task"] {
        for report_type in [RESULTS, ERRORS] {
            let listed = emulator.list_reports(task_id, report_type, &[]).await;
            assert_eq!(
                listed.status, 400,
                "case {task_id:?}/{report_type}: expected 400, got {} {}",
                listed.status, listed.body
            );
            assert_eq!(
                listed.error_code.as_deref(),
                Some(INVALID_REQUEST),
                "case {task_id:?}/{report_type}: expected {INVALID_REQUEST}, got {:?}",
                listed.error_code
            );
            assert_eq!(
                listed.text("message").as_deref(),
                Some(MISSING_TASK),
                "case {task_id:?}/{report_type}: expected {MISSING_TASK:?}, got {}",
                listed.body
            );
        }

        // And the emulator's own Amazon S3 holds no report object of a task nobody started:
        // a report is the objects a RUN wrote, so there are none to read.
        for index in [0, 7] {
            let bucket = report_bucket(REGION);
            let key = report_object_key(task_id, RESULTS, index);
            let (status, _) = emulator.get_object(&bucket, &key).await;
            assert_eq!(
                status, 404,
                "case {bucket}/{key}: a report object no run wrote is not found, got {status}"
            );
        }
    }
}
