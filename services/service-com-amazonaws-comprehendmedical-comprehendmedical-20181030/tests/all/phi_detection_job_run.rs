// SPDX-License-Identifier: CC0-1.0

//! What an asynchronous PHI-detection job's RUN does, end to end.
//!
//! RULE: a batch detection job's terminal status reports what its run actually did — a
//! run that read every document under the submitted `InputDataConfig`, analysed each
//! one's text and wrote that analysis back under the submitted `OutputDataConfig` prefix
//! reaches `COMPLETED` and reports the path of the file describing those results, while
//! the modelled failure state is reserved for a run that could not be carried out and
//! says why. The analysis written for a document is the SAME protected-health-information
//! pass the synchronous `DetectPHI` answers with, so the output of a job is the entities
//! of that document's own text.
//!
//! The second rule here is about the submission: `ClientRequestToken` is the model's
//! `@idempotencyToken`, so a repeat of ONE submission (the token with the same
//! parameters) names the job it already created, while the same token carrying DIFFERENT
//! parameters is a submission of its own and starts an independent job, leaving the first
//! untouched.
//!
//! The tables span what those rules generalise over: how many documents an input holds
//! and how they are keyed, whether the input prefix names a directory or one object,
//! whether input and output share a bucket, whether an output prefix was given at all,
//! what the documents say, and which parameter a reused token changes.
//!
//! Object storage is mounted beside this service on ONE engine and every document is
//! staged through S3's own `PutObject`, so a run reads only what the object store
//! actually holds and writes its output where a client can read it back — over the same
//! in-process callback the handler uses.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

/// The `X-Amz-Target` prefix from the service's wire protocol.
const TARGET_PREFIX: &str = "ComprehendMedical_20181030";

/// The Region every request signs for, and the one object storage is addressed to.
const REGION: &str = "us-east-1";

/// The statuses the model's `JobStatus` enum calls finished, for the poll below.
const TERMINAL_STATUSES: &[&str] = &["COMPLETED", "PARTIAL_SUCCESS", "FAILED", "STOPPED"];

/// How long a poll waits for a job to finish before the test gives up.
///
/// Deliberately generous: the job's own spans are fractions of a second, and this bound
/// is only here so a wedged run fails the test instead of hanging it.
const TERMINAL_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll waits between two looks at a job.
///
/// `std::thread::sleep` rather than `tokio::time::sleep`: the workspace pins tokio
/// without its `time` feature, and each test awaits nothing else while it waits.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// This service mounted beside the object storage its jobs read and write, on ONE shared
/// engine, plus an S3 client over the router's own in-process callback — the very seam a
/// job's run reaches object storage through.
struct Emulator {
    router: MultiServiceRouter,
    s3: ::aws_sdk_s3::Client,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_comprehendmedical_comprehendmedical_20181030::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        let callback = EmbeddedEmulator::from_router(router.clone());
        let s3 = ::service_common::s3_client::s3_client(&callback, REGION);
        Self { router, s3 }
    }

    /// Call `operation` with `body` over the wire this service speaks.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/comprehendmedical/\
                     aws4_request, SignedHeaders=host;x-amz-date, Signature=deadbeef"
                ),
            )
            .body(bytes::Bytes::from(body.to_string()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// Create `bucket` through S3's own `CreateBucket`.
    ///
    /// Idempotent for this suite's purposes: a bucket this account already holds is
    /// accepted, so a row may name a bucket an earlier row created.
    async fn create_bucket(&self, case: &str, bucket: &str) {
        if let Err(error) = self.s3.create_bucket().bucket(bucket).send().await {
            let already_held = error
                .as_service_error()
                .is_some_and(|service| service.is_bucket_already_owned_by_you());
            assert!(
                already_held,
                "setup for case {case:?}: S3 must create {bucket}: {error:?}"
            );
        }
    }

    /// Stage one document at `bucket`/`key` through S3's own `PutObject`.
    async fn put_object(&self, case: &str, bucket: &str, key: &str, body: &[u8]) {
        self.s3
            .put_object()
            .bucket(bucket)
            .key(key)
            .body(::aws_sdk_s3::primitives::ByteStream::from(body.to_vec()))
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("setup for case {case:?}: S3 must hold s3://{bucket}/{key}: {error:?}")
            });
    }

    /// The object at `bucket`/`key`, or `None` when the store holds nothing there.
    async fn object(&self, bucket: &str, key: &str) -> Option<Vec<u8>> {
        let answer = self.s3.get_object().bucket(bucket).key(key).send().await.ok()?;
        let body = answer.body.collect().await.ok()?;
        Some(body.into_bytes().to_vec())
    }

    /// Every key under `bucket`/`prefix`.
    async fn keys_under(&self, bucket: &str, prefix: &str) -> Vec<String> {
        self.s3
            .list_objects_v2()
            .bucket(bucket)
            .prefix(prefix)
            .send()
            .await
            .map(|page| {
                page.contents()
                    .iter()
                    .filter_map(|object| object.key().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Start a PHI job with `body` and return the `JobId` it minted.
    async fn start_phi_job(&self, case: &str, body: &Value) -> String {
        let (status, answer) = self.call("StartPHIDetectionJob", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: starting a PHI job must answer 200, got {status} {answer}"
        );
        answer["JobId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a JobId, got {answer}"))
            .to_string()
    }

    /// Describe a PHI job and return its `ComprehendMedicalAsyncJobProperties`.
    async fn describe_phi_job(&self, case: &str, job_id: &str) -> Value {
        let (status, answer) = self
            .call("DescribePHIDetectionJob", &json!({"JobId": job_id}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: describing {job_id} must answer 200, got {status} {answer}"
        );
        answer["ComprehendMedicalAsyncJobProperties"].clone()
    }

    /// Look at the job until it reports a terminal status, or the deadline passes.
    ///
    /// Waits for the CONDITION rather than for a span: the job finishes when the clock
    /// owes it its terminal status, and the loop simply looks again until it does.
    async fn poll_until_terminal(&self, case: &str, job_id: &str) -> Value {
        let started = Instant::now();
        loop {
            let properties = self.describe_phi_job(case, job_id).await;
            let status = properties["JobStatus"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            if TERMINAL_STATUSES.contains(&status.as_str()) {
                return properties;
            }
            assert!(
                started.elapsed() < TERMINAL_DEADLINE,
                "case {case:?}: the job never reached a terminal status, last seen \
                 {status:?} in {properties}"
            );
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// The entities the SYNCHRONOUS detector reports for `text`, which is the analysis a
    /// run writes for a document holding that text.
    async fn detected_entities(&self, case: &str, text: &str) -> Value {
        let (status, answer) = self.call("DetectPHI", &json!({"Text": text})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DetectPHI must answer 200, got {status} {answer}"
        );
        answer["Entities"].clone()
    }
}

/// The `(Type, Text)` pairs an `Entities` array reports, for an assertion that names what
/// was placed rather than how it was ordered.
fn typed_texts(entities: &Value) -> BTreeSet<(String, String)> {
    entities
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entity| {
            (
                entity["Type"].as_str().unwrap_or_default().to_string(),
                entity["Text"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// A start body carrying everything the model requires, plus `extra`.
///
/// The required members are the model's own: `InputDataConfig`, `OutputDataConfig`,
/// `DataAccessRoleArn` and `LanguageCode`
/// (`smithy_query(query="operation",operation="StartPHIDetectionJob")`).
fn start_body(input: Value, output: Value, extra: Vec<(&str, Value)>) -> Value {
    let mut body = json!({
        "InputDataConfig": input,
        "OutputDataConfig": output,
        "DataAccessRoleArn": "arn:aws:iam::123456789012:role/CMBatchRole",
        "LanguageCode": "en",
    });
    for (member, value) in extra {
        body[member] = value;
    }
    body
}

/// The directory a run writes its output into: the submitted prefix, then a directory
/// named for the account, the analysis and the job id (`OutputDataConfig$S3Key`: the
/// service "creates an output directory using the job ID").
fn output_directory(output_key: Option<&str>, job_id: &str) -> String {
    let prefix = match output_key.unwrap_or("").trim_end_matches('/') {
        "" => String::new(),
        trimmed => format!("{trimmed}/"),
    };
    format!(
        "{prefix}{}-PHI-{job_id}/",
        ::emulator_server::emulator_account_id()
    )
}

/// A row's case as a `JobName` the model admits: its `JobName` pattern
/// (`^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$`) lists no comma, so a case sentence contributes
/// its words alone.
fn job_name(case: &str) -> String {
    case.chars()
        .filter(|character| character.is_alphanumeric() || *character == ' ')
        .collect()
}

/// One document a row stages, and what the pass must place in its text.
struct Document {
    key: &'static str,
    text: &'static str,
    /// The `(Type, Text)` pairs the analysis of this document reports.
    expected: Vec<(&'static str, &'static str)>,
}

/// One row of the run table.
struct RunCase {
    case: &'static str,
    input_bucket: &'static str,
    input_prefix: &'static str,
    output_bucket: &'static str,
    output_key: Option<&'static str>,
    documents: Vec<Document>,
}

/// A finished PHI job reports `COMPLETED` and its run has written, under the submitted
/// output prefix, one `.out` file per input document carrying the entities of THAT
/// document's text — the same entities the synchronous detector reports — beside the file
/// describing the results that `ManifestFilePath` points at.
#[tokio::test]
async fn a_completed_phi_job_wrote_the_analysis_of_every_input_document() {
    let rows = vec![
        RunCase {
            case: "one document under an input prefix",
            input_bucket: "phi-run-one",
            input_prefix: "input/",
            output_bucket: "phi-run-one",
            output_key: Some("output/"),
            documents: vec![Document {
                key: "input/doc.txt",
                text: "Patient Jane Doe, age 52, was seen on 03/09/2023 for a follow-up visit.",
                expected: vec![("NAME", "Jane Doe"), ("AGE", "52"), ("DATE", "03/09/2023")],
            }],
        },
        RunCase {
            case: "several documents, one of them under a nested key",
            input_bucket: "phi-run-many",
            input_prefix: "corpus/",
            output_bucket: "phi-run-many",
            output_key: Some("results/phi/"),
            documents: vec![
                Document {
                    key: "corpus/first.txt",
                    text: "Results sent to jane.roe@example.com.",
                    expected: vec![("EMAIL", "jane.roe@example.com")],
                },
                Document {
                    key: "corpus/deeper/second.txt",
                    text: "Call the clinic at (555) 123-4567.",
                    expected: vec![("PHONE_OR_FAX", "(555) 123-4567")],
                },
                Document {
                    key: "corpus/third.txt",
                    text: "Blood pressure was stable throughout.",
                    expected: vec![],
                },
            ],
        },
        RunCase {
            case: "an input prefix naming one document rather than a directory",
            input_bucket: "phi-run-single",
            input_prefix: "only/one.txt",
            output_bucket: "phi-run-single",
            output_key: Some("out"),
            documents: vec![Document {
                key: "only/one.txt",
                text: "Mr. Okonkwo was seen today.",
                expected: vec![("NAME", "Okonkwo")],
            }],
        },
        RunCase {
            case: "input and output in different buckets",
            input_bucket: "phi-run-in",
            input_prefix: "docs/",
            output_bucket: "phi-run-out",
            output_key: Some("analysed/"),
            documents: vec![Document {
                key: "docs/note.txt",
                text: "Dr. Alice Nguyen signed the note.",
                expected: vec![("NAME", "Alice Nguyen")],
            }],
        },
        RunCase {
            case: "no output prefix at all",
            input_bucket: "phi-run-root",
            input_prefix: "in/",
            output_bucket: "phi-run-root",
            output_key: None,
            documents: vec![Document {
                key: "in/note.txt",
                text: "MRN: 1234567 on file.",
                expected: vec![("ID", "1234567")],
            }],
        },
    ];

    let emulator = Emulator::new();
    for row in &rows {
        let case = row.case;
        emulator.create_bucket(case, row.input_bucket).await;
        emulator.create_bucket(case, row.output_bucket).await;
        for document in &row.documents {
            emulator
                .put_object(case, row.input_bucket, document.key, document.text.as_bytes())
                .await;
        }

        let mut output = json!({"S3Bucket": row.output_bucket});
        if let Some(output_key) = row.output_key {
            output["S3Key"] = json!(output_key);
        }
        let job_id = emulator
            .start_phi_job(
                case,
                &start_body(
                    json!({"S3Bucket": row.input_bucket, "S3Key": row.input_prefix}),
                    output,
                    vec![("JobName", json!(job_name(case)))],
                ),
            )
            .await;

        let properties = emulator.poll_until_terminal(case, &job_id).await;
        assert_eq!(
            properties["JobStatus"].as_str(),
            Some("COMPLETED"),
            "case {case:?}: a run that read and wrote its documents must report COMPLETED, \
             got {properties}"
        );
        assert!(
            properties["EndTime"].as_f64() > properties["SubmitTime"].as_f64(),
            "case {case:?}: a finished job carries an EndTime after its SubmitTime, got \
             {properties}"
        );

        // The analysed output of each document, where the submitted prefix says it is.
        let directory = output_directory(row.output_key, &job_id);
        for document in &row.documents {
            let relative = document
                .key
                .strip_prefix(row.input_prefix)
                .filter(|relative| !relative.is_empty())
                .unwrap_or_else(|| document.key.rsplit('/').next().unwrap_or(document.key));
            let output_key = format!("{directory}{relative}.out");
            let written = emulator
                .object(row.output_bucket, &output_key)
                .await
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected the analysis of {} at \
                         s3://{}/{output_key}, found nothing",
                        document.key, row.output_bucket,
                    )
                });
            let analysed: Value = serde_json::from_slice(&written).unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: the output at s3://{}/{output_key} must be JSON ({error}): \
                     {:?}",
                    row.output_bucket,
                    String::from_utf8_lossy(&written),
                )
            });
            let found = typed_texts(&analysed["Entities"]);
            let wanted: BTreeSet<(String, String)> = document
                .expected
                .iter()
                .map(|(phi_type, text)| (phi_type.to_string(), text.to_string()))
                .collect();
            assert_eq!(
                found, wanted,
                "case {case:?}: the analysis of {} must be of its own text, expected \
                 {wanted:?}, got {analysed}",
                document.key,
            );
            // The analysis written for a document is the SAME pass the synchronous
            // operation answers with, member for member.
            let synchronous = emulator.detected_entities(case, document.text).await;
            assert_eq!(
                analysed["Entities"], synchronous,
                "case {case:?}: the output for {} must carry the entities the synchronous \
                 detector reports, got {analysed}",
                document.key,
            );
            assert_eq!(
                analysed["ModelVersion"].as_str().map(str::is_empty),
                Some(false),
                "case {case:?}: the output for {} names the model that analysed it, got \
                 {analysed}",
                document.key,
            );
        }

        // The file describing the results, which is what `ManifestFilePath` points at.
        let manifest_path = properties["ManifestFilePath"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a completed job reports a manifest, got {properties}"))
            .to_string();
        let expected_path = format!("s3://{}/{directory}Manifest.json", row.output_bucket);
        assert_eq!(
            manifest_path, expected_path,
            "case {case:?}: expected the manifest at {expected_path}, got {manifest_path}"
        );
        let manifest = emulator
            .object(row.output_bucket, &format!("{directory}Manifest.json"))
            .await
            .unwrap_or_else(|| {
                panic!("case {case:?}: the reported manifest {manifest_path} must exist")
            });
        let manifest: Value = serde_json::from_slice(&manifest)
            .unwrap_or_else(|error| panic!("case {case:?}: the manifest must be JSON: {error}"));
        assert_eq!(
            manifest["DocumentCount"].as_u64(),
            Some(row.documents.len() as u64),
            "case {case:?}: the manifest must count the documents analysed, got {manifest}"
        );
        assert_eq!(
            manifest["JobId"].as_str(),
            Some(job_id.as_str()),
            "case {case:?}: the manifest must name its own job, got {manifest}"
        );

        // Nothing else was written under the job's directory: one output file per
        // document, plus the manifest.
        let written = emulator.keys_under(row.output_bucket, &directory).await;
        assert_eq!(
            written.len(),
            row.documents.len() + 1,
            "case {case:?}: expected one output file per document and one manifest, got \
             {written:?}"
        );
    }
}

/// One row of the failure table: a submission whose run cannot be carried out, and the
/// part of the reason the job must report.
struct FailureCase {
    case: &'static str,
    input_bucket: &'static str,
    input_prefix: &'static str,
    /// The document staged for the row, if any.
    document: Option<(&'static str, &'static [u8])>,
    /// A fragment the reported `Message` must name.
    names: &'static str,
}

/// A run that could not be carried out leaves the job in the modelled failure state,
/// saying why — and writes nothing under the output prefix, so no manifest is reported.
#[tokio::test]
async fn a_run_that_could_not_be_carried_out_reports_the_modelled_failure() {
    let rows = vec![
        FailureCase {
            case: "an input prefix holding no document",
            input_bucket: "phi-fail-empty",
            input_prefix: "nothing/",
            document: Some(("elsewhere/doc.txt", b"Patient Jane Doe was seen today.")),
            names: "No input documents were found at s3://phi-fail-empty/nothing/",
        },
        FailureCase {
            case: "a bucket this account holds no documents in",
            input_bucket: "phi-fail-absent",
            input_prefix: "input/",
            document: None,
            names: "s3://phi-fail-absent/input/",
        },
        FailureCase {
            case: "a document that is not UTF-8 text",
            input_bucket: "phi-fail-binary",
            input_prefix: "input/",
            document: Some(("input/scan.bin", &[0xff, 0xfe, 0x00, 0x01])),
            names: "s3://phi-fail-binary/input/scan.bin",
        },
    ];

    let emulator = Emulator::new();
    // The output bucket exists for every row, so a row that writes nothing does so
    // because its run never got that far.
    emulator.create_bucket("the output bucket", "phi-fail-out").await;
    for row in &rows {
        let case = row.case;
        // The absent-bucket row is the one bucket that is deliberately never created.
        if row.document.is_some() {
            emulator.create_bucket(case, row.input_bucket).await;
        }
        if let Some((key, body)) = row.document {
            emulator
                .put_object(case, row.input_bucket, key, body)
                .await;
        }

        let job_id = emulator
            .start_phi_job(
                case,
                &start_body(
                    json!({"S3Bucket": row.input_bucket, "S3Key": row.input_prefix}),
                    json!({"S3Bucket": "phi-fail-out", "S3Key": "out/"}),
                    vec![("JobName", json!(job_name(case)))],
                ),
            )
            .await;

        let properties = emulator.poll_until_terminal(case, &job_id).await;
        assert_eq!(
            properties["JobStatus"].as_str(),
            Some("FAILED"),
            "case {case:?}: a run that could not be carried out must report FAILED, got \
             {properties}"
        );
        let message = properties["Message"].as_str().unwrap_or_default();
        assert!(
            message.contains(row.names),
            "case {case:?}: the failure must name {:?}, got {message:?}",
            row.names
        );
        assert!(
            properties["ManifestFilePath"].is_null(),
            "case {case:?}: a run that produced no results reports no manifest, got \
             {properties}"
        );
        let written = emulator
            .keys_under("phi-fail-out", &output_directory(Some("out/"), &job_id))
            .await;
        assert!(
            written.is_empty(),
            "case {case:?}: a failed run writes no output, got {written:?}"
        );
    }
}

/// One row of the submission table: which member a reuse of the token changes.
struct SubmissionCase {
    case: &'static str,
    /// The members this submission sets beside the required ones.
    extra: Vec<(&'static str, Value)>,
    /// The input location, when the row changes it.
    input: Value,
    /// The output location, when the row changes it.
    output: Value,
    /// Whether this submission is the SAME one as the first and so must be answered with
    /// the job it already created.
    same_submission: bool,
}

/// One `ClientRequestToken` identifies a SUBMISSION: repeating that submission answers
/// with the job it already created, while the same token carrying different parameters
/// starts an independent job and leaves the first job exactly as it was.
#[tokio::test]
async fn a_reused_request_token_is_idempotent_only_for_the_same_submission() {
    let emulator = Emulator::new();
    let case = "the first submission";
    emulator.create_bucket(case, "phi-token-in").await;
    emulator.create_bucket(case, "phi-token-out").await;
    emulator.create_bucket(case, "phi-token-other").await;
    emulator
        .put_object(
            case,
            "phi-token-in",
            "input/doc.txt",
            b"Patient Jane Doe, age 52, was seen on 03/09/2023.",
        )
        .await;
    emulator
        .put_object(
            case,
            "phi-token-in",
            "other/doc.txt",
            b"Patient John Doe reports no pain.",
        )
        .await;

    let token = json!("sim2real-token-1");
    let input = json!({"S3Bucket": "phi-token-in", "S3Key": "input/"});
    let output = json!({"S3Bucket": "phi-token-out", "S3Key": "output/"});
    let first_extra = vec![
        ("ClientRequestToken", token.clone()),
        ("JobName", json!("token-job")),
        ("KMSKey", json!("alias/cm")),
    ];
    let first = emulator
        .start_phi_job(
            case,
            &start_body(input.clone(), output.clone(), first_extra.clone()),
        )
        .await;
    let first_properties = emulator.describe_phi_job(case, &first).await;

    let rows = vec![
        SubmissionCase {
            case: "the identical submission again",
            extra: first_extra.clone(),
            input: input.clone(),
            output: output.clone(),
            same_submission: true,
        },
        SubmissionCase {
            case: "the same token with a changed JobName",
            extra: vec![
                ("ClientRequestToken", token.clone()),
                ("JobName", json!("token-job-CHANGED")),
                ("KMSKey", json!("alias/cm")),
            ],
            input: input.clone(),
            output: output.clone(),
            same_submission: false,
        },
        SubmissionCase {
            case: "the same token with no JobName at all",
            extra: vec![
                ("ClientRequestToken", token.clone()),
                ("KMSKey", json!("alias/cm")),
            ],
            input: input.clone(),
            output: output.clone(),
            same_submission: false,
        },
        SubmissionCase {
            case: "the same token with a changed input prefix",
            extra: first_extra.clone(),
            input: json!({"S3Bucket": "phi-token-in", "S3Key": "other/"}),
            output: output.clone(),
            same_submission: false,
        },
        SubmissionCase {
            case: "the same token with a changed output bucket",
            extra: first_extra.clone(),
            input: input.clone(),
            output: json!({"S3Bucket": "phi-token-other", "S3Key": "output/"}),
            same_submission: false,
        },
        SubmissionCase {
            case: "the same token with a changed output encryption key",
            extra: vec![
                ("ClientRequestToken", token.clone()),
                ("JobName", json!("token-job")),
                ("KMSKey", json!("alias/other")),
            ],
            input: input.clone(),
            output: output.clone(),
            same_submission: false,
        },
        SubmissionCase {
            case: "the same token with a changed data access role",
            extra: vec![
                ("ClientRequestToken", token.clone()),
                ("JobName", json!("token-job")),
                ("KMSKey", json!("alias/cm")),
                (
                    "DataAccessRoleArn",
                    json!("arn:aws:iam::123456789012:role/OtherBatchRole"),
                ),
            ],
            input: input.clone(),
            output: output.clone(),
            same_submission: false,
        },
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    minted.insert(first.clone());
    for row in &rows {
        let case = row.case;
        let repeated = emulator
            .start_phi_job(
                case,
                &start_body(row.input.clone(), row.output.clone(), row.extra.clone()),
            )
            .await;
        if row.same_submission {
            assert_eq!(
                repeated, first,
                "case {case:?}: repeating one submission must name the job it created, got \
                 {repeated:?}"
            );
        } else {
            assert_ne!(
                repeated, first,
                "case {case:?}: a token carrying different parameters is another \
                 submission and must start its own job, got the first job back"
            );
            assert!(
                minted.insert(repeated.clone()),
                "case {case:?}: each submission must get a JobId of its own, got the \
                 already-minted {repeated:?}"
            );
            // The new job is independent: it carries its own request, and the first
            // job's record is untouched.
            let properties = emulator.describe_phi_job(case, &repeated).await;
            assert_eq!(
                properties["JobId"].as_str(),
                Some(repeated.as_str()),
                "case {case:?}: the new job must describe itself, got {properties}"
            );
            let first_again = emulator.describe_phi_job(case, &first).await;
            for member in [
                "JobName",
                "InputDataConfig",
                "OutputDataConfig",
                "KMSKey",
                "DataAccessRoleArn",
                "SubmitTime",
            ] {
                assert_eq!(
                    first_again[member], first_properties[member],
                    "case {case:?}: the first job's {member} must be untouched, expected \
                     {} got {}",
                    first_properties[member], first_again[member]
                );
            }
        }
    }
}

/// Stopping a PHI job: a job that has not finished takes the stop and ends `STOPPED`,
/// one that already finished has nothing left to stop, and a `JobId` the family holds no
/// job for is refused as a not-found resource.
#[tokio::test]
async fn stopping_a_phi_job_depends_only_on_whether_it_has_finished() {
    let emulator = Emulator::new();
    let case = "a stoppable job";
    emulator.create_bucket(case, "phi-stop").await;
    emulator
        .put_object(
            case,
            "phi-stop",
            "input/doc.txt",
            b"Patient Jane Doe, age 52, was seen today.",
        )
        .await;
    let body = start_body(
        json!({"S3Bucket": "phi-stop", "S3Key": "input/"}),
        json!({"S3Bucket": "phi-stop", "S3Key": "output/"}),
        vec![("JobName", json!("to-stop"))],
    );

    // A stop asked for while the job has not finished is accepted, and the job then ends
    // STOPPED; a stop that arrives after it finished is refused instead. Which of the two
    // a given attempt meets is up to the clock, so BOTH are asserted on their own terms
    // rather than on how fast this machine ran.
    let job_id = emulator.start_phi_job(case, &body).await;
    let (status, answer) = emulator
        .call("StopPHIDetectionJob", &json!({"JobId": job_id.clone()}))
        .await;
    if status == 200 {
        assert_eq!(
            answer["JobId"].as_str(),
            Some(job_id.as_str()),
            "case {case:?}: an accepted stop answers with the job it stopped, got {answer}"
        );
        let properties = emulator.poll_until_terminal(case, &job_id).await;
        assert_eq!(
            properties["JobStatus"].as_str(),
            Some("STOPPED"),
            "case {case:?}: a job whose stop was accepted must end STOPPED, got {properties}"
        );
    } else {
        assert_eq!(
            status, 400,
            "case {case:?}: a refused stop answers 400, got {status} {answer}"
        );
        assert_eq!(
            answer["__type"].as_str(),
            Some("InvalidRequestException"),
            "case {case:?}: a job that had finished is refused as an invalid request, got \
             {answer}"
        );
        let properties = emulator.describe_phi_job(case, &job_id).await;
        let reported = properties["JobStatus"].as_str().unwrap_or_default();
        assert!(
            TERMINAL_STATUSES.contains(&reported),
            "case {case:?}: a stop is only refused for a job that finished, got {properties}"
        );
    }

    // A job that has reached its terminal status has nothing left to stop.
    let finished = emulator.start_phi_job("a finished job", &body).await;
    let properties = emulator.poll_until_terminal("a finished job", &finished).await;
    assert_eq!(
        properties["JobStatus"].as_str(),
        Some("COMPLETED"),
        "case {:?}: the job must finish before the stop, got {properties}",
        "a finished job"
    );
    let (status, answer) = emulator
        .call("StopPHIDetectionJob", &json!({"JobId": finished}))
        .await;
    assert_eq!(
        status, 400,
        "case {:?}: stopping a finished job is refused, got {status} {answer}",
        "a finished job"
    );
    assert_eq!(
        answer["__type"].as_str(),
        Some("InvalidRequestException"),
        "case {:?}: expected InvalidRequestException, got {answer}",
        "a finished job"
    );
    assert_eq!(
        answer["Message"].as_str(),
        Some("Job is already in terminal state."),
        "case {:?}: expected the service's terminal-state sentence, got {answer}",
        "a finished job"
    );

    // An id this family holds no job for — including one another family minted.
    let other_family = {
        let (status, answer) = emulator
            .call(
                "StartEntitiesDetectionV2Job",
                &start_body(
                    json!({"S3Bucket": "phi-stop", "S3Key": "input/"}),
                    json!({"S3Bucket": "phi-stop", "S3Key": "output/"}),
                    vec![],
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "setup: starting the other family's job must answer 200, got {status} {answer}"
        );
        answer["JobId"].as_str().unwrap_or_default().to_string()
    };
    for (case, job_id) in [
        ("a job id nothing ever minted", "doesnotexist".to_string()),
        ("a job id at the modelled length maximum", "j".repeat(32)),
        ("a job id of another family", other_family),
    ] {
        let (status, answer) = emulator
            .call("StopPHIDetectionJob", &json!({"JobId": job_id}))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {answer}"
        );
        assert_eq!(
            answer["__type"].as_str(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {answer}"
        );
        assert_eq!(
            answer["Message"].as_str(),
            Some("Could not find specified job."),
            "case {case:?}: expected the service's fixed not-found sentence, got {answer}"
        );
    }
}
